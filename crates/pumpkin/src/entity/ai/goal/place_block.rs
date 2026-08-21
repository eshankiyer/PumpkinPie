use std::sync::Arc;

use super::{Goal, GoalFuture, to_goal_ticks};
use crate::entity::EntityBase;
use crate::entity::boss::ender_dragon::EnderDragonEntity;
use crate::entity::mob::Mob;
use crate::entity::mob::enderman::EndermanEntity;
use pumpkin_data::block_properties::is_air;
use pumpkin_data::game_event::GameEvent;
use pumpkin_data::{Block, BlockState};
use pumpkin_util::math::vector3::Vector3;
use pumpkin_util::math::{boundingbox::BoundingBox, position::BlockPos};
use pumpkin_world::world::BlockFlags;
use rand::RngExt;

use crate::world::game_event::{GameEventContext, emit_game_event};

pub struct PlaceBlockGoal {
    enderman: Arc<EndermanEntity>,
}

impl PlaceBlockGoal {
    pub const fn new(enderman: Arc<EndermanEntity>) -> Self {
        Self { enderman }
    }
}

impl Goal for PlaceBlockGoal {
    fn can_start<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        Box::pin(async move {
            if self.enderman.get_carried_block().is_none() {
                return false;
            }

            let entity = &mob.get_mob_entity().living_entity.entity;
            let world = entity.world.load();
            if !world.level_info.load().game_rules.mob_griefing {
                return false;
            }

            if mob.get_random().random_range(0..to_goal_ticks(2000)) != 0 {
                return false;
            }

            true
        })
    }

    #[allow(clippy::too_many_lines)]
    fn tick<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async move {
            let Some(block_state_id) = self.enderman.get_carried_block() else {
                return;
            };

            let entity = &mob.get_mob_entity().living_entity.entity;
            let pos = entity.pos.load();

            let (bx, by, bz) = {
                let mut rng = mob.get_random();
                (
                    pos.x.floor() as i32 + rng.random_range(-1..=1),
                    pos.y.floor() as i32 + rng.random_range(0..=2),
                    pos.z.floor() as i32 + rng.random_range(-1..=1),
                )
            };

            let world = entity.world.load();
            let target_pos = BlockPos::new(bx, by, bz);

            let state_id = world.get_block_state_id(&target_pos);
            if !is_air(state_id) {
                return;
            }

            let below_pos = BlockPos::new(bx, by - 1, bz);
            let (below_block, below_state) = world.get_block_and_state(&below_pos);
            if !below_state.is_full_cube() || below_block == &Block::BEDROCK {
                return;
            }

            let carried_state_id = world
                .update_from_neighbor_shapes_vanilla(block_state_id, &target_pos)
                .await;
            let carried_block = Block::from_state_id(carried_state_id);
            let carried_state = BlockState::from_id(carried_state_id);
            if !world.block_registry.can_place_at(
                None,
                Some(&world),
                world.as_ref(),
                None,
                carried_block,
                carried_state,
                &target_pos,
                None,
                None,
            ) {
                return;
            }

            let target_box = BoundingBox::from_block(&target_pos);
            let enderman_id = entity.entity_id;
            let occupied_by_entity = world.get_all_at_box(&target_box).iter().any(|candidate| {
                !candidate.is_spectator() && candidate.get_entity().entity_id != enderman_id
            });
            let occupied_by_dragon_part = world.entities.load().iter().any(|candidate| {
                candidate
                    .cast_any()
                    .downcast_ref::<EnderDragonEntity>()
                    .is_some_and(|dragon| {
                        dragon
                            .parts
                            .iter()
                            .any(|part| part.entity.bounding_box.load().intersects(&target_box))
                    })
            });
            if occupied_by_entity || occupied_by_dragon_part {
                return;
            }

            if !world
                .set_block_state_if_validated(
                    &target_pos,
                    state_id,
                    carried_state_id,
                    BlockFlags::NOTIFY_ALL,
                    Arc::new(move |world: &crate::world::World| {
                        let (below_block, below_state) = world.get_block_and_state(&below_pos);
                        if !below_state.is_full_cube() || below_block == &Block::BEDROCK {
                            return false;
                        }
                        if !world.block_registry.can_place_at(
                            None,
                            Some(world),
                            world,
                            None,
                            carried_block,
                            carried_state,
                            &target_pos,
                            None,
                            None,
                        ) {
                            return false;
                        }
                        if !is_air(world.get_block_state_id(&target_pos)) {
                            return false;
                        }

                        let occupied_by_entity =
                            world.get_all_at_box(&target_box).iter().any(|candidate| {
                                !candidate.is_spectator()
                                    && candidate.get_entity().entity_id != enderman_id
                            });
                        let occupied_by_dragon_part =
                            world.entities.load().iter().any(|candidate| {
                                candidate
                                    .cast_any()
                                    .downcast_ref::<EnderDragonEntity>()
                                    .is_some_and(|dragon| {
                                        dragon.parts.iter().any(|part| {
                                            part.entity.bounding_box.load().intersects(&target_box)
                                        })
                                    })
                            });
                        !occupied_by_entity && !occupied_by_dragon_part
                    }),
                )
                .await
            {
                return;
            }

            emit_game_event(
                &world,
                GameEvent::BlockPlace,
                Vector3::new(
                    f64::from(target_pos.0.x) + 0.5,
                    f64::from(target_pos.0.y) + 0.5,
                    f64::from(target_pos.0.z) + 0.5,
                ),
                GameEventContext::of_entity_with_block_state(
                    self.enderman.clone() as Arc<dyn EntityBase>,
                    carried_state_id,
                ),
            )
            .await;
            self.enderman.set_carried_block(None);
        })
    }

    fn should_continue<'a>(&'a mut self, _mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        Box::pin(async { false })
    }
}
