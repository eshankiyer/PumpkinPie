use crate::math::{vector2::Vector2, vector3::Axis};

use super::{position::BlockPos, vector3::Vector3};

/// Represents an axis-aligned bounding box in 3D space.
#[derive(Clone, Copy, Debug)]
pub struct BoundingBox {
    /// The minimum corner of the box.
    pub min: Vector3<f64>,
    /// The maximum corner of the box.
    pub max: Vector3<f64>,
}

/// Represents a 2D bounding plane used for collision checks.
#[derive(Clone, Copy, Debug)]
struct BoundingPlane {
    /// The minimum corner of the plane.
    pub min: Vector2<f64>,
    /// The maximum corner of the plane.
    pub max: Vector2<f64>,
}

impl BoundingPlane {
    /// Checks whether this plane intersects another plane.
    ///
    /// # Arguments
    /// * `other` – The other bounding plane to check against.
    pub fn intersects(&self, other: &Self) -> bool {
        self.min.x < other.max.x
            && self.max.x > other.min.x
            && self.min.y < other.max.y
            && self.max.y > other.min.y
    }

    /// Projects a 3D bounding box onto a 2D plane by excluding one axis.
    ///
    /// # Arguments
    /// * `bounding_box` – The 3D bounding box to project.
    /// * `excluded` – The axis to exclude from the projection.
    pub const fn from_box(bounding_box: &BoundingBox, excluded: Axis) -> Self {
        let [axis1, axis2] = Axis::excluding(excluded);

        Self {
            min: Vector2::new(
                bounding_box.get_side(false).get_axis(axis1),
                bounding_box.get_side(false).get_axis(axis2),
            ),

            max: Vector2::new(
                bounding_box.get_side(true).get_axis(axis1),
                bounding_box.get_side(true).get_axis(axis2),
            ),
        }
    }
}

impl BoundingBox {
    /// Creates a default bounding box at the origin using entity dimensions.
    ///
    /// # Arguments
    /// * `size` – Dimensions of the entity.
    #[must_use]
    pub fn new_default(size: &EntityDimensions) -> Self {
        Self::new_from_pos(0., 0., 0., size)
    }

    /// Creates a bounding box from a position and entity dimension.
    ///
    /// # Arguments
    /// * `x` – X coordinate of the position.
    /// * `y` – Y coordinate of the position.
    /// * `z` – Z coordinate of the position.
    /// * `size` – Dimensions of the entity.
    #[must_use]
    pub fn new_from_pos(x: f64, y: f64, z: f64, size: &EntityDimensions) -> Self {
        let f = f64::from(size.width) / 2.;
        Self {
            min: Vector3::new(x - f, y, z - f),
            max: Vector3::new(x + f, y + f64::from(size.height), z + f),
        }
    }

    /// Expands this box by given amounts along each axis.
    ///
    /// # Arguments
    /// * `x` – Amount to expand along the X axis.
    /// * `y` – Amount to expand along the Y axis.
    /// * `z` – Amount to expand along the Z axis.
    #[must_use]
    pub fn expand(&self, x: f64, y: f64, z: f64) -> Self {
        Self {
            min: Vector3::new(self.min.x - x, self.min.y - y, self.min.z - z),
            max: Vector3::new(self.max.x + x, self.max.y + y, self.max.z + z),
        }
    }

    /// Expands this bounding box towards a specific direction.
    ///
    /// If a provided value is negative, it extends the minimum boundary along that axis.
    /// If a provided value is positive, it extends the maximum boundary along that axis.
    ///
    /// # Arguments
    /// * `x` – Amount to expand towards on the X axis.
    /// * `y` – Amount to expand towards on the Y axis.
    /// * `z` – Amount to expand towards on the Z axis.
    #[must_use]
    pub fn expand_towards(&self, x: f64, y: f64, z: f64) -> Self {
        let mut min_x = self.min.x;
        let mut min_y = self.min.y;
        let mut min_z = self.min.z;

        let mut max_x = self.max.x;
        let mut max_y = self.max.y;
        let mut max_z = self.max.z;

        if x < 0.0 {
            min_x += x;
        } else if x > 0.0 {
            max_x += x;
        }

        if y < 0.0 {
            min_y += y;
        } else if y > 0.0 {
            max_y += y;
        }

        if z < 0.0 {
            min_z += z;
        } else if z > 0.0 {
            max_z += z;
        }

        Self {
            min: Vector3::new(min_x, min_y, min_z),
            max: Vector3::new(max_x, max_y, max_z),
        }
    }

