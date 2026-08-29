use std::sync::Arc;

use pumpkin_data::block_properties::{Axis, BlockProperties, CreakingHeartLikeProperties, CreakingHeartState, PaleOakWoodLikeProperties};
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_data::{Block, BlockDirection, BlockId, BlockStateId};
use pumpkin_data::tag::{self, Taggable};
use pumpkin_macros::pumpkin_block;
use pumpkin_util::GameMode;
use pumpkin_util::math::position::BlockPos;
use pumpkin_world::tick::TickPriority;
use pumpkin_world::world::BlockFlags;
use rand::RngExt;

use crate::block::entities::creaking_heart::CreakingHeartBlockEntity;
use crate::block::{
    BlockBehaviour, BrokenArgs, ExplodeArgs, GetComparatorOutputArgs,
    GetStateForNeighborUpdateArgs, OnNeighborUpdateArgs, OnPlaceArgs, OnScheduledTickArgs,
    PlacedArgs,
};
use crate::entity::experience_orb::ExperienceOrbEntity;
use crate::world::World;

/// `net.minecraft.world.level.block.CreakingHeartBlock`.
pub struct CreakingHeartBlock;

impl CreakingHeartBlock {
    const fn is_pale_oak_log(id: BlockId) -> bool {
        matches!(id, BlockId::PALE_OAK_LOG | BlockId::STRIPPED_PALE_OAK_LOG)
    }

    fn check_active_logs(world: &dyn pumpkin_world::world::BlockAccessor, pos: &BlockPos, axis: Axis) -> bool {
        let (a, b) = match axis {
            Axis::X => (pos.west(), pos.east()), Axis::Y => (pos.down(), pos.up()),
            Axis::Z => (pos.north(), pos.south()),
        };
        Self::is_pale_oak_log(world.get_block_state_id(&a).to_block_id())
            && Self::is_pale_oak_log(world.get_block_state_id(&b).to_block_id())
    }
}

/// `EnvironmentAttributes.CREAKING_ACTIVE`, driven by the vanilla day timeline.
fn creaking_active(world: &World) -> bool {
    let time = world.level_time.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    (12600..23401).contains(&time.time_of_day.rem_euclid(24000))
}

/// `CreakingHeartBlock.hasRequiredLogs`: both axis neighbours must be matching pale-oak logs.
fn has_required_logs(world: &World, pos: &BlockPos, axis: Axis) -> bool {
    for direction in match axis {
        Axis::X => [BlockDirection::West, BlockDirection::East],
        Axis::Y => [BlockDirection::Down, BlockDirection::Up],
        Axis::Z => [BlockDirection::North, BlockDirection::South],
    } {
        let neighbour = pos.offset(direction.to_offset());
        let (block, state) = world.get_block_and_state(&neighbour);
        if !block.has_tag(&tag::Block::MINECRAFT_PALE_OAK_LOGS) { return false; }
        if PaleOakWoodLikeProperties::from_state_id(state.id, block).axis != axis { return false; }
    }
    true
}

fn update_state(world: &World, pos: &BlockPos, props: &mut CreakingHeartLikeProperties) -> bool {
    if props.creaking_heart_state != CreakingHeartState::Uprooted || !has_required_logs(world, pos, props.axis) {
        return false;
    }
    props.creaking_heart_state = if creaking_active(world) { CreakingHeartState::Awake } else { CreakingHeartState::Dormant };
    true
}

