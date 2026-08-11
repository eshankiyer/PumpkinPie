use super::{Mob, MobEntity};
use crate::entity::ai::goal::break_door::{self, BreakDoorGoal};
use crate::entity::ai::goal::destroy_egg::DestroyEggGoal;
use crate::entity::ai::goal::look_around::RandomLookAroundGoal;
use crate::entity::ai::goal::move_through_village::MoveThroughVillageGoal;
use crate::entity::ai::goal::revenge::RevengeGoal;
use crate::entity::ai::goal::spear_use::SpearUseGoal;
use crate::entity::ai::goal::swim::SwimGoal;
use crate::entity::ai::goal::wander_around::WanderAroundGoal;
use crate::entity::ai::goal::zombie_attack::ZombieAttackGoal;
use crate::entity::mob::equipment::RegionalDifficulty;
use crate::entity::{
    Entity, EntityBase, NBTStorage, NbtFuture,
    ai::goal::{active_target::ActiveTargetGoal, look_at_entity::LookAtEntityGoal},
};
use crate::world::{World, scoreboard::entity_scoreboard_name};
use pumpkin_data::entity::EntityType;
use pumpkin_nbt::compound::NbtCompound;
use std::sync::atomic::{AtomicI8, AtomicI32, Ordering};
use std::sync::{Arc, Weak};

pub(crate) const CAN_BREAK_DOORS_UNDECIDED: i8 = -1;
const BREAK_DOOR_CHANCE: f32 = 0.1;

pub mod drowned;
pub mod husk;
#[allow(clippy::module_inception)]
pub mod zombie;
pub mod zombie_villager;

pub struct ZombieEntityBase {
    pub mob_entity: MobEntity,
    pub(crate) can_break_doors: AtomicI8,
    pub(crate) in_water_time: AtomicI32,
    pub(crate) conversion_time: AtomicI32,
}

impl ZombieEntityBase {
    pub fn new(entity: Entity) -> Arc<Self> {
        let mob_entity = MobEntity::new(entity);
        let zombie = Self {
            mob_entity,
            can_break_doors: AtomicI8::new(CAN_BREAK_DOORS_UNDECIDED),
            in_water_time: AtomicI32::new(0),
            conversion_time: AtomicI32::new(-1),
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
            goal_selector.add_goal(6, Box::new(MoveThroughVillageGoal::new(1.0, true, 4)));
            goal_selector.add_goal(7, Box::new(WanderAroundGoal::new(1.0)));
            goal_selector.add_goal(
                8,
                LookAtEntityGoal::with_default(mob_weak, &EntityType::PLAYER, 8.0),
            );
            goal_selector.add_goal(8, Box::new(RandomLookAroundGoal::default()));

            target_selector.add_goal(1, Box::new(RevengeGoal::new(true)));
            target_selector.add_goal(
                2,
                ActiveTargetGoal::with_default(&mob_arc.mob_entity, &EntityType::PLAYER, true),
            );
            target_selector.add_goal(
                3,
                ActiveTargetGoal::with_default(&mob_arc.mob_entity, &EntityType::VILLAGER, true),
            );
            target_selector.add_goal(
                3,
                ActiveTargetGoal::with_default(&mob_arc.mob_entity, &EntityType::IRON_GOLEM, true),
            );
            target_selector.add_goal(
                5,
                ActiveTargetGoal::with_default(&mob_arc.mob_entity, &EntityType::TURTLE, true),
            );
        };

        mob_arc
    }

    /// Vanilla `Zombie::setCanBreakDoors`, shared by every class that extends `Zombie`.
    pub(crate) async fn set_can_break_doors(&self, can_break_doors: bool) {
        let new_value = i8::from(can_break_doors);
        let previous = self.can_break_doors.swap(new_value, Ordering::Relaxed);
        if previous == new_value {
            return;
        }

        self.mob_entity
            .navigator
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .set_can_open_doors(can_break_doors);
        if can_break_doors {
            self.mob_entity
                .goals_selector
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .add_goal(1, Box::new(BreakDoorGoal::new(break_door::hard_only)));
        } else if previous != CAN_BREAK_DOORS_UNDECIDED {
            let mut goals = {
                let mut guard = self
                    .mob_entity
                    .goals_selector
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                std::mem::take(&mut *guard)
            };
            goals.remove_goal::<BreakDoorGoal>(self).await;
            *self
                .mob_entity
                .goals_selector
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = goals;
        }
    }

