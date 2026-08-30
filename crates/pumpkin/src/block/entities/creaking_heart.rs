use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicI32, Ordering};

use crossbeam::atomic::AtomicCell;
use pumpkin_data::Block;
use pumpkin_data::block_properties::{
    BlockProperties, CreakingHeartLikeProperties, CreakingHeartState,
};
use pumpkin_data::damage::DamageType;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_nbt::tag::NbtTag;
use pumpkin_util::math::position::BlockPos;
use pumpkin_world::world::BlockFlags;
use rand::RngExt;
use uuid::Uuid;

use crate::block::blocks::creaking_heart::{creaking_active, has_required_logs};
use crate::entity::mob::creaking::CreakingEntity;
use crate::world::World;

use super::BlockEntity;

/// `net.minecraft.world.level.block.entity.CreakingHeartBlockEntity`.
///
/// The creaking-mob half (`spawnProtector`, `removeProtector`, `creakingHurt`,
/// `spreadResin` and the hurt-emitter particles) is deliberately not ported: it needs
/// `Creaking.setTransient`/`tearDown`/`playerIsStuckInYou` and a heart<->mob binding that
/// Pumpkin's `CreakingEntity` does not have. The persisted `creaking` UUID is still
/// round-tripped so an existing binding survives save/load once that half lands, and
/// `computeAnalogOutputSignal` returns vanilla's 0 branch for a heart with no live
/// protector -- which is every heart until then.
pub struct CreakingHeartBlockEntity {
    pub position: BlockPos,
    /// `creakingInfo`, reduced to the persisted UUID arm (`Either.right`).
    pub creaking_uuid: AtomicCell<Option<Uuid>>,
    pub ticker: AtomicI32,
    pub output_signal: AtomicI32,
}

