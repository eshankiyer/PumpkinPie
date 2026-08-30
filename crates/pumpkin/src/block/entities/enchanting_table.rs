use super::BlockEntity;
use pumpkin_data::data_component_impl::CustomNameImpl;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_util::math::position::BlockPos;
use std::pin::Pin;
use tokio::sync::Mutex;

pub struct EnchantingTableBlockEntity {
    pub position: BlockPos,
    pub custom_name: Mutex<Option<String>>,
}

impl BlockEntity for EnchantingTableBlockEntity {
    fn resource_location(&self) -> &'static str {
        Self::ID
    }

    fn get_position(&self) -> BlockPos {
        self.position
    }

    fn from_nbt(nbt: &pumpkin_nbt::compound::NbtCompound, position: BlockPos) -> Self
    where
        Self: Sized,
    {
        let custom_name = nbt.get_string("CustomName").map(ToString::to_string);
        Self {
            position,
            custom_name: Mutex::new(custom_name),
        }
    }

    fn write_nbt<'a>(
        &'a self,
        nbt: &'a mut NbtCompound,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            if let Some(name) = self.custom_name.lock().await.as_ref() {
                nbt.put_string("CustomName", name.clone());
            }
        })
    }

    fn chunk_data_nbt(&self) -> Option<NbtCompound> {
        let mut nbt = NbtCompound::new();
        if let Ok(name) = self.custom_name.try_lock()
            && let Some(ref name) = *name
        {
            nbt.put_string("CustomName", name.clone());
        }
        Some(nbt)
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

impl EnchantingTableBlockEntity {
    pub const ID: &'static str = "minecraft:enchanting_table";
    #[must_use]
    pub const fn new(position: BlockPos) -> Self {
        Self {
            position,
            custom_name: Mutex::const_new(None),
        }
    }

    /// Applies `EnchantingTableBlockEntity.applyImplicitComponents`
    /// (`EnchantingTableBlockEntity.java:123-126`) during item placement.
    pub fn apply_implicit_components(&self, stack: &ItemStack) {
        let custom_name = stack
            .get_data_component::<CustomNameImpl>()
            .and_then(|component| pumpkin_util::serde_json::to_string(&component.name).ok());
        if let Ok(mut name) = self.custom_name.try_lock() {
            *name = custom_name;
        }
    }
}
