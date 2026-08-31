use pumpkin_util::math::boundingbox::BoundingBox;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::{Axis as MathAxis, Vector3};

use crate::block_properties::{COLLISION_SHAPES, NoteblockInstrument};
use crate::{Block, BlockDirection, BlockId};

/// Represents a specific state of a block, including its properties and physical behaviors.
///
/// A single `Block` (like a Hopper) can have multiple `BlockState`s (e.g., pointing North,
/// South, or being powered). This struct is optimized for high-speed lookups during
/// physics and lighting calculations.
#[derive(Debug)]
pub struct BlockState {
    /// The global palette ID used for network serialization and chunk storage.
    pub id: BlockStateId,
    /// Bit-flags representing boolean or enum properties (e.g., `waterlogged`, `lit`, `facing`).
    pub state_flags: u16,
    /// Cached flags for each of the 6 sides to speed up ambient occlusion and face culling.
    pub side_flags: u8,
    /// The note block instrument produced when this block is placed underneath one.
    pub instrument: NoteblockInstrument,
    /// The light level emitted by this block, ranging from 0 to 15.
    pub luminance: u8,
    /// Defines how the block reacts to being pushed or pulled by a piston.
    pub piston_behavior: PistonBehavior,
    /// Overrides the base block hardness for this specific state if necessary.
    pub hardness: f32,
    /// Indices into a global voxel-shape registry for physical entity collisions.
    pub collision_shapes: &'static [u16],
    /// Indices into a global voxel-shape registry for the selection highlight box.
    pub outline_shapes: &'static [u16],
    /// How much light is subtracted as it passes through this block (0 for transparent, 15 for opaque).
    pub opacity: u8,
    /// Whether vanilla uses this state's voxel shape when checking light occlusion.
    pub use_shape_for_light_occlusion: bool,
    /// The ID of the block entity associated with this state.
    /// Set to `u16::MAX` if the block does not hold NBT data.
    pub block_entity_type: u16,
    /// Vanilla's cached `BlockState.isSolidRender()` result.
    pub solid_render: bool,
}

/// Helper struct to ensure the validity of `BlockStateIds` parsed from external sources.
/// Every [`BlockStateId`] is guaranteed to correspond to a valid [`BlockState`].
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[repr(transparent)]
pub struct BlockStateId(u16);

#[derive(Clone, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum PistonBehavior {
    Normal,
    Destroy,
    Block,
    Ignore,
    PushOnly,
}

impl PartialEq<BlockStateId> for BlockState {
    fn eq(&self, other: &BlockStateId) -> bool {
        self.id == *other
    }
}

impl PartialEq<BlockState> for BlockStateId {
    fn eq(&self, other: &BlockState) -> bool {
        *self == other.id
    }
}

impl PartialEq for BlockState {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

impl Eq for BlockState {}

impl BlockState {
    #[must_use]
    pub const fn is_air(&self) -> bool {
        self.state_flags & IS_AIR != 0
    }

    #[must_use]
    pub const fn burnable(&self) -> bool {
        self.state_flags & BURNABLE != 0
    }

    #[must_use]
    pub const fn tool_required(&self) -> bool {
        self.state_flags & TOOL_REQUIRED != 0
    }

    /// Whether brushing this state should emit block dust particles.
    /// `BlockBehaviour.Properties.noTerrainParticles` clears this value for the
    /// barrier and structure-void registrations (`BlockBehaviour.java:1265-1267`,
    /// `Blocks.java:2936,3776`), and `BrushItem.onUseTick` checks it before spawning
    /// dust (`BrushItem.java:65-70`).
    #[must_use]
    pub const fn should_spawn_terrain_particles(&self) -> bool {
        !matches!(
            Block::from_state_id(self.id).id,
            BlockId::BARRIER | BlockId::STRUCTURE_VOID
        )
    }

    #[must_use]
    pub const fn sided_transparency(&self) -> bool {
        self.state_flags & SIDED_TRANSPARENCY != 0
    }

    #[must_use]
    pub const fn replaceable(&self) -> bool {
        self.state_flags & REPLACEABLE != 0
    }

    #[must_use]
    pub const fn is_liquid(&self) -> bool {
        self.state_flags & IS_LIQUID != 0
    }

