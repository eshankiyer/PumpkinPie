use core::f32;
use std::sync::atomic::Ordering;

use crate::entity::{
    Entity, EntityBase, EntityBaseFuture, NBTStorage, NbtFuture, living::LivingEntity,
};
use arc_swap::ArcSwap;
use crossbeam::atomic::AtomicCell;
use pumpkin_data::BlockDirection;
use pumpkin_data::damage::DamageType;
use pumpkin_data::tracked_data;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_nbt::tag::NbtTag;
use pumpkin_protocol::codec::var_int::VarInt;
use pumpkin_protocol::java::client::play::{CSpawnEntity, Metadata};
use pumpkin_util::math::boundingbox::BoundingBox;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;

/// Painting variant information (width and height in quads / 16px units)
/// Based on vanilla PaintingVariants.java (MC 1.26.2)
pub struct PaintingVariantInfo {
    pub name: &'static str,
    pub width_quads: i32,
    pub height_quads: i32,
}

/// All vanilla painting variants with their dimensions
/// Source: net.minecraft.world.entity.decoration.painting.PaintingVariants (MC 1.26.2)
pub const PAINTING_VARIANTS: &[PaintingVariantInfo] = &[
    PaintingVariantInfo {
        name: "minecraft:kebab",
        width_quads: 1,
        height_quads: 1,
    },
    PaintingVariantInfo {
        name: "minecraft:aztec",
        width_quads: 1,
        height_quads: 1,
    },
    PaintingVariantInfo {
        name: "minecraft:alban",
        width_quads: 1,
        height_quads: 1,
    },
    PaintingVariantInfo {
        name: "minecraft:aztec2",
        width_quads: 1,
        height_quads: 1,
    },
    PaintingVariantInfo {
        name: "minecraft:bomb",
        width_quads: 1,
        height_quads: 1,
    },
    PaintingVariantInfo {
        name: "minecraft:plant",
        width_quads: 1,
        height_quads: 1,
    },
    PaintingVariantInfo {
        name: "minecraft:wasteland",
        width_quads: 1,
        height_quads: 1,
    },
    PaintingVariantInfo {
        name: "minecraft:pool",
        width_quads: 2,
        height_quads: 1,
    },
    PaintingVariantInfo {
        name: "minecraft:courbet",
        width_quads: 2,
        height_quads: 1,
    },
    PaintingVariantInfo {
        name: "minecraft:sea",
        width_quads: 2,
        height_quads: 1,
    },
    PaintingVariantInfo {
        name: "minecraft:sunset",
        width_quads: 2,
        height_quads: 1,
    },
    PaintingVariantInfo {
        name: "minecraft:creebet",
        width_quads: 2,
        height_quads: 1,
    },
    PaintingVariantInfo {
        name: "minecraft:wanderer",
        width_quads: 1,
        height_quads: 2,
    },
    PaintingVariantInfo {
        name: "minecraft:graham",
        width_quads: 1,
        height_quads: 2,
    },
    PaintingVariantInfo {
        name: "minecraft:match",
        width_quads: 2,
        height_quads: 2,
    },
    PaintingVariantInfo {
        name: "minecraft:bust",
        width_quads: 2,
        height_quads: 2,
    },
    PaintingVariantInfo {
        name: "minecraft:stage",
        width_quads: 2,
        height_quads: 2,
    },
    PaintingVariantInfo {
        name: "minecraft:void",
        width_quads: 2,
        height_quads: 2,
    },
    PaintingVariantInfo {
        name: "minecraft:skull_and_roses",
        width_quads: 2,
        height_quads: 2,
    },
    PaintingVariantInfo {
        name: "minecraft:wither",
        width_quads: 2,
        height_quads: 2,
    },
    PaintingVariantInfo {
        name: "minecraft:fighters",
        width_quads: 4,
        height_quads: 2,
    },
    PaintingVariantInfo {
        name: "minecraft:pointer",
        width_quads: 4,
        height_quads: 4,
    },
    PaintingVariantInfo {
        name: "minecraft:pigscene",
        width_quads: 4,
        height_quads: 4,
    },
    PaintingVariantInfo {
        name: "minecraft:burning_skull",
        width_quads: 4,
        height_quads: 4,
    },
    PaintingVariantInfo {
        name: "minecraft:skeleton",
        width_quads: 4,
        height_quads: 3,
    },
    PaintingVariantInfo {
        name: "minecraft:earth",
        width_quads: 2,
        height_quads: 2,
    },
    PaintingVariantInfo {
        name: "minecraft:wind",
        width_quads: 2,
        height_quads: 2,
    },
    PaintingVariantInfo {
        name: "minecraft:water",
        width_quads: 2,
        height_quads: 2,
    },
    PaintingVariantInfo {
        name: "minecraft:fire",
        width_quads: 2,
        height_quads: 2,
    },
    PaintingVariantInfo {
        name: "minecraft:donkey_kong",
        width_quads: 4,
        height_quads: 3,
    },
    PaintingVariantInfo {
        name: "minecraft:baroque",
        width_quads: 2,
        height_quads: 2,
    },
    PaintingVariantInfo {
        name: "minecraft:humble",
        width_quads: 2,
        height_quads: 2,
    },
    PaintingVariantInfo {
        name: "minecraft:meditative",
        width_quads: 1,
        height_quads: 1,
    },
    PaintingVariantInfo {
        name: "minecraft:prairie_ride",
        width_quads: 1,
        height_quads: 2,
    },
    PaintingVariantInfo {
        name: "minecraft:unpacked",
        width_quads: 4,
        height_quads: 4,
    },
    PaintingVariantInfo {
        name: "minecraft:backyard",
        width_quads: 3,
        height_quads: 4,
    },
    PaintingVariantInfo {
        name: "minecraft:bouquet",
        width_quads: 3,
        height_quads: 3,
    },
    PaintingVariantInfo {
        name: "minecraft:cavebird",
        width_quads: 3,
        height_quads: 3,
    },
    PaintingVariantInfo {
        name: "minecraft:changing",
        width_quads: 4,
        height_quads: 2,
    },
    PaintingVariantInfo {
        name: "minecraft:cotan",
        width_quads: 3,
        height_quads: 3,
    },
    PaintingVariantInfo {
        name: "minecraft:endboss",
        width_quads: 3,
        height_quads: 3,
    },
    PaintingVariantInfo {
        name: "minecraft:fern",
        width_quads: 3,
        height_quads: 3,
    },
    PaintingVariantInfo {
        name: "minecraft:finding",
        width_quads: 4,
        height_quads: 2,
    },
    PaintingVariantInfo {
        name: "minecraft:lowmist",
        width_quads: 4,
        height_quads: 2,
    },
    PaintingVariantInfo {
        name: "minecraft:orb",
        width_quads: 4,
        height_quads: 4,
    },
    PaintingVariantInfo {
        name: "minecraft:owlemons",
        width_quads: 3,
        height_quads: 3,
    },
    PaintingVariantInfo {
        name: "minecraft:passage",
        width_quads: 4,
        height_quads: 2,
    },
    PaintingVariantInfo {
        name: "minecraft:pond",
        width_quads: 3,
        height_quads: 4,
    },
    PaintingVariantInfo {
        name: "minecraft:sunflowers",
        width_quads: 3,
        height_quads: 3,
    },
    PaintingVariantInfo {
        name: "minecraft:tides",
        width_quads: 3,
        height_quads: 3,
    },
    PaintingVariantInfo {
        name: "minecraft:dennis",
        width_quads: 3,
        height_quads: 3,
    },
];

