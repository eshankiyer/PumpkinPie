use std::sync::Arc;

use pumpkin_data::block_properties::{BlockProperties, NetherWartLikeProperties};
use pumpkin_data::dimension::Dimension;
use pumpkin_data::tag::{self, Taggable};
use pumpkin_data::{Block, BlockDirection, BlockId, BlockState, BlockStateId};
use pumpkin_util::GameMode;
use pumpkin_util::math::position::BlockPos;
use pumpkin_world::tick::TickPriority;
use pumpkin_world::world::BlockFlags;
use rand::RngExt;

use crate::block::{
    BlockBehaviour, BlockMetadata, BrokenArgs, OnNeighborUpdateArgs, OnScheduledTickArgs,
    PlacedArgs, RandomTickArgs,
};
use crate::world::World;

/// Melts ice at the given position into water (or removes it in ultrawarm dimensions like the Nether).
pub fn melt(world: &Arc<World>, position: &BlockPos) {
    if world.dimension == Dimension::THE_NETHER {
        world.set_block_state(position, BlockStateId::AIR, BlockFlags::NOTIFY_ALL);
    } else {
        world.set_block_state(
            position,
            Block::WATER.default_state.id,
            BlockFlags::NOTIFY_ALL,
        );
        // `level.neighborChanged(pos, Blocks.WATER, null)`: tell the fresh water source about its
        // own surroundings so that it starts flowing right away.
        world.update_neighbor(position, &Block::WATER);
    }
}

/// Returns whether the frosted ice block at `pos` has fewer adjacent frosted ice blocks than `limit`.
pub fn fewer_neighbors_than(world: &World, pos: &BlockPos, limit: usize) -> bool {
    let mut count = 0;
    for dir in BlockDirection::all() {
        let neighbor_pos = pos.offset(dir.to_offset());
        if world.get_block(&neighbor_pos) == &Block::FROSTED_ICE {
            count += 1;
            if count >= limit {
                return false;
            }
        }
    }
    true
}

fn slightly_melt(world: &Arc<World>, pos: &BlockPos, block: &Block, age: u8) -> bool {
    if age < 3 {
        let mut new_props = NetherWartLikeProperties::default(block);
        new_props.r#age = age + 1;
        world.set_block_state(pos, new_props.to_state_id(block), BlockFlags::NOTIFY_ALL);
        false
    } else {
        melt(world, pos);
        true
    }
}

pub struct IceBlock;

impl BlockMetadata for IceBlock {
    fn ids() -> Box<[BlockId]> {
        [BlockId::ICE].into()
    }
}

impl BlockBehaviour for IceBlock {
    /// `IceBlock#playerDestroy`: without Silk Touch, breaking ice never drops an ice item and
    /// instead behaves like a melt - evaporating in the Nether, or turning into a water source
    /// if the block below can support it. Silk Touch (tagged `prevents_ice_melting`) suppresses
    /// this entirely, leaving the normal item drop from the loot table intact.
    ///
    /// `ServerPlayerGameMode#destroyBlock` never calls `playerDestroy` for creative players
    /// (`Player#preventsBlockDrops` returns true for `instabuild`), so this hook must reproduce
    /// that guard itself, since Pumpkin's `broken` callback fires on the creative path too.
    fn broken(&self, args: BrokenArgs<'_>) {
        if args.player.gamemode.load() == GameMode::Creative {
            return;
        }

        let tool = args.player.inventory().held_item();
        let silk_touched = tool
            .get_data_component::<pumpkin_data::data_component_impl::EnchantmentsImpl>()
            .is_some_and(|enchantments| {
                enchantments.enchantment.iter().any(|(enchantment, _)| {
                    enchantment.has_tag(&tag::Enchantment::MINECRAFT_PREVENTS_ICE_MELTING)
                })
            });
        if silk_touched {
            return;
        }

        if args.world.dimension == Dimension::THE_NETHER {
            // The block was already replaced with air by `break_block`; nothing further to do.
            return;
        }

        let below_pos = args.position.down();
        let below_state = args.world.get_block_state(&below_pos);
        let below_block = args.world.get_block(&below_pos);
        if supports_water(below_block, below_state) {
            // `level.setBlockAndUpdate(pos, meltsInto())`: unlike `melt`, `playerDestroy`
            // does not also call `neighborChanged` on the new water.
            args.world.set_block_state(
                args.position,
                Block::WATER.default_state.id,
                BlockFlags::NOTIFY_ALL,
            );
        }
    }

