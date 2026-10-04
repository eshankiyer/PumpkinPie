//! Shared framework for the horse family (`AbstractHorse` in vanilla): Horse, Donkey, Mule,
// Legacy invariant checks retained for vanilla behavior; migrate these paths before removing this allow.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! `ZombieHorse`, `SkeletonHorse`. See `designs/equine-framework.md` for the design rationale.
//!
//! Phase 1 scope: taming, feeding, chest inventory, breeding attribute inheritance,
//! `randomizeAttributes`, sounds, and server-side rider movement/jumping.
//!
//! Deliberate simplifications versus vanilla, noted here once rather than at every call site:
//! - Saddle/body-armor equipping is done directly in `abstract_horse_mob_interact` (mirroring
//!   `HappyGhastEntity::try_equip_harness`) rather than through a generic
//!   `ItemStack.interactLivingEntity` dispatch, which doesn't exist in Pumpkin yet.
//! - The chest inventory screen is opened via the existing generic-container screen handler
//!   (`create_generic_9x3`), not a ported `HorseInventoryMenu`/`ClientboundMountScreenOpenPacket`.
//!   Vanilla's real screen has dedicated saddle/armor slots plus the chest columns in one menu;
//!   here the saddle/armor stay equipment-slot-only (no slot in the chest GUI) and only the
//!   chest storage is shown. This is a real, working inventory (right-click+sneak on a tamed
//!   chested horse opens and persists chest contents) but is not pixel-identical to vanilla's
//!   menu layout -- flagged as a follow-up if the exact vanilla GUI is required.
//! - The shared `mob_tick` hook now handles the server-side grazing trigger and standing timer;
//!   species-specific hooks call it before their own extra per-tick behavior.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU8, Ordering::Relaxed};

use pumpkin_data::data_component_impl::{EquipmentSlot, EquippableImpl, IDSet, IdOr};
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::particle::Particle;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_data::{
    Block, BlockState,
    entity::{EntityStatus, EntityType},
    tag::Taggable,
    tracked_data,
};
use pumpkin_inventory::generic_container_screen_handler::create_generic_9x3;
use pumpkin_inventory::player::player_inventory::PlayerInventory;
use pumpkin_inventory::screen_handler::{
    BoxFuture, InventoryPlayer, ScreenHandlerFactory, SharedScreenHandler,
};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_protocol::bedrock::server::actor_event::ActorEventType;
use pumpkin_protocol::java::client::play::Metadata;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_util::text::TextComponent;
use pumpkin_world::inventory::{Inventory, SimpleInventory};
use rand::RngExt;
use tokio::sync::Mutex as TokioMutex;

pub(crate) fn is_valid_saddle_item(stack: &ItemStack, entity_type: &EntityType) -> bool {
    stack
        .get_data_component::<EquippableImpl>()
        .is_some_and(|equippable| {
            *equippable.slot == EquipmentSlot::SADDLE
                && equippable
                    .allowed_entities
                    .as_ref()
                    .is_none_or(|allowed| match allowed {
                        IDSet::Tag(tag) => entity_type.is_tagged_with(tag).unwrap_or(false),
                        IDSet::IDs(ids) => ids.iter().any(|allowed| allowed.id == entity_type.id),
                    })
        })
}

/// `AbstractHorse.equipBodyArmor` uses the animal-body equipment component and
/// its allowed-entity predicate. Keep this separate from the generic mob helper,
/// whose tag handling is intentionally more limited.
pub(crate) fn is_valid_body_armor_item(stack: &ItemStack, entity_type: &EntityType) -> bool {
    stack
        .get_data_component::<EquippableImpl>()
        .is_some_and(|equippable| {
            *equippable.slot == EquipmentSlot::BODY
                && equippable
                    .allowed_entities
                    .as_ref()
                    .is_none_or(|allowed| match allowed {
                        IDSet::Tag(tag) => entity_type.is_tagged_with(tag).unwrap_or(false),
                        IDSet::IDs(ids) => ids.iter().any(|allowed| allowed.id == entity_type.id),
                    })
        })
}

pub(crate) fn saddle_equip_on_interact(stack: &ItemStack, entity_type: &EntityType) -> bool {
    stack
        .get_data_component::<EquippableImpl>()
        .is_some_and(|equippable| {
            equippable.equip_on_interact && is_valid_saddle_item(stack, entity_type)
        })
}

pub(crate) async fn equip_saddle_item(
    mob_entity: &MobEntity,
    player: &Arc<Player>,
    item_stack: &mut ItemStack,
) {
    let equip_sound = item_stack
        .get_data_component::<EquippableImpl>()
        .map_or(IdOr::Id(Sound::ItemArmorEquipGeneric), |equippable| {
            equippable.equip_sound.clone()
        });
    let equip_sound = if matches!(
        mob_entity.living_entity.entity.entity_type.id,
        id if id == EntityType::HORSE.id
            || id == EntityType::DONKEY.id
            || id == EntityType::MULE.id
            || id == EntityType::SKELETON_HORSE.id
            || id == EntityType::ZOMBIE_HORSE.id
    ) {
        // `AbstractHorse.getEquipSound` (`AbstractHorse.java:315-317`) returns the horse saddle
        // sound for the saddle slot, even when the item's equippable component has another sound.
        IdOr::Id(Sound::EntityHorseSaddle)
    } else if mob_entity.living_entity.entity.entity_type.id == EntityType::CAMEL.id {
        // `Camel.getEquipSound` (`Camel.java:634-641`) likewise returns `CAMEL_SADDLE` for the
        // saddle slot.
        IdOr::Id(Sound::EntityCamelSaddle)
    } else if mob_entity.living_entity.entity.entity_type.id == EntityType::PIG.id {
        // `Pig.getEquipSound` (`Pig.java:194-197`).
        IdOr::Id(Sound::EntityPigSaddle)
    } else if mob_entity.living_entity.entity.entity_type.id == EntityType::STRIDER.id {
        // `Strider.getEquipSound` (`Strider.java:145-147`).
        IdOr::Id(Sound::EntityStriderSaddle)
    } else {
        equip_sound
    };
    let new_stack = item_stack.split_unless_creative(player.gamemode.load(), 1);
    {
        let mut equipment = mob_entity.living_entity.entity_equipment.lock().await;
        equipment.put(&EquipmentSlot::SADDLE, new_stack.clone());
    };
    mob_entity
        .living_entity
        .equipment_drop_chances
        .lock()
        .await
        // `Mob.setGuaranteedDrop` stores the preserved `2.0F` marker
        // (`Mob.java:601-603`, `DropChances.java:28-29`).
        .insert(EquipmentSlot::SADDLE, 2.0);
    mob_entity
        .living_entity
        .send_equipment_changes(&[(EquipmentSlot::SADDLE, new_stack)]);

    let entity = &mob_entity.living_entity.entity;
    let world = entity.world.load();
    world.play_sound_event(&equip_sound, SoundCategory::Neutral, &entity.pos.load());
}

pub(crate) async fn equip_body_armor_item(
    mob_entity: &MobEntity,
    player: &Arc<Player>,
    item_stack: &mut ItemStack,
) {
    let equip_sound = item_stack
        .get_data_component::<EquippableImpl>()
        .map_or(IdOr::Id(Sound::ItemArmorEquipGeneric), |equippable| {
            equippable.equip_sound.clone()
        });
    let new_stack = item_stack.split_unless_creative(player.gamemode.load(), 1);
    let mut equipment = mob_entity.living_entity.entity_equipment.lock().await;
    equipment.put(&EquipmentSlot::BODY, new_stack.clone());
    drop(equipment);
    mob_entity
        .living_entity
        .equipment_drop_chances
        .lock()
        .await
        // `Mob.setGuaranteedDrop` stores the preserved `2.0F` marker
        // (`Mob.java:601-603`, `DropChances.java:28-29`).
        .insert(EquipmentSlot::BODY, 2.0);
    mob_entity
        .living_entity
        .send_equipment_changes(&[(EquipmentSlot::BODY, new_stack)]);

    let entity = &mob_entity.living_entity.entity;
    let world = entity.world.load();
    world.play_sound_event(&equip_sound, SoundCategory::Neutral, &entity.pos.load());
}

pub(crate) async fn mount_player(mob_entity: &MobEntity, player: &Arc<Player>) {
    if !player.get_entity().can_start_riding().await {
        return;
    }

    let entity = &mob_entity.living_entity.entity;
    let world = entity.world.load();
    let Some(vehicle) = world.get_entity_by_id(entity.entity_id) else {
        return;
    };
    let Some(passenger) = world.get_player_by_id(player.entity_id()) else {
        return;
    };
    entity
        .add_passenger(vehicle, passenger as Arc<dyn EntityBase>)
        .await;
}

use crate::entity::{
    EntityBase, EntityBaseFuture,
    ai::goal::{Controls, Goal, GoalFuture, escape_danger::EscapeDangerGoal},
    mob::{Mob, MobEntity},
    passive::animal::Animal,
    player::{Player, advancement::trigger::AdvancementTrigger},
};

/// `ItemTags.HORSE_TEMPT_ITEMS` (`AbstractHorse.java:151`): golden carrot, golden apple,
/// enchanted golden apple.
///
/// `TemptGoal` only supports a static item list (not a tag predicate), so the tag is
/// inlined here. Shared by Horse, Donkey and Mule (all three inherit `AbstractHorse`'s
/// base tempt goal; SkeletonHorse/ZombieHorse override `addBehaviourGoals` with their own
/// food).
pub const HORSE_TEMPT_ITEMS: &[&Item] = &[
    &Item::GOLDEN_CARROT,
    &Item::GOLDEN_APPLE,
    &Item::ENCHANTED_GOLDEN_APPLE,
];