    /// Returns the legacy value for whether a block is solid.
    #[must_use]
    pub const fn is_solid(&self) -> bool {
        self.state_flags & IS_SOLID != 0
    }

    #[must_use]
    pub const fn is_full_cube(&self) -> bool {
        self.state_flags & IS_FULL_CUBE != 0
    }

    #[must_use]
    pub const fn is_solid_render(&self) -> bool {
        self.solid_render
    }

    /// Returns whether the block is solid.
    /// Solid blocks conduct redstone and block redstone wire.
    /// Non-solid blocks don't allow redstone wire on top to propagate their signal downwards in java.
    #[must_use]
    pub const fn is_solid_block(&self) -> bool {
        self.state_flags & IS_SOLID_BLOCK != 0
    }

    /// Returns whether this block state can emit a redstone signal.
    ///
    /// Vanilla derives this from the concrete block implementation rather than
    /// from the state properties. The generated registry retains the block name,
    /// which is the equivalent type discriminator here.
    #[must_use]
    pub fn is_signal_source(&self) -> bool {
        let name = Block::from_state_id(self.id).name;
        name.ends_with("_button")
            || name.ends_with("_pressure_plate")
            || matches!(
                name,
                "calibrated_sculk_sensor"
                    | "comparator"
                    | "daylight_detector"
                    | "detector_rail"
                    | "jukebox"
                    | "lectern"
                    | "lever"
                    | "lightning_rod"
                    | "observer"
                    | "redstone_block"
                    | "redstone_torch"
                    | "redstone_wall_torch"
                    | "redstone_wire"
                    | "repeater"
                    | "sculk_sensor"
                    | "target"
                    | "trapped_chest"
                    | "tripwire_hook"
                    | "exposed_lightning_rod"
                    | "weathered_lightning_rod"
                    | "oxidized_lightning_rod"
                    | "waxed_lightning_rod"
                    | "waxed_exposed_lightning_rod"
                    | "waxed_weathered_lightning_rod"
                    | "waxed_oxidized_lightning_rod"
            )
    }

    #[must_use]
    pub const fn has_random_ticks(&self) -> bool {
        self.state_flags & HAS_RANDOM_TICKS != 0
    }

    ///`isFaceSturdy()` in Java!
    #[must_use]
    pub const fn is_side_solid(&self, side: BlockDirection) -> bool {
        match side {
            BlockDirection::Down => self.side_flags & DOWN_SIDE_SOLID != 0,
            BlockDirection::Up => self.side_flags & UP_SIDE_SOLID != 0,
            BlockDirection::North => self.side_flags & NORTH_SIDE_SOLID != 0,
            BlockDirection::South => self.side_flags & SOUTH_SIDE_SOLID != 0,
            BlockDirection::West => self.side_flags & WEST_SIDE_SOLID != 0,
            BlockDirection::East => self.side_flags & EAST_SIDE_SOLID != 0,
        }
    }

    ///isSideSolid(..., Direction.UP, SideShapeType.CENTER) in Java!
    ///Only valid for UP and DOWN sides
    #[must_use]
    pub const fn is_center_solid(&self, side: BlockDirection) -> bool {
        match side {
            BlockDirection::Down => self.side_flags & DOWN_CENTER_SOLID != 0,
            BlockDirection::Up => self.side_flags & UP_CENTER_SOLID != 0,
            _ => false,
        }
    }

    #[must_use]
    pub fn is_waterlogged(&self) -> bool {
        let block = Block::from_state_id(self.id);

        block.properties(self.id).is_some_and(|props| {
            props
                .to_props()
                .iter()
                .any(|(k, v)| k == &"waterlogged" && v == &"true")
        })
    }

    /// Produce a new state identical to `self` except the waterlogged property
    /// is set to `true`.  If the block type does not support waterlogging or
    /// the state was already waterlogged, `None` is returned.
    #[must_use]
    pub fn with_waterlogged(&self) -> Option<&'static BlockState> {
        let block = Block::from_state_id(self.id);
        block.with_waterlogged(self.id)
    }

