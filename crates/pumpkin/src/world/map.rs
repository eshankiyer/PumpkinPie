use crate::entity::player::Player;
use dashmap::DashMap;
use pumpkin_data::dimension::Dimension;
use pumpkin_data::map_decoration::MapDecorationType;
use pumpkin_util::math::{position::BlockPos, vector2::Vector2};
use std::sync::Arc;
use tokio::sync::Mutex;

pub struct MapManager {
    pub maps: DashMap<i32, Arc<Mutex<MapData>>>,
}

impl Default for MapManager {
    fn default() -> Self {
        Self::new()
    }
}

impl MapManager {
    #[must_use]
    pub fn new() -> Self {
        Self {
            maps: DashMap::new(),
        }
    }

    #[must_use]
    pub fn get_map(&self, id: i32) -> Option<Arc<Mutex<MapData>>> {
        self.maps.get(&id).map(|m| m.clone())
    }

    #[must_use]
    pub fn create_map(
        &self,
        id: i32,
        dimension: Dimension,
        x: i32,
        z: i32,
        scale: i8,
    ) -> Arc<Mutex<MapData>> {
        let map = Arc::new(Mutex::new(MapData::new(dimension, x, z, scale)));
        self.maps.insert(id, map.clone());
        map
    }
}

pub struct MapData {
    pub scale: i8,
    pub locked: bool,
    pub dimension: Dimension,
    pub center_x: i32,
    pub center_z: i32,
    pub colors: Box<[u8; 128 * 128]>,
    pub decorations: Vec<MapDecoration>,
    pub dirty: bool,
    pub fully_updated: bool,
}

impl MapData {
    #[must_use]
    pub fn new(dimension: Dimension, x: i32, z: i32, scale: i8) -> Self {
        Self {
            scale,
            locked: false,
            dimension,
            center_x: x,
            center_z: z,
            colors: Box::new([0; 128 * 128]),
            decorations: Vec::new(),
            dirty: true,
            fully_updated: false,
        }
    }

    pub fn set_color(&mut self, x: usize, z: usize, color: u8) {
        if x < 128 && z < 128 {
            let idx = z * 128 + x;
            if self.colors[idx] != color {
                self.colors[idx] = color;
                self.dirty = true;
            }
        }
    }

    /// `MapItemSavedData.toggleBanner` (`MapItemSavedData.java:392-419`) stores or removes a
    /// banner marker only while the banner lies inside the map's 63-block half-size.
    pub fn toggle_banner(
        &mut self,
        position: BlockPos,
        decoration_type: &MapDecorationType,
        display_name: Option<String>,
    ) -> bool {
        let scale = 1i32 << self.scale;
        let x_pos = f64::from(position.0.x) + 0.5;
        let z_pos = f64::from(position.0.z) + 0.5;
        let x_delta = (x_pos - f64::from(self.center_x)) / f64::from(scale);
        let z_delta = (z_pos - f64::from(self.center_z)) / f64::from(scale);
        if !(-63.0..=63.0).contains(&x_delta) || !(-63.0..=63.0).contains(&z_delta) {
            return false;
        }

        let key = format!("banner-{},{},{}", position.0.x, position.0.y, position.0.z);
        if let Some(index) = self
            .decorations
            .iter()
            .position(|decoration| decoration.key == key)
        {
            if self.decorations[index].icon_type == decoration_type.id as i32
                && self.decorations[index].display_name.as_ref() == display_name.as_ref()
            {
                self.decorations.remove(index);
            } else {
                self.decorations[index] = MapDecoration {
                    key,
                    icon_type: decoration_type.id as i32,
                    x: clamp_map_coordinate(x_delta),
                    z: clamp_map_coordinate(z_delta),
                    direction: 8,
                    display_name,
                };
            }
            self.dirty = true;
            return true;
        }
        if self.decorations.len() >= 256 {
            return false;
        }

        self.decorations.push(MapDecoration {
            key,
            icon_type: decoration_type.id as i32,
            x: clamp_map_coordinate(x_delta),
            z: clamp_map_coordinate(z_delta),
            direction: 8,
            display_name,
        });
        self.dirty = true;
        true
    }

