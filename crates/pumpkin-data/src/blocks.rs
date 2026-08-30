use crate::{
    BlockState, BlockStateId,
    tag::{RegistryKey, Tag, Taggable},
};
use pumpkin_util::{
    loot_table::LootTable,
    math::{experience::Experience, position::BlockPos, vector3::Vector3},
    random::hash_block_pos,
    resource_location::{FromResourceLocation, ResourceLocation, ToResourceLocation},
};
use std::hash::{Hash, Hasher};

/// Represents the static definition of a Minecraft block type.
///
/// This struct contains the base properties shared by all instances of a block
/// Data-driven attributes like `hardness` and `blast_resistance` are defined here,
/// while specific orientations or variations are stored in the associated `BlockState`.
#[derive(Debug, Clone)]
pub struct Block {
    /// The numeric ID used for internal registry mapping.
    pub id: BlockId,
    /// The unique namespaced ID (e.g., "`diamond_ore`").
    pub name: &'static str,
    /// How hard the block is to break. A value of -1.0 indicates an unbreakable block (e.g., Bedrock).
    pub hardness: f32,
    /// The block's resistance to explosions.
    pub blast_resistance: f32,
    pub map_color: u8,
    /// The friction coefficient. Default is 0.6; Ice is 0.98.
    pub slipperiness: f32,
    /// How much this block affects the speed of an entity walking on it (e.g., Soul Sand).
    pub velocity_multiplier: f32,
    /// How much this block affects an entity's jump height (e.g., Honey Blocks).
    pub jump_velocity_multiplier: f32,
    /// The ID of the item form of this block, used for inventory and drops.
    pub item_id: u16,
    /// The initial state of the block when placed without extra data.
    pub default_state: &'static BlockState,
    /// A list of all possible valid states (properties like rotation, waterlogged, etc.) for this block.
    pub states: &'static [BlockState],
    /// Fire behavior settings. If `None`, the block is not flammable.
    pub flammable: Option<Flammable>,
    /// Defines the items dropped when this block is destroyed.
    pub loot_table: Option<LootTable>,
    /// Defines the amount of XP dropped when the block is mined (e.g., Coal or Diamond).
    pub experience: Option<Experience>,
}

/// Helper struct to ensure the validity of BlockIds parsed from external sources.
/// Every [`BlockId`] is guaranteed to correspond to a valid [`Block`].
///
/// Also enables [`Block`]-type pattern matching, even in const contexts:
/// ```rs
/// const fn to_waxed(block: &'static Block) -> Option<&'static Block> {
///     match block.id {
///         BlockId::COPPER_BLOCK => Some(Block::WAXED_COPPER_BLOCK),
///         //...
///         _ => None
///     }
/// }
/// ```
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[repr(transparent)]
pub struct BlockId(u16);

impl PartialEq<BlockId> for Block {
    fn eq(&self, other: &BlockId) -> bool {
        self.id == *other
    }
}

impl PartialEq<Block> for BlockId {
    fn eq(&self, other: &Block) -> bool {
        *self == other.id
    }
}

impl PartialEq for Block {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

impl Eq for Block {}

impl Hash for Block {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.id.hash(state);
    }
}

impl Taggable for Block {
    #[inline]
    fn tag_key() -> RegistryKey {
        RegistryKey::Block
    }

    #[inline]
    fn registry_key(&self) -> &str {
        self.name
    }

    #[inline]
    fn registry_id(&self) -> u16 {
        self.id.as_u16()
    }
}

impl ToResourceLocation for &'static Block {
    fn to_resource_location(&self) -> ResourceLocation {
        format!("minecraft:{}", self.name)
    }
}

impl FromResourceLocation for &'static Block {
    fn from_resource_location(resource_location: &ResourceLocation) -> Option<Self> {
        Block::from_registry_key(
            resource_location
                .strip_prefix("minecraft:")
                .unwrap_or(resource_location),
        )
    }
}

impl Block {
    /// Returns the vanilla `shouldSpawnTerrainParticles` property for this block.
    /// The only 26.2 registrations using `noTerrainParticles` are barrier and
    /// structure void (`BlockBehaviour.java:1265-1267`, `Blocks.java:2936,3776`).
    #[must_use]
    pub const fn should_spawn_terrain_particles(&self) -> bool {
        !matches!(self.id, BlockId::BARRIER | BlockId::STRUCTURE_VOID)
    }

