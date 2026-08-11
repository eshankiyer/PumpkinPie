// Legacy invariant checks retained for vanilla behavior; migrate these paths before removing this allow.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use std::sync::Arc;
use std::sync::atomic::Ordering;

use pumpkin_data::entity::EntityType;
use pumpkin_data::world::WorldEvent;
use pumpkin_nbt::compound::NbtCompound;

use crate::entity::mob::zombie::{
    ZombieEntityBase, drowned::DrownedEntity, transfer_conversion_team,
    transfer_conversion_topology,
};
use crate::entity::{
    Entity, EntityBase, EntityBaseFuture, NBTStorage, NbtFuture,
    mob::{Mob, MobEntity},
};

/// `Zombie::inWaterTime` threshold (`Zombie.java` `tick`, `if (this.inWaterTime >= 600)`):
/// ticks the zombie must have `isEyeInFluid(FluidTags.WATER)` before it starts converting.
/// Same value as `Husk`'s own underwater-conversion timer (`husk.rs`'s
/// `WATER_TICKS_TO_START_CONVERSION`) -- both are driven by the same base `Zombie::tick`.
const WATER_TICKS_TO_START_CONVERSION: i32 = 600;
/// `Zombie::startUnderWaterConversion(300)` (`Zombie.java` `tick`).
const CONVERSION_TICKS: i32 = 300;

pub struct ZombieEntity {
    pub(crate) entity: Arc<ZombieEntityBase>,
}

impl ZombieEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let entity = ZombieEntityBase::new(entity);
        let zombie = Self { entity };
        Arc::new(zombie)
    }

    /// `Zombie::doUnderWaterConversion` (`Zombie.java:240-245`): replaces this zombie with a
    /// `Drowned` at the same position. Mirrors `HuskEntity::finish_conversion`'s simplified
    /// copy set (position/velocity/rotation/age/custom name/active effects) -- there is no
    /// generic `Mob::convertTo` here (equipment/leash/passenger transfer), so those are not
    /// carried over.
    async fn finish_conversion(&self) {
        let old_entity = self.get_entity();
        let world = old_entity.world.load().clone();
        let pos = old_entity.pos.load();

        let new_entity = Entity::new(world.clone(), pos, &EntityType::DROWNED);
        let drowned = DrownedEntity::new(new_entity);
        self.entity.copy_conversion_state_to(&drowned.entity).await;

        let new_entity = drowned.get_entity();
        new_entity.velocity.store(old_entity.velocity.load());
        new_entity.yaw.store(old_entity.yaw.load());
        new_entity.pitch.store(old_entity.pitch.load());
        new_entity.on_ground.store(
            old_entity.on_ground.load(Ordering::Relaxed),
            Ordering::Relaxed,
        );
        new_entity
            .age
            .store(old_entity.age.load(Ordering::Relaxed), Ordering::Relaxed);
        new_entity.invulnerable.store(
            old_entity.invulnerable.load(Ordering::Relaxed),
            Ordering::Relaxed,
        );
        if let Some(name) = &**old_entity.custom_name.load() {
            new_entity.set_custom_name(name.clone());
        }
        new_entity.custom_name_visible.store(
            old_entity.custom_name_visible.load(Ordering::Relaxed),
            Ordering::Relaxed,
        );

        let effects: Vec<_> = self
            .entity
            .mob_entity
            .living_entity
            .active_effects
            .lock()
            .await
            .values()
            .cloned()
            .collect();
        let new_living = &drowned.get_mob_entity().living_entity;
        for effect in effects {
            new_living.add_effect(effect).await;
        }

        transfer_conversion_topology(
            old_entity,
            new_entity,
            drowned.clone() as Arc<dyn EntityBase>,
        )
        .await;
        transfer_conversion_team(world.as_ref(), self, drowned.as_ref()).await;

        world.spawn_entity(drowned).await;
        // Zombie.java:242 gates this on `!isSilent()`, which has no equivalent field here.
        world.sync_world_event(
            WorldEvent::SoundZombieToDrowned,
            old_entity.block_pos.load(),
            0,
        );

        old_entity.remove().await;
    }
}

impl NBTStorage for ZombieEntity {
    /// `Zombie::addAdditionalSaveData` (`Zombie.java:399-405`):
    /// `putInt("InWaterTime", isInWater() ? inWaterTime : -1)` and
    /// `putInt("DrownedConversionTime", isUnderWaterConverting() ? conversionTime : -1)`.
    /// `conversion_time` is already `-1` except while actively converting, so writing it
    /// unconditionally already matches the gated vanilla value.
    fn write_nbt<'a>(&'a self, nbt: &'a mut NbtCompound) -> NbtFuture<'a, ()> {
        Box::pin(async {
            self.entity.write_nbt(nbt).await;
        })
    }

    fn read_nbt_non_mut<'a>(&'a self, nbt: &'a NbtCompound) -> NbtFuture<'a, ()> {
        Box::pin(async {
            self.entity.read_nbt_non_mut(nbt).await;
        })
    }
}

impl Mob for ZombieEntity {
    fn get_mob_entity(&self) -> &MobEntity {
        &self.entity.mob_entity
    }

    fn can_break_doors(&self) -> bool {
        self.entity.can_break_doors()
    }

    /// Delegates the shared zombie-family spawn initialization, including the door-breaking
    /// roll inherited from `Zombie`.
    fn mob_init_data_tracker(&self) -> EntityBaseFuture<'_, ()> {
        Box::pin(async move {
            self.entity.mob_init_data_tracker().await;
            self.entity.initialize_can_break_doors().await;
        })
    }

    /// `Zombie::tick` (`Zombie.java:212-233`): the base-Zombie half of the underwater
    /// conversion timer that `Husk` also drives (see `HuskEntity::mob_tick`), this time
    /// converting into `Drowned` instead of plain `Zombie`.
    fn mob_tick<'a>(&'a self, _caller: &'a Arc<dyn EntityBase>) -> EntityBaseFuture<'a, ()> {
        Box::pin(async move {
            if self
                .entity
                .mob_entity
                .living_entity
                .dead
                .load(Ordering::Relaxed)
            {
                return;
            }

            let converting_time = self.entity.conversion_time.load(Ordering::Relaxed);
            if converting_time >= 0 {
                let new_time = converting_time - 1;
                self.entity
                    .conversion_time
                    .store(new_time, Ordering::Relaxed);
                if new_time < 0 {
                    self.finish_conversion().await;
                }
            } else if self
                .entity
                .mob_entity
                .living_entity
                .entity
                .touching_water
                .load(Ordering::Relaxed)
            {
                let new_time = self.entity.in_water_time.fetch_add(1, Ordering::Relaxed) + 1;
                if new_time >= WATER_TICKS_TO_START_CONVERSION {
                    self.entity.in_water_time.store(0, Ordering::Relaxed);
                    self.entity
                        .conversion_time
                        .store(CONVERSION_TICKS, Ordering::Relaxed);
                }
            } else {
                self.entity.in_water_time.store(-1, Ordering::Relaxed);
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{CONVERSION_TICKS, WATER_TICKS_TO_START_CONVERSION};

    #[test]
    fn matches_vanilla_zombie_to_drowned_thresholds() {
        // `Zombie.java:223-224`: `if (this.inWaterTime >= 600) { startUnderWaterConversion(300); }`
        assert_eq!(WATER_TICKS_TO_START_CONVERSION, 600);
        assert_eq!(CONVERSION_TICKS, 300);
    }
}