/// `AbstractHorse` bounds used by `setOffspringAttributes`.
pub const MIN_HEALTH: f64 = 15.0;
pub const MAX_HEALTH: f64 = 30.0;
pub const MIN_JUMP_STRENGTH: f64 = 0.4;
pub const MAX_JUMP_STRENGTH: f64 = 1.0;
pub const MIN_MOVEMENT_SPEED: f64 = 0.1125;
pub const MAX_MOVEMENT_SPEED: f64 = 0.3375;

/// `AbstractHorse.java` `DATA_ID_FLAGS` bits (`AbstractHorse.java:97-101`).
///
/// `FLAG_TAME` is stored in `AbstractHorseData::flags` only for a horse that is tame without an
/// owner (a skeleton trap horse or a `Tame:1b` entity with no `Owner`); a horse with an owner is
/// tame through `MobEntity::owner` (see `AbstractHorse::is_tamed`). The synched byte gets the
/// bit from `AbstractHorse::is_tamed` in `AbstractHorse::horse_flags_byte`.
pub const FLAG_TAME: u8 = 2;
pub const FLAG_BRED: u8 = 8;
pub const FLAG_EATING: u8 = 16;
pub const FLAG_STANDING: u8 = 32;
pub const FLAG_OPEN_MOUTH: u8 = 64;

/// Shared per-instance state mirroring vanilla `AbstractHorse`'s fields that aren't already
/// covered by `MobEntity` (owner/love-ticks) or `LivingEntity` (equipment for saddle/armor).
pub struct AbstractHorseData {
    pub flags: AtomicU8,
    pub temper: AtomicI32,
    pub eating_counter: AtomicI32,
    pub stand_counter: AtomicI32,
    /// Vanilla `AbstractHorse.mouthCounter` (`AbstractHorse.java:109`): ticks since `openMouth`,
    /// 0 while the mouth is closed.
    pub mouth_counter: AtomicI32,
    /// Vanilla `AbstractHorse.gallopSoundCounter` (`AbstractHorse.java:123-124`).
    pub gallop_sound_counter: AtomicI32,
    pub jump_pending_scale: AtomicI32,
    pub allow_stand_sliding: AtomicBool,
    /// The `DATA_ID_FLAGS` byte last published to clients. `SynchedEntityData.set` ignores an
    /// unchanged value (`SynchedEntityData.java:60-68`), so the sync helpers compare against
    /// this rather than resending on every per-tick `set_flag`.
    pub synced_flags: AtomicU8,
}

impl Default for AbstractHorseData {
    fn default() -> Self {
        Self {
            flags: AtomicU8::new(0),
            temper: AtomicI32::new(0),
            eating_counter: AtomicI32::new(0),
            stand_counter: AtomicI32::new(0),
            mouth_counter: AtomicI32::new(0),
            gallop_sound_counter: AtomicI32::new(0),
            jump_pending_scale: AtomicI32::new(0),
            allow_stand_sliding: AtomicBool::new(false),
            synced_flags: AtomicU8::new(0),
        }
    }
}

impl AbstractHorseData {
    #[must_use]
    pub fn get_flag(&self, flag: u8) -> bool {
        self.flags.load(Relaxed) & flag != 0
    }

    pub fn set_flag(&self, flag: u8, value: bool) {
        if value {
            self.flags.fetch_or(flag, Relaxed);
        } else {
            self.flags.fetch_and(!flag, Relaxed);
        }
    }
}

/// `AbstractHorse.MountPanicGoal.shouldPanic` (`AbstractHorse.java:1053-1057`) gates the live
/// `PanicGoal` on mob-controlled passengers.
///
/// The existing `EscapeDangerGoal` supplies the shared panic-causing damage/fire behavior and
/// is registered by each horse constructor.
pub struct MountPanicGoal<T: ?Sized> {
    horse: std::sync::Weak<T>,
    inner: EscapeDangerGoal,
}

#[must_use]
const fn mount_panic_allowed(is_mob_controlled: bool) -> bool {
    !is_mob_controlled
}

impl<T: AbstractHorse + Mob + ?Sized> MountPanicGoal<T> {
    #[must_use]
    pub fn new(horse: std::sync::Weak<T>, speed: f64) -> Box<Self> {
        Box::new(Self {
            horse,
            inner: *EscapeDangerGoal::new(speed),
        })
    }
}

impl<T: AbstractHorse + Mob + ?Sized + Send + Sync + 'static> Goal for MountPanicGoal<T> {
    fn is_panic_goal(&self) -> bool {
        true
    }

    fn can_start<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        Box::pin(async move {
            let Some(horse) = self.horse.upgrade() else {
                return false;
            };
            if !mount_panic_allowed(horse.is_mob_controlled().await) {
                return false;
            }
            self.inner.can_start(mob).await
        })
    }

    fn should_continue<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        Box::pin(async move { self.inner.should_continue(mob).await })
    }

    fn start<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async move { self.inner.start(mob).await })
    }

    fn stop<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async move { self.inner.stop(mob).await })
    }

    fn tick<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async move { self.inner.tick(mob).await })
    }

    fn controls(&self) -> Controls {
        self.inner.controls()
    }
}

/// Chest-capable species' extra state (`AbstractChestedHorse.java`).
pub struct ChestedHorseData {
    pub has_chest: std::sync::atomic::AtomicBool,
    pub inventory: TokioMutex<Arc<SimpleInventory>>,
    /// The `DATA_ID_CHEST` value last published to clients (`AbstractChestedHorse.java:32,49-53`).
    pub synced_chest: AtomicBool,
}

impl Default for ChestedHorseData {
    fn default() -> Self {
        Self {
            has_chest: std::sync::atomic::AtomicBool::new(false),
            inventory: TokioMutex::new(Arc::new(SimpleInventory::new(0))),
            synced_chest: AtomicBool::new(false),
        }
    }
}

// --- Pure attribute-roll helpers: `AbstractHorse.java:938-947,841-863` ---

/// `AbstractHorse.generateMaxHealth`: `15 + rnd(8) + rnd(9)`.
#[must_use]
pub fn generate_max_health(random: &mut impl RngExt) -> f64 {
    15.0 + f64::from(random.random_range(0..8)) + f64::from(random.random_range(0..9))
}

/// `AbstractHorse.generateJumpStrength`: `0.4 + 3x rnd(0..1)*0.2`.
#[must_use]
pub fn generate_jump_strength(random: &mut impl RngExt) -> f64 {
    0.4 + random.random_range(0.0..1.0) * 0.2
        + random.random_range(0.0..1.0) * 0.2
        + random.random_range(0.0..1.0) * 0.2
}

/// `AbstractHorse.generateSpeed`: `(0.45 + 3x rnd(0..1)*0.3) * 0.25`.
#[must_use]
pub fn generate_speed(random: &mut impl RngExt) -> f64 {
    (0.45
        + random.random_range(0.0..1.0) * 0.3
        + random.random_range(0.0..1.0) * 0.3
        + random.random_range(0.0..1.0) * 0.3)
        * 0.25
}

/// `ZombieHorse.generateZombieHorseJumpStrength`.
#[must_use]
pub fn generate_zombie_horse_jump_strength(random: &mut impl RngExt) -> f64 {
    0.5 + random.random_range(0.0..1.0) / 15.0
        + random.random_range(0.0..1.0) / 15.0
        + random.random_range(0.0..1.0) / 15.0
}

/// `ZombieHorse.generateZombieHorseSpeed`.
#[must_use]
pub fn generate_zombie_horse_speed(random: &mut impl RngExt) -> f64 {
    (9.0 + random.random_range(0.0..1.0)
        + random.random_range(0.0..1.0)
        + random.random_range(0.0..1.0))
        / 42.16
}

/// `AbstractHorse.createOffspringAttribute`.
#[must_use]
pub fn create_offspring_attribute(
    parent_a: f64,
    parent_b: f64,
    range_min: f64,
    range_max: f64,
    random: &mut impl RngExt,
) -> f64 {
    assert!(range_max > range_min, "Incorrect range for an attribute");

    let a = parent_a.clamp(range_min, range_max);
    let b = parent_b.clamp(range_min, range_max);
    let margin = 0.15 * (range_max - range_min);
    let range = (a - b).abs() + margin * 2.0;
    let average = f64::midpoint(a, b);
    let quality = (random.random_range(0.0..1.0)
        + random.random_range(0.0..1.0)
        + random.random_range(0.0..1.0))
        / 3.0
        - 0.5;
    let value = average + range * quality;

    if value > range_max {
        range_max - (value - range_max)
    } else if value < range_min {
        range_min + (range_min - value)
    } else {
        value
    }
}

/// `PlayerRideableJumping.getPlayerJumpPendingScale` (`PlayerRideableJumping.java:12-16`).
#[must_use]
fn player_jump_pending_scale(jump_amount: i32) -> i32 {
    if jump_amount >= 90 {
        1000
    } else {
        400 + (400 * jump_amount.max(0) / 90)
    }
}

/// The synched `DATA_ID_FLAGS` byte (`AbstractHorse.java:96-101`) from the stored bred, eating,
/// standing and open-mouth bits and the owner-derived tame bit.
#[must_use]
const fn synced_flags_byte(stored: u8, tamed: bool) -> u8 {
    let stored = stored & (FLAG_BRED | FLAG_EATING | FLAG_STANDING | FLAG_OPEN_MOUTH);
    if tamed { stored | FLAG_TAME } else { stored }
}