    pub(crate) fn shape_offset_delta(&self, pos: &BlockPos) -> Vector3<f64> {
        let Some(shape_offset) = self.shape_offset() else {
            return Vector3::new(0.0, 0.0, 0.0);
        };

        let seed = hash_block_pos(pos.0.x, 0, pos.0.z) as u64;
        let max_horizontal = f64::from(shape_offset.max_horizontal);
        let x = (f64::from((seed & 15) as f32 / 15.0) - 0.5) * 0.5;
        let x = x.clamp(-max_horizontal, max_horizontal);
        let z = (f64::from(((seed >> 8) & 15) as f32 / 15.0) - 0.5) * 0.5;
        let z = z.clamp(-max_horizontal, max_horizontal);
        let y = match shape_offset.offset_type {
            ShapeOffsetType::Xz => 0.0,
            ShapeOffsetType::Xyz => {
                (f64::from(((seed >> 4) & 15) as f32 / 15.0) - 1.0)
                    * f64::from(shape_offset.max_vertical)
            }
        };

        // Extracted shapes are sampled at BlockPos::ZERO, where vanilla uses the
        // negative horizontal limits and, for XYZ offsets, the negative vertical
        // limit. Return only the delta from that sample.
        Vector3::new(
            x + max_horizontal,
            y + shape_offset.offset_type.origin_y(shape_offset.max_vertical),
            z + max_horizontal,
        )
    }

    #[must_use]
    pub fn is_waterlogged(&self, id: BlockStateId) -> bool {
        self.properties(id).is_some_and(|properties| {
            properties
                .to_props()
                .into_iter()
                .any(|(key, value)| key == "waterlogged" && value == "true")
        })
    }

    /// Returns a new [`BlockState`] reference for the given [`BlockStateId`] with the
    /// `waterlogged` property forced to `true` if the block supports that
    /// property.  If the state is already waterlogged or the block does not
    /// expose a `waterlogged` property then `None` is returned.
    #[must_use]
    pub fn with_waterlogged(&self, id: BlockStateId) -> Option<&'static BlockState> {
        // Check if already waterlogged
        if self.is_waterlogged(id) {
            return Some(BlockState::from_id(id));
        }

        // Modify the property list if available
        if let Some(props_source) = self.properties(id) {
            let mut props: Vec<(&str, &str)> = props_source
                .to_props()
                .iter()
                .map(|(k, v)| (*k, *v))
                .collect();

            // Look for an existing waterlogged key or add one
            if let Some(idx) = props.iter().position(|(k, _)| *k == "waterlogged") {
                props[idx] = ("waterlogged", "true");
            } else {
                props.push(("waterlogged", "true"));
            }

            let new_state_id = self.from_properties(&props).to_state_id(self);
            return Some(BlockState::from_id(new_state_id));
        }

        None
    }

    /// Returns whether this block is solid (based on default state)
    #[must_use]
    pub const fn is_solid(&self) -> bool {
        self.default_state.is_solid()
    }

    /// Returns whether this block is air (based on default state)
    #[must_use]
    pub const fn is_air(&self) -> bool {
        self.default_state.is_air()
    }

    /// Applies a property transform to a state and resolves the result back to a
    /// `BlockState`. States without properties, and transforms that leave every
    /// property alone, resolve back to the input state.
    fn transform_state(
        &self,
        id: BlockStateId,
        transform: impl FnOnce(&mut [(&'static str, &'static str)]),
    ) -> &'static BlockState {
        let Some(props_source) = self.properties(id) else {
            return BlockState::from_id(id);
        };
        let mut props = props_source.to_props();
        transform(&mut props);
        BlockState::from_id(self.from_properties(&props).to_state_id(self))
    }

    /// Mirrors this block state.
    ///
    /// Vanilla dispatches this per block (`BlockBehaviour.mirror`,
    /// `BlockBehaviour.java:260-262`, returning the state unchanged unless a block
    /// overrides it). This is the property-driven equivalent: it mirrors whichever of
    /// `facing`, `rotation`, `hinge`, `shape`, `orientation` and the four side
    /// properties the state carries. See [`crate::block_rotation::Mirror::apply_to_props`]
    /// for the per-family citations.
    #[must_use]
    pub fn mirror(
        &self,
        id: BlockStateId,
        mirror: crate::block_rotation::Mirror,
    ) -> &'static BlockState {
        if mirror == crate::block_rotation::Mirror::None {
            return BlockState::from_id(id);
        }
        self.transform_state(id, |props| mirror.apply_to_props(self.name, props))
    }

    /// Rotates this block state about the Y axis.
    ///
    /// The property-driven equivalent of vanilla's per-block `rotate` overrides
    /// (`BlockBehaviour.rotate`, `BlockBehaviour.java:256-258`). See
    /// [`crate::block_rotation::Rotation::apply_to_props`] for the per-family citations.
    #[must_use]
    pub fn rotate(
        &self,
        id: BlockStateId,
        rotation: crate::block_rotation::Rotation,
    ) -> &'static BlockState {
        if rotation == crate::block_rotation::Rotation::None {
            return BlockState::from_id(id);
        }
        self.transform_state(id, |props| rotation.apply_to_props(self.name, props))
    }

    /// Parses a block state argument such as `minecraft:wheat[age=0]`, resolving any
    /// supplied properties against the block's definition. Returns `None` if the block
    /// name is unknown, a property name/value is not valid for the block, or the block
    /// does not have properties at all but some were supplied.
    #[must_use]
    pub fn from_state_str(input: &str) -> Option<&'static BlockState> {
        let (name, props) = match input.find('[') {
            Some(idx) if input.ends_with(']') => (&input[..idx], &input[idx + 1..input.len() - 1]),
            _ => (input, ""),
        };

        let block = Self::from_name(name)?;

        let props = props.trim();
        if props.is_empty() {
            return Some(block.default_state);
        }

        let valid_keys: Vec<&str> = block
            .properties(block.default_state.id)
            .map(|properties| {
                properties
                    .to_props()
                    .into_iter()
                    .map(|(key, _)| key)
                    .collect()
            })
            .unwrap_or_default();

        let mut pairs = Vec::new();
        for part in props.split(',') {
            let mut kv = part.splitn(2, '=');
            let key = kv.next().unwrap_or("").trim();
            let value = kv.next().unwrap_or("").trim();

            if !valid_keys.contains(&key) {
                return None;
            }

            // Some property values only fail once resolved against the block's generated
            // enum types; validate each key in isolation before committing to the batch
            // below so a bad value is reported here instead of panicking downstream.
            if std::panic::catch_unwind(|| block.from_properties(&[(key, value)])).is_err() {
                return None;
            }

            pairs.push((key, value));
        }

        let state_id = block.from_properties(&pairs).to_state_id(block);
        Some(BlockState::from_id(state_id))
    }
}

