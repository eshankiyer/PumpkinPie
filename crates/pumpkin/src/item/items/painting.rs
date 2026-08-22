use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use crate::entity::Entity;
use crate::entity::EntityBase;
use crate::entity::decoration::painting::{
    PAINTING_VARIANTS, PLACEABLE_VARIANTS, PaintingEntity, PaintingVariantInfo,
};
use crate::entity::player::Player;
use crate::item::{ItemBehaviour, ItemMetadata};
use crate::server::Server;
use crate::world::World;
use crate::world::game_event::{GameEventContext, emit_game_event};
use pumpkin_data::entity::EntityType;
use pumpkin_data::game_event::GameEvent;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_data::{Block, BlockDirection, BlockState};
use pumpkin_util::math::boundingbox::BoundingBox;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use rand::seq::IndexedRandom;

/// `HangingEntityItem` bound to `EntityTypes.PAINTING` (`Items.java:1115`).
pub struct PaintingItem;

impl ItemMetadata for PaintingItem {
    fn ids() -> Box<[u16]> {
        [Item::PAINTING.id].into()
    }
}

/// The placeable variants, resolved once against the dimension table.
fn placeable_variants() -> impl Iterator<Item = &'static PaintingVariantInfo> {
    PAINTING_VARIANTS
        .iter()
        .filter(|variant| PLACEABLE_VARIANTS.contains(&variant.name))
}

/// `HangingEntity.survives` (`HangingEntity.java:81-92`) for a painting of `width` x
/// `height` blocks hung on the `facing` face of `anchor`, evaluated before any entity is
/// constructed so that `Painting.create` can test every variant cheaply.
fn painting_survives(
    world: &World,
    anchor: BlockPos,
    facing: BlockDirection,
    width: i32,
    height: i32,
    border: &crate::world::border::Worldborder,
) -> bool {
    let pop_box = PaintingEntity::calculate_bounding_box(anchor, facing, width, height);

    // `HangingEntity.hasLevelCollision` (`HangingEntity.java:107-110`): no block
    // collision and no world-border collision.
    if !(border.contains(pop_box.min.x, pop_box.min.z)
        && border.contains(pop_box.max.x - 1.0e-5, pop_box.max.z - 1.0e-5))
    {
        return false;
    }
    if !world.is_space_empty(pop_box) {
        return false;
    }

    // `HangingEntity.calculateSupportBox` (`HangingEntity.java:94-96`): the box shifted
    // half a block into the wall and deflated, every block of which must be solid or a
    // diode (`DiodeBlock.isDiode`).
    let step = facing.to_offset();
    let support_box = BoundingBox::new(
        Vector3::new(
            pop_box.min.x - f64::from(step.x) * 0.5 + 1.0e-7,
            pop_box.min.y - f64::from(step.y) * 0.5 + 1.0e-7,
            pop_box.min.z - f64::from(step.z) * 0.5 + 1.0e-7,
        ),
        Vector3::new(
            pop_box.max.x - f64::from(step.x) * 0.5 - 1.0e-7,
            pop_box.max.y - f64::from(step.y) * 0.5 - 1.0e-7,
            pop_box.max.z - f64::from(step.z) * 0.5 - 1.0e-7,
        ),
    );
    #[expect(clippy::cast_possible_truncation)]
    let (min, max) = (
        BlockPos(Vector3::new(
            support_box.min.x.floor() as i32,
            support_box.min.y.floor() as i32,
            support_box.min.z.floor() as i32,
        )),
        BlockPos(Vector3::new(
            support_box.max.x.floor() as i32,
            support_box.max.y.floor() as i32,
            support_box.max.z.floor() as i32,
        )),
    );
    for x in min.0.x..=max.0.x {
        for y in min.0.y..=max.0.y {
            for z in min.0.z..=max.0.z {
                let position = BlockPos(Vector3::new(x, y, z));
                // An unloaded support block must not read as air, or the placement is
                // rejected for a chunk that simply is not there yet.
                let Some(state_id) = world.get_block_state_id_if_loaded(&position) else {
                    return false;
                };
                let (block, state) = BlockState::from_id_with_block(state_id);
                if !(state.is_solid() || block == &Block::REPEATER || block == &Block::COMPARATOR) {
                    return false;
                }
            }
        }
    }

    // `HangingEntity.canCoexist(false)` (`HangingEntity.java:98-105`): another painting
    // blocks whatever way it faces, other hanging entities only when they face the same
    // way. The painting being tested does not exist yet, so there is no self to skip.
    !world.get_entities_at_box(&pop_box).iter().any(|other| {
        let entity = other.get_entity();
        match entity.entity_type {
            t if t == &EntityType::PAINTING => true,
            t if t == &EntityType::ITEM_FRAME || t == &EntityType::GLOW_ITEM_FRAME => {
                entity.data.load(Ordering::Relaxed) == i32::from(facing.to_index())
            }
            _ => false,
        }
    })
}

