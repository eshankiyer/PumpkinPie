// Legacy invariant checks retained for vanilla behavior; migrate these paths before removing this allow.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicI32, Ordering, Ordering::Relaxed},
};

use pumpkin_data::attributes::Attributes;
use pumpkin_data::data_component_impl::{EquipmentSlot, EquippableImpl, IDSet, IdOr};
use pumpkin_data::entity::EntityType;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_data::tag::{self, Taggable};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_protocol::java::client::play::Metadata;
use pumpkin_protocol::java::server::play::SPlayerInput;
use pumpkin_util::math::boundingbox::{BoundingBox, EntityDimensions};
use pumpkin_util::math::vector3::Vector3;

use crate::entity::{
    Entity, EntityBase, EntityBaseFuture, NBTStorage, NbtFuture,
    ageable::{AgeableData, AgeableMob},
    ai::brain::behavior::gate::GateBehavior,
    ai::brain::behavior::look_at_target_sink::LookAtTargetSink,
    ai::brain::behavior::move_to_target_sink::MoveToTargetSink,
    ai::brain::behavior::random_stroll::RandomStrollFly,
    ai::brain::behavior::set_walk_target_from_look_target::SetWalkTargetFromLookTarget,
    ai::brain::behavior::{animal_panic::AnimalPanic, swim::Swim},
    ai::brain::{Activity, ActivityData, Brain},
    ai::control::{flying_move_control::FlyingMoveControl, ghast_move_control::GhastMoveControl},
    ai::goal::{ghast_random_float::GhastRandomFloatAroundGoal, swim::SwimGoal, tempt::TemptGoal},
    mob::{Mob, MobEntity},
    passive::animal::Animal,
    player::Player,
};

/// What tempts a happy ghast: the snowball.
///
/// Vanilla's tempt set is the harness-color tags plus the snowball
/// (`ItemTags.HAPPY_GHAST_TEMPT_ITEMS`); Pumpkin's `TemptGoal` only accepts a
/// static item list, not a tag/predicate, so this is narrowed to the snowball,
/// matching `ItemTags.HAPPY_GHAST_FOOD` (the baby feed/growth tag) exactly.
pub const HAPPY_GHAST_FOOD: &[&Item] = &[&Item::SNOWBALL];

const HEAL_INTERVAL_TICKS: i32 = 600;

/// `HappyGhast.checkFallDamage` (`HappyGhast.java:167-168`) is an empty override, so a
/// happy ghast never converts a fall into damage.
const fn happy_ghast_fall_damage(_fall_distance: f64, _damage_modifier: f32) -> i32 {
    0
}

/// `HappyGhast.getVoicePitch` (`HappyGhast.java:205-207`) always returns the neutral pitch.
const fn happy_ghast_voice_pitch() -> f32 {
    1.0
}

/// `HappyGhast.java:448-450`.
#[must_use]
pub const fn restriction_radius(is_baby: bool, has_body_armor: bool) -> i32 {
    if !is_baby && !has_body_armor { 64 } else { 32 }
}

/// `AABB.contains` (`AABB.java:259-265`) includes the minimum edge and excludes the maximum.
fn happy_ghast_detection_box_contains(box_: &BoundingBox, position: Vector3<f64>) -> bool {
    position.x >= box_.min.x
        && position.x < box_.max.x
        && position.y >= box_.min.y
        && position.y < box_.max.y
        && position.z >= box_.min.z
        && position.z < box_.max.z
}

/// Represents a Happy Ghast, a passive flying mob that can be equipped with a
/// dyed Harness and ridden.
///
/// Wiki: <https://minecraft.wiki/w/Happy_Ghast>
pub struct HappyGhastEntity {
    pub mob_entity: MobEntity,
    pub ageable_data: AgeableData,
    heal_ticks: AtomicI32,
    pub server_still_timeout: AtomicI32,
    pub leash_holder_time: AtomicI32,
    pub is_leash_holder: AtomicBool,
    pub stays_still: AtomicBool,
    baby_setup: AtomicBool,
}