/// `AbstractHorse.tick`'s `mouthCounter` step (`AbstractHorse.java:579-582`): `++mouthCounter`
/// while it is running, resetting once it passes 30. Returns the next counter and whether the
/// open-mouth flag clears.
#[must_use]
const fn next_mouth_counter(counter: i32) -> (i32, bool) {
    if counter <= 0 {
        return (counter, false);
    }
    let next = counter + 1;
    if next > 30 { (0, true) } else { (next, false) }
}

/// What one ridden footstep of a gallop-capable horse plays (`AbstractHorse.java:350-356`).
#[derive(Debug, PartialEq, Eq)]
enum GallopStep {
    /// `playGallopSound`.
    Gallop,
    /// `HORSE_STEP_WOOD`, for the first five steps.
    Step,
    /// Neither: `gallopSoundCounter > 5` on the two steps between gallops.
    Silent,
}

/// Picks the sound of a ridden step from `gallopSoundCounter` after its increment
/// (`AbstractHorse.java:351-356`).
const fn gallop_step(counter: i32) -> GallopStep {
    if counter > 5 && counter % 3 == 0 {
        GallopStep::Gallop
    } else if counter <= 5 {
        GallopStep::Step
    } else {
        GallopStep::Silent
    }
}

/// `AbstractHorse.isWoodSoundType` (`AbstractHorse.java:365-371`): `WOOD`, `NETHER_WOOD`, `STEM`,
/// `CHERRY_WOOD` and `BAMBOO_WOOD`, told apart by the step sound `block_sound_type` reports.
const fn is_wood_step_sound(step: Sound) -> bool {
    matches!(
        step,
        Sound::BlockWoodStep
            | Sound::BlockNetherWoodStep
            | Sound::BlockStemStep
            | Sound::BlockCherryWoodStep
            | Sound::BlockBambooWoodStep
    )
}

/// `AbstractHorse.playGallopSound` (`AbstractHorse.java:373-375`) as `(sound, volume, pitch)`,
/// given the block sound type's volume and pitch.
#[must_use]
pub const fn gallop_sound(block_volume: f32, block_pitch: f32) -> (Sound, f32, f32) {
    (Sound::EntityHorseGallop, block_volume * 0.15, block_pitch)
}

struct HorseChestScreenFactory(Arc<dyn Inventory>);

impl ScreenHandlerFactory for HorseChestScreenFactory {
    fn create_screen_handler<'a>(
        &'a self,
        sync_id: u8,
        player_inventory: &'a Arc<PlayerInventory>,
        _player: &'a dyn InventoryPlayer,
    ) -> BoxFuture<'a, Option<SharedScreenHandler>> {
        Box::pin(async move {
            let handler = create_generic_9x3(sync_id, player_inventory, self.0.clone()).await;
            Some(Arc::new(TokioMutex::new(handler)) as SharedScreenHandler)
        })
    }

    fn get_display_name(&self) -> TextComponent {
        TextComponent::text("Horse")
    }
}

/// Mirrors vanilla `AbstractHorse` (Horse.java's superclass). Species implement this to get
/// taming, feeding, saddling, breeding-attribute-inheritance and `randomizeAttributes` for free.
///
/// Deliberately `Animal`-bound (not just `Mob`) since vanilla's `AbstractHorse` itself extends
/// `Animal`; this also means the required `Animal::is_food` impl species already carry for the
/// generic baby-growth path doubles as the horse-specific food-tag check here.
pub trait AbstractHorse: Animal {
    fn horse_data(&self) -> &AbstractHorseData;

    /// `AbstractHorse.isBred` (`AbstractHorse.java:215-220`).
    fn is_bred(&self) -> bool {
        self.horse_data().get_flag(FLAG_BRED)
    }

    fn max_temper(&self) -> i32 {
        100
    }

    /// `AbstractHorse.getTemper`.
    fn get_temper(&self) -> i32 {
        self.horse_data().temper.load(Relaxed)
    }

    /// `AbstractHorse.modifyTemper` (`AbstractHorse.java:247-250`).
    fn modify_temper(&self, amount: i32) -> i32 {
        let temper = (self.get_temper() + amount).clamp(0, self.max_temper());
        self.horse_data().temper.store(temper, Relaxed);
        temper
    }