impl BlockBehaviour for CreakingHeartBlock {
    /// `CreakingHeartBlock.playerWillDestroy` (`CreakingHeartBlock.java:175-187`).
    fn broken(&self, args: BrokenArgs<'_>) {
        let props = CreakingHeartLikeProperties::from_state_id(args.state.id, args.block);
        if props.r#natural && !matches!(args.player.gamemode.load(), GameMode::Creative | GameMode::Spectator) {
            ExperienceOrbEntity::spawn(args.world, args.position.to_centered_f64(), rand::rng().random_range(20..=24));
        }
        args.world.play_sound(Sound::BlockCreakingHeartBreak, SoundCategory::Blocks, &args.position.to_f64());
        args.world.drop_stack(args.position, ItemStack::new(1, &Item::CREAKING_HEART));
    }

    /// `CreakingHeartBlock.onExplosionHit` (`CreakingHeartBlock.java:159-172`).
    fn explode(&self, args: ExplodeArgs<'_>) {
        if let Some(entity) = args.world.get_block_entity(args.position)
            && let Some(heart) = entity.as_any().downcast_ref::<CreakingHeartBlockEntity>()
        {
            tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current().block_on(heart.remove_protector_on_removal(args.world));
            });
        }
    }

    fn on_place(&self, args: OnPlaceArgs<'_>) -> BlockStateId {
        let mut props = CreakingHeartLikeProperties::default(args.block);
        props.axis = match args.direction {
            BlockDirection::East | BlockDirection::West => Axis::X,
            BlockDirection::Up | BlockDirection::Down => Axis::Y,
            BlockDirection::North | BlockDirection::South => Axis::Z,
        };
        update_state(args.world, args.position, &mut props);
        props.to_state_id(args.block)
    }

    fn placed(&self, args: PlacedArgs<'_>) {
        args.world.add_block_entity(Arc::new(CreakingHeartBlockEntity::new(*args.position)));
        let state_id = args.world.get_block_state_id(args.position);
        let mut props = CreakingHeartLikeProperties::from_state_id(state_id, args.block);
        if Self::check_active_logs(args.world.as_ref(), args.position, props.axis) {
            props.creaking_heart_state = CreakingHeartState::Dormant;
            args.world.set_block_state(args.position, props.to_state_id(args.block), BlockFlags::NOTIFY_ALL);
            args.world.play_sound(Sound::BlockCreakingHeartSpawn, SoundCategory::Blocks, &args.position.to_f64());
        }
    }

    fn get_state_for_neighbor_update(&self, args: GetStateForNeighborUpdateArgs<'_>) -> BlockStateId {
        args.world.schedule_block_tick(args.block, *args.position, 1, TickPriority::Normal);
        args.state_id
    }

    fn on_scheduled_tick(&self, args: OnScheduledTickArgs<'_>) {
        let state = args.world.get_block_state(args.position);
        let mut props = CreakingHeartLikeProperties::from_state_id(state.id, args.block);
        if update_state(args.world, args.position, &mut props) {
            args.world.set_block_state(args.position, props.to_state_id(args.block), BlockFlags::NOTIFY_ALL);
        }
    }

    fn on_neighbor_update(&self, args: OnNeighborUpdateArgs<'_>) {
        let state_id = args.world.get_block_state_id(args.position);
        let mut props = CreakingHeartLikeProperties::from_state_id(state_id, args.block);
        let active = Self::check_active_logs(args.world.as_ref(), args.position, props.axis);
        let new_state = if active {
            if props.creaking_heart_state == CreakingHeartState::Uprooted {
                args.world.play_sound(Sound::BlockCreakingHeartSpawn, SoundCategory::Blocks, &args.position.to_f64());
            }
            CreakingHeartState::Dormant
        } else { CreakingHeartState::Uprooted };
        if props.creaking_heart_state != new_state {
            props.creaking_heart_state = new_state;
            args.world.set_block_state(args.position, props.to_state_id(args.block), BlockFlags::NOTIFY_ALL);
        }
    }

    fn get_comparator_output(&self, args: GetComparatorOutputArgs<'_>) -> Option<u8> {
        let props = CreakingHeartLikeProperties::from_state_id(args.state.id, args.block);
        if props.creaking_heart_state == CreakingHeartState::Uprooted { return Some(0); }
        Some(args.world.get_block_entity(args.position)
            .and_then(|entity| entity.as_any().downcast_ref::<CreakingHeartBlockEntity>()
                .map(CreakingHeartBlockEntity::get_analog_output_signal)).unwrap_or(0))
    }
}
