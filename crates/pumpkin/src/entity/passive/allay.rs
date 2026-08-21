// Legacy invariant checks retained for vanilla behavior; migrate these paths before removing this allow.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
// Port of net.minecraft.world.entity.animal.allay.{Allay, AllayAi}.
//
// This is Stage 2 of `BRAIN_DESIGN.md`: the Allay is the first mob in this codebase driven by
// `crate::entity::ai::brain` rather than by the Goal system, matching vanilla, where `Allay`
// has no `goalSelector` entries at all and `AllayAi.getActivities()` (`AllayAi.java:57-59`)
// supplies a CORE and an IDLE activity. The previous revision of this file carried
// `LIKED_PLAYER` / `LIKED_NOTEBLOCK_POSITION` / `LIKED_NOTEBLOCK_COOLDOWN_TICKS` as plain
// fields and had no item-fetch loop at all; those three are now real brain memories and the
// fetch loop is the vanilla behavior set.
//
// Concretely ported: the CORE activity (`AllayAi.java:61-73`) and the IDLE activity
// (`AllayAi.java:75-90`) minus `SetEntityLookTargetSometimes`, the item give/take/duplicate
// `mobInteract` branch (`Allay.java:281-317`), jukebox/note-block dancing
// (`setJukeboxPlaying`, `hearNoteblock`, `shouldStopDancing`), the duplication cooldown, and
// the `wantsToPickUp`/`canPickUpLoot`/`pickUpItem` inventory path (`Allay.java:263-362`).
//
// Explicitly NOT ported, with reasons:
//
// - `SetEntityLookTargetSometimes.create(6.0F, UniformInt.of(30, 60))` (`AllayAi.java:85`).
//   It reads `NEAREST_VISIBLE_LIVING_ENTITIES`, which needs a `NearestLivingEntitiesSensor`;
//   only `NearestItemSensor` is ported (`ai/brain/sensor/`). An Allay therefore does not
//   glance at nearby players while idling. `SetWalkTargetFromLookTarget` in the `RunOne` is
//   kept, but with nothing writing `LOOK_TARGET` from the world it only ever follows a look
//   target some other behavior set.
// - `DATA_DANCING`/`DATA_CAN_DUPLICATE` client-synced booleans and the client-side
//   `holdingItemAnimationTicks`/`dancingAnimationTicks`/`spinningAnimationTicks` animation
//   state (`Allay.java:80-81, 228-260, 393-404`): Pumpkin's `Entity` has no generic boolean
//   tracked-data slot exposed to per-mob code the way vanilla's `SynchedEntityData.Builder`
//   does, and no client animation-state channel at all (the same gap `warden.rs` notes for
//   `AnimationState`). `is_dancing` below is server-side only.
// - `hasNonMatchingPotion` (`Allay.java:353-358`), which compares
//   `DataComponents.POTION_CONTENTS`: the item-identity half of `allayConsidersItemEqual` is
//   ported, the potion-content special case is not, so an Allay holding a healing potion will
//   also fetch a poison potion.
// - `CriteriaTriggers.ALLAY_DROP_ITEM_ON_BLOCK` in `AllayAi.onItemThrown` (`AllayAi.java:159`):
//   no advancement trigger of that name is wired up here.
// - `Allay.getPickupReach()` (`Allay.java:336-339`), a custom pickup box. The shared
//   `Mob::mob_try_pick_up_items` pass uses its own reach and is not parameterised.
//
// Two deviations worth naming because they are structural, not omissions:
//
// - The main-hand item is mirrored into a plain `std::sync::Mutex<ItemStack>`. Vanilla reads
//   it via `getItemInHand`, but `Mob::wants_to_pick_up_item` is a synchronous trait method and
//   `LivingEntity::entity_equipment` is behind a `tokio::sync::Mutex`, so the sync mirror is
//   what `wantsToPickUp` and `canPickUpLoot` consult. Every write goes to both. An external
//   equipment write (`/item replace`) would desync the mirror; vanilla blocks dispensers from
//   this slot anyway (`canDispenserEquipIntoSlot`, `Allay.java:271-274`).
// - `LIKED_NOTEBLOCK_POSITION` is a `BlockPos`, not a `GlobalPos`; see
//   `ai/brain/memory.rs`'s module comment. `shouldDepositItemsAtLikedNoteblock`'s
//   dimension equality check is therefore not performed.

use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Weak};

