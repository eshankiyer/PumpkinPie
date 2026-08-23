use pumpkin_protocol::java::client::play::{
    CInitializeWorldBorder, CSetBorderCenter, CSetBorderLerpSize, CSetBorderSize,
    CSetBorderWarningDelay, CSetBorderWarningDistance,
};

use pumpkin_world::world_info::data_files::WorldBorderData;

use crate::net::java::JavaClient;

use super::World;

pub struct Worldborder {
    pub center_x: f64,
    pub center_z: f64,
    pub old_diameter: f64,
    pub new_diameter: f64,
    /// The actual size used for containment/damage/clamping checks, interpolated
    /// each tick from `old_diameter` toward `new_diameter` (vanilla
    /// `WorldBorder.MovingBorderExtent`). `old_diameter`/`new_diameter` stay pure
    /// lerp endpoints for the client packets, matching vanilla's `from`/`to`.
    current_diameter: f64,
    lerp_ticks_total: i64,
    lerp_ticks_remaining: i64,
    pub speed: i64,
    pub portal_teleport_boundary: i32,
    pub warning_blocks: i32,
    pub warning_time: i32,
    pub damage_per_block: f32,
    pub buffer: f32,
}

impl Worldborder {
    #[must_use]
    pub const fn new(
        x: f64,
        z: f64,
        diameter: f64,
        speed: i64,
        warning_blocks: i32,
        warning_time: i32,
    ) -> Self {
        Self {
            center_x: x,
            center_z: z,
            old_diameter: diameter,
            new_diameter: diameter,
            current_diameter: diameter,
            lerp_ticks_total: 0,
            lerp_ticks_remaining: 0,
            speed,
            portal_teleport_boundary: 29_999_984,
            warning_blocks,
            warning_time,
            damage_per_block: 0.2,
            buffer: 5.0,
        }
    }

    /// Restores a border from its persisted `WorldBorder.Settings`.
    ///
    /// Vanilla `WorldBorder.applyInitialSettings` (`WorldBorder.java:285-299`)
    /// resumes an in-flight interpolation when the stored `lerp_time` is positive
    /// and otherwise makes the border static at `size`. The stored `lerp_time` is
    /// the *remaining* tick count (`MovingBorderExtent.getLerpTime` returns
    /// `lerpProgress`, `WorldBorder.java:408-410`), so it becomes the full
    /// duration of the resumed lerp.
    #[must_use]
    pub const fn from_settings(settings: &WorldBorderData) -> Self {
        let lerping = settings.lerp_time > 0;
        Self {
            center_x: settings.center_x,
            center_z: settings.center_z,
            old_diameter: settings.size,
            new_diameter: if lerping {
                settings.lerp_target
            } else {
                settings.size
            },
            current_diameter: settings.size,
            lerp_ticks_total: if lerping { settings.lerp_time } else { 0 },
            lerp_ticks_remaining: if lerping { settings.lerp_time } else { 0 },
            speed: settings.lerp_time,
            portal_teleport_boundary: 29_999_984,
            warning_blocks: settings.warning_blocks,
            warning_time: settings.warning_time,
            damage_per_block: settings.damage_per_block as f32,
            buffer: settings.safe_zone as f32,
        }
    }

    /// Snapshots the border for persistence.
    ///
    /// Mirrors the `WorldBorder.Settings(WorldBorder)` constructor
    /// (`WorldBorder.java:475-487`), which stores the *current* size along with the
    /// remaining lerp time and its target.
    #[must_use]
    pub fn to_settings(&self) -> WorldBorderData {
        WorldBorderData {
            center_x: self.center_x,
            center_z: self.center_z,
            damage_per_block: f64::from(self.damage_per_block),
            safe_zone: f64::from(self.buffer),
            warning_blocks: self.warning_blocks,
            warning_time: self.warning_time,
            size: self.current_diameter,
            lerp_time: self.lerp_ticks_remaining,
            lerp_target: self.new_diameter,
        }
    }

    /// Vanilla `WorldBorder.getSize()`: the current (interpolated) diameter.
    #[must_use]
    pub const fn size(&self) -> f64 {
        self.current_diameter
    }