    /// `AbstractHorse.isMobControlled`, default `false`. `ZombieHorseEntity` is the only
    /// current override (a non-player mob riding counts as "in control").
    fn is_mob_controlled(&self) -> BoxFuture<'_, bool> {
        Box::pin(async { false })
    }

    fn can_perform_rearing(&self) -> bool {
        true
    }

    /// `AbstractHorse.canEatGrass`; llamas override this to `false`.
    fn can_eat_grass(&self) -> bool {
        true
    }

    /// Server-side portion of `AbstractHorse.aiStep` and `tick`: expire the open mouth and the
    /// 20-tick standing pose, heal slowly, and start haystack eating beneath the horse.
    ///
    /// Species call this from `Mob::post_tick`, not `mob_tick`: neither vanilla method is gated
    /// on `NoAI` (only `Mob.serverAiStep` is), whereas `mob_tick` is skipped for a `NoAI` mob, and
    /// a `NoAI` horse that reared or ate would keep the pose and open mouth forever.
    fn tick_horse_ai(&self) -> EntityBaseFuture<'_, ()> {
        Box::pin(async move {
            let data = self.horse_data();
            // `AbstractHorse.tick` (`AbstractHorse.java:579-582`).
            let previous_mouth = data
                .mouth_counter
                .fetch_update(Relaxed, Relaxed, |counter| {
                    Some(next_mouth_counter(counter).0)
                })
                .unwrap_or_default();
            if next_mouth_counter(previous_mouth).1 {
                self.set_horse_flag(FLAG_OPEN_MOUTH, false);
            }
            if data.stand_counter.load(Relaxed) > 0 && data.stand_counter.fetch_sub(1, Relaxed) <= 1
            {
                self.clear_standing();
            }
            // `AbstractHorse.tick` (`AbstractHorse.java:620-622`): the rider's steering freeze
            // while rearing is only lifted for the pose a jump started.
            if !data.get_flag(FLAG_STANDING) {
                data.allow_stand_sliding.store(false, Relaxed);
            }

            // The rest of `AbstractHorse.aiStep` only runs for a living horse
            // (`AbstractHorse.java:538`); `LivingEntity.isAlive` is `!isRemoved() && health > 0`,
            // so a horse in its death animation neither heals nor grazes.
            let entity = self.get_entity();
            if !entity.is_alive() || self.get_mob_entity().living_entity.is_dead_or_dying() {
                return;
            }

            // Natural regeneration (`AbstractHorse.java:539-541`); `deathTime == 0` holds for
            // any horse that is still alive. `heal` clamps to full health, so skipping it there
            // only spares a regain-health event for a no-op.
            if self.get_random().random_range(0..900) == 0 {
                let living = &self.get_mob_entity().living_entity;
                if living.health.load() < living.get_max_health() {
                    living.heal(1.0);
                }
            }

            let is_vehicle = !entity.passengers.lock().await.is_empty();
            if self.can_eat_grass()
                && !data.get_flag(FLAG_EATING)
                && !is_vehicle
                && self.get_random().random_range(0..300) == 0
                && entity
                    .world
                    .load()
                    .get_block(&entity.block_pos.load().down())
                    .id
                    == Block::GRASS_BLOCK.id
            {
                self.set_horse_flag(FLAG_EATING, true);
            }

            if data.get_flag(FLAG_EATING) && data.eating_counter.fetch_add(1, Relaxed) + 1 > 50 {
                data.eating_counter.store(0, Relaxed);
                self.set_horse_flag(FLAG_EATING, false);
            }
        })
    }

    /// The synched `DATA_ID_FLAGS` byte (`AbstractHorse.java:96-101`): the stored bred, eating,
    /// standing and open-mouth bits plus `FLAG_TAME`, which is derived from the owner.
    fn horse_flags_byte(&self) -> u8 {
        synced_flags_byte(self.horse_data().flags.load(Relaxed), self.is_tamed())
    }

    fn send_horse_flags_byte(&self, byte: u8) {
        self.get_entity().send_meta_data(
            &[Metadata::new(
                tracked_data::abstract_horse::DATA_ID_FLAGS,
                byte as i8,
            )],
            None,
        );
    }

    /// Publishes `DATA_ID_FLAGS` when it differs from what clients already have, as
    /// `SynchedEntityData.set` does for every `setFlag` (`AbstractHorse.java:163-170`).
    fn sync_horse_flags(&self) {
        let byte = self.horse_flags_byte();
        if self.horse_data().synced_flags.swap(byte, Relaxed) != byte {
            self.send_horse_flags_byte(byte);
        }
    }

    /// `AbstractHorse.setFlag`: stores the bit and syncs it.
    fn set_horse_flag(&self, flag: u8, value: bool) {
        self.horse_data().set_flag(flag, value);
        self.sync_horse_flags();
    }

    /// The `mob_init_data_tracker` half of `AbstractHorse.defineSynchedData`
    /// (`AbstractHorse.java:154-157`): the client default is 0, so only a non-zero byte -- a
    /// tamed horse, or one loaded with NBT flags -- has to be sent.
    fn send_initial_horse_flags(&self) {
        let byte = self.horse_flags_byte();
        self.horse_data().synced_flags.store(byte, Relaxed);
        if byte != 0 {
            self.send_horse_flags_byte(byte);
        }
    }

    /// What a species' `mob_init_data_tracker` publishes for an `AbstractHorse`: the
    /// `AgeableMob` baby flag, which overriding the `Mob` default would otherwise drop, and the
    /// horse flags.
    fn send_horse_init_metadata(&self) {
        let entity = self.get_entity();
        if entity.age.load(Relaxed) < 0 {
            entity.send_meta_data(
                &[Metadata::new(tracked_data::ageable_mob::DATA_BABY_ID, true)],
                None,
            );
        }
        self.send_initial_horse_flags();
    }

    /// `AbstractHorse.isImmobile`: `super.isImmobile() && isVehicle() && isSaddled() ||
    /// isEating() || isStanding()`. The `super.isImmobile()` (dead-or-dying) clause combined
    /// with the ridden-and-saddled check is omitted here as a rare edge case; eating/standing
    /// are the two states `RandomStandGoal` actually cares about.
    fn is_immobile(&self) -> bool {
        let data = self.horse_data();
        data.get_flag(FLAG_EATING) || data.get_flag(FLAG_STANDING)
    }

    fn can_fall_in_love(&self) -> bool {
        true
    }

    fn eating_sound(&self) -> Option<Sound> {
        None
    }

    /// `AbstractHorse.openMouth` (`AbstractHorse.java:670-675`).
    fn open_mouth(&self) {
        self.horse_data().mouth_counter.store(1, Relaxed);
        self.set_horse_flag(FLAG_OPEN_MOUTH, true);
    }

    /// `AbstractHorse.eating` (`AbstractHorse.java:258-276`): opens the mouth and plays the
    /// species' eating sound at `1.0` volume with a `1.0 +- 0.2` pitch spread.
    fn eating(&self) {
        self.open_mouth();
        let entity = self.get_entity();
        if !entity.is_silent()
            && let Some(sound) = self.eating_sound()
        {
            let mut random = self.get_random();
            let pitch = 1.0 + (random.random::<f32>() - random.random::<f32>()) * 0.2;
            entity.world.load().play_sound_fine(
                sound,
                self.get_sound_source(),
                &entity.pos.load(),
                1.0,
                pitch,
            );
        }
    }

    /// `AbstractHorse.canGallop` (`AbstractHorse.java:123`); `AbstractChestedHorse` clears it
    /// (`AbstractChestedHorse.java:38`).
    fn can_gallop(&self) -> bool {
        true
    }

    /// `AbstractHorse.playGallopSound` (`AbstractHorse.java:373-375`) as `(sound, volume, pitch)`
    /// entries; `Horse` adds its breathing on top (`Horse.java:123-128`).
    fn gallop_sounds(&self, block_volume: f32, block_pitch: f32) -> Vec<(Sound, f32, f32)> {
        vec![gallop_sound(block_volume, block_pitch)]
    }

    /// `AbstractHorse.playStepSound` (`AbstractHorse.java:341-363`) as `(sound, volume, pitch)`
    /// entries, empty when the step is silent.
    ///
    /// `block_sound_type` supplies the `SoundType` volume and pitch; the snow layer above the
    /// supporting block takes over its sound type, as in vanilla. Whether the horse is a
    /// vehicle is read with `try_lock` (the hook is not async), so a contended lock counts as
    /// unridden for that one step.
    fn horse_step_sounds(
        &self,
        supporting_block: &Block,
        supporting_state: &BlockState,
        above_block: &Block,
    ) -> Vec<(Sound, f32, f32)> {
        if supporting_state.is_liquid() {
            return Vec::new();
        }
        let sound_block = if above_block.id == Block::SNOW.id {
            above_block
        } else {
            supporting_block
        };
        let (step_sound, _, block_volume, block_pitch) =
            crate::block::block_sound_type(sound_block);
        let step_volume = block_volume * 0.15;

        let entity = self.get_entity();
        let is_vehicle = entity
            .passengers
            .try_lock()
            .is_ok_and(|passengers| !passengers.is_empty());
        if is_vehicle && self.can_gallop() {
            let counter = self.horse_data().gallop_sound_counter.fetch_add(1, Relaxed) + 1;
            match gallop_step(counter) {
                GallopStep::Gallop => self.gallop_sounds(block_volume, block_pitch),
                GallopStep::Step => {
                    vec![(Sound::EntityHorseStepWood, step_volume, block_pitch)]
                }
                GallopStep::Silent => Vec::new(),
            }
        } else if is_wood_step_sound(step_sound) {
            vec![(Sound::EntityHorseStepWood, step_volume, block_pitch)]
        } else if self.is_baby() {
            vec![(Sound::EntityBabyHorseStep, step_volume, block_pitch)]
        } else {
            vec![(Sound::EntityHorseStep, step_volume, block_pitch)]
        }
    }

    fn angry_sound(&self) -> Option<Sound> {
        None
    }

    fn saddle_sound(&self) -> Sound {
        Sound::EntityHorseSaddle
    }

    /// Plays the mount jump sound from `AbstractHorse.handleStartJump`.
    ///
    /// Vanilla: `AbstractHorse.java:897-901` calls `playJumpSound`; the base implementation is
    /// `AbstractHorse.java:783-785`. The server command path calls this from `handle_start_jump`.
    fn play_jump_sound(&self) {
        let entity = self.get_entity();
        let world = entity.world.load();
        world.play_sound_fine(
            Sound::EntityHorseJump,
            SoundCategory::Neutral,
            &entity.pos.load(),
            0.4,
            1.0,
        );
    }

    /// `randomizeAttributes` -- a no-op default; species override and call this from their
    /// `new()` (before any NBT read overwrites the rolled base values on load, see
    /// `write_horse_attributes_nbt`/`read_horse_attributes_nbt`).
    fn randomize_attributes(&self, _random: &mut impl RngExt)
    where
        Self: Sized,
    {
    }

    fn is_baby(&self) -> bool {
        self.get_entity().age.load(Relaxed) < 0
    }

    /// `AbstractHorse.isTamed` (`AbstractHorse.java:169-171`). Vanilla's tame flag is
    /// independent of its owner reference (`SkeletonTrapGoal` tames with no owner), so a horse is
    /// tame with an owner (`MobEntity::owner`) or with the stored `FLAG_TAME` bit.
    fn is_tamed(&self) -> bool {
        self.get_mob_entity().is_tamed() || self.horse_data().get_flag(FLAG_TAME)
    }

    /// `AbstractHorse.setTamed(true)` with no owner, as `SkeletonTrapGoal.tick` does
    /// (`SkeletonTrapGoal.java:36`).
    fn set_tamed_ownerless(&self) {
        self.set_horse_flag(FLAG_TAME, true);
    }

    /// `AbstractHorse.tameWithName` (`AbstractHorse.java:709-718`): the tamer becomes the owner,
    /// the synched tame bit goes out, `TAME_ANIMAL` fires, and clients spawn the heart particles
    /// from entity event 7 (`handleEntityEvent`, `AbstractHorse.java:919-921`).
    fn set_tamed<'a>(&'a self, player: &'a Player) -> EntityBaseFuture<'a, ()> {
        Box::pin(async move {
            self.get_mob_entity().set_owner(player.gameprofile.id);
            self.sync_horse_flags();
            player
                .trigger_advancement(AdvancementTrigger::TamedAnimal)
                .await;
            let entity = self.get_entity();
            entity.world.load().send_entity_status(
                entity,
                EntityStatus::TamingSucceeded,
                Some(ActorEventType::TamingSucceeded),
            );
        })
    }

    fn can_use_saddle_slot(&self) -> bool {
        self.get_entity().is_alive() && !self.is_baby() && self.is_tamed()
    }

    fn is_saddled(&self) -> BoxFuture<'_, bool> {
        Box::pin(async move {
            let equipment = self
                .get_mob_entity()
                .living_entity
                .entity_equipment
                .lock()
                .await;
            let stack = equipment.get(&EquipmentSlot::SADDLE);
            self.can_use_saddle_slot()
                && is_valid_saddle_item(&stack, self.get_entity().entity_type)
        })
    }

    /// Vanilla `AbstractHorse.getControllingPassenger`: the first player controls a
    /// saddled horse-family mob, regardless of the player's held item.
    fn has_saddled_player_passenger(&self) -> EntityBaseFuture<'_, bool> {
        Box::pin(async move {
            if !AbstractHorse::is_saddled(self).await {
                return self.default_has_controlling_passenger().await;
            }
            let passenger = self.get_entity().passengers.lock().await.first().cloned();
            if passenger.is_some_and(|passenger| passenger.get_player().is_some()) {
                return true;
            }
            self.default_has_controlling_passenger().await
        })
    }

    fn equip_saddle<'a>(
        &'a self,
        player: &'a Arc<Player>,
        item_stack: &'a mut ItemStack,
    ) -> EntityBaseFuture<'a, ()> {
        Box::pin(async move {
            equip_saddle_item(self.get_mob_entity(), player, item_stack).await;
        })
    }

    /// `AbstractHorse.makeMad`.
    fn make_mad(&self) {
        let data = self.horse_data();
        if !data.get_flag(FLAG_STANDING) {
            self.stand_if_possible();
            if let Some(sound) = self.angry_sound() {
                let entity = self.get_entity();
                let world = entity.world.load();
                world.play_sound(sound, SoundCategory::Neutral, &entity.pos.load());
            }
        }
    }

    /// `AbstractHorse.setStanding`/`standIfPossible`.
    fn stand_if_possible(&self) {
        if self.can_perform_rearing() {
            self.horse_data().set_flag(FLAG_EATING, false);
            self.horse_data().set_flag(FLAG_STANDING, true);
            self.horse_data().stand_counter.store(20, Relaxed);
            self.sync_horse_flags();
        }
    }

    /// `AbstractHorse.hurtServer` (`AbstractHorse.java:319-327`): a hit that landed makes the
    /// horse rear one time in three. Called from the species' `Mob::on_damage`, which only runs
    /// for accepted damage.
    fn horse_on_damage(&self) {
        if self.get_random().random_range(0..3) == 0 {
            self.stand_if_possible();
        }
    }

    /// `AbstractHorse.clearStanding`.
    fn clear_standing(&self) {
        self.horse_data().set_flag(FLAG_STANDING, false);
        self.horse_data().stand_counter.store(0, Relaxed);
        self.sync_horse_flags();
    }

    /// `AbstractHorse.onElasticLeashPull` (`AbstractHorse.java:189-195`) stops grazing when
    /// the shared leash solver applies an elastic pull.
    fn on_elastic_leash_pull(&self) {
        self.default_on_elastic_leash_pull();
        self.set_horse_flag(FLAG_EATING, false);
    }

    /// `AbstractHorse.supportQuadLeash`/`getQuadLeashOffsets` (`AbstractHorse.java:197-205`).
    /// The offsets are retained in the horse abstraction for the leash packet path to consume
    /// when quad-leash rendering is exposed.
    fn support_quad_leash(&self) -> bool {
        true
    }

    fn get_quad_leash_offsets(&self) -> [Vector3<f64>; 4] {
        let width = f64::from(self.get_entity().width());
        let height = f64::from(self.get_entity().height());
        let front_offset = 0.04 * width;
        let front_back = 0.52 * width;
        let left_right = 0.23 * width;
        let y = 0.87 * height;
        [
            Vector3::new(-left_right, y, front_back + front_offset),
            Vector3::new(-left_right, y, -front_back + front_offset),
            Vector3::new(left_right, y, -front_back + front_offset),
            Vector3::new(left_right, y, front_back + front_offset),
        ]
    }

    /// `AbstractHorse.getRiddenInput` (`AbstractHorse.java:751-764`) translated from the
    /// server's `SPlayerInput` flags. The horse's standing gate is kept before movement input is
    /// applied, matching vanilla's zero input while rearing.
    fn get_ridden_input(&self, input: i8) -> Vector3<f64> {
        let data = self.horse_data();
        if self.get_entity().on_ground.load(Relaxed)
            && data.jump_pending_scale.load(Relaxed) == 0
            && data.get_flag(FLAG_STANDING)
            && !data.allow_stand_sliding.load(Relaxed)
        {
            return Vector3::default();
        }

        let sideways = if input & pumpkin_protocol::java::server::play::SPlayerInput::LEFT != 0 {
            0.5
        } else if input & pumpkin_protocol::java::server::play::SPlayerInput::RIGHT != 0 {
            -0.5
        } else {
            0.0
        };
        let mut forward =
            if input & pumpkin_protocol::java::server::play::SPlayerInput::FORWARD != 0 {
                1.0
            } else if input & pumpkin_protocol::java::server::play::SPlayerInput::BACKWARD != 0 {
                -1.0
            } else {
                0.0
            };
        if forward <= 0.0 {
            forward *= 0.25;
        }
        Vector3::new(sideways, 0.0, forward)
    }

    /// `AbstractHorse.getRiddenRotation`/`getRiddenSpeed` (`AbstractHorse.java:741-743,766-769`).
    fn get_ridden_rotation(&self, rider: &Player) -> (f32, f32) {
        (
            rider.get_entity().yaw.load(),
            rider.get_entity().pitch.load() * 0.5,
        )
    }

    fn get_ridden_speed(&self) -> f64 {
        self.get_mob_entity()
            .living_entity
            .get_attribute_value(&pumpkin_data::attributes::Attributes::MOVEMENT_SPEED)
    }

    /// `PlayerRideableJumping.getPlayerJumpPendingScale` and
    /// `AbstractHorse.onPlayerJump` (`PlayerRideableJumping.java:12-16`; `AbstractHorse.java:878-895`).
    fn on_player_jump(&self, jump_amount: i32) {
        let pending = player_jump_pending_scale(jump_amount);
        self.horse_data().allow_stand_sliding.store(true, Relaxed);
        self.stand_if_possible();
        self.horse_data().jump_pending_scale.store(pending, Relaxed);
    }

    fn can_jump_now(&self) -> EntityBaseFuture<'_, bool> {
        Box::pin(async move { AbstractHorse::is_saddled(self).await })
    }

    /// `AbstractHorse.handleStartJump` (`AbstractHorse.java:897-901`) starts the rearing
    /// animation and emits the jump sound; movement consumes the stored scale in
    /// `tick_ridden` below.
    fn handle_start_jump(&self, _jump_scale: i32) {
        self.horse_data().allow_stand_sliding.store(true, Relaxed);
        self.stand_if_possible();
        self.play_jump_sound();
    }

    /// `AbstractHorse.handleStopJump` (`AbstractHorse.java:904-905`) has no server-side action.
    fn handle_stop_jump(&self) {}

    /// `AbstractHorse.executeRidersJump` (`AbstractHorse.java:771-780`). The impulse is
    /// `getJumpPower(amount)` (`LivingEntity.java:2371-2373`): the jump-strength attribute scaled
    /// by the block jump factor, plus the Jump Boost bonus.
    fn execute_riders_jump(
        &self,
        scale_thousandths: i32,
        input: Vector3<f64>,
    ) -> EntityBaseFuture<'_, ()> {
        Box::pin(async move {
            let amount = f64::from(scale_thousandths) / 1000.0;
            let impulse = self
                .get_mob_entity()
                .living_entity
                .get_jump_velocity(amount)
                .await;
            let entity = self.get_entity();
            let mut velocity = entity.velocity.load();
            velocity.y = impulse;
            if input.z > 0.0 {
                let yaw = f64::from(entity.yaw.load()).to_radians();
                velocity.x += -0.4 * yaw.sin() * amount;
                velocity.z += 0.4 * yaw.cos() * amount;
            }
            entity.set_velocity(velocity);
        })
    }

    /// `AbstractHorse.tickRidden` (`AbstractHorse.java:720-738`) and the existing
    /// `Mob::custom_travel` hook provide the server-side ridden travel path for all horse types.
    fn custom_travel<'a>(&'a self, caller: &'a Arc<dyn EntityBase>) -> EntityBaseFuture<'a, bool> {
        Box::pin(async move {
            let entity = self.get_entity();
            let first_passenger = {
                let passengers = entity.passengers.lock().await;
                passengers.first().cloned()
            };
            let Some(passenger) = first_passenger else {
                return false;
            };
            let Some(player) = passenger.get_player() else {
                return false;
            };
            if !AbstractHorse::is_saddled(self).await {
                return false;
            }

            let (yaw, pitch) = self.get_ridden_rotation(player);
            entity.set_rotation(yaw, pitch);
            entity.head_yaw.store(yaw);
            entity.body_yaw.store(yaw);

            let input_flags = player.last_input.load(Relaxed);
            let input = self.get_ridden_input(input_flags);
            // Vanilla `AbstractHorse.tickRidden` (`AbstractHorse.java:726-729`) resets the
            // gallop counter whenever the rider is not moving forward.
            if input.z <= 0.0 {
                self.horse_data().gallop_sound_counter.store(0, Relaxed);
            }
            // Only a grounded horse consumes the pending jump (`AbstractHorse.java:731-737`);
            // a jump requested mid-air is kept until the landing tick.
            if entity.on_ground.load(Relaxed) {
                let pending = self.horse_data().jump_pending_scale.swap(0, Relaxed);
                if pending > 0 && !self.get_mob_entity().living_entity.jumping.load(Relaxed) {
                    self.execute_riders_jump(pending, input).await;
                }
            }

            entity.update_velocity_from_input(input, self.get_ridden_speed());
            let mut velocity = entity.velocity.load();
            if !entity.on_ground.load(Relaxed) {
                velocity.y -= self.get_mob_gravity();
            }
            entity.move_entity(caller, velocity).await;
            let friction = if entity.on_ground.load(Relaxed) {
                f64::from(entity.get_block_with_y_offset(0.500_001).1.slipperiness) * 0.91
            } else {
                0.91
            };
            velocity = entity.velocity.load();
            velocity.x *= friction;
            velocity.z *= friction;
            velocity.y *= 0.98;
            entity.velocity.store(velocity);
            true
        })
    }

    /// `AbstractHorse.doPlayerRide`, using `Entity::add_passenger` the same way
    /// `HappyGhastEntity::try_mount` does (see that file for the established mounting pattern).
    fn do_player_ride<'a>(&'a self, player: &'a Arc<Player>) -> EntityBaseFuture<'a, ()> {
        Box::pin(async move {
            self.horse_data().set_flag(FLAG_EATING, false);
            self.clear_standing();

            if !player.get_entity().can_start_riding().await {
                return;
            }

            let entity = self.get_entity();
            let world = entity.world.load();
            let Some(passenger) = world.get_player_by_id(player.entity_id()) else {
                return;
            };
            let Some(vehicle) = world.get_entity_by_id(entity.entity_id) else {
                return;
            };
            entity
                .add_passenger(vehicle, passenger as Arc<dyn EntityBase>)
                .await;
        })
    }

    /// `AbstractHorse.handleEating`: the default wheat/sugar/hay/apple/mushroom/carrot/golden
    /// carrot/golden apple table (`AbstractHorse.java:423-493`). Species with a different food
    /// table (Llama, out of scope here) override this instead of extending it.
    fn handle_eating<'a>(
        &'a self,
        player: &'a Arc<Player>,
        item_stack: &'a ItemStack,
    ) -> EntityBaseFuture<'a, bool> {
        Box::pin(async move {
            let id = item_stack.item.id;
            let (heal, age_up_seconds, temper): (f32, i32, i32) = if id == Item::WHEAT.id {
                (2.0, 20, 3)
            } else if id == Item::SUGAR.id {
                (1.0, 30, 3)
            } else if id == Item::HAY_BLOCK.id {
                (20.0, 180, 0)
            } else if id == Item::APPLE.id {
                (3.0, 60, 3)
            } else if id == Item::RED_MUSHROOM.id {
                (3.0, 0, 3)
            } else if id == Item::CARROT.id {
                (3.0, 60, 3)
            } else if id == Item::GOLDEN_CARROT.id {
                (4.0, 60, 5)
            } else if id == Item::GOLDEN_APPLE.id || id == Item::ENCHANTED_GOLDEN_APPLE.id {
                (10.0, 240, 10)
            } else {
                return false;
            };

            let mut item_used = false;
            let is_golden = id == Item::GOLDEN_CARROT.id
                || id == Item::GOLDEN_APPLE.id
                || id == Item::ENCHANTED_GOLDEN_APPLE.id;

            let mob_entity = self.get_mob_entity();
            if is_golden
                && self.can_fall_in_love()
                && self.is_tamed()
                && !self.is_baby()
                && !mob_entity.is_in_love()
            {
                item_used = true;
                mob_entity.set_love_ticks(600, Some(player.gameprofile.id));
                let entity = &mob_entity.living_entity.entity;
                entity.world.load().send_entity_status(
                    entity,
                    pumpkin_data::entity::EntityStatus::InLoveHearts,
                    Some(pumpkin_protocol::bedrock::server::actor_event::ActorEventType::InLoveHearts),
                );
            }

            let living = &mob_entity.living_entity;
            if living.health.load() < living.get_max_health() && heal > 0.0 {
                living.heal(heal);
                item_used = true;
            }

            if self.is_baby() && age_up_seconds > 0 {
                let entity = self.get_entity();
                let world = entity.world.load();
                let pos = entity.pos.load();
                world.spawn_particle(
                    pos + Vector3::new(0.0, f64::from(entity.height()) * 0.5, 0.0),
                    Vector3::new(0.5, 0.5, 0.5),
                    1.0,
                    7,
                    Particle::HappyVillager,
                );
                let new_age = (entity.age.load(Relaxed) + age_up_seconds * 20).min(0);
                entity.age.store(new_age, Relaxed);
                item_used = true;
            }

            if temper > 0
                && (item_used || !self.is_tamed())
                && self.horse_data().temper.load(Relaxed) < self.max_temper()
            {
                let new_temper =
                    (self.horse_data().temper.load(Relaxed) + temper).clamp(0, self.max_temper());
                self.horse_data().temper.store(new_temper, Relaxed);
                item_used = true;
            }

            // `AbstractHorse.java:486-489`: `eating()` (open mouth + eating sound), then the
            // `EAT` game event. This is not the grazing pose, so `FLAG_EATING` is untouched.
            if item_used {
                self.eating();
                mob_entity.ate().await;
            }

            item_used
        })
    }

    /// `AbstractHorse.fedFood`.
    fn fed_food<'a>(
        &'a self,
        player: &'a Arc<Player>,
        item_stack: &'a mut ItemStack,
    ) -> EntityBaseFuture<'a, bool> {
        Box::pin(async move {
            let ate = self.handle_eating(player, item_stack).await;
            if ate {
                item_stack.decrement_unless_creative(player.gamemode.load(), 1);
            }
            ate
        })
    }

    /// Whether this species can currently show an inventory screen. Non-chested horses have
    /// nothing to show (saddle/armor are equipment-slot-only here, see the module doc comment),
    /// so the default is a no-op; `AbstractChestedHorse::chested_mob_interact` overrides this
    /// path with the real chest-inventory screen.
    fn open_custom_inventory_screen<'a>(
        &'a self,
        _player: &'a Arc<Player>,
    ) -> EntityBaseFuture<'a, ()> {
        Box::pin(async {})
    }

    /// `AbstractHorse.mobInteract` (`AbstractHorse.java:644-669`), shared by Horse, `ZombieHorse`
    /// and (post-tame-gate) `SkeletonHorse` -- the species' own `mobInteract` overrides in
    /// vanilla reduce to this exact dispatch once their food/mad early-returns are inlined (see
    /// the per-species files for the trace showing why).
    fn abstract_horse_mob_interact<'a>(
        &'a self,
        player: &'a Arc<Player>,
        item_stack: &'a mut ItemStack,
    ) -> EntityBaseFuture<'a, bool>
    where
        Self: Sized,
    {
        Box::pin(async move {
            let mob_entity = self.get_mob_entity();
            let entity = &mob_entity.living_entity.entity;
            let is_vehicle = !entity.passengers.lock().await.is_empty();

            if is_vehicle || self.is_baby() {
                return mob_entity
                    .mob_interact(player, item_stack, self.can_be_leashed())
                    .await;
            }

            if self.is_tamed() && player.get_entity().is_sneaking() {
                AbstractHorse::open_custom_inventory_screen(self, player).await;
                return true;
            }

            if !item_stack.is_empty() {
                if self.is_food(item_stack) {
                    return self.fed_food(player, item_stack).await;
                }

                if !self.is_tamed() {
                    self.make_mad();
                    return true;
                }

                let body_armor = {
                    let equipment = mob_entity.living_entity.entity_equipment.lock().await;
                    equipment.get(&EquipmentSlot::BODY)
                };
                if is_valid_body_armor_item(item_stack, entity.entity_type) && body_armor.is_empty()
                {
                    equip_body_armor_item(mob_entity, player, item_stack).await;
                    return true;
                }

                if saddle_equip_on_interact(item_stack, entity.entity_type)
                    && !AbstractHorse::is_saddled(self).await
                    && self.can_use_saddle_slot()
                {
                    self.equip_saddle(player, item_stack).await;
                    return true;
                }
            }

            self.do_player_ride(player).await;
            true
        })
    }

    /// `AbstractHorse.addAdditionalSaveData` (`AbstractHorse.java:788-795`) plus the randomized
    /// attribute base values (see the module doc comment on `randomize_attributes`).
    ///
    /// `MobEntity` has no generic owner persistence, so `Tame` and `Owner` are written here.
    fn write_horse_nbt(&self, nbt: &mut NbtCompound) {
        let data = self.horse_data();
        nbt.put_bool("EatingHaystack", data.get_flag(FLAG_EATING));
        nbt.put_bool("Bred", data.get_flag(FLAG_BRED));
        nbt.put_int("Temper", data.temper.load(Relaxed));
        nbt.put_bool("Tame", self.is_tamed());
        if let Some(owner) = self.get_mob_entity().owner.load() {
            nbt.put_uuid("Owner", owner);
        }

        let attributes = self
            .get_mob_entity()
            .living_entity
            .attributes
            .read()
            .unwrap();
        if let Some(a) = attributes.get(&pumpkin_data::attributes::Attributes::MAX_HEALTH.id) {
            nbt.put_double("PumpkinHorseMaxHealth", a.base_value);
        }
        if let Some(a) = attributes.get(&pumpkin_data::attributes::Attributes::MOVEMENT_SPEED.id) {
            nbt.put_double("PumpkinHorseSpeed", a.base_value);
        }
        if let Some(a) = attributes.get(&pumpkin_data::attributes::Attributes::JUMP_STRENGTH.id) {
            nbt.put_double("PumpkinHorseJumpStrength", a.base_value);
        }
    }

    fn read_horse_nbt(&self, nbt: &NbtCompound) {
        let data = self.horse_data();
        data.set_flag(FLAG_EATING, nbt.get_bool("EatingHaystack").unwrap_or(false));
        data.set_flag(FLAG_BRED, nbt.get_bool("Bred").unwrap_or(false));
        data.temper
            .store(nbt.get_int("Temper").unwrap_or(0), Relaxed);
        // `AbstractHorse.readAdditionalSaveData` (`AbstractHorse.java:798-805`). The tame bit is
        // kept apart from the owner so that `Tame:1b` without an `Owner` stays tame. Like every
        // other read here this precedes `mob_init_data_tracker`, which publishes the flags.
        data.set_flag(FLAG_TAME, nbt.get_bool("Tame").unwrap_or(false));
        if let Some(owner) = nbt.get_uuid("Owner") {
            self.get_mob_entity().set_owner(owner);
        }

        let mut attributes = self
            .get_mob_entity()
            .living_entity
            .attributes
            .write()
            .unwrap();
        if let Some(v) = nbt.get_double("PumpkinHorseMaxHealth")
            && let Some(a) =
                attributes.get_mut(&pumpkin_data::attributes::Attributes::MAX_HEALTH.id)
        {
            a.base_value = v;
            a.dirty.store(true, Relaxed);
        }
        if let Some(v) = nbt.get_double("PumpkinHorseSpeed")
            && let Some(a) =
                attributes.get_mut(&pumpkin_data::attributes::Attributes::MOVEMENT_SPEED.id)
        {
            a.base_value = v;
            a.dirty.store(true, Relaxed);
        }
        if let Some(v) = nbt.get_double("PumpkinHorseJumpStrength")
            && let Some(a) =
                attributes.get_mut(&pumpkin_data::attributes::Attributes::JUMP_STRENGTH.id)
        {
            a.base_value = v;
            a.dirty.store(true, Relaxed);
        }
        drop(attributes);

        let max_health = self.get_mob_entity().living_entity.get_max_health();
        if self.get_mob_entity().living_entity.health.load() > max_health {
            self.get_mob_entity().living_entity.health.store(max_health);
        }
    }
}