/// Placeable painting variants (from pumpkin-data tag.rs - `MINECRAFT_PLACEABLE`)
/// All variants except: earth, wind, water, fire
pub const PLACEABLE_VARIANTS: &[&str] = &[
    "minecraft:kebab",
    "minecraft:aztec",
    "minecraft:alban",
    "minecraft:aztec2",
    "minecraft:bomb",
    "minecraft:plant",
    "minecraft:wasteland",
    "minecraft:pool",
    "minecraft:courbet",
    "minecraft:sea",
    "minecraft:sunset",
    "minecraft:creebet",
    "minecraft:wanderer",
    "minecraft:graham",
    "minecraft:match",
    "minecraft:bust",
    "minecraft:stage",
    "minecraft:void",
    "minecraft:skull_and_roses",
    "minecraft:wither",
    "minecraft:fighters",
    "minecraft:pointer",
    "minecraft:pigscene",
    "minecraft:burning_skull",
    "minecraft:skeleton",
    "minecraft:donkey_kong",
    "minecraft:baroque",
    "minecraft:humble",
    "minecraft:meditative",
    "minecraft:prairie_ride",
    "minecraft:unpacked",
    "minecraft:backyard",
    "minecraft:bouquet",
    "minecraft:cavebird",
    "minecraft:changing",
    "minecraft:cotan",
    "minecraft:endboss",
    "minecraft:fern",
    "minecraft:finding",
    "minecraft:lowmist",
    "minecraft:orb",
    "minecraft:owlemons",
    "minecraft:passage",
    "minecraft:pond",
    "minecraft:sunflowers",
    "minecraft:tides",
    "minecraft:dennis",
];