    pub fn get_block_collision_shapes(&self) -> impl Iterator<Item = BoundingBox> + '_ {
        self.collision_shapes
            .iter()
            .map(|&id| COLLISION_SHAPES[id as usize])
    }

    /// Returns whether this state must be considered from the neighboring block
    /// positions during collision queries. Vanilla's cache returns true for
    /// dynamic shapes and for shapes extending outside the unit block
    /// (`BlockBehaviour.java:572-574, 923-947`; `Blocks.java:830, 4265, 4377,
    /// 5126, 5336-5353, 5781`).
    #[must_use]
    pub fn has_large_collision_shape(&self) -> bool {
        let block = Block::from_state_id(self.id);
        let dynamic_shape = matches!(
            block.name,
            "moving_piston"
                | "bamboo"
                | "scaffolding"
                | "powder_snow"
                | "pointed_dripstone"
                | "sulfur_spike"
        ) || block.name == "shulker_box"
            || block.name.ends_with("_shulker_box");

        dynamic_shape
            || block.shape_offset().is_some()
            || self.get_block_collision_shapes().any(|shape| {
                shape.min.x < 0.0
                    || shape.max.x > 1.0
                    || shape.min.y < 0.0
                    || shape.max.y > 1.0
                    || shape.min.z < 0.0
                    || shape.max.z > 1.0
            })
    }

    /// Returns block-local collision shapes with vanilla's coordinate-derived offset applied.
    pub fn get_block_collision_shapes_at(
        &self,
        pos: &BlockPos,
    ) -> impl Iterator<Item = BoundingBox> + '_ {
        let offset = Block::from_state_id(self.id).shape_offset_delta(pos);
        self.get_block_collision_shapes()
            .map(move |shape| shape.shift(offset))
    }

    pub fn get_block_outline_shapes(&self) -> impl Iterator<Item = BoundingBox> + '_ {
        let base_shapes = self
            .outline_shapes
            .iter()
            .map(|&id| COLLISION_SHAPES[id as usize]);

        let water_shape = self
            .is_waterlogged()
            .then(|| BoundingBox::new(Vector3::new(0.0, 0.0, 0.0), Vector3::new(1.0, 0.875, 1.0)));

        base_shapes.chain(water_shape)
    }

    /// Returns the voxel boxes used by vanilla's light-occlusion shape.
    ///
    /// Most blocks use their outline shape. Lecterns and sculk shriekers override
    /// `getOcclusionShape` in vanilla and therefore use their collision shape.
    pub fn get_block_light_occlusion_shapes(&self) -> impl Iterator<Item = BoundingBox> + '_ {
        let shapes = match Block::from_state_id(self.id).name {
            "lectern" | "sculk_shrieker" => self.collision_shapes,
            _ => self.outline_shapes,
        };
        shapes.iter().map(|&id| COLLISION_SHAPES[id as usize])
    }

    /// Returns block-local outline shapes with vanilla's coordinate-derived offset applied.
    pub fn get_block_outline_shapes_at(
        &self,
        pos: &BlockPos,
    ) -> impl Iterator<Item = BoundingBox> + '_ {
        let offset = Block::from_state_id(self.id).shape_offset_delta(pos);
        let base_shapes = self
            .outline_shapes
            .iter()
            .map(move |&id| COLLISION_SHAPES[id as usize].shift(offset));

        let water_shape = self
            .is_waterlogged()
            .then(|| BoundingBox::new(Vector3::new(0.0, 0.0, 0.0), Vector3::new(1.0, 0.875, 1.0)));

        base_shapes.chain(water_shape)
    }

    #[must_use]
    pub fn rotate(&self, rotation: crate::block_rotation::Rotation) -> &'static Self {
        Block::from_state_id(self.id).rotate(self.id, rotation)
    }

    #[must_use]
    pub fn mirror(&self, mirror: crate::block_rotation::Mirror) -> &'static Self {
        Block::from_state_id(self.id).mirror(self.id, mirror)
    }
}

const LIGHT_SHAPE_EPSILON: f64 = 1.0e-7;