use pumpkin_data::data_component_impl::EquipmentSlot;
use pumpkin_data::entity::EntityType;
use pumpkin_data::game_event::GameEvent;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_data::tag::{self, Taggable};
use pumpkin_util::GameMode;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use rand::RngExt;
use uuid::Uuid;

use crate::entity::ai::brain::behavior::count_down_cooldown_ticks::CountDownCooldownTicks;
use crate::entity::ai::brain::behavior::do_nothing::DoNothing;
use crate::entity::ai::brain::behavior::gate::GateBehavior;
use crate::entity::ai::brain::behavior::go_and_give_items_to_target::GoAndGiveItemsToTarget;
use crate::entity::ai::brain::behavior::go_to_wanted_item::GoToWantedItem;
use crate::entity::ai::brain::behavior::look_at_target_sink::LookAtTargetSink;
use crate::entity::ai::brain::behavior::move_to_target_sink::MoveToTargetSink;
use crate::entity::ai::brain::behavior::random_stroll::RandomStrollFly;
use crate::entity::ai::brain::behavior::set_walk_target_from_look_target::SetWalkTargetFromLookTarget;
use crate::entity::ai::brain::behavior::stay_close_to_target::StayCloseToTarget;
use crate::entity::ai::brain::behavior::{animal_panic::AnimalPanic, swim::Swim};
use crate::entity::ai::brain::memory::{
    ItemPickupCooldownTicksMemory, LikedNoteblockCooldownTicksMemory, LikedNoteblockPositionMemory,
    LikedPlayerMemory, NearestVisibleWantedItemMemory, PositionTracker,
};
use crate::entity::ai::brain::sensor::nearest_item::NearestItemSensor;
use crate::entity::ai::brain::{Activity, ActivityData, Brain};
use crate::entity::player::Player;
use crate::entity::{
    Entity, EntityBase, EntityBaseFuture, NBTStorage,
    mob::{Mob, MobEntity},
};
use crate::world::World;
use crate::world::game_event::{
    GameEventContext, GameEventFuture, GameEventListener, PositionSource,
};

/// `Allay.DUPLICATION_COOLDOWN_TICKS`
const DUPLICATION_COOLDOWN_TICKS: i64 = 6000;
/// `AllayAi.TIME_TO_FORGET_NOTEBLOCK` (`AllayAi.java:53`)
const TIME_TO_FORGET_NOTEBLOCK_TICKS: i32 = 600;
/// The chessboard distance `shouldDepositItemsAtLikedNoteblock` allows (`AllayAi.java:131`).
const MAX_NOTEBLOCK_DISTANCE: i32 = 1024;
/// `AllayAi.SPEED_MULTIPLIER_WHEN_RETRIEVING_ITEM` (`AllayAi.java:46`)
const SPEED_WHEN_RETRIEVING_ITEM: f32 = 1.75;
/// `AllayAi.SPEED_MULTIPLIER_WHEN_FOLLOWING_DEPOSIT_TARGET` (`AllayAi.java:45`)
const SPEED_WHEN_FOLLOWING_DEPOSIT_TARGET: f32 = 2.25;
/// `AllayAi.SPEED_MULTIPLIER_WHEN_IDLING` (`AllayAi.java:44`)
const SPEED_WHEN_IDLING: f32 = 1.0;
/// `AllayAi.SPEED_MULTIPLIER_WHEN_PANICKING` (`AllayAi.java:47`)
const SPEED_WHEN_PANICKING: f32 = 2.5;
/// `AllayAi.CLOSE_ENOUGH_TO_TARGET` / `TOO_FAR_FROM_TARGET` (`AllayAi.java:48-49`)
const CLOSE_ENOUGH_TO_TARGET: i32 = 4;
const TOO_FAR_FROM_TARGET: i32 = 16;
/// `AllayAi.DISTANCE_TO_WANTED_ITEM` (`AllayAi.java:54`)
const DISTANCE_TO_WANTED_ITEM: f64 = 32.0;
/// `AllayAi.GIVE_ITEM_TIMEOUT_DURATION` (`AllayAi.java:55`)
const GIVE_ITEM_TIMEOUT_DURATION: i32 = 20;
/// `AllayAi.MIN_WAIT_DURATION` / `MAX_WAIT_DURATION` (`AllayAi.java:51-52`), the `DoNothing`
/// bounds inside the idle `RunOne`.
const MIN_WAIT_DURATION: i32 = 30;
const MAX_WAIT_DURATION: i32 = 60;
/// `AllayAi.getLikedPlayer`'s liveness radius (`AllayAi.java:148`).
const LIKED_PLAYER_MAX_DISTANCE: f64 = 64.0;
/// `Allay.NUM_OF_DUPLICATION_HEARTS`. Currently unused: there is no heart-particle
/// broadcast channel wired up for this yet (see module doc comment on client sync gaps).
#[allow(dead_code)]
const NUM_OF_DUPLICATION_HEARTS: u32 = 3;
/// The notification radius `GameEvent.JUKEBOX_PLAY` uses (`shouldStopDancing` compares against
/// `GameEvent.JUKEBOX_PLAY.value().notificationRadius()`, which is 10 -- see
/// `crate::world::game_event::notification_radius`).
const JUKEBOX_DANCE_RADIUS: f64 = 10.0;
/// `Allay.THROW_SOUND_PITCHES` (`Allay.java:87-89`).
const THROW_SOUND_PITCHES: [f32; 16] = [
    0.5625, 0.625, 0.75, 0.9375, 1.0, 1.0, 1.125, 1.25, 1.5, 1.875, 2.0, 2.25, 2.5, 3.0, 3.75, 4.0,
];