pub struct PaintingEntity {
    entity: Entity,
    /// The painting variant (registry name like "kebab", "aztec", etc.)
    variant: ArcSwap<String>,
    /// `BlockAttachedEntity.pos` (`BlockAttachedEntity.java:25`): the block the painting
    /// hangs on the face of. This is not `Entity::block_pos`, which floors the bounding
    /// box centre and therefore names a different block for even-sized paintings.
    anchor: AtomicCell<BlockPos>,
}

impl PaintingEntity {
    /// `Painting.DEPTH` (`Painting.java:44`).
    pub const DEPTH: f64 = 0.0625;

    pub fn new(entity: Entity) -> Self {
        let anchor = entity.block_pos.load();
        Self {
            entity,
            variant: ArcSwap::new(std::sync::Arc::new("minecraft:kebab".to_string())),
            anchor: AtomicCell::new(anchor),
        }
    }

    #[must_use]
    pub fn anchor(&self) -> BlockPos {
        self.anchor.load()
    }

    #[must_use]
    pub fn facing(&self) -> BlockDirection {
        BlockDirection::from_index(self.entity.data.load(Ordering::Relaxed) as u8)
            .unwrap_or(BlockDirection::South)
    }

    /// `Painting.offsetForPaintingSize` (`Painting.java:167-169`).
    const fn offset_for_painting_size(size: i32) -> f64 {
        if size % 2 == 0 { 0.5 } else { 0.0 }
    }

    /// `Painting.calculateBoundingBox` (`Painting.java:152-165`), for a painting of
    /// `width` x `height` blocks hung on the `facing` side of `pos`.
    #[must_use]
    pub fn calculate_bounding_box(
        pos: BlockPos,
        facing: BlockDirection,
        width: i32,
        height: i32,
    ) -> BoundingBox {
        let step = facing.to_offset();
        // `Vec3.atCenterOf(pos).relative(direction, -0.46875)`.
        let attached = Vector3::new(
            f64::from(pos.0.x) + 0.5 - f64::from(step.x) * 0.468_75,
            f64::from(pos.0.y) + 0.5 - f64::from(step.y) * 0.468_75,
            f64::from(pos.0.z) + 0.5 - f64::from(step.z) * 0.468_75,
        );
        let left = facing.rotate_counter_clockwise().to_offset();
        let horizontal_offset = Self::offset_for_painting_size(width);
        let vertical_offset = Self::offset_for_painting_size(height);
        let centre = Vector3::new(
            attached.x + f64::from(left.x) * horizontal_offset,
            attached.y + vertical_offset,
            attached.z + f64::from(left.z) * horizontal_offset,
        );
        let horizontal = f64::from(width);
        let (size_x, size_z) = if facing.to_axis() == pumpkin_data::block_properties::Axis::X {
            (Self::DEPTH, horizontal)
        } else {
            (horizontal, Self::DEPTH)
        };
        let size_y = f64::from(height);
        // `AABB.ofSize(position, xSize, ySize, zSize)`.
        BoundingBox::new(
            Vector3::new(
                centre.x - size_x / 2.0,
                centre.y - size_y / 2.0,
                centre.z - size_z / 2.0,
            ),
            Vector3::new(
                centre.x + size_x / 2.0,
                centre.y + size_y / 2.0,
                centre.z + size_z / 2.0,
            ),
        )
    }