    /// Vanilla `WorldBorder.getLerpTime()`: ticks left in the active interpolation.
    #[must_use]
    pub const fn lerp_time(&self) -> i64 {
        self.lerp_ticks_remaining
    }

    pub async fn init_client(&self, client: &JavaClient) {
        client
            .enqueue_client_packet(&CInitializeWorldBorder::new(
                self.center_x,
                self.center_z,
                self.current_diameter,
                self.new_diameter,
                self.lerp_ticks_remaining.into(),
                self.portal_teleport_boundary.into(),
                self.warning_blocks.into(),
                self.warning_time.into(),
            ))
            .await;
    }

    pub fn set_center(&mut self, world: &World, x: f64, z: f64) {
        self.center_x = x;
        self.center_z = z;

        world.broadcast_packet_all(&CSetBorderCenter::new(self.center_x, self.center_z));
    }

    pub fn set_diameter(&mut self, world: &World, diameter: f64, speed: Option<i64>) {
        // Vanilla `WorldBorderCommand.setSize` lerps from `border.getSize()`, the
        // current interpolated size, not from the previous lerp target.
        self.old_diameter = self.current_diameter;
        self.new_diameter = diameter;

        // A zero (or negative) tick duration has nothing to interpolate over --
        // vanilla's own `calculateSize` degenerates to `to` immediately in that case
        // (`(duration - progress) / duration` is NaN, which fails the `< 1.0` check).
        if let Some(ticks) = speed.filter(|ticks| *ticks > 0) {
            self.lerp_ticks_total = ticks;
            self.lerp_ticks_remaining = ticks;
            self.current_diameter = self.old_diameter;
            world.broadcast_packet_all(&CSetBorderLerpSize::new(
                self.old_diameter,
                self.new_diameter,
                ticks.into(),
            ));
        } else {
            self.lerp_ticks_total = 0;
            self.lerp_ticks_remaining = 0;
            self.current_diameter = self.new_diameter;
            if speed.is_some() {
                world.broadcast_packet_all(&CSetBorderLerpSize::new(
                    self.old_diameter,
                    self.new_diameter,
                    0i64.into(),
                ));
            } else {
                world.broadcast_packet_all(&CSetBorderSize::new(self.new_diameter));
            }
        }
    }

    pub fn add_diameter(&mut self, world: &World, offset: f64, speed: Option<i64>) {
        self.set_diameter(world, self.current_diameter + offset, speed);
    }

    /// Per-tick lerp update, mirroring vanilla `WorldBorder.MovingBorderExtent::update`.
    /// A no-op once the lerp has completed (`lerp_ticks_remaining == 0`).
    pub fn tick(&mut self, _world: &World) {
        if self.lerp_ticks_remaining > 0 {
            self.lerp_ticks_remaining -= 1;
            self.current_diameter = if self.lerp_ticks_remaining > 0 {
                let progress = (self.lerp_ticks_total - self.lerp_ticks_remaining) as f64
                    / self.lerp_ticks_total as f64;
                self.old_diameter + (self.new_diameter - self.old_diameter) * progress
            } else {
                self.new_diameter
            };
        }
    }

    pub fn set_warning_delay(&mut self, world: &World, delay: i32) {
        self.warning_time = delay;

        world.broadcast_packet_all(&CSetBorderWarningDelay::new(self.warning_time.into()));
    }

    pub fn set_warning_distance(&mut self, world: &World, distance: i32) {
        self.warning_blocks = distance;

        world.broadcast_packet_all(&CSetBorderWarningDistance::new(self.warning_blocks.into()));
    }

    /// `(min_x, max_x, min_z, max_z)`, matching vanilla's `BorderExtent` getters:
    /// the half-diameter offsets from the center, each clamped to
    /// `±absoluteMaxSize` (`WorldBorder.StaticBorderExtent.updateBox`).
    fn bounds(&self) -> (f64, f64, f64, f64) {
        let half = self.current_diameter / 2.0;
        let limit = f64::from(self.portal_teleport_boundary);
        (
            clamp(self.center_x - half, -limit, limit),
            clamp(self.center_x + half, -limit, limit),
            clamp(self.center_z - half, -limit, limit),
            clamp(self.center_z + half, -limit, limit),
        )
    }