/// Returns whether the two adjacent block faces cover their complete shared
/// face, matching vanilla's `Shapes.faceShapeOccludes` for the generated
/// axis-aligned voxel shapes.
#[must_use]
pub fn light_shape_occludes(
    first: &'static BlockState,
    second: &'static BlockState,
    direction: crate::BlockDirection,
) -> bool {
    let face_boxes = |state: &'static BlockState, face_direction: crate::BlockDirection| {
        if !state.use_shape_for_light_occlusion {
            return Vec::new();
        }

        let axis: MathAxis = face_direction.to_axis().into();
        let [first_axis, second_axis] = MathAxis::excluding(axis);
        let positive = face_direction.positive();
        state
            .get_block_light_occlusion_shapes()
            .filter_map(|shape| {
                let face = if positive {
                    shape.max.get_axis(axis)
                } else {
                    shape.min.get_axis(axis)
                };
                let expected_face = if positive { 1.0 } else { 0.0 };
                if (face - expected_face).abs() > LIGHT_SHAPE_EPSILON {
                    return None;
                }

                Some((
                    shape.min.get_axis(first_axis),
                    shape.max.get_axis(first_axis),
                    shape.min.get_axis(second_axis),
                    shape.max.get_axis(second_axis),
                ))
            })
            .collect::<Vec<_>>()
    };

    let mut rectangles = face_boxes(first, direction);
    rectangles.extend(face_boxes(second, direction.opposite()));
    if rectangles.is_empty() {
        return false;
    }

    let mut x_edges = vec![0.0, 1.0];
    let mut y_edges = vec![0.0, 1.0];
    for (min_x, max_x, min_y, max_y) in &rectangles {
        x_edges.extend([*min_x, *max_x]);
        y_edges.extend([*min_y, *max_y]);
    }
    x_edges.sort_by(f64::total_cmp);
    y_edges.sort_by(f64::total_cmp);
    x_edges.dedup_by(|left, right| (*left - *right).abs() <= LIGHT_SHAPE_EPSILON);
    y_edges.dedup_by(|left, right| (*left - *right).abs() <= LIGHT_SHAPE_EPSILON);

    x_edges.windows(2).all(|x| {
        y_edges.windows(2).all(|y| {
            let x_mid = (x[0] + x[1]) * 0.5;
            let y_mid = (y[0] + y[1]) * 0.5;
            rectangles.iter().any(|(min_x, max_x, min_y, max_y)| {
                x_mid >= *min_x - LIGHT_SHAPE_EPSILON
                    && x_mid <= *max_x + LIGHT_SHAPE_EPSILON
                    && y_mid >= *min_y - LIGHT_SHAPE_EPSILON
                    && y_mid <= *max_y + LIGHT_SHAPE_EPSILON
            })
        })
    })
}

impl BlockStateId {
    // depends on generated impl:
    // pub(crate) const STATE_COUNT: u16;

    // SAFETY: There must never be a BlockStateId where self.0 >= BlockStateId::STATE_COUNT

    #[inline]
    #[must_use]
    pub const fn new(inner: u16) -> Option<Self> {
        if inner < Self::STATE_COUNT {
            return Some(Self(inner));
        }
        None
    }

    #[inline]
    #[must_use]
    pub const fn new_or_air(inner: u16) -> Self {
        if inner < Self::STATE_COUNT {
            return Self(inner);
        }
        Self::AIR
    }

    #[inline(always)]
    #[must_use]
    pub const fn as_u16(self) -> u16 {
        self.0
    }

    #[inline]
    #[must_use]
    pub const fn to_state(self) -> &'static BlockState {
        BlockState::from_id(self)
    }

    #[inline]
    #[must_use]
    pub const fn to_block_id(self) -> BlockId {
        BlockId::from_state_id(self)
    }

    #[inline]
    #[must_use]
    pub const fn to_block(self) -> &'static Block {
        Block::from_state_id(self)
    }

    #[inline]
    #[must_use]
    pub fn rotate(self, rotation: crate::block_rotation::Rotation) -> &'static BlockState {
        Block::from_state_id(self).rotate(self, rotation)
    }

    #[inline]
    #[must_use]
    pub fn mirror(self, mirror: crate::block_rotation::Mirror) -> &'static BlockState {
        Block::from_state_id(self).mirror(self, mirror)
    }
}

impl Default for BlockStateId {
    #[inline]
    fn default() -> Self {
        Self::AIR
    }
}