    /// Expands this box uniformly along all axes.
    ///
    /// # Arguments
    /// * `value` – Amount to expand along all axes.
    #[must_use]
    pub fn expand_all(&self, value: f64) -> Self {
        self.expand(value, value, value)
    }

    /// Contracts this box uniformly along all axes.
    ///
    /// # Arguments
    /// * `value` – Amount to contract along all axes.
    #[must_use]
    pub fn contract_all(&self, value: f64) -> Self {
        self.expand_all(-value)
    }

    /// Returns a new bounding box shifted to a specific block position.
    ///
    /// # Arguments
    /// * `pos` – Block position to move the box to.
    #[must_use]
    pub fn at_pos(&self, pos: BlockPos) -> Self {
        let vec3 = Vector3 {
            x: f64::from(pos.0.x),
            y: f64::from(pos.0.y),
            z: f64::from(pos.0.z),
        };
        Self {
            min: self.min + vec3,
            max: self.max + vec3,
        }
    }

    /// Returns a new bounding box offset by another bounding box.
    ///
    /// # Arguments
    /// * `other` – The bounding box to add as an offset.
    #[must_use]
    pub fn offset(&self, other: Self) -> Self {
        Self {
            min: self.min.add(&other.min),
            max: self.max.add(&other.max),
        }
    }

    /// Creates a bounding box from explicit min and max coordinates.
    ///
    /// # Arguments
    /// * `min` – Minimum corner of the box.
    /// * `max` – Maximum corner of the box.
    #[must_use]
    pub const fn new(min: Vector3<f64>, max: Vector3<f64>) -> Self {
        Self { min, max }
    }

    /// Creates a bounding box from arrays of min and max coordinates.
    ///
    /// # Arguments
    /// * `min` – Minimum corner as an array [x, y, z].
    /// * `max` – Maximum corner as an array [x, y, z].
    #[must_use]
    pub const fn new_array(min: [f64; 3], max: [f64; 3]) -> Self {
        Self {
            min: Vector3::new(min[0], min[1], min[2]),
            max: Vector3::new(max[0], max[1], max[2]),
        }
    }

    /// Returns a bounding box representing a full block from (0,0,0) to (1,1,1).
    #[must_use]
    pub const fn full_block() -> Self {
        Self {
            min: Vector3::new(0f64, 0f64, 0f64),
            max: Vector3::new(1f64, 1f64, 1f64),
        }
    }

    /// Creates a bounding box from a block position covering a full block.
    ///
    /// # Arguments
    /// * `position` – Block position to base the bounding box on.
    #[must_use]
    pub fn from_block(position: &BlockPos) -> Self {
        let position = position.0;
        Self {
            min: Vector3::new(
                f64::from(position.x),
                f64::from(position.y),
                f64::from(position.z),
            ),
            max: Vector3::new(
                f64::from(position.x) + 1.0,
                f64::from(position.y) + 1.0,
                f64::from(position.z) + 1.0,
            ),
        }
    }

    /// Returns the min or max side of the bounding box.
    ///
    /// # Arguments
    /// * `max` – Whether to return the max side (true) or min side (false).
    #[must_use]
    pub const fn get_side(&self, max: bool) -> Vector3<f64> {
        if max { self.max } else { self.min }
    }

    /// Calculates the collision time with another bounding box along a movement vector.
    ///
    /// # Arguments
    /// * `other` – The bounding box to test collision against.
    /// * `movement` – Movement vector of this box.
    /// * `axis` – Axis along which to calculate collision.
    /// * `max_time` – Maximum allowed collision time.
    ///
    /// # Returns
    /// Some(f64) if a collision occurs within `max_time`, None otherwise.
    #[must_use]
    pub fn calculate_collision_time(
        &self,
        other: &Self,
        movement: Vector3<f64>,
        axis: Axis,
        max_time: f64, // NOTE: Start with 1.0
    ) -> Option<f64> {
        let movement_on_axis = movement.get_axis(axis);

        if movement_on_axis == 0.0 {
            return None;
        }

        let move_positive = movement_on_axis.is_sign_positive();
        let self_plane_const = self.get_side(move_positive).get_axis(axis);
        let other_plane_const = other.get_side(!move_positive).get_axis(axis);
        let collision_time = (other_plane_const - self_plane_const) / movement_on_axis;

        if collision_time < 0.0 || collision_time >= max_time {
            return None;
        }

        let self_moved = self.shift(movement * collision_time);
        let self_plane_moved = BoundingPlane::from_box(&self_moved, axis);
        let other_plane = BoundingPlane::from_box(other, axis);

        if !self_plane_moved.intersects(&other_plane) {
            return None;
        }

        Some(collision_time)
    }

