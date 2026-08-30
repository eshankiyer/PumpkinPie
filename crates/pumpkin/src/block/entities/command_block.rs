use std::{
    pin::Pin,
    sync::atomic::{AtomicBool, AtomicU32, Ordering},
};

use crossbeam::atomic::AtomicCell;
use pumpkin_data::{
    Block, FacingExt,
    block_properties::{BlockProperties, CommandBlockLikeProperties},
    data_component_impl::CustomNameImpl,
    item_stack::ItemStack,
};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::text::TextComponent;

use std::sync::Mutex as StdMutex;
use tokio::sync::Mutex;

use super::BlockEntity;
use crate::world::World;

// todo: LastExecution, UpdateLastExecution
pub struct CommandBlockEntity {
    pub position: AtomicCell<BlockPos>,
    pub powered: AtomicBool,
    pub condition_met: AtomicBool,
    pub auto: AtomicBool,
    pub dirty: AtomicBool,
    pub command: Mutex<String>,
    pub last_output: Mutex<String>,
    pub track_output: AtomicBool,
    pub success_count: AtomicU32,
    /// Mirrors `BaseCommandBlock.customName`, applied by the live item-component placement path.
    /// (`CommandBlockEntity.java:29-62, 160-164`.)
    pub custom_name: StdMutex<Option<TextComponent>>,
}

impl CommandBlockEntity {
    pub const ID: &'static str = "minecraft:command_block";
    #[must_use]
    pub fn new(position: BlockPos, track_output: bool, is_chain: bool) -> Self {
        Self {
            position: AtomicCell::new(position),
            powered: AtomicBool::new(false),
            condition_met: AtomicBool::new(false),
            auto: AtomicBool::new(is_chain),
            dirty: AtomicBool::new(false),
            command: Mutex::new(String::new()),
            last_output: Mutex::new(String::new()),
            track_output: AtomicBool::new(track_output),
            success_count: AtomicU32::new(0),
            custom_name: StdMutex::new(None),
        }
    }

    /// Applies the command block's implicit custom-name component.
    /// (`CommandBlockEntity.java:160-164`.)
    pub fn apply_implicit_components(&self, stack: &ItemStack) {
        let custom_name = stack
            .get_data_component::<CustomNameImpl>()
            .map(|component| component.name.clone());
        *self
            .custom_name
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = custom_name;
    }

    /// Port of `CommandBlockEntity.markConditionMet` (`CommandBlockEntity.java:129-141`).
    /// The command-block tick uses the value from the previous scheduling decision, then
    /// refreshes it for the next tick just as vanilla's `CommandBlock.tick` does.
    pub fn mark_condition_met(&self, world: &World) -> bool {
        let position = self.position.load();
        let (block, state_id) = world.get_block_and_state_id(&position);
        let properties = CommandBlockLikeProperties::from_state_id(state_id, block);
        let condition_met = !properties.conditional || {
            let relative = position.offset(
                properties
                    .facing
                    .opposite()
                    .to_block_direction()
                    .to_offset(),
            );
            matches!(
                world.get_block(&relative).id,
                id if id == Block::COMMAND_BLOCK.id
                    || id == Block::CHAIN_COMMAND_BLOCK.id
                    || id == Block::REPEATING_COMMAND_BLOCK.id
            ) && world
                .get_block_entity(&relative)
                .and_then(|entity| {
                    entity
                        .as_any()
                        .downcast_ref::<Self>()
                        .map(|command| command.success_count.load(Ordering::Acquire) > 0)
                })
                .unwrap_or(false)
        };
        self.condition_met.store(condition_met, Ordering::Release);
        condition_met
    }

    /// Port of `CommandBlockEntity.onUpdated` (`CommandBlockEntity.java:37-40`).
    pub fn on_updated(self: &std::sync::Arc<Self>, world: &World) {
        let block_entity: std::sync::Arc<dyn BlockEntity> = self.clone();
        world.update_block_entity(&block_entity);
    }
}