    /// `Painting(level, blockPos, direction, variant)` (`Painting.java:130-134`): the
    /// variant and direction together fix the bounding box, and
    /// `HangingEntity.recalculateBoundingBox` (`HangingEntity.java:70-77`) then moves the
    /// entity to its centre.
    pub fn set_placement(&self, anchor: BlockPos, facing: BlockDirection, variant: &str) {
        self.anchor.store(anchor);
        self.set_variant_with_dimensions(variant);
        // `HangingEntity.setDirection` (`HangingEntity.java:60-67`): the spawn packet's
        // data field carries the 3D direction id and the yaw is `get2DDataValue() * 90`.
        self.entity
            .data
            .store(i32::from(facing.to_index()), Ordering::Relaxed);
        let yaw = match facing {
            BlockDirection::West => 90.0,
            BlockDirection::North => 180.0,
            BlockDirection::East => 270.0,
            _ => 0.0,
        };
        self.entity.set_rotation(yaw, 0.0);
        let (width, height) = self.get_current_dimensions();
        let bounding_box = Self::calculate_bounding_box(anchor, facing, width, height);
        let centre = Vector3::new(
            f64::midpoint(bounding_box.min.x, bounding_box.max.x),
            f64::midpoint(bounding_box.min.y, bounding_box.max.y),
            f64::midpoint(bounding_box.min.z, bounding_box.max.z),
        );
        self.entity.pos.store(centre);
        self.entity.bounding_box.store(bounding_box);
    }

    pub fn get_variant(&self) -> String {
        self.variant.load().to_string()
    }

    pub fn set_variant(&self, variant: String) {
        self.variant.store(std::sync::Arc::new(variant));
    }

    /// Get the dimensions of a painting variant in quads
    #[must_use]
    pub fn get_variant_dimensions(variant: &str) -> Option<(i32, i32)> {
        PAINTING_VARIANTS
            .iter()
            .find(|v| v.name == variant)
            .map(|v| (v.width_quads, v.height_quads))
    }

    /// Get the dimensions of the current variant in quads
    pub fn get_current_dimensions(&self) -> (i32, i32) {
        let variant = self.get_variant();
        Self::get_variant_dimensions(&variant).unwrap_or((1, 1))
    }

    /// Pick a random placeable variant
    /// This is a simplified selection that doesn't check fit - it just picks
    /// a random variant that can be placed.
    #[must_use]
    pub fn pick_random_variant() -> String {
        use rand::seq::IndexedRandom;
        let mut rng = rand::rng();
        PLACEABLE_VARIANTS
            .choose(&mut rng)
            .map_or_else(|| "minecraft:kebab".to_string(), ToString::to_string)
    }

    /// Set the variant and optionally update the entity dimensions.
    /// Painting dimensions in pixels are: `width_quads` * 16, `height_quads` * 16
    pub fn set_variant_with_dimensions(&self, variant: &str) {
        self.set_variant(variant.to_string());
        if let Some((width_quads, height_quads)) = Self::get_variant_dimensions(variant) {
            // `PaintingVariant.width()`/`height()` are already in blocks
            // (`Painting.java:161-163` feeds them straight into `AABB.ofSize`).
            let width = width_quads as f32;
            let height = height_quads as f32;

            let dimensions = pumpkin_util::math::boundingbox::EntityDimensions {
                width,
                height,
                eye_height: 0.0,
            };
            self.entity.entity_dimension.store(dimensions);
        }
    }
}

