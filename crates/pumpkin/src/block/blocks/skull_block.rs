use crate::block::blocks::redstone::block_receives_redstone_power;
use crate::block::{BlockBehaviour, BlockFuture, BlockMetadata, OnNeighborUpdateArgs, OnPlaceArgs};
use crate::entity::EntityBase;
use pumpkin_data::BlockId;
use pumpkin_data::BlockStateId;
use pumpkin_data::block_properties::{BlockProperties, HorizontalFacing};
use pumpkin_world::world::BlockFlags;

type SkeletonSkullLikeProperties = pumpkin_data::block_properties::SkeletonSkullLikeProperties;

pub struct SkullBlock;

impl BlockMetadata for SkullBlock {
    fn ids() -> Box<[BlockId]> {
        [
            BlockId::SKELETON_SKULL,
            BlockId::PLAYER_HEAD,
            BlockId::ZOMBIE_HEAD,
            BlockId::CREEPER_HEAD,
            BlockId::PIGLIN_HEAD,
            BlockId::DRAGON_HEAD,
        ]
        .into()
    }
}

use crate::block::PlacedArgs;
use crate::block::entities::skull::SkullBlockEntity;
use std::sync::Arc;

impl BlockBehaviour for SkullBlock {
    fn placed<'a>(&'a self, args: PlacedArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async move {
            let entity = SkullBlockEntity::new(*args.position);
            args.world.add_block_entity(Arc::new(entity));
        })
    }

    fn on_place<'a>(&'a self, args: OnPlaceArgs<'a>) -> BlockFuture<'a, BlockStateId> {
        Box::pin(async move {
            let mut props = SkeletonSkullLikeProperties::default(args.block);
            props.rotation = args.player.get_entity().get_rotation_16();
            props.powered = block_receives_redstone_power(args.world, args.position).await;
            props.to_state_id(args.block)
        })
    }

    fn on_neighbor_update<'a>(&'a self, args: OnNeighborUpdateArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async move {
            let state = args.world.get_block_state(args.position);
            let mut props = SkeletonSkullLikeProperties::from_state_id(state.id, args.block);
            let is_receiving_power = block_receives_redstone_power(args.world, args.position).await;
            if props.powered != is_receiving_power {
                props.powered = is_receiving_power;
                args.world
                    .set_block_state(
                        args.position,
                        props.to_state_id(args.block),
                        BlockFlags::NOTIFY_LISTENERS,
                    )
                    .await;
            }
        })
    }
}

type SkeletonWallSkullLikeProperties =
    pumpkin_data::block_properties::SkeletonWallSkullLikeProperties;

/// `WallSkullBlock`: the wall-mounted skull variants.
///
/// They share `AbstractSkullBlock`'s block entity and redstone-powered state with the floor
/// skulls, but carry `facing` where those carry `rotation` (WallSkullBlock.java:22), so they
/// cannot simply join `SkullBlock`'s id list.
pub struct WallSkullBlock;

impl BlockMetadata for WallSkullBlock {
    fn ids() -> Box<[BlockId]> {
        [
            BlockId::SKELETON_WALL_SKULL,
            BlockId::WITHER_SKELETON_WALL_SKULL,
            BlockId::PLAYER_WALL_HEAD,
            BlockId::ZOMBIE_WALL_HEAD,
            BlockId::CREEPER_WALL_HEAD,
            BlockId::PIGLIN_WALL_HEAD,
            BlockId::DRAGON_WALL_HEAD,
        ]
        .into()
    }
}

impl BlockBehaviour for WallSkullBlock {
    fn placed<'a>(&'a self, args: PlacedArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async move {
            let entity = SkullBlockEntity::new(*args.position);
            args.world.add_block_entity(Arc::new(entity));
        })
    }

    fn on_place<'a>(&'a self, args: OnPlaceArgs<'a>) -> BlockFuture<'a, BlockStateId> {
        Box::pin(async move {
            let mut props = SkeletonWallSkullLikeProperties::default(args.block);
            // `WallSkullBlock.getStateForPlacement` faces away from the wall it is placed on.
            props.facing = args
                .direction
                .to_horizontal_facing()
                .unwrap_or(HorizontalFacing::North);
            props.powered = block_receives_redstone_power(args.world, args.position).await;
            props.to_state_id(args.block)
        })
    }

    fn on_neighbor_update<'a>(&'a self, args: OnNeighborUpdateArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async move {
            let state = args.world.get_block_state(args.position);
            let mut props = SkeletonWallSkullLikeProperties::from_state_id(state.id, args.block);
            let is_receiving_power = block_receives_redstone_power(args.world, args.position).await;
            if props.powered != is_receiving_power {
                props.powered = is_receiving_power;
                args.world
                    .set_block_state(
                        args.position,
                        props.to_state_id(args.block),
                        BlockFlags::NOTIFY_LISTENERS,
                    )
                    .await;
            }
        })
    }
}