impl std::fmt::Display for BlockStateId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "BlockStateId({} = \"{}\")",
            self.0,
            Block::from_state_id(*self).name
        )
    }
}

//This is the Layout of state_props in the right order
// state_flags
const IS_AIR: u16 = 1 << 0;
const BURNABLE: u16 = 1 << 1;
const TOOL_REQUIRED: u16 = 1 << 2;
const SIDED_TRANSPARENCY: u16 = 1 << 3;
const REPLACEABLE: u16 = 1 << 4;
const IS_LIQUID: u16 = 1 << 5;
const IS_SOLID: u16 = 1 << 6;
const IS_FULL_CUBE: u16 = 1 << 7;
const IS_SOLID_BLOCK: u16 = 1 << 8;
const HAS_RANDOM_TICKS: u16 = 1 << 9;

// side_flags
const DOWN_SIDE_SOLID: u8 = 1 << 0;
const UP_SIDE_SOLID: u8 = 1 << 1;
const NORTH_SIDE_SOLID: u8 = 1 << 2;
const SOUTH_SIDE_SOLID: u8 = 1 << 3;
const WEST_SIDE_SOLID: u8 = 1 << 4;
const EAST_SIDE_SOLID: u8 = 1 << 5;
const DOWN_CENTER_SOLID: u8 = 1 << 6;
const UP_CENTER_SOLID: u8 = 1 << 7;

#[cfg(test)]
mod tests {
    use crate::{Block, BlockStateId, block_state_remap::remap_block_state_for_version};
    use pumpkin_util::{math::position::BlockPos, version::JavaMinecraftVersion};

    fn assert_close(actual: f64, expected: f64) {
        assert!((actual - expected).abs() < 1.0e-6, "{actual} != {expected}");
    }

    #[test]
    fn bamboo_collision_shape_uses_its_world_position() {
        let state = Block::BAMBOO.default_state;
        let origin_shape = state
            .get_block_collision_shapes_at(&BlockPos::new(0, 64, 0))
            .next()
            .unwrap();
        let shifted_shape = state
            .get_block_collision_shapes_at(&BlockPos::new(-18, 64, -7))
            .next()
            .unwrap();

        assert_close(origin_shape.min.x, 0.15625);
        assert_close(origin_shape.max.x, 0.34375);
        assert_close(shifted_shape.min.x, 0.65625);
        assert_close(shifted_shape.max.x, 0.84375);
        assert_close(shifted_shape.min.z, 0.65625);
        assert_close(shifted_shape.max.z, 0.84375);
    }

    #[test]
    fn large_collision_shape_matches_vanilla_cache_rule() {
        // `BlockBehaviour.BlockStateBase.hasLargeCollisionShape` treats offset
        // states as dynamic and therefore checks neighboring cursor positions
        // (`BlockBehaviour.java:572-574, 923-947`; `Blocks.java:4265`).
        assert!(!Block::STONE.default_state.has_large_collision_shape());
        assert!(Block::BAMBOO.default_state.has_large_collision_shape());
        assert!(Block::POWDER_SNOW.default_state.has_large_collision_shape());
    }

    #[test]
    fn supported_client_versions_keep_offset_collisions_mapped() {
        let versions = [
            JavaMinecraftVersion::V_1_20_5,
            JavaMinecraftVersion::V_1_21,
            JavaMinecraftVersion::V_1_21_2,
            JavaMinecraftVersion::V_1_21_4,
            JavaMinecraftVersion::V_1_21_5,
            JavaMinecraftVersion::V_1_21_6,
            JavaMinecraftVersion::V_1_21_7,
            JavaMinecraftVersion::V_1_21_9,
            JavaMinecraftVersion::V_1_21_11,
            JavaMinecraftVersion::V_26_1,
            JavaMinecraftVersion::V_26_2,
        ];

        for version in versions {
            for block in [Block::BAMBOO, Block::POINTED_DRIPSTONE] {
                assert_ne!(
                    remap_block_state_for_version(block.default_state.id.as_u16(), version),
                    BlockStateId::AIR.as_u16(),
                    "{} mapped to air for {version}",
                    block.name
                );
            }
        }
    }
}