/// Represents an Allay, a passive, flying entity that can collect items for the player.
///
/// Wiki: <https://minecraft.wiki/w/Allay>
pub struct AllayEntity {
    pub mob_entity: MobEntity,
    is_dancing: AtomicBool,
    jukebox_pos: std::sync::Mutex<Option<BlockPos>>,
    /// `Allay.inventory = new SimpleContainer(1)` (`Allay.java:95`). One slot, so every
    /// container operation collapses to an operation on a single stack.
    inventory: std::sync::Mutex<ItemStack>,
    /// Synchronous mirror of `EquipmentSlot::MAIN_HAND`; see the module comment.
    item_in_hand: std::sync::Mutex<ItemStack>,
    duplication_cooldown: AtomicI64,
    listener_registered: AtomicBool,
    vibration_listener: std::sync::Mutex<Option<Arc<AllayVibrationListener>>>,
    jukebox_listener: std::sync::Mutex<Option<Arc<AllayJukeboxListener>>>,
}

impl AllayEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let mut mob_entity = MobEntity::new(entity);
        mob_entity.brain = Some(Self::make_brain());

        let allay = Self {
            mob_entity,
            is_dancing: AtomicBool::new(false),
            jukebox_pos: std::sync::Mutex::new(None),
            inventory: std::sync::Mutex::new(ItemStack::EMPTY.clone()),
            item_in_hand: std::sync::Mutex::new(ItemStack::EMPTY.clone()),
            duplication_cooldown: AtomicI64::new(0),
            listener_registered: AtomicBool::new(false),
            vibration_listener: std::sync::Mutex::new(None),
            jukebox_listener: std::sync::Mutex::new(None),
        };
        let mob_arc = Arc::new(allay);

        let uuid = mob_arc.mob_entity.living_entity.entity.entity_uuid;
        *mob_arc.vibration_listener.lock().unwrap() = Some(Arc::new(AllayVibrationListener {
            allay: Arc::downgrade(&mob_arc),
            uuid,
        }));
        *mob_arc.jukebox_listener.lock().unwrap() = Some(Arc::new(AllayJukeboxListener {
            allay: Arc::downgrade(&mob_arc),
            uuid,
        }));

        mob_arc
    }

    /// `Allay.BRAIN_PROVIDER` / `AllayAi.getActivities()` (`AllayAi.java:57-59`). The Allay has
    /// no goals: every entry below is a `Behavior`, not a `Goal`.
    fn make_brain() -> Brain {
        let brain = Brain::new(
            vec![NearestItemSensor::new()],
            vec![Self::init_core_activity(), Self::init_idle_activity()],
        );
        // Memories read through `getMemory` rather than declared as a behavior's required
        // memory still have to be registered, or `Brain.checkMemory` reports them absent
        // (`Brain.java:242-249`). Vanilla registers them via `Allay.MEMORY_TYPES`.
        brain.register::<LikedPlayerMemory>();
        brain.register::<LikedNoteblockPositionMemory>();
        brain.register::<LikedNoteblockCooldownTicksMemory>();
        brain
    }

    /// `AllayAi.initCoreActivity` (`AllayAi.java:61-73`).
    fn init_core_activity() -> ActivityData {
        ActivityData::create(
            Activity::Core,
            0,
            vec![
                Swim::new(0.8),
                AnimalPanic::new(SPEED_WHEN_PANICKING),
                LookAtTargetSink::new(45, 90),
                MoveToTargetSink::new(),
                CountDownCooldownTicks::<LikedNoteblockCooldownTicksMemory>::new(),
                CountDownCooldownTicks::<ItemPickupCooldownTicksMemory>::new(),
            ],
        )
    }

    /// `AllayAi.initIdleActivity` (`AllayAi.java:75-90`), minus
    /// `SetEntityLookTargetSometimes` (see module comment).
    fn init_idle_activity() -> ActivityData {
        ActivityData::create(
            Activity::Idle,
            0,
            vec![
                GoToWantedItem::new(SPEED_WHEN_RETRIEVING_ITEM, true, DISTANCE_TO_WANTED_ITEM),
                GoAndGiveItemsToTarget::new(
                    get_item_deposit_position,
                    SPEED_WHEN_FOLLOWING_DEPOSIT_TARGET,
                    GIVE_ITEM_TIMEOUT_DURATION,
                    on_item_thrown,
                ),
                StayCloseToTarget::new(
                    get_item_deposit_position,
                    does_not_have_wanted_item,
                    CLOSE_ENOUGH_TO_TARGET,
                    TOO_FAR_FROM_TARGET,
                    SPEED_WHEN_FOLLOWING_DEPOSIT_TARGET,
                ),
                GateBehavior::run_one(vec![
                    (RandomStrollFly::new(SPEED_WHEN_IDLING), 2),
                    (SetWalkTargetFromLookTarget::new(SPEED_WHEN_IDLING, 3), 2),
                    (DoNothing::new(MIN_WAIT_DURATION, MAX_WAIT_DURATION), 1),
                ]),
            ],
        )
    }

    const fn brain(&self) -> &Brain {
        self.mob_entity
            .brain
            .as_ref()
            .expect("AllayEntity is always constructed with a brain")
    }

    #[must_use]
    pub fn is_dancing(&self) -> bool {
        self.is_dancing.load(Ordering::Relaxed)
    }

    /// `Allay.setDancing`, minus the `isEffectiveAi`/panic gate and the client `DATA_DANCING`
    /// sync (see module doc comment).
    fn set_dancing(&self, dancing: bool) {
        self.is_dancing.store(dancing, Ordering::Relaxed);
    }

    /// `Allay.setJukeboxPlaying`
    fn set_jukebox_playing(&self, jukebox: BlockPos, is_playing: bool) {
        let mut jukebox_pos = self.jukebox_pos.lock().unwrap();
        if is_playing {
            if !self.is_dancing() {
                *jukebox_pos = Some(jukebox);
                drop(jukebox_pos);
                self.set_dancing(true);
            }
        } else if *jukebox_pos == Some(jukebox) || jukebox_pos.is_none() {
            *jukebox_pos = None;
            drop(jukebox_pos);
            self.set_dancing(false);
        }
    }

    /// `Allay.shouldStopDancing`, minus the "is it actually still a jukebox block" check
    /// (would need a block-state lookup call site outside `mob_tick`'s borrow shape; the
    /// distance check below is the one vanilla treats as authoritative in the common case
    /// of the jukebox simply running out of range or finishing its song, since
    /// `JukeboxSongPlayer` stops emitting `JUKEBOX_PLAY` once it does).
    fn should_stop_dancing(&self, my_pos: Vector3<f64>) -> bool {
        let jukebox_pos = *self.jukebox_pos.lock().unwrap();
        jukebox_pos.is_none_or(|pos| {
            let center = Vector3::new(
                f64::from(pos.0.x) + 0.5,
                f64::from(pos.0.y) + 0.5,
                f64::from(pos.0.z) + 0.5,
            );
            (center - my_pos).length_squared() > JUKEBOX_DANCE_RADIUS * JUKEBOX_DANCE_RADIUS
        })
    }

    /// `AllayAi.hearNoteblock` (`AllayAi.java:96-106`).
    fn hear_noteblock(&self, pos: BlockPos) {
        let brain = self.brain();
        match brain.get::<LikedNoteblockPositionMemory>() {
            None => {
                brain.set::<LikedNoteblockPositionMemory>(pos);
                brain.set::<LikedNoteblockCooldownTicksMemory>(TIME_TO_FORGET_NOTEBLOCK_TICKS);
            }
            Some(existing) if existing == pos => {
                brain.set::<LikedNoteblockCooldownTicksMemory>(TIME_TO_FORGET_NOTEBLOCK_TICKS);
            }
            Some(_) => {}
        }
    }

    fn is_on_duplication_cooldown(&self) -> bool {
        self.duplication_cooldown.load(Ordering::Relaxed) > 0
    }

    fn reset_duplication_cooldown(&self) {
        self.duplication_cooldown
            .store(DUPLICATION_COOLDOWN_TICKS, Ordering::Relaxed);
    }

    /// `Allay.hasItemInHand` (`Allay.java:267-269`), off the sync mirror.
    #[must_use]
    pub fn has_item_in_hand(&self) -> bool {
        !self.item_in_hand.lock().unwrap().is_empty()
    }

    /// `Allay.isOnPickupCooldown` (`Allay.java:278-280`).
    fn is_on_pickup_cooldown(&self) -> bool {
        self.brain().has_value::<ItemPickupCooldownTicksMemory>()
    }

    /// Writes both the sync mirror and the real equipment slot.
    async fn set_item_in_hand(&self, stack: ItemStack) {
        *self.item_in_hand.lock().unwrap() = stack.clone();
        self.mob_entity
            .living_entity
            .entity_equipment
            .lock()
            .await
            .put(&EquipmentSlot::MAIN_HAND, stack);
    }

    /// `SimpleContainer.canAddItem` for a one-slot container (`SimpleContainer.java:86-97`).
    fn inventory_can_add(&self, stack: &ItemStack) -> bool {
        let slot = self.inventory.lock().unwrap();
        slot.is_empty()
            || (slot.are_items_and_components_equal(stack)
                && slot.item_count < slot.get_max_stack_size())
    }

    /// `allayConsidersItemEqual`, minus the potion-content special case (see module doc
    /// comment).
    #[must_use]
    pub const fn considers_item_equal(a: &ItemStack, b: &ItemStack) -> bool {
        a.item.id == b.item.id
    }

    /// `Allay.duplicateAllay`
    async fn duplicate(&self) {
        let world = self.mob_entity.living_entity.entity.world.load_full();
        let pos = self.mob_entity.living_entity.entity.pos.load();
        let new_entity = Entity::new(world.clone(), pos, &EntityType::ALLAY);
        let clone = Self::new(new_entity);
        clone.reset_duplication_cooldown();
        self.reset_duplication_cooldown();
        world.spawn_entity(clone).await;
    }

    async fn register_listeners_once(&self) {
        if self.listener_registered.swap(true, Ordering::Relaxed) {
            return;
        }
        let vibration_listener = self.vibration_listener.lock().unwrap().clone();
        let jukebox_listener = self.jukebox_listener.lock().unwrap().clone();
        let world = self.mob_entity.living_entity.entity.world.load();
        if let Some(listener) = vibration_listener {
            world.register_game_event_listener(listener).await;
        }
        if let Some(listener) = jukebox_listener {
            world.register_game_event_listener(listener).await;
        }
    }
}

