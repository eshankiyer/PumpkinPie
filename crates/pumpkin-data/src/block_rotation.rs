//! Block rotation and mirroring transformations.
//!
//! These transformations are used to rotate and mirror blocks
//! when placing them in the world, matching vanilla Minecraft behavior.

use pumpkin_util::math::vector3::Vector3;
use serde::Deserialize;

/// Rotation around the Y axis in 90-degree increments.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Rotation {
    /// No rotation (0 degrees)
    #[default]
    None,
    /// 90 degrees clockwise
    Clockwise90,
    /// 180 degrees
    Rotate180,
    /// 270 degrees clockwise (90 degrees counter-clockwise)
    CounterClockwise90,
}

impl Rotation {
    /// Returns all possible rotations.
    #[must_use]
    pub const fn values() -> [Self; 4] {
        [
            Self::None,
            Self::Clockwise90,
            Self::Rotate180,
            Self::CounterClockwise90,
        ]
    }

    /// Gets a random rotation from the given random value (0-3).
    #[must_use]
    pub const fn from_index(index: u8) -> Self {
        match index % 4 {
            0 => Self::None,
            1 => Self::Clockwise90,
            2 => Self::Rotate180,
            _ => Self::CounterClockwise90,
        }
    }

    /// Transforms a position within the template bounds according to this rotation.
    ///
    /// The position is rotated around the Y axis. The `size` parameter defines
    /// the template dimensions, used to calculate the pivot point.
    #[must_use]
    pub const fn transform_pos(&self, pos: Vector3<i32>, size: Vector3<i32>) -> Vector3<i32> {
        match self {
            Self::None => pos,
            Self::Clockwise90 => Vector3::new(size.z - 1 - pos.z, pos.y, pos.x),
            Self::Rotate180 => Vector3::new(size.x - 1 - pos.x, pos.y, size.z - 1 - pos.z),
            Self::CounterClockwise90 => Vector3::new(pos.z, pos.y, size.x - 1 - pos.x),
        }
    }

    /// Rotates an X/Z offset around the origin.
    ///
    /// Unlike `transform_pos` which rotates within template bounds,
    /// this rotates a simple offset (e.g. sub-template positioning).
    #[must_use]
    pub const fn rotate_offset(self, x: i32, z: i32) -> (i32, i32) {
        match self {
            Self::None => (x, z),
            Self::Clockwise90 => (-z, x),
            Self::Rotate180 => (-x, -z),
            Self::CounterClockwise90 => (z, -x),
        }
    }

    /// Rotates the template size dimensions according to this rotation.
    ///
    /// For 90 and 270 degree rotations, X and Z dimensions are swapped.
    #[must_use]
    pub const fn transform_size(&self, size: Vector3<i32>) -> Vector3<i32> {
        match self {
            Self::None | Self::Rotate180 => size,
            Self::Clockwise90 | Self::CounterClockwise90 => Vector3::new(size.z, size.y, size.x),
        }
    }

