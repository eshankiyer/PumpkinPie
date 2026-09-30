use pumpkin_codecs::codec::list::validate_fixed_size;
use pumpkin_codecs::{DataResult, FlatTryFrom, comap_flat_map_codec_impl};
use pumpkin_nbt::tag::NbtTag;
use serde::{Deserialize, Serialize};

/// Represents a 3D rotation using Euler angles in degrees.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct EulerAngle {
    /// Rotation around the X-axis in degrees.
    pub pitch: f32,
    /// Rotation around the Y-axis in degrees.
    pub yaw: f32,
    /// Rotation around the Z-axis in degrees.
    pub roll: f32,
}

impl EulerAngle {
    /// Creates a new `EulerAngle` with the given pitch, yaw, and roll in degrees.
    ///
    /// Values are normalized to the range [0, 360].
    ///
    /// # Arguments
    /// * `pitch` – Rotation around the X-axis.
    /// * `yaw` – Rotation around the Y-axis.
    /// * `roll` – Rotation around the Z-axis.
    #[must_use]
    pub fn new(pitch: f32, yaw: f32, roll: f32) -> Self {
        // Vanilla `Rotations` (`Rotations.java:34-38`) maps NaN/infinity to 0 before `% 360`.
        let normalize = |v: f32| if v.is_finite() { v % 360.0 } else { 0.0 };
        let pitch = normalize(pitch);
        let yaw = normalize(yaw);
        let roll = normalize(roll);

        Self { pitch, yaw, roll }
    }

    /// A constant representing zero rotation on all axes.
    pub const ZERO: Self = Self {
        pitch: 0.0,
        yaw: 0.0,
        roll: 0.0,
    };
}

impl Default for EulerAngle {
    fn default() -> Self {
        Self::ZERO
    }
}

impl From<EulerAngle> for NbtTag {
    fn from(val: EulerAngle) -> Self {
        Self::List(vec![
            Self::Float(val.pitch),
            Self::Float(val.yaw),
            Self::Float(val.roll),
        ])
    }
}

/// Vanilla reads each element through `NbtOps.getNumberValue` (`NbtOps.java:65-67`), which
/// accepts every numeric tag; anything else fails the codec.
fn numeric_tag_as_f32(tag: &NbtTag) -> Option<f32> {
    Some(match tag {
        NbtTag::Byte(v) => f32::from(*v),
        NbtTag::Short(v) => f32::from(*v),
        #[expect(clippy::cast_precision_loss)]
        NbtTag::Int(v) => *v as f32,
        #[expect(clippy::cast_precision_loss)]
        NbtTag::Long(v) => *v as f32,
        NbtTag::Float(v) => *v,
        #[expect(clippy::cast_possible_truncation)]
        NbtTag::Double(v) => *v as f32,
        _ => return None,
    })
}

impl EulerAngle {
    /// Strict decode matching vanilla `Rotations.CODEC`: the tag must be a list of exactly three
    /// numeric tags, otherwise `None` (callers then fall back to their own default).
    #[must_use]
    pub fn try_from_nbt(tag: &NbtTag) -> Option<Self> {
        if let NbtTag::List(list) = tag
            && list.len() == 3
        {
            return Some(Self::new(
                numeric_tag_as_f32(&list[0])?,
                numeric_tag_as_f32(&list[1])?,
                numeric_tag_as_f32(&list[2])?,
            ));
        }
        None
    }
}

impl From<NbtTag> for EulerAngle {
    fn from(tag: NbtTag) -> Self {
        Self::try_from_nbt(&tag).unwrap_or(Self::ZERO)
    }
}

impl From<&EulerAngle> for Vec<f32> {
    fn from(value: &EulerAngle) -> Self {
        let EulerAngle { pitch, yaw, roll } = value;
        vec![*pitch, *yaw, *roll]
    }
}

impl FlatTryFrom<Vec<f32>> for EulerAngle {
    fn flat_try_from(value: Vec<f32>) -> DataResult<Self> {
        validate_fixed_size(value, 3).flat_map(|v| {
            v.try_into().map_or_else(
                |_| DataResult::new_error("Expected 3 elements"),
                |arr| {
                    let [x, y, z]: [f32; 3] = arr;
                    DataResult::new_success(Self {
                        pitch: x,
                        yaw: y,
                        roll: z,
                    })
                },
            )
        })
    }
}

comap_flat_map_codec_impl!(Vec<f32> => EulerAngle, EulerAngle::flat_try_from, Vec::<f32>::from);

#[cfg(test)]
mod tests {
    use super::EulerAngle;
    use pumpkin_nbt::tag::NbtTag;

    #[test]
    fn numeric_tags_are_coerced_and_non_finite_maps_to_zero() {
        let angle = EulerAngle::from(NbtTag::List(vec![
            NbtTag::Int(10),
            NbtTag::Double(20.5),
            NbtTag::Byte(-3),
        ]));
        assert_eq!(angle, EulerAngle::new(10.0, 20.5, -3.0));
        assert_eq!(EulerAngle::new(f32::NAN, f32::INFINITY, 370.0).pitch, 0.0);
        assert_eq!(EulerAngle::new(f32::NAN, f32::INFINITY, 370.0).yaw, 0.0);
        assert_eq!(EulerAngle::new(f32::NAN, f32::INFINITY, 370.0).roll, 10.0);
    }
}