    /// Returns the average side length of the bounding box.
    #[must_use]
    pub fn get_average_side_length(&self) -> f64 {
        let width = self.max.x - self.min.x;
        let height = self.max.y - self.min.y;
        let depth = self.max.z - self.min.z;

        (width + height + depth) / 3.0
    }

    /// Returns the minimum block position covered by this bounding box.
    #[must_use]
    pub const fn min_block_pos(&self) -> BlockPos {
        BlockPos::floored_v(self.min)
    }

    /// Returns the maximum block position covered by this bounding box.
    #[must_use]
    pub const fn max_block_pos(&self) -> BlockPos {
        // Use a tiny epsilon and floor the max coordinates so that a box whose
        // max is exactly on a block boundary does not include the adjacent
        // block. This mirrors vanilla behaviour where max block is inclusive
        // only when the entity actually overlaps that block.
        let eps = 1e-9f64;
        BlockPos::floored_v(Vector3::new(
            self.max.x - eps,
            self.max.y - eps,
            self.max.z - eps,
        ))
    }

    /// Returns a new bounding box shifted by a delta vector.
    ///
    /// # Arguments
    /// * `delta` – Vector to shift the bounding box by.
    #[must_use]
    pub fn shift(&self, delta: Vector3<f64>) -> Self {
        Self {
            min: self.min + delta,
            max: self.max + delta,
        }
    }

    /// Stretches this bounding box along each axis by a given vector.
    ///
    /// # Arguments
    /// * `other` – Vector specifying how much to stretch along each axis.
    #[must_use]
    pub const fn stretch(&self, other: Vector3<f64>) -> Self {
        let mut new = *self;

        if other.x < 0.0 {
            new.min.x += other.x;
        } else if other.x > 0.0 {
            new.max.x += other.x;
        }

        if other.y < 0.0 {
            new.min.y += other.y;
        } else if other.y > 0.0 {
            new.max.y += other.y;
        }

        if other.z < 0.0 {
            new.min.z += other.z;
        } else if other.z > 0.0 {
            new.max.z += other.z;
        }

        new
    }

    /// Creates a bounding box from a block position with zero volume.
    ///
    /// # Arguments
    /// * `position` – Block position to base the bounding box on.
    #[must_use]
    pub fn from_block_raw(position: &BlockPos) -> Self {
        let position = position.0;
        Self {
            min: Vector3::new(
                f64::from(position.x),
                f64::from(position.y),
                f64::from(position.z),
            ),
            max: Vector3::new(
                f64::from(position.x),
                f64::from(position.y),
                f64::from(position.z),
            ),
        }
    }

    /// Checks if this bounding box intersects another bounding box.
    ///
    /// # Arguments
    /// * `other` – The other bounding box to check against.
    #[must_use]
    pub fn intersects(&self, other: &Self) -> bool {
        self.min.x < other.max.x
            && self.max.x > other.min.x
            && self.min.y < other.max.y
            && self.max.y > other.min.y
            && self.min.z < other.max.z
            && self.max.z > other.min.z
    }

    /// Computes the squared magnitude from a point to the nearest point on this bounding box.
    ///
    /// # Arguments
    /// * `pos` – The point to measure from.
    #[must_use]
    pub fn squared_magnitude(&self, pos: Vector3<f64>) -> f64 {
        let d = f64::max(f64::max(self.min.x - pos.x, pos.x - self.max.x), 0.0);
        let e = f64::max(f64::max(self.min.y - pos.y, pos.y - self.max.y), 0.0);
        let f = f64::max(f64::max(self.min.z - pos.z, pos.z - self.max.z), 0.0);

        super::squared_magnitude(d, e, f)
    }

    /// Computes the squared distance between this bounding box and another bounding box.
    ///
    /// # Arguments
    /// * `other` – The other bounding box to measure the distance to.
    #[must_use]
    pub fn squared_distance_to_box(&self, other: &Self) -> f64 {
        let d = f64::max(
            f64::max(self.min.x - other.max.x, other.min.x - self.max.x),
            0.0,
        );
        let e = f64::max(
            f64::max(self.min.y - other.max.y, other.min.y - self.max.y),
            0.0,
        );
        let f = f64::max(
            f64::max(self.min.z - other.max.z, other.min.z - self.max.z),
            0.0,
        );

        super::squared_magnitude(d, e, f)
    }
}