    pub(crate) fn can_break_doors(&self) -> bool {
        self.can_break_doors.load(Ordering::Relaxed) == 1
    }

    pub(crate) async fn initialize_can_break_doors(&self) {
        if self.can_break_doors.load(Ordering::Relaxed) == CAN_BREAK_DOORS_UNDECIDED {
            let (world, pos) = {
                let entity = &self.mob_entity.living_entity.entity;
                (entity.world.load_full(), entity.pos.load())
            };
            let difficulty = RegionalDifficulty::at(&world, pos);
            self.set_can_break_doors(
                rand::random::<f32>() < BREAK_DOOR_CHANCE * difficulty.special_multiplier,
            )
            .await;
        }
    }

    /// Copies the state preserved by vanilla `ConversionType.SINGLE` when one zombie-family mob
    /// is replaced by another. The destination is initialized before it is spawned, so the
    /// copied door capability also prevents a second random spawn roll.
    pub(crate) async fn copy_conversion_state_to(&self, target: &Self) {
        let equipment = {
            let mut source_equipment = self.mob_entity.living_entity.entity_equipment.lock().await;
            std::mem::take(&mut source_equipment.equipment)
        };
        {
            let mut target_equipment = target
                .mob_entity
                .living_entity
                .entity_equipment
                .lock()
                .await;
            for (slot, stack) in equipment {
                target_equipment.put(&slot, stack);
            }
        }
        let source_drop_chances = self
            .mob_entity
            .living_entity
            .equipment_drop_chances
            .lock()
            .await
            .clone();
        target
            .mob_entity
            .living_entity
            .equipment_drop_chances
            .lock()
            .await
            .clone_from(&source_drop_chances);

        target
            .mob_entity
            .set_can_pick_up_loot(self.mob_entity.can_pick_up_loot());
        target
            .mob_entity
            .set_left_handed(self.mob_entity.is_left_handed());
        target.mob_entity.set_no_ai(self.mob_entity.is_no_ai());
        target
            .mob_entity
            .living_entity
            .entity
            .persistence_required
            .store(self.mob_entity.is_persistence_required(), Ordering::Relaxed);
        target.mob_entity.living_entity.entity.invulnerable.store(
            self.mob_entity
                .living_entity
                .entity
                .invulnerable
                .load(Ordering::Relaxed),
            Ordering::Relaxed,
        );
        target
            .mob_entity
            .living_entity
            .entity
            .portal_cooldown
            .store(
                self.mob_entity
                    .living_entity
                    .entity
                    .portal_cooldown
                    .load(Ordering::Relaxed),
                Ordering::Relaxed,
            );
        target
            .mob_entity
            .living_entity
            .entity
            .set_silent(self.mob_entity.living_entity.entity.is_silent());
        target
            .mob_entity
            .living_entity
            .entity
            .set_has_no_gravity(self.mob_entity.living_entity.entity.has_no_gravity());
        target
            .mob_entity
            .living_entity
            .fall_distance
            .store(self.mob_entity.living_entity.fall_distance.load());
        self.copy_conversion_entity_state(target).await;
        target.set_can_break_doors(self.can_break_doors()).await;
    }

    async fn copy_conversion_entity_state(&self, target: &Self) {
        let source_entity = &self.mob_entity.living_entity.entity;
        let target_entity = &target.mob_entity.living_entity.entity;
        target
            .mob_entity
            .living_entity
            .absorption
            .store(self.mob_entity.living_entity.absorption.load());
        target_entity.body_yaw.store(source_entity.body_yaw.load());
        target_entity.fall_flying.store(
            source_entity.fall_flying.load(Ordering::Relaxed),
            Ordering::Relaxed,
        );
        target_entity
            .set_on_fire(source_entity.fire_ticks.load(Ordering::Relaxed) > 0)
            .await;
        target_entity
            .custom_data
            .store(Arc::new((**source_entity.custom_data.load()).clone()));
        let source_tags = source_entity.scoreboard_tags.lock().await.clone();
        target_entity
            .scoreboard_tags
            .lock()
            .await
            .clone_from(&source_tags);
    }
}