impl HappyGhastEntity {
    pub(crate) fn can_breathe_underwater(&self) -> bool {
        self.is_baby()
    }
    pub fn new(entity: Entity) -> Arc<Self> {
        let mut mob_entity = MobEntity::new(entity);
        // `HappyGhast.BRAIN_PROVIDER` and `HappyGhastAi.getActivities`
        // (`HappyGhast.java:400-409`, `HappyGhastAi.java:23-66`) give babies the shared
        // passive-flying Brain activity ladder after the adult goals are removed.
        mob_entity.brain = Some(Self::make_brain());
        let happy_ghast = Self {
            mob_entity,
            ageable_data: AgeableData::default(),
            heal_ticks: AtomicI32::new(0),
            server_still_timeout: AtomicI32::new(0),
            leash_holder_time: AtomicI32::new(0),
            is_leash_holder: AtomicBool::new(false),
            stays_still: AtomicBool::new(false),
            baby_setup: AtomicBool::new(false),
        };

        let mob_arc = Arc::new(happy_ghast);

        {
            let mut goal_selector = mob_arc
                .mob_entity
                .goals_selector
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);

            // `HappyGhast.registerGoals` (`HappyGhast.java:102-118`) registers exactly three
            // goals, at priorities 3/4/5. No look-at-player or random-look-around goal appears
            // in that list -- the head yaw is driven by `HappyGhastLookControl`
            // (`HappyGhast.java:639-660`) instead, so both are dropped here.
            //
            // Divergence: vanilla registers this set only for adults. `babyGhastSetup`
            // (`HappyGhast.java:132-138`) calls `removeAllGoals(goal -> true)` and registers
            // nothing, and `adultGhastSetup` (`HappyGhast.java:120-129`) clears and re-registers
            // on the age boundary. This codebase registers once at construction, so a baby happy
            // ghast still runs the adult goal list.
            //
            // `HappyGhast.HappyGhastFloatGoal` (`HappyGhast.java:628-637`) is a `FloatGoal`
            // whose `canUse` additionally requires `!isOnStillTimeout()`; that extra gate is not
            // modelled by this codebase's `SwimGoal`.
            goal_selector.add_goal(3, Box::new(SwimGoal::default()));
            // `TemptGoal.ForNonPathfinders(this, 1.0, <tag predicate>, false, 7.0)`
            // (`HappyGhast.java:104-118`). Vanilla picks `HAPPY_GHAST_TEMPT_ITEMS` for an
            // unarmored adult and `HAPPY_GHAST_FOOD` otherwise; see `HAPPY_GHAST_FOOD` above for
            // why the single-list approximation is used.
            goal_selector.add_goal(
                4,
                Box::new(TemptGoal::with_stop_distance(
                    1.0,
                    HAPPY_GHAST_FOOD,
                    false,
                    7.0,
                )),
            );
            // `Ghast.RandomFloatAroundGoal(this, 16)` (`HappyGhast.java:117`). Happy ghasts fly,
            // so the previous ground-pathing `WanderAroundGoal` was wrong. Divergence: this
            // codebase's goal is the `distanceToBlocks == 0` form (see
            // `ghast_random_float.rs`), so vanilla's 16-block clearance check on the chosen
            // target position is not applied.
            goal_selector.add_goal(5, Box::new(GhastRandomFloatAroundGoal::new()));
        };

        // `HappyGhast` starts with the adult controller. A baby loaded or spawned after
        // construction switches to `FlyingMoveControl` on its first server tick, matching
        // `HappyGhast.babyGhastSetup` (`HappyGhast.java:132-138`).
        *mob_arc
            .mob_entity
            .move_control
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            Box::new(GhastMoveControl::default());

