use super::BlockEntity;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_nbt::tag::NbtTag;
use pumpkin_util::math::position::BlockPos;
use std::pin::Pin;
use tokio::sync::Mutex;

/// `TheEndGatewayBlockEntity.saveAdditional` (`TheEndGatewayBlockEntity.java:51`) stores the
/// exit position under `exit_portal` with `BlockPos.CODEC`, which is `Codec.INT_STREAM`
/// (`net/minecraft/core/BlockPos.java:33`) - an NBT int array, not an `X`/`Y`/`Z` compound.
fn write_exit_portal(nbt: &mut NbtCompound, pos: BlockPos) {
    nbt.put(
        "exit_portal",
        NbtTag::IntArray(vec![pos.0.x, pos.0.y, pos.0.z]),
    );
}

/// Reads the vanilla `exit_portal` int array, falling back to the `ExitPortal` compound that
/// older Pumpkin saves wrote so those worlds do not silently lose their gateway destination.
fn read_exit_portal(nbt: &NbtCompound) -> Option<BlockPos> {
    if let Some(array) = nbt.get_int_array("exit_portal")
        && let [x, y, z] = array
    {
        return Some(BlockPos::new(*x, *y, *z));
    }
    nbt.get_compound("ExitPortal").map(|c| {
        BlockPos::new(
            c.get_int("X").unwrap_or(0),
            c.get_int("Y").unwrap_or(0),
            c.get_int("Z").unwrap_or(0),
        )
    })
}

pub struct EndGatewayBlockEntity {
    pub position: BlockPos,
    pub age: Mutex<i64>,
    pub exact_teleport: Mutex<bool>,
    pub exit_portal: Mutex<Option<BlockPos>>,
}

impl BlockEntity for EndGatewayBlockEntity {
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
        let age = nbt.get_long("Age").unwrap_or(0);
        let exact_teleport = nbt.get_bool("ExactTeleport").unwrap_or(false);
        let exit_portal = read_exit_portal(nbt);
        Self {
            position,
            age: Mutex::new(age),
            exact_teleport: Mutex::new(exact_teleport),
            exit_portal: Mutex::new(exit_portal),
        }
    }

    fn write_nbt<'a>(
        &'a self,
        nbt: &'a mut NbtCompound,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            nbt.put_long("Age", *self.age.lock().await);
            // `TheEndGatewayBlockEntity.saveAdditional` (:52-54) only emits
            // `ExactTeleport` when it is true.
            if *self.exact_teleport.lock().await {
                nbt.put_bool("ExactTeleport", true);
            }
            if let Some(exit) = self.exit_portal.lock().await.as_ref() {
                write_exit_portal(nbt, *exit);
            }
        })
    }

    fn chunk_data_nbt(&self) -> Option<NbtCompound> {
        let mut nbt = NbtCompound::new();
        nbt.put_long("Age", *self.age.try_lock().ok()?);
        if *self.exact_teleport.try_lock().ok()? {
            nbt.put_bool("ExactTeleport", true);
        }
        if let Ok(exit) = self.exit_portal.try_lock()
            && let Some(ref exit) = *exit
        {
            write_exit_portal(&mut nbt, *exit);
        }
        Some(nbt)
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

impl EndGatewayBlockEntity {
    pub const ID: &'static str = "minecraft:end_gateway";
    #[must_use]
    pub fn new(position: BlockPos) -> Self {
        Self {
            position,
            age: Mutex::new(0),
            exact_teleport: Mutex::new(false),
            exit_portal: Mutex::new(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{EndGatewayBlockEntity, read_exit_portal, write_exit_portal};
    use crate::block::entities::BlockEntity;
    use pumpkin_nbt::compound::NbtCompound;
    use pumpkin_util::math::position::BlockPos;

    fn write(entity: &EndGatewayBlockEntity) -> NbtCompound {
        let mut nbt = NbtCompound::new();
        futures::executor::block_on(entity.write_nbt(&mut nbt));
        nbt
    }

    #[test]
    fn exit_portal_is_written_as_an_int_array() {
        let mut nbt = NbtCompound::new();
        write_exit_portal(&mut nbt, BlockPos::new(100, 75, -8));
        assert_eq!(
            nbt.get_int_array("exit_portal"),
            Some([100, 75, -8].as_ref())
        );
        assert_eq!(read_exit_portal(&nbt), Some(BlockPos::new(100, 75, -8)));
    }

    #[test]
    fn legacy_exit_portal_compound_still_loads() {
        let mut legacy = NbtCompound::new();
        let mut inner = NbtCompound::new();
        inner.put_int("X", 1);
        inner.put_int("Y", 2);
        inner.put_int("Z", 3);
        legacy.put_compound("ExitPortal", inner);
        assert_eq!(read_exit_portal(&legacy), Some(BlockPos::new(1, 2, 3)));
    }

    #[test]
    fn gateway_state_round_trips_through_nbt() {
        let pos = BlockPos::new(0, 64, 0);
        let entity = EndGatewayBlockEntity::new(pos);
        *futures::executor::block_on(entity.age.lock()) = 512;
        *futures::executor::block_on(entity.exact_teleport.lock()) = true;
        *futures::executor::block_on(entity.exit_portal.lock()) = Some(BlockPos::new(-7, 60, 9));

        let nbt = write(&entity);
        let loaded = EndGatewayBlockEntity::from_nbt(&nbt, pos);

        assert_eq!(*futures::executor::block_on(loaded.age.lock()), 512);
        assert!(*futures::executor::block_on(loaded.exact_teleport.lock()));
        assert_eq!(
            *futures::executor::block_on(loaded.exit_portal.lock()),
            Some(BlockPos::new(-7, 60, 9))
        );
    }

    #[test]
    fn exact_teleport_is_omitted_when_false() {
        let entity = EndGatewayBlockEntity::new(BlockPos::new(0, 64, 0));
        assert!(write(&entity).get_bool("ExactTeleport").is_none());
    }
}
