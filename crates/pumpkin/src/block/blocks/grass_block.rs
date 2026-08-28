use pumpkin_data::BlockStateId;
use pumpkin_data::fluid::Fluid;
use pumpkin_data::{
    Block, BlockDirection, BlockState,
    block_properties::{
        BlockProperties, DoubleBlockHalf, GrassBlockLikeProperties, SnowLikeProperties,
        TallSeagrassLikeProperties,
    },
    tag::{self, Taggable},
};
use pumpkin_macros::pumpkin_block;
use pumpkin_util::math::{position::BlockPos, vector3::Vector3};
use pumpkin_util::random::{RandomGenerator, RandomImpl, xoroshiro128::Xoroshiro};
use pumpkin_world::generation::feature::{
    configured_features::{BONE_MEAL_FEATURES, CONFIGURED_FEATURES, ConfiguredFeature},
    placed_features::{Feature, PLACED_FEATURES},
};
use pumpkin_world::lighting::light_dampening_into;
use pumpkin_world::tick::TickPriority;
use pumpkin_world::world::BlockFlags;
use rand::RngExt;

use crate::block::{BlockBehaviour, BlockFuture, GetStateForNeighborUpdateArgs, RandomTickArgs};
use crate::world::World;

/// `LevelReader#getMaxLightLevel` in vanilla. A covering block that dims light by at least this
/// much makes the grass below it die.
const MAX_LIGHT_LEVEL: u8 = 15;

/// The `FluidState#getAmount` of a full fluid column (a source, a falling fluid or the water held
/// by a waterlogged block). Vanilla's `canBeGrass` rejects exactly this value.
const FULL_FLUID_AMOUNT: i16 = 8;

/// Minimum `getMaxLocalRawBrightness` above a grass block for it to spread.
const MIN_SPREAD_BRIGHTNESS: u8 = 9;

/// How many spread attempts vanilla makes per random tick.
const SPREAD_ATTEMPTS: u8 = 4;

#[pumpkin_block("minecraft:grass_block")]
pub struct GrassBlock;

impl BlockBehaviour for GrassBlock {
    /// `SpreadingSnowyDirtBlock#randomTick`: grass that is covered turns back into dirt, otherwise
    /// it makes four attempts to spread onto nearby dirt.
    fn random_tick<'a>(&'a self, args: RandomTickArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async move {
            let world = args.world;
            let position = args.position;

            if !can_be_grass(world, position) {
                world
                    .set_block_state(
                        position,
                        Block::DIRT.default_state.id,
                        BlockFlags::NOTIFY_ALL,
                    )
                    .await;
                return;
            }

            if world.get_max_local_raw_brightness(&position.up()) < MIN_SPREAD_BRIGHTNESS {
                return;
            }

            for _ in 0..SPREAD_ATTEMPTS {
                // Vanilla: `pos.offset(random.nextInt(3) - 1, random.nextInt(5) - 3, random.nextInt(3) - 1)`
                let target = position.offset(Vector3::new(
                    rand::random_range(-1..=1),
                    rand::random_range(-3..=1),
                    rand::random_range(-1..=1),
                ));

                if world.get_block(&target) != &Block::DIRT || !can_propagate(world, &target) {
                    continue;
                }

                let mut props = GrassBlockLikeProperties::default(&Block::GRASS_BLOCK);
                props.snowy = is_snowy_setting(world, &target);
                world
                    .set_block_state(
                        &target,
                        props.to_state_id(&Block::GRASS_BLOCK),
                        BlockFlags::NOTIFY_ALL,
                    )
                    .await;
            }
        })
    }

    fn is_valid_bonemeal_target(&self, args: crate::block::BonemealArgs<'_>) -> bool {
        let above = args.position.up();
        args.world.is_in_height_limit(above.0.y)
            && args.world.is_loaded(&above)
            && args.world.get_block_state(&above).is_air()
    }

    fn perform_bonemeal<'a>(&'a self, args: crate::block::BonemealArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async move {
            const SPREAD_ATTEMPTS: i32 = 128;
            const ATTEMPTS_PER_STEP: i32 = 16;
            const FLOWER_CHANCE: i32 = 8;

            let origin = args.position.up();
            for attempt in 0..SPREAD_ATTEMPTS {
                let mut target = origin;
                let mut valid = true;
                for _ in 0..attempt / ATTEMPTS_PER_STEP {
                    let offset_x = rand::rng().random_range(0..3) - 1;
                    let offset_y =
                        ((rand::rng().random_range(0..3) - 1) * rand::rng().random_range(0..3)) / 2;
                    let offset_z = rand::rng().random_range(0..3) - 1;
                    target = BlockPos::new(
                        target.0.x + offset_x,
                        target.0.y + offset_y,
                        target.0.z + offset_z,
                    );

                    if !args.world.is_loaded(&target)
                        || args.world.get_block(&target.down()) != args.block
                        || args.world.get_block_state(&target).is_full_cube()
                    {
                        valid = false;
                        break;
                    }
                }

                if !valid {
                    continue;
                }
                let target_state = args.world.get_block_state(&target);
                if Block::from_state_id(target_state.id) == &Block::SHORT_GRASS
                    && rand::rng().random_range(0..10) == 0
                {
                    let above = target.up();
                    if args.world.is_in_height_limit(above.0.y)
                        && args.world.is_loaded(&above)
                        && args.world.get_block_state(&above).is_air()
                    {
                        place_tall_grass(args.world, target).await;
                    }
                } else if target_state.is_air() && args.world.is_in_height_limit(target.0.y) {
                    let selected = if rand::rng().random_range(0..FLOWER_CHANCE) == 0 {
                        biome_bonemeal_state(args.world, target)
                    } else {
                        Some((Block::SHORT_GRASS.default_state, false))
                    };
                    let Some((state, schedule_tick)) = selected else {
                        continue;
                    };
                    let placed_block = Block::from_state_id(state.id);
                    if !args.world.block_registry.can_place_at(
                        None,
                        Some(args.world),
                        args.world.as_ref(),
                        None,
                        placed_block,
                        state,
                        &target,
                        None,
                        None,
                    ) {
                        continue;
                    }
                    if placed_block == &Block::TALL_GRASS
                        && (!args.world.is_loaded(&target.up())
                            || !args.world.get_block_state(&target.up()).is_air())
                    {
                        continue;
                    }
                    args.world
                        .set_block_state(&target, state.id, BlockFlags::NOTIFY_LISTENERS)
                        .await;
                    if schedule_tick {
                        args.world.schedule_block_tick(
                            placed_block,
                            target,
                            1,
                            TickPriority::Normal,
                        );
                    }
                    if placed_block == &Block::TALL_GRASS {
                        place_tall_grass_upper(args.world, target, state.id).await;
                    }
                }
            }
        })
    }

    fn get_state_for_neighbor_update<'a>(
        &'a self,
        args: GetStateForNeighborUpdateArgs<'a>,
    ) -> BlockFuture<'a, BlockStateId> {
        Box::pin(async move {
            let mut props =
                GrassBlockLikeProperties::from_state_id(args.state_id, &Block::GRASS_BLOCK);
            let should_be_snowy = is_snowy_setting(args.world, args.position);
            if props.snowy == should_be_snowy {
                return args.state_id;
            }
            props.snowy = should_be_snowy;

            props.to_state_id(&Block::GRASS_BLOCK)
        })
    }
}