        mob_arc
    }

    /// `HappyGhastAi.initCoreActivity` / `initIdleActivity` (`HappyGhastAi.java:23-66`).
    /// The adult-only goal selector remains separate; the Brain is used after baby setup clears
    /// those goals, matching the two vanilla age-specific AI paths.
    fn make_brain() -> Brain {
        Brain::new(
            Vec::new(),
            vec![
                ActivityData::create(
                    Activity::Core,
                    0,
                    vec![
                        Swim::new(0.8),
                        AnimalPanic::new(2.0),
                        LookAtTargetSink::new(45, 90),
                        MoveToTargetSink::new(),
                    ],
                ),
                ActivityData::create(
                    Activity::Idle,
                    0,
                    vec![GateBehavior::run_one(vec![
                        (RandomStrollFly::new(1.0), 2),
                        (SetWalkTargetFromLookTarget::new(1.0, 3), 2),
                    ])],
                ),
            ],
        )
    }

    /// `HappyGhast.customServerAiStep` (`HappyGhast.java:400-409`) ticks the Brain only while
    /// the ghast is a baby; adults use the goal selector registered by `registerGoals`.
    const fn should_tick_brain_for_age(age: i32) -> bool {
        age < 0
    }

    fn should_tick_brain_now(&self) -> bool {
        Self::should_tick_brain_for_age(self.mob_entity.living_entity.entity.age.load(Relaxed))
    }

    async fn body_armor_stack(&self) -> ItemStack {
        self.mob_entity
            .living_entity
            .entity_equipment
            .lock()
            .await
            .get(&EquipmentSlot::BODY)
    }

    async fn is_wearing_body_armor(&self) -> bool {
        !self.body_armor_stack().await.is_empty()
    }

    fn is_harness(item_stack: &ItemStack) -> Option<&EquippableImpl> {
        let equippable = item_stack.get_data_component::<EquippableImpl>()?;
        (*equippable.slot == EquipmentSlot::BODY
            && equippable.equip_on_interact
            && matches!(&equippable.allowed_entities, Some(IDSet::Tag(tag)) if tag.as_ref() == "can_equip_harness"))
        .then_some(equippable)
    }

    async fn setup_adult(&self) {
        // `HappyGhast.adultGhastSetup` (`HappyGhast.java:120-129`) replaces the flying
        // controller and clears/re-registers the server goals after the age boundary.
        *self
            .mob_entity
            .move_control
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            Box::new(GhastMoveControl::default());
        // `Mob.removeFreeWill` (`Mob.java:1417-1421`) is the live reset used before adult goals
        // are registered.
        self.mob_entity.remove_free_will(self).await;
        self.mob_entity.add_goal(3, SwimGoal::default());
        self.mob_entity.add_goal(
            4,
            TemptGoal::with_stop_distance(1.0, HAPPY_GHAST_FOOD, false, 7.0),
        );
        self.mob_entity
            .add_goal(5, GhastRandomFloatAroundGoal::new());
    }

    async fn setup_baby(&self) {
        // `HappyGhast.babyGhastSetup` (`HappyGhast.java:132-138`) has no goals and uses
        // `FlyingMoveControl(this, 180, true)`.
        *self
            .mob_entity
            .move_control
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            Box::new(FlyingMoveControl::new(180.0, true));
        self.set_server_still_timeout(0);
        // `Mob.removeFreeWill` (`Mob.java:1417-1421`) is the live reset for the baby goal set.
        self.mob_entity.remove_free_will(self).await;
    }

    async fn try_equip_harness(&self, player: &Arc<Player>, item_stack: &mut ItemStack) -> bool {
        let Some(equippable) = Self::is_harness(item_stack) else {
            return false;
        };

        if self.is_wearing_body_armor().await {
            return false;
        }

        let equip_sound = match &equippable.equip_sound {
            IdOr::Id(sound) => *sound,
            IdOr::Value(_) => Sound::ItemArmorEquipGeneric,
        };

        let new_stack = item_stack.split_unless_creative(player.gamemode.load(), 1);
        {
            let mut equipment = self.mob_entity.living_entity.entity_equipment.lock().await;
            equipment.put(&EquipmentSlot::BODY, new_stack.clone())
        };
        self.mob_entity
            .living_entity
            .send_equipment_changes(&[(EquipmentSlot::BODY, new_stack)]);

        let entity = &self.mob_entity.living_entity.entity;
        let world = entity.world.load();
        world.play_sound(equip_sound, SoundCategory::Neutral, &entity.pos.load());
        true
    }

    async fn try_mount(&self, player: &Arc<Player>) -> bool {
        if !self.is_wearing_body_armor().await || !player.get_entity().can_start_riding().await {
            return false;
        }

        let entity = &self.mob_entity.living_entity.entity;
        let passengers_len = entity.passengers.lock().await.len();
        // HappyGhast.java:339, MAX_PASSENGERS (vanilla misspells the constant name).
        if passengers_len >= 4 {
            return false;
        }

        let world = entity.world.load();
        let Some(vehicle) = world.get_entity_by_id(entity.entity_id) else {
            return false;
        };
        let Some(passenger) = world.get_player_by_id(player.entity_id()) else {
            return false;
        };

        if passengers_len == 0 {
            world.play_sound(
                Sound::EntityHappyGhastHarnessGogglesDown,
                SoundCategory::Neutral,
                &entity.pos.load(),
            );
        }

        entity
            .add_passenger(vehicle, passenger as Arc<dyn EntityBase>)
            .await;
        true
    }

    fn continuous_heal(&self) {
        let living = &self.mob_entity.living_entity;
        if living.dead.load(Relaxed) {
            return;
        }

        if living.health.load() >= living.get_max_health() {
            self.heal_ticks.store(0, Relaxed);
            return;
        }

        let ticks = self.heal_ticks.fetch_add(1, Relaxed) + 1;
        if ticks >= HEAL_INTERVAL_TICKS {
            self.heal_ticks.store(0, Relaxed);
            living.heal(1.0);
        }
    }

    // HappyGhast.java:452-459. Only called from `mob_tick` while not a vehicle
    // (`this.isVehicle()` there).
    async fn check_restriction(&self) {
        let entity = &self.mob_entity.living_entity.entity;
        if entity.leashed_to.lock().await.is_some() {
            return;
        }

        let is_baby = self.is_baby();
        let has_body_armor = self.is_wearing_body_armor().await;

        let radius = restriction_radius(is_baby, has_body_armor);
        self.mob_entity
            .position_target
            .store(entity.block_pos.load());
        self.mob_entity.position_target_range.store(radius, Relaxed);
    }

    /// `HappyGhast.scanPlayerAboveGhast` (`HappyGhast.java:547-568`) detects a non-spectator
    /// player's root vehicle in the box above the ghast. The caller refreshes the still timeout
    /// on the same tick as vanilla (`HappyGhast.java:432-434`).
    async fn scan_player_above_ghast(&self) -> bool {
        let entity = &self.mob_entity.living_entity.entity;
        let ghast_box = entity.bounding_box.load();
        let detection_box = BoundingBox {
            min: Vector3::new(
                ghast_box.min.x - 1.0,
                ghast_box.max.y - 1.0e-5,
                ghast_box.min.z - 1.0,
            ),
            max: Vector3::new(
                ghast_box.max.x + 1.0,
                ghast_box.max.y + f64::from(entity.entity_dimension.load().height) / 2.0,
                ghast_box.max.z + 1.0,
            ),
        };

        let players = entity.world.load().players.load().clone();
        for player in players.iter() {
            if player.is_spectator() {
                continue;
            }

            let mut root_vehicle: Arc<dyn EntityBase> = player.clone();
            loop {
                let Some(vehicle) = root_vehicle.get_entity().vehicle.lock().await.clone() else {
                    break;
                };
                root_vehicle = vehicle;
            }

            let root_entity = root_vehicle.get_entity();
            let position = root_entity.pos.load();
            if root_entity.entity_type != &EntityType::HAPPY_GHAST
                && happy_ghast_detection_box_contains(&detection_box, position)
            {
                return true;
            }
        }

        false
    }

    pub fn set_server_still_timeout(&self, timeout: i32) {
        self.server_still_timeout.store(timeout, Ordering::Relaxed);
        self.sync_stay_still_flag();
    }

    #[must_use]
    pub fn is_on_still_timeout(&self) -> bool {
        self.stays_still.load(Ordering::Relaxed)
            || self.server_still_timeout.load(Ordering::Relaxed) > 0
    }

    fn set_leash_holder(&self, is_leash_holder: bool) {
        self.is_leash_holder
            .store(is_leash_holder, Ordering::Relaxed);
        let entity = self.get_entity();
        entity.send_meta_data(
            &[Metadata::new(
                pumpkin_data::tracked_data::happy_ghast::IS_LEASH_HOLDER,
                is_leash_holder,
            )],
            None,
        );
    }

    fn sync_stay_still_flag(&self) {
        let stays_still = self.server_still_timeout.load(Ordering::Relaxed) > 0;
        self.stays_still.store(stays_still, Ordering::Relaxed);
        let entity = self.get_entity();
        entity.send_meta_data(
            &[Metadata::new(
                pumpkin_data::tracked_data::happy_ghast::STAYS_STILL,
                stays_still,
            )],
            None,
        );
    }
}

