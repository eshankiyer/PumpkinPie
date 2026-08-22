//! `BlockBehaviour.isPathfindable(BlockState, PathComputationType)`.
//!
//! Ported from `net/minecraft/world/level/block/state/BlockBehaviour.java:138` together
//! with every block-class override of it in 26.2 (59 declaring classes).
//!
//! The vanilla default is `LAND | AIR -> !state.isCollisionShapeFullBlock(...)` and
//! `WATER -> state.getFluidState().is(FluidTags.WATER)`.
//!
//! `isCollisionShapeFullBlock` is exactly `BlockState::is_full_cube` here: the generated
//! `IS_FULL_CUBE` flag is set iff `collision_shapes == [full cube]` for every one of the
//! game's block states (checked against `assets/blocks.json`).
use pumpkin_data::{
    Block, BlockId, BlockState,
    block_properties::{
        BlockProperties, OakDoorLikeProperties, OakFenceGateLikeProperties,
        OakTrapdoorLikeProperties, SnowLikeProperties,
    },
    fluid::Fluid,
    tag::{self, Taggable},
};

/// `net/minecraft/world/level/pathfinder/PathComputationType.java`
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathComputationType {
    Land,
    Water,
    Air,
}

/// `BlockState.getFluidState().is(FluidTags.WATER)`.
///
/// A waterlogged block's `getFluidState` returns water (`SimpleWaterloggedBlock`
/// implementors all override `getFluidState` that way), so waterlogging counts.
///
/// Five block classes have no `waterlogged` property and instead return water source
/// unconditionally from `getFluidState`: `BubbleColumnBlock.java:73`, `KelpBlock.java:72`,
/// `KelpPlantBlock.java:35`, `SeagrassBlock.java:86`, `TallSeagrassBlock.java:79`.
#[must_use]
pub fn state_fluid_is_water(state: &BlockState) -> bool {
    Fluid::from_state_id(state.id).is_some_and(|fluid| fluid.has_tag(&tag::Fluid::MINECRAFT_WATER))
        || state.is_waterlogged()
        || matches!(
            Block::from_state_id(state.id).id,
            BlockId::BUBBLE_COLUMN
                | BlockId::KELP
                | BlockId::KELP_PLANT
                | BlockId::SEAGRASS
                | BlockId::TALL_SEAGRASS
        )
}

