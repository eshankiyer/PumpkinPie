//! `Fox.FoxEatBerriesGoal` (`Fox.java:906-984`), a `MoveToBlockGoal` subclass registered at
//! priority 10 as `new Fox.FoxEatBerriesGoal(1.2F, 12, 1)` (`Fox.java:198`).
//!
//! Deviations from vanilla, both forced by `move_to_target_pos.rs`'s shape:
//! * `shouldRecalculatePath` (`Fox.java:919-921`, `tryTicks % 100`) is not overridable here;
//!   the base's `trying_time % 40` cadence is used instead.
//! * Glow-berry harvesting drops one `GLOW_BERRIES` directly rather than rolling
//!   `BuiltInLootTables.HARVEST_CAVE_VINE` (`CaveVines.java:26-33`), which that table's only
//!   entry produces anyway.

use std::pin::Pin;
use std::sync::Arc;

use pumpkin_data::block_properties::{
    BlockProperties, CaveVinesLikeProperties, CaveVinesPlantLikeProperties,
    NetherWartLikeProperties,
};
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_data::{Block, BlockStateId};
use pumpkin_util::math::position::BlockPos;
use pumpkin_world::world::BlockFlags;
use rand::RngExt;

use super::move_to_target_pos::{MoveToTargetPos, MoveToTargetPosGoal};
use super::{Controls, Goal, GoalFuture, ParentHandle};
use crate::entity::mob::Mob;
use crate::entity::passive::fox::FoxEntity;
use crate::world::World;

/// `Fox.FoxEatBerriesGoal.WAIT_TICKS` (`Fox.java:907`).
const WAIT_TICKS: i32 = 40;
/// `acceptedDistance` (`Fox.java:914-916`).
const ACCEPTED_DISTANCE: f64 = 2.0;
/// Minimum `SweetBerryBushBlock.AGE` a fox will harvest (`Fox.java:926`).
const MIN_BERRY_AGE: u8 = 2;

pub struct FoxEatBerriesGoal {
    move_to_target_pos_goal: MoveToTargetPosGoal<Self>,
    ticks_waited: i32,
}

impl FoxEatBerriesGoal {
    #[must_use]
    pub fn new(speed: f64, search_range: i32, vertical_search_range: i32) -> Box<Self> {
        let mut this = Box::new(Self {
            move_to_target_pos_goal: MoveToTargetPosGoal::new(
                ParentHandle::none(),
                speed,
                search_range,
                vertical_search_range,
            ),
            ticks_waited: 0,
        });
        // SAFETY: mirrors `turtle_lay_egg.rs` - the boxed allocation outlives the handle.
        this.move_to_target_pos_goal.move_to_target_pos = unsafe { ParentHandle::new(&this) };
        this
    }

    /// `isValidTarget` (`Fox.java:924-928`).
    fn is_berry_block(world: &Arc<World>, pos: &BlockPos) -> bool {
        let (block, state_id) = world.get_block_and_state_id(pos);
        if block == &Block::SWEET_BERRY_BUSH {
            let props = NetherWartLikeProperties::from_state_id(state_id, block);
            return props.age >= MIN_BERRY_AGE;
        }
        Self::has_glow_berries(block, state_id)
    }

    /// `CaveVines.hasGlowBerries` (`CaveVines.java:48-50`).
    fn has_glow_berries(block: &Block, state_id: BlockStateId) -> bool {
        if block == &Block::CAVE_VINES {
            CaveVinesLikeProperties::from_state_id(state_id, block).berries
        } else if block == &Block::CAVE_VINES_PLANT {
            CaveVinesPlantLikeProperties::from_state_id(state_id, block).berries
        } else {
            false
        }
    }

    /// `onReachedTarget` (`Fox.java:943-952`).
    async fn on_reached_target(&self, mob: &dyn Mob) {
        let entity = mob.get_entity();
        let world = entity.world.load_full();
        if !world.level_info.load().game_rules.mob_griefing {
            return;
        }
        let pos = self.move_to_target_pos_goal.target_pos;
        let (block, state_id) = world.get_block_and_state_id(&pos);
        if block == &Block::SWEET_BERRY_BUSH {
            self.pick_sweet_berries(mob, &world, pos, state_id).await;
        } else if Self::has_glow_berries(block, state_id) {
            Self::pick_glow_berry(mob, &world, pos, block, state_id).await;
        }
    }

