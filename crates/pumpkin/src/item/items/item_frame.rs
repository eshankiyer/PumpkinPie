use std::pin::Pin;
use std::sync::Arc;

use crate::entity::Entity;
use crate::entity::EntityBase;
use crate::entity::decoration::item_frame::ItemFrameEntity;
use crate::entity::player::Player;
use crate::item::{ItemBehaviour, ItemMetadata};
use crate::server::Server;
use crate::world::game_event::{GameEventContext, emit_game_event};
use pumpkin_data::entity::EntityType;
use pumpkin_data::game_event::GameEvent;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_data::{Block, BlockDirection};
use pumpkin_util::math::boundingbox::BoundingBox;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;

/// The shared placement item for item frames and glow item frames.
///
/// Paintings share the same vanilla class (`HangingEntityItem`) but need painting-variant
/// selection (`Painting.create`, area-sorted `hasSpace`) that has no placement item
/// counterpart in Pumpkin yet, so they are out of scope here.
pub struct ItemFrameItem;

impl ItemFrameItem {
    const fn entity_type(item: &Item) -> &'static EntityType {
        if item.id == Item::GLOW_ITEM_FRAME.id {
            &EntityType::GLOW_ITEM_FRAME
        } else {
            &EntityType::ITEM_FRAME
        }
    }

    /// `HangingEntityItem.createBoundingBox` with `hasFramedMap = false`
    /// (`HangingEntityItem.java:114-124`; a freshly placed frame never has a map yet).
    /// `face` is always horizontal here -- the caller rejects vertical faces first,
    /// matching `HangingEntityItem.mayPlace` (`HangingEntityItem.java:79-81`).
    fn pop_box(position: Vector3<f64>, face: BlockDirection) -> BoundingBox {
        let (half_x, half_y, half_z) = match face {
            BlockDirection::North | BlockDirection::South => (0.375, 0.375, 0.03125),
            _ => (0.03125, 0.375, 0.375),
        };
        BoundingBox::new(
            Vector3::new(
                position.x - half_x,
                position.y - half_y,
                position.z - half_z,
            ),
            Vector3::new(
                position.x + half_x,
                position.y + half_y,
                position.z + half_z,
            ),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placement_box_is_thin_along_the_facing_axis() {
        let position = Vector3::new(10.0, 20.0, 30.0);

        let north = ItemFrameItem::pop_box(position, BlockDirection::North);
        assert_eq!(north.max.x - north.min.x, 0.75);
        assert_eq!(north.max.y - north.min.y, 0.75);
        assert_eq!(north.max.z - north.min.z, 0.0625);

        let east = ItemFrameItem::pop_box(position, BlockDirection::East);
        assert_eq!(east.max.x - east.min.x, 0.0625);
        assert_eq!(east.max.y - east.min.y, 0.75);
        assert_eq!(east.max.z - east.min.z, 0.75);
    }

    #[test]
    fn placement_box_is_centered_at_the_hanging_entity_position() {
        let position = Vector3::new(10.0, 20.0, 30.0);
        let bounding_box = ItemFrameItem::pop_box(position, BlockDirection::South);

        assert_eq!(bounding_box.min.x + bounding_box.max.x, 20.0);
        assert_eq!(bounding_box.min.y + bounding_box.max.y, 40.0);
        assert_eq!(bounding_box.min.z + bounding_box.max.z, 60.0);
    }
}

impl ItemMetadata for ItemFrameItem {
    fn ids() -> Box<[u16]> {
        [Item::ITEM_FRAME.id, Item::GLOW_ITEM_FRAME.id].into()
    }
}

impl ItemBehaviour for ItemFrameItem {
    /// `HangingEntityItem.useOn` (`HangingEntityItem.java:34-77`).
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
            // `HangingEntityItem.mayPlace` (line 79-81): only non-vertical faces are valid.
            // `player.mayUseItemAt` beyond that is a world-protection hook Pumpkin doesn't
            // model, so it is not checked here.
            if !face.is_horizontal() {
                return;
            }

            let target = BlockPos(location.0 + face.to_offset());
            // `Vec3.atCenterOf(blockPos).relative(direction, -0.46875)` (line 116).
            let offset = face.to_offset();
            let position = Vector3::new(
                f64::from(target.0.x) + 0.5 - f64::from(offset.x) * 0.46875,
                f64::from(target.0.y) + 0.5 - f64::from(offset.y) * 0.46875,
                f64::from(target.0.z) + 0.5 - f64::from(offset.z) * 0.46875,
            );

            let world = player.world();
            let pop_box = Self::pop_box(position, face);
            // `HangingEntity.hasLevelCollision` (`HangingEntity.java:107-110`).
            if !world.is_space_empty(pop_box) {
                return;
            }
            // `HangingEntity.canCoexist` (`HangingEntity.java:98-105`): only other hanging
            // entities (frames, glow frames, paintings) can block a placement -- unlike
            // `is_space_empty`, this must not reject the placing player standing in the box.
            let blocked_by_hanging_entity = world.get_entities_at_box(&pop_box).iter().any(|e| {
                let ty = e.get_entity().entity_type;
                ty == &EntityType::ITEM_FRAME
                    || ty == &EntityType::GLOW_ITEM_FRAME
                    || ty == &EntityType::PAINTING
            });
            if blocked_by_hanging_entity {
                return;
            }

            let entity_type = Self::entity_type(item.item);
            let entity = Entity::new(world.clone(), position, entity_type);
            let frame = ItemFrameEntity::new(entity);
            frame.set_facing(face);

            // `HangingEntity.survives` (`HangingEntity.java:82-92`): support-block check.
            if !frame.survives() {
                return;
            }

            let is_glow = entity_type == &EntityType::GLOW_ITEM_FRAME;
            world.play_sound(
                if is_glow {
                    Sound::EntityGlowItemFramePlace
                } else {
                    Sound::EntityItemFramePlace
                },
                SoundCategory::Blocks,
                &position,
            );

            if let Some(player_arc) = world.get_player_by_id(player.get_entity().entity_id) {
                emit_game_event(
                    &world,
                    GameEvent::EntityPlace,
                    position,
                    GameEventContext::of_entity(player_arc),
                )
                .await;
            }

            world.spawn_entity(Arc::new(frame)).await;
            item.decrement_unless_creative(player.gamemode.load(), 1);
        })
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}