    /// Rotates a horizontal facing direction.
    ///
    /// Takes a facing string (north/south/east/west) and returns the rotated facing.
    #[must_use]
    pub fn rotate_facing(&self, facing: &str) -> &'static str {
        match self {
            Self::None => match facing {
                "north" => "north",
                "south" => "south",
                "east" => "east",
                "west" => "west",
                "up" => "up",
                "down" => "down",
                _ => leak_str(facing),
            },
            Self::Clockwise90 => match facing {
                "north" => "east",
                "east" => "south",
                "south" => "west",
                "west" => "north",
                "up" => "up",
                "down" => "down",
                _ => leak_str(facing),
            },
            Self::Rotate180 => match facing {
                "north" => "south",
                "south" => "north",
                "east" => "west",
                "west" => "east",
                "up" => "up",
                "down" => "down",
                _ => leak_str(facing),
            },
            Self::CounterClockwise90 => match facing {
                "north" => "west",
                "west" => "south",
                "south" => "east",
                "east" => "north",
                "up" => "up",
                "down" => "down",
                _ => leak_str(facing),
            },
        }
    }

    /// Rotates a horizontal axis.
    ///
    /// Takes an axis string (x/z) and returns the rotated axis.
    #[must_use]
    pub fn rotate_axis(&self, axis: &str) -> &'static str {
        match self {
            Self::None | Self::Rotate180 => match axis {
                "x" => "x",
                "z" => "z",
                "y" => "y",
                _ => leak_str(axis),
            },
            Self::Clockwise90 | Self::CounterClockwise90 => match axis {
                "x" => "z",
                "z" => "x",
                "y" => "y",
                _ => leak_str(axis),
            },
        }
    }

    /// Rotates a block rotation value (0-15, used for signs and banners).
    #[must_use]
    pub const fn rotate_block_rotation(&self, rotation: i32) -> i32 {
        match self {
            Self::None => rotation,
            Self::Clockwise90 => (rotation + 4) % 16,
            Self::Rotate180 => (rotation + 8) % 16,
            Self::CounterClockwise90 => (rotation + 12) % 16,
        }
    }

    /// Combines this rotation with another rotation.
    #[must_use]
    pub const fn then(&self, other: Self) -> Self {
        match self {
            Self::None => other,
            Self::Clockwise90 => match other {
                Self::None => Self::Clockwise90,
                Self::Clockwise90 => Self::Rotate180,
                Self::Rotate180 => Self::CounterClockwise90,
                Self::CounterClockwise90 => Self::None,
            },
            Self::Rotate180 => match other {
                Self::None => Self::Rotate180,
                Self::Clockwise90 => Self::CounterClockwise90,
                Self::Rotate180 => Self::None,
                Self::CounterClockwise90 => Self::Clockwise90,
            },
            Self::CounterClockwise90 => match other {
                Self::None => Self::CounterClockwise90,
                Self::Clockwise90 => Self::None,
                Self::Rotate180 => Self::Clockwise90,
                Self::CounterClockwise90 => Self::Rotate180,
            },
        }
    }

    /// Returns the inverse of this rotation.
    #[must_use]
    pub const fn inverse(&self) -> Self {
        match self {
            Self::None => Self::None,
            Self::Clockwise90 => Self::CounterClockwise90,
            Self::Rotate180 => Self::Rotate180,
            Self::CounterClockwise90 => Self::Clockwise90,
        }
    }

    /// Converts rotation to a primary axis for bounding box creation.
    #[must_use]
    pub const fn to_axis(self) -> pumpkin_util::math::vector3::Axis {
        match self {
            Self::None | Self::Rotate180 => pumpkin_util::math::vector3::Axis::Z,
            Self::Clockwise90 | Self::CounterClockwise90 => pumpkin_util::math::vector3::Axis::X,
        }
    }
}

/// Mirror transformation for structure templates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mirror {
    /// No mirroring
    #[default]
    None,
    /// Mirror along the Z axis (`OctahedralGroup.INVERT_Z` in vanilla) - flips north/south
    LeftRight,
    /// Mirror along the X axis (`OctahedralGroup.INVERT_X` in vanilla) - flips east/west
    FrontBack,
}

impl Mirror {
    /// Returns all possible mirrors.
    #[must_use]
    pub const fn values() -> [Self; 3] {
        [Self::None, Self::LeftRight, Self::FrontBack]
    }

    /// Transforms a position within the template bounds according to this mirror.
    #[must_use]
    pub const fn transform_pos(&self, pos: Vector3<i32>, size: Vector3<i32>) -> Vector3<i32> {
        match self {
            Self::None => pos,
            Self::LeftRight => Vector3::new(pos.x, pos.y, size.z - 1 - pos.z),
            Self::FrontBack => Vector3::new(size.x - 1 - pos.x, pos.y, pos.z),
        }
    }