impl AgeableMob for HappyGhastEntity {
    fn get_ageable_data(&self) -> &AgeableData {
        &self.ageable_data
    }

    fn baby_dimensions(&self) -> Option<EntityDimensions> {
        Some(EntityDimensions::new(0.95, 0.95, 0.46875))
    }
}

impl NBTStorage for HappyGhastEntity {
    fn write_nbt<'a>(&'a self, nbt: &'a mut NbtCompound) -> NbtFuture<'a, ()> {
        Box::pin(async move {
            self.mob_entity.living_entity.write_nbt(nbt).await;
            self.write_ageable_nbt(nbt);
            self.write_animal_nbt(nbt);
            nbt.put_int(
                "still_timeout",
                self.server_still_timeout.load(Ordering::Relaxed),
            );
        })
    }

    fn read_nbt_non_mut<'a>(&'a self, nbt: &'a NbtCompound) -> NbtFuture<'a, ()> {
        Box::pin(async move {
            self.mob_entity.living_entity.read_nbt_non_mut(nbt).await;
            self.read_ageable_nbt(nbt);
            self.read_animal_nbt(nbt);
            if let Some(timeout) = nbt.get_int("still_timeout") {
                self.set_server_still_timeout(timeout);
            }
        })
    }
}

impl Animal for HappyGhastEntity {
    fn is_food(&self, item_stack: &ItemStack) -> bool {
        item_stack
            .item
            .has_tag(&tag::Item::MINECRAFT_HAPPY_GHAST_FOOD)
            || HAPPY_GHAST_FOOD.iter().any(|i| i.id == item_stack.item.id)
    }
}