/// The most `PASSENGER` points any vanilla type declares (`HAPPY_GHAST`, `EntityTypes.java:488`).
const MAX_PASSENGER_POINTS: usize = 4;

/// Vanilla `EntityAttachments` (`EntityAttachments.java`): the per-kind attachment points of an
/// entity, in entity-local space. The client-only `NAME_TAG` kind is not kept.
#[derive(Clone, Copy, Debug)]
pub struct EntityAttachments {
    /// `EntityAttachment.PASSENGER` seats; only the first `passenger_len` are meaningful.
    passenger: [Vector3<f64>; MAX_PASSENGER_POINTS],
    /// How many `passenger` points are set (at least one).
    passenger_len: u8,
    /// `EntityAttachment.VEHICLE`: where this entity attaches to the seat it rides.
    vehicle: Vector3<f64>,
    /// `EntityAttachment.WARDEN_CHEST`: the sonic boom origin.
    warden_chest: Vector3<f64>,
}

impl EntityAttachments {
    /// `EntityAttachments.createDefault` (`EntityAttachments.java:19-21`): every kind uses its
    /// fallback (`EntityAttachment.java`): `PASSENGER` at the top, `VEHICLE` at the feet and
    /// `WARDEN_CHEST` at the centre.
    #[must_use]
    pub const fn fallback(width: f32, height: f32) -> Self {
        EntityAttachmentsBuilder::new().build(width, height)
    }

    /// `EntityAttachments.scale` (`EntityAttachments.java:27-37`): every point multiplied
    /// component-wise.
    #[must_use]
    pub fn scale(self, x: f32, y: f32, z: f32) -> Self {
        let (x, y, z) = (f64::from(x), f64::from(y), f64::from(z));
        Self {
            passenger: self.passenger.map(|point| point.multiply(x, y, z)),
            passenger_len: self.passenger_len,
            vehicle: self.vehicle.multiply(x, y, z),
            warden_chest: self.warden_chest.multiply(x, y, z),
        }
    }

    /// `EntityAttachments.transformPoint` (`EntityAttachments.java:78-80`) with `Vec3.yRot`
    /// (`Vec3.java:241-248`), which reads the float sine table.
    #[must_use]
    pub fn rotate_y(point: Vector3<f64>, rot_y: f32) -> Vector3<f64> {
        let radians = -rot_y * (std::f64::consts::PI / 180.0) as f32;
        let cos = f64::from(super::cos(radians));
        let sin = f64::from(super::sin(radians));
        Vector3::new(
            point.x * cos + point.z * sin,
            point.y,
            point.z * cos - point.x * sin,
        )
    }

    /// `EntityAttachments.getClamped(PASSENGER, index, rotY)` (`EntityAttachments.java:68-76`):
    /// an index past either end of the list uses the nearest seat.
    #[must_use]
    pub fn passenger_clamped(&self, index: i32, rot_y: f32) -> Vector3<f64> {
        let last = i32::from(self.passenger_len.max(1)) - 1;
        let index = index.clamp(0, last) as usize;
        Self::rotate_y(self.passenger[index], rot_y)
    }

    /// `EntityAttachments.get(VEHICLE, 0, rotY)` (`EntityAttachments.java:44-51`).
    #[must_use]
    pub fn vehicle_point(&self, rot_y: f32) -> Vector3<f64> {
        Self::rotate_y(self.vehicle, rot_y)
    }

    /// `EntityAttachments.get(WARDEN_CHEST, 0, rotY)` (`EntityAttachments.java:44-51`).
    #[must_use]
    pub fn warden_chest_point(&self, rot_y: f32) -> Vector3<f64> {
        Self::rotate_y(self.warden_chest, rot_y)
    }

    /// `EntityAttachments.getAverage(PASSENGER)` (`EntityAttachments.java:53-66`): the mean of
    /// the unrotated seats, scaled by the float `1.0F / size`.
    #[must_use]
    pub fn average_passenger(&self) -> Vector3<f64> {
        let len = usize::from(self.passenger_len.max(1));
        let mut sum = Vector3::new(0.0, 0.0, 0.0);
        for point in &self.passenger[..len] {
            sum += *point;
        }
        let factor = f64::from(1.0f32 / len as f32);
        sum.multiply(factor, factor, factor)
    }
}

