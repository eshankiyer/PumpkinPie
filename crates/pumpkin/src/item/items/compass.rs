use std::any::Any;
use std::future::Future;
use std::pin::Pin;

use crate::entity::{EntityBase, player::Player};
use crate::item::{ItemBehaviour, ItemMetadata};
use crate::server::Server;
use pumpkin_data::data_component::DataComponent;
use pumpkin_data::data_component_impl::{DataComponentImpl, LodestoneTarget, LodestoneTrackerImpl};
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_data::{Block, BlockDirection};
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;

pub struct CompassItem;

impl ItemMetadata for CompassItem {
    fn ids() -> Box<[u16]> {
        Box::new([Item::COMPASS.id, Item::RECOVERY_COMPASS.id])
    }
}

/// `new LodestoneTracker(Optional.empty(), true)`: clears the target while keeping the compass
/// in tracking mode (still shows the lodestone-compass appearance/name, just spins).
fn clear_lodestone_tracker(item: &mut ItemStack) {
    item.patch
        .retain(|(id, _)| *id != DataComponent::LodestoneTracker);
    item.patch.push((
        DataComponent::LodestoneTracker,
        Some(
            LodestoneTrackerImpl {
                target: None,
                tracked: true,
            }
            .to_dyn(),
        ),
    ));
}

impl ItemBehaviour for CompassItem {
    fn inventory_tick<'a>(
        &'a self,
        item: &'a mut ItemStack,
        owner: &'a dyn EntityBase,
        _server: &'a Server,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            let Some(tracker) = item.get_data_component::<LodestoneTrackerImpl>() else {
                return;
            };
            if !tracker.tracked {
                return;
            }
            let Some(target) = &tracker.target else {
                return;
            };

            let world = owner.get_entity().world.load();
            // Vanilla `LodestoneTracker.tick`: a tracker whose target is in a different
            // dimension from this one is left untouched (it isn't checked until the item is
            // back in that dimension).
            if target.dimension != world.dimension.minecraft_name {
                return;
            }

            // Vanilla `isInWorldBounds` failing short-circuits straight to invalidating the
            // tracker, unlike the different-dimension case above.
            if target.y < world.min_y || target.y >= world.min_y + world.dimension.height {
                clear_lodestone_tracker(item);
                return;
            }

            let target_pos = BlockPos(Vector3::new(target.x, target.y, target.z));
            // Vanilla queries the POI manager without forcing an unloaded target chunk to load.
            // Preserve the component until that location is available to inspect.
            if !world.is_loaded(&target_pos) {
                return;
            }
            if pumpkin_data::Block::from_state_id(world.get_block_state_id(&target_pos))
                == &Block::LODESTONE
            {
                return;
            }

            clear_lodestone_tracker(item);
        })
    }

    fn use_on_block<'a>(
        &'a self,
        item: &'a mut ItemStack,
        player: &'a Player,
        location: BlockPos,
        _face: BlockDirection,
        _cursor_pos: Vector3<f32>,
        block: &'a Block,
        _server: &'a Server,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            if block != &Block::LODESTONE {
                return;
            }

            let world = player.world();
            world.play_sound(
                Sound::ItemLodestoneCompassLock,
                SoundCategory::Players,
                &location.to_centered_f64(),
            );

            let target = LodestoneTrackerImpl {
                target: Some(LodestoneTarget {
                    dimension: world.dimension.minecraft_name.to_string(),
                    x: location.0.x,
                    y: location.0.y,
                    z: location.0.z,
                }),
                tracked: true,
            };

            let replace_existing_stack = !player.is_creative() && item.item_count == 1;
            if replace_existing_stack {
                item.patch
                    .push((DataComponent::LodestoneTracker, Some(target.to_dyn())));
            } else {
                let mut lodestone_compass = ItemStack::new(1, &Item::COMPASS);
                lodestone_compass
                    .patch
                    .push((DataComponent::LodestoneTracker, Some(target.to_dyn())));

                item.decrement_unless_creative(player.gamemode.load(), 1);

                player
                    .inventory()
                    .offer_or_drop_stack(lodestone_compass, player)
                    .await;
            }
        })
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}