impl Mob for HappyGhastEntity {
    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    /// `HappyGhast.customServerAiStep` (`HappyGhast.java:400-409`) runs the Brain for babies;
    /// adults use the goal selector registered by `registerGoals`.
    fn should_tick_brain(&self) -> bool {
        self.should_tick_brain_now()
    }

    fn should_follow_leash(&self) -> bool {
        false
    }

    /// `HappyGhast.notifyLeashHolder` refreshes the five-tick holder flag only for a
    /// quad-leash-capable leashed entity. The concrete `supportQuadLeash` overrides are
    /// `AbstractBoat.java:365-367`, `Sniffer.java:153-155`, `Llama.java:418-420`, and
    /// `AbstractHorse.java:198-200` (`HappyGhast.java:494-495,525-528`).
    fn notify_leash_holder(&self, entity: &dyn EntityBase) {
        let entity_type = entity.get_entity().entity_type.id;
        let supports_quad_leash = entity_type == EntityType::ACACIA_BOAT.id
            || entity_type == EntityType::BIRCH_BOAT.id
            || entity_type == EntityType::DARK_OAK_BOAT.id
            || entity_type == EntityType::JUNGLE_BOAT.id
            || entity_type == EntityType::MANGROVE_BOAT.id
            || entity_type == EntityType::OAK_BOAT.id
            || entity_type == EntityType::PALE_OAK_BOAT.id
            || entity_type == EntityType::SPRUCE_BOAT.id
            || entity_type == EntityType::BAMBOO_RAFT.id
            || entity_type == EntityType::CHERRY_BOAT.id
            || entity_type == EntityType::ACACIA_CHEST_BOAT.id
            || entity_type == EntityType::BAMBOO_CHEST_RAFT.id
            || entity_type == EntityType::BIRCH_CHEST_BOAT.id
            || entity_type == EntityType::CHERRY_CHEST_BOAT.id
            || entity_type == EntityType::DARK_OAK_CHEST_BOAT.id
            || entity_type == EntityType::JUNGLE_CHEST_BOAT.id
            || entity_type == EntityType::MANGROVE_CHEST_BOAT.id
            || entity_type == EntityType::OAK_CHEST_BOAT.id
            || entity_type == EntityType::PALE_OAK_CHEST_BOAT.id
            || entity_type == EntityType::SPRUCE_CHEST_BOAT.id
            || entity_type == EntityType::HORSE.id
            || entity_type == EntityType::DONKEY.id
            || entity_type == EntityType::MULE.id
            || entity_type == EntityType::SKELETON_HORSE.id
            || entity_type == EntityType::ZOMBIE_HORSE.id
            || entity_type == EntityType::LLAMA.id
            || entity_type == EntityType::TRADER_LLAMA.id
            || entity_type == EntityType::SNIFFER.id;
        if supports_quad_leash {
            self.leash_holder_time.store(5, Relaxed);
        }
    }