/// `AllayAi.hasWantedItem` (`AllayAi.java:122-125`), negated -- vanilla passes
/// `Predicate.not(AllayAi::hasWantedItem)` to `StayCloseToTarget` (`AllayAi.java:84`), so an
/// Allay that has spotted an item on the ground stops trailing its deposit target.
fn does_not_have_wanted_item(_mob: &dyn Mob, brain: &Brain) -> bool {
    !brain.has_value::<NearestVisibleWantedItemMemory>()
}

/// `AllayAi.getItemDepositPosition` (`AllayAi.java:109-120`): the liked note block if it is
/// still valid, otherwise the liked player.
fn get_item_deposit_position(mob: &dyn Mob, brain: &Brain) -> Option<PositionTracker> {
    if let Some(noteblock_pos) = brain.get::<LikedNoteblockPositionMemory>() {
        if should_deposit_items_at_liked_noteblock(mob, brain, noteblock_pos) {
            return Some(PositionTracker::of_block(noteblock_pos.up()));
        }
        brain.erase::<LikedNoteblockPositionMemory>();
    }
    get_liked_player_position_tracker(mob, brain)
}

/// `AllayAi.shouldDepositItemsAtLikedNoteblock` (`AllayAi.java:127-134`).
/// `GlobalPos.isCloseEnough` is a **chessboard** distance compare (`GlobalPos.java:31-33`), not
/// Euclidean, and its dimension-equality half is not representable here (see module comment).
fn should_deposit_items_at_liked_noteblock(
    mob: &dyn Mob,
    brain: &Brain,
    noteblock_pos: BlockPos,
) -> bool {
    let entity = &mob.get_mob_entity().living_entity.entity;
    let mob_block_pos = entity.block_pos.load();
    let chessboard_distance = (noteblock_pos.0.x - mob_block_pos.0.x)
        .abs()
        .max((noteblock_pos.0.y - mob_block_pos.0.y).abs())
        .max((noteblock_pos.0.z - mob_block_pos.0.z).abs());
    if chessboard_distance > MAX_NOTEBLOCK_DISTANCE {
        return false;
    }
    if entity.world.load().get_block(&noteblock_pos) != &pumpkin_data::Block::NOTE_BLOCK {
        return false;
    }
    brain.has_value::<LikedNoteblockCooldownTicksMemory>()
}

