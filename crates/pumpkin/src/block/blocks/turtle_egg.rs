use std::sync::Arc;

use pumpkin_data::block_properties::{BlockProperties, TurtleEggLikeProperties};
use pumpkin_data::entity::{EntityPose, EntityType};
use pumpkin_data::game_event::GameEvent;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_data::tag::Taggable;
use pumpkin_data::world::WorldEvent;
use pumpkin_data::{Block, BlockDirection, BlockStateId, tag};
use pumpkin_macros::pumpkin_block;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_world::tick::TickPriority;
use pumpkin_world::world::{BlockAccessor, BlockFlags};
use rand::{RngExt, rng};

use crate::block::{
    BlockBehaviour, BlockIsReplacing, BrokenArgs, CanPlaceAtArgs, CanUpdateAtArgs,
    GetStateForNeighborUpdateArgs, OnEntityStepArgs, OnLandedUponArgs, OnPlaceArgs,
    OnScheduledTickArgs, RandomTickArgs,
};
use crate::entity::EntityBase;
use crate::world::game_event::GameEventContext;
use crate::entity::r#type::from_type;
use crate::world::World;
use uuid::Uuid;

type TurtleEggProperties = TurtleEggLikeProperties;

#[pumpkin_block("minecraft:turtle_egg")]
pub struct TurtleEggBlock;

impl TurtleEggBlock {
    #[must_use]
    pub fn is_sand(world: &dyn BlockAccessor, pos: &BlockPos) -> bool {
        world.get_block(pos).has_tag(&tag::Block::MINECRAFT_SAND)
    }

    #[must_use]
    pub fn on_sand(world: &dyn BlockAccessor, pos: &BlockPos) -> bool {
        Self::is_sand(world, &pos.down())
    }

    #[must_use]
    pub fn can_destroy_egg(world: &World, entity: &dyn EntityBase) -> bool {
        let entity_type = entity.get_entity().entity_type;
        if entity_type.id == EntityType::TURTLE.id || entity_type.id == EntityType::BAT.id {
            return false;
        }
        if entity.get_living_entity().is_none() {
            return false;
        }
        if entity_type.id == EntityType::PLAYER.id {
            return true;
        }
        world.level_info.load().game_rules.mob_griefing
    }

    pub fn decrease_eggs(
        world: &Arc<World>,
        pos: &BlockPos,
        state_id: BlockStateId,
        block: &Block,
    ) {
        world.play_sound_raw(
            Sound::EntityTurtleEggBreak as u16,
            SoundCategory::Blocks,
            &pos.to_f64(),
            0.7,
            0.9 + rand::random::<f32>() * 0.2,
        );

        let props = TurtleEggProperties::from_state_id(state_id, block);
        if props.eggs <= 1 {
            world.break_block(pos, None, BlockFlags::SKIP_DROPS);
        } else {
            let mut new_props = props;
            new_props.eggs -= 1;
            world.set_block_state(
                pos,
                new_props.to_state_id(block),
                BlockFlags::NOTIFY_LISTENERS,
            );
            let world = world.clone();
            let position = pos.to_centered_f64();
            tokio::spawn(async move {
                crate::world::game_event::emit_game_event(
                    &world,
                    GameEvent::BlockDestroy,
                    position,
                    GameEventContext::none(),
                )
                .await;
            });
            world.sync_world_event(
                WorldEvent::ParticlesDestroyBlock,
                *pos,
                i32::from(state_id.as_u16()),
            );
        }
    }

    pub fn destroy_egg(
        world: &Arc<World>,
        pos: &BlockPos,
        state_id: BlockStateId,
        block: &Block,
        entity: &dyn EntityBase,
        randomness: u32,
    ) {
        if Self::can_destroy_egg(world, entity)
            && (randomness <= 1 || rand::random::<u32>().is_multiple_of(randomness))
        {
            Self::decrease_eggs(world, pos, state_id, block);
        }
    }
}

impl BlockBehaviour for TurtleEggBlock {
    fn on_place(&self, args: OnPlaceArgs<'_>) -> BlockStateId {
        if args.player.get_entity().pose.load() != EntityPose::Crouching
            && let BlockIsReplacing::Itself(state_id) = args.replacing
        {
            let mut properties = TurtleEggProperties::from_state_id(state_id, args.block);
            if properties.eggs < 4 {
                properties.eggs += 1;
            }
            return properties.to_state_id(args.block);
        }

        let properties = TurtleEggProperties::default(args.block);
        properties.to_state_id(args.block)
    }

