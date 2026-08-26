use crate::generation::proto_chunk::GenerationCache;
use pumpkin_data::block_properties::{
    BlockProperties, PointedDripstoneLikeProperties, SpeleothemThickness, VerticalDirection,
};
use pumpkin_data::{Block, BlockDirection};
use pumpkin_util::{
    math::position::BlockPos,
    random::{RandomGenerator, RandomImpl},
};

pub struct SmallDripstoneFeature {
    pub taller_dripstone: f32,
    pub directional_spread: f32,
    pub spread_radius2: f32,
    pub spread_radius3: f32,
}

impl SmallDripstoneFeature {
    /// Port of vanilla `SpeleothemFeature.place`
    /// (`SpeleothemFeature.java:17-35`): the single (non-cluster) dripstone
    /// speleothem.
    pub fn generate<T: GenerationCache>(
        &self,
        chunk: &mut T,
        random: &mut RandomGenerator,
        pos: BlockPos,
    ) -> bool {
        if let Some(dir) = Self::get_direction(chunk, pos, random) {
            let root_pos = pos.offset(dir.opposite().to_offset());
            self.gen_dripstone_blocks(chunk, root_pos, random);
            // `SpeleothemFeature.place:28-31`: a taller (2-block) speleothem rolls
            // `chanceOfTallerGeneration` and requires empty/water space ahead of
            // the tip.
            let tip_pos = pos.offset(dir.to_offset());
            let height =
                if random.next_f32() < self.taller_dripstone && is_empty_or_water(chunk, tip_pos) {
                    2
                } else {
                    1
                };
            grow_speleothem(chunk, pos, dir, height, false);
            return true;
        }
        false
    }

    fn get_direction<T: GenerationCache>(
        chunk: &T,
        pos: BlockPos,
        random: &mut RandomGenerator,
    ) -> Option<BlockDirection> {
        let up =
            super::can_replace(GenerationCache::get_block_state(chunk, &pos.up().0).to_block_id());
        let down: bool = super::can_replace(
            GenerationCache::get_block_state(chunk, &pos.down().0).to_block_id(),
        );
        if up && down {
            return if random.next_bool() {
                Some(BlockDirection::Down)
            } else {
                Some(BlockDirection::Up)
            };
        }
        if up {
            return Some(BlockDirection::Down);
        }
        if down {
            return Some(BlockDirection::Up);
        }
        None
    }

    fn gen_dripstone_blocks<T: GenerationCache>(
        &self,
        chunk: &mut T,
        pos: BlockPos,
        random: &mut RandomGenerator,
    ) {
        super::gen_dripstone(chunk, pos);
        for dir in BlockDirection::horizontal_worldgen() {
            if random.next_f32() > self.directional_spread {
                continue;
            }
            let pos = pos.offset(dir.to_offset());
            super::gen_dripstone(chunk, pos);
            if random.next_f32() > self.spread_radius2 {
                continue;
            }
            let pos = pos.offset(BlockDirection::random(random).to_offset());
            super::gen_dripstone(chunk, pos);
            if random.next_f32() > self.spread_radius3 {
                continue;
            }
            let pos = pos.offset(BlockDirection::random(random).to_offset());
            super::gen_dripstone(chunk, pos);
        }
    }
}

/// `SpeleothemUtils.isEmptyOrWater(LevelAccessor, BlockPos)` (`SpeleothemUtils.java:44-46`,
/// predicate at `:122-124`).
fn is_empty_or_water<T: GenerationCache>(chunk: &T, pos: BlockPos) -> bool {
    let id = GenerationCache::get_block_state(chunk, &pos.0).to_block_id();
    id == Block::AIR.id || id == Block::WATER.id
}

/// `SpeleothemUtils.growSpeleothem` (`SpeleothemUtils.java:75-93`): grows a single pointed
/// speleothem column from `start_pos` toward `tip_direction`, provided the block behind the
/// start position can support it (`isBase`, ported here as [`super::can_replace`] since both
/// check "is the configured base block or a replaceable-tagged block").
fn grow_speleothem<T: GenerationCache>(
    chunk: &mut T,
    start_pos: BlockPos,
    tip_direction: BlockDirection,
    height: i32,
    merged_tip: bool,
) {
    let anchor_pos = start_pos.offset(tip_direction.opposite().to_offset());
    let anchor_id = GenerationCache::get_block_state(chunk, &anchor_pos.0).to_block_id();
    if !super::can_replace(anchor_id) {
        return;
    }

    let vertical_direction = if tip_direction == BlockDirection::Up {
        VerticalDirection::Up
    } else {
        VerticalDirection::Down
    };

    let mut pos = start_pos;
    build_base_to_tip_column(height, merged_tip, |thickness| {
        let waterlogged =
            GenerationCache::get_block_state(chunk, &pos.0).to_block_id() == Block::WATER.id;
        let mut props = PointedDripstoneLikeProperties::default(&Block::POINTED_DRIPSTONE);
        props.thickness = thickness;
        props.vertical_direction = vertical_direction;
        props.waterlogged = waterlogged;
        chunk.set_block_state(
            &pos.0,
            props.to_state_id(&Block::POINTED_DRIPSTONE).to_state(),
        );
        pos = pos.offset(tip_direction.to_offset());
    });
}

/// `SpeleothemUtils.buildBaseToTipColumn` (`SpeleothemUtils.java:59-73`): the thickness
/// sequence for a column of the given total length, base-to-tip.
fn build_base_to_tip_column(
    total_length: i32,
    merged_tip: bool,
    mut consumer: impl FnMut(SpeleothemThickness),
) {
    if total_length >= 3 {
        consumer(SpeleothemThickness::Base);
        for _ in 0..total_length - 3 {
            consumer(SpeleothemThickness::Middle);
        }
    }
    if total_length >= 2 {
        consumer(SpeleothemThickness::Frustum);
    }
    if total_length >= 1 {
        consumer(if merged_tip {
            SpeleothemThickness::TipMerge
        } else {
            SpeleothemThickness::Tip
        });
    }
}
