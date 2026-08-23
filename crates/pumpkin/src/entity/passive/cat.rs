// Legacy invariant checks retained for vanilla behavior; migrate these paths before removing this allow.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use std::sync::{
    Arc, Weak,
    atomic::{AtomicBool, AtomicU8, Ordering},
};

use pumpkin_data::entity::{EntityPose, EntityStatus, EntityType};
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::tag::{self, Taggable};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_protocol::bedrock::server::actor_event::ActorEventType;
use pumpkin_protocol::codec::var_int::VarInt;
use pumpkin_protocol::java::client::play::Metadata;
use rand::RngExt;
use uuid::Uuid;

use crate::block::entities::sign::DyeColor;
use crate::entity::{
    Entity, EntityBase, EntityBaseFuture, NBTStorage, NbtFuture,
    ai::goal::{
        avoid_entity::AvoidEntityGoal, breed::BreedGoal, cat_lie_on_bed::CatLieOnBedGoal,
        cat_relax_on_owner::CatRelaxOnOwnerGoal, cat_sit_on_block::CatSitOnBlockGoal,
        escape_danger::EscapeDangerGoal, follow_owner::FollowOwnerGoal,
        leap_at_target::LeapAtTargetGoal, look_at_entity::LookAtEntityGoal,
        non_tame_random_target::NonTameRandomTargetGoal, ocelot_attack::OcelotAttackGoal,
        sit::SitGoal, swim::SwimGoal, tempt::TemptGoal, wander_around::WanderAroundGoal,
    },
    mob::{Mob, MobEntity},
    passive::{
        animal::Animal,
        tamable::{TamableAnimal, TamableData},
    },
    player::Player,
};

const TEMPT_ITEMS: &[&Item] = &[&Item::COD, &Item::SALMON];

// Vanilla Cat.java: `public static final DyeColor DEFAULT_COLLAR_COLOR = DyeColor.RED;`
const DEFAULT_COLLAR_COLOR: u8 = DyeColor::Red as u8;

/// Vanilla `Cat.CatAvoidEntityGoal`'s flee/walk/sprint-speed constants
/// (`new Cat.CatAvoidEntityGoal<>(this, Player.class, 16.0F, 0.8, 1.33)`).
const AVOID_PLAYER_DISTANCE: f64 = 16.0;
const AVOID_PLAYER_SLOW_SPEED: f64 = 0.8;
const AVOID_PLAYER_FAST_SPEED: f64 = 1.33;

const NATURAL_CAT_VARIANTS: [&str; 10] = [
    "tabby",
    "black",
    "red",
    "siamese",
    "british_shorthair",
    "calico",
    "persian",
    "ragdoll",
    "white",
    "jellie",
];

/// Moon brightness per lunar phase index (0 = full moon), from vanilla
/// `DimensionType.MOON_BRIGHTNESS_PER_PHASE`.
const MOON_BRIGHTNESS_PER_PHASE: [f32; 8] = [1.0, 0.75, 0.5, 0.25, 0.0, 0.25, 0.5, 0.75];

/// `MoonBrightnessCheck` gate used by the `all_black` cat variant's spawn selector.
///
/// `CatVariants.java` pairs `all_black` with `MinMaxBounds.Doubles.atLeast(0.9)`
/// against the current moon phase's brightness, which only the full moon
/// (phase 0, brightness 1.0) satisfies.
#[must_use]
pub fn moon_brightness_allows_all_black(time_of_day: i64) -> bool {
    let phase = time_of_day.div_euclid(24000).rem_euclid(8) as usize;
    MOON_BRIGHTNESS_PER_PHASE[phase] >= 0.9
}

