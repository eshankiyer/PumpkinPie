use std::sync::Arc;
use std::sync::atomic::Ordering::Relaxed;

use super::panda_lie_on_back::PandaLieOnBackGoal;
use super::{Controls, Goal, GoalFuture};
use crate::entity::ageable::AgeableMob;
use crate::entity::ai::pathfinder::NavigatorGoal;
use crate::entity::mob::Mob;
use crate::entity::passive::panda::PandaEntity;
use crate::entity::{Entity, EntityBase};
use rand::RngExt;

/// `Panda.PandaSitGoal` (`Panda.java:1076-1129`): an adult panda sits down to eat bamboo, either
/// the piece already in its hand or one it walks over to pick up.
pub struct PandaSitGoal {
    /// `PandaSitGoal.cooldown`, compared against the panda's tick count.
    cooldown: i32,
}

impl PandaSitGoal {
    #[must_use]
    pub fn new() -> Box<Self> {
        Box::new(Self { cooldown: 0 })
    }

    /// `Panda.canPickUpAndEat`'s entity half: the whole predicate vanilla passes to
    /// `getEntitiesOfClass(ItemEntity.class, ...)`.
    async fn edible_items(entity: &Entity, radius: f64) -> Vec<Arc<dyn EntityBase>> {
        let world = entity.world.load();
        let area = entity.bounding_box.load().expand_all(radius);
        let mut out = Vec::new();
        for candidate in world.get_entities_at_box(&area) {
            let Some(item_entity) = candidate.clone().get_item_entity() else {
                continue;
            };
            if !item_entity.get_entity().is_alive() || item_entity.has_pickup_delay() {
                continue;
            }
            let stack = item_entity.get_item_stack().lock().await.clone();
            if PandaEntity::can_pick_up_and_eat(&stack) {
                out.push(candidate);
            }
        }
        out
    }
}

impl Goal for PandaSitGoal {
    fn can_start<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        Box::pin(async move {
            let Some(panda) = mob.cast_any().downcast_ref::<PandaEntity>() else {
                return false;
            };
            let mob_entity = panda.get_mob_entity();
            if self.cooldown > mob_entity.tick_count.load(Relaxed)
                || panda.is_baby()
                || mob_entity.living_entity.is_in_water()
                || panda.get_unhappy_counter() > 0
            {
                return false;
            }
            if !panda.can_perform_action().await {
                return false;
            }

            if !panda.held_stack().await.is_empty() {
                return true;
            }
            !Self::edible_items(&mob_entity.living_entity.entity, 6.0)
                .await
                .is_empty()
        })
    }

    /// Identical body to `PandaLieOnBackGoal.canContinueToUse` in vanilla; shared here rather
    /// than duplicated.
    fn should_continue<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        Box::pin(async move {
            let Some(panda) = mob.cast_any().downcast_ref::<PandaEntity>() else {
                return false;
            };
            PandaLieOnBackGoal::continue_roll(
                panda.is_lazy(),
                panda.get_mob_entity().living_entity.is_in_water(),
                self.get_tick_count(600),
                self.get_tick_count(2000),
            )
        })
    }

    fn start<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async move {
            let Some(panda) = mob.cast_any().downcast_ref::<PandaEntity>() else {
                return;
            };
            let mob_entity = panda.get_mob_entity();
            if panda.held_stack().await.is_empty() {
                let items = Self::edible_items(&mob_entity.living_entity.entity, 8.0).await;
                if let Some(first) = items.first() {
                    let from = mob_entity.living_entity.entity.pos.load();
                    let to = first.get_entity().pos.load();
                    mob_entity
                        .navigator
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .set_progress(NavigatorGoal::new(from, to, 1.2));
                }
            } else {
                panda.try_to_sit();
            }
            self.cooldown = 0;
        })
    }

    fn tick<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async move {
            let Some(panda) = mob.cast_any().downcast_ref::<PandaEntity>() else {
                return;
            };
            if !panda.is_sitting_panda() && !panda.held_stack().await.is_empty() {
                panda.try_to_sit();
            }
        })
    }

    fn stop<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async move {
            let Some(panda) = mob.cast_any().downcast_ref::<PandaEntity>() else {
                return;
            };
            let mob_entity = panda.get_mob_entity();
            let held = panda.held_stack().await;
            if !held.is_empty() {
                let entity = &mob_entity.living_entity.entity;
                entity
                    .world
                    .load()
                    .drop_stack(&entity.block_pos.load(), held)
                    .await;
                panda
                    .set_held_stack(pumpkin_data::item_stack::ItemStack::EMPTY.clone())
                    .await;
                // A lazy panda waits 10-59 seconds before sitting down again; anyone else 10-159.
                let wait_seconds = if panda.is_lazy() {
                    rand::rng().random_range(0..50) + 10
                } else {
                    rand::rng().random_range(0..150) + 10
                };
                self.cooldown = mob_entity.tick_count.load(Relaxed) + wait_seconds * 20;
            }
            panda.sit(false);
        })
    }

    fn controls(&self) -> Controls {
        Controls::MOVE
    }
}