    fn can_be_collided_with(&self) -> bool {
        // HappyGhast.canBeCollidedWith only exposes adult, alive ghasts to
        // collision queries. Babies must remain non-collidable while growing.
        !self.is_baby() && self.get_entity().is_alive()
    }

    fn get_mob_gravity(&self) -> f64 {
        0.0
    }

    /// `HappyGhast.checkFallDamage` (`HappyGhast.java:167-168`) is an empty override. The
    /// shared movement path already skips fall-distance accumulation, and this hook also keeps
    /// direct `causeFallDamage` calls from producing damage.
    fn mob_calculate_fall_damage(&self, fall_distance: f64, damage_modifier: f32) -> i32 {
        happy_ghast_fall_damage(fall_distance, damage_modifier)
    }

    fn custom_travel<'a>(&'a self, caller: &'a Arc<dyn EntityBase>) -> EntityBaseFuture<'a, bool> {
        Box::pin(async move {
            let living = &self.mob_entity.living_entity;
            let entity = &living.entity;
            let flying_speed = living.get_attribute_value(&Attributes::FLYING_SPEED);

            // HappyGhast.getRiddenInput/getRiddenRotation/tickRidden
            // (`HappyGhast.java:350-387`) are folded into this reachable travel hook. The
            // packet's input flags are the server representation of the rider's xxa/zza/jumping.
            let rider_input = {
                let passengers = entity.passengers.lock().await;
                passengers.first().and_then(|passenger| {
                    passenger.get_player().map(|rider| {
                        (
                            rider.last_input.load(Relaxed),
                            rider.get_entity().pitch.load(),
                            rider.get_entity().yaw.load(),
                        )
                    })
                })
            };
            let movement_input = if !self.is_on_still_timeout()
                && self.is_wearing_body_armor().await
                && let Some((input, pitch, yaw)) = rider_input
            {
                // `ServerPlayer.getLastClientMoveIntent` (`ServerPlayer.java:2231`): left is
                // positive strafe, right is negative.
                let mut strafe = 0.0;
                if input & SPlayerInput::LEFT != 0 {
                    strafe += 1.0;
                }
                if input & SPlayerInput::RIGHT != 0 {
                    strafe -= 1.0;
                }

                let mut up = 0.0;
                let mut forward = 0.0;
                if input & (SPlayerInput::FORWARD | SPlayerInput::BACKWARD) != 0 {
                    let pitch_radians = f64::from(pitch).to_radians();
                    forward = pitch_radians.cos();
                    up = -pitch_radians.sin();
                    if input & SPlayerInput::BACKWARD != 0 {
                        forward *= -0.5;
                        up *= -0.5;
                    }
                }
                if input & SPlayerInput::JUMP != 0 {
                    up += 0.5;
                }

                let diff = pumpkin_util::math::wrap_degrees(yaw - entity.yaw.load());
                let new_yaw = entity.yaw.load() + diff * 0.08;
                entity.yaw.store(new_yaw);
                entity.pitch.store(pitch * 0.5);
                entity.head_yaw.store(new_yaw);
                entity.body_yaw.store(new_yaw);

                Vector3::new(strafe, up, forward) * (3.9 * flying_speed)
            } else {
                living.movement_input.load()
            };

            // `HappyGhast.travel` (`HappyGhast.java:176-179`) always uses travelFlying with
            // FLYING_SPEED * 5/3, including in water and lava. `travelFlying` applies the
            // corresponding 0.8/0.5/0.91 velocity drag (`LivingEntity.java:2439-2457`).
            living
                .entity
                .update_velocity_from_input(movement_input, flying_speed * (5.0 / 3.0));
            let velocity = entity.velocity.load();
            entity.move_entity(caller, velocity).await;

            let drag = if entity.touching_water.load(Relaxed) {
                0.8
            } else if entity.touching_lava.load(Relaxed) {
                0.5
            } else {
                0.91
            };
            entity.velocity.store(entity.velocity.load() * drag);
            true
        })
    }