/// `AbstractHorse.setOffspringAttribute` (`AbstractHorse.java:831-838`): the baby's base value
/// for `attribute` becomes `create_offspring_attribute` of the two parents' base values.
/// `range_min`/`range_max` are the species-independent bounds shared by every horse family
/// member (`AbstractHorse.java:83-88`). A partner that is not a mob (never the case in vanilla,
/// where it is an `AgeableMob`) counts as sitting at the range minimum.
fn apply_offspring_attribute(
    parent: &dyn Mob,
    partner: &dyn EntityBase,
    baby: &dyn Mob,
    attribute: &'static pumpkin_data::attributes::Attributes,
    range_min: f64,
    range_max: f64,
    random: &mut impl RngExt,
) {
    let parent_value = parent
        .get_mob_entity()
        .living_entity
        .get_attribute_base(attribute);
    let partner_value = partner.get_mob().map_or(range_min, |partner| {
        partner
            .get_mob_entity()
            .living_entity
            .get_attribute_base(attribute)
    });
    let new_value =
        create_offspring_attribute(parent_value, partner_value, range_min, range_max, random);
    let mut attributes = baby
        .get_mob_entity()
        .living_entity
        .attributes
        .write()
        .unwrap();
    if let Some(a) = attributes.get_mut(&attribute.id) {
        a.base_value = new_value;
        a.dirty.store(true, Relaxed);
    }
}

