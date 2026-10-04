use super::{Mob, MobEntity, zombification};
use crate::entity::EntityBaseFuture;
use crate::entity::ageable::AgeableMob;
use crate::entity::ai::goal::destroy_egg::DestroyEggGoal;
use crate::entity::ai::goal::look_around::RandomLookAroundGoal;
use crate::entity::ai::goal::move_through_village::MoveThroughVillageGoal;
use crate::entity::ai::goal::non_tame_random_target::baby_turtle_on_land;
use crate::entity::ai::goal::revenge::RevengeGoal;
use crate::entity::ai::goal::spear_use::SpearUseGoal;
use crate::entity::ai::goal::swim::SwimGoal;
use crate::entity::ai::goal::wander_around::WanderAroundGoal;
use crate::entity::ai::goal::zombie_attack::ZombieAttackGoal;
use crate::entity::attributes::{Modifier, ModifierOperation};
use crate::entity::living::LivingEntity;
use crate::entity::mob::equipment::RegionalDifficulty;
use crate::entity::mob::zombie::zombie_villager::ZombieVillagerEntity;
use crate::entity::passive::villager::VillagerEntity;
use crate::entity::passive::villager::gossip::GossipContainer;
use crate::entity::r#type::{SpawnRuleContext, check_spawn_rules, from_type};
use crate::entity::{
    Entity, EntityBase, NBTStorage, NbtFuture,
    ai::goal::{active_target::ActiveTargetGoal, look_at_entity::LookAtEntityGoal},
};
use crate::world::World;
use crate::world::natural_spawner::{is_spawn_position_ok, spawn_dimensions};
use pumpkin_data::attributes::Attributes;
use pumpkin_data::entity::EntityType;
use pumpkin_data::tag;
use pumpkin_data::tracked_data;
use pumpkin_data::world::WorldEvent;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_protocol::java::client::play::Metadata;
use pumpkin_util::Difficulty;
use pumpkin_util::math::boundingbox::{BoundingBox, EntityDimensions};
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};
use uuid::Uuid;

pub mod drowned;
pub mod husk;
#[allow(clippy::module_inception)]
pub mod zombie;
pub mod zombie_villager;

pub struct ZombieEntityBase {
    pub mob_entity: MobEntity,
    /// Set by every `read_nbt_non_mut` on the zombie family. Vanilla's `finalizeSpawn` -- and
    /// so `Zombie::handleAttributes` (`Zombie.java:505`) -- never runs for an entity restored
    /// from disk, but Pumpkin calls `init_data_tracker` on both fresh spawns and chunk loads.
    /// NBT is read first in both paths, so this flag distinguishes the two and keeps a chunk
    /// reload from re-rolling leader status.
    restored_from_nbt: AtomicBool,
    /// Whether the last `Zombie::handleAttributes` roll made this a leader zombie
    /// (`Zombie.java:543`). Read by `ZombieEntity` to force door breaking
    /// (`Zombie.java:556`).
    pub is_leader: AtomicBool,
    /// Vanilla's synced `DATA_BABY_ID` (`Zombie.java:82`), the source of `Zombie::isBaby`.
    /// `Entity::age` cannot stand in for it: it is a tick counter for a zombie, not an
    /// `AgeableMob` breeding age.
    is_baby: AtomicBool,
    /// Vanilla `Zombie::canBreakDoors` (`Zombie.java:152-154`), the `BooleanSupplier` handed to
    /// `MoveThroughVillageGoal`. `ZombieEntity` keeps it in step with its door-breaking state;
    /// the variants that never roll one leave it `false`.
    can_break_doors: Arc<AtomicBool>,
}

impl ZombieEntityBase {
    pub fn new(entity: Entity) -> Arc<Self> {
        let mob_entity = MobEntity::new(entity);
        let zombie = Self {
            mob_entity,
            restored_from_nbt: AtomicBool::new(false),
            is_leader: AtomicBool::new(false),
            is_baby: AtomicBool::new(false),
            can_break_doors: Arc::new(AtomicBool::new(false)),
        };
        let can_break_doors = zombie.can_break_doors.clone();
        let mob_arc = Arc::new(zombie);
        let mob_weak: Weak<dyn Mob> = {
            let mob_arc: Arc<dyn Mob> = mob_arc.clone();
            Arc::downgrade(&mob_arc)
        };

        {
            let mut goal_selector = mob_arc
                .mob_entity
                .goals_selector
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let mut target_selector = mob_arc
                .mob_entity
                .target_selector
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);

            goal_selector.add_goal(0, Box::new(SwimGoal::default()));
            goal_selector.add_goal(2, SpearUseGoal::new(1.0, 1.0, 10.0, 2.0));
            goal_selector.add_goal(3, ZombieAttackGoal::new(1.0, false));
            goal_selector.add_goal(4, DestroyEggGoal::new(1.0, 3));
            // `Zombie.java:122`.
            goal_selector.add_goal(
                6,
                MoveThroughVillageGoal::new(1.0, true, 4, can_break_doors),
            );
            goal_selector.add_goal(7, Box::new(WanderAroundGoal::new_water_avoiding(1.0)));
            goal_selector.add_goal(
                8,
                LookAtEntityGoal::with_default(mob_weak, &EntityType::PLAYER, 8.0),
            );
            goal_selector.add_goal(8, Box::new(RandomLookAroundGoal::default()));

            // `Zombie.java:124` calls `setAlertOthers(ZombifiedPiglin.class)` on this goal; the
            // subclass-inclusive alert set is resolved from the hurt mob's type in `RevengeGoal`.
            target_selector.add_goal(1, Box::new(RevengeGoal::new(true).alert_others()));
            target_selector.add_goal(
                2,
                ActiveTargetGoal::with_default(&mob_arc.mob_entity, &EntityType::PLAYER, true),
            );
            target_selector.add_goal(
                3,
                // `Zombie#addBehaviourGoals` targets `AbstractVillager` with visibility
                // disabled. The concrete implementations in this version are villagers and
                // wandering traders.
                ActiveTargetGoal::with_default_types(
                    &mob_arc.mob_entity,
                    &[&EntityType::VILLAGER, &EntityType::WANDERING_TRADER],
                    false,
                ),
            );
            target_selector.add_goal(
                3,
                ActiveTargetGoal::with_default(&mob_arc.mob_entity, &EntityType::IRON_GOLEM, true),
            );
            target_selector.add_goal(
                5,
                Box::new(ActiveTargetGoal::new(
                    &mob_arc.mob_entity,
                    &EntityType::TURTLE,
                    10,
                    true,
                    false,
                    Some(baby_turtle_on_land),
                )),
            );
        };