    /// Mirrors a horizontal facing direction.
    #[must_use]
    pub fn mirror_facing(&self, facing: &str) -> &'static str {
        match self {
            Self::None => match facing {
                "north" => "north",
                "south" => "south",
                "east" => "east",
                "west" => "west",
                "up" => "up",
                "down" => "down",
                _ => leak_str(facing),
            },
            Self::LeftRight => match facing {
                "north" => "south",
                "south" => "north",
                "east" => "east",
                "west" => "west",
                "up" => "up",
                "down" => "down",
                _ => leak_str(facing),
            },
            Self::FrontBack => match facing {
                "east" => "west",
                "west" => "east",
                "north" => "north",
                "south" => "south",
                "up" => "up",
                "down" => "down",
                _ => leak_str(facing),
            },
        }
    }

    /// Mirrors a block rotation value (0-15, used for signs and banners).
    ///
    /// Matches vanilla `Mirror.mirror(rotation, steps)` for `steps == 16`: vanilla first
    /// corrects `rotation` into `-8..=8` before applying the formula, but for `steps == 16`
    /// that correction is a no-op modulo 16 over the `0..16` input range this takes, so it's
    /// omitted here.
    #[must_use]
    pub const fn mirror_block_rotation(&self, rotation: i32) -> i32 {
        match self {
            Self::None => rotation,
            Self::LeftRight => (8 - rotation + 16) % 16,
            Self::FrontBack => (16 - rotation) % 16,
        }
    }

    /// Returns the rotation needed to achieve this mirror from a base rotation.
    ///
    /// Unlike the other methods on this type, this has no identified vanilla counterpart
    /// (vanilla's `Mirror.getRotation` takes a `Direction`, not a `Rotation`) and currently
    /// has no callers, so its behavior is left as-is rather than guessed at.
    #[must_use]
    pub const fn get_rotation(&self, rotation: Rotation) -> Rotation {
        match self {
            Self::None => rotation,
            Self::LeftRight => match rotation {
                Rotation::None => Rotation::None,
                Rotation::Clockwise90 => Rotation::CounterClockwise90,
                Rotation::Rotate180 => Rotation::Rotate180,
                Rotation::CounterClockwise90 => Rotation::Clockwise90,
            },
            Self::FrontBack => match rotation {
                Rotation::None => Rotation::Rotate180,
                Rotation::Clockwise90 => Rotation::Clockwise90,
                Rotation::Rotate180 => Rotation::None,
                Rotation::CounterClockwise90 => Rotation::CounterClockwise90,
            },
        }
    }
}

/// Block names that carry `north`/`south`/`east`/`west` properties but which vanilla
/// deliberately leaves untransformed: `FireBlock` (`FireBlock.java:30`) and
/// `ChorusPlantBlock` (`ChorusPlantBlock.java:18`) declare neither `rotate` nor `mirror`,
/// so `BlockBehaviour.rotate`/`mirror` (`BlockBehaviour.java:256-262`) returns the state
/// unchanged for them.
const SIDE_PERMUTATION_EXEMPT: [&str; 2] = ["fire", "chorus_plant"];

fn permutes_sides(block_name: &str) -> bool {
    !SIDE_PERMUTATION_EXEMPT.contains(&block_name)
}

fn is_direction(value: &str) -> bool {
    matches!(value, "north" | "south" | "east" | "west" | "up" | "down")
}

/// Maps a `0..16` rotation index to a static string, avoiding an allocation on the
/// structure-placement hot path.
const fn rotation16_to_str(rotation: i32) -> &'static str {
    match rotation.rem_euclid(16) {
        1 => "1",
        2 => "2",
        3 => "3",
        4 => "4",
        5 => "5",
        6 => "6",
        7 => "7",
        8 => "8",
        9 => "9",
        10 => "10",
        11 => "11",
        12 => "12",
        13 => "13",
        14 => "14",
        15 => "15",
        _ => "0",
    }
}

/// Recombines a `FrontAndTop` pair into the `orientation` property value used by
/// jigsaw blocks and crafters. Returns `None` when the pair is not one of the twelve
/// valid combinations.
fn orientation_value(front: &str, top: &str) -> Option<&'static str> {
    Some(match (front, top) {
        ("down", "east") => "down_east",
        ("down", "north") => "down_north",
        ("down", "south") => "down_south",
        ("down", "west") => "down_west",
        ("up", "east") => "up_east",
        ("up", "north") => "up_north",
        ("up", "south") => "up_south",
        ("up", "west") => "up_west",
        ("west", "up") => "west_up",
        ("east", "up") => "east_up",
        ("north", "up") => "north_up",
        ("south", "up") => "south_up",
        _ => return None,
    })
}