/// `AbstractHorse.setOffspringAttributes` (`AbstractHorse.java:825-829`): the baby inherits
/// max health, jump strength and movement speed from `parent` and `partner`.
///
/// Vanilla builds the baby from the plain 53-health base attributes, so lowering its max health
/// to the inherited value clamps its health to it (`LivingEntity.onAttributeUpdated`,
/// `LivingEntity.java:1133-1138`) and the foal starts at full inherited health. Pumpkin builds
/// it through `randomize_attributes`, which leaves health at an unrelated rolled maximum, so the
/// same result is set explicitly once the attributes are in.
pub fn set_offspring_attributes(parent: &dyn Mob, partner: &dyn EntityBase, baby: &dyn Mob) {
    use pumpkin_data::attributes::Attributes;

    let mut random = rand::rng();
    apply_offspring_attribute(
        parent,
        partner,
        baby,
        &Attributes::MAX_HEALTH,
        MIN_HEALTH,
        MAX_HEALTH,
        &mut random,
    );
    apply_offspring_attribute(
        parent,
        partner,
        baby,
        &Attributes::JUMP_STRENGTH,
        MIN_JUMP_STRENGTH,
        MAX_JUMP_STRENGTH,
        &mut random,
    );
    apply_offspring_attribute(
        parent,
        partner,
        baby,
        &Attributes::MOVEMENT_SPEED,
        MIN_MOVEMENT_SPEED,
        MAX_MOVEMENT_SPEED,
        &mut random,
    );
    let baby_living = &baby.get_mob_entity().living_entity;
    baby_living.health.store(baby_living.get_max_health());
}

