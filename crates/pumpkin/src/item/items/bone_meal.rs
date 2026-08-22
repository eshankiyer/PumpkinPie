use std::any::Any;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use crate::block::blocks::coral::coral_fan::CoralWallFanLikeProperties;
use crate::entity::player::Player;
use crate::item::items::bucket::set_waterlogged;
use crate::item::{ItemBehaviour, ItemMetadata};
use crate::server::Server;
use crate::world::World;
use pumpkin_data::HorizontalFacingExt;
use pumpkin_data::block_properties::BlockProperties;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::tag::Taggable;
use pumpkin_data::world::WorldEvent;
use pumpkin_data::{Block, BlockDirection, BlockId, BlockState, BlockStateId, tag};
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_util::random::{RandomGenerator, RandomImpl, get_seed, xoroshiro128::Xoroshiro};
use pumpkin_world::world::BlockFlags;

pub struct BoneMealItem;

impl ItemMetadata for BoneMealItem {
    fn ids() -> Box<[u16]> {
        Box::new([Item::BONE_MEAL.id])
    }
}

impl ItemBehaviour for BoneMealItem {
    #[allow(clippy::too_many_lines)]
    fn use_on_block<'a>(
        &'a self,
        item: &'a mut ItemStack,
        player: &'a Player,
        location: BlockPos,
        face: BlockDirection,
        _cursor_pos: Vector3<f32>,
        block: &'a Block,
        server: &'a Server,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            let world = player.world();
            let state_id = world.get_block_state_id(&location);
            if server
                .block_registry
                .bone_meal(block, &world, &location, state_id)
                .await
            {
                world.sync_world_event(WorldEvent::ParticlesAndSoundPlantGrowth, location, 15);
                item.decrement_unless_creative(player.gamemode.load(), 1);
                return;
            }

            // Saplings still have no registered bone-meal behaviour; vanilla
            // SaplingBlock.performBonemeal advances stage 0 -> 1 before growing the tree.
            let sapling_action = block.properties(state_id).and_then(|props| {
                let prop_map = props.to_props();
                prop_map
                    .iter()
                    .find(|(k, _)| *k == "stage")
                    .and_then(|(_, stage_val)| stage_val.parse::<u8>().ok())
                    .filter(|&stage| stage < 1)
                    .map(|_| {
                        let new_props: Vec<(&str, &str)> = prop_map
                            .iter()
                            .map(|(k, v)| if *k == "stage" { (*k, "1") } else { (*k, *v) })
                            .collect();
                        block.from_properties(&new_props).to_state_id(block)
                    })
            });

            if let Some(new_state_id) = sapling_action {
                world
                    .set_block_state(&location, new_state_id, BlockFlags::NOTIFY_ALL)
                    .await;
                world.sync_world_event(WorldEvent::ParticlesAndSoundPlantGrowth, location, 15);
                item.decrement_unless_creative(player.gamemode.load(), 1);
                return;
            }

            // BoneMealItem.java:49-57: when growCrop fails, bone meal applied to a sturdy face
            // seeds sea plants in the water block against that face.
            if !world.get_block_state(&location).is_side_solid(face) {
                return;
            }
            let relative = BlockPos(location.0 + face.to_offset());
            if grow_water_plant(&world, server, relative, face).await {
                world.sync_world_event(WorldEvent::ParticlesAndSoundPlantGrowth, relative, 15);
                item.decrement_unless_creative(player.gamemode.load(), 1);
            }
        })
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

fn is_water_source(world: &Arc<World>, pos: &BlockPos) -> bool {
    let state_id = world.get_block_state_id(pos);
    Block::from_state_id(state_id).id == Block::WATER.id
        && state_id == Block::WATER.default_state.id
}

fn wall_fan_with_facing(block: &'static Block, facing: BlockDirection) -> BlockStateId {
    let mut props = CoralWallFanLikeProperties::default(block);
    if let Some(horizontal) = facing.to_horizontal_facing() {
        props.facing = horizontal;
    }
    props.waterlogged = true;
    props.to_state_id(block)
}