    /// `pickSweetBerries` (`Fox.java:958-973`).
    async fn pick_sweet_berries(
        &self,
        mob: &dyn Mob,
        world: &Arc<World>,
        pos: BlockPos,
        state_id: BlockStateId,
    ) {
        let props = NetherWartLikeProperties::from_state_id(state_id, &Block::SWEET_BERRY_BUSH);
        let age = props.age;
        let mut count = 1 + mob.get_random().random_range(0..2) + i32::from(age == 3);

        if let Some(fox) = mob.cast_any().downcast_ref::<FoxEntity>()
            && fox.can_hold_item()
        {
            fox.set_held_item(ItemStack::new(1, &Item::SWEET_BERRIES))
                .await;
            count -= 1;
        }

        if count > 0 {
            world
                .clone()
                .drop_stack(&pos, ItemStack::new(count as u8, &Item::SWEET_BERRIES))
                .await;
        }

        let entity_pos = mob.get_entity().pos.load();
        world.play_sound(
            Sound::BlockSweetBerryBushPickBerries,
            SoundCategory::Blocks,
            &entity_pos,
        );
        let picked = NetherWartLikeProperties { age: 1 };
        world
            .set_block_state(
                &pos,
                picked.to_state_id(&Block::SWEET_BERRY_BUSH),
                BlockFlags::NOTIFY_LISTENERS,
            )
            .await;
    }

    /// `pickGlowBerry` -> `CaveVines.use` (`Fox.java:954-956`, `CaveVines.java:23-45`).
    async fn pick_glow_berry(
        mob: &dyn Mob,
        world: &Arc<World>,
        pos: BlockPos,
        block: &Block,
        state_id: BlockStateId,
    ) {
        let picked_state = if block == &Block::CAVE_VINES {
            let props = CaveVinesLikeProperties::from_state_id(state_id, block);
            CaveVinesLikeProperties {
                age: props.age,
                berries: false,
            }
            .to_state_id(block)
        } else {
            CaveVinesPlantLikeProperties { berries: false }.to_state_id(block)
        };

        world
            .clone()
            .drop_stack(&pos, ItemStack::new(1, &Item::GLOW_BERRIES))
            .await;
        let _ = mob;
        world.play_sound(
            Sound::BlockCaveVinesPickBerries,
            SoundCategory::Blocks,
            &pos.to_centered_f64(),
        );
        world
            .set_block_state(&pos, picked_state, BlockFlags::NOTIFY_LISTENERS)
            .await;
    }
}

impl Goal for FoxEatBerriesGoal {
    fn can_start<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        Box::pin(async move {
            let is_sleeping = mob
                .cast_any()
                .downcast_ref::<FoxEntity>()
                .is_some_and(FoxEntity::is_sleeping);
            if is_sleeping {
                return false;
            }
            self.move_to_target_pos_goal.can_start(mob).await
        })
    }

    fn should_continue<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        Box::pin(async move { self.move_to_target_pos_goal.should_continue(mob).await })
    }

    fn start<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async move {
            self.ticks_waited = 0;
            if let Some(fox) = mob.cast_any().downcast_ref::<FoxEntity>() {
                fox.set_sitting(false);
            }
            self.move_to_target_pos_goal.start(mob).await;
        })
    }

    fn tick<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async move {
            if self.move_to_target_pos_goal.reached {
                if self.ticks_waited >= WAIT_TICKS {
                    self.on_reached_target(mob).await;
                } else {
                    self.ticks_waited += 1;
                }
            } else if { mob.get_random().random::<f32>() } < 0.05 {
                let entity = mob.get_entity();
                entity.world.load().play_sound(
                    Sound::EntityFoxSniff,
                    SoundCategory::Neutral,
                    &entity.pos.load(),
                );
            }
            self.move_to_target_pos_goal.tick(mob).await;
        })
    }

    fn should_run_every_tick(&self) -> bool {
        true
    }

    fn controls(&self) -> Controls {
        self.move_to_target_pos_goal.controls()
    }
}

impl MoveToTargetPos for FoxEatBerriesGoal {
    fn is_target_pos<'a>(
        &'a self,
        world: Arc<World>,
        block_pos: BlockPos,
    ) -> Pin<Box<dyn Future<Output = bool> + Send + 'a>> {
        Box::pin(async move { Self::is_berry_block(&world, &block_pos) })
    }

    fn get_desired_distance_to_target(&self) -> f64 {
        ACCEPTED_DISTANCE
    }
}