impl ItemBehaviour for PaintingItem {
    /// `HangingEntityItem.useOn` (`HangingEntityItem.java:34-77`) with
    /// `Painting.create` (`Painting.java:93-120`).
    fn use_on_block<'a>(
        &'a self,
        item: &'a mut ItemStack,
        player: &'a Player,
        location: BlockPos,
        face: BlockDirection,
        _cursor_pos: Vector3<f32>,
        _block: &'a Block,
        _server: &'a Server,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            // `HangingEntityItem.mayPlace` (`HangingEntityItem.java:79-81`) rejects the
            // vertical faces that item frames accept.
            if !face.is_horizontal() {
                return;
            }
            let anchor = BlockPos(location.0 + face.to_offset());
            let world = player.world();
            if !world.is_in_height_limit(anchor.0.y) {
                return;
            }

            let border = world.worldborder.lock().await;
            // `Painting.create`: keep the placeable variants that survive here, then the
            // largest of those by area, then pick one at random.
            let mut candidates: Vec<&PaintingVariantInfo> = placeable_variants()
                .filter(|variant| {
                    painting_survives(
                        &world,
                        anchor,
                        face,
                        variant.width_quads,
                        variant.height_quads,
                        &border,
                    )
                })
                .collect();
            let Some(largest_area) = candidates
                .iter()
                .map(|variant| variant.width_quads * variant.height_quads)
                .max()
            else {
                return;
            };
            candidates.retain(|variant| variant.width_quads * variant.height_quads == largest_area);
            drop(border);
            let Some(chosen) = candidates.choose(&mut rand::rng()) else {
                return;
            };

            let entity = Entity::new(
                world.clone(),
                Vector3::new(
                    f64::from(anchor.0.x),
                    f64::from(anchor.0.y),
                    f64::from(anchor.0.z),
                ),
                &EntityType::PAINTING,
            );
            let painting = PaintingEntity::new(entity);
            painting.set_placement(anchor, face, chosen.name);

            // `Painting.playPlacementSound` (`Painting.java:181-184`).
            let position = painting.get_entity().pos.load();
            world.play_sound(Sound::EntityPaintingPlace, SoundCategory::Blocks, &position);
            if let Some(player_arc) = world.get_player_by_id(player.get_entity().entity_id) {
                emit_game_event(
                    &world,
                    GameEvent::EntityPlace,
                    position,
                    GameEventContext::of_entity(player_arc),
                )
                .await;
            }

            world.spawn_entity(Arc::new(painting)).await;
            item.decrement_unless_creative(player.gamemode.load(), 1);
        })
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn area(variant: &PaintingVariantInfo) -> i32 {
        variant.width_quads * variant.height_quads
    }

    #[test]
    fn every_placeable_variant_has_dimensions() {
        assert_eq!(placeable_variants().count(), PLACEABLE_VARIANTS.len());
    }

    /// `Painting.create` keeps only the largest-area survivors (`Painting.java:110-111`).
    #[test]
    fn largest_area_filter_keeps_only_the_biggest_variants() {
        let mut candidates: Vec<&PaintingVariantInfo> = placeable_variants().collect();
        let largest = candidates.iter().copied().map(area).max().unwrap();
        candidates.retain(|variant| area(variant) == largest);
        assert_eq!(largest, 16);
        assert!(candidates.iter().all(|variant| area(variant) == 16));
        assert!(
            candidates
                .iter()
                .any(|variant| variant.name == "minecraft:pointer")
        );
    }

    /// `Painting.calculateBoundingBox` (`Painting.java:152-165`): depth along the facing
    /// axis, variant size across it.
    #[test]
    fn bounding_box_is_variant_sized_and_depth_thin() {
        let box_2x1 = PaintingEntity::calculate_bounding_box(
            BlockPos::new(0, 0, 0),
            BlockDirection::South,
            2,
            1,
        );
        assert!((box_2x1.max.x - box_2x1.min.x - 2.0).abs() < 1.0e-9);
        assert!((box_2x1.max.y - box_2x1.min.y - 1.0).abs() < 1.0e-9);
        assert!((box_2x1.max.z - box_2x1.min.z - PaintingEntity::DEPTH).abs() < 1.0e-9);

        let box_east = PaintingEntity::calculate_bounding_box(
            BlockPos::new(0, 0, 0),
            BlockDirection::East,
            2,
            1,
        );
        assert!((box_east.max.x - box_east.min.x - PaintingEntity::DEPTH).abs() < 1.0e-9);
        assert!((box_east.max.z - box_east.min.z - 2.0).abs() < 1.0e-9);
    }

    /// `Painting.offsetForPaintingSize` (`Painting.java:167-169`): odd sizes stay centred
    /// on the anchor block, even sizes are shifted half a block towards the left side.
    #[test]
    fn odd_sized_painting_is_centred_on_its_anchor_block() {
        let odd = PaintingEntity::calculate_bounding_box(
            BlockPos::new(0, 0, 0),
            BlockDirection::South,
            1,
            1,
        );
        assert!((odd.min.x + odd.max.x - 1.0).abs() < 1.0e-9);
        assert!((odd.min.y + odd.max.y - 1.0).abs() < 1.0e-9);

        // South's counter-clockwise side is East, so a 2-wide painting moves +0.5 in x.
        let even = PaintingEntity::calculate_bounding_box(
            BlockPos::new(0, 0, 0),
            BlockDirection::South,
            2,
            2,
        );
        assert!((even.min.x + even.max.x - 2.0).abs() < 1.0e-9);
        assert!((even.min.y + even.max.y - 2.0).abs() < 1.0e-9);
    }
}