/// Picks a natural-spawn cat variant.
///
/// Replicates `CatVariants.bootstrap`'s `PriorityProvider.pick` outcome with
/// the `StructureCheck` selector for `all_black` (swamp-hut detection)
/// dropped: Pumpkin has no structure-manager lookup by structure tag. On a
/// full moon, `all_black` joins the uniform pool of the 10 base variants;
/// otherwise only the base variants are eligible.
#[must_use]
pub fn select_natural_cat_variant(time_of_day: i64) -> &'static str {
    if moon_brightness_allows_all_black(time_of_day) {
        const POOL: [&str; 11] = [
            "tabby",
            "black",
            "red",
            "siamese",
            "british_shorthair",
            "calico",
            "persian",
            "ragdoll",
            "white",
            "jellie",
            "all_black",
        ];
        POOL[rand::random_range(0..POOL.len())]
    } else {
        NATURAL_CAT_VARIANTS[rand::random_range(0..NATURAL_CAT_VARIANTS.len())]
    }
}

fn get_dye_color_from_item(item: &Item) -> Option<u8> {
    let key = item.registry_key;
    if key.contains("white") {
        Some(0)
    } else if key.contains("orange") {
        Some(1)
    } else if key.contains("magenta") {
        Some(2)
    } else if key.contains("light_blue") {
        Some(3)
    } else if key.contains("yellow") {
        Some(4)
    } else if key.contains("lime") {
        Some(5)
    } else if key.contains("pink") {
        Some(6)
    } else if key.contains("light_gray") {
        Some(8)
    } else if key.contains("gray") {
        Some(7)
    } else if key.contains("cyan") {
        Some(9)
    } else if key.contains("purple") {
        Some(10)
    } else if key.contains("blue") {
        Some(11)
    } else if key.contains("brown") {
        Some(12)
    } else if key.contains("green") {
        Some(13)
    } else if key.contains("red") {
        Some(14)
    } else if key.contains("black") {
        Some(15)
    } else {
        None
    }
}

pub struct CatEntity {
    pub mob_entity: MobEntity,
    pub variant: AtomicU8,
    pub sound_variant: AtomicU8,
    pub collar_color: AtomicU8,
    pub tamable_data: TamableData,
    pub is_lying: AtomicBool,
    pub relax_state_one: AtomicBool,
}