/// Vanilla `EntityAttachments.Builder` (`EntityAttachments.java:82-103`): the kinds left unset
/// get their fallback from the dimensions the builder is finally applied to.
#[derive(Clone, Copy, Debug)]
pub struct EntityAttachmentsBuilder {
    passenger: [Vector3<f64>; MAX_PASSENGER_POINTS],
    passenger_len: u8,
    vehicle: Option<Vector3<f64>>,
    warden_chest: Option<Vector3<f64>>,
}

impl Default for EntityAttachmentsBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl EntityAttachmentsBuilder {
    const ZERO: Vector3<f64> = Vector3::new(0.0, 0.0, 0.0);

    /// `EntityAttachments.builder()`: nothing attached yet.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            passenger: [Self::ZERO; MAX_PASSENGER_POINTS],
            passenger_len: 0,
            vehicle: None,
            warden_chest: None,
        }
    }

    /// `attach(PASSENGER, point)`. A point past the fourth is ignored: no vanilla type has one.
    #[must_use]
    pub const fn passenger(mut self, point: Vector3<f64>) -> Self {
        if (self.passenger_len as usize) < MAX_PASSENGER_POINTS {
            self.passenger[self.passenger_len as usize] = point;
            self.passenger_len += 1;
        }
        self
    }

    /// `EntityType.Builder.passengerAttachments(float)` (`EntityType.java:516-522`): a seat at
    /// `(0, y, 0)` with the float widened.
    #[must_use]
    pub const fn passenger_y(self, y: f32) -> Self {
        self.passenger(Vector3::new(0.0, y as f64, 0.0))
    }

    /// `attach(VEHICLE, point)` (`EntityType.Builder.vehicleAttachment`, `EntityType.java:532-534`).
    #[must_use]
    pub const fn vehicle(mut self, point: Vector3<f64>) -> Self {
        self.vehicle = Some(point);
        self
    }

    /// `EntityType.Builder.ridingOffset` (`EntityType.java:536-538`): `VEHICLE` at
    /// `(0, -ridingOffset, 0)`.
    #[must_use]
    pub const fn riding_offset(self, riding_offset: f32) -> Self {
        self.vehicle(Vector3::new(0.0, -riding_offset as f64, 0.0))
    }

    /// `attach(WARDEN_CHEST, point)`.
    #[must_use]
    pub const fn warden_chest(mut self, point: Vector3<f64>) -> Self {
        self.warden_chest = Some(point);
        self
    }

    /// `EntityAttachments.Builder.build` (`EntityAttachments.java:97-103`) with the
    /// `EntityAttachment.Fallback` shapes (`EntityAttachment.java`).
    #[must_use]
    pub const fn build(self, _width: f32, height: f32) -> EntityAttachments {
        let mut passenger = self.passenger;
        let mut passenger_len = self.passenger_len;
        if passenger_len == 0 {
            passenger[0] = Vector3::new(0.0, height as f64, 0.0);
            passenger_len = 1;
        }
        EntityAttachments {
            passenger,
            passenger_len,
            vehicle: match self.vehicle {
                Some(point) => point,
                None => Self::ZERO,
            },
            warden_chest: match self.warden_chest {
                Some(point) => point,
                None => Vector3::new(0.0, height as f64 / 2.0, 0.0),
            },
        }
    }
}

/// Represents the dimensions of an entity.
#[derive(Clone, Copy, Debug)]
pub struct EntityDimensions {
    /// Width of the entity.
    pub width: f32,
    /// Height of the entity.
    pub height: f32,
    /// Eye height relative to the bottom of the entity.
    pub eye_height: f32,
    /// The entity's attachment points (`EntityDimensions.attachments`).
    pub attachments: EntityAttachments,
    /// Whether this dimension set ignores non-uniform scaling.
    pub fixed: bool,
}

impl EntityDimensions {
    /// Creates a new entity dimensions object.
    ///
    /// # Arguments
    /// * `width` – Width of the entity.
    /// * `height` – Height of the entity.
    /// * `eye_height` – Eye height of the entity.
    #[must_use]
    pub const fn new(width: f32, height: f32, eye_height: f32) -> Self {
        Self {
            width,
            height,
            eye_height,
            attachments: EntityAttachments::fallback(width, height),
            fixed: false,
        }
    }

    /// `EntityDimensions.scalable` (`EntityDimensions.java:41-43`).
    #[must_use]
    pub const fn scalable(width: f32, height: f32) -> Self {
        Self::new(width, height, height * 0.85)
    }

