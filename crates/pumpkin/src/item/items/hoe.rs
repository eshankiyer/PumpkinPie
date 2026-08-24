use crate::entity::Entity;
use crate::entity::EntityBase;
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
use std::pin::Pin;
use std::sync::Arc;

pub struct HoeItem;

impl ItemMetadata for HoeItem {
    fn ids() -> Box<[u16]> {
        tag::Item::MINECRAFT_HOES.1.into()
    }
}

impl ItemBehaviour for HoeItem {
    fn use_on_block<'a>(
        &'a self,
        _item: &'a mut ItemStack,
        player: &'a Player,
        location: BlockPos,
        face: BlockDirection,
        _cursor_pos: Vector3<f32>,
        block: &'a Block,
        _server: &'a Server,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            // Yes, Minecraft does hardcode these
            if block == &Block::GRASS_BLOCK
                || block == &Block::DIRT_PATH
                || block == &Block::DIRT
                || block == &Block::COARSE_DIRT
                || block == &Block::ROOTED_DIRT
            {
                let mut future_block = block;
                let world = player.world();
                let mut changed = false;

                //Only rooted can be right-clicked on the bottom of the block
                if face == BlockDirection::Down {
                    if block == &Block::ROOTED_DIRT {
                        future_block = &Block::DIRT;
                        changed = true;
                    }
                } else {
                    // grass, dirt && dirt path become farmland
                    if (block == &Block::GRASS_BLOCK
                        || block == &Block::DIRT_PATH
                        || block == &Block::DIRT)
                        && world.get_block_state(&location.up()).is_air()
                    {
                        future_block = &Block::FARMLAND;
                        changed = true;
                    }
                    // Coarse dirt becomes dirt, but (like grass/dirt/dirt path above) only if
                    // there's air above it; rooted dirt is the only tillable with no air
                    // requirement (vanilla HoeItem.TILLABLES: onlyIfAirAbove vs unconditional).
                    else if (block == &Block::COARSE_DIRT
                        && world.get_block_state(&location.up()).is_air())
                        || block == &Block::ROOTED_DIRT
                    {
                        future_block = &Block::DIRT;
                        changed = true;
                    }
                }

                // Vanilla returns PASS without touching the block when nothing is tilled,
                // otherwise the rewrite would reset properties such as `snowy` on grass blocks.
                if changed {
                    world.play_block_sound_expect(
                        player,
                        Sound::ItemHoeTill,
                        SoundCategory::Blocks,
                        location,
                    );
                    let new_state_id = future_block.default_state.id;
                    world
                        .set_block_state(&location, new_state_id, BlockFlags::NOTIFY_ALL)
                        .await;
                    // Vanilla's till actions end with
                    // `gameEvent(GameEvent.BLOCK_CHANGE, pos, Context.of(player, state))`
                    // (`HoeItem.java:73` for plain tills and `HoeItem.java:80` for the
                    // rooted-dirt drop variant).
                    if let Some(player_arc) = world.get_player_by_id(player.get_entity().entity_id)
                    {
                        crate::world::game_event::emit_game_event(
                            &world,
                            pumpkin_data::game_event::GameEvent::BlockChange,
                            location.to_centered_f64(),
                            crate::world::game_event::GameEventContext::of_entity_with_block_state(
                                player_arc,
                                new_state_id,
                            ),
                        )
                        .await;
                    }
                }

                //Also rooted_dirt drop a hanging_root
                if block == &Block::ROOTED_DIRT {
                    let location = match face {
                        BlockDirection::Up => location.up().to_f64(),
                        BlockDirection::Down => location.down().to_f64(),
                        BlockDirection::North => location.up().to_f64().add_raw(0.0, -0.4, -1.0),
                        BlockDirection::South => location.up().to_f64().add_raw(0.0, -0.4, 1.0),
                        BlockDirection::West => location.up().to_f64().add_raw(-1.0, -0.4, 0.0),
                        BlockDirection::East => location.up().to_f64().add_raw(1.0, -0.4, 0.0),
                    };
                    let entity = Entity::new(world.clone(), location, &EntityType::ITEM);
                    // TODO: Merge stacks together
                    let item_entity = Arc::new(ItemEntity::new(
                        entity,
                        ItemStack::new(1, &Item::HANGING_ROOTS),
                    ));
                    world.spawn_entity(item_entity).await;
                }

                if changed && player.gamemode.load() != GameMode::Creative {
                    player.damage_held_item(1).await;
                }
            }
        })
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}