impl Rotation {
    /// Rotates a rail shape.
    ///
    /// Mirrors `BaseRailBlock.rotate(RailShape, Rotation)`
    /// (`BaseRailBlock.java:146-228`). Returns `None` for values that are not rail
    /// shapes, such as the stair shapes, which vanilla's `StairBlock.rotate`
    /// (`StairBlock.java:169-171`) leaves untouched.
    #[must_use]
    pub fn rotate_rail_shape(self, shape: &str) -> Option<&'static str> {
        Some(match self {
            Self::None => return None,
            Self::Clockwise90 => match shape {
                "north_south" => "east_west",
                "east_west" => "north_south",
                "ascending_east" => "ascending_south",
                "ascending_west" => "ascending_north",
                "ascending_north" => "ascending_east",
                "ascending_south" => "ascending_west",
                "south_east" => "south_west",
                "south_west" => "north_west",
                "north_west" => "north_east",
                "north_east" => "south_east",
                _ => return None,
            },
            Self::Rotate180 => match shape {
                "north_south" => "north_south",
                "east_west" => "east_west",
                "ascending_east" => "ascending_west",
                "ascending_west" => "ascending_east",
                "ascending_north" => "ascending_south",
                "ascending_south" => "ascending_north",
                "south_east" => "north_west",
                "south_west" => "north_east",
                "north_west" => "south_east",
                "north_east" => "south_west",
                _ => return None,
            },
            Self::CounterClockwise90 => match shape {
                "north_south" => "east_west",
                "east_west" => "north_south",
                "ascending_east" => "ascending_north",
                "ascending_west" => "ascending_south",
                "ascending_north" => "ascending_west",
                "ascending_south" => "ascending_east",
                "south_east" => "north_east",
                "south_west" => "south_east",
                "north_west" => "south_west",
                "north_east" => "north_west",
                _ => return None,
            },
        })
    }

    /// Rotates a jigsaw/crafter `orientation` value.
    ///
    /// Vanilla applies the rotation's `OctahedralGroup` to the `FrontAndTop`
    /// (`JigsawBlock.java:41-43`, `CrafterBlock.java:234-236`), which for a rotation
    /// about Y is the same as rotating the front and top directions independently.
    #[must_use]
    pub fn rotate_orientation(self, value: &str) -> Option<&'static str> {
        let (front, top) = value.split_once('_')?;
        if !is_direction(front) || !is_direction(top) {
            return None;
        }
        orientation_value(self.rotate_facing(front), self.rotate_facing(top))
    }

    /// Applies this rotation to a block state's property list in place.
    ///
    /// This is the property-driven equivalent of vanilla's per-block `rotate`
    /// overrides: rotating whichever of `facing`, `axis`, `rotation`, `shape`,
    /// `orientation` and the four side properties a state happens to carry.
    pub fn apply_to_props(self, block_name: &str, props: &mut [(&'static str, &'static str)]) {
        if self == Self::None {
            return;
        }
        let sides = permutes_sides(block_name);
        for (key, value) in props.iter_mut() {
            match *key {
                // HorizontalDirectionalBlock.java:21-23, DirectionalBlock subclasses.
                "facing" if is_direction(*value) => *value = self.rotate_facing(*value),
                // RotatedPillarBlock.rotatePillar, RotatedPillarBlock.java:31-45.
                "axis" => *value = self.rotate_axis(*value),
                // StandingSignBlock.java:72-74, BannerBlock, SkullBlock.
                "rotation" => {
                    if let Ok(parsed) = value.parse::<i32>() {
                        *value = rotation16_to_str(self.rotate_block_rotation(parsed));
                    }
                }
                "shape" => {
                    if let Some(rotated) = self.rotate_rail_shape(*value) {
                        *value = rotated;
                    }
                }
                "orientation" => {
                    if let Some(rotated) = self.rotate_orientation(*value) {
                        *value = rotated;
                    }
                }
                // CrossCollisionBlock.java:96-112, WallBlock.java:271-287,
                // MultifaceBlock, VineBlock, TripWireBlock, RedStoneWireBlock: the
                // value stays put and moves to the rotated side.
                "north" | "south" | "east" | "west" if sides => *key = self.rotate_facing(*key),
                _ => {}
            }
        }
    }
}