#[derive(Clone, Copy)]
pub(crate) enum ShapeOffsetType {
    Xz,
    Xyz,
}

#[derive(Clone, Copy)]
pub(crate) struct ShapeOffset {
    pub offset_type: ShapeOffsetType,
    pub max_horizontal: f32,
    pub max_vertical: f32,
}

impl ShapeOffsetType {
    fn origin_y(self, max_vertical: f32) -> f64 {
        match self {
            Self::Xz => 0.0,
            Self::Xyz => f64::from(max_vertical),
        }
    }
}

impl BlockId {
    // depends on generated impl:
    // pub(crate) const BLOCK_COUNT: u16;

    // SAFETY: There must never be a BlockId where self.0 >= BlockId::BLOCK_COUNT

    #[inline]
    #[must_use]
    pub const fn new(inner: u16) -> Option<Self> {
        if inner < Self::BLOCK_COUNT {
            return Some(Self(inner));
        }
        None
    }

    #[inline]
    #[must_use]
    pub const fn new_or_air(inner: u16) -> Self {
        if inner < Self::BLOCK_COUNT {
            return Self(inner);
        }
        Self::AIR
    }

    #[inline]
    #[must_use]
    pub const fn to_block(self) -> &'static Block {
        Block::from_id(self)
    }

    #[inline(always)]
    #[must_use]
    pub const fn as_u16(self) -> u16 {
        self.0
    }

    #[inline]
    #[must_use]
    pub fn has_tag(self, tag: Tag) -> bool {
        tag.1.contains(&self.0)
    }
}

impl From<BlockId> for u16 {
    #[inline]
    fn from(value: BlockId) -> Self {
        value.as_u16()
    }
}

impl Default for BlockId {
    #[inline]
    fn default() -> Self {
        Self::AIR
    }
}

impl std::fmt::Display for BlockId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "BlockId({} = \"{}\")",
            self.0,
            Block::from_id(*self).name
        )
    }
}

#[derive(Clone, Debug)]
pub struct Flammable {
    pub spread_chance: u8,
    pub burn_chance: u8,
}

#[cfg(test)]
mod tests {
    use super::{Block, BlockId, ShapeOffsetType};
    use pumpkin_util::math::position::BlockPos;

    fn assert_close(actual: f64, expected: f64) {
        assert!((actual - expected).abs() < 1.0e-6, "{actual} != {expected}");
    }

    #[test]
    fn shape_offset_registry_matches_vanilla_26_2() {
        let mut xz = 0;
        let mut xyz = 0;

        for raw_id in 0..BlockId::BLOCK_COUNT {
            match Block::from_id(BlockId::new(raw_id).unwrap())
                .shape_offset()
                .map(|offset| offset.offset_type)
            {
                Some(ShapeOffsetType::Xz) => xz += 1,
                Some(ShapeOffsetType::Xyz) => xyz += 1,
                None => {}
            }
        }

        assert_eq!(xz, 34);
        assert_eq!(xyz, 5);
    }