/// Transfers the relationship state required by vanilla `ConversionType.SINGLE`.
pub(crate) async fn transfer_conversion_topology(
    source: &Entity,
    target: &Entity,
    target_arc: Arc<dyn EntityBase>,
) {
    let passenger = source.passengers.lock().await.first().cloned();
    if let Some(passenger) = passenger {
        source
            .remove_passenger(passenger.get_entity().entity_id)
            .await;
        passenger
            .get_entity()
            .riding_cooldown
            .store(0, std::sync::atomic::Ordering::Relaxed);
        target.add_passenger(target_arc.clone(), passenger).await;
    }

    let vehicle = source.vehicle.lock().await.clone();
    if let Some(vehicle) = vehicle {
        vehicle
            .get_entity()
            .remove_passenger(source.entity_id)
            .await;
        vehicle
            .get_entity()
            .add_passenger(vehicle.clone(), target_arc.clone())
            .await;
    }

    let leash_holder = source.leashed_to.lock().await.clone();
    if let Some(leash_holder) = leash_holder {
        target.leash_to(leash_holder).await;
    }
}

pub(crate) async fn transfer_conversion_team(
    world: &World,
    source: &dyn EntityBase,
    target: &dyn EntityBase,
) {
    let source_name = entity_scoreboard_name(source);
    let target_name = entity_scoreboard_name(target);
    let team_name = {
        let scoreboard = world.scoreboard.lock().await;
        scoreboard
            .get_team_for_scoreboard_name(&source_name)
            .map(|team| team.name.clone())
    };
    if let Some(team_name) = team_name {
        let mut scoreboard = world.scoreboard.lock().await;
        scoreboard.remove_player_from_team(world, &team_name, &source_name);
        scoreboard.add_player_to_team(world, &team_name, target_name);
    }
}

impl NBTStorage for ZombieEntityBase {
    fn write_nbt<'a>(&'a self, nbt: &'a mut NbtCompound) -> NbtFuture<'a, ()> {
        Box::pin(async move {
            self.mob_entity.living_entity.write_nbt(nbt).await;
            nbt.put_bool(
                "IsBaby",
                self.mob_entity
                    .living_entity
                    .entity
                    .age
                    .load(Ordering::Relaxed)
                    < 0,
            );
            nbt.put_bool("CanBreakDoors", self.can_break_doors());
            let in_water_time = if self
                .mob_entity
                .living_entity
                .entity
                .touching_water
                .load(Ordering::Relaxed)
            {
                self.in_water_time.load(Ordering::Relaxed)
            } else {
                -1
            };
            nbt.put_int("InWaterTime", in_water_time);
            nbt.put_int(
                "DrownedConversionTime",
                self.conversion_time.load(Ordering::Relaxed),
            );
        })
    }

    fn read_nbt_non_mut<'a>(&'a self, nbt: &'a NbtCompound) -> NbtFuture<'a, ()> {
        Box::pin(async move {
            self.mob_entity.living_entity.read_nbt_non_mut(nbt).await;
            self.mob_entity.living_entity.entity.set_age(
                if nbt.get_bool("IsBaby").unwrap_or(false) {
                    -24_000
                } else {
                    0
                },
            );
            self.set_can_break_doors(nbt.get_bool("CanBreakDoors").unwrap_or(false))
                .await;
            self.in_water_time
                .store(nbt.get_int("InWaterTime").unwrap_or(0), Ordering::Relaxed);
            self.conversion_time.store(
                nbt.get_int("DrownedConversionTime").unwrap_or(-1),
                Ordering::Relaxed,
            );
        })
    }
}

impl Mob for ZombieEntityBase {
    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    fn can_break_doors(&self) -> bool {
        Self::can_break_doors(self)
    }
}
