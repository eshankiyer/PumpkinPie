use std::pin::Pin;

use pumpkin_nbt::compound::NbtCompound;
use pumpkin_nbt::tag::NbtTag;
use pumpkin_util::math::position::BlockPos;
use tokio::sync::Mutex;

use super::BlockEntity;

pub struct TestInstanceBlockBlockEntity {
    pub position: BlockPos,
    data: Mutex<NbtCompound>,
    errors: Mutex<Vec<NbtCompound>>,
}

/// `TestInstanceBlockEntity` constructs this `Data` value by default
/// (`TestInstanceBlockEntity.java:67-70`); its codec stores the size as a
/// three-int list and the enum values as their serialized names
/// (`TestInstanceBlockEntity.java:415-430`).
fn default_data() -> NbtCompound {
    let mut data = NbtCompound::new();
    data.put_list("size", vec![NbtTag::Int(0), NbtTag::Int(0), NbtTag::Int(0)]);
    data.put_string("rotation", "none".to_string());
    data.put_bool("ignore_entities", false);
    data.put_string("status", "cleared".to_string());
    data
}

impl BlockEntity for TestInstanceBlockBlockEntity {
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
        // `loadAdditional` reads the nested `data` codec and optional `errors` list
        // (`TestInstanceBlockEntity.java:160-165`).
        let data = nbt
            .get_compound("data")
            .cloned()
            .unwrap_or_else(default_data);
        let errors = nbt
            .get_list("errors")
            .map(|entries| {
                entries
                    .iter()
                    .filter_map(|entry| entry.extract_compound().cloned())
                    .collect()
            })
            .unwrap_or_default();
        Self {
            position,
            data: Mutex::new(data),
            errors: Mutex::new(errors),
        }
    }

    fn write_nbt<'a>(
        &'a self,
        nbt: &'a mut NbtCompound,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            // `loadAdditional` and `saveAdditional` persist `data` and the optional
            // `errors` list (`TestInstanceBlockEntity.java:160-172`).
            nbt.put_compound("data", self.data.lock().await.clone());
            let errors = self.errors.lock().await;
            if !errors.is_empty() {
                nbt.put_list(
                    "errors",
                    errors.iter().cloned().map(NbtTag::Compound).collect(),
                );
            }
        })
    }

    fn chunk_data_nbt(&self) -> Option<NbtCompound> {
        // `getUpdateTag` returns `saveCustomOnly`, which contains the same custom
        // fields sent by the live block-entity update path (`TestInstanceBlockEntity.java:155-172`).
        let mut nbt = NbtCompound::new();
        nbt.put_compound("data", self.data.try_lock().ok()?.clone());
        let errors = self.errors.try_lock().ok()?;
        if !errors.is_empty() {
            nbt.put_list(
                "errors",
                errors.iter().cloned().map(NbtTag::Compound).collect(),
            );
        }
        Some(nbt)
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

impl TestInstanceBlockBlockEntity {
    pub const ID: &'static str = "minecraft:test_instance_block";
    #[must_use]
    pub fn new(position: BlockPos) -> Self {
        Self {
            position,
            data: Mutex::new(default_data()),
            errors: Mutex::new(Vec::new()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{BlockEntity, TestInstanceBlockBlockEntity};
    use pumpkin_nbt::tag::NbtTag;
    use pumpkin_util::math::position::BlockPos;

    #[test]
    fn update_tag_contains_vanilla_default_data() {
        let entity = TestInstanceBlockBlockEntity::new(BlockPos::new(0, 64, 0));
        let tag = entity.chunk_data_nbt().expect("update tag");
        let data = tag.get_compound("data").expect("data compound");

        assert_eq!(data.get_string("rotation"), Some("none"));
        assert_eq!(data.get_bool("ignore_entities"), Some(false));
        assert_eq!(data.get_string("status"), Some("cleared"));
        assert_eq!(data.get_list("size").map(<[NbtTag]>::len), Some(3));
    }

    #[test]
    fn nbt_load_and_save_preserve_custom_payload() {
        let mut input = pumpkin_nbt::compound::NbtCompound::new();
        let mut data = pumpkin_nbt::compound::NbtCompound::new();
        data.put_string("rotation", "clockwise_90".to_string());
        input.put_compound("data", data.clone());

        let entity = TestInstanceBlockBlockEntity::from_nbt(&input, BlockPos::new(0, 64, 0));
        let tag = entity.chunk_data_nbt().expect("update tag");
        assert_eq!(tag.get_compound("data"), Some(&data));
    }
}