impl CatEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let mob_entity = MobEntity::new(entity);
        let cat = Self {
            mob_entity,
            variant: AtomicU8::new(1),       // Default to black
            sound_variant: AtomicU8::new(0), // Default to classic
            collar_color: AtomicU8::new(DEFAULT_COLLAR_COLOR), // Vanilla Cat.DEFAULT_COLLAR_COLOR
            tamable_data: TamableData::default(),
            is_lying: AtomicBool::new(false),
            relax_state_one: AtomicBool::new(false),
        };
        let mob_arc = Arc::new(cat);
        let mob_weak: Weak<dyn Mob> = {
            let mob_arc: Arc<dyn Mob> = mob_arc.clone();
            Arc::downgrade(&mob_arc)
        };
        let cat_weak: Weak<Self> = Arc::downgrade(&mob_arc);

        {
            let mut goal_selector = mob_arc
                .mob_entity
                .goals_selector
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);

            // Goal 1: SwimGoal (FloatGoal)
            goal_selector.add_goal(1, Box::new(SwimGoal::default()));
            // Goal 1: TamableAnimalPanicGoal (EscapeDangerGoal)
            goal_selector.add_goal(1, EscapeDangerGoal::new(1.5));
            goal_selector.add_goal(2, SitGoal::new());
            // Cat.java:110 -- Goal 3: `Cat.CatRelaxOnOwnerGoal`.
            goal_selector.add_goal(3, CatRelaxOnOwnerGoal::new(cat_weak.clone()));
            goal_selector.add_goal(4, Box::new(TemptGoal::new(0.6, TEMPT_ITEMS, true)));
            // Vanilla priority 4, `Cat.CatAvoidEntityGoal<Player>`: only present while untamed
            // (added here since a freshly spawned cat always starts untamed; removed by
            // `reassess_tame_goals` -- see `mob_interact`'s taming branch below -- once the cat
            // is tamed, mirroring vanilla's `reassessTameGoals`). The
            // `EntitySelector.NO_CREATIVE_OR_SPECTATOR` filter vanilla applies isn't ported --
            // `AvoidEntityGoal` has no selector-predicate hook here -- so this cat will
            // (harmlessly) also flee from creative/spectator players.
            goal_selector.add_goal(
                4,
                Box::new(AvoidEntityGoal::new(
                    &EntityType::PLAYER,
                    AVOID_PLAYER_DISTANCE,
                    AVOID_PLAYER_SLOW_SPEED,
                    AVOID_PLAYER_FAST_SPEED,
                )),
            );
            // Cat.java:112 -- Goal 5: `CatLieOnBedGoal(this, 1.1, 8)`.
            goal_selector.add_goal(5, CatLieOnBedGoal::new(cat_weak.clone(), 1.1));
            goal_selector.add_goal(6, FollowOwnerGoal::new(1.0, 10.0, 5.0));
            // Cat.java:114 -- Goal 7: `CatSitOnBlockGoal(this, 0.8)`.
            goal_selector.add_goal(7, CatSitOnBlockGoal::new(cat_weak, 0.8));
            // Cat.java:115 -- Goal 8: `LeapAtTargetGoal(this, 0.3F)`.
            goal_selector.add_goal(8, LeapAtTargetGoal::new(0.3));
            goal_selector.add_goal(9, Box::new(OcelotAttackGoal::new()));
            // Cat.java:117 -- Goal 10: `BreedGoal(this, 0.8)`. Previously registered at 5, which
            // put breeding ahead of lying on a bed and following its owner.
            goal_selector.add_goal(10, BreedGoal::new(0.8));
            // No `FollowParentGoal` is registered: `Cat.registerGoals` (Cat.java:105-123) has
            // none, and neither does any supertype it inherits from.
            // Cat.java:118 -- Goal 11: WaterAvoidingRandomStrollGoal(this, 0.8, 1.0000001E-5F)
            goal_selector.add_goal(
                11,
                Box::new(WanderAroundGoal::new_water_avoiding_with_probability(
                    0.8, 0.00001,
                )),
            );
            // Cat.java:119 -- Goal 12: LookAtPlayerGoal(this, Player.class, 10.0F)
            goal_selector.add_goal(
                12,
                LookAtEntityGoal::with_default(mob_weak, &EntityType::PLAYER, 10.0),
            );
            // No `RandomLookAroundGoal`: `Cat.registerGoals` (Cat.java:105-123) does not add one.

            let mut target_selector = mob_arc.mob_entity.target_selector.lock().unwrap();
            target_selector.add_goal(
                1,
                NonTameRandomTargetGoal::without_predicate(
                    &mob_arc.mob_entity,
                    &[&EntityType::RABBIT],
                    false,
                ),
            );
            target_selector.add_goal(
                1,
                NonTameRandomTargetGoal::new(
                    &mob_arc.mob_entity,
                    crate::entity::ai::goal::non_tame_random_target::TURTLE_TYPES,
                    false,
                    Some(crate::entity::ai::goal::non_tame_random_target::baby_turtle_on_land),
                ),
            );
        };

        mob_arc
    }

    pub fn get_tame_flags(&self) -> u8 {
        let mut flags = 0u8;
        if self.is_in_sitting_pose() {
            flags |= 0x01;
        }
        if self.is_tame() {
            flags |= 0x04;
        }
        flags
    }

    pub fn is_sitting(&self) -> bool {
        self.is_in_sitting_pose()
    }

    pub fn is_lying(&self) -> bool {
        self.is_lying.load(Ordering::Relaxed)
    }

    pub fn is_relax_state_one(&self) -> bool {
        self.relax_state_one.load(Ordering::Relaxed)
    }

    pub fn get_collar_color(&self) -> u8 {
        self.collar_color.load(Ordering::Relaxed)
    }

    pub fn set_collar_color(&self, color: u8) {
        self.collar_color.store(color, Ordering::Relaxed);
        let entity = self.get_entity();
        entity.send_meta_data(
            &[Metadata::new(
                pumpkin_data::tracked_data::cat::CAT_COLLAR_COLOR,
                VarInt(color as i32),
            )],
            None,
        );
    }

    /// Vanilla `Cat::reassessTameGoals`: removes the flee-from-players goal once the cat is
    /// tamed (a still-untamed cat never has anything to remove, so this is only meaningfully
    /// called from `mob_interact`'s taming branch). Uses the take/put-back pattern `mob_tick`
    /// itself uses (see `mob/mod.rs`) since `GoalSelector` sits behind a non-async `Mutex` that
    /// can't be held across the `.await` `remove_goal` needs.
    async fn reassess_tame_goals(&self) {
        let mut goal_selector = {
            let mut guard = self.mob_entity.goals_selector.lock().unwrap();
            std::mem::take(&mut *guard)
        };
        goal_selector.remove_goal::<AvoidEntityGoal>(self).await;
        *self.mob_entity.goals_selector.lock().unwrap() = goal_selector;
    }

    pub fn set_sitting(&self, sitting: bool) {
        self.set_in_sitting_pose(sitting);
        let entity = self.get_entity();
        entity.send_meta_data(
            &[Metadata::new(
                pumpkin_data::tracked_data::cat::TAMEABLE_FLAGS,
                self.get_tame_flags(),
            )],
            None,
        );
    }

    pub fn set_tame(&self, tame: bool, owner: Option<Uuid>) {
        self.tamable_data.is_tame.store(tame, Ordering::Relaxed);
        self.set_owner(owner);
        let entity = self.get_entity();
        entity.send_meta_data(
            &[Metadata::new(
                pumpkin_data::tracked_data::cat::TAMEABLE_FLAGS,
                self.get_tame_flags(),
            )],
            None,
        );
        entity.send_meta_data(
            &[Metadata::new(
                pumpkin_data::tracked_data::cat::OWNER_UUID,
                owner,
            )],
            None,
        );
    }

    pub fn set_variant(&self, variant: u8) {
        self.variant.store(variant, Ordering::Relaxed);
        let entity = self.get_entity();
        entity.send_meta_data(
            &[Metadata::new(
                pumpkin_data::tracked_data::cat::CAT_VARIANT,
                VarInt(variant as i32),
            )],
            None,
        );
    }

    pub fn set_lying(&self, lying: bool) {
        self.is_lying.store(lying, Ordering::Relaxed);
        let entity = self.get_entity();
        entity.send_meta_data(
            &[Metadata::new(
                pumpkin_data::tracked_data::cat::IS_LYING,
                lying,
            )],
            None,
        );
    }

    pub fn set_relax_state_one(&self, relax: bool) {
        self.relax_state_one.store(relax, Ordering::Relaxed);
        let entity = self.get_entity();
        entity.send_meta_data(
            &[Metadata::new(
                pumpkin_data::tracked_data::cat::RELAX_STATE_ONE,
                relax,
            )],
            None,
        );
    }

    pub fn play_eating_sound(&self) {
        let mob_entity = self.get_mob_entity();
        let entity = &mob_entity.living_entity.entity;
        let world = entity.world.load();
        world.play_sound(
            pumpkin_data::sound::Sound::EntityCatEat,
            pumpkin_data::sound::SoundCategory::Neutral,
            &entity.pos.load(),
        );
    }
}

