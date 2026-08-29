// Legacy invariant checks retained for vanilla behavior; migrate these paths before removing this allow.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;
use std::sync::atomic::{AtomicI8, AtomicI32, Ordering};

use pumpkin_data::damage::DamageType;
use pumpkin_data::entity::EntityType;
use pumpkin_data::world::WorldEvent;
use pumpkin_nbt::compound::NbtCompound;

use crate::entity::mob::equipment::RegionalDifficulty;
use crate::entity::mob::zombie::{
    ZombieEntityBase, drowned::DrownedEntity, try_spawn_reinforcements,
};
use crate::entity::{Entity, EntityBase, mob::{Mob, MobEntity}};
use crate::world::World;

/// `Zombie::inWaterTime` threshold (`Zombie.java` `tick`, `if (this.inWaterTime >= 600)`):
/// ticks the zombie must have `isEyeInFluid(FluidTags.WATER)` before it starts converting.
/// Same value as `Husk`'s own underwater-conversion timer (`husk.rs`'s
/// `WATER_TICKS_TO_START_CONVERSION`) -- both are driven by the same base `Zombie::tick`.
const WATER_TICKS_TO_START_CONVERSION: i32 = 600;
/// `Zombie::startUnderWaterConversion(300)` (`Zombie.java` `tick`).
const CONVERSION_TICKS: i32 = 300;

/// `can_break_doors` sentinel: no `CanBreakDoors` NBT loaded yet and no spawn roll made yet.
/// Distinguishes a freshly spawned zombie (roll on first tick) from one restored from a chunk.
const CAN_BREAK_DOORS_UNDECIDED: i8 = -1;
/// `Zombie.BREAK_DOOR_CHANCE` (`Zombie.java:93`), rolled against `finalizeSpawn`'s
/// `difficultyModifier` at `Zombie.java:493`: `random.nextFloat() < difficultyModifier * 0.1F`.
const BREAK_DOOR_CHANCE: f32 = 0.1;

pub struct ZombieEntity {
    entity: Arc<ZombieEntityBase>,
    /// Vanilla `Zombie::inWaterTime`.
    in_water_time: AtomicI32,
    /// Vanilla `Zombie::conversionTime`. `-1` while not converting.
    conversion_time: AtomicI32,
    /// Local spawn/load sentinel; the actual goal and navigator state live on the upstream base.
    can_break_doors: AtomicI8,
}

impl ZombieEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let entity = ZombieEntityBase::new(entity);
        Arc::new(Self {
            entity,
            in_water_time: AtomicI32::new(0),
            conversion_time: AtomicI32::new(-1),
            can_break_doors: AtomicI8::new(CAN_BREAK_DOORS_UNDECIDED),
        })
    }

    #[must_use]
    pub fn with_can_break_doors(entity: Entity, can_break_doors: bool) -> Arc<Self> {
        let entity = ZombieEntityBase::with_can_break_doors(entity, can_break_doors);
        Arc::new(Self {
            entity,
            in_water_time: AtomicI32::new(0),
            conversion_time: AtomicI32::new(-1),
            can_break_doors: AtomicI8::new(i8::from(can_break_doors)),
        })
    }

    /// `Zombie::setCanBreakDoors` (`Zombie.java:156-170`). The upstream zombie base owns the
    /// synchronous selector mutation; this wrapper retains the fork's undecided/load bookkeeping.
    pub fn set_can_break_doors(&self, can_break_doors: bool) {
        self.can_break_doors
            .store(i8::from(can_break_doors), Ordering::Relaxed);
        self.entity.set_can_break_doors(can_break_doors, self);
    }

    /// `Zombie::doUnderWaterConversion` (`Zombie.java:240-245`): replaces this zombie with a
    /// `Drowned` at the same position. Mirrors the existing simplified conversion copy set.
    async fn finish_conversion(&self) {
        let old_entity = self.get_entity();
        let world = old_entity.world.load().clone();
        let pos = old_entity.pos.load();

        let new_entity = Entity::new(world.clone(), pos, &EntityType::DROWNED);
        let drowned = DrownedEntity::new(new_entity);

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
            new_entity.custom_name_visible.store(
                old_entity.custom_name_visible.load(Ordering::Relaxed),
                Ordering::Relaxed,
            );
        }

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

        world.spawn_entity(drowned).await;
        // Zombie.java:242 gates this on `!isSilent()`, which has no equivalent field here.
        world.sync_world_event(
            WorldEvent::SoundZombieToDrowned,
            old_entity.block_pos.load(),
            0,
        );
        old_entity.remove().await;
    }

    fn write_zombie_nbt(&self, nbt: &mut NbtCompound) {
        self.entity.mob_write_nbt(nbt);
        let in_water_time = if self
            .entity
            .mob_entity
            .living_entity
            .entity
            .touching_water
            .load(Ordering::Relaxed)
        {
            self.in_water_time.load(Ordering::Relaxed)
        } else {
            -1
        };
        // `Zombie::addAdditionalSaveData` (`Zombie.java:399-405`): in-water and conversion
        // timers are written even when inactive using their vanilla sentinel values.
        nbt.put_int("InWaterTime", in_water_time);
        nbt.put_int(
            "DrownedConversionTime",
            self.conversion_time.load(Ordering::Relaxed),
        );
        // `Zombie::addAdditionalSaveData` (`Zombie.java:402`): `putBoolean("CanBreakDoors",
        // this.canBreakDoors())`.
        nbt.put_bool(
            "CanBreakDoors",
            self.can_break_doors.load(Ordering::Relaxed) == 1,
        );
    }

    fn read_zombie_nbt(&self, nbt: &NbtCompound) {
        self.entity.mob_read_nbt(nbt);
        self.in_water_time
            .store(nbt.get_int("InWaterTime").unwrap_or(0), Ordering::Relaxed);
        let time = nbt.get_int("DrownedConversionTime").unwrap_or(-1);
        self.conversion_time.store(time, Ordering::Relaxed);
        // `Zombie::readAdditionalSaveData` (`Zombie.java:411`):
        // `setCanBreakDoors(input.getBooleanOr("CanBreakDoors", false))`.
        let can_break = nbt.get_bool("CanBreakDoors").unwrap_or(false);
        self.can_break_doors
            .store(i8::from(can_break), Ordering::Relaxed);
        self.entity.set_can_break_doors(can_break, self);
    }
}