impl NBTStorage for PaintingEntity {
    fn write_nbt<'a>(&'a self, nbt: &'a mut NbtCompound) -> NbtFuture<'a, ()> {
        Box::pin(async {
            self.entity.write_nbt(nbt).await;
            nbt.put_byte("facing", self.entity.data.load(Ordering::Relaxed) as i8);
            nbt.put_string("variant", self.get_variant());
            // `BlockAttachedEntity.addAdditionalSaveData` (`BlockAttachedEntity.java:119-121`).
            let anchor = self.anchor.load();
            nbt.put(
                "block_pos",
                NbtTag::IntArray(vec![anchor.0.x, anchor.0.y, anchor.0.z]),
            );
        })
    }

    fn read_nbt_non_mut<'a>(&'a self, nbt: &'a NbtCompound) -> NbtFuture<'a, ()> {
        Box::pin(async {
            self.entity.read_nbt_non_mut(nbt).await;
            let facing = nbt.get_byte("facing").unwrap_or(3);
            self.entity.data.store(facing as i32, Ordering::Relaxed);
            if let Some(variant) = nbt.get_string("variant") {
                self.set_variant_with_dimensions(variant);
            }
            // `BlockAttachedEntity.readAdditionalSaveData` (`BlockAttachedEntity.java:124-127`).
            if let Some(&[x, y, z]) = nbt.get_int_array("block_pos") {
                self.anchor.store(BlockPos(Vector3::new(x, y, z)));
            } else {
                self.anchor.store(self.entity.block_pos.load());
            }
        })
    }
}

impl EntityBase for PaintingEntity {
    fn get_entity(&self) -> &Entity {
        &self.entity
    }

    fn get_living_entity(&self) -> Option<&LivingEntity> {
        None
    }

    fn is_pickable(&self) -> bool {
        true
    }

    fn init_data_tracker(&self) -> EntityBaseFuture<'_, ()> {
        Box::pin(async {
            let entity = self.get_entity();
            entity.send_meta_data(
                &[Metadata::new(
                    tracked_data::painting::PAINTING_VARIANT_ID,
                    self.get_variant(),
                )],
                None,
            );
        })
    }

    fn damage_with_context<'a>(
        &'a self,
        _caller: &'a dyn EntityBase,
        _amount: f32,
        _damage_type: DamageType,
        _position: Option<Vector3<f64>>,
        _source: Option<&'a dyn EntityBase>,
        _cause: Option<&'a dyn EntityBase>,
    ) -> EntityBaseFuture<'a, bool> {
        Box::pin(async {
            // TODO
            self.entity.remove().await;
            true
        })
    }

    /// `Painting.getAddEntityPacket` (`Painting.java:196-199`): the painting sends the
    /// block it hangs on (`trackingPosition`, `Painting.java:191-194`) rather than its
    /// bounding-box centre, because the client rebuilds the box from the variant itself.
    fn send_java_spawn_packet<'a>(
        &'a self,
        client: &'a crate::net::java::JavaClient,
    ) -> EntityBaseFuture<'a, ()> {
        Box::pin(async move {
            let anchor = self.anchor.load();
            let packet = CSpawnEntity::new(
                VarInt(self.entity.entity_id),
                self.entity.entity_uuid,
                VarInt(i32::from(self.entity.entity_type.id)),
                Vector3::new(
                    f64::from(anchor.0.x),
                    f64::from(anchor.0.y),
                    f64::from(anchor.0.z),
                ),
                self.entity.pitch.load(),
                self.entity.yaw.load(),
                self.entity.head_yaw.load(),
                self.entity.data.load(Ordering::Relaxed).into(),
                Vector3::new(0.0, 0.0, 0.0),
            );
            if let Ok(data) = client.serialize_packet(&packet) {
                client.enqueue_packet(data).await;
            }
        })
    }

    fn as_nbt_storage(&self) -> &dyn NBTStorage {
        self
    }

    fn cast_any(&self) -> &dyn std::any::Any {
        self
    }
}