impl Animal for CatEntity {
    fn is_food(&self, item_stack: &ItemStack) -> bool {
        let item = item_stack.get_item();
        item.has_tag(&tag::Item::MINECRAFT_CAT_FOOD) || item == &Item::COD || item == &Item::SALMON
    }
}

impl TamableAnimal for CatEntity {
    fn get_tamable_data(&self) -> &TamableData {
        &self.tamable_data
    }
}

impl NBTStorage for CatEntity {
    fn write_nbt<'a>(&'a self, nbt: &'a mut NbtCompound) -> NbtFuture<'a, ()> {
        Box::pin(async {
            self.mob_entity.living_entity.write_nbt(nbt).await;
            self.write_animal_nbt(nbt);
            let variant_str = match self.variant.load(Ordering::Relaxed) {
                0 => "minecraft:all_black",
                1 => "minecraft:black",
                2 => "minecraft:british_shorthair",
                3 => "minecraft:calico",
                4 => "minecraft:jellie",
                5 => "minecraft:persian",
                6 => "minecraft:ragdoll",
                7 => "minecraft:red",
                8 => "minecraft:siamese",
                10 => "minecraft:white",
                _ => "minecraft:tabby",
            };
            nbt.put_string("variant", variant_str.to_string());
            // Vanilla Cat.java persists collar color as a legacy dye-color id (0-15), the same
            // "CollarColor" key/codec Wolf uses.
            nbt.put_byte(
                "CollarColor",
                self.collar_color.load(Ordering::Relaxed) as i8,
            );
            // Vanilla `TamableAnimal.addAdditionalSaveData` (TamableAnimal.java:58-63) stores only
            // the owner reference and `Sitting`; tameness is derived from the owner on load.
            self.write_tamable_nbt(nbt);
        })
    }

