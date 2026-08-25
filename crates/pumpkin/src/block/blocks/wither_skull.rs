use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::{
    Block, BlockDirection, BlockStateId,
    entity::EntityType,
    item::Item,
    tag::{self, Taggable},
    world::WorldEvent,
};
use pumpkin_macros::pumpkin_block;
use pumpkin_util::Difficulty;
use pumpkin_util::math::position::BlockPos;
use pumpkin_world::world::BlockFlags;
use std::sync::Arc;

use crate::{
    block::{
        BlockBehaviour, BlockFuture, OnPlaceArgs, PlacedArgs,
        blocks::{skull_block::SkullBlock, skull_block::WallSkullBlock},
    },
    entity::{Entity, boss::wither::WitherEntity},
};

#[pumpkin_block("wither_skeleton_skull")]
pub struct WitherSkeletonSkullBlock;

impl BlockBehaviour for WitherSkeletonSkullBlock {
    fn on_place<'a>(&'a self, args: OnPlaceArgs<'a>) -> BlockFuture<'a, BlockStateId> {
        SkullBlock::on_place(&SkullBlock, args)
    }

    fn placed<'a>(&'a self, args: PlacedArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async move {
            let entity = crate::block::entities::skull::SkullBlockEntity::new(*args.position);
            args.world.add_block_entity(std::sync::Arc::new(entity));
            check_spawn(args.world, args.position).await;
        })
    }
}

/// The wall-mounted counterpart of `WitherSkeletonSkullBlock`.
///
/// Vanilla `WitherWallSkullBlock` keeps the wall-skull placement state from
/// `WallSkullBlock`, but overrides `setPlacedBy` to call
/// `WitherSkullBlock.checkSpawn` (`WitherWallSkullBlock.java:12-27`).
#[pumpkin_block("wither_skeleton_wall_skull")]
pub struct WitherWallSkullBlock;

impl BlockBehaviour for WitherWallSkullBlock {
    /// `WitherWallSkullBlock` inherits `WallSkullBlock` placement state and
    /// horizontal facing (`WallSkullBlock.java:41-58`; constructor selected by
    /// `WitherWallSkullBlock.java:20-22`).
    fn on_place<'a>(&'a self, args: OnPlaceArgs<'a>) -> BlockFuture<'a, BlockStateId> {
        WallSkullBlock::on_place(&WallSkullBlock, args)
    }

    /// `WitherWallSkullBlock.setPlacedBy` calls `WitherSkullBlock.checkSpawn`
    /// (`WitherWallSkullBlock.java:24-27`). Pumpkin's placement lifecycle exposes
    /// this as `BlockBehaviour::placed`, so retain the wall skull block entity
    /// setup and run the spawn check immediately afterward.
    fn placed<'a>(&'a self, args: PlacedArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async move {
            let entity = crate::block::entities::skull::SkullBlockEntity::new(*args.position);
            args.world.add_block_entity(std::sync::Arc::new(entity));
            check_spawn(args.world, args.position).await;
        })
    }
}

fn is_wither_skull(block: &Block) -> bool {
    block == &Block::WITHER_SKELETON_SKULL || block == &Block::WITHER_SKELETON_WALL_SKULL
}

fn is_wither_base(block: &Block) -> bool {
    block.has_tag(&tag::Block::MINECRAFT_WITHER_SUMMON_BASE_BLOCKS)
}

/// `getOrCreateWitherBase` (`WitherSkullBlock.java:111-121`): `aisle("   ", "###", "~#~")`.
/// The base row (one below the skull row) is three `WITHER_SUMMON_BASE_BLOCKS`; the row
/// below that has a base block only in the center, and its two side cells (`'~'`) must be
/// air.
fn base_pattern_matches(
    world: &crate::world::World,
    center: &BlockPos,
    direction: BlockDirection,
) -> bool {
    let opposite = direction.opposite();
    let top_middle = center.down();
    let base = top_middle.down();
    let arm1 = top_middle.offset(direction.to_offset());
    let arm2 = top_middle.offset(opposite.to_offset());
    let under_arm1 = base.offset(direction.to_offset());
    let under_arm2 = base.offset(opposite.to_offset());
    is_wither_base(world.get_block(&top_middle))
        && is_wither_base(world.get_block(&base))
        && is_wither_base(world.get_block(&arm1))
        && is_wither_base(world.get_block(&arm2))
        && world.get_block_state(&under_arm1).is_air()
        && world.get_block_state(&under_arm2).is_air()
}