/// Mirrors vanilla `AbstractChestedHorse` (Donkey/Mule; Llama is out of scope for this task).
pub trait AbstractChestedHorse: AbstractHorse {
    fn chested_data(&self) -> &ChestedHorseData;

    fn has_chest(&self) -> bool {
        self.chested_data().has_chest.load(Relaxed)
    }

    /// `AbstractChestedHorse.getQuadLeashOffsets` (`AbstractChestedHorse.java:176-179`) uses
    /// the shorter chested-horse body dimensions instead of `AbstractHorse`'s offsets.
    fn get_quad_leash_offsets(&self) -> [Vector3<f64>; 4] {
        let width = f64::from(self.get_entity().width());
        let height = f64::from(self.get_entity().height());
        let front_offset = 0.04 * width;
        let front_back = 0.41 * width;
        let left_right = 0.18 * width;
        let y = 0.73 * height;
        [
            Vector3::new(-left_right, y, front_back + front_offset),
            Vector3::new(-left_right, y, -front_back + front_offset),
            Vector3::new(left_right, y, -front_back + front_offset),
            Vector3::new(left_right, y, front_back + front_offset),
        ]
    }

    fn get_inventory_columns(&self) -> u8 {
        if self.has_chest() { 5 } else { 0 }
    }

    fn play_chest_equips_sound(&self) {
        let entity = self.get_entity();
        let world = entity.world.load();
        world.play_sound(
            Sound::EntityDonkeyChest,
            SoundCategory::Neutral,
            &entity.pos.load(),
        );
    }

