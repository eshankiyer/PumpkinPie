use std::any::Any;
use std::future::Future;
use std::pin::Pin;

use crate::entity::player::Player;
use crate::item::{ItemBehaviour, ItemMetadata};
use crate::server::Server;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::world::WorldEvent;
use pumpkin_data::{Block, BlockDirection};
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_world::world::BlockFlags;

pub struct BoneMealItem;

impl ItemMetadata for BoneMealItem {
    fn ids() -> Box<[u16]> {
        Box::new([Item::BONE_MEAL.id])
    }
}

impl ItemBehaviour for BoneMealItem {
    #[allow(clippy::too_many_lines)]
    fn use_on_block<'a>(
        &'a self,
        item: &'a mut ItemStack,
        player: &'a Player,
        location: BlockPos,
        _face: BlockDirection,
        _cursor_pos: Vector3<f32>,
        block: &'a Block,
        server: &'a Server,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            let world = player.world();
            let state_id = world.get_block_state_id(&location);
            if server
                .block_registry
                .bone_meal(block, &world, &location, state_id)
                .await
            {
                world.sync_world_event(WorldEvent::ParticlesAndSoundPlantGrowth, location, 15);
                item.decrement_unless_creative(player.gamemode.load(), 1);
                return;
            }

            // Saplings still have no registered bone-meal behaviour; vanilla
            // SaplingBlock.performBonemeal advances stage 0 -> 1 before growing the tree.
            let sapling_action = block.properties(state_id).and_then(|props| {
                let prop_map = props.to_props();
                prop_map
                    .iter()
                    .find(|(k, _)| *k == "stage")
                    .and_then(|(_, stage_val)| stage_val.parse::<u8>().ok())
                    .filter(|&stage| stage < 1)
                    .map(|_| {
                        let new_props: Vec<(&str, &str)> = prop_map
                            .iter()
                            .map(|(k, v)| if *k == "stage" { (*k, "1") } else { (*k, *v) })
                            .collect();
                        block.from_properties(&new_props).to_state_id(block)
                    })
            });

            if let Some(new_state_id) = sapling_action {
                world
                    .set_block_state(&location, new_state_id, BlockFlags::NOTIFY_ALL)
                    .await;
                world.sync_world_event(WorldEvent::ParticlesAndSoundPlantGrowth, location, 15);
                item.decrement_unless_creative(player.gamemode.load(), 1);
            }
        })
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}