        mob_arc
    }
}

impl ZombieEntityBase {
    /// Builds a base from an already-configured `MobEntity` (used by variants that register
    /// their own goal set instead of the shared zombie one).
    pub fn from_mob_entity(mob_entity: MobEntity) -> Self {
        Self {
            mob_entity,
            restored_from_nbt: AtomicBool::new(false),
            is_leader: AtomicBool::new(false),
            is_baby: AtomicBool::new(false),
            can_break_doors: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Marks this zombie as restored from disk, suppressing the fresh-spawn attribute roll.
    pub fn mark_restored_from_nbt(&self) {
        self.restored_from_nbt.store(true, Ordering::Relaxed);
    }

    /// Vanilla `Zombie::isBaby` (`Zombie.java:173-176`).
    pub fn is_baby(&self) -> bool {
        self.is_baby.load(Ordering::Relaxed)
    }

    /// Vanilla `Zombie::setBaby` (`Zombie.java:187-197`) together with the
    /// `onSyncedDataUpdated` reaction to `DATA_BABY_ID` (`Zombie.java:199-206`), which refreshes
    /// the dimensions: publishes the synced flag, swaps the `minecraft:baby` movement-speed
    /// modifier and resizes the hitbox.
    pub async fn set_baby(&self, baby: bool) {
        let living = &self.mob_entity.living_entity;
        let entity = &living.entity;
        self.is_baby.store(baby, Ordering::Relaxed);
        entity.send_meta_data(
            &[Metadata::new(tracked_data::zombie::DATA_BABY_ID, baby)],
            None,
        );

        // `speed.removeModifier(SPEED_MODIFIER_BABY_ID); if (baby) addTransientModifier(...)`.
        living.update_attribute(&Attributes::MOVEMENT_SPEED, |instance| {
            instance.remove_modifier(SPEED_MODIFIER_BABY_ID);
            if baby {
                instance.add_or_update_transient_modifier(Modifier {
                    id: SPEED_MODIFIER_BABY_ID.to_string(),
                    amount: SPEED_MODIFIER_BABY_AMOUNT,
                    operation: ModifierOperation::MultiplyBase,
                });
            }
        });
        crate::entity::attributes::send_attribute_updates_for_living(
            living,
            vec![Attributes::MOVEMENT_SPEED],
        )
        .await;

        // `Zombie.getDefaultDimensions` (`Zombie.java:437-440`) and the per-variant overrides.
        let dimensions = if baby {
            baby_dimensions(entity.entity_type)
        } else {
            EntityDimensions::new(
                entity.entity_type.dimension[0],
                entity.entity_type.dimension[1],
                entity.entity_type.eye_height,
            )
        };
        entity.base_dimension.store(dimensions);
        entity.entity_dimension.store(dimensions);
        let pos = entity.pos.load();
        entity
            .bounding_box
            .store(BoundingBox::new_from_pos(pos.x, pos.y, pos.z, &dimensions));
    }

    /// `Zombie::getBaseExperienceReward` (`Zombie.java:178-185`): a baby's reward is the type's
    /// scaled by 2.5 and truncated.
    pub fn base_experience_reward(&self) -> u32 {
        let base = self
            .mob_entity
            .living_entity
            .entity
            .entity_type
            .experience_reward;
        if self.is_baby() {
            baby_experience_reward(base)
        } else {
            base
        }
    }

    /// `LivingEntity::getVoicePitch` (`LivingEntity.java:2321-2325`), which reads `isBaby`.
    pub fn voice_pitch(&self) -> f32 {
        voice_pitch(self.is_baby(), rand::random(), rand::random())
    }

    /// Runs `Zombie::handleAttributes` (`Zombie.java:531-558`) once, and only for a genuine
    /// fresh spawn, recording the leader outcome in `is_leader`.
    pub async fn roll_spawn_attributes(&self) {
        if self.restored_from_nbt.load(Ordering::Relaxed) {
            return;
        }
        let entity = &self.mob_entity.living_entity.entity;
        let world = entity.world.load_full();
        let difficulty = RegionalDifficulty::at(&world, entity.pos.load());
        let is_leader = handle_attributes(
            &self.mob_entity.living_entity,
            difficulty.special_multiplier,
            true,
        )
        .await;
        self.is_leader.store(is_leader, Ordering::Relaxed);
    }

    /// The `handleAttributes(specialMultiplier, EntitySpawnReason.CONVERSION)` call that
    /// `Zombie::convertToZombieType` (`Zombie.java:247-253`) and
    /// `Zombie::convertVillagerToZombieVillager` (`Zombie.java:256-274`, through `finalizeSpawn`)
    /// run on the replacement. A converted zombie keeps its health, so a leader roll does not
    /// heal (`Zombie.java:552-554`).
    async fn handle_conversion_attributes(&self) {
        let entity = &self.mob_entity.living_entity.entity;
        let world = entity.world.load_full();
        let difficulty = RegionalDifficulty::at(&world, entity.pos.load());
        let is_leader = handle_attributes(
            &self.mob_entity.living_entity,
            difficulty.special_multiplier,
            false,
        )
        .await;
        self.is_leader.store(is_leader, Ordering::Relaxed);
    }
}

impl NBTStorage for ZombieEntityBase {
    /// `Zombie::addAdditionalSaveData` (`Zombie.java:399-405`), its `IsBaby` half.
    fn write_nbt<'a>(&'a self, nbt: &'a mut NbtCompound) -> NbtFuture<'a, ()> {
        Box::pin(async move {
            self.mob_entity.living_entity.write_nbt(nbt).await;
            nbt.put_bool("IsBaby", self.is_baby());
        })
    }

    /// `Zombie::readAdditionalSaveData` (`Zombie.java:407-419`), its `IsBaby` half:
    /// `setBaby(getBooleanOr("IsBaby", false))`. This is also what a spawner or `/summon` tag such
    /// as `IsBaby:1b` goes through.
    fn read_nbt_non_mut<'a>(&'a self, nbt: &'a NbtCompound) -> NbtFuture<'a, ()> {
        Box::pin(async move {
            self.mark_restored_from_nbt();
            self.mob_entity.living_entity.read_nbt_non_mut(nbt).await;
            let baby = nbt.get_bool("IsBaby").unwrap_or(false);
            if baby != self.is_baby() {
                self.set_baby(baby).await;
            }
        })
    }
}

impl Mob for ZombieEntityBase {
    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    /// Adds `Zombie::finalizeSpawn`'s baby roll (`Zombie.java:463-469`) and `handleAttributes`
    /// call (`Zombie.java:505`) to the spawn path. Both are `finalizeSpawn` work, so a zombie
    /// restored from disk or built by a conversion skips them.
    ///
    /// `groupData == null` is assumed for every spawn: `ZombieGroupData` is not plumbed through
    /// the natural spawner, so a pack rolls its members' baby status one by one instead of
    /// sharing the first member's roll (`Zombie.java:463-465`). The chicken jockey that follows a
    /// baby roll (`Zombie.java:470-490`) is not modeled: Pumpkin has no chicken-jockey state and
    /// nothing can mount a mob from inside `World::spawn_entity`.
    fn mob_init_data_tracker(&self) -> EntityBaseFuture<'_, ()> {
        Box::pin(async move {
            if !self.restored_from_nbt.load(Ordering::Relaxed)
                && spawn_as_baby_odds(rand::random::<f32>())
            {
                self.set_baby(true).await;
            }
            self.roll_spawn_attributes().await;
        })
    }

    fn get_base_experience_reward(&self) -> u32 {
        self.base_experience_reward()
    }

    fn get_sound_pitch(&self) -> f32 {
        self.voice_pitch()
    }
}

/// The common surface of the four `Zombie` subclasses (`Zombie`, `Husk`, `Drowned`,
/// `ZombieVillager`), which share [`ZombieEntityBase`] the way vanilla shares the superclass.
pub trait ZombieFamily {
    fn zombie_base(&self) -> &ZombieEntityBase;
}

/// `Zombie.SPEED_MODIFIER_BABY_ID` (`Zombie.java:72`).
const SPEED_MODIFIER_BABY_ID: &str = "minecraft:baby";
/// `Zombie.SPEED_MODIFIER_BABY` amount (`Zombie.java:73-75`), `ADD_MULTIPLIED_BASE`.
const SPEED_MODIFIER_BABY_AMOUNT: f64 = 0.5;
/// `Zombie.getSpawnAsBabyOdds`' threshold (`Zombie.java:527-529`).
pub const BABY_SPAWN_CHANCE: f32 = 0.05;
/// The multiplier `Zombie.getBaseExperienceReward` applies to a baby (`Zombie.java:181`).
const BABY_EXPERIENCE_MULTIPLIER: f64 = 2.5;

/// `Zombie::getSpawnAsBabyOdds` (`Zombie.java:527-529`): `random.nextFloat() < 0.05F`.
#[must_use]
pub fn spawn_as_baby_odds(roll: f32) -> bool {
    roll < BABY_SPAWN_CHANCE
}

/// `(int)(this.xpReward * 2.5)` (`Zombie.java:181`).
#[must_use]
pub fn baby_experience_reward(base: u32) -> u32 {
    (f64::from(base) * BABY_EXPERIENCE_MULTIPLIER) as u32
}

/// `LivingEntity::getVoicePitch` (`LivingEntity.java:2321-2325`): the two rolls' difference
/// scaled by `0.2F` around 1.5 for a baby and 1.0 for an adult.
#[must_use]
pub fn voice_pitch(is_baby: bool, first: f32, second: f32) -> f32 {
    (first - second).mul_add(0.2, if is_baby { 1.5 } else { 1.0 })
}

/// The baby hitbox each variant declares as `BABY_DIMENSIONS`: 0.49 x 0.98 everywhere, with the
/// eye height of `Zombie`/`Drowned` (`Zombie.java:90-92`, `Drowned.java:70-72`), `Husk`
/// (`Husk.java` `BABY_DIMENSIONS`) or `ZombieVillager` (`ZombieVillager.java:80-82`).
const fn baby_dimensions(entity_type: &'static EntityType) -> EntityDimensions {
    let eye_height = if entity_type.id == EntityType::HUSK.id {
        0.825
    } else if entity_type.id == EntityType::ZOMBIE_VILLAGER.id {
        0.67
    } else {
        0.775
    };
    EntityDimensions::new(0.49, 0.98, eye_height)
}

/// `Zombie.ZOMBIE_LEADER_CHANCE` (`Zombie.java:85`), rolled against the regional difficulty's
/// special multiplier at `Zombie.java:543`.
pub const ZOMBIE_LEADER_CHANCE: f32 = 0.05;
/// `Zombie.REINFORCEMENT_ATTEMPTS` (`Zombie.java:86`).
pub const REINFORCEMENT_ATTEMPTS: i32 = 50;
/// `Zombie.REINFORCEMENT_RANGE_MAX` (`Zombie.java:87`).
pub const REINFORCEMENT_RANGE_MAX: i32 = 40;
/// `Zombie.REINFORCEMENT_RANGE_MIN` (`Zombie.java:88`).
pub const REINFORCEMENT_RANGE_MIN: i32 = 7;
/// `Zombie.ZOMBIE_REINFORCEMENT_CALLEE_CHARGE`'s amount (`Zombie.java:78`), and the same value
/// the caller subtracts from its own charge modifier at `Zombie.java:320-326`.
pub const REINFORCEMENT_CHARGE: f64 = -0.05;
/// `level.hasNearbyAlivePlayer(xt, yt, zt, 7.0)` (`Zombie.java:313`).
const REINFORCEMENT_PLAYER_EXCLUSION_RADIUS: f64 = 7.0;

const REINFORCEMENT_CALLER_CHARGE_ID: &str = "minecraft:reinforcement_caller_charge";
const REINFORCEMENT_CALLEE_CHARGE_ID: &str = "minecraft:reinforcement_callee_charge";
const LEADER_ZOMBIE_BONUS_ID: &str = "minecraft:leader_zombie_bonus";
const RANDOM_SPAWN_BONUS_ID: &str = "minecraft:random_spawn_bonus";
const ZOMBIE_RANDOM_SPAWN_BONUS_ID: &str = "minecraft:zombie_random_spawn_bonus";

/// `Zombie.java:543`'s leader gate: `random.nextFloat() < difficultyModifier * 0.05F`. Split
/// out so the threshold is testable without a live world.
#[must_use]
pub fn leader_roll_threshold(difficulty_modifier: f32) -> f32 {
    difficulty_modifier * ZOMBIE_LEADER_CHANCE
}

/// `Zombie.java:536`: the `FOLLOW_RANGE` bonus is only installed when it exceeds `1.0`.
#[must_use]
pub const fn follow_range_modifier_applies(modifier: f64) -> bool {
    modifier > 1.0
}

/// `Zombie.java:322-326`: each successful reinforcement call replaces the caller's charge
/// modifier with its previous amount minus another `0.05`, so the penalty accumulates.
#[must_use]
pub const fn accumulated_caller_charge(existing: f64) -> f64 {
    existing + REINFORCEMENT_CHARGE
}

/// `Zombie::randomizeReinforcementsChance` (`Zombie.java:560-562`):
/// `setBaseValue(random.nextDouble() * 0.1F)`. The `0.1F` is widened to a double exactly as
/// javac does, so the upper bound is `0.10000000149011612`, not `0.1`.
fn randomize_reinforcements_chance(living: &LivingEntity) {
    let roll = rand::random::<f64>() * f64::from(0.1f32);
    living.set_attribute_base(&Attributes::SPAWN_REINFORCEMENTS, roll);
}

/// `Zombie::handleAttributes` (`Zombie.java:531-558`), the per-spawn attribute roll.
///
/// Every zombie variant inherits it. Returns `true` when this zombie rolled "leader": vanilla
/// then forces door breaking (`Zombie.java:556`), which is the caller's job here because only
/// `ZombieEntity` owns a `setCanBreakDoors` equivalent.
///
/// `heal` stands for the spawn reason: vanilla heals a new leader to its raised maximum unless
/// the reason is `CONVERSION`/`LOAD`/`DIMENSION_TRAVEL` (`Zombie.java:552-554`). A fresh spawn
/// passes `true`; a conversion, which keeps the converted mob's health, passes `false`. A mob
/// read back from disk never reaches this function.
#[allow(clippy::too_many_lines)]
pub async fn handle_attributes(
    living: &LivingEntity,
    difficulty_modifier: f32,
    heal: bool,
) -> bool {
    randomize_reinforcements_chance(living);

    // `Zombie.java:533-535`: KNOCKBACK_RESISTANCE += random.nextDouble() * 0.05F.
    living.update_attribute(&Attributes::KNOCKBACK_RESISTANCE, |instance| {
        instance.add_or_replace_modifier(Modifier {
            id: RANDOM_SPAWN_BONUS_ID.to_string(),
            amount: rand::random::<f64>() * f64::from(0.05f32),
            operation: ModifierOperation::Add,
        });
    });

    // `Zombie.java:536-542`: a FOLLOW_RANGE multiplier, applied only when it exceeds 1.0.
    let follow_range_modifier = rand::random::<f64>() * 1.5 * f64::from(difficulty_modifier);
    if follow_range_modifier_applies(follow_range_modifier) {
        living.update_attribute(&Attributes::FOLLOW_RANGE, |instance| {
            instance.add_or_replace_modifier(Modifier {
                id: ZOMBIE_RANDOM_SPAWN_BONUS_ID.to_string(),
                amount: follow_range_modifier,
                operation: ModifierOperation::MultiplyTotal,
            });
        });
    }

    let mut touched = vec![
        Attributes::SPAWN_REINFORCEMENTS,
        Attributes::KNOCKBACK_RESISTANCE,
        Attributes::FOLLOW_RANGE,
    ];

    // `Zombie.java:543-557`: the leader roll.
    let is_leader = rand::random::<f32>() < leader_roll_threshold(difficulty_modifier);
    if is_leader {
        living.update_attribute(&Attributes::SPAWN_REINFORCEMENTS, |instance| {
            instance.add_or_replace_modifier(Modifier {
                id: LEADER_ZOMBIE_BONUS_ID.to_string(),
                amount: rand::random::<f64>() * 0.25 + 0.5,
                operation: ModifierOperation::Add,
            });
        });
        living.update_attribute(&Attributes::MAX_HEALTH, |instance| {
            instance.add_or_replace_modifier(Modifier {
                id: LEADER_ZOMBIE_BONUS_ID.to_string(),
                amount: rand::random::<f64>() * 3.0 + 1.0,
                operation: ModifierOperation::MultiplyTotal,
            });
        });
        touched.push(Attributes::MAX_HEALTH);
        // `Zombie.java:552-554`.
        if heal {
            living.set_health(living.get_max_health());
        }
    }

    crate::entity::attributes::send_attribute_updates_for_living(living, touched).await;
    is_leader
}

/// `Zombie::canSpawnInLiquids` (`Zombie.java:379-381`) is `false`; `Drowned` overrides it to
/// `true` (`Drowned.java:192-194`).
const fn can_spawn_in_liquids(entity_type: &'static EntityType) -> bool {
    entity_type.id == EntityType::DROWNED.id
}

/// `LevelReader::containsAnyLiquid` (`LevelReader.java:140-161`).
fn contains_any_liquid(world: &World, bounding_box: &BoundingBox) -> bool {
    for x in bounding_box.min.x.floor() as i32..bounding_box.max.x.ceil() as i32 {
        for y in bounding_box.min.y.floor() as i32..bounding_box.max.y.ceil() as i32 {
            for z in bounding_box.min.z.floor() as i32..bounding_box.max.z.ceil() as i32 {
                if !world
                    .get_fluid_and_fluid_state(&BlockPos::new(x, y, z))
                    .1
                    .is_empty
                {
                    return true;
                }
            }
        }
    }
    false
}

/// The placement half of `Zombie.java:313-316` that follows the player-distance check:
/// `level.isUnobstructed(reinforcement) && level.noCollision(reinforcement) &&
/// (reinforcement.canSpawnInLiquids() || !level.containsAnyLiquid(box))`.
///
/// The entity half of the first two checks is the same collidable-entity scan the natural spawner
/// uses for its own `noCollision` (`natural_spawner::is_valid_spawn_position_for_type`).
fn reinforcement_fits(
    world: &Arc<World>,
    bounding_box: &BoundingBox,
    entity_type: &'static EntityType,
) -> bool {
    if !world.is_space_empty(*bounding_box)
        || world
            .get_all_at_box(&bounding_box.expand_all(1.0e-7))
            .iter()
            .any(|entity| !entity.is_spectator() && entity.can_be_collided_with())
    {
        return false;
    }
    can_spawn_in_liquids(entity_type) || !contains_any_liquid(world, bounding_box)
}

/// `Zombie::hurtServer`'s reinforcement half (`Zombie.java:288-340`).
///
/// Reached from `Mob::on_damage` -- which, like vanilla's `if (!super.hurtServer(...)) return
/// false;`, only runs once the hit has actually landed.
///
/// Divergences, all noted rather than silently approximated:
/// * the reinforcement's box is built from the entity type's dimensions instead of from the
///   created entity, so the check runs before the entity exists; a baby roll happens later in
///   `finalizeSpawn` in vanilla too, so both see the adult box.
/// * `EntitySpawnReason.REINFORCEMENT` is passed as `SpawnRuleContext::Natural`, the only
///   non-worldgen context this codebase models.
/// * positions in unloaded chunks are skipped instead of forcing a chunk load.
#[allow(clippy::too_many_lines)]
pub async fn try_spawn_reinforcements(mob: &MobEntity, source: Option<&dyn EntityBase>) {
    let living = &mob.living_entity;
    let entity = &living.entity;
    let world = entity.world.load_full();

    // `Zombie.java:289-292`: the current target, falling back to the attacker.
    let target = if let Some(target) = mob.get_target().await {
        target
    } else {
        let Some(source) = source else {
            return;
        };
        if source.get_living_entity().is_none() {
            return;
        }
        let Some(source_arc) = world.get_entity_by_id(source.get_entity().entity_id) else {
            return;
        };
        source_arc
    };

    {
        let level_info = world.level_info.load();
        // `Zombie.java:293`.
        if level_info.difficulty != Difficulty::Hard {
            return;
        }
        // `ServerLevel::isSpawningMonsters` (`ServerLevel.java:1776-1778`), called at
        // `Zombie.java:295`: `SPAWN_MOBS && SPAWN_MONSTERS`.
        if !level_info.game_rules.spawn_mobs || !level_info.game_rules.spawn_monsters {
            return;
        }
    }

    // `Zombie.java:294`.
    if rand::random::<f64>() >= living.get_attribute_value(&Attributes::SPAWN_REINFORCEMENTS) {
        return;
    }

    let pos = entity.pos.load();
    let base_x = pos.x.floor() as i32;
    let base_y = pos.y.floor() as i32;
    let base_z = pos.z.floor() as i32;
    let entity_type = entity.entity_type;
    let is_thundering = world.weather.lock().await.thundering;

    for _ in 0..REINFORCEMENT_ATTEMPTS {
        // `Zombie.java:304-306`: `Mth.nextInt` is inclusive on both bounds.
        let offset = || {
            rand::random_range(REINFORCEMENT_RANGE_MIN..=REINFORCEMENT_RANGE_MAX)
                * rand::random_range(-1..=1)
        };
        let x = base_x + offset();
        let y = base_y + offset();
        let z = base_z + offset();
        let spawn_pos = BlockPos::new(x, y, z);

        if !world.is_loaded(&spawn_pos) {
            continue;
        }
        if !is_spawn_position_ok(&world, &spawn_pos, entity_type) {
            continue;
        }
        if !check_spawn_rules(
            entity_type,
            &world,
            &spawn_pos,
            SpawnRuleContext::Natural,
            is_thundering,
        ) {
            continue;
        }

        let spawn_pos_f64 = Vector3::new(f64::from(x), f64::from(y), f64::from(z));
        // `Zombie.java:313`.
        if world
            .get_closest_player(spawn_pos_f64, REINFORCEMENT_PLAYER_EXCLUSION_RADIUS)
            .is_some()
        {
            continue;
        }

        // `Zombie.java:314-316`, after `reinforcement.setPos(xt, yt, zt)` -- the position is a
        // block corner, not a block centre, so the box straddles four columns.
        let bounding_box = BoundingBox::new_from_pos(
            spawn_pos_f64.x,
            spawn_pos_f64.y,
            spawn_pos_f64.z,
            &spawn_dimensions(entity_type),
        );
        if !reinforcement_fits(&world, &bounding_box, entity_type) {
            continue;
        }

        // `Zombie.java:302`: the reinforcement is of the caller's own type, so a hurt drowned
        // summons drowned rather than plain zombies.
        let reinforcement = from_type(entity_type, spawn_pos_f64, &world, Uuid::new_v4());
        if let Some(reinforcement_mob) = reinforcement.get_mob() {
            // `Zombie.java:317`.
            reinforcement_mob
                .get_mob_entity()
                .set_target(Some(target.clone()))
                .await;
        }
        if let Some(reinforcement_living) = reinforcement.get_living_entity() {
            // `Zombie.java:327`.
            reinforcement_living.update_attribute(&Attributes::SPAWN_REINFORCEMENTS, |instance| {
                instance.add_or_replace_modifier(Modifier {
                    id: REINFORCEMENT_CALLEE_CHARGE_ID.to_string(),
                    amount: REINFORCEMENT_CHARGE,
                    operation: ModifierOperation::Add,
                });
            });
        }

        // `Zombie.java:320-326`: the caller's own charge accumulates, one -0.05 per successful
        // call. Read-modify-write of the existing modifier's amount, done inside one
        // `update_attribute` closure so the whole thing happens under a single write lock.
        living.update_attribute(&Attributes::SPAWN_REINFORCEMENTS, |instance| {
            let existing = instance
                .modifiers
                .iter()
                .find(|modifier| modifier.id == REINFORCEMENT_CALLER_CHARGE_ID)
                .map_or(0.0, |modifier| modifier.amount);
            instance.add_or_replace_modifier(Modifier {
                id: REINFORCEMENT_CALLER_CHARGE_ID.to_string(),
                amount: accumulated_caller_charge(existing),
                operation: ModifierOperation::Add,
            });
        });

        // `Zombie.java:318-319`: `finalizeSpawn` then `addFreshEntityWithPassengers`.
        // `World::spawn_entity` runs `init_data_tracker`, which is where this codebase's
        // `finalizeSpawn` equivalent (`Mob::mob_init_data_tracker`) lives.
        world.spawn_entity(reinforcement).await;
        break;
    }
}

/// `Zombie.tick`'s `isEyeInFluid(FluidTags.WATER)` (`Zombie.java:221`): the eyes, not any part of
/// the body, must be under water for the drowning conversion timer to run.
fn is_eye_in_water(entity: &Entity) -> bool {
    entity.is_eye_in_fluid(&entity.world.load_full(), &tag::Fluid::MINECRAFT_WATER)
}

/// `Zombie.startUnderWaterConversion` (`Zombie.java:235-238`) and the `DrownedConversionTime`
/// read (`Zombie.java:414-415`) publish the synced `DATA_DROWNED_CONVERSION_ID`, which drives the
/// client's shaking animation.
fn publish_under_water_conversion(entity: &Entity) {
    entity.send_meta_data(
        &[Metadata::new(
            tracked_data::zombie::DATA_DROWNED_CONVERSION_ID,
            true,
        )],
        None,
    );
}

/// `Entity::isOnFire` (`Entity.java:2652-2655`) on the server.
fn is_on_fire(entity: &Entity) -> bool {
    !entity.fire_immune.load(Ordering::Relaxed) && entity.fire_ticks.load(Ordering::Relaxed) > 0
}

/// `Zombie.doHurtTarget`'s ignite chance per point of effective difficulty (`Zombie.java:342`).
const IGNITE_CHANCE_PER_DIFFICULTY: f32 = 0.3;

/// `2 * (int)difficulty` (`Zombie.java:343`): the cast truncates before the doubling.
#[must_use]
const fn ignite_seconds(effective_difficulty: f32) -> f32 {
    (2 * (effective_difficulty as i32)) as f32
}

/// The ignite half of `Zombie::doHurtTarget` (`Zombie.java:338-349`), run after a hit landed.
///
/// A burning zombie with an empty main hand sets its target alight with probability
/// `effectiveDifficulty * 0.3`, for `2 * (int)effectiveDifficulty` seconds. `Husk` and `Drowned`
/// reach it through their superclass.
pub async fn ignite_target_on_hit(mob: &MobEntity, target: &dyn EntityBase) {
    let living = &mob.living_entity;
    let entity = &living.entity;
    if !living.held_item(entity).await.is_empty() || !is_on_fire(entity) {
        return;
    }
    let difficulty =
        RegionalDifficulty::at(&entity.world.load_full(), entity.pos.load()).effective_difficulty;
    if rand::random::<f32>() < difficulty * IGNITE_CHANCE_PER_DIFFICULTY {
        target.set_on_fire_for(ignite_seconds(difficulty));
    }
}

/// `Zombie::convertToZombieType` (`Zombie.java:247-253`), up to but excluding the spawn:
/// `Mob.convertTo(type, ConversionParams.single(this, true, true), afterConversion)` with
/// `afterConversion` running `handleAttributes(specialMultiplier, CONVERSION)`. Finish with
/// [`zombification::complete_conversion`].
///
/// `convertCommon` also carries the baby flag (`ConversionType.java:79-81`). Neither the loot
/// pickup roll nor the baby roll of `finalizeSpawn` runs: `create(level, CONVERSION)` never calls
/// it, which the restored flags raised here express.
///
/// Divergence: `ConversionType.convertCommon`'s `setCanBreakDoors(true)` for a door-breaking
/// zombie (`ConversionType.java:120-122`) is the caller's to apply, because only `ZombieEntity`
/// has that state and a `Drowned` here does not.
pub(super) async fn prepare_zombie_conversion<Old, New>(
    old: &Old,
    new_type: &'static EntityType,
    build: impl FnOnce(Entity) -> Arc<New>,
) -> Arc<New>
where
    Old: ZombieFamily,
    New: ZombieFamily + EntityBase + Send + Sync + 'static,
{
    let old_base = old.zombie_base();
    let converted = zombification::prepare_conversion(&old_base.mob_entity, new_type, build).await;
    let new_base = converted.zombie_base();
    if old_base.is_baby() {
        new_base.set_baby(true).await;
    }
    new_base.mark_restored_from_nbt();
    new_base.handle_conversion_attributes().await;
    converted
}

/// `Zombie::convertVillagerToZombieVillager` (`Zombie.java:256-274`).
///
/// Replaces `villager` with a zombie villager that keeps its equipment, baby state, profession
/// data, XP, gossips and trade offers, and plays the infection sound at the zombie
/// (`!this.isSilent()`).
///
/// Returns `false` when the villager is already gone (`Mob.convertTo` returns `null` for a
/// removed mob).
///
/// Divergences: the `VillagerDataFinalized` flag is copied as `true`, because Pumpkin's villager
/// has no such flag and always carries final data. `finalizeSpawn`'s `setCanBreakDoors` roll is
/// not run: a zombie villager has no door-breaking state here.
pub async fn convert_villager_to_zombie_villager(
    zombie: &ZombieEntityBase,
    villager: &VillagerEntity,
) -> bool {
    let old = &villager.mob_entity;
    if old.living_entity.entity.is_removed() {
        return false;
    }

    let zombie_villager = zombification::prepare_conversion(
        old,
        &EntityType::ZOMBIE_VILLAGER,
        ZombieVillagerEntity::new,
    )
    .await;
    let base = zombie_villager.zombie_base();
    if AgeableMob::is_baby(villager) {
        base.set_baby(true).await;
    }
    base.mark_restored_from_nbt();
    base.handle_conversion_attributes().await;

    zombie_villager.set_villager_data_finalized(true);
    let data = *villager.villager_data.lock().await;
    zombie_villager.set_villager_data(data).await;
    let gossips = GossipContainer::from_raw(villager.gossips.lock().await.raw().clone());
    zombie_villager.set_gossips(gossips).await;
    let offers = villager.offers.lock().await.clone();
    zombie_villager.set_trade_offers(offers).await;
    zombie_villager.set_villager_xp(villager.xp.load(Ordering::Relaxed));

    let zombie_entity = &zombie.mob_entity.living_entity.entity;
    if !zombie_entity.silent.load(Ordering::Relaxed) {
        zombie_entity.world.load().sync_world_event(
            WorldEvent::SoundZombieInfected,
            zombie_entity.block_pos.load(),
            0,
        );
    }

    zombification::complete_conversion(old, zombie_villager).await;
    true
}

/// `Zombie::killedEntity` (`Zombie.java:421-435`), returning `perished`.
///
/// `perished` is whether the victim still dies the ordinary death. On Normal a villager is
/// infected half the time, on Hard always; an infected villager is replaced by a zombie villager
/// and so drops nothing.
pub async fn zombie_killed_entity(zombie: &ZombieEntityBase, victim: &dyn EntityBase) -> bool {
    let difficulty = zombie
        .mob_entity
        .living_entity
        .entity
        .world
        .load()
        .level_info
        .load()
        .difficulty;
    if !matches!(difficulty, Difficulty::Normal | Difficulty::Hard) {
        return true;
    }
    let Some(villager) = victim.cast_any().downcast_ref::<VillagerEntity>() else {
        return true;
    };
    if difficulty != Difficulty::Hard && rand::random::<bool>() {
        return true;
    }
    !convert_villager_to_zombie_villager(zombie, villager).await
}

#[cfg(test)]
mod tests {
    use super::{
        BABY_SPAWN_CHANCE, REINFORCEMENT_ATTEMPTS, REINFORCEMENT_CHARGE, REINFORCEMENT_RANGE_MAX,
        REINFORCEMENT_RANGE_MIN, ZOMBIE_LEADER_CHANCE, accumulated_caller_charge, baby_dimensions,
        baby_experience_reward, can_spawn_in_liquids, follow_range_modifier_applies,
        ignite_seconds, leader_roll_threshold, spawn_as_baby_odds, voice_pitch,
    };
    use pumpkin_data::entity::EntityType;