    fn read_nbt_non_mut<'a>(&'a self, nbt: &'a NbtCompound) -> NbtFuture<'a, ()> {
        Box::pin(async {
            self.mob_entity.living_entity.read_nbt_non_mut(nbt).await;
            self.read_animal_nbt(nbt);
            if let Some(variant_str) = nbt.get_string("variant") {
                let variant = match variant_str
                    .strip_prefix("minecraft:")
                    .unwrap_or(variant_str)
                {
                    "all_black" => 0,
                    "black" => 1,
                    "british_shorthair" => 2,
                    "calico" => 3,
                    "jellie" => 4,
                    "persian" => 5,
                    "ragdoll" => 6,
                    "red" => 7,
                    "siamese" => 8,
                    "white" => 10,
                    _ => 9,
                };
                self.variant.store(variant, Ordering::Relaxed);
            }
            if let Some(collar) = nbt.get_byte("CollarColor") {
                self.collar_color.store(collar as u8, Ordering::Relaxed);
            } else if let Some(collar_int) = nbt.get_int("CollarColor") {
                self.collar_color.store(collar_int as u8, Ordering::Relaxed);
            }
            // Vanilla `TamableAnimal.readAdditionalSaveData` (TamableAnimal.java:66-83) derives
            // tameness purely from the stored owner reference, and
            // `EntityReference.readWithOldOwnerConversion` accepts both the current UUID
            // representation and the legacy owner name. Resolve the latter through the
            // server user cache, just as vanilla resolves old owner names to profiles.
            let owner = if let Some(owner) = nbt.get_uuid("Owner") {
                Some(owner)
            } else if let Some(owner_name) = nbt.get_string("Owner") {
                let owner_name = owner_name.to_owned();
                let world = self.mob_entity.living_entity.entity.world.load();
                if let Some(server) = world.server.upgrade() {
                    server
                        .data
                        .user_cache
                        .write()
                        .await
                        .get_by_name(&owner_name)
                        .map(|profile| profile.uuid)
                } else {
                    None
                }
            } else {
                None
            };

            self.tamable_data.owner.store(owner);
            self.tamable_data
                .is_tame
                .store(owner.is_some(), Ordering::Relaxed);
            // `SitGoal` and `MobEntity::is_tamed` read the shared `MobEntity` taming state, so
            // keep it in step with the cat-local `tamable_data` the metadata path uses.
            if let Some(owner) = owner {
                self.mob_entity.set_owner(owner);
            } else {
                self.mob_entity.clear_owner();
            }
            let sitting = nbt.get_bool("Sitting").unwrap_or(false);
            self.tamable_data
                .ordered_to_sit
                .store(sitting, Ordering::Relaxed);
            self.mob_entity.set_ordered_to_sit(sitting);

            // Vanilla calls `reassessTameGoals` from `setTame` during load, so a restored
            // tamed cat must lose its untamed player-avoidance goal before it starts ticking.
            if self.is_tame() {
                self.reassess_tame_goals().await;
            }
        })
    }
}