    fn get_ambient_sound(&self) -> Option<Sound> {
        Some(if self.is_baby() {
            Sound::EntityGhastlingAmbient
        } else {
            Sound::EntityHappyGhastAmbient
        })
    }

    fn get_hurt_sound(&self) -> Option<Sound> {
        Some(if self.is_baby() {
            Sound::EntityGhastlingHurt
        } else {
            Sound::EntityHappyGhastHurt
        })
    }

    /// `HappyGhast.getVoicePitch` (`HappyGhast.java:205-207`) overrides the normal random
    /// baby/adult mob pitch with a constant value.
    fn get_sound_pitch(&self) -> f32 {
        happy_ghast_voice_pitch()
    }

    fn mob_init_data_tracker(&self) -> EntityBaseFuture<'_, ()> {
        Box::pin(async move {
            let entity = self.get_entity();
            let is_baby = entity.age.load(Ordering::Relaxed) < 0;
            if is_baby {
                entity.send_meta_data(
                    &[Metadata::new(
                        pumpkin_data::tracked_data::happy_ghast::BABY_ID,
                        true,
                    )],
                    None,
                );
            }
            entity.send_meta_data(
                &[
                    Metadata::new(
                        pumpkin_data::tracked_data::happy_ghast::IS_LEASH_HOLDER,
                        self.is_leash_holder.load(Ordering::Relaxed),
                    ),
                    Metadata::new(
                        pumpkin_data::tracked_data::happy_ghast::STAYS_STILL,
                        self.stays_still.load(Ordering::Relaxed),
                    ),
                ],
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
            // HappyGhast.java:281-283: babies never equip/ride, only eat to grow up.
            if self.is_baby() {
                use crate::entity::passive::animal::Animal as _;
                return self
                    .animal_interact(player, item_stack, Sound::EntityGhastlingAmbient)
                    .await;
            }

            if !item_stack.is_empty() && self.try_equip_harness(player, item_stack).await {
                return true;
            }

            if self.try_mount(player).await {
                return true;
            }

            self.mob_entity
                .mob_interact(player, item_stack, self.can_be_leashed())
                .await
        })
    }