fn random_tag_block(ids: &[u16], random: &mut RandomGenerator) -> Option<&'static Block> {
    if ids.is_empty() {
        return None;
    }
    let index = random.next_bounded_i32(i32::try_from(ids.len()).ok()?) as usize;
    Some(Block::from_id(BlockId::new_or_air(ids[index])))
}

fn can_survive(
    world: &Arc<World>,
    server: &Server,
    block: &'static Block,
    state_id: BlockStateId,
    pos: &BlockPos,
) -> bool {
    server.block_registry.can_place_at(
        Some(server),
        Some(world),
        world.as_ref(),
        None,
        block,
        BlockState::from_id(state_id),
        pos,
        None,
        None,
    )
}

/// Vanilla `BoneMealItem.growWaterPlant` (BoneMealItem.java:81-142): 128 attempts spreading out
/// from the clicked water block, placing seagrass, or coral in biomes tagged
/// `produces_corals_from_bonemeal`, and bone-mealing existing seagrass one time in ten.
async fn grow_water_plant(
    world: &Arc<World>,
    server: &Server,
    pos: BlockPos,
    clicked_face: BlockDirection,
) -> bool {
    if !is_water_source(world, &pos) {
        return false;
    }

    let mut random = RandomGenerator::Xoroshiro(Xoroshiro::from_seed(get_seed()));

    'attempt: for j in 0..128 {
        let mut test_pos = pos;
        let mut grow_block: &'static Block = &Block::SEAGRASS;
        let mut grow_state = Block::SEAGRASS.default_state.id;

        for _ in 0..(j / 16) {
            test_pos = BlockPos(test_pos.0.add_raw(
                random.next_bounded_i32(3) - 1,
                (random.next_bounded_i32(3) - 1) * random.next_bounded_i32(3) / 2,
                random.next_bounded_i32(3) - 1,
            ));
            if world.get_block_state(&test_pos).is_full_cube() {
                continue 'attempt;
            }
        }

        let produces_corals = world.get_biome(&test_pos).is_some_and(|biome| {
            biome.has_tag(&tag::WorldgenBiome::MINECRAFT_PRODUCES_CORALS_FROM_BONEMEAL)
        });
        if produces_corals {
            if j == 0 && clicked_face.is_horizontal() {
                if let Some(coral) =
                    random_tag_block(tag::Block::MINECRAFT_WALL_CORALS.1, &mut random)
                {
                    grow_block = coral;
                    grow_state = wall_fan_with_facing(coral, clicked_face);
                }
            } else if random.next_bounded_i32(4) == 0
                && let Some(plant) =
                    random_tag_block(tag::Block::MINECRAFT_UNDERWATER_BONEMEALS.1, &mut random)
            {
                grow_block = plant;
                // Corals and coral fans are placed into a water source, so they must be
                // waterlogged or the water is destroyed (vanilla's default states carry
                // WATERLOGGED = true).
                grow_state = set_waterlogged(plant, plant.default_state.id, true);
            }
        }

        // BoneMealItem.java:118-122: a wall fan is turned until it finds a wall to attach to,
        // giving up after four tries.
        if grow_block.has_tag(&tag::Block::MINECRAFT_WALL_CORALS) {
            let mut tries = 0;
            while tries < 4 && !can_survive(world, server, grow_block, grow_state, &test_pos) {
                let facing =
                    BlockDirection::horizontal_worldgen()[random.next_bounded_i32(4) as usize];
                grow_state = wall_fan_with_facing(grow_block, facing.to_block_direction());
                tries += 1;
            }
        }

        if !can_survive(world, server, grow_block, grow_state, &test_pos) {
            continue;
        }

        if is_water_source(world, &test_pos) {
            world
                .set_block_state(&test_pos, grow_state, BlockFlags::NOTIFY_ALL)
                .await;
        } else {
            let test_state_id = world.get_block_state_id(&test_pos);
            if Block::from_state_id(test_state_id).id == Block::SEAGRASS.id
                && random.next_bounded_i32(10) == 0
            {
                server
                    .block_registry
                    .bone_meal(&Block::SEAGRASS, world, &test_pos, test_state_id)
                    .await;
            }
        }
    }

    true
}