/// Speed a feline's `AvoidEntityGoal`/`TemptGoal` requests while stalking; the pose step reads
/// it back out of the move control. Vanilla compares against the literal `0.6`.
pub const FELINE_CROUCH_SPEED: f64 = 0.6;
/// Speed a fleeing feline is given; vanilla compares against the literal `1.33`.
pub const FELINE_SPRINT_SPEED: f64 = 1.33;

/// Pose and sprint flag a feline should hold, per its move control.
///
/// Vanilla `Cat.customServerAiStep` (`Cat.java:234-251`) and the byte-identical
/// `Ocelot.customServerAiStep` (`Ocelot.java:116-133`): a feline crouches while creeping at
/// `0.6`, sprints while fleeing at `1.33`, and stands otherwise. The comparison is an exact
/// float equality in vanilla, so a goal's requested speed must match these constants bit for
/// bit to select anything but the standing branch.
#[must_use]
pub fn feline_pose_for(has_wanted: bool, speed_modifier: f64) -> (EntityPose, bool) {
    if !has_wanted {
        return (EntityPose::Standing, false);
    }
    if speed_modifier == FELINE_CROUCH_SPEED {
        (EntityPose::Crouching, false)
    } else {
        (EntityPose::Standing, speed_modifier == FELINE_SPRINT_SPEED)
    }
}

/// Applies [`feline_pose_for`] to a live cat or ocelot.
pub async fn feline_pose_step(mob: &dyn Mob) {
    let mob_entity = mob.get_mob_entity();
    let (has_wanted, speed) = {
        let control = mob_entity.move_control.lock().unwrap();
        (control.has_wanted(), control.get_speed_modifier())
    };

    let entity = &mob_entity.living_entity.entity;
    let (pose, sprinting) = feline_pose_for(has_wanted, speed);

    // Vanilla calls both setters unconditionally every tick; `SynchedEntityData` is dirty-tracked
    // there, while `send_meta_data`/`set_flag` here broadcast whatever they are handed. Guarding
    // on change keeps the observable result identical without a per-tick packet per feline.
    if entity.pose.load() != pose {
        entity.set_pose(pose);
    }
    if entity.is_sprinting() != sprinting {
        entity.set_sprinting(sprinting).await;
    }
}

impl Mob for CatEntity {
    // Upstream addition: exposes this cat generically as `Animal`/`TamableAnimal` (e.g. for
    // shared taming/leash goals that only hold `&dyn Mob`), matching the pattern already used
    // by `WolfEntity`.
    fn as_animal(&self) -> Option<&dyn Animal> {
        Some(self)
    }

    fn as_tamable(&self) -> Option<&dyn TamableAnimal> {
        Some(self)
    }

    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    fn mob_tick<'a>(&'a self, _caller: &'a Arc<dyn EntityBase>) -> EntityBaseFuture<'a, ()> {
        Box::pin(async move {
            feline_pose_step(self).await;
        })
    }

    fn get_owner_uuid(&self) -> Option<Uuid> {
        self.get_owner()
    }

    fn is_sitting(&self) -> bool {
        self.is_ordered_to_sit()
    }