    #[test]
    fn terrain_particle_property_matches_vanilla_registrations() {
        assert!(Block::STONE.should_spawn_terrain_particles());
        assert!(!Block::BARRIER.should_spawn_terrain_particles());
        assert!(!Block::STRUCTURE_VOID.should_spawn_terrain_particles());
    }

    #[test]
    fn shape_offset_limits_match_vanilla_26_2() {
        let bamboo = Block::BAMBOO.shape_offset().unwrap();
        assert_eq!(bamboo.max_horizontal, 0.25);
        assert_eq!(bamboo.max_vertical, 0.2);

        let pointed_dripstone = Block::POINTED_DRIPSTONE.shape_offset().unwrap();
        assert_eq!(pointed_dripstone.max_horizontal, 0.125);

        let small_dripleaf = Block::SMALL_DRIPLEAF.shape_offset().unwrap();
        assert_eq!(small_dripleaf.max_vertical, 0.1);
    }

    #[test]
    fn shape_offset_delta_matches_vanilla_coordinate_hash() {
        let origin = BlockPos::new(0, 64, 0);
        let positive_extreme = BlockPos::new(-18, 64, -7);

        let origin_delta = Block::BAMBOO.shape_offset_delta(&origin);
        assert_eq!(origin_delta.x, 0.0);
        assert_eq!(origin_delta.y, 0.0);
        assert_eq!(origin_delta.z, 0.0);

        let bamboo_delta = Block::BAMBOO.shape_offset_delta(&positive_extreme);
        assert_eq!(bamboo_delta.x, 0.5);
        assert_eq!(bamboo_delta.y, 0.0);
        assert_eq!(bamboo_delta.z, 0.5);

        let speleothem_delta = Block::POINTED_DRIPSTONE.shape_offset_delta(&positive_extreme);
        assert_eq!(speleothem_delta.x, 0.25);
        assert_eq!(speleothem_delta.y, 0.0);
        assert_eq!(speleothem_delta.z, 0.25);
        assert_eq!(
            Block::SULFUR_SPIKE.shape_offset_delta(&positive_extreme),
            speleothem_delta
        );

        let xyz_delta = Block::SHORT_GRASS.shape_offset_delta(&positive_extreme);
        assert_eq!(xyz_delta.x, 0.5);
        assert_close(xyz_delta.y, 0.08);
        assert_eq!(xyz_delta.z, 0.5);

        assert_eq!(Block::STONE.shape_offset_delta(&positive_extreme).x, 0.0);
    }

    #[test]
    fn rotate_and_mirror_resolve_to_real_states() {
        use crate::block_rotation::{Mirror, Rotation};

        let stairs = Block::from_state_str(
            "minecraft:oak_stairs[facing=north,half=bottom,shape=inner_left,waterlogged=false]",
        )
        .expect("oak stairs state");
        let block = Block::from_name("oak_stairs").expect("oak stairs");

        // Rotation.NONE / Mirror.NONE are the identity (BlockBehaviour.java:256-262).
        assert_eq!(block.rotate(stairs.id, Rotation::None).id, stairs.id);
        assert_eq!(block.mirror(stairs.id, Mirror::None).id, stairs.id);

        let rotated = block.rotate(stairs.id, Rotation::Clockwise90);
        let expected = Block::from_state_str(
            "minecraft:oak_stairs[facing=east,half=bottom,shape=inner_left,waterlogged=false]",
        )
        .expect("rotated oak stairs state");
        assert_eq!(rotated.id, expected.id);

        let mirrored = block.mirror(stairs.id, Mirror::LeftRight);
        let expected = Block::from_state_str(
            "minecraft:oak_stairs[facing=south,half=bottom,shape=inner_right,waterlogged=false]",
        )
        .expect("mirrored oak stairs state");
        assert_eq!(mirrored.id, expected.id);
    }

    #[test]
    fn rotating_a_propertyless_block_is_a_no_op() {
        use crate::block_rotation::Rotation;

        let stone = &Block::STONE;
        assert_eq!(
            stone
                .rotate(stone.default_state.id, Rotation::Clockwise90)
                .id,
            stone.default_state.id
        );
    }

    #[test]
    fn pillar_axis_round_trips_through_state_ids() {
        use crate::block_rotation::Rotation;

        let block = Block::from_name("oak_log").expect("oak log");
        let x_axis = Block::from_state_str("minecraft:oak_log[axis=x]").expect("oak log axis=x");
        let rotated = block.rotate(x_axis.id, Rotation::Clockwise90);
        let z_axis = Block::from_state_str("minecraft:oak_log[axis=z]").expect("oak log axis=z");
        assert_eq!(rotated.id, z_axis.id);
        assert_eq!(
            block.rotate(rotated.id, Rotation::Clockwise90).id,
            x_axis.id
        );
    }
}
