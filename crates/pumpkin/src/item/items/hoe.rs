use crate::entity::Entity;
use crate::entity::item::ItemEntity;
use crate::entity::player::Player;
use crate::item::{ItemBehaviour, ItemMetadata};
use crate::server::Server;
use pumpkin_data::BlockDirection;
use pumpkin_data::entity::EntityType;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_data::{Block, tag};
use pumpkin_util::GameMode;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_world::world::BlockFlags;
use std::sync::Arc;

pub struct HoeItem;

impl ItemMetadata for HoeItem {
    fn ids() -> Box<[u16]> {
        tag::Item::MINECRAFT_HOES.1.into()
    }
}

impl ItemBehaviour for HoeItem {
    fn use_on_block(
        &self,
        item: &mut ItemStack,
        player: &Player,
        location: BlockPos,
        face: BlockDirection,
        _cursor_pos: Vector3<f32>,
        block: &Block,
        _server: &Server,
    ) {
        if !matches!(block, &Block::GRASS_BLOCK | &Block::DIRT_PATH | &Block::DIRT | &Block::COARSE_DIRT | &Block::ROOTED_DIRT) {
            return;
        }
        let mut future_block = block;
        let world = player.world();
        let mut changed = false;
        if face == BlockDirection::Down {
            if block == &Block::ROOTED_DIRT {
                future_block = &Block::DIRT;
                changed = true;
            }
        } else if (block == &Block::GRASS_BLOCK || block == &Block::DIRT_PATH || block == &Block::DIRT)
            && world.get_block_state(&location.up()).is_air()
        {
            future_block = &Block::FARMLAND;
            changed = true;
        } else if (block == &Block::COARSE_DIRT && world.get_block_state(&location.up()).is_air())
            || block == &Block::ROOTED_DIRT
        {
            future_block = &Block::DIRT;
            changed = true;
        }

        if changed {
            world.play_block_sound_expect(player, Sound::ItemHoeTill, SoundCategory::Blocks, location);
            let new_state_id = future_block.default_state.id;
            world.set_block_state(&location, new_state_id, BlockFlags::NOTIFY_ALL);
            // Vanilla till actions emit BLOCK_CHANGE (`HoeItem.java:73`), including the
            // rooted-dirt drop variant (`HoeItem.java:80`).
            if let Some(player_arc) = world.get_player_by_id(player.get_entity().entity_id) {
                crate::world::game_event::emit_game_event(
                    &world,
                    pumpkin_data::game_event::GameEvent::BlockChange,
                    location.to_centered_f64(),
                    crate::world::game_event::GameEventContext::of_entity_with_block_state(player_arc, new_state_id),
                );
            }
        }

        if block == &Block::ROOTED_DIRT {
            let drop_location = match face {
                BlockDirection::Up => location.up().to_f64(),
                BlockDirection::Down => location.down().to_f64(),
                BlockDirection::North => location.up().to_f64().add_raw(0.0, -0.4, -1.0),
                BlockDirection::South => location.up().to_f64().add_raw(0.0, -0.4, 1.0),
                BlockDirection::West => location.up().to_f64().add_raw(-1.0, -0.4, 0.0),
                BlockDirection::East => location.up().to_f64().add_raw(1.0, -0.4, 0.0),
            };
            let entity = Entity::new(world.clone(), drop_location, &EntityType::ITEM);
            let item_entity = Arc::new(ItemEntity::new(entity, ItemStack::new(1, &Item::HANGING_ROOTS)));
            world.spawn_entity(item_entity);
        }
        if changed && player.gamemode.load() != GameMode::Creative {
            let _ = item.damage_item(1);
        }
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}