impl Mirror {
    /// Mirrors a rail shape.
    ///
    /// Mirrors `BaseRailBlock.mirror(RailShape, Mirror)` (`BaseRailBlock.java:230-276`).
    #[must_use]
    pub fn mirror_rail_shape(self, shape: &str) -> Option<&'static str> {
        Some(match self {
            Self::None => return None,
            Self::LeftRight => match shape {
                "ascending_north" => "ascending_south",
                "ascending_south" => "ascending_north",
                "south_east" => "north_east",
                "south_west" => "north_west",
                "north_west" => "south_west",
                "north_east" => "south_east",
                _ => return None,
            },
            Self::FrontBack => match shape {
                "ascending_east" => "ascending_west",
                "ascending_west" => "ascending_east",
                "south_east" => "south_west",
                "south_west" => "south_east",
                "north_west" => "north_east",
                "north_east" => "north_west",
                _ => return None,
            },
        })
    }

    /// Mirrors a stair `shape`, given whether this mirror flipped the stair's facing.
    ///
    /// `StairBlock.mirror` (`StairBlock.java:174-212`) only remaps the shape when the
    /// facing lies on the mirrored axis, and the two mirrors are deliberately
    /// asymmetric: `LEFT_RIGHT` swaps both the inner and the outer corners, while
    /// `FRONT_BACK` swaps only the outer ones.
    #[must_use]
    pub fn mirror_stairs_shape(self, shape: &str, facing_flipped: bool) -> Option<&'static str> {
        if !facing_flipped {
            return None;
        }
        Some(match self {
            Self::None => return None,
            Self::LeftRight => match shape {
                "inner_left" => "inner_right",
                "inner_right" => "inner_left",
                "outer_left" => "outer_right",
                "outer_right" => "outer_left",
                _ => return None,
            },
            Self::FrontBack => match shape {
                "outer_left" => "outer_right",
                "outer_right" => "outer_left",
                _ => return None,
            },
        })
    }

    /// Mirrors a jigsaw/crafter `orientation` value
    /// (`JigsawBlock.java:46-48`, `CrafterBlock.java:239-241`).
    #[must_use]
    pub fn mirror_orientation(self, value: &str) -> Option<&'static str> {
        let (front, top) = value.split_once('_')?;
        if !is_direction(front) || !is_direction(top) {
            return None;
        }
        orientation_value(self.mirror_facing(front), self.mirror_facing(top))
    }

    /// Applies this mirror to a block state's property list in place.
    pub fn apply_to_props(self, block_name: &str, props: &mut [(&'static str, &'static str)]) {
        if self == Self::None {
            return;
        }
        let sides = permutes_sides(block_name);
        // StairBlock.mirror reads the pre-mirror facing to decide the shape remap.
        let facing_flipped = props
            .iter()
            .find(|(key, _)| *key == "facing")
            .map(|(_, value)| *value)
            .is_some_and(|value| self.mirror_facing(value) != value);
        for (key, value) in props.iter_mut() {
            match *key {
                "facing" if is_direction(*value) => *value = self.mirror_facing(*value),
                // Mirror.java: INVERT_X / INVERT_Z never swap axes, so `axis` is fixed.
                "rotation" => {
                    if let Ok(parsed) = value.parse::<i32>() {
                        *value = rotation16_to_str(self.mirror_block_rotation(parsed));
                    }
                }
                // DoorBlock.java:257-259 cycles HINGE for any non-NONE mirror, even
                // when the facing itself is unaffected.
                "hinge" => {
                    *value = match *value {
                        "left" => "right",
                        "right" => "left",
                        other => other,
                    };
                }
                "shape" => {
                    if let Some(mirrored) = self
                        .mirror_rail_shape(*value)
                        .or_else(|| self.mirror_stairs_shape(*value, facing_flipped))
                    {
                        *value = mirrored;
                    }
                }
                "orientation" => {
                    if let Some(mirrored) = self.mirror_orientation(*value) {
                        *value = mirrored;
                    }
                }
                "north" | "south" | "east" | "west" if sides => *key = self.mirror_facing(*key),
                _ => {}
            }
        }
    }
}

/// Leaks a string to get a 'static str.
/// This is used for non-standard property values that aren't covered by static strings.
pub fn leak_str(s: &str) -> &'static str {
    s.to_string().leak()
}

#[cfg(test)]
mod mirror_tests {
    use super::*;

    // Vanilla `Mirror.java`: LEFT_RIGHT is OctahedralGroup.INVERT_Z and only flips
    // directions on the Z axis (north/south). FRONT_BACK is INVERT_X and only flips
    // directions on the X axis (east/west).

    #[test]
    fn left_right_flips_north_south() {
        assert_eq!(Mirror::LeftRight.mirror_facing("north"), "south");
        assert_eq!(Mirror::LeftRight.mirror_facing("south"), "north");
    }

    #[test]
    fn left_right_leaves_east_west_unchanged() {
        assert_eq!(Mirror::LeftRight.mirror_facing("east"), "east");
        assert_eq!(Mirror::LeftRight.mirror_facing("west"), "west");
    }

