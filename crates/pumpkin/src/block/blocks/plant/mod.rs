use pumpkin_data::{Block, BlockStateId, tag, tag::Taggable};
use pumpkin_util::math::position::BlockPos;
use pumpkin_world::world::BlockAccessor;

pub mod bamboo;
pub mod bamboo_sapling;
pub mod big_dripleaf;
pub mod big_dripleaf_stem;
pub mod bush;
pub mod cactus;
pub mod cactus_flower;
pub mod cave_vines;
pub mod chorus_flower;
pub mod chorus_plant;
pub mod crop;
pub mod dry_vegetation;
pub mod flower;
pub mod flowerbed;
pub mod fungus;
pub mod kelp;
pub mod leaf_litter;
pub mod lily_pad;
pub mod mangrove_propagule;
pub mod mushroom_plant;
pub mod nether_sprouts;
pub mod roots;
pub mod sapling;
pub mod sea_pickles;
pub mod seagrass;
pub mod segmented;
pub mod short_plant;
pub mod small_dripleaf;
pub mod spore_blossom;
pub mod sugar_cane;
pub mod tall_plant;
pub mod tall_seagrass;
pub mod twisting_vines;
pub mod weeping_vines;
pub mod wither_rose;

trait PlantBlockBase {
    fn can_plant_on_top(&self, block_accessor: &dyn BlockAccessor, pos: &BlockPos) -> bool {
        let block = block_accessor.get_block(pos);
        block.has_tag(&tag::Block::MINECRAFT_SUPPORTS_VEGETATION)
    }

    async fn get_state_for_neighbor_update(
        &self,
        block_accessor: &dyn BlockAccessor,
        block_pos: &BlockPos,
        block_state: BlockStateId,
    ) -> BlockStateId {
        if !self.can_place_at(block_accessor, block_pos) {
            return Block::AIR.default_state.id;
        }
        block_state
    }

    fn can_place_at(&self, block_accessor: &dyn BlockAccessor, block_pos: &BlockPos) -> bool {
        self.can_plant_on_top(block_accessor, &block_pos.down())
    }
}

/// `GrowingPlantHeadBlock.MAX_AGE` (`GrowingPlantHeadBlock.java:21`).
pub(super) const MAX_PLANT_HEAD_AGE: u8 = 25;

/// `GrowingPlantHeadBlock.randomTick` (`GrowingPlantHeadBlock.java:50-57`):
/// `state.getValue(AGE) < 25 && random.nextDouble() < this.growPerTickProbability`.
pub(super) fn should_grow_plant_head(age: u8, roll: f64, grow_per_tick_probability: f64) -> bool {
    age < MAX_PLANT_HEAD_AGE && roll < grow_per_tick_probability
}

/// Shared `GrowingPlantHeadBlock` growth step (`GrowingPlantHeadBlock.java:50-57`).
///
/// The head advances one block along `growth_direction`, taking `state.cycle(AGE)` as its
/// new age, and the block it left behind becomes the body block. Vanilla gets that second
/// write for free from `updateShape` (`GrowingPlantHeadBlock.java:97-105`); pumpkin has no
/// equivalent chain for these blocks, so it is applied explicitly, exactly as
/// `plant/cave_vines.rs` does.
pub(super) async fn grow_plant_head(
    world: &std::sync::Arc<crate::world::World>,
    pos: &BlockPos,
    head: &'static Block,
    body: &'static Block,
    growth_direction: pumpkin_data::BlockDirection,
    grow_per_tick_probability: f64,
    can_grow_into: fn(&'static Block) -> bool,
) {
    use pumpkin_data::block_properties::{BlockProperties, KelpLikeProperties};
    use pumpkin_world::world::BlockFlags;
    use rand::RngExt;

    let (block, state_id) = world.get_block_and_state_id(pos);
    if block != head {
        return;
    }
    let age = KelpLikeProperties::from_state_id(state_id, block).age;
    if !should_grow_plant_head(age, rand::rng().random::<f64>(), grow_per_tick_probability) {
        return;
    }

    let grow_pos = pos.offset(growth_direction.to_offset());
    if !world.is_in_height_limit(grow_pos.0.y) || !can_grow_into(world.get_block(&grow_pos)) {
        return;
    }

    let grown = KelpLikeProperties { age: age + 1 };
    world
        .set_block_state(
            &grow_pos,
            grown.to_state_id(head),
            BlockFlags::NOTIFY_NEIGHBORS,
        )
        .await;
    world
        .set_block_state(pos, body.default_state.id, BlockFlags::NOTIFY_NEIGHBORS)
        .await;
}

#[cfg(test)]
mod tests {
    use super::{MAX_PLANT_HEAD_AGE, should_grow_plant_head};

    /// `KelpBlock.java:23` / `NetherVines.java:8`.
    const KELP_PROBABILITY: f64 = 0.14;
    const NETHER_VINE_PROBABILITY: f64 = 0.1;

    #[test]
    fn max_age_matches_vanilla() {
        assert_eq!(MAX_PLANT_HEAD_AGE, 25);
    }

    #[test]
    fn a_maxed_head_never_grows() {
        assert!(!should_grow_plant_head(25, 0.0, KELP_PROBABILITY));
        assert!(!should_grow_plant_head(25, 0.0, NETHER_VINE_PROBABILITY));
    }

    #[test]
    fn growth_gate_is_a_strict_less_than_on_the_probability() {
        assert!(should_grow_plant_head(24, 0.0, KELP_PROBABILITY));
        assert!(should_grow_plant_head(0, 0.139, KELP_PROBABILITY));
        assert!(!should_grow_plant_head(
            0,
            KELP_PROBABILITY,
            KELP_PROBABILITY
        ));
        // Kelp is the faster of the two: a roll that grows kelp can still fail a vine.
        assert!(!should_grow_plant_head(0, 0.12, NETHER_VINE_PROBABILITY));
        assert!(should_grow_plant_head(0, 0.12, KELP_PROBABILITY));
    }
}