    #[test]
    fn reinforcement_constants_match_vanilla() {
        // `Zombie.java:85-88`.
        assert!((ZOMBIE_LEADER_CHANCE - 0.05).abs() < f32::EPSILON);
        assert_eq!(REINFORCEMENT_ATTEMPTS, 50);
        assert_eq!(REINFORCEMENT_RANGE_MAX, 40);
        assert_eq!(REINFORCEMENT_RANGE_MIN, 7);
        // `Zombie.java:78`.
        assert!((REINFORCEMENT_CHARGE + 0.05).abs() < f64::EPSILON);
    }

    #[test]
    fn leader_threshold_scales_with_regional_difficulty() {
        // `Zombie.java:543`: the roll is against `difficultyModifier * 0.05F`, so a chunk with
        // a zero special multiplier (fresh chunk, early game, or Peaceful) never makes leaders.
        assert!((leader_roll_threshold(1.0) - 0.05).abs() < f32::EPSILON);
        assert!((leader_roll_threshold(0.5) - 0.025).abs() < f32::EPSILON);
        assert!(leader_roll_threshold(0.0) <= 0.0);
    }

    #[test]
    fn follow_range_bonus_is_gated_above_one() {
        // `Zombie.java:536`.
        assert!(!follow_range_modifier_applies(1.0));
        assert!(!follow_range_modifier_applies(0.9));
        assert!(follow_range_modifier_applies(1.0001));
    }