    #[test]
    fn front_back_flips_east_west() {
        assert_eq!(Mirror::FrontBack.mirror_facing("east"), "west");
        assert_eq!(Mirror::FrontBack.mirror_facing("west"), "east");
    }

    #[test]
    fn front_back_leaves_north_south_unchanged() {
        assert_eq!(Mirror::FrontBack.mirror_facing("north"), "north");
        assert_eq!(Mirror::FrontBack.mirror_facing("south"), "south");
    }

    #[test]
    fn left_right_transform_pos_flips_z() {
        let size = Vector3::new(4, 1, 6);
        let pos = Vector3::new(1, 0, 2);
        assert_eq!(
            Mirror::LeftRight.transform_pos(pos, size),
            Vector3::new(1, 0, 3)
        );
    }

    #[test]
    fn front_back_transform_pos_flips_x() {
        let size = Vector3::new(4, 1, 6);
        let pos = Vector3::new(1, 0, 2);
        assert_eq!(
            Mirror::FrontBack.transform_pos(pos, size),
            Vector3::new(2, 0, 2)
        );
    }

    #[test]
    fn left_right_block_rotation_matches_vanilla_formula() {
        // vanilla: (halfSteps - correctedRotation + steps) % steps, steps=16, halfSteps=8
        for rotation in 0..16 {
            assert_eq!(
                Mirror::LeftRight.mirror_block_rotation(rotation),
                (8 - rotation + 16) % 16
            );
        }
    }

    #[test]
    fn front_back_block_rotation_matches_vanilla_formula() {
        // vanilla: (steps - correctedRotation) % steps, steps=16
        for rotation in 0..16 {
            assert_eq!(
                Mirror::FrontBack.mirror_block_rotation(rotation),
                (16 - rotation) % 16
            );
        }
    }

    #[test]
    fn none_is_identity() {
        assert_eq!(Mirror::None.mirror_facing("north"), "north");
        assert_eq!(Mirror::None.mirror_block_rotation(5), 5);
        let size = Vector3::new(4, 1, 6);
        let pos = Vector3::new(1, 0, 2);
        assert_eq!(Mirror::None.transform_pos(pos, size), pos);
    }
}

#[cfg(test)]
mod transform_tests {
    use super::*;

    fn props(pairs: &[(&'static str, &'static str)]) -> Vec<(&'static str, &'static str)> {
        pairs.to_vec()
    }