fn full_pattern_matches(
    world: &crate::world::World,
    center: &BlockPos,
    direction: BlockDirection,
) -> bool {
    let opposite = direction.opposite();
    let skull1 = center.down().offset(direction.to_offset()).up();
    let skull2 = center.down().offset(opposite.to_offset()).up();
    base_pattern_matches(world, center, direction)
        && is_wither_skull(world.get_block(center))
        && is_wither_skull(world.get_block(&skull1))
        && is_wither_skull(world.get_block(&skull2))
}

/// `WitherSkullBlock.canSpawnMob` (`WitherSkullBlock.java:84-90`): checks the item, the
/// minimum Y, non-peaceful difficulty, and the wither-summon-base-blocks base pattern.
pub fn can_spawn_mob(world: &crate::world::World, pos: &BlockPos, item_stack: &ItemStack) -> bool {
    item_stack.item.id == Item::WITHER_SKELETON_SKULL.id
        && pos.0.y >= world.min_y + 2
        && world.level_info.load().difficulty != Difficulty::Peaceful
        && [BlockDirection::North, BlockDirection::West]
            .into_iter()
            .any(|direction| {
                [
                    pos.offset(direction.opposite().to_offset()),
                    *pos,
                    pos.offset(direction.to_offset()),
                ]
                .into_iter()
                .any(|center| base_pattern_matches(world, &center, direction))
            })
}

/// `WitherSkullBlock.checkSpawn` (`WitherSkullBlock.java:42-82`): validates the placed skull,
/// difficulty, height, and full pattern before clearing the pattern and spawning an invulnerable
/// wither.
pub async fn check_spawn(world: &Arc<crate::world::World>, pos: &BlockPos) {
    if !is_wither_skull(world.get_block(pos))
        || pos.0.y < world.min_y
        || world.level_info.load().difficulty == Difficulty::Peaceful
    {
        return;
    }

    for direction in [BlockDirection::North, BlockDirection::West] {
        let opposite = direction.opposite();
        for center in [
            pos.offset(opposite.to_offset()),
            *pos,
            pos.offset(direction.to_offset()),
        ] {
            if !full_pattern_matches(world, &center, direction) {
                continue;
            }

            let top_middle = center.down();
            let pattern = [
                center,
                center.offset(direction.to_offset()).up(),
                center.offset(opposite.to_offset()).up(),
                top_middle,
                top_middle.offset(direction.to_offset()),
                top_middle.offset(opposite.to_offset()),
                top_middle.down(),
            ];
            for pattern_pos in pattern {
                world
                    .set_block_state(
                        &pattern_pos,
                        Block::AIR.default_state.id,
                        BlockFlags::NOTIFY_ALL,
                    )
                    .await;
                world.sync_world_event(
                    WorldEvent::ParticlesDestroyBlock,
                    pattern_pos,
                    Block::SOUL_SAND.default_state.id.as_u16().into(),
                );
            }

            // `checkSpawn` (:56-58): spawnPos is `match.getBlock(1, 2, 0)` - the pattern's
            // bottom row (index 2 of 3), i.e. the base block directly under `top_middle`,
            // not `top_middle` itself. Pre-existing bug fixed here: the wither was spawning
            // one block too high. Vanilla also offsets Y by +0.55 rather than centering, and
            // sets a yaw depending on the match's forward axis; the yaw is not ported.
            let entity = Entity::new(
                world.clone(),
                top_middle.down().to_centered_f64(),
                &EntityType::WITHER,
            );
            let wither = WitherEntity::new(entity);
            wither.make_invulnerable();
            world.spawn_entity(wither).await;
            return;
        }
    }
}