/// `AllayAi.getLikedPlayerPositionTracker` / `getLikedPlayer` (`AllayAi.java:136-155`).
fn get_liked_player_position_tracker(mob: &dyn Mob, brain: &Brain) -> Option<PositionTracker> {
    let uuid = brain.get::<LikedPlayerMemory>()?;
    let entity = &mob.get_mob_entity().living_entity.entity;
    let player = entity.world.load().get_player_by_uuid(uuid)?;

    if !matches!(
        player.gamemode.load(),
        GameMode::Survival | GameMode::Creative
    ) {
        return None;
    }
    let distance_sq = player
        .living_entity
        .entity
        .pos
        .load()
        .squared_distance_to_vec(&entity.pos.load());
    if distance_sq > LIKED_PLAYER_MAX_DISTANCE * LIKED_PLAYER_MAX_DISTANCE {
        return None;
    }

    let player: Arc<dyn EntityBase> = player;
    Some(PositionTracker::of_entity(&player, true))
}

/// `AllayAi.onItemThrown` (`AllayAi.java:157-164`), minus the advancement trigger.
/// `SoundEvents.ALLAY_THROW` is the registry name `entity.allay.item_thrown`
/// (`SoundEvents.java:30`).
fn on_item_thrown(mob: &dyn Mob, _item: &ItemStack, _target_pos: BlockPos, game_time: i64) {
    if game_time % 7 != 0 {
        return;
    }
    let mut rng = mob.get_random();
    if rng.random::<f64>() >= 0.9 {
        return;
    }
    let index = rng.random_range(0..THROW_SOUND_PITCHES.len());
    let entity = &mob.get_mob_entity().living_entity.entity;
    entity.world.load().play_sound_raw(
        Sound::EntityAllayItemThrown as u16,
        SoundCategory::Neutral,
        &entity.pos.load(),
        1.0,
        THROW_SOUND_PITCHES[index],
    );
}