    #[test]
    fn caller_charge_accumulates_per_reinforcement() {
        // `Zombie.java:322-326`: three successful calls leave the caller at -0.15.
        let mut charge = 0.0;
        for _ in 0..3 {
            charge = accumulated_caller_charge(charge);
        }
        assert!((charge + 0.15).abs() < 1e-9);
    }

    #[test]
    fn baby_roll_is_a_five_percent_float_threshold() {
        // `Zombie.getSpawnAsBabyOdds` (`Zombie.java:527-529`): `nextFloat() < 0.05F`.
        assert!((BABY_SPAWN_CHANCE - 0.05).abs() < f32::EPSILON);
        assert!(spawn_as_baby_odds(0.0));
        assert!(spawn_as_baby_odds(0.049));
        assert!(!spawn_as_baby_odds(0.05));
        assert!(!spawn_as_baby_odds(0.9));
    }

    #[test]
    fn baby_experience_is_scaled_and_truncated() {
        // `(int)(this.xpReward * 2.5)` (`Zombie.java:181`): a zombie's 5 becomes 12, not 13.
        assert_eq!(baby_experience_reward(5), 12);
        assert_eq!(baby_experience_reward(0), 0);
        assert_eq!(baby_experience_reward(10), 25);
    }

    #[test]
    fn voice_pitch_is_centered_on_the_baby_or_adult_base() {
        // `LivingEntity.getVoicePitch` (`LivingEntity.java:2321-2325`).
        assert!((voice_pitch(false, 0.5, 0.5) - 1.0).abs() < 1.0e-6);
        assert!((voice_pitch(true, 0.5, 0.5) - 1.5).abs() < 1.0e-6);
        assert!((voice_pitch(true, 1.0, 0.0) - 1.7).abs() < 1.0e-6);
        assert!((voice_pitch(false, 0.0, 1.0) - 0.8).abs() < 1.0e-6);
    }