    fn can_place_at(&self, args: CanPlaceAtArgs<'_>) -> bool {
        can_place_at(args.block_accessor, args.position)
    }

    fn can_update_at(&self, args: CanUpdateAtArgs<'_>) -> bool {
        let b = BlockAccessor::get_block(args.world, args.position);
        args.player.get_entity().pose.load() != EntityPose::Crouching
            && TurtleEggProperties::from_state_id(args.state_id, args.block).eggs < 4
            && args.block.id == b.id
    }

    fn get_state_for_neighbor_update(
        &self,
        args: GetStateForNeighborUpdateArgs<'_>,
    ) -> BlockStateId {
        if !can_place_at(args.world, args.position) {
            args.world
                .schedule_block_tick(args.block, *args.position, 1, TickPriority::Normal);
        }
        args.state_id
    }

    fn on_scheduled_tick(&self, args: OnScheduledTickArgs<'_>) {
        if !can_place_at(args.world.as_ref(), args.position) {
            args.world
                .break_block(args.position, None, BlockFlags::empty());
        }
    }

    fn on_entity_step(&self, args: OnEntityStepArgs<'_>) {
        // Vanilla `stepOn` (TurtleEggBlock.java:56-62): anything not stepping carefully
        // (`isSteppingCarefully` == shift-key-down, `Entity.java:2681-2683`) can crush an egg,
        // rolled each tick against randomness 100.
        if !args.entity.get_entity().is_sneaking() {
            Self::destroy_egg(
                args.world,
                args.position,
                args.state.id,
                args.block,
                args.entity,
                100,
            );
        }
    }

    fn on_landed_upon(&self, args: OnLandedUponArgs<'_>) {
        if let Some(living) = args.entity.get_living_entity() {
            living.handle_fall_damage(args.entity, args.fall_distance, 1.0);
        }

        if args.entity.get_entity().entity_type.id != EntityType::ZOMBIE.id {
            let entity_pos = args.entity.get_entity().pos.load();
            let pos = BlockPos(Vector3::new(
                entity_pos.x.floor() as i32,
                entity_pos.y.floor() as i32,
                entity_pos.z.floor() as i32,
            ));
            let (block, state) = args.world.get_block_and_state(&pos);
            if block == &Block::TURTLE_EGG {
                Self::destroy_egg(args.world, &pos, state.id, block, args.entity, 3);
            }
        }
    }

    fn random_tick(&self, args: RandomTickArgs<'_>) {
        if !Self::on_sand(args.world.as_ref(), args.position) {
            return;
        }

        let state_id = args.world.get_block_state_id(args.position);
        let mut props = TurtleEggProperties::from_state_id(state_id, args.block);

        if props.hatch < 2 {
            props.hatch += 1;
            args.world.set_block_state(
                args.position,
                props.to_state_id(args.block),
                BlockFlags::NOTIFY_ALL,
            );

            args.world.play_sound(
                Sound::EntityTurtleEggCrack,
                SoundCategory::Blocks,
                &args.position.to_f64(),
            );
        } else {
            args.world
                .break_block(args.position, None, BlockFlags::SKIP_DROPS);

            args.world.play_sound(
                Sound::EntityTurtleEggHatch,
                SoundCategory::Blocks,
                &args.position.to_f64(),
            );

            if args.world.level_info.load().game_rules.spawn_mobs {
                for i in 0..props.eggs {
                    let spawn_pos = Vector3::new(
                        args.position.0.x as f64 + 0.3 + f64::from(i) * 0.2,
                        args.position.0.y as f64,
                        args.position.0.z as f64 + 0.3,
                    );
                    let turtle =
                        from_type(&EntityType::TURTLE, spawn_pos, args.world, Uuid::new_v4());
                    turtle.get_entity().set_age(-24000);
                    args.world.spawn_entity_non_save(turtle);
                }
            }
        }
    }

    /// Vanilla `playerDestroy` (TurtleEggBlock.java:142-152): after the normal break
    /// (loot handled by the engine), the cluster loses exactly one egg.
    fn broken(&self, args: BrokenArgs<'_>) {
        {
            Self::decrease_eggs(
                args.world,
                args.position,
                args.state.id,
                args.block,
            );
        }
    }

}

fn can_place_at(block_accessor: &dyn BlockAccessor, position: &BlockPos) -> bool {
    let (support_block, state) = block_accessor.get_block_and_state(&position.down());
    support_block.has_tag(&tag::Block::MINECRAFT_SAND) || state.is_center_solid(BlockDirection::Up)
}