impl NBTStorage for AllayEntity {}

impl Mob for AllayEntity {
    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    fn should_follow_leash(&self) -> bool {
        false
    }

    /// `Allay.canPickUpLoot` (`Allay.java:262-265`).
    fn can_pick_up_loot(&self) -> bool {
        !self.is_on_pickup_cooldown() && self.has_item_in_hand()
    }

    /// `Allay.wantsToPickUp` (`Allay.java:340-347`).
    fn wants_to_pick_up_item(&self, world: &World, stack: &ItemStack) -> bool {
        let held = self.item_in_hand.lock().unwrap().clone();
        !held.is_empty()
            && world.level_info.load().game_rules.mob_griefing
            && self.inventory_can_add(stack)
            && Self::considers_item_equal(&held, stack)
    }

    /// `InventoryCarrier.pickUpItem` (`Allay.java:360-362`): the stack goes into the single
    /// inventory slot, up to that slot's maximum.
    fn on_item_pickup(&self, stack: &ItemStack) -> u8 {
        let mut slot = self.inventory.lock().unwrap();
        if slot.is_empty() {
            let taken = stack.item_count.min(stack.get_max_stack_size());
            *slot = stack.copy_with_count(taken);
            return taken;
        }
        if !slot.are_items_and_components_equal(stack) {
            return 0;
        }
        let room = slot.get_max_stack_size().saturating_sub(slot.item_count);
        let taken = stack.item_count.min(room);
        slot.item_count += taken;
        taken
    }

