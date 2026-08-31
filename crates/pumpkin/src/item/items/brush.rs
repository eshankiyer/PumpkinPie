use std::any::Any;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use crate::block::blocks::brushable_block::brush_sound;
use crate::block::entities::brushable_block::BrushableBlockBlockEntity;
use crate::entity::item::ItemEntity;
use crate::entity::player::Player;
use crate::entity::{Entity, EntityBase};
use crate::item::{ItemBehaviour, ItemMetadata};
use crate::server::Server;
use pumpkin_data::data_component_impl::EquipmentSlot;
use pumpkin_data::entity::EntityType;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_data::{Block, BlockDirection};
use pumpkin_protocol::codec::var_int::VarInt;
use pumpkin_util::Hand;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;

pub struct BrushItem;

impl ItemMetadata for BrushItem {
    fn ids() -> Box<[u16]> {
        Box::new([Item::BRUSH.id])
    }
}

impl ItemBehaviour for BrushItem {
    /// `BrushItem.useOn` (`BrushItem.java:37-45`) recalculates the view hit before
    /// starting the use animation; all of the work happens in `onUseTick`.
    fn use_on_block<'a>(
        &'a self,
        item: &'a mut ItemStack,
        player: &'a Player,
        _location: BlockPos,
        _face: BlockDirection,
        _cursor_pos: Vector3<f32>,
        _block: &'a Block,
        _server: &'a Server,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            let world = player.world();
            let (start, end) = self.get_start_and_end_pos(player);
            if world
                .raycast(start, end, async |pos, w| !w.get_block_state(pos).is_air())
                .await
                .is_none()
            {
                return;
            }