    /// The `createInventory` half of `AbstractChestedHorse.setChest`: replaces the backing
    /// inventory, preserving any existing stacks in the overlapping slot range, without telling
    /// clients. NBT loading uses this directly since `mob_init_data_tracker` publishes the state.
    fn resize_chest_inventory(&self, value: bool) -> EntityBaseFuture<'_, ()> {
        Box::pin(async move {
            self.chested_data().has_chest.store(value, Relaxed);
            let new_size = if value {
                usize::from(self.get_inventory_columns()) * 3
            } else {
                0
            };

            let mut slot = self.chested_data().inventory.lock().await;
            let old = slot.clone();
            let new_inventory = Arc::new(SimpleInventory::new(new_size));
            let max = old.size().min(new_inventory.size());
            for i in 0..max {
                let stack = old.get_stack(i).await;
                if !stack.is_empty() {
                    new_inventory.set_stack(i, stack).await;
                }
            }
            *slot = new_inventory;
        })
    }

    /// `AbstractChestedHorse.setChest` + `createInventory` (`AbstractChestedHorse.java:63-65`).
    fn set_chest(&self, value: bool) -> EntityBaseFuture<'_, ()> {
        Box::pin(async move {
            self.resize_chest_inventory(value).await;
            self.sync_chest();
        })
    }

    fn send_chest_flag(&self, has_chest: bool) {
        self.get_entity().send_meta_data(
            &[Metadata::new(
                tracked_data::donkey::DATA_ID_CHEST,
                has_chest,
            )],
            None,
        );
    }

    /// Publishes `DATA_ID_CHEST` when it differs from what clients already have.
    fn sync_chest(&self) {
        let has_chest = self.has_chest();
        if self.chested_data().synced_chest.swap(has_chest, Relaxed) != has_chest {
            self.send_chest_flag(has_chest);
        }
    }

    /// The `mob_init_data_tracker` half of `AbstractChestedHorse.defineSynchedData`
    /// (`AbstractChestedHorse.java:49-53`); the client default is no chest.
    fn send_initial_chest(&self) {
        let has_chest = self.has_chest();
        self.chested_data().synced_chest.store(has_chest, Relaxed);
        if has_chest {
            self.send_chest_flag(true);
        }
    }

    /// `AbstractHorse.dropEquipment` (`AbstractHorse.java:518-529`) followed by
    /// `AbstractChestedHorse.dropEquipment` (`AbstractChestedHorse.java:73-79`): every stack in
    /// the chest that is not bound by Curse of Vanishing is dropped, then the chest itself.
    fn drop_chest_contents(&self) -> EntityBaseFuture<'_, ()> {
        Box::pin(async move {
            let entity = self.get_entity();
            let world = entity.world.load();
            let pos = entity.block_pos.load();
            let inventory = self.chested_data().inventory.lock().await.clone();
            for slot in 0..inventory.size() {
                let stack = inventory.get_stack(slot).await;
                if !stack.is_empty()
                    && !crate::entity::living::LivingEntity::item_prevents_equipment_drop(&stack)
                {
                    world.drop_stack(&pos, stack).await;
                }
            }
            if self.has_chest() {
                world
                    .drop_stack(&pos, ItemStack::new(1, &Item::CHEST))
                    .await;
                self.set_chest(false).await;
            }
        })
    }

    /// Opens the chest as a generic 9x3 screen. Without a chest there is nothing to show -- the
    /// screen would be 27 slots over a size-0 inventory that silently discards what is moved into
    /// them -- so, until the real mount menu with its saddle and armor slots exists (see the
    /// module doc comment), a chestless horse opens nothing.
    fn open_chest_inventory<'a>(&'a self, player: &'a Arc<Player>) -> EntityBaseFuture<'a, ()> {
        Box::pin(async move {
            if !self.has_chest() {
                return;
            }
            let inventory = self.chested_data().inventory.lock().await.clone();
            player
                .open_handled_screen(
                    &HorseChestScreenFactory(inventory as Arc<dyn Inventory>),
                    None,
                )
                .await;
        })
    }

    /// `AbstractChestedHorse.mobInteract` (`AbstractChestedHorse.java:143-167`).
    fn chested_mob_interact<'a>(
        &'a self,
        player: &'a Arc<Player>,
        item_stack: &'a mut ItemStack,
    ) -> EntityBaseFuture<'a, bool>
    where
        Self: Sized,
    {
        Box::pin(async move {
            let mob_entity = self.get_mob_entity();
            let entity = &mob_entity.living_entity.entity;
            let is_vehicle = !entity.passengers.lock().await.is_empty();
            let should_open_inventory =
                !self.is_baby() && self.is_tamed() && player.get_entity().is_sneaking();
            let baby_with_dandelion =
                self.is_baby() && item_stack.item.id == Item::GOLDEN_DANDELION.id;

            if is_vehicle || should_open_inventory || baby_with_dandelion {
                if should_open_inventory && !is_vehicle {
                    self.open_chest_inventory(player).await;
                    return true;
                }
                return mob_entity
                    .mob_interact(player, item_stack, self.can_be_leashed())
                    .await;
            }

            if !item_stack.is_empty() {
                if self.is_food(item_stack) {
                    return self.fed_food(player, item_stack).await;
                }

                if !self.is_tamed() {
                    self.make_mad();
                    return true;
                }

                if !self.has_chest() && item_stack.item.id == Item::CHEST.id {
                    self.set_chest(true).await;
                    self.play_chest_equips_sound();
                    item_stack.decrement_unless_creative(player.gamemode.load(), 1);
                    return true;
                }
            }

            self.abstract_horse_mob_interact(player, item_stack).await
        })
    }

    /// `AbstractChestedHorse.addAdditionalSaveData` (`AbstractChestedHorse.java:82-95`): the
    /// `ChestedHorse` flag and, with a chest, an `Items` list of `Slot`-tagged stacks.
    fn write_chested_horse_nbt<'a>(&'a self, nbt: &'a mut NbtCompound) -> EntityBaseFuture<'a, ()> {
        Box::pin(async move {
            nbt.put_bool("ChestedHorse", self.has_chest());
            if self.has_chest() {
                let inventory = self.chested_data().inventory.lock().await.clone();
                inventory.write_inventory_nbt(nbt, true).await;
            }
        })
    }

    /// `AbstractChestedHorse.readAdditionalSaveData` (`AbstractChestedHorse.java:98-109`): the
    /// chest is sized first, then every `Items` entry whose slot fits it is put back.
    fn read_chested_horse_nbt<'a>(&'a self, nbt: &'a NbtCompound) -> EntityBaseFuture<'a, ()> {
        Box::pin(async move {
            let has_chest = nbt.get_bool("ChestedHorse").unwrap_or(false);
            self.resize_chest_inventory(has_chest).await;
            if has_chest {
                let inventory = self.chested_data().inventory.lock().await.clone();
                let mut stacks = vec![ItemStack::EMPTY.clone(); inventory.size()];
                inventory.read_data(nbt, &mut stacks);
                for (slot, stack) in stacks.into_iter().enumerate() {
                    if !stack.is_empty() {
                        inventory.set_stack(slot, stack).await;
                    }
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{
        FLAG_BRED, FLAG_EATING, FLAG_OPEN_MOUTH, FLAG_STANDING, FLAG_TAME, GallopStep,
        create_offspring_attribute, gallop_sound, gallop_step, generate_jump_strength,
        generate_max_health, generate_speed, generate_zombie_horse_jump_strength,
        generate_zombie_horse_speed, is_wood_step_sound, mount_panic_allowed, next_mouth_counter,
        player_jump_pending_scale, synced_flags_byte,
    };
    use pumpkin_data::sound::Sound;
    use rand::rng;

    /// `AbstractHorse.MIN_HEALTH`/`MAX_HEALTH`: `generateMaxHealth(i -> 0)` and
    /// `generateMaxHealth(i -> i - 1)`.
    #[test]
    fn generate_max_health_stays_within_vanilla_bounds() {
        let mut random = rng();
        for _ in 0..1000 {
            let health = generate_max_health(&mut random);
            assert!((15.0..=30.0).contains(&health), "{health} out of range");
        }
    }

    /// `PlayerRideableJumping.getPlayerJumpPendingScale` (`PlayerRideableJumping.java:12-16`).
    #[test]
    fn jump_charge_maps_to_vanilla_scale() {
        assert_eq!(player_jump_pending_scale(-1), 400);
        assert_eq!(player_jump_pending_scale(0), 400);
        assert_eq!(player_jump_pending_scale(90), 1000);
        assert_eq!(player_jump_pending_scale(120), 1000);
    }

    #[test]
    fn generate_jump_strength_stays_within_vanilla_bounds() {
        let mut random = rng();
        for _ in 0..1000 {
            let jump = generate_jump_strength(&mut random);
            assert!((0.4..=1.0).contains(&jump), "{jump} out of range");
        }
    }

    #[test]
    fn generate_speed_stays_within_vanilla_bounds() {
        let mut random = rng();
        for _ in 0..1000 {
            let speed = generate_speed(&mut random);
            assert!((0.1125..=0.3375).contains(&speed), "{speed} out of range");
        }
    }

    #[test]
    fn generate_zombie_horse_jump_strength_stays_within_vanilla_bounds() {
        let mut random = rng();
        for _ in 0..1000 {
            let jump = generate_zombie_horse_jump_strength(&mut random);
            assert!((0.5..=0.7).contains(&jump), "{jump} out of range");
        }
    }

    #[test]
    fn generate_zombie_horse_speed_stays_within_vanilla_bounds() {
        let mut random = rng();
        for _ in 0..1000 {
            let speed = generate_zombie_horse_speed(&mut random);
            assert!((0.213..=0.2847).contains(&speed), "{speed} out of range");
        }
    }

    /// `AbstractHorse.createOffspringAttribute`: result must stay within `[range_min, range_max]`
    /// regardless of the parent values or random rolls.
    #[test]
    fn create_offspring_attribute_stays_within_bounds() {
        let mut random = rng();
        for _ in 0..1000 {
            let value = create_offspring_attribute(15.0, 30.0, 15.0, 30.0, &mut random);
            assert!((15.0..=30.0).contains(&value), "{value} out of range");
        }
    }

    #[test]
    #[should_panic(expected = "Incorrect range for an attribute")]
    fn create_offspring_attribute_rejects_inverted_range() {
        let mut random = rng();
        let _ = create_offspring_attribute(15.0, 30.0, 30.0, 15.0, &mut random);
    }

    #[test]
    /// `AbstractHorse.MountPanicGoal.shouldPanic` (`AbstractHorse.java:1053-1057`) rejects
    /// mob-controlled mounts.
    fn mount_panic_is_disabled_for_mob_controlled_horses() {
        assert!(mount_panic_allowed(false));
        assert!(!mount_panic_allowed(true));
    }

    /// `AbstractHorse.java:96-101`: the vanilla bit values, with the tame bit added from the
    /// owner and no bit outside the five defined ones ever sent.
    #[test]
    fn synced_flags_byte_uses_the_vanilla_bit_layout() {
        assert_eq!(FLAG_TAME, 2);
        assert_eq!(FLAG_BRED, 8);
        assert_eq!(FLAG_EATING, 16);
        assert_eq!(FLAG_STANDING, 32);
        assert_eq!(FLAG_OPEN_MOUTH, 64);
        assert_eq!(synced_flags_byte(0, false), 0);
        assert_eq!(synced_flags_byte(0, true), 2);
        assert_eq!(synced_flags_byte(FLAG_EATING | FLAG_STANDING, true), 50);
        assert_eq!(synced_flags_byte(FLAG_BRED | FLAG_OPEN_MOUTH, false), 72);
        assert_eq!(synced_flags_byte(0b1, false), 0);
        assert_eq!(synced_flags_byte(FLAG_TAME, false), 0);
    }

    /// `AbstractHorse.tick` (`AbstractHorse.java:579-582`): `mouthCounter` runs 1..=30 after
    /// `openMouth` and the flag clears on the tick it passes 30.
    #[test]
    fn mouth_counter_closes_after_thirty_ticks() {
        assert_eq!(next_mouth_counter(0), (0, false));
        assert_eq!(next_mouth_counter(1), (2, false));
        assert_eq!(next_mouth_counter(29), (30, false));
        assert_eq!(next_mouth_counter(30), (0, true));
    }

    /// `AbstractHorse.playStepSound` (`AbstractHorse.java:350-356`): the first five ridden steps
    /// play the wood step, then every third step gallops and the ones between are silent.
    #[test]
    fn ridden_steps_follow_the_gallop_counter() {
        for counter in 1..=5 {
            assert_eq!(gallop_step(counter), GallopStep::Step, "step {counter}");
        }
        assert_eq!(gallop_step(6), GallopStep::Gallop);
        assert_eq!(gallop_step(7), GallopStep::Silent);
        assert_eq!(gallop_step(8), GallopStep::Silent);
        assert_eq!(gallop_step(9), GallopStep::Gallop);
        assert_eq!(gallop_step(12), GallopStep::Gallop);
    }

    /// `AbstractHorse.isWoodSoundType` (`AbstractHorse.java:365-371`).
    #[test]
    fn wood_sound_types_are_recognised_by_step_sound() {
        assert!(is_wood_step_sound(Sound::BlockWoodStep));
        assert!(is_wood_step_sound(Sound::BlockNetherWoodStep));
        assert!(is_wood_step_sound(Sound::BlockStemStep));
        assert!(is_wood_step_sound(Sound::BlockCherryWoodStep));
        assert!(is_wood_step_sound(Sound::BlockBambooWoodStep));
        assert!(!is_wood_step_sound(Sound::BlockStoneStep));
        assert!(!is_wood_step_sound(Sound::BlockBambooStep));
    }

    /// `AbstractHorse.playGallopSound` (`AbstractHorse.java:373-375`) scales the block volume by
    /// `0.15` and keeps its pitch.
    #[test]
    fn gallop_sound_scales_block_volume() {
        let (sound, volume, pitch) = gallop_sound(0.5, 1.2);
        assert_eq!(sound, Sound::EntityHorseGallop);
        assert!((volume - 0.075).abs() < f32::EPSILON);
        assert!((pitch - 1.2).abs() < f32::EPSILON);
    }
}