/// Blocks whose override is an unconditional `return false`.
fn always_blocked(block: &Block, id: BlockId) -> bool {
    // AbstractCauldronBlock:85, AbstractSkullBlock:59, AnvilBlock:124, BedBlock:289,
    // CampfireBlock:325, CandleCakeBlock:147, ChainBlock:80, CrossCollisionBlock:91
    // (FenceBlock:55, IronBarsBlock and its stained-pane subclass), FlowerPotBlock:133,
    // LanternBlock:103, StairBlock:225, WallBlock:101, WallHangingSignBlock:179.
    if block.has_tag(&tag::Block::MINECRAFT_CAULDRONS)
        || block.has_tag(&tag::Block::MINECRAFT_ANVIL)
        || block.has_tag(&tag::Block::MINECRAFT_BEDS)
        || block.has_tag(&tag::Block::MINECRAFT_CAMPFIRES)
        || block.has_tag(&tag::Block::MINECRAFT_CANDLE_CAKES)
        || block.has_tag(&tag::Block::MINECRAFT_CHAINS)
        || block.has_tag(&tag::Block::MINECRAFT_FENCES)
        || block.has_tag(&tag::Block::MINECRAFT_BARS)
        || block.has_tag(&tag::Block::C_GLASS_PANES)
        || block.has_tag(&tag::Block::MINECRAFT_FLOWER_POTS)
        || block.has_tag(&tag::Block::MINECRAFT_LANTERNS)
        || block.has_tag(&tag::Block::MINECRAFT_STAIRS)
        || block.has_tag(&tag::Block::MINECRAFT_WALLS)
        || block.has_tag(&tag::Block::MINECRAFT_WALL_HANGING_SIGNS)
    {
        return true;
    }

    matches!(
        id,
        // AbstractSkullBlock:59 - the `minecraft:skulls` tag omits the wall variants.
        BlockId::SKELETON_SKULL
            | BlockId::SKELETON_WALL_SKULL
            | BlockId::WITHER_SKELETON_SKULL
            | BlockId::WITHER_SKELETON_WALL_SKULL
            | BlockId::ZOMBIE_HEAD
            | BlockId::ZOMBIE_WALL_HEAD
            | BlockId::PLAYER_HEAD
            | BlockId::PLAYER_WALL_HEAD
            | BlockId::CREEPER_HEAD
            | BlockId::CREEPER_WALL_HEAD
            | BlockId::DRAGON_HEAD
            | BlockId::DRAGON_WALL_HEAD
            | BlockId::PIGLIN_HEAD
            | BlockId::PIGLIN_WALL_HEAD
            // AzaleaBlock:63
            | BlockId::AZALEA
            | BlockId::FLOWERING_AZALEA
            // BambooStalkBlock:68
            | BlockId::BAMBOO
            // BellBlock:284, BrewingStandBlock:106, CactusBlock:138, CakeBlock:154
            | BlockId::BELL
            | BlockId::BREWING_STAND
            | BlockId::CACTUS
            | BlockId::CAKE
            // ChestBlock:380 (+ TrappedChestBlock, CopperChestBlock), EnderChestBlock:167
            | BlockId::CHEST
            | BlockId::TRAPPED_CHEST
            | BlockId::ENDER_CHEST
            | BlockId::COPPER_CHEST
            | BlockId::EXPOSED_COPPER_CHEST
            | BlockId::WEATHERED_COPPER_CHEST
            | BlockId::OXIDIZED_COPPER_CHEST
            | BlockId::WAXED_COPPER_CHEST
            | BlockId::WAXED_EXPOSED_COPPER_CHEST
            | BlockId::WAXED_WEATHERED_COPPER_CHEST
            | BlockId::WAXED_OXIDIZED_COPPER_CHEST
            // ChorusPlantBlock:119, CocoaBlock:127, ComposterBlock:362, ConduitBlock:95
            | BlockId::CHORUS_PLANT
            | BlockId::COCOA
            | BlockId::COMPOSTER
            | BlockId::CONDUIT
            // DecoratedPotBlock:156, DirtPathBlock:78, DragonEggBlock:88, DriedGhastBlock:204
            | BlockId::DECORATED_POT
            | BlockId::DIRT_PATH
            | BlockId::DRAGON_EGG
            | BlockId::DRIED_GHAST
            // EnchantingTableBlock:116, EndPortalFrameBlock:128, FarmlandBlock:147
            | BlockId::ENCHANTING_TABLE
            | BlockId::END_PORTAL_FRAME
            | BlockId::FARMLAND
            // GrindstoneBlock:108, HeavyCoreBlock:77, HopperBlock:172, LecternBlock:257
            | BlockId::GRINDSTONE
            | BlockId::HEAVY_CORE
            | BlockId::HOPPER
            | BlockId::LECTERN
            // MudBlock:42, SoulSandBlock:42
            | BlockId::MUD
            | BlockId::SOUL_SAND
            // PistonBaseBlock:395, PistonHeadBlock:143, MovingPistonBlock:142
            | BlockId::PISTON
            | BlockId::STICKY_PISTON
            | BlockId::PISTON_HEAD
            | BlockId::MOVING_PISTON
            // RespawnAnchorBlock:242, StonecutterBlock:93
            | BlockId::RESPAWN_ANCHOR
            | BlockId::STONECUTTER
            // RodBlock:41
            | BlockId::END_ROD
            | BlockId::LIGHTNING_ROD
            | BlockId::EXPOSED_LIGHTNING_ROD
            | BlockId::WEATHERED_LIGHTNING_ROD
            | BlockId::OXIDIZED_LIGHTNING_ROD
            | BlockId::WAXED_LIGHTNING_ROD
            | BlockId::WAXED_EXPOSED_LIGHTNING_ROD
            | BlockId::WAXED_WEATHERED_LIGHTNING_ROD
            | BlockId::WAXED_OXIDIZED_LIGHTNING_ROD
            // SculkSensorBlock:279, SeaPickleBlock:172, SnifferEggBlock:96
            | BlockId::SCULK_SENSOR
            | BlockId::CALIBRATED_SCULK_SENSOR
            | BlockId::SEA_PICKLE
            | BlockId::SNIFFER_EGG
            // SpeleothemBlock:286 (PointedDripstoneBlock, SulfurSpikeBlock)
            | BlockId::POINTED_DRIPSTONE
            | BlockId::SULFUR_SPIKE
    )
}