impl BlockEntity for CommandBlockEntity {
    fn resource_location(&self) -> &'static str {
        Self::ID
    }
    fn get_position(&self) -> BlockPos {
        self.position.load()
    }

    fn from_nbt(nbt: &pumpkin_nbt::compound::NbtCompound, position: BlockPos) -> Self
    where
        Self: Sized,
    {
        let condition_met = AtomicBool::new(nbt.get_bool("conditionMet").unwrap_or(false));
        let auto = AtomicBool::new(nbt.get_bool("auto").unwrap_or(false));
        let powered = AtomicBool::new(nbt.get_bool("powered").unwrap_or(false));
        let command = Mutex::new(nbt.get_string("Command").unwrap_or("").to_string());
        let last_output = Mutex::new(nbt.get_string("LastOutput").unwrap_or("").to_string());
        let track_output = AtomicBool::new(nbt.get_bool("TrackOutput").unwrap_or(false));
        let success_count =
            AtomicU32::new(nbt.get_int("SuccessCount").unwrap_or(0).cast_unsigned());
        // `CommandBlockEntity.saveAdditional` delegates command state persistence to
        // `BaseCommandBlock.save` (`CommandBlockEntity.java:69-84`).
        let custom_name = nbt
            .get_string("CustomName")
            .and_then(|name| pumpkin_util::serde_json::from_str(name).ok());

        Self {
            position: AtomicCell::new(position),
            condition_met,
            auto,
            powered,
            command,
            last_output,
            track_output,
            success_count,
            dirty: AtomicBool::new(false),
            custom_name: StdMutex::new(custom_name),
        }
    }

    fn write_nbt<'a>(
        &'a self,
        nbt: &'a mut NbtCompound,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        Box::pin(async {
            nbt.put_bool("auto", self.auto.load(Ordering::SeqCst));
            nbt.put_string("Command", self.command.lock().await.to_string());
            nbt.put_bool("conditionMet", self.condition_met.load(Ordering::SeqCst));
            nbt.put_string("LastOutput", self.last_output.lock().await.to_string());
            nbt.put_bool("powered", self.powered.load(Ordering::SeqCst));
            nbt.put_bool("TrackOutput", self.track_output.load(Ordering::SeqCst));
            nbt.put_bool("UpdateLastExecution", false);
            nbt.put_int(
                "SuccessCount",
                self.success_count.load(Ordering::SeqCst).cast_signed(),
            );
            if let Some(name) = self
                .custom_name
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .as_ref()
                && let Ok(name_json) = pumpkin_util::serde_json::to_string(name)
            {
                nbt.put_string("CustomName", name_json);
            }
        })
    }

    fn chunk_data_nbt(&self) -> Option<NbtCompound> {
        let mut nbt = NbtCompound::new();
        futures::executor::block_on(async {
            self.write_nbt(&mut nbt).await;
        });
        Some(nbt)
    }

    fn is_dirty(&self) -> bool {
        self.dirty.load(Ordering::Relaxed)
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::CommandBlockEntity;
    use pumpkin_data::data_component::DataComponent;
    use pumpkin_data::data_component_impl::CustomNameImpl;
    use pumpkin_data::item::Item;
    use pumpkin_data::item_stack::ItemStack;
    use pumpkin_util::math::position::BlockPos;
    use pumpkin_util::text::TextComponent;

    #[test]
    fn custom_name_component_is_applied() {
        // `applyImplicitComponents` copies `DataComponents.CUSTOM_NAME` to the command block
        // (`CommandBlockEntity.java:160-164`).
        let stack = ItemStack::new_with_component(
            1,
            &Item::COMMAND_BLOCK,
            vec![(
                DataComponent::CustomName,
                Some(Box::new(CustomNameImpl {
                    name: TextComponent::text("named"),
                })),
            )],
        );
        let entity = CommandBlockEntity::new(BlockPos::new(0, 64, 0), true, false);

        entity.apply_implicit_components(&stack);

        assert_eq!(
            entity.custom_name.lock().unwrap().as_ref(),
            Some(&TextComponent::text("named"))
        );
    }
}
