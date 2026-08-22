use super::BlockEntity;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_util::math::position::BlockPos;
use std::pin::Pin;
use tokio::sync::Mutex;

pub struct SkullBlockEntity {
    pub position: BlockPos,
    pub note_block_sound: Mutex<Option<String>>,
    pub profile: Mutex<Option<NbtCompound>>,
    /// `SkullBlockEntity.customName` (`SkullBlockEntity.java:29`), saved under `custom_name`
    /// (`SkullBlockEntity.java:40`) as a serialized text component. Held as the raw JSON
    /// string so it round-trips unchanged.
    pub custom_name: Mutex<Option<String>>,
}

impl BlockEntity for SkullBlockEntity {
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
        let note_block_sound = nbt.get_string("note_block_sound").map(ToString::to_string);
        let profile = nbt.get_compound("profile").cloned();
        let custom_name = nbt.get_string("custom_name").map(ToString::to_string);
        Self {
            position,
            note_block_sound: Mutex::new(note_block_sound),
            profile: Mutex::new(profile),
            custom_name: Mutex::new(custom_name),
        }
    }

    fn write_nbt<'a>(
        &'a self,
        nbt: &'a mut NbtCompound,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            if let Some(sound) = self.note_block_sound.lock().await.as_ref() {
                nbt.put_string("note_block_sound", sound.clone());
            }
            if let Some(prof) = self.profile.lock().await.as_ref() {
                nbt.put_compound("profile", prof.clone());
            }
            if let Some(name) = self.custom_name.lock().await.as_ref() {
                nbt.put_string("custom_name", name.clone());
            }
        })
    }

    fn chunk_data_nbt(&self) -> Option<NbtCompound> {
        let mut nbt = NbtCompound::new();
        if let Ok(sound) = self.note_block_sound.try_lock()
            && let Some(ref sound) = *sound
        {
            nbt.put_string("note_block_sound", sound.clone());
        }
        if let Ok(profile) = self.profile.try_lock()
            && let Some(ref prof) = *profile
        {
            nbt.put_compound("profile", prof.clone());
        }
        if let Ok(name) = self.custom_name.try_lock()
            && let Some(ref name) = *name
        {
            nbt.put_string("custom_name", name.clone());
        }
        Some(nbt)
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

impl SkullBlockEntity {
    pub const ID: &'static str = "minecraft:skull";
    #[must_use]
    pub const fn new(position: BlockPos) -> Self {
        Self {
            position,
            note_block_sound: Mutex::const_new(None),
            profile: Mutex::const_new(None),
            custom_name: Mutex::const_new(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::SkullBlockEntity;
    use crate::block::entities::BlockEntity;
    use pumpkin_nbt::compound::NbtCompound;
    use pumpkin_util::math::position::BlockPos;

    #[test]
    fn custom_name_round_trips_through_nbt() {
        let pos = BlockPos::new(3, 70, 5);
        let entity = SkullBlockEntity::new(pos);
        *futures::executor::block_on(entity.custom_name.lock()) =
            Some("{\"text\":\"Steve\"}".to_string());

        let mut nbt = NbtCompound::new();
        futures::executor::block_on(entity.write_nbt(&mut nbt));
        let loaded = SkullBlockEntity::from_nbt(&nbt, pos);

        assert_eq!(
            *futures::executor::block_on(loaded.custom_name.lock()),
            Some("{\"text\":\"Steve\"}".to_string())
        );
    }

    #[test]
    fn absent_custom_name_stays_absent() {
        let entity = SkullBlockEntity::new(BlockPos::new(0, 0, 0));
        let mut nbt = NbtCompound::new();
        futures::executor::block_on(entity.write_nbt(&mut nbt));
        assert!(nbt.get_string("custom_name").is_none());
    }
}