/// `SnowyDirtBlock#isSnowySetting` applied to the block covering `position`.
fn is_snowy_setting(world: &World, position: &BlockPos) -> bool {
    world
        .get_block(&position.up())
        .has_tag(&tag::Block::MINECRAFT_SNOW)
}

/// `SpreadingSnowyDirtBlock#canBeGrass`: grass survives as long as the block covering it neither
/// holds a full fluid column nor blocks all light.
fn can_be_grass(world: &World, position: &BlockPos) -> bool {
    let above = position.up();

    if is_covered_by_full_fluid(world, &above) {
        return false;
    }

    let (above_block, above_state) = world.get_block_and_state(&above);

    // Handle the snow layers explicitly, since that is the one case the raw
    // opacity gets wrong that vanilla calls out by name.
    if above_block == &Block::SNOW {
        return SnowLikeProperties::from_state_id(above_state.id, above_block).layers <= 1;
    }

    light_dampening_into(
        Block::GRASS_BLOCK.default_state,
        above_state,
        BlockDirection::Up,
        above_state.opacity,
    ) < MAX_LIGHT_LEVEL
}

/// `SpreadingSnowyDirtBlock#canPropagate`: grass cannot spread into a spot that has water on top.
fn can_propagate(world: &World, position: &BlockPos) -> bool {
    can_be_grass(world, position)
        && !world
            .get_fluid(&position.up())
            .has_tag(&tag::Fluid::MINECRAFT_WATER)
}

/// Whether the block at `position` reports a fluid state with an amount of 8, i.e. a fluid source,
/// a falling fluid or a waterlogged block.
fn is_covered_by_full_fluid(world: &World, position: &BlockPos) -> bool {
    let state_id = world.get_block_state_id(position);

    Fluid::from_state_id(state_id).map_or_else(
        // Not a fluid block itself; a waterlogged block still carries a water source.
        || {
            world
                .get_fluid(position)
                .has_tag(&tag::Fluid::MINECRAFT_WATER)
        },
        |fluid| {
            fluid
                .states
                .iter()
                .any(|state| state.block_state_id == state_id && state.level == FULL_FLUID_AMOUNT)
        },
    )
}

async fn place_tall_grass(world: &std::sync::Arc<crate::world::World>, position: BlockPos) {
    let state = Block::TALL_GRASS.default_state.id;
    world
        .set_block_state(&position, state, BlockFlags::NOTIFY_LISTENERS)
        .await;
    place_tall_grass_upper(world, position, state).await;
}

async fn place_tall_grass_upper(
    world: &std::sync::Arc<crate::world::World>,
    position: BlockPos,
    lower_state: BlockStateId,
) {
    let mut props = TallSeagrassLikeProperties::from_state_id(lower_state, &Block::TALL_GRASS);
    props.half = DoubleBlockHalf::Upper;
    world
        .set_block_state(
            &position.up(),
            props.to_state_id(&Block::TALL_GRASS),
            BlockFlags::NOTIFY_LISTENERS,
        )
        .await;
}

fn biome_bonemeal_state(
    world: &crate::world::World,
    position: BlockPos,
) -> Option<(&'static BlockState, bool)> {
    let features: Vec<_> = world
        .get_biome(&position)?
        .features
        .iter()
        .flat_map(|step| step.iter())
        .filter_map(|key| PLACED_FEATURES.get(key))
        .filter_map(|feature| match &feature.feature {
            Feature::Named(key) if BONE_MEAL_FEATURES.contains(key) => Some(*key),
            Feature::Named(_) | Feature::Inlined(_) => None,
        })
        .collect();
    if features.is_empty() {
        return None;
    }

    let mut random = RandomGenerator::Xoroshiro(Xoroshiro::from_seed(rand::rng().random()));
    let key = features[random.next_bounded_i32(features.len() as i32) as usize];
    let ConfiguredFeature::SimpleBlock(feature) = CONFIGURED_FEATURES.get(&key)? else {
        return None;
    };
    feature
        .to_place
        .get_for_bonemeal(&mut random, position)
        .map(|state| (state, feature.schedule_tick.unwrap_or(false)))
}