    fn random_tick(&self, args: RandomTickArgs<'_>) {
        // `IceBlock.randomTick` melts once the block light exceeds
        // `11 - state.getLightBlock()`. Java's `getLightBlock()` is `BlockState::opacity`
        // here, and ice has an opacity of 1, so the effective threshold is a block light
        // level above 10 (one lower than the flat `> 11` used for snow layers).
        let opacity = args.world.get_block_state(args.position).opacity;
        if args.world.get_block_light_level(args.position).unwrap_or(0)
            > 11u8.saturating_sub(opacity)
        {
            melt(args.world, args.position);
        }
    }
}

/// `IceBlock#playerDestroy`'s below-block check: `belowState.blocksMotion() || belowState.liquid()`.
/// `blocksMotion()` is legacy-solid minus cobweb/bamboo sapling (`BlockBehaviour.java:542`).
fn supports_water(below_block: &Block, below_state: &BlockState) -> bool {
    below_state.is_liquid()
        || (below_state.is_solid()
            && below_block != &Block::COBWEB
            && below_block != &Block::BAMBOO_SAPLING)
}

pub struct FrostedIceBlock;

impl BlockMetadata for FrostedIceBlock {
    fn ids() -> Box<[BlockId]> {
        [BlockId::FROSTED_ICE].into()
    }
}

impl BlockBehaviour for FrostedIceBlock {
    fn placed(&self, args: PlacedArgs<'_>) {
        let delay = rand::rng().random_range(60..=120);
        args.world
            .schedule_block_tick(args.block, *args.position, delay, TickPriority::Normal);
    }

    fn on_scheduled_tick(&self, args: OnScheduledTickArgs<'_>) {
        let should_check_melt =
            rand::rng().random_range(0..3) == 0 || fewer_neighbors_than(args.world, args.position, 4);

        if should_check_melt {
            let state_id = args.world.get_block_state_id(args.position);
            let state = args.world.get_block_state(args.position);
            let props = NetherWartLikeProperties::from_state_id(state_id, args.block);
            let age = props.r#age;

            let brightness = if args.world.dimension == Dimension::THE_END {
                args.world.get_block_light_level(args.position).unwrap_or(0)
            } else {
                args.world.get_max_local_raw_brightness(args.position)
            };

            let threshold = 11u8.saturating_sub(age).saturating_sub(state.opacity);
            if brightness > threshold && slightly_melt(args.world, args.position, args.block, age) {
                for dir in BlockDirection::all() {
                    let neighbor_pos = args.position.offset(dir.to_offset());
                    let (neighbor_block, neighbor_state_id) =
                        args.world.get_block_and_state_id(&neighbor_pos);
                    if neighbor_block == &Block::FROSTED_ICE {
                        let neighbor_props = NetherWartLikeProperties::from_state_id(
                            neighbor_state_id,
                            neighbor_block,
                        );
                        if !slightly_melt(
                            args.world,
                            &neighbor_pos,
                            neighbor_block,
                            neighbor_props.r#age,
                        ) {
                            let delay = rand::rng().random_range(20..=40);
                            args.world.schedule_block_tick(
                                neighbor_block,
                                neighbor_pos,
                                delay,
                                TickPriority::Normal,
                            );
                        }
                    }
                }
                return;
            }
        }

        let delay = rand::rng().random_range(20..=40);
        args.world
            .schedule_block_tick(args.block, *args.position, delay, TickPriority::Normal);
    }

    fn on_neighbor_update(&self, args: OnNeighborUpdateArgs<'_>) {
        if args.source_block == &Block::FROSTED_ICE
            && fewer_neighbors_than(args.world, args.position, 2)
        {
            melt(args.world, args.position);
        }
    }

    fn broken(&self, args: BrokenArgs<'_>) {
        IceBlock.broken(args);
    }
}

#[cfg(test)]
mod tests {
    use super::supports_water;
    use pumpkin_data::Block;

    #[test]
    fn stone_supports_water() {
        assert!(supports_water(&Block::STONE, Block::STONE.default_state));
    }

    #[test]
    fn water_supports_water() {
        assert!(supports_water(&Block::WATER, Block::WATER.default_state));
    }

    #[test]
    fn air_does_not_support_water() {
        assert!(!supports_water(&Block::AIR, Block::AIR.default_state));
    }

    #[test]
    fn cobweb_does_not_support_water() {
        assert!(!supports_water(&Block::COBWEB, Block::COBWEB.default_state));
    }

    #[test]
    fn bamboo_sapling_does_not_support_water() {
        assert!(!supports_water(
            &Block::BAMBOO_SAPLING,
            Block::BAMBOO_SAPLING.default_state,
        ));
    }
}