impl BlockEntity for CreakingHeartBlockEntity {
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
        // `CreakingHeartBlockEntity.loadAdditional` (`CreakingHeartBlockEntity.java:348-352`)
        // restores the UUID through setCreakingInfo.
        let creaking_uuid = nbt.get_int_array("creaking").and_then(uuid_from_int_array);
        let entity = Self::new(position);
        entity.set_creaking_uuid(creaking_uuid);
        entity
    }

    fn write_nbt<'a>(
        &'a self,
        nbt: &'a mut NbtCompound,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            let creaking_uuid = self.creaking_uuid.load();
            if let Some(uuid) = creaking_uuid {
                nbt.put("creaking", uuid_to_int_array(uuid));
            }
        })
    }

    /// `CreakingHeartBlockEntity.serverTick`, minus the protector spawn/upkeep branch.
    /// Vanilla's `CreakingHeartBlock.getTicker` returns null while the heart is UPROOTED,
    /// so an uprooted heart does not tick here either.
    fn tick<'a>(&'a self, world: &'a Arc<World>) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            let (block, state) = world.get_block_and_state(&self.position);
            if block.id != Block::CREAKING_HEART.id {
                return;
            }
            let mut props = CreakingHeartLikeProperties::from_state_id(state.id, block);
            if props.creaking_heart_state == CreakingHeartState::Uprooted {
                return;
            }

            let computed = self.compute_analog_output_signal();
            if self.output_signal.swap(computed, Ordering::Relaxed) != computed {
                world.update_comparators(&self.position, block).await;
            }

            // `if (entity.ticker-- < 0)`: post-decrement, so the body runs on the tick the
            // pre-decrement value is negative. Reseeds to `nextInt(5) + 20`.
            if self.ticker.fetch_sub(1, Ordering::Relaxed) >= 0 {
                return;
            }
            self.ticker
                .store(20 + rand::rng().random_range(0..5), Ordering::Relaxed);

            // updateCreakingState: an uprooted-eligible heart (no logs, no bound creaking)
            // goes UPROOTED, otherwise it tracks the creaking_active environment attribute.
            let new_state = if has_required_logs(world, &self.position, props.axis)
                || self.creaking_uuid.load().is_some()
            {
                if creaking_active(world).await {
                    CreakingHeartState::Awake
                } else {
                    CreakingHeartState::Dormant
                }
            } else {
                CreakingHeartState::Uprooted
            };

            if new_state != props.creaking_heart_state {
                props.creaking_heart_state = new_state;
                world
                    .set_block_state(
                        &self.position,
                        props.to_state_id(block),
                        BlockFlags::NOTIFY_ALL,
                    )
                    .await;
            }
        })
    }

    fn chunk_data_nbt(&self) -> Option<NbtCompound> {
        // getUpdateTag == saveCustomOnly.
        let mut nbt = NbtCompound::new();
        let creaking_uuid = self.creaking_uuid.load();
        if let Some(uuid) = creaking_uuid {
            nbt.put("creaking", uuid_to_int_array(uuid));
        }
        Some(nbt)
    }

    fn on_block_replaced<'a>(
        self: Arc<Self>,
        world: Arc<World>,
        _position: BlockPos,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>>
    where
        Self: 'a,
    {
        Box::pin(async move {
            self.remove_protector_on_removal(&world).await;
        })
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

impl CreakingHeartBlockEntity {
    pub const ID: &'static str = "minecraft:creaking_heart";

    #[must_use]
    pub const fn new(position: BlockPos) -> Self {
        Self {
            position,
            creaking_uuid: AtomicCell::new(None),
            ticker: AtomicI32::new(0),
            output_signal: AtomicI32::new(0),
        }
    }

    pub fn set_creaking_uuid(&self, uuid: Option<Uuid>) {
        self.creaking_uuid.store(uuid);
    }

    /// `getAnalogOutputSignal`: the value cached by the last tick, not a fresh compute.
    pub fn get_analog_output_signal(&self) -> u8 {
        self.output_signal.load(Ordering::Relaxed).clamp(0, 15) as u8
    }

    /// `computeAnalogOutputSignal`: `15 - floor(clamp(dist, 0, 32) / 32 * 15)` for a live
    /// protector, else 0. Always the 0 branch until the creaking binding is implemented.
    pub const fn compute_analog_output_signal(&self) -> i32 {
        0
    }

    /// `CreakingHeartBlockEntity.isProtector`. Since `spawn_protector`/`remove_protector` (the
    /// mechanics that would reassign or clear `creaking_uuid` mid-game) aren't ported yet, this
    /// is exactly "is this the creaking whose UUID we're holding" -- there is no live `Creaking`
    /// reference stored here to compare identity against, only the persisted UUID.
    pub fn is_protector(&self, creaking_uuid: Uuid) -> bool {
        self.creaking_uuid.load() == Some(creaking_uuid)
    }

    /// `CreakingHeartBlockEntity.removeProtector(null)`
    /// (`CreakingHeartBlockEntity.java:309-327`): removal without a damage source tears down
    /// the bound creaking and clears the persisted binding atomically.
    pub async fn remove_protector_on_removal(&self, world: &Arc<World>) {
        self.remove_protector(world, None).await;
    }

    /// `CreakingHeartBlockEntity.removeProtector` (`CreakingHeartBlockEntity.java:314-327`):
    /// damage removes the binding, starts the creaking death effects, and sets health to zero;
    /// removal without damage tears the protector down immediately.
    pub async fn remove_protector(&self, world: &Arc<World>, damage_type: Option<DamageType>) {
        let Some(uuid) = self.creaking_uuid.swap(None) else {
            return;
        };
        if let Some(entity) = world.get_entity_by_uuid(uuid)
            && let Some(creaking) = entity.cast_any().downcast_ref::<CreakingEntity>()
        {
            if damage_type.is_some() {
                creaking.creaking_death_effects();
                creaking.mob_entity.living_entity.set_health(0.0);
            } else {
                creaking.tear_down().await;
            }
        }
    }

    /// `CreakingHeartBlockEntity.removeProtector(damageSource)`
    /// (`CreakingHeartBlockEntity.java:314-327`): a player break uses the death
    /// effects path before the heart block is removed.
    pub fn remove_protector_after_player_attack(&self, world: &Arc<World>) {
        let Some(uuid) = self.creaking_uuid.swap(None) else {
            return;
        };
        if let Some(entity) = world.get_entity_by_uuid(uuid)
            && let Some(creaking) = entity.cast_any().downcast_ref::<CreakingEntity>()
        {
            creaking.creaking_death_effects();
            creaking.mob_entity.living_entity.health.store(0.0);
        }
    }

    /// `CreakingHeartBlockEntity.creakingHurt`. The particle/resin-spread effects this drives
    /// in vanilla are deliberately not ported here (same scope cut as this file's existing
    /// doc comment already calls out for `spawnProtector`/`removeProtector`/`spreadResin`);
    /// only the hurt sound is played.
    pub fn creaking_hurt(&self, world: &Arc<World>) {
        world.play_sound(
            Sound::BlockCreakingHeartHurt,
            SoundCategory::Blocks,
            &self.position.to_f64(),
        );
    }
}

/// `UUIDUtil.CODEC`: four big-endian ints, most-significant first.
const fn uuid_from_int_array(values: &[i32]) -> Option<Uuid> {
    let &[a, b, c, d] = values else { return None };
    Some(Uuid::from_u128(
        ((a as u32 as u128) << 96)
            | ((b as u32 as u128) << 64)
            | ((c as u32 as u128) << 32)
            | (d as u32 as u128),
    ))
}

fn uuid_to_int_array(u: Uuid) -> NbtTag {
    let v = u.as_u128();
    NbtTag::IntArray(vec![
        (v >> 96) as i32,
        ((v >> 64) & 0xFFFF_FFFF) as i32,
        ((v >> 32) & 0xFFFF_FFFF) as i32,
        (v & 0xFFFF_FFFF) as i32,
    ])
}

#[cfg(test)]
mod tests {
    use pumpkin_nbt::compound::NbtCompound;
    use pumpkin_util::math::position::BlockPos;
    use uuid::Uuid;

    use super::{BlockEntity, CreakingHeartBlockEntity, uuid_to_int_array};

    #[test]
    fn load_restores_creaking_uuid() {
        // `CreakingHeartBlockEntity.loadAdditional`/`saveAdditional`
        // (`CreakingHeartBlockEntity.java:348-359`) round-trip the creaking UUID.
        let uuid = Uuid::from_u128(0x0011_2233_4455_6677_8899_aabb_ccdd_eeff);
        let mut nbt = NbtCompound::new();
        nbt.put("creaking", uuid_to_int_array(uuid));

        let entity = CreakingHeartBlockEntity::from_nbt(&nbt, BlockPos::new(1, 2, 3));

        assert_eq!(entity.creaking_uuid.load(), Some(uuid));
    }
}