    fn get<'a>(props: &'a [(&'static str, &'static str)], key: &str) -> &'a str {
        props
            .iter()
            .find(|(k, _)| *k == key)
            .map(|(_, v)| *v)
            .unwrap_or_else(|| panic!("missing property {key}"))
    }

    // HorizontalDirectionalBlock.java:21-23
    #[test]
    fn facing_rotates_clockwise() {
        let mut p = props(&[("facing", "north"), ("half", "bottom")]);
        Rotation::Clockwise90.apply_to_props("oak_stairs", &mut p);
        assert_eq!(get(&p, "facing"), "east");
        assert_eq!(get(&p, "half"), "bottom");
    }

    // Rotation.rotate(Direction) returns Y-axis directions unchanged (Rotation.java:88-99).
    #[test]
    fn vertical_facing_is_rotation_invariant() {
        let mut p = props(&[("facing", "down")]);
        Rotation::Clockwise90.apply_to_props("hopper", &mut p);
        assert_eq!(get(&p, "facing"), "down");
    }

    // RotatedPillarBlock.rotatePillar, RotatedPillarBlock.java:31-45
    #[test]
    fn pillar_axis_swaps_on_quarter_turns_only() {
        for rotation in [Rotation::Clockwise90, Rotation::CounterClockwise90] {
            let mut p = props(&[("axis", "x")]);
            rotation.apply_to_props("oak_log", &mut p);
            assert_eq!(get(&p, "axis"), "z");
        }
        let mut p = props(&[("axis", "x")]);
        Rotation::Rotate180.apply_to_props("oak_log", &mut p);
        assert_eq!(get(&p, "axis"), "x");
        let mut p = props(&[("axis", "y")]);
        Rotation::Clockwise90.apply_to_props("oak_log", &mut p);
        assert_eq!(get(&p, "axis"), "y");
    }

    // Mirror.java: INVERT_X / INVERT_Z never swap axes.
    #[test]
    fn mirror_leaves_axis_alone() {
        let mut p = props(&[("axis", "x")]);
        Mirror::FrontBack.apply_to_props("oak_log", &mut p);
        assert_eq!(get(&p, "axis"), "x");
    }

    // StandingSignBlock.java:72-78, Rotation.rotate(int, 16) / Mirror.mirror(int, 16).
    #[test]
    fn sign_rotation_16_steps() {
        let mut p = props(&[("rotation", "15")]);
        Rotation::Clockwise90.apply_to_props("oak_sign", &mut p);
        assert_eq!(get(&p, "rotation"), "3");

        let mut p = props(&[("rotation", "3")]);
        Mirror::LeftRight.apply_to_props("oak_sign", &mut p);
        assert_eq!(get(&p, "rotation"), "5");
    }

    // CrossCollisionBlock.java:96-112: CLOCKWISE_90 sets NORTH from WEST, EAST from
    // NORTH, SOUTH from EAST, WEST from SOUTH.
    #[test]
    fn cross_collision_permutes_sides_clockwise() {
        let mut p = props(&[
            ("north", "true"),
            ("east", "false"),
            ("south", "false"),
            ("west", "false"),
        ]);
        Rotation::Clockwise90.apply_to_props("oak_fence", &mut p);
        assert_eq!(get(&p, "east"), "true");
        assert_eq!(get(&p, "north"), "false");
        assert_eq!(get(&p, "south"), "false");
        assert_eq!(get(&p, "west"), "false");
    }

    // WallBlock.java:290-299: LEFT_RIGHT swaps NORTH and SOUTH, values untouched.
    #[test]
    fn wall_sides_mirror_and_keep_their_values() {
        let mut p = props(&[
            ("north", "tall"),
            ("east", "low"),
            ("south", "none"),
            ("west", "none"),
        ]);
        Mirror::LeftRight.apply_to_props("cobblestone_wall", &mut p);
        assert_eq!(get(&p, "south"), "tall");
        assert_eq!(get(&p, "north"), "none");
        assert_eq!(get(&p, "east"), "low");
    }

    // FireBlock.java:30 and ChorusPlantBlock.java:18 declare no rotate/mirror override.
    #[test]
    fn exempt_blocks_keep_their_sides() {
        for name in ["fire", "chorus_plant"] {
            let mut p = props(&[
                ("north", "true"),
                ("east", "false"),
                ("south", "false"),
                ("west", "false"),
            ]);
            Rotation::Clockwise90.apply_to_props(name, &mut p);
            assert_eq!(get(&p, "north"), "true");
            assert_eq!(get(&p, "east"), "false");
        }
    }

    // DoorBlock.java:257-259: the hinge is cycled for any non-NONE mirror, including
    // the case where the facing lies off the mirrored axis and does not move.
    #[test]
    fn door_hinge_flips_even_when_facing_does_not() {
        let mut p = props(&[("facing", "east"), ("hinge", "left"), ("half", "lower")]);
        Mirror::LeftRight.apply_to_props("oak_door", &mut p);
        assert_eq!(get(&p, "facing"), "east");
        assert_eq!(get(&p, "hinge"), "right");
    }

    #[test]
    fn door_hinge_untouched_by_rotation() {
        let mut p = props(&[("facing", "east"), ("hinge", "left")]);
        Rotation::Clockwise90.apply_to_props("oak_door", &mut p);
        assert_eq!(get(&p, "facing"), "south");
        assert_eq!(get(&p, "hinge"), "left");
    }

    // BaseRailBlock.java:146-228, full CLOCKWISE_90 table.
    #[test]
    fn rail_shape_rotates_clockwise() {
        let expected = [
            ("north_south", "east_west"),
            ("east_west", "north_south"),
            ("ascending_east", "ascending_south"),
            ("ascending_west", "ascending_north"),
            ("ascending_north", "ascending_east"),
            ("ascending_south", "ascending_west"),
            ("south_east", "south_west"),
            ("south_west", "north_west"),
            ("north_west", "north_east"),
            ("north_east", "south_east"),
        ];
        for (input, want) in expected {
            let mut p = props(&[("shape", input)]);
            Rotation::Clockwise90.apply_to_props("rail", &mut p);
            assert_eq!(get(&p, "shape"), want, "rotating {input}");
        }
    }

    // BaseRailBlock.java:230-276.
    #[test]
    fn rail_shape_mirrors() {
        let left_right = [
            ("ascending_north", "ascending_south"),
            ("ascending_south", "ascending_north"),
            ("ascending_east", "ascending_east"),
            ("north_south", "north_south"),
            ("south_east", "north_east"),
            ("south_west", "north_west"),
            ("north_west", "south_west"),
            ("north_east", "south_east"),
        ];
        for (input, want) in left_right {
            let mut p = props(&[("shape", input)]);
            Mirror::LeftRight.apply_to_props("rail", &mut p);
            assert_eq!(get(&p, "shape"), want, "left_right on {input}");
        }
        let front_back = [
            ("ascending_east", "ascending_west"),
            ("ascending_west", "ascending_east"),
            ("ascending_north", "ascending_north"),
            ("south_east", "south_west"),
            ("north_east", "north_west"),
        ];
        for (input, want) in front_back {
            let mut p = props(&[("shape", input)]);
            Mirror::FrontBack.apply_to_props("rail", &mut p);
            assert_eq!(get(&p, "shape"), want, "front_back on {input}");
        }
    }

    // StairBlock.java:169-171: rotation never touches the shape.
    #[test]
    fn stairs_shape_is_rotation_invariant() {
        let mut p = props(&[("facing", "north"), ("shape", "inner_left")]);
        Rotation::Clockwise90.apply_to_props("oak_stairs", &mut p);
        assert_eq!(get(&p, "facing"), "east");
        assert_eq!(get(&p, "shape"), "inner_left");
    }

    // StairBlock.java:174-212. LEFT_RIGHT swaps inner and outer corners, but only when
    // the facing is on the Z axis.
    #[test]
    fn stairs_left_right_mirror() {
        let mut p = props(&[("facing", "north"), ("shape", "inner_left")]);
        Mirror::LeftRight.apply_to_props("oak_stairs", &mut p);
        assert_eq!(get(&p, "facing"), "south");
        assert_eq!(get(&p, "shape"), "inner_right");

        let mut p = props(&[("facing", "east"), ("shape", "inner_left")]);
        Mirror::LeftRight.apply_to_props("oak_stairs", &mut p);
        assert_eq!(get(&p, "facing"), "east");
        assert_eq!(get(&p, "shape"), "inner_left");
    }

    // StairBlock.java:194-208: FRONT_BACK swaps only the outer corners; INNER_LEFT and
    // INNER_RIGHT are deliberately mapped to themselves.
    #[test]
    fn stairs_front_back_mirror_keeps_inner_corners() {
        let mut p = props(&[("facing", "east"), ("shape", "inner_left")]);
        Mirror::FrontBack.apply_to_props("oak_stairs", &mut p);
        assert_eq!(get(&p, "facing"), "west");
        assert_eq!(get(&p, "shape"), "inner_left");

        let mut p = props(&[("facing", "east"), ("shape", "outer_left")]);
        Mirror::FrontBack.apply_to_props("oak_stairs", &mut p);
        assert_eq!(get(&p, "facing"), "west");
        assert_eq!(get(&p, "shape"), "outer_right");
    }

    // JigsawBlock.java:41-48, CrafterBlock.java:234-241.
    #[test]
    fn orientation_rotates_front_and_top() {
        let mut p = props(&[("orientation", "down_east")]);
        Rotation::Clockwise90.apply_to_props("jigsaw", &mut p);
        assert_eq!(get(&p, "orientation"), "down_south");

        let mut p = props(&[("orientation", "north_up")]);
        Rotation::Clockwise90.apply_to_props("jigsaw", &mut p);
        assert_eq!(get(&p, "orientation"), "east_up");

        let mut p = props(&[("orientation", "north_up")]);
        Mirror::LeftRight.apply_to_props("jigsaw", &mut p);
        assert_eq!(get(&p, "orientation"), "south_up");

        let mut p = props(&[("orientation", "down_east")]);
        Mirror::LeftRight.apply_to_props("jigsaw", &mut p);
        assert_eq!(get(&p, "orientation"), "down_east");
    }

    #[test]
    fn identity_transforms_change_nothing() {
        let original = props(&[
            ("facing", "north"),
            ("shape", "inner_left"),
            ("hinge", "left"),
        ]);
        let mut p = original.clone();
        Rotation::None.apply_to_props("oak_stairs", &mut p);
        Mirror::None.apply_to_props("oak_stairs", &mut p);
        assert_eq!(p, original);
    }
}
