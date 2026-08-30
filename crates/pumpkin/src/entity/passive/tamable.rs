use std::sync::atomic::{AtomicBool, Ordering::Relaxed};

use crossbeam::atomic::AtomicCell;
use pumpkin_data::data_component_impl::FoodImpl;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_protocol::java::client::play::Metadata;
use uuid::Uuid;

use crate::entity::{living::LivingEntity, passive::animal::Animal, player::Player};

// TamableAnimal.java:135-140 (shared healing helper used by Wolf.java:453-459 and
// Cat.java:423-428). The Rust interaction hooks already own the hand stack, so consuming it
// directly is the equivalent of usePlayerItem.
pub(crate) fn feed(
    player: &Player,
    item_stack: &mut ItemStack,
    living: &LivingEntity,
    healing_factor: f32,
    default_heal: f32,
    eating_sound: Option<pumpkin_data::sound::Sound>,
) {
    let nutrition = item_stack
        .get_data_component::<FoodImpl>()
        .map(|food| food.nutrition);
    let healing = feed_healing_amount(nutrition, healing_factor, default_heal);
    item_stack.decrement_unless_creative(player.gamemode.load(), 1);
    living.heal(healing);
    if let Some(sound) = eating_sound {
        let entity = &living.entity;
        let world = entity.world.load();
        world.play_sound(
            sound,
            pumpkin_data::sound::SoundCategory::Neutral,
            &entity.pos.load(),
        );
    }
}

fn feed_healing_amount(nutrition: Option<i32>, healing_factor: f32, default_heal: f32) -> f32 {
    nutrition.map_or(default_heal, |nutrition| healing_factor * nutrition as f32)
}

pub const SITTING_FLAG: u8 = 1;
pub const TAME_FLAG: u8 = 4;
pub const TELEPORT_WHEN_DISTANCE_IS_SQ: f64 = 144.0;

pub struct TamableData {
    pub is_tame: AtomicBool,
    pub ordered_to_sit: AtomicBool,
    pub owner: AtomicCell<Option<Uuid>>,
}

impl Default for TamableData {
    fn default() -> Self {
        Self {
            is_tame: AtomicBool::new(false),
            ordered_to_sit: AtomicBool::new(false),
            owner: AtomicCell::new(None),
        }
    }
}

pub trait TamableAnimal: Animal {
    fn get_tamable_data(&self) -> &TamableData;

    fn is_tame(&self) -> bool {
        self.get_tamable_data().is_tame.load(Relaxed)
    }

    fn set_tame(&self, tame: bool) {
        let mob_entity = self.get_mob_entity();
        let entity = &mob_entity.living_entity.entity;
        self.get_tamable_data().is_tame.store(tame, Relaxed);
        let mut flags = if self.is_in_sitting_pose() {
            SITTING_FLAG
        } else {
            0
        };
        if tame {
            flags |= TAME_FLAG;
        }
        entity.send_meta_data(
            &[Metadata::new(
                pumpkin_data::tracked_data::tamable_animal::DATA_FLAGS_ID,
                flags as i8,
            )],
            None,
        );
    }

    fn is_in_sitting_pose(&self) -> bool {
        self.get_tamable_data().ordered_to_sit.load(Relaxed)
    }

    fn set_in_sitting_pose(&self, sitting: bool) {
        let mob_entity = self.get_mob_entity();
        let entity = &mob_entity.living_entity.entity;
        self.get_tamable_data()
            .ordered_to_sit
            .store(sitting, Relaxed);
        let mut flags = if sitting { SITTING_FLAG } else { 0 };
        if self.is_tame() {
            flags |= TAME_FLAG;
        }
        entity.send_meta_data(
            &[Metadata::new(
                pumpkin_data::tracked_data::tamable_animal::DATA_FLAGS_ID,
                flags as i8,
            )],
            None,
        );
    }

    fn is_ordered_to_sit(&self) -> bool {
        self.get_tamable_data().ordered_to_sit.load(Relaxed)
    }

    fn set_ordered_to_sit(&self, ordered_to_sit: bool) {
        self.set_in_sitting_pose(ordered_to_sit);
    }

    fn get_owner(&self) -> Option<Uuid> {
        self.get_tamable_data().owner.load()
    }

    fn set_owner(&self, owner: Option<Uuid>) {
        let mob_entity = self.get_mob_entity();
        let entity = &mob_entity.living_entity.entity;
        self.get_tamable_data().owner.store(owner);
        entity.send_meta_data(
            &[Metadata::new(
                pumpkin_data::tracked_data::tamable_animal::DATA_OWNERUUID_ID,
                owner,
            )],
            None,
        );
    }

    fn is_owned_by(&self, player_uuid: &Uuid) -> bool {
        self.get_owner().is_some_and(|id| id == *player_uuid)
    }

    fn tame(&self, player_id: Uuid) {
        self.set_tame(true);
        self.set_owner(Some(player_id));
    }

    fn spawn_taming_particles(&self, success: bool) {
        use pumpkin_data::particle::Particle;
        use pumpkin_util::math::vector3::Vector3;
        let mob_entity = self.get_mob_entity();
        let entity = &mob_entity.living_entity.entity;
        let world = entity.world.load();
        let pos = entity.pos.load();
        let particle = if success {
            Particle::Heart
        } else {
            Particle::Smoke
        };
        world.spawn_particle(
            pos + Vector3::new(0.0, f64::from(entity.height()) * 0.5, 0.0),
            Vector3::new(0.5, 0.5, 0.5),
            0.02,
            7,
            particle,
        );
    }

    fn write_tamable_nbt(&self, nbt: &mut NbtCompound) {
        if let Some(owner) = self.get_owner() {
            nbt.put_uuid("Owner", owner);
        }
        nbt.put_bool("Sitting", self.is_ordered_to_sit());
    }

    fn read_tamable_nbt(&self, nbt: &NbtCompound) {
        if let Some(owner) = nbt.get_uuid("Owner") {
            self.set_owner(Some(owner));
            self.set_tame(true);
        } else if let Some(is_tame) = nbt.get_bool("IsTame") {
            self.set_tame(is_tame);
        }
        let sitting = nbt
            .get_bool("Sitting")
            .or_else(|| nbt.get_byte("Sitting").map(|b| b != 0))
            .unwrap_or(false);
        self.set_ordered_to_sit(sitting);
    }
}

#[cfg(test)]
mod tests {
    use super::feed_healing_amount;

    // TamableAnimal.java:135-140 selects food nutrition when present and otherwise uses the
    // caller's default healing amount.
    #[test]
    fn feed_healing_uses_component_nutrition_or_default() {
        assert_eq!(feed_healing_amount(Some(3), 2.0, 2.0), 6.0);
        assert_eq!(feed_healing_amount(None, 2.0, 2.0), 2.0);
    }
}