    pub fn update(&mut self, player: &Player) {
        let world = player.world();
        let scale = 1 << self.scale;
        let center_x = self.center_x;
        let center_z = self.center_z;

        let player_pos = player.position();
        let player_x = player_pos.x as i32;
        let player_z = player_pos.z as i32;

        let start_img_x = ((player_x - center_x) / scale + 64).clamp(0, 127) as usize;
        let start_img_z = ((player_z - center_z) / scale + 64).clamp(0, 127) as usize;

        let radius = 16;
        let (range_x, range_z) = if self.fully_updated {
            (
                (start_img_x.saturating_sub(radius))..(start_img_x + radius).min(128),
                (start_img_z.saturating_sub(radius))..(start_img_z + radius).min(128),
            )
        } else {
            self.fully_updated = true;
            (0..128, 0..128)
        };

        for img_x in range_x {
            let mut prev_y = -1;
            for img_z in range_z.clone() {
                let world_x = (img_x as i32 - 64) * scale + center_x;
                let world_z = (img_z as i32 - 64) * scale + center_z;

                let top_y = world.get_top_block(Vector2::new(world_x, world_z));
                let block = world.get_block(&BlockPos::new(world_x, top_y, world_z));

                let color_base = block.map_color;

                let mut brightness = 2; // Normal
                if prev_y != -1 {
                    if top_y > prev_y {
                        brightness = 3; // High
                    } else if top_y < prev_y {
                        brightness = 1; // Low
                    }
                }
                prev_y = top_y;

                let color = color_base * 4 + brightness;
                self.set_color(img_x, img_z, color);
            }
        }
    }
}

/// `MapItemSavedData.clampMapCoordinate` (`MapItemSavedData.java:355-362`) preserves the
/// protocol's asymmetric `-128..127` map-coordinate range.
fn clamp_map_coordinate(delta: f64) -> i8 {
    if delta <= -63.0 {
        -128
    } else if delta >= 63.0 {
        127
    } else {
        (delta * 2.0 + 0.5) as i8
    }
}

pub struct MapDecoration {
    /// `MapItemSavedData` keys banner decorations by its block position (`MapItemSavedData.java:405-413`).
    pub key: String,
    pub icon_type: i32,
    pub x: i8,
    pub z: i8,
    pub direction: i8,
    pub display_name: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::MapData;
    use pumpkin_data::dimension::Dimension;
    use pumpkin_data::map_decoration::MapDecorationType;
    use pumpkin_util::math::position::BlockPos;

    #[test]
    fn toggle_banner_adds_then_removes_marker() {
        let mut map = MapData::new(Dimension::OVERWORLD, 0, 0, 0);
        let position = BlockPos::new(4, 70, -3);

        assert!(map.toggle_banner(position, &MapDecorationType::BANNER_WHITE, None));
        assert_eq!(map.decorations.len(), 1);
        assert_eq!(
            map.decorations[0].icon_type,
            MapDecorationType::BANNER_WHITE.id as i32
        );

        assert!(map.toggle_banner(position, &MapDecorationType::BANNER_RED, None));
        assert_eq!(map.decorations.len(), 1);
        assert_eq!(
            map.decorations[0].icon_type,
            MapDecorationType::BANNER_RED.id as i32
        );

        assert!(map.toggle_banner(position, &MapDecorationType::BANNER_RED, None));
        assert!(map.decorations.is_empty());
    }

    #[test]
    fn toggle_banner_rejects_positions_outside_map() {
        let mut map = MapData::new(Dimension::OVERWORLD, 0, 0, 0);

        assert!(!map.toggle_banner(
            BlockPos::new(64, 70, 0),
            &MapDecorationType::BANNER_WHITE,
            None
        ));
        assert!(map.decorations.is_empty());
    }
}