    #[must_use]
    pub fn contains(&self, x: f64, z: f64) -> bool {
        self.contains_with_margin(x, z, 0.0)
    }

    /// Vanilla `WorldBorder.isWithinBounds(double, double, double)`
    /// (`WorldBorder.java:72-74`): bounds widened outward by `margin` on all four sides.
    #[must_use]
    pub fn contains_with_margin(&self, x: f64, z: f64, margin: f64) -> bool {
        let (min_x, max_x, min_z, max_z) = self.bounds();
        x >= min_x - margin && x < max_x + margin && z >= min_z - margin && z < max_z + margin
    }

    /// Vanilla `WorldBorder.isWithinBounds(BlockPos)`, which tests the single
    /// `(getX(), getZ())` pair. The two-corner form belongs to the `ChunkPos`
    /// overload.
    #[must_use]
    pub fn contains_block(&self, x: i32, z: i32) -> bool {
        self.contains(f64::from(x), f64::from(z))
    }

    /// Signed distance from `(x, z)` to the nearest border edge; negative when outside.
    #[must_use]
    pub fn distance_to_border(&self, x: f64, z: f64) -> f64 {
        let (min_x, max_x, min_z, max_z) = self.bounds();

        let from_west = x - min_x;
        let from_east = max_x - x;
        let from_north = z - min_z;
        let from_south = max_z - z;

        from_west.min(from_east).min(from_north).min(from_south)
    }

    /// Vanilla `WorldBorder.clampToBounds`: clamp into
    /// `[min, max - 1.0E-5]` on each axis, then `BlockPos.containing` (floor).
    #[must_use]
    pub fn clamp_block(&self, x: i32, z: i32) -> (i32, i32) {
        let (min_x, max_x, min_z, max_z) = self.bounds();
        (
            clamp(f64::from(x), min_x, max_x - 1.0e-5).floor() as i32,
            clamp(f64::from(z), min_z, max_z - 1.0e-5).floor() as i32,
        )
    }

    /// Vanilla `WorldBorder.getLerpSpeed()`: the current interpolation speed in blocks per tick.
    ///
    /// Vanilla `WorldBorder.MovingBorderExtent.getLerpSpeed` (`WorldBorder.java:403-405`)
    /// returns `Math.abs(this.from - this.to) / (this.lerpEnd - this.lerpBegin)`.
    /// When not lerping, the speed is 0.
    #[must_use]
    pub const fn lerp_speed(&self) -> f64 {
        if self.lerp_ticks_total > 0 {
            (self.old_diameter - self.new_diameter).abs() / self.lerp_ticks_total as f64
        } else {
            0.0
        }
    }

    /// Vanilla `WorldBorder.isInsideCloseToBorder` (`WorldBorder.java:114-117`).
    ///
    /// Checks if an entity's bounding box is close enough to the border to take damage.
    /// The threshold is `max(|boundingBox.xSize|, |boundingBox.zSize|, 1.0) * 2.0`.
    #[must_use]
    pub fn is_inside_close_to_border(
        &self,
        x: f64,
        z: f64,
        bbox_xsize: f64,
        bbox_zsize: f64,
    ) -> bool {
        let distance = self.distance_to_border(x, z);
        let bb_max = bbox_xsize.abs().max(bbox_zsize.abs()).max(1.0);
        distance < bb_max * 2.0 && self.contains_with_margin(x, z, bb_max)
    }

    /// Vanilla `WorldBorder.setAbsoluteMaxSize` (`WorldBorder.java:216-219`).
    ///
    /// Sets the absolute maximum size (clamping boundary) for the border.
    /// Vanilla default is 29999984.
    pub const fn set_absolute_max_size(&mut self, size: i32) {
        self.portal_teleport_boundary = size;
    }