    /// `EntityDimensions.fixed` (`EntityDimensions.java:45-47`).
    #[must_use]
    pub const fn fixed(width: f32, height: f32) -> Self {
        Self {
            width,
            height,
            eye_height: height * 0.85,
            attachments: EntityAttachments::fallback(width, height),
            fixed: true,
        }
    }

    /// `EntityDimensions.makeBoundingBox` (`EntityDimensions.java:15-22`).
    #[must_use]
    pub fn make_bounding_box(&self, pos: Vector3<f64>) -> BoundingBox {
        BoundingBox::new_from_pos(pos.x, pos.y, pos.z, self)
    }

    /// `EntityDimensions.withEyeHeight` (`EntityDimensions.java:49-51`).
    #[must_use]
    pub const fn with_eye_height(self, eye_height: f32) -> Self {
        Self { eye_height, ..self }
    }

    /// `EntityDimensions.withAttachments` (`EntityDimensions.java:53-55`): replaces every
    /// attachment, the unset kinds falling back to this box's own size.
    #[must_use]
    pub const fn with_attachments(self, attachments: EntityAttachmentsBuilder) -> Self {
        Self {
            attachments: attachments.build(self.width, self.height),
            ..self
        }
    }

    /// `EntityDimensions.scale` (`EntityDimensions.java:27-37`).
    #[must_use]
    pub fn scale(self, width_scale: f32, height_scale: f32) -> Self {
        if self.fixed || (width_scale == 1.0 && height_scale == 1.0) {
            return self;
        }
        Self {
            width: self.width * width_scale,
            height: self.height * height_scale,
            eye_height: self.eye_height * height_scale,
            attachments: self
                .attachments
                .scale(width_scale, height_scale, width_scale),
            fixed: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{EntityAttachmentsBuilder, EntityDimensions, Vector3};

    #[test]
    fn fallback_points_follow_the_box() {
        let dims = EntityDimensions::new(0.6, 1.8, 1.62);
        let attachments = dims.attachments;
        assert_eq!(attachments.passenger_clamped(0, 0.0).y, f64::from(1.8f32));
        assert_eq!(attachments.vehicle_point(0.0), Vector3::new(0.0, 0.0, 0.0));
        assert_eq!(
            attachments.warden_chest_point(0.0).y,
            f64::from(1.8f32) / 2.0
        );
    }

    #[test]
    fn scale_multiplies_explicit_points() {
        let dims = EntityDimensions::new(1.2, 0.4, 0.34)
            .with_attachments(EntityAttachmentsBuilder::new().passenger(Vector3::new(
                0.0,
                f64::from(0.4f32),
                f64::from(-0.25f32),
            )))
            .scale(0.3, 0.3);
        let seat = dims.attachments.passenger_clamped(0, 0.0);
        assert_eq!(seat.y, f64::from(0.4f32) * f64::from(0.3f32));
        assert_eq!(seat.z, -0.25 * f64::from(0.3f32));
    }

    #[test]
    fn passenger_index_is_clamped_and_rotated() {
        let ghast = EntityDimensions::new(4.0, 4.0, 3.6).with_attachments(
            EntityAttachmentsBuilder::new()
                .passenger(Vector3::new(0.0, 4.0, 1.7))
                .passenger(Vector3::new(-1.7, 4.0, 0.0))
                .passenger(Vector3::new(0.0, 4.0, -1.7))
                .passenger(Vector3::new(1.7, 4.0, 0.0)),
        );
        let attachments = ghast.attachments;
        assert_eq!(
            attachments.passenger_clamped(7, 0.0),
            Vector3::new(1.7, 4.0, 0.0)
        );
        assert_eq!(attachments.passenger_clamped(-1, 0.0).z, 1.7);
        // yaw 90: radians -PI/2, table cos 0 and sin -1, so (0, 4, 1.7) -> (-1.7, 4, 0).
        assert_eq!(
            attachments.passenger_clamped(0, 90.0),
            Vector3::new(-1.7, 4.0, 0.0)
        );
        assert_eq!(attachments.average_passenger(), Vector3::new(0.0, 4.0, 0.0));
    }

    #[test]
    fn riding_offset_negates_into_the_vehicle_point() {
        let skeleton = EntityDimensions::new(0.6, 1.99, 1.74)
            .with_attachments(EntityAttachmentsBuilder::new().riding_offset(-0.7));
        assert_eq!(skeleton.attachments.vehicle_point(0.0).y, f64::from(0.7f32));
        assert_eq!(
            skeleton.attachments.passenger_clamped(0, 0.0).y,
            f64::from(1.99f32)
        );
    }
}