    fn mob_tick<'a>(&'a self, _caller: &'a Arc<dyn EntityBase>) -> EntityBaseFuture<'a, ()> {
        Box::pin(async move {
            if !self.mob_entity.living_entity.entity.is_alive() {
                return;
            }

            // `HappyGhastBodyRotationControl.clientTick` keeps a vehicle's head and body
            // aligned to its yaw (`HappyGhast.java:612-625`; `Mob.java:358-361`). Apply the same
            // invariant on the server's live mob tick before the shared head-turn pass.
            if !self
                .mob_entity
                .living_entity
                .entity
                .passengers
                .lock()
                .await
                .is_empty()
            {
                let yaw = self.get_entity().yaw.load();
                self.get_entity().head_yaw.store(yaw);
                self.get_entity().body_yaw.store(yaw);
            }

            // Vanilla `HappyGhast.aiStep` updates `requiresPrecisePosition` from
            // `isOnStillTimeout` before the base movement tick (`HappyGhast.java:439-445`).
            self.get_entity()
                .set_requires_precise_position(self.is_on_still_timeout());

            self.ageable_ai_step();

            if self.is_baby() {
                if !self.baby_setup.swap(true, Relaxed) {
                    self.setup_baby().await;
                }
            } else if self.baby_setup.swap(false, Relaxed) {
                self.setup_adult().await;
            }

            // `HappyGhastAi.updateActivity` (`HappyGhast.java:400-409`) selects the first valid
            // non-core activity after the baby Brain tick.
            if self.is_baby() {
                self.mob_entity
                    .brain
                    .as_ref()
                    .expect("HappyGhastEntity is always constructed with a brain")
                    .set_active_activity_to_first_valid(&[Activity::Idle]);
            }

            let leash_time = self.leash_holder_time.load(Relaxed);
            if leash_time > 0 {
                self.leash_holder_time.fetch_sub(1, Relaxed);
            }
            self.set_leash_holder(leash_time > 0);

            if self.server_still_timeout.load(Relaxed) > 0 {
                if self.get_entity().age.load(Relaxed) > 60 {
                    self.server_still_timeout.fetch_sub(1, Relaxed);
                }
                self.sync_stay_still_flag();
            }

            // `HappyGhast.tick` refreshes the rider-overhead grace timeout from
            // `scanPlayerAboveGhast` (`HappyGhast.java:432-434,547-568`).
            if self.scan_player_above_ghast().await {
                self.set_server_still_timeout(10);
            }

            // Vanilla `HappyGhast.aiStep` (`HappyGhast.java:438-442`) requests precise position
            // packets while the still-timeout is active, after its server AI step updates it.
            self.get_entity()
                .set_requires_precise_position(self.is_on_still_timeout());

            self.continuous_heal();

            let is_vehicle = !self
                .mob_entity
                .living_entity
                .entity
                .passengers
                .lock()
                .await
                .is_empty();
            if !is_vehicle {
                self.check_restriction().await;
            }
        })
    }
}

#[cfg(test)]
mod test {
    use super::{
        BoundingBox, HappyGhastEntity, happy_ghast_detection_box_contains, happy_ghast_fall_damage,
        happy_ghast_voice_pitch, restriction_radius,
    };
    use pumpkin_util::math::vector3::Vector3;

    #[test]
    fn restriction_radius_matches_vanilla() {
        assert_eq!(restriction_radius(false, false), 64);
        assert_eq!(restriction_radius(false, true), 32);
        assert_eq!(restriction_radius(true, false), 32);
        assert_eq!(restriction_radius(true, true), 32);
    }

    #[test]
    fn fall_damage_and_voice_pitch_match_vanilla() {
        assert_eq!(happy_ghast_fall_damage(100.0, 10.0), 0);
        assert_eq!(happy_ghast_voice_pitch(), 1.0);
    }

    // `HappyGhast.customServerAiStep` (`HappyGhast.java:400-409`) gates Brain ticking on age.
    #[test]
    fn brain_ticks_only_for_babies() {
        assert!(HappyGhastEntity::should_tick_brain_for_age(-1));
        assert!(!HappyGhastEntity::should_tick_brain_for_age(0));
    }

    // `HappyGhast.scanPlayerAboveGhast` uses `AABB.contains` (`HappyGhast.java:547-568`;
    // `AABB.java:259-265`).
    #[test]
    fn player_above_detection_uses_half_open_bounds() {
        let detection_box = BoundingBox {
            min: Vector3::new(-1.0, 2.0, -1.0),
            max: Vector3::new(1.0, 4.0, 1.0),
        };
        assert!(happy_ghast_detection_box_contains(
            &detection_box,
            Vector3::new(-1.0, 2.0, 0.0)
        ));
        assert!(!happy_ghast_detection_box_contains(
            &detection_box,
            Vector3::new(1.0, 2.0, 0.0)
        ));
        assert!(!happy_ghast_detection_box_contains(
            &detection_box,
            Vector3::new(0.0, 4.0, 0.0)
        ));
    }
}