    /// Vanilla `WorldBorder.getAbsoluteMaxSize` (`WorldBorder.java:221-223`).
    ///
    /// Returns the absolute maximum size (clamping boundary) for the border.
    #[must_use]
    pub const fn absolute_max_size(&self) -> i32 {
        self.portal_teleport_boundary
    }
}

/// `Mth.clamp`: the lower bound wins when the range is inverted, unlike
/// `f64::clamp`, which panics.
fn clamp(value: f64, min: f64, max: f64) -> f64 {
    if value < min {
        min
    } else if value > max {
        max
    } else {
        value
    }
}

#[cfg(test)]
mod tests {
    use super::Worldborder;

    fn border(center: f64, diameter: f64) -> Worldborder {
        Worldborder::new(center, center, diameter, 0, 5, 300)
    }

    #[test]
    fn contains_block_tests_a_single_point() {
        let border = border(0.0, 16.0);
        // The border spans [-8, 8); block 7 is the last one inside.
        assert!(border.contains_block(7, 7));
        assert!(!border.contains_block(8, 8));
        assert!(border.contains_block(-8, -8));
    }

    #[test]
    fn clamp_block_matches_clamp_to_bounds() {
        let border = border(0.0, 16.0);
        assert_eq!(border.clamp_block(3, -3), (3, -3));
        // floor(clamp(100, -8, 8 - 1e-5)) == 7
        assert_eq!(border.clamp_block(100, 100), (7, 7));
        assert_eq!(border.clamp_block(-100, -100), (-8, -8));
    }

    #[test]
    fn bounds_are_clamped_to_the_absolute_max_size() {
        let border = border(2.0e7, 3.0e7);
        // max = 2e7 + 1.5e7 = 3.5e7, clamped down to 29_999_984.
        assert!(!border.contains(3.0e7, 2.0e7));
        assert!(border.contains(2.999_998_3E7, 2.0e7));
    }

    /// `WorldBorder.applyInitialSettings` (`WorldBorder.java:285-299`) resumes an
    /// in-flight lerp: the saved `size` is where the border is *now* and
    /// `lerp_time` is what is left of the interpolation.
    #[test]
    fn settings_round_trip_preserves_an_in_flight_lerp() {
        let mut border = Worldborder::new(1.0, -2.0, 100.0, 0, 7, 30);
        border.old_diameter = 100.0;
        border.new_diameter = 20.0;
        border.lerp_ticks_total = 200;
        border.lerp_ticks_remaining = 120;
        border.damage_per_block = 0.35;
        border.buffer = 4.0;

        let settings = border.to_settings();
        assert_eq!(settings.size, 100.0);
        assert_eq!(settings.lerp_target, 20.0);
        assert_eq!(settings.lerp_time, 120);
        assert_eq!(settings.warning_blocks, 7);
        assert_eq!(settings.warning_time, 30);

        let restored = Worldborder::from_settings(&settings);
        assert_eq!(restored.size(), 100.0);
        assert_eq!(restored.new_diameter, 20.0);
        assert_eq!(restored.lerp_time(), 120);
        assert_eq!(restored.center_x, 1.0);
        assert_eq!(restored.center_z, -2.0);
        assert!((restored.damage_per_block - 0.35).abs() < 1.0e-6);
        assert!((restored.buffer - 4.0).abs() < 1.0e-6);
    }

    /// A finished (or never started) lerp saves as `lerp_time == 0` and reloads
    /// static, vanilla `setSize` rather than `lerpSizeBetween`.
    #[test]
    fn settings_round_trip_of_a_static_border_stays_static() {
        let border = Worldborder::new(0.0, 0.0, 512.0, 0, 5, 300);
        let settings = border.to_settings();
        assert_eq!(settings.lerp_time, 0);
        assert_eq!(settings.lerp_target, 512.0);

        let restored = Worldborder::from_settings(&settings);
        assert_eq!(restored.size(), 512.0);
        assert_eq!(restored.lerp_time(), 0);
        assert_eq!(restored.old_diameter, 512.0);
        assert_eq!(restored.new_diameter, 512.0);
    }
}