/// `type == PathComputationType.WATER && state.getFluidState().is(FluidTags.WATER)`:
/// SlabBlock:134, CopperGolemStatueBlock:126, ShelfBlock:87.
fn water_only(block: &Block, id: BlockId) -> bool {
    block.has_tag(&tag::Block::MINECRAFT_SLABS)
        || matches!(
            id,
            BlockId::COPPER_GOLEM_STATUE
                | BlockId::EXPOSED_COPPER_GOLEM_STATUE
                | BlockId::WEATHERED_COPPER_GOLEM_STATUE
                | BlockId::OXIDIZED_COPPER_GOLEM_STATUE
                | BlockId::WAXED_COPPER_GOLEM_STATUE
                | BlockId::WAXED_EXPOSED_COPPER_GOLEM_STATUE
                | BlockId::WAXED_WEATHERED_COPPER_GOLEM_STATUE
                | BlockId::WAXED_OXIDIZED_COPPER_GOLEM_STATUE
                | BlockId::OAK_SHELF
                | BlockId::SPRUCE_SHELF
                | BlockId::BIRCH_SHELF
                | BlockId::JUNGLE_SHELF
                | BlockId::ACACIA_SHELF
                | BlockId::DARK_OAK_SHELF
                | BlockId::PALE_OAK_SHELF
                | BlockId::MANGROVE_SHELF
                | BlockId::CHERRY_SHELF
                | BlockId::BAMBOO_SHELF
                | BlockId::CRIMSON_SHELF
                | BlockId::WARPED_SHELF
        )
}