    fn carried_inventory_is_empty(&self) -> bool {
        self.inventory.lock().unwrap().is_empty()
    }

    fn remove_one_carried_item(&self) -> ItemStack {
        self.inventory.lock().unwrap().split(1)
    }

    fn mob_tick<'a>(&'a self, _caller: &'a Arc<dyn EntityBase>) -> EntityBaseFuture<'a, ()> {
        Box::pin(async move {
            self.register_listeners_once().await;

            // `AllayAi.updateActivity` (`AllayAi.java:92-94`), called from
            // `Allay.customServerAiStep` right after the brain ticks.
            self.brain()
                .set_active_activity_to_first_valid(&[Activity::Idle]);

            let age = self
                .mob_entity
                .living_entity
                .entity
                .age
                .load(Ordering::Relaxed);

            // `Allay.aiStep`: heal 1 HP every 10 ticks while alive.
            if age % 10 == 0 && self.mob_entity.living_entity.entity.is_alive() {
                self.mob_entity.living_entity.heal(1.0);
            }

            // `Allay.aiStep`'s `shouldStopDancing` check runs every 20 ticks.
            if self.is_dancing() && age % 20 == 0 {
                let pos = self.mob_entity.living_entity.entity.pos.load();
                if self.should_stop_dancing(pos) {
                    self.set_dancing(false);
                    *self.jukebox_pos.lock().unwrap() = None;
                }
            }

            // `Allay.updateDuplicationCooldown`
            if self.duplication_cooldown.load(Ordering::Relaxed) > 0 {
                self.duplication_cooldown.fetch_sub(1, Ordering::Relaxed);
            }
        })
    }

    fn mob_interact<'a>(
        &'a self,
        player: &'a Arc<Player>,
        item_stack: &'a mut ItemStack,
    ) -> EntityBaseFuture<'a, bool> {
        Box::pin(async move {
            let world = self.mob_entity.living_entity.entity.world.load_full();
            let held = self.item_in_hand.lock().unwrap().clone();
            let my_pos = self.mob_entity.living_entity.entity.pos.load();

            // `Allay.mobInteract`, dancing + DUPLICATES_ALLAYS + canDuplicate branch.
            if self.is_dancing()
                && item_stack
                    .item
                    .has_tag(&tag::Item::MINECRAFT_DUPLICATES_ALLAYS)
                && !self.is_on_duplication_cooldown()
            {
                self.duplicate().await;
                world.play_sound(
                    Sound::BlockAmethystBlockChime,
                    SoundCategory::Neutral,
                    &my_pos,
                );
                item_stack.decrement(1);
                return true;
            }

            // Empty-handed Allay + player holding an item: give it to the Allay.
            if held.is_empty() && !item_stack.is_empty() {
                self.set_item_in_hand(item_stack.copy_with_count(1)).await;
                item_stack.decrement(1);
                world.play_sound(Sound::EntityAllayItemGiven, SoundCategory::Neutral, &my_pos);
                self.brain()
                    .set::<LikedPlayerMemory>(player.get_entity().entity_uuid);
                return true;
            }

            // Allay holding an item + player empty-handed: take it back, and release whatever
            // the Allay had already collected (`Allay.java:306-308`).
            if !held.is_empty() && item_stack.is_empty() {
                self.set_item_in_hand(ItemStack::EMPTY.clone()).await;
                world.play_sound(Sound::EntityAllayItemTaken, SoundCategory::Neutral, &my_pos);
                self.brain().erase::<LikedPlayerMemory>();

                let collected = std::mem::replace(
                    &mut *self.inventory.lock().unwrap(),
                    ItemStack::EMPTY.clone(),
                );
                if !collected.is_empty() {
                    let mut collected = collected;
                    if !player.inventory.insert_stack_anywhere(&mut collected).await {
                        player.drop_item(collected).await;
                    }
                }

                let mut taken = held;
                if !player.inventory.insert_stack_anywhere(&mut taken).await {
                    player.drop_item(taken).await;
                }
                return true;
            }

            false
        })
    }
}

/// `Allay.VibrationUser` collapsed onto this project's `GameEventListener` trait.
struct AllayVibrationListener {
    allay: Weak<AllayEntity>,
    uuid: Uuid,
}

impl GameEventListener for AllayVibrationListener {
    fn listener_source(&self) -> PositionSource {
        PositionSource::Entity(self.uuid)
    }