    fn mob_set_variant_name(&self, name: &str) {
        let variant = match name.strip_prefix("minecraft:").unwrap_or(name) {
            "all_black" => 0,
            "black" => 1,
            "british_shorthair" => 2,
            "calico" => 3,
            "jellie" => 4,
            "persian" => 5,
            "ragdoll" => 6,
            "red" => 7,
            "siamese" => 8,
            "white" => 10,
            _ => 9,
        };
        self.variant.store(variant, Ordering::Relaxed);
    }

    fn mob_init_data_tracker(&self) -> EntityBaseFuture<'_, ()> {
        Box::pin(async move {
            let entity = self.get_entity();
            let is_baby = entity.age.load(Ordering::Relaxed) < 0;
            if is_baby {
                entity.send_meta_data(
                    &[Metadata::new(
                        pumpkin_data::tracked_data::cat::BABY_ID,
                        true,
                    )],
                    None,
                );
            }
            entity.send_meta_data(
                &[Metadata::new(
                    pumpkin_data::tracked_data::cat::TAMEABLE_FLAGS,
                    self.get_tame_flags(),
                )],
                None,
            );
            entity.send_meta_data(
                &[Metadata::new(
                    pumpkin_data::tracked_data::cat::OWNER_UUID,
                    self.get_owner(),
                )],
                None,
            );
            entity.send_meta_data(
                &[Metadata::new(
                    pumpkin_data::tracked_data::cat::CAT_VARIANT,
                    VarInt(self.variant.load(Ordering::Relaxed) as i32),
                )],
                None,
            );
            entity.send_meta_data(
                &[Metadata::new(
                    pumpkin_data::tracked_data::cat::IS_LYING,
                    self.is_lying.load(Ordering::Relaxed),
                )],
                None,
            );
            entity.send_meta_data(
                &[Metadata::new(
                    pumpkin_data::tracked_data::cat::RELAX_STATE_ONE,
                    self.relax_state_one.load(Ordering::Relaxed),
                )],
                None,
            );
            entity.send_meta_data(
                &[Metadata::new(
                    pumpkin_data::tracked_data::cat::CAT_COLLAR_COLOR,
                    VarInt(self.collar_color.load(Ordering::Relaxed) as i32),
                )],
                None,
            );
            entity.send_meta_data(
                &[Metadata::new(
                    pumpkin_data::tracked_data::cat::SOUND_VARIANT,
                    VarInt(self.sound_variant.load(Ordering::Relaxed) as i32),
                )],
                None,
            );
        })
    }

    fn mob_interact<'a>(
        &'a self,
        player: &'a Arc<Player>,
        item_stack: &'a mut ItemStack,
    ) -> EntityBaseFuture<'a, bool> {
        Box::pin(async move {
            let item = item_stack.get_item();
            let is_food = self.is_food(item_stack);

            if self.is_tame() {
                if self.get_owner_uuid() == Some(player.gameprofile.id) {
                    if item.has_tag(&tag::Item::MINECRAFT_CAT_COLLAR_DYES)
                        || item.has_tag(&tag::Item::C_DYES)
                    {
                        if let Some(color) = get_dye_color_from_item(item)
                            && color != self.get_collar_color()
                        {
                            self.set_collar_color(color);
                            item_stack.decrement_unless_creative(player.gamemode.load(), 1);
                            return true;
                        }
                    } else if is_food
                        && self.mob_entity.living_entity.health.load()
                            < self.mob_entity.living_entity.get_max_health()
                    {
                        item_stack.decrement_unless_creative(player.gamemode.load(), 1);
                        self.mob_entity.living_entity.heal(2.0);
                        self.play_eating_sound();
                        return true;
                    }

                    let parent_interaction = self
                        .mob_entity
                        .mob_interact(player, item_stack, self.can_be_leashed())
                        .await;
                    if !parent_interaction {
                        self.set_sitting(!self.is_sitting());
                        return true;
                    }
                    return parent_interaction;
                }
            } else if is_food {
                item_stack.decrement_unless_creative(player.gamemode.load(), 1);
                self.play_eating_sound();

                // `ThreadRng` is not `Send`, so the roll has to finish before the
                // `reassess_tame_goals` await below. Vanilla `Cat.mobInteract`:
                // `this.random.nextInt(3) == 0`.
                let tames = rand::rng().random_range(0..3) == 0;
                if tames {
                    self.set_tame(true, Some(player.gameprofile.id));
                    self.set_sitting(true);
                    // Vanilla `Cat.setTame` -> `reassessTameGoals`: a tamed cat stops
                    // fleeing from players.
                    self.reassess_tame_goals().await;
                    self.get_entity().world.load().send_entity_status(
                        self.get_entity(),
                        EntityStatus::TamingSucceeded,
                        Some(ActorEventType::TamingSucceeded),
                    );
                } else {
                    self.get_entity().world.load().send_entity_status(
                        self.get_entity(),
                        EntityStatus::TamingFailed,
                        Some(ActorEventType::TamingFailed),
                    );
                }

                return true;
            }

            self.mob_entity
                .mob_interact(player, item_stack, self.can_be_leashed())
                .await
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{
        FELINE_CROUCH_SPEED, FELINE_SPRINT_SPEED, feline_pose_for,
        moon_brightness_allows_all_black, select_natural_cat_variant,
    };
    use pumpkin_data::entity::EntityPose;

    /// `Cat.java:235-251` / `Ocelot.java:117-133`: only a wanted destination at exactly 0.6
    /// crouches, only exactly 1.33 sprints, and everything else stands still and unsprinting.
    ///
    /// `EntityPose` has no `Debug`, so poses are compared through their discriminant.
    fn pose_for(has_wanted: bool, speed: f64) -> (u8, bool) {
        let (pose, sprinting) = feline_pose_for(has_wanted, speed);
        (pose as u8, sprinting)
    }

    #[test]
    fn feline_pose_matches_vanilla_speed_branches() {
        let crouching = EntityPose::Crouching as u8;
        let standing = EntityPose::Standing as u8;
        assert_eq!(pose_for(true, FELINE_CROUCH_SPEED), (crouching, false));
        assert_eq!(pose_for(true, FELINE_SPRINT_SPEED), (standing, true));
        assert_eq!(pose_for(true, 1.0), (standing, false));
        // No wanted position outranks the speed entirely.
        assert_eq!(pose_for(false, FELINE_CROUCH_SPEED), (standing, false));
        assert_eq!(pose_for(false, FELINE_SPRINT_SPEED), (standing, false));
    }

    /// A near-miss speed must not crouch or sprint: vanilla's comparison is exact equality.
    #[test]
    fn feline_pose_speed_comparison_is_exact() {
        let standing = EntityPose::Standing as u8;
        assert_eq!(pose_for(true, 0.6001), (standing, false));
        assert_eq!(pose_for(true, 1.3299), (standing, false));
    }

    #[test]
    fn only_full_moon_allows_all_black() {
        for phase in 0..8 {
            let time_of_day = phase * 24000;
            let allowed = moon_brightness_allows_all_black(time_of_day);
            assert_eq!(
                allowed,
                phase == 0,
                "phase {phase} should only allow all_black on full moon"
            );
        }
    }

    #[test]
    fn phase_wraps_across_multiple_lunar_cycles() {
        assert!(moon_brightness_allows_all_black(8 * 24000));
        assert!(!moon_brightness_allows_all_black(9 * 24000));
    }

    #[test]
    fn all_black_only_selectable_on_full_moon() {
        // Day 4 (new moon, brightness 0.0) is never a full moon.
        for _ in 0..200 {
            assert_ne!(select_natural_cat_variant(4 * 24000), "all_black");
        }
        let mut saw_all_black = false;
        for _ in 0..500 {
            if select_natural_cat_variant(0) == "all_black" {
                saw_all_black = true;
                break;
            }
        }
        assert!(saw_all_black);
    }
}
