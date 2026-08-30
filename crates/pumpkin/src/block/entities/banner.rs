use super::BlockEntity;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_nbt::tag::NbtTag;
use pumpkin_util::math::position::BlockPos;
use std::pin::Pin;
use tokio::sync::Mutex;

pub struct BannerBlockEntity {
    pub position: BlockPos,
    pub custom_name: Mutex<Option<String>>,
    pub patterns: Mutex<Option<Vec<NbtTag>>>,
}

impl BlockEntity for BannerBlockEntity {
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
        let patterns = nbt.get_list("patterns").map(<[_]>::to_vec);
        Self {
            position,
            custom_name: Mutex::new(custom_name),
            patterns: Mutex::new(patterns),
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
            if let Some(pats) = self.patterns.lock().await.as_ref() {
                nbt.put_list("patterns", pats.clone());
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
        if let Some(pats) = self.get_patterns() {
            nbt.put_list("patterns", pats);
        }
        Some(nbt)
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

impl BannerBlockEntity {
    pub const ID: &'static str = "minecraft:banner";

    /// Vanilla `BannerBlockEntity.getPatterns` (`BannerBlockEntity.java:74-76`): expose the
    /// stored layers to the live pick-block and client-update paths.
    #[must_use]
    pub fn get_patterns(&self) -> Option<Vec<NbtTag>> {
        self.patterns
            .try_lock()
            .ok()
            .and_then(|patterns| patterns.clone())
    }

    #[must_use]
    pub const fn new(position: BlockPos) -> Self {
        Self {
            position,
            custom_name: Mutex::const_new(None),
            patterns: Mutex::const_new(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::BlockEntity;
    use super::BannerBlockEntity;
    use pumpkin_nbt::compound::NbtCompound;
    use pumpkin_nbt::tag::NbtTag;
    use pumpkin_util::math::position::BlockPos;

    // `BannerBlockEntity.loadAdditional` restores the layers (`BannerBlockEntity.java:59-63`).
    #[test]
    fn patterns_accessor_reads_persisted_layers() {
        let mut nbt = NbtCompound::new();
        nbt.put_list("patterns", vec![NbtTag::String("base".into())]);
        let banner = <BannerBlockEntity as BlockEntity>::from_nbt(&nbt, BlockPos::new(0, 64, 0));

        assert_eq!(
            banner.get_patterns(),
            Some(vec![NbtTag::String("base".into())])
        );
    }
}