impl Mob for ZombieEntity {
    fn get_mob_entity(&self) -> &MobEntity {
        &self.entity.mob_entity
    }

    fn populate_default_equipment_slots(
        &self,
        world: &Arc<World>,
        difficulty: &RegionalDifficulty,
    ) {
        self.entity.populate_default_equipment_slots(world, difficulty);
    }

    fn populate_default_equipment_enchantments(&self, difficulty: &RegionalDifficulty) {
        self.entity
            .populate_default_equipment_enchantments(difficulty);
    }

    fn mob_write_nbt(&self, nbt: &mut NbtCompound) {
        self.write_zombie_nbt(nbt);
    }

    fn mob_read_nbt(&self, nbt: &NbtCompound) {
        self.read_zombie_nbt(nbt);
    }

    /// Delegates to the upstream zombie base, then performs the fork's fresh-spawn door roll.
    /// `Zombie::finalizeSpawn` rolls at `Zombie.java:493`; `Zombie::handleAttributes` forces a
    /// leader's door breaking at `Zombie.java:556`.
    fn mob_init_data_tracker(&self) {
        self.entity.mob_init_data_tracker();
        if self.can_break_doors.load(Ordering::Relaxed) == CAN_BREAK_DOORS_UNDECIDED {
            let entity = self.get_entity();
            let world = entity.world.load_full();
            let difficulty = RegionalDifficulty::at(&world, entity.pos.load());
            let roll = BREAK_DOOR_CHANCE * difficulty.special_multiplier;
            self.set_can_break_doors(rand::random::<f32>() < roll);
        }
        if self.entity.is_leader.load(Ordering::Relaxed) {
            self.set_can_break_doors(true);
        }
    }

    /// `Zombie::hurtServer`'s reinforcement half (`Zombie.java:288-340`). The upstream helper is
    /// synchronous now, but the hook remains at the same post-damage point.
    fn on_damage(&self, _damage_type: DamageType, source: Option<&dyn EntityBase>) {
        try_spawn_reinforcements(&self.entity.mob_entity, source);
    }

    /// `Zombie::tick` (`Zombie.java:212-233`): the base-Zombie underwater conversion timer,
    /// converting into `Drowned` rather than plain `Zombie`.
    fn mob_tick(&self, _caller: &dyn EntityBase) {
        if self
            .entity
            .mob_entity
            .living_entity
            .dead
            .load(Ordering::Relaxed)
        {
            return;
        }

        let converting_time = self.conversion_time.load(Ordering::Relaxed);
        if converting_time >= 0 {
            let new_time = converting_time - 1;
            self.conversion_time.store(new_time, Ordering::Relaxed);
            if new_time < 0 {
                futures::executor::block_on(self.finish_conversion());
            }
        } else if self
            .entity
            .mob_entity
            .living_entity
            .entity
            .touching_water
            .load(Ordering::Relaxed)
        {
            let new_time = self.in_water_time.fetch_add(1, Ordering::Relaxed) + 1;
            if new_time >= WATER_TICKS_TO_START_CONVERSION {
                self.in_water_time.store(0, Ordering::Relaxed);
                self.conversion_time
                    .store(CONVERSION_TICKS, Ordering::Relaxed);
            }
        } else {
            self.in_water_time.store(-1, Ordering::Relaxed);
        }
    }
}

impl ZombieEntity {
    #[must_use]
    pub fn can_break_doors(&self) -> bool {
        self.entity.can_break_doors()
    }

    #[must_use]
    pub fn is_baby(&self) -> bool {
        self.entity
            .mob_entity
            .living_entity
            .entity
            .age
            .load(Ordering::Relaxed)
            < 0
    }

    pub fn set_baby(&self, baby: bool) {
        let age = if baby { -24000 } else { 0 };
        self.entity
            .mob_entity
            .living_entity
            .entity
            .age
            .store(age, Ordering::Relaxed);
        self.entity.mob_entity.living_entity.entity.send_meta_data(
            &[pumpkin_protocol::java::client::play::Metadata::new(
                pumpkin_data::tracked_data::zombie::BABY,
                baby,
            )],
            None,
        );
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
