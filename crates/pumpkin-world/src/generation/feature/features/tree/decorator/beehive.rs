use pumpkin_data::{
    Block, BlockState,
    block_properties::{BeeNestLikeProperties, BlockProperties, HorizontalFacing},
};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_nbt::tag::NbtTag;
use pumpkin_util::{
    math::position::BlockPos,
    random::{RandomGenerator, RandomImpl},
};

use crate::generation::feature::features::coral::shuffle;
use crate::generation::feature::java_set::vanilla_hash_set_order;
use crate::generation::proto_chunk::GenerationCache;

pub struct BeehiveTreeDecorator {
    pub probability: f32,
}

/// `BeehiveDecorator.WORLDGEN_FACING` (`BeehiveDecorator.java:20`).
const WORLDGEN_FACING: HorizontalFacing = HorizontalFacing::South;

/// `BeehiveDecorator.SPAWN_DIRECTIONS` (`BeehiveDecorator.java:21-24`): the horizontal plane in
/// `Direction.Plane.HORIZONTAL` order (`Direction.java:577`) minus the opposite of
/// `WORLDGEN_FACING`. The order is load bearing - `Util.shuffle` consumes the random stream over
/// this list.
const SPAWN_DIRECTIONS: [HorizontalFacing; 3] = [
    HorizontalFacing::East,
    HorizontalFacing::South,
    HorizontalFacing::West,
];

/// `BeehiveBlockEntity.Occupant.create` (`BeehiveBlockEntity.java:397-399`).
const MIN_TICKS_IN_HIVE: i32 = 600;

impl BeehiveTreeDecorator {
    /// `BeehiveDecorator.place` (`BeehiveDecorator.java:37-67`).
    pub fn generate<T: GenerationCache>(
        &self,
        chunk: &mut T,
        random: &mut RandomGenerator,
        log_positions: &[BlockPos],
        foliage_positions: &[BlockPos],
    ) {
        // `TreeDecorator.Context` sorts the trunk and foliage sets by Y
        // (`TreeDecorator.java:45-50`); the sort is stable, so ties keep the `HashSet` order.
        let mut logs = vanilla_hash_set_order(log_positions);
        logs.sort_by_key(|pos| pos.0.y);
        if logs.is_empty() {
            return;
        }
        if random.next_f32() >= self.probability {
            return;
        }

        let mut leaves = vanilla_hash_set_order(foliage_positions);
        leaves.sort_by_key(|pos| pos.0.y);

        let first_log_y = logs[0].0.y;
        let hive_y = if leaves.is_empty() {
            (first_log_y + 1 + random.next_bounded_i32(3)).min(logs[logs.len() - 1].0.y)
        } else {
            (leaves[0].0.y - 1).max(first_log_y + 1)
        };

        let mut placements: Vec<BlockPos> = logs
            .iter()
            .filter(|pos| pos.0.y == hive_y)
            .flat_map(|pos| {
                SPAWN_DIRECTIONS
                    .iter()
                    .map(move |dir| pos.offset(dir.to_offset()))
            })
            .collect();
        if placements.is_empty() {
            return;
        }
        shuffle(&mut placements, random);

        let Some(hive_pos) = placements.into_iter().find(|pos| {
            chunk.is_air(&pos.0) && chunk.is_air(&pos.offset(WORLDGEN_FACING.to_offset()).0)
        }) else {
            return;
        };

        let mut props = BeeNestLikeProperties::default(&Block::BEE_NEST);
        props.facing = WORLDGEN_FACING;
        chunk.set_block_state(
            &hive_pos.0,
            BlockState::from_id(props.to_state_id(&Block::BEE_NEST)),
        );

        let bee_count = 2 + random.next_bounded_i32(2);
        let bees: Vec<NbtTag> = (0..bee_count)
            .map(|_| {
                let mut entity_data = NbtCompound::new();
                entity_data.put_string("id", "minecraft:bee".to_string());
                let mut occupant = NbtCompound::new();
                occupant.put_compound("entity_data", entity_data);
                occupant.put_int("ticks_in_hive", random.next_bounded_i32(599));
                occupant.put_int("min_ticks_in_hive", MIN_TICKS_IN_HIVE);
                NbtTag::Compound(occupant)
            })
            .collect();

        let mut nbt = NbtCompound::new();
        nbt.put_string("id", "minecraft:beehive".to_string());
        nbt.put_int("x", hive_pos.0.x);
        nbt.put_int("y", hive_pos.0.y);
        nbt.put_int("z", hive_pos.0.z);
        nbt.put_list("bees", bees);
        chunk.add_block_entity(&hive_pos.0, nbt);
    }
}
