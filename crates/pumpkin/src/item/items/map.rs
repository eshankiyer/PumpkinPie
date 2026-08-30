use crate::block::entities::banner::BannerBlockEntity;
use crate::entity::player::Player;
use crate::item::ItemBehaviour;
use crate::item::ItemMetadata;
use crate::server::Server;
use pumpkin_data::data_component::DataComponent;
use pumpkin_data::data_component_impl::DataComponentImpl;
use pumpkin_data::data_component_impl::MapIdImpl;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::map_decoration::MapDecorationType;
use pumpkin_data::tag::{self, Taggable};
use pumpkin_util::GameMode;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use std::any::Any;
use std::future::Future;
use std::pin::Pin;

pub struct MapItem;

impl ItemMetadata for MapItem {
    // Vanilla registers `MapItem` for filled maps and `EmptyMapItem` for empty maps
    // (`Items.java:1336-1340, 1509`); Pumpkin combines their existing item behavior here.
    fn ids() -> Box<[u16]> {
        [Item::MAP.id, Item::FILLED_MAP.id].into()
    }
}

impl ItemBehaviour for MapItem {
    fn normal_use<'a>(
        &'a self,
        item: &'a Item,
        player: &'a Player,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            if item.id != Item::MAP.id {
                return;
            }
            let Some(server) = player.world().server.upgrade() else {
                return;
            };

            let inventory = player.inventory();
            let held_stack = inventory.held_item().await;
            let (found, mut hand_stack, hand) =
                if !held_stack.is_empty() && held_stack.item.id == Item::MAP.id {
                    (true, held_stack, pumpkin_util::Hand::Right)
                } else {
                    let off_hand = inventory.off_hand_item().await;
                    if !off_hand.is_empty() && off_hand.item.id == Item::MAP.id {
                        (true, off_hand, pumpkin_util::Hand::Left)
                    } else {
                        (false, held_stack, pumpkin_util::Hand::Right)
                    }
                };

            if found {
                let map_id = server.next_map_id();
                let _ = server.map_manager.create_map(
                    map_id,
                    player.world().dimension.clone(),
                    player.position().x as i32,
                    player.position().z as i32,
                    0, // Default scale
                );

                let mut filled_map = ItemStack::new(1, &Item::FILLED_MAP);
                filled_map.patch.push((
                    DataComponent::MapId,
                    Some(MapIdImpl { id: map_id }.to_dyn()),
                ));

                let gamemode = player.gamemode.load();
                if hand_stack.item_count == 1 && gamemode != GameMode::Creative {
                    inventory.set_stack_in_hand(hand, filled_map).await;
                } else {
                    hand_stack.decrement_unless_creative(gamemode, 1);
                    inventory.set_stack_in_hand(hand, hand_stack).await;
                    inventory.offer_or_drop_stack(filled_map, player).await;
                }
            }
        })
    }

    fn use_on_block<'a>(
        &'a self,
        item: &'a mut ItemStack,
        player: &'a Player,
        location: BlockPos,
        _face: pumpkin_data::BlockDirection,
        _cursor_pos: Vector3<f32>,
        block: &'a pumpkin_data::Block,
        server: &'a Server,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            // `MapItem.useOn` (`MapItem.java:324-337`) delegates banner marker changes to
            // `MapItemSavedData.toggleBanner` (`MapItemSavedData.java:392-419`).
            if item.item.id != Item::FILLED_MAP.id || !block.has_tag(&tag::Block::MINECRAFT_BANNERS)
            {
                return;
            }
            let Some(map_id) = item
                .get_data_component::<MapIdImpl>()
                .map(|component| component.id)
            else {
                return;
            };
            let Some(map) = server.map_manager.get_map(map_id) else {
                return;
            };
            let Some(entity) = player.world().get_block_entity(&location) else {
                return;
            };
            let Some(banner) = entity.as_any().downcast_ref::<BannerBlockEntity>() else {
                return;
            };
            let Some(color) = block.name.strip_suffix("_banner") else {
                return;
            };
            let decoration_name = format!("banner_{color}");
            let Some(decoration_type) = MapDecorationType::from_name(&decoration_name) else {
                return;
            };
            let display_name = banner
                .custom_name
                .try_lock()
                .ok()
                .and_then(|name| name.clone());
            map.lock()
                .await
                .toggle_banner(location, decoration_type, display_name);
        })
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}