    fn listener_radius(&self) -> i32 {
        // Allay.VibrationUser.VIBRATION_EVENT_LISTENER_RANGE
        16
    }

    fn handle_game_event<'a>(
        &'a self,
        _world: &'a Arc<World>,
        event: &'a GameEvent,
        _context: &'a GameEventContext,
        source_position: Vector3<f64>,
    ) -> GameEventFuture<'a> {
        Box::pin(async move {
            let Some(allay) = self.allay.upgrade() else {
                return false;
            };
            // GameEventTags.ALLAY_CAN_LISTEN is just `note_block_play`
            // (pumpkin-data/src/generated/tag.rs, GameEvent::MINECRAFT_ALLAY_CAN_LISTEN).
            if !matches!(event, GameEvent::NoteBlockPlay) {
                return false;
            }
            if allay.mob_entity.is_no_ai() {
                return false;
            }
            let pos = BlockPos::new(
                source_position.x.floor() as i32,
                source_position.y.floor() as i32,
                source_position.z.floor() as i32,
            );
            allay.hear_noteblock(pos);
            true
        })
    }
}

/// `Allay.JukeboxListener`.
struct AllayJukeboxListener {
    allay: Weak<AllayEntity>,
    uuid: Uuid,
}

impl GameEventListener for AllayJukeboxListener {
    fn listener_source(&self) -> PositionSource {
        PositionSource::Entity(self.uuid)
    }

    fn listener_radius(&self) -> i32 {
        // GameEvent.JUKEBOX_PLAY.value().notificationRadius() == 10
        10
    }

    fn handle_game_event<'a>(
        &'a self,
        _world: &'a Arc<World>,
        event: &'a GameEvent,
        _context: &'a GameEventContext,
        source_position: Vector3<f64>,
    ) -> GameEventFuture<'a> {
        Box::pin(async move {
            let Some(allay) = self.allay.upgrade() else {
                return false;
            };
            let pos = BlockPos::new(
                source_position.x.floor() as i32,
                source_position.y.floor() as i32,
                source_position.z.floor() as i32,
            );
            match event {
                GameEvent::JukeboxPlay => {
                    allay.set_jukebox_playing(pos, true);
                    true
                }
                GameEvent::JukeboxStopPlay => {
                    allay.set_jukebox_playing(pos, false);
                    true
                }
                _ => false,
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duplication_cooldown_constant_matches_vanilla() {
        assert_eq!(DUPLICATION_COOLDOWN_TICKS, 6000);
    }

    #[test]
    fn time_to_forget_noteblock_constant_matches_vanilla() {
        assert_eq!(TIME_TO_FORGET_NOTEBLOCK_TICKS, 600);
    }

    #[test]
    fn considers_item_equal_compares_by_item_id() {
        let a = ItemStack::new(1, &pumpkin_data::item::Item::DIAMOND);
        let b = ItemStack::new(3, &pumpkin_data::item::Item::DIAMOND);
        let c = ItemStack::new(1, &pumpkin_data::item::Item::EMERALD);
        assert!(AllayEntity::considers_item_equal(&a, &b));
        assert!(!AllayEntity::considers_item_equal(&a, &c));
    }

    /// The brain must come up with CORE and IDLE both active, or nothing in the idle activity
    /// ever gets a `try_start` and the Allay never moves.
    #[test]
    fn brain_starts_with_core_and_idle_active() {
        let brain = AllayEntity::make_brain();
        assert!(brain.is_active(Activity::Core));
        assert!(brain.is_active(Activity::Idle));
    }

    /// `AllayAi.shouldDepositItemsAtLikedNoteblock` requires the cooldown memory to be
    /// *present* (`AllayAi.java:133`), which is what makes `CountDownCooldownTicks`'
    /// erase-on-stop the mechanism that forgets a note block.
    #[test]
    fn noteblock_memories_are_registered_and_start_empty() {
        let brain = AllayEntity::make_brain();
        assert!(!brain.has_value::<LikedNoteblockPositionMemory>());
        assert!(!brain.has_value::<LikedNoteblockCooldownTicksMemory>());
        brain.set::<LikedNoteblockPositionMemory>(BlockPos::new(1, 2, 3));
        assert_eq!(
            brain.get::<LikedNoteblockPositionMemory>(),
            Some(BlockPos::new(1, 2, 3))
        );
    }
}
