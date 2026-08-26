use pumpkin_data::{
    Block, BlockState,
    block_properties::{BlockProperties, OakFenceLikeProperties},
};
use pumpkin_util::{
    math::position::BlockPos,
    math::{boundingbox::BoundingBox, vector3::Vector3},
    random::{RandomGenerator, RandomImpl, legacy_rand::LegacyRand},
};

use crate::generation::proto_chunk::GenerationCache;
use crate::{generation::section_coords, world::WorldPortalExt};

pub struct EndSpikeFeature {
    pub crystal_invulnerable: bool,
    pub spikes: Vec<Spike>,
}

#[derive(Clone)]
pub struct Spike {
    pub center_x: i32,
    pub center_z: i32,
    pub radius: i32,
    pub height: i32,
    pub guarded: bool,
}

impl Spike {
    /// Returns the spike list selected by vanilla's `getSpikesForLevel`.
    ///
    /// Vanilla first derives a 16-bit cache key from the level seed, then uses
    /// that key to shuffle the ten possible spike sizes.
    #[must_use]
    pub fn for_level_seed(seed: u64) -> Vec<Self> {
        let mut seed_random = RandomGenerator::Legacy(LegacyRand::from_seed(seed));
        let cache_key = seed_random.next_i64() as u64 & 65_535;
        let mut random = RandomGenerator::Legacy(LegacyRand::from_seed(cache_key));
        let mut sizes: Vec<i32> = (0..10).collect();

        for i in (1..10usize).rev() {
            let j = random.next_bounded_i32(i as i32 + 1) as usize;
            sizes.swap(i, j);
        }

        sizes
            .into_iter()
            .enumerate()
            .map(|(i, size)| {
                let angle =
                    2.0 * (-std::f64::consts::PI + (std::f64::consts::PI / 10.0) * i as f64);
                Self {
                    center_x: (42.0 * angle.cos()).floor() as i32,
                    center_z: (42.0 * angle.sin()).floor() as i32,
                    radius: 2 + size / 3,
                    height: 76 + size * 3,
                    guarded: size == 1 || size == 2,
                }
            })
            .collect()
    }

    #[must_use]
    pub const fn is_in_chunk(&self, pos: &BlockPos) -> bool {
        section_coords::block_to_section(pos.0.x) == section_coords::block_to_section(self.center_x)
            && section_coords::block_to_section(pos.0.z)
                == section_coords::block_to_section(self.center_z)
    }

    /// Vanilla `EndSpike.getTopBoundingBox`: all world heights over the
    /// spike's X/Z footprint, used when scanning for spike crystals.
    #[must_use]
    pub const fn top_bounding_box(&self) -> BoundingBox {
        BoundingBox::new(
            Vector3::new(
                (self.center_x - self.radius) as f64,
                -2032.0,
                (self.center_z - self.radius) as f64,
            ),
            Vector3::new(
                (self.center_x + self.radius) as f64,
                2031.0,
                (self.center_z + self.radius) as f64,
            ),
        )
    }
}

impl EndSpikeFeature {
    #[expect(clippy::too_many_arguments)]
    pub fn generate<T: GenerationCache>(
        &self,
        chunk: &mut T,
        _block_registry: &dyn WorldPortalExt,
        _min_y: i8,
        _height: u16,
        _feature: pumpkin_data::placed_feature::PlacedFeature, // This placed feature
        random: &mut RandomGenerator,
        pos: BlockPos,
    ) -> bool {
        let mut spikes = self.spikes.clone();
        if spikes.is_empty() {
            let mut sizes: Vec<i32> = (0..10).collect();
            for i in (1..10usize).rev() {
                let j = random.next_bounded_i32(i as i32 + 1) as usize;
                sizes.swap(i, j);
            }

            for (i, &size) in sizes.iter().enumerate() {
                let angle = 2.0 * (-std::f64::consts::PI + 0.3141592653589793 * i as f64);
                spikes.push(Spike {
                    center_x: (42.0 * angle.cos()).floor() as i32,
                    center_z: (42.0 * angle.sin()).floor() as i32,
                    radius: 2 + size / 3,
                    height: 76 + size * 3,
                    guarded: size == 1 || size == 2,
                });
            }
        }
        for spike in spikes {
            if !spike.is_in_chunk(&pos) {
                continue;
            }
            Self::gen_spike(&spike, chunk);
        }

        true
    }

    fn gen_spike<T: GenerationCache>(spike: &Spike, chunk: &mut T) {
        let radius = spike.radius;
        for pos in BlockPos::iterate(
            BlockPos::new(
                spike.center_x - radius,
                chunk.bottom_y() as i32,
                spike.center_z - radius,
            ),
            BlockPos::new(
                spike.center_x + radius,
                spike.height + 10,
                spike.center_z + radius,
            ),
        ) {
            if pos
                .0
                .squared_distance_to(spike.center_x, pos.0.y, spike.center_z)
                <= (radius * radius + 1)
                && pos.0.y < spike.height
            {
                chunk.set_block_state(&pos.0, Block::OBSIDIAN.default_state);
                continue;
            }
            if pos.0.y <= 65 {
                continue;
            }
            chunk.set_block_state(&pos.0, Block::AIR.default_state);
        }

        // Bedrock cap serves as the crystal base, fire sits on top of it
        chunk.set_block_state(
            &pumpkin_util::math::vector3::Vector3::new(
                spike.center_x,
                spike.height,
                spike.center_z,
            ),
            Block::BEDROCK.default_state,
        );
        chunk.set_block_state(
            &pumpkin_util::math::vector3::Vector3::new(
                spike.center_x,
                spike.height + 1,
                spike.center_z,
            ),
            Block::FIRE.default_state,
        );

        // Iron-bar cage for guarded spikes: 5x5 walls + open top frame at dy=3.
        if spike.guarded {
            for dy in 0i32..=3 {
                for dx in -2i32..=2 {
                    for dz in -2i32..=2 {
                        // Only place on perimeter walls and the top frame
                        let x_wall_present = dx.abs() == 2;
                        let z_wall_present = dz.abs() == 2;
                        let on_top = dy == 3;
                        if !x_wall_present && !z_wall_present && !on_top {
                            continue;
                        }

                        // Connectivity rules
                        let x_edge = x_wall_present || on_top;
                        let z_edge = z_wall_present || on_top;

                        let mut props = OakFenceLikeProperties::default(&Block::IRON_BARS);
                        props.north = x_edge && dz != 2;
                        props.south = x_edge && dz != -2;
                        props.west = z_edge && dx != 2;
                        props.east = z_edge && dx != -2;

                        let bar_state = BlockState::from_id(props.to_state_id(&Block::IRON_BARS));
                        chunk.set_block_state(
                            &pumpkin_util::math::vector3::Vector3::new(
                                spike.center_x + dx,
                                spike.height + dy,
                                spike.center_z + dz,
                            ),
                            bar_state,
                        );
                    }
                }
            }
        }
    }
}
