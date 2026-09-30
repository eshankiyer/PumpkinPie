use std::sync::atomic::Ordering::Relaxed;

use pumpkin_data::sound::Sound;
use pumpkin_util::math::vector3::Vector3;
use rand::RngExt;

use crate::entity::mob::Mob;

pub mod allay;
pub mod animal;
pub mod armadillo;
pub mod axolotl;
pub mod bee;
pub mod camel;
pub mod cat;
pub mod chicken;
pub mod cod;
pub mod copper_golem;
pub mod cow;
pub mod dolphin;
pub mod donkey;
pub mod equine;
pub mod fish_variant;
pub mod fox;
pub mod frog;
pub mod glow_squid;
pub mod goat;
pub mod happy_ghast;
pub mod horse;
pub mod iron_golem;
pub mod llama;
pub mod mooshroom;
pub mod mule;
pub mod nautilus;
pub mod ocelot;
pub mod panda;
pub mod parrot;
pub mod pig;
pub mod polar_bear;
pub mod pufferfish;
pub mod rabbit;
pub mod salmon;
pub mod sheep;
pub mod skeleton_horse;
pub mod sniffer;
pub mod snow_golem;
pub mod squid;
pub mod strider;
pub mod tadpole;
pub mod tamable;
pub mod trader_llama;
pub mod tropical_fish;
pub mod turtle;
pub mod villager;
pub mod wandering_trader;
pub mod wolf;
pub mod zombie_horse;
pub mod zombie_nautilus;

/// `AbstractFish.aiStep` flop (`AbstractFish.java:115-126`): a fish out of water and on the ground
/// hops with a small random horizontal kick and plays its species flop sound. Pumpkin has no
/// `verticalCollision` field, so `on_ground` stands in for it (a grounded entity is pushed into
/// the floor every tick).
pub(crate) fn fish_flop(mob: &dyn Mob, sound: Sound) {
    let entity = mob.get_entity();
    if entity.touching_water.load(Relaxed) || !entity.on_ground.load(Relaxed) {
        return;
    }
    let mut rng = rand::rng();
    let dx = f64::from((rng.random::<f32>() * 2.0 - 1.0) * 0.05);
    let dz = f64::from((rng.random::<f32>() * 2.0 - 1.0) * 0.05);
    entity.add_velocity(Vector3::new(dx, f64::from(0.4f32), dz));
    entity.on_ground.store(false, Relaxed);
    entity.world.load().play_sound_fine(
        sound,
        mob.get_sound_source(),
        &entity.pos.load(),
        1.0,
        mob.get_sound_pitch(),
    );
}