/// `BlockState.isPathfindable(PathComputationType)`, i.e. whether a mob's path may pass
/// through this block state.
#[must_use]
pub fn is_pathfindable(state: &BlockState, kind: PathComputationType) -> bool {
    let block = Block::from_state_id(state.id);
    let id = block.id;

    if always_blocked(block, id) {
        return false;
    }

    if water_only(block, id) {
        return kind == PathComputationType::Water && state_fluid_is_water(state);
    }

    // LiquidBlock:120 - `!this.fluid.is(FluidTags.LAVA)`, for every computation type.
    if matches!(id, BlockId::WATER | BlockId::LAVA) {
        return id == BlockId::WATER;
    }

    // PowderSnowBlock:163 - unconditionally true.
    if id == BlockId::POWDER_SNOW {
        return true;
    }

    // SnowLayerBlock:42
    if id == BlockId::SNOW {
        return kind == PathComputationType::Land
            && SnowLikeProperties::from_state_id(state.id, block).layers < 5;
    }

    // DoorBlock:131 - LAND/AIR follow OPEN, WATER is always false.
    if block.has_tag(&tag::Block::MINECRAFT_DOORS) {
        return kind != PathComputationType::Water
            && OakDoorLikeProperties::from_state_id(state.id, block).open;
    }

    // TrapDoorBlock:76 - LAND/AIR follow OPEN, WATER follows WATERLOGGED.
    if block.has_tag(&tag::Block::MINECRAFT_TRAPDOORS) {
        let props = OakTrapdoorLikeProperties::from_state_id(state.id, block);
        return if kind == PathComputationType::Water {
            props.waterlogged
        } else {
            props.open
        };
    }

    // FenceGateBlock:117 - LAND/AIR follow OPEN, WATER is always false.
    if block.has_tag(&tag::Block::MINECRAFT_FENCE_GATES) {
        return kind != PathComputationType::Water
            && OakFenceGateLikeProperties::from_state_id(state.id, block).open;
    }

    // `VegetationBlock:55` returns true for AIR on collision-less plants, which is what the
    // default already yields for them, so it needs no branch here.

    match kind {
        PathComputationType::Land | PathComputationType::Air => !state.is_full_cube(),
        PathComputationType::Water => state_fluid_is_water(state),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pumpkin_data::block_properties::HorizontalFacing;

    fn default_state(block: &'static Block) -> &'static BlockState {
        block.default_state
    }

    #[test]
    fn default_land_is_not_a_full_collision_cube() {
        assert!(!is_pathfindable(
            default_state(&Block::STONE),
            PathComputationType::Land
        ));
        assert!(is_pathfindable(
            default_state(&Block::AIR),
            PathComputationType::Land
        ));
        assert!(is_pathfindable(
            default_state(&Block::TORCH),
            PathComputationType::Land
        ));
    }

    #[test]
    fn overrides_block_land_paths_through_non_full_blocks() {
        // Each of these is not a full collision cube, so only the override makes it blocked.
        for block in [
            &Block::CHEST,
            &Block::ANVIL,
            &Block::CAULDRON,
            &Block::OAK_STAIRS,
            &Block::COBBLESTONE_WALL,
            &Block::OAK_FENCE,
            &Block::IRON_BARS,
            &Block::GLASS_PANE,
            &Block::SOUL_SAND,
            &Block::FARMLAND,
            &Block::DIRT_PATH,
            &Block::LECTERN,
            &Block::HOPPER,
            &Block::CACTUS,
            &Block::CAMPFIRE,
            &Block::LANTERN,
            &Block::POINTED_DRIPSTONE,
            &Block::SCULK_SENSOR,
            &Block::WHITE_BED,
            &Block::FLOWER_POT,
        ] {
            let state = default_state(block);
            assert!(
                !state.is_full_cube(),
                "{} is a full cube, test would pass vacuously",
                block.name
            );
            assert!(
                !is_pathfindable(state, PathComputationType::Land),
                "{} should not be pathfindable on land",
                block.name
            );
        }
    }

    #[test]
    fn doors_follow_the_open_property() {
        let closed = OakDoorLikeProperties::from_state_id(
            Block::OAK_DOOR.default_state.id,
            &Block::OAK_DOOR,
        );
        assert!(!closed.open);
        assert!(!is_pathfindable(
            default_state(&Block::OAK_DOOR),
            PathComputationType::Land
        ));

        let mut open = closed;
        open.open = true;
        open.facing = HorizontalFacing::North;
        let open_state = BlockState::from_id(open.to_state_id(&Block::OAK_DOOR));
        assert!(is_pathfindable(open_state, PathComputationType::Land));
        assert!(is_pathfindable(open_state, PathComputationType::Air));
        // WATER is false even for an open door.
        assert!(!is_pathfindable(open_state, PathComputationType::Water));
    }

    #[test]
    fn trapdoors_use_open_for_land_and_waterlogged_for_water() {
        let mut props = OakTrapdoorLikeProperties::from_state_id(
            Block::OAK_TRAPDOOR.default_state.id,
            &Block::OAK_TRAPDOOR,
        );
        props.open = false;
        props.waterlogged = true;
        let state = BlockState::from_id(props.to_state_id(&Block::OAK_TRAPDOOR));
        assert!(!is_pathfindable(state, PathComputationType::Land));
        assert!(is_pathfindable(state, PathComputationType::Water));

        props.open = true;
        props.waterlogged = false;
        let state = BlockState::from_id(props.to_state_id(&Block::OAK_TRAPDOOR));
        assert!(is_pathfindable(state, PathComputationType::Land));
        assert!(!is_pathfindable(state, PathComputationType::Water));
    }

    #[test]
    fn fence_gates_follow_the_open_property() {
        let mut props = OakFenceGateLikeProperties::from_state_id(
            Block::OAK_FENCE_GATE.default_state.id,
            &Block::OAK_FENCE_GATE,
        );
        props.open = true;
        let state = BlockState::from_id(props.to_state_id(&Block::OAK_FENCE_GATE));
        assert!(is_pathfindable(state, PathComputationType::Land));
        assert!(!is_pathfindable(state, PathComputationType::Water));

        props.open = false;
        let state = BlockState::from_id(props.to_state_id(&Block::OAK_FENCE_GATE));
        assert!(!is_pathfindable(state, PathComputationType::Land));
    }

    #[test]
    fn slabs_are_water_only() {
        let state = default_state(&Block::OAK_SLAB);
        assert!(!is_pathfindable(state, PathComputationType::Land));
        assert!(!is_pathfindable(state, PathComputationType::Air));
        assert!(!is_pathfindable(state, PathComputationType::Water));

        let waterlogged = state.with_waterlogged().expect("slabs waterlog");
        assert!(is_pathfindable(waterlogged, PathComputationType::Water));
        assert!(!is_pathfindable(waterlogged, PathComputationType::Land));
    }

    #[test]
    fn shelves_and_copper_golem_statues_are_water_only() {
        for block in [&Block::OAK_SHELF, &Block::COPPER_GOLEM_STATUE] {
            let state = default_state(block);
            assert!(!is_pathfindable(state, PathComputationType::Land));
            let waterlogged = state.with_waterlogged().expect("waterloggable");
            assert!(is_pathfindable(waterlogged, PathComputationType::Water));
        }
    }

    #[test]
    fn snow_layers_below_five_are_walkable() {
        for layers in 1..=8u8 {
            let props = SnowLikeProperties { layers };
            let state = BlockState::from_id(props.to_state_id(&Block::SNOW));
            assert_eq!(
                is_pathfindable(state, PathComputationType::Land),
                layers < 5,
                "snow layers = {layers}"
            );
            assert!(!is_pathfindable(state, PathComputationType::Air));
        }
    }

    #[test]
    fn water_is_pathfindable_and_lava_is_not() {
        for kind in [
            PathComputationType::Land,
            PathComputationType::Water,
            PathComputationType::Air,
        ] {
            assert!(is_pathfindable(default_state(&Block::WATER), kind));
            assert!(!is_pathfindable(default_state(&Block::LAVA), kind));
        }
    }

    #[test]
    fn powder_snow_is_always_pathfindable() {
        for kind in [
            PathComputationType::Land,
            PathComputationType::Water,
            PathComputationType::Air,
        ] {
            assert!(is_pathfindable(default_state(&Block::POWDER_SNOW), kind));
        }
    }

    #[test]
    fn inherently_watery_blocks_are_water_pathfindable() {
        for block in [
            &Block::KELP,
            &Block::KELP_PLANT,
            &Block::SEAGRASS,
            &Block::TALL_SEAGRASS,
            &Block::BUBBLE_COLUMN,
        ] {
            let state = default_state(block);
            assert!(
                !state.is_waterlogged(),
                "{} has no waterlogged property, test would pass vacuously",
                block.name
            );
            assert!(
                is_pathfindable(state, PathComputationType::Water),
                "{} should be water-pathfindable",
                block.name
            );
            assert!(is_pathfindable(state, PathComputationType::Land));
        }
    }

    #[test]
    fn water_default_needs_water() {
        // A non-waterloggable, non-full block: pathfindable on land, not in water.
        let state = default_state(&Block::TORCH);
        assert!(is_pathfindable(state, PathComputationType::Land));
        assert!(!is_pathfindable(state, PathComputationType::Water));
    }
}