    #[test]
    fn baby_hitbox_keeps_each_variants_eye_height() {
        // `Zombie.java:90-92`, `Drowned.java:70-72`, `Husk.java` and `ZombieVillager.java:80-82`.
        for (entity_type, eye_height) in [
            (&EntityType::ZOMBIE, 0.775),
            (&EntityType::DROWNED, 0.775),
            (&EntityType::HUSK, 0.825),
            (&EntityType::ZOMBIE_VILLAGER, 0.67),
        ] {
            let dimensions = baby_dimensions(entity_type);
            assert!((dimensions.width - 0.49).abs() < f32::EPSILON);
            assert!((dimensions.height - 0.98).abs() < f32::EPSILON);
            assert!((dimensions.eye_height - eye_height).abs() < f32::EPSILON);
        }
    }

    #[test]
    fn ignite_duration_truncates_difficulty_before_doubling() {
        // `2 * (int)difficulty` (`Zombie.java:343`).
        assert!((ignite_seconds(2.0) - 4.0).abs() < f32::EPSILON);
        assert!((ignite_seconds(2.9) - 4.0).abs() < f32::EPSILON);
        assert!((ignite_seconds(3.5) - 6.0).abs() < f32::EPSILON);
        assert!(ignite_seconds(0.9).abs() < f32::EPSILON);
    }

    #[test]
    fn only_a_drowned_reinforcement_may_spawn_in_liquid() {
        // `Zombie.canSpawnInLiquids` is `false`; `Drowned.java:192-194` overrides it.
        assert!(can_spawn_in_liquids(&EntityType::DROWNED));
        assert!(!can_spawn_in_liquids(&EntityType::ZOMBIE));
        assert!(!can_spawn_in_liquids(&EntityType::HUSK));
        assert!(!can_spawn_in_liquids(&EntityType::ZOMBIE_VILLAGER));
    }
}
