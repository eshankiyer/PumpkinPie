use super::{Mob, MobEntity};
use crate::entity::EntityBaseFuture;
use crate::entity::ai::goal::destroy_egg::DestroyEggGoal;
use crate::entity::ai::goal::look_around::RandomLookAroundGoal;
use crate::entity::ai::goal::non_tame_random_target::baby_turtle_on_land;
use crate::entity::ai::goal::revenge::RevengeGoal;
use crate::entity::ai::goal::spear_use::SpearUseGoal;
use crate::entity::ai::goal::swim::SwimGoal;
use crate::entity::ai::goal::wander_around::WanderAroundGoal;
use crate::entity::ai::goal::zombie_attack::ZombieAttackGoal;
use crate::entity::attributes::{Modifier, ModifierOperation};
use crate::entity::living::LivingEntity;
use crate::entity::mob::equipment::RegionalDifficulty;
use crate::entity::r#type::{SpawnRuleContext, check_spawn_rules, from_type};
use crate::entity::{
    Entity, EntityBase, NBTStorage, NbtFuture,
    ai::goal::{active_target::ActiveTargetGoal, look_at_entity::LookAtEntityGoal},
};
use crate::world::natural_spawner::is_spawn_position_ok;
use pumpkin_data::attributes::Attributes;
use pumpkin_data::entity::EntityType;
use pumpkin_data::tracked_data;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_protocol::java::client::play::Metadata;
use pumpkin_util::Difficulty;
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
}

impl ZombieEntityBase {
    pub fn new(entity: Entity) -> Arc<Self> {
        let mob_entity = MobEntity::new(entity);
        let zombie = Self {
            mob_entity,
            restored_from_nbt: AtomicBool::new(false),
            is_leader: AtomicBool::new(false),
        };
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
            goal_selector.add_goal(7, Box::new(WanderAroundGoal::new_water_avoiding(1.0)));
            goal_selector.add_goal(
                8,
                LookAtEntityGoal::with_default(mob_weak, &EntityType::PLAYER, 8.0),
            );
            goal_selector.add_goal(8, Box::new(RandomLookAroundGoal::default()));

            // `Zombie.java:124` calls `setAlertOthers(ZombifiedPiglin.class)` on this goal.
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
    pub const fn from_mob_entity(mob_entity: MobEntity) -> Self {
        Self {
            mob_entity,
            restored_from_nbt: AtomicBool::new(false),
            is_leader: AtomicBool::new(false),
        }
    }

    /// Marks this zombie as restored from disk, suppressing the fresh-spawn attribute roll.
    pub fn mark_restored_from_nbt(&self) {
        self.restored_from_nbt.store(true, Ordering::Relaxed);
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
        )
        .await;
        self.is_leader.store(is_leader, Ordering::Relaxed);
    }
}

impl NBTStorage for ZombieEntityBase {
    fn write_nbt<'a>(&'a self, nbt: &'a mut NbtCompound) -> NbtFuture<'a, ()> {
        self.mob_entity.living_entity.write_nbt(nbt)
    }

    fn read_nbt_non_mut<'a>(&'a self, nbt: &'a NbtCompound) -> NbtFuture<'a, ()> {
        self.mark_restored_from_nbt();
        self.mob_entity.living_entity.read_nbt_non_mut(nbt)
    }
}

impl Mob for ZombieEntityBase {
    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    /// Adds `Zombie::finalizeSpawn`'s `handleAttributes` call (`Zombie.java:505`) on top of the
    /// `Mob` default's baby-metadata send. That default is inlined rather than reached through
    /// `Mob::mob_init_data_tracker(self)`, which would resolve straight back into this override
    /// and recurse.
    fn mob_init_data_tracker(&self) -> EntityBaseFuture<'_, ()> {
        Box::pin(async move {
            let entity = self.get_entity();
            if entity.age.load(Ordering::Relaxed) < 0 {
                entity.send_meta_data(
                    &[Metadata::new(tracked_data::ageable_mob::DATA_BABY_ID, true)],
                    None,
                );
            }
            self.roll_spawn_attributes().await;
        })
    }
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
/// then heals it to its new maximum (`Zombie.java:553`) and forces door breaking
/// (`Zombie.java:556`), both of which are the caller's job here because only `ZombieEntity`
/// owns a `setCanBreakDoors` equivalent.
///
/// Divergence: vanilla skips the heal for `CONVERSION`/`LOAD`/`DIMENSION_TRAVEL` spawn
/// reasons; Pumpkin has no spawn-reason plumbing, so callers only invoke this on a genuine
/// fresh spawn, which covers the same intent.
pub async fn handle_attributes(living: &LivingEntity, difficulty_modifier: f32) -> bool {
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
        // `Zombie.java:553`.
        living.set_health(living.get_max_health());
    }

    crate::entity::attributes::send_attribute_updates_for_living(living, touched).await;
    is_leader
}

/// `Zombie::hurtServer`'s reinforcement half (`Zombie.java:288-340`).
///
/// Reached from `Mob::on_damage` -- which, like vanilla's `if (!super.hurtServer(...)) return
/// false;`, only runs once the hit has actually landed.
///
/// Divergences, all noted rather than silently approximated:
/// * vanilla's `level.isUnobstructed(reinforcement)` / `noCollision` / `containsAnyLiquid`
///   bounding-box checks have no equivalent here; `is_spawn_position_ok` plus the entity type's
///   own spawn rules are the closest available stand-in.
/// * `EntitySpawnReason.REINFORCEMENT` is passed as `SpawnRuleContext::Natural`, the only
///   non-worldgen context this codebase models.
/// * positions in unloaded chunks are skipped instead of forcing a chunk load.
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

#[cfg(test)]
mod tests {
    use super::{
        REINFORCEMENT_ATTEMPTS, REINFORCEMENT_CHARGE, REINFORCEMENT_RANGE_MAX,
        REINFORCEMENT_RANGE_MIN, ZOMBIE_LEADER_CHANCE, accumulated_caller_charge,
        follow_range_modifier_applies, leader_roll_threshold,
    };

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
}