            let off_hand = player.inventory().off_hand_item().await;
            let hand = brushing_hand(item, &off_hand);
            player
                .living_entity
                .set_active_hand(hand, item.clone(), Self::USE_DURATION)
                .await;
        })
    }

    /// `BrushItem.onUseTick` (`BrushItem.java:57-96`): one brush stroke lands on every
    /// tick where `elapsed % 10 == 5`, and the brush is damaged only when the stroke
    /// finished the block off (`BrushItem.java:81-87`).
    fn on_use_tick<'a>(
        &'a self,
        stack: &'a ItemStack,
        player: &'a Player,
        remaining_use_ticks: i32,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            if remaining_use_ticks < 0 {
                player.living_entity.clear_active_hand().await;
                return;
            }

            let world = player.world();
            let (start, end) = self.get_start_and_end_pos(player);
            let Some((position, face)) = world
                .raycast(start, end, async |pos, w| !w.get_block_state(pos).is_air())
                .await
            else {
                // `BrushItem.java:90-92`: losing the target stops the use.
                player.living_entity.clear_active_hand().await;
                return;
            };

            let elapsed = Self::USE_DURATION - remaining_use_ticks + 1;
            if elapsed % 10 != 5 {
                return;
            }

            let block = world.get_block(&position);
            let state = world.get_block_state(&position);
            // `BrushItem.onUseTick` emits dust only when the state allows terrain particles
            // (`BrushItem.java:65-70`); the property comes from `noTerrainParticles`
            // (`BlockBehaviour.java:1265-1267`).
            if state.should_spawn_terrain_particles() {
                let mut particle_data = Vec::new();
                let _ = VarInt(i32::from(pumpkin_data::BlockState::to_be_network_id(
                    state.id,
                )))
                .encode(&mut particle_data);
                let look = player.living_entity.get_looking_vector();
                let dust = match face {
                    BlockDirection::Down | BlockDirection::Up => (look.z, -look.x),
                    BlockDirection::North => (1.0, -0.1),
                    BlockDirection::South => (-1.0, 0.1),
                    BlockDirection::West => (-0.1, -1.0),
                    BlockDirection::East => (0.1, 1.0),
                };
                let mut particle_position = position.to_centered_f64();
                match face {
                    BlockDirection::Down => particle_position.y -= 0.5,
                    BlockDirection::Up => particle_position.y += 0.5,
                    BlockDirection::North => particle_position.z -= 0.5,
                    BlockDirection::South => particle_position.z += 0.5,
                    BlockDirection::West => particle_position.x -= 0.5,
                    BlockDirection::East => particle_position.x += 0.5,
                }
                world.spawn_particle_with_data(
                    particle_position,
                    Vector3::new((dust.0 * 3.0) as f32, 0.0, (dust.1 * 3.0) as f32),
                    0.0,
                    rand::random_range(7..12),
                    pumpkin_data::particle::Particle::BlockCrumble,
                    &particle_data,
                );
            }
            world.play_sound(
                brush_sound(block),
                SoundCategory::Blocks,
                &position.to_f64(),
            );

            let Some(be) = world.get_block_entity(&position) else {
                return;
            };
            let Some(brush_be) = be.as_any().downcast_ref::<BrushableBlockBlockEntity>() else {
                return;
            };

            let game_time = world.get_world_age().await;
            if brush_be.brush(&world, game_time, face).await {
                // `BrushItem.onUseTick` chooses OFFHAND when the active stack equals the
                // off-hand stack, otherwise MAINHAND (`BrushItem.java:80-87`).
                let off_hand = player.inventory().off_hand_item().await;
                let hand = brushing_hand(stack, &off_hand);
                let equipment_slot = match hand {
                    Hand::Right => EquipmentSlot::MAIN_HAND,
                    Hand::Left => EquipmentSlot::OFF_HAND,
                };
                player.damage_item_in_slot(&equipment_slot, 1).await;
            }
        })
    }

    fn use_on_entity<'a>(
        &'a self,
        _item: &'a mut ItemStack,
        player: &'a Player,
        entity: Arc<dyn EntityBase>,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            let ent = entity.get_entity();
            if ent.entity_type == &EntityType::ARMADILLO {
                let world = player.world();
                world.play_sound(
                    Sound::EntityArmadilloBrush,
                    SoundCategory::Neutral,
                    &ent.pos.load(),
                );

                let item_entity = Arc::new(ItemEntity::new(
                    Entity::new(world.clone(), ent.pos.load(), &EntityType::ITEM),
                    ItemStack::new(1, &Item::ARMADILLO_SCUTE),
                ));
                world.spawn_entity(item_entity).await;

                player.damage_held_item(16).await;
            } else {
                let world = player.world();
                world.play_sound(
                    Sound::ItemBrushBrushingGeneric,
                    SoundCategory::Neutral,
                    &ent.pos.load(),
                );
            }

            let stack = player.inventory().held_item().await;
            player
                .living_entity
                .set_active_hand(pumpkin_util::Hand::Right, stack, Self::USE_DURATION)
                .await;
        })
    }

    fn get_use_duration(&self) -> i32 {
        Self::USE_DURATION
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

impl BrushItem {
    /// `BrushItem.USE_DURATION` (`BrushItem.java:31`).
    pub const USE_DURATION: i32 = 200;
}

/// `ItemStack.equals` in the vanilla off-hand choice (`BrushItem.java:83-85`) includes the
/// item count and components; `ItemStack.are_equal` is the matching workspace primitive.
fn brushing_hand(stack: &ItemStack, off_hand: &ItemStack) -> Hand {
    if stack.are_equal(off_hand) {
        Hand::Left
    } else {
        Hand::Right
    }
}

#[cfg(test)]
mod tests {
    use pumpkin_data::item::Item;
    use pumpkin_data::item_stack::ItemStack;
    use pumpkin_util::Hand;

    use super::{BrushItem, brushing_hand};

    /// `BrushItem.onUseTick` (`BrushItem.java:62-64`): with `getUseDuration` = 200 and a
    /// `remaining_use_ticks` that starts at 200 and decrements by one per tick, a stroke
    /// lands every ten ticks starting on the fifth.
    #[test]
    fn a_stroke_lands_every_ten_ticks_starting_at_five() {
        let strokes: Vec<i32> = (0..40)
            .map(|tick| BrushItem::USE_DURATION - tick)
            .filter(|remaining| (BrushItem::USE_DURATION - remaining + 1) % 10 == 5)
            .map(|remaining| BrushItem::USE_DURATION - remaining)
            .collect();
        assert_eq!(strokes, vec![4, 14, 24, 34]);
    }

    /// `BrushItem.onUseTick` (`BrushItem.java:83-85`) prefers the off-hand when its
    /// complete stack equals the active brush stack.
    #[test]
    fn matching_off_hand_is_damaged() {
        let active = ItemStack::new(1, &Item::BRUSH);
        let off_hand = ItemStack::new(1, &Item::BRUSH);

        assert!(matches!(brushing_hand(&active, &off_hand), Hand::Left));
    }
}
