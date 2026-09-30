use std::sync::Arc;

use pumpkin_data::particle::Particle;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_data::{Block, item_stack::ItemStack};

use crate::entity::{
    EntityBaseFuture,
    ageable::{AgeableMob, speed_up_seconds_when_feeding},
    mob::Mob,
    player::Player,
};
use pumpkin_protocol::bedrock::server::actor_event::ActorEventType;
use pumpkin_util::math::{position::BlockPos, vector3::Vector3};

pub trait Animal: Mob {
    fn is_food(&self, item_stack: &ItemStack) -> bool;

    /// Vanilla `Animal.getWalkTargetValue`: grass is preferred, otherwise the
    /// position's light-dependent pathfinding cost is used.
    fn get_walk_target_value(&self, pos: &BlockPos) -> f64 {
        let world = self.get_entity().world.load();
        if world.get_block(&pos.down()).id == Block::GRASS_BLOCK.id {
            return 10.0;
        }

        let brightness = f32::from(world.get_max_local_raw_brightness(pos)) / 15.0;
        let curved_brightness = brightness / (4.0 - 3.0 * brightness);
        f64::from(
            curved_brightness + world.dimension.ambient_light * (1.0 - curved_brightness) - 0.5,
        )
    }

    /// `ZombieHorse.canAgeUp` overrides this to `false` so babies never grow up from food.
    /// Every other current `Animal` implementor keeps the default (vanilla's own default is
    /// also `true`).
    fn can_age_up(&self) -> bool {
        true
    }

    /// This animal's `AgeableMob` view, for the feeding path (`Animal.mobInteract` calls
    /// `ageUp`). Animals that implement `AgeableMob` override it to return `Some(self)`; the rest
    /// (horses, llamas, cats, ocelots, nautilus) have no age-lock/forced-age state to route through.
    fn as_ageable_mob(&self) -> Option<&dyn AgeableMob> {
        None
    }

    fn play_eating_sound(&self, sound: Sound) {
        let mob_entity = self.get_mob_entity();
        let entity = &mob_entity.living_entity.entity;
        let world = entity.world.load();
        world.play_sound(sound, SoundCategory::Neutral, &entity.pos.load());
    }

    fn write_animal_nbt(&self, nbt: &mut pumpkin_nbt::compound::NbtCompound) {
        let mob_entity = self.get_mob_entity();
        let in_love = mob_entity
            .love_ticks
            .load(std::sync::atomic::Ordering::Relaxed);
        nbt.put_int("InLove", in_love);
        if let Some(uuid) = mob_entity.breeder.load() {
            nbt.put_uuid("LoveCause", uuid);
        }
    }

    fn read_animal_nbt(&self, nbt: &pumpkin_nbt::compound::NbtCompound) {
        let mob_entity = self.get_mob_entity();
        let in_love = nbt.get_int("InLove").unwrap_or(0);
        let love_cause = nbt.get_uuid("LoveCause");
        mob_entity.set_love_ticks(in_love, love_cause);
    }

    fn animal_interact<'a>(
        &'a self,
        player: &'a Arc<Player>,
        item_stack: &'a mut ItemStack,
        ambient_sound: Sound,
    ) -> EntityBaseFuture<'a, bool> {
        Box::pin(async move {
            let mob_entity = self.get_mob_entity();
            if self.is_food(item_stack) {
                let age = mob_entity
                    .living_entity
                    .entity
                    .age
                    .load(std::sync::atomic::Ordering::Relaxed);

                if age >= 0 && mob_entity.is_breeding_ready() && !mob_entity.is_in_love() {
                    item_stack.decrement_unless_creative(player.gamemode.load(), 1);

                    mob_entity.set_love_ticks(600, Some(player.gameprofile.id));
                    let entity = &mob_entity.living_entity.entity;
                    let world = entity.world.load();
                    let pos = entity.pos.load();

                    world.send_entity_status(
                        entity,
                        pumpkin_data::entity::EntityStatus::InLoveHearts,
                        Some(ActorEventType::InLoveHearts),
                    );

                    world.spawn_particle(
                        pos + Vector3::new(0.0, f64::from(entity.height()), 0.0),
                        Vector3::new(0.5, 0.5, 0.5),
                        1.0,
                        7,
                        Particle::Heart,
                    );
                    world.play_sound(ambient_sound, SoundCategory::Neutral, &entity.pos.load());
                    return true;
                }

                // Vanilla `Animal.mobInteract` (`Animal.java:141-146`): `canAgeUp()` is
                // `isBaby() && !isAgeLocked()`; an age-locked baby falls through to `Mob`.
                let ageable = self.as_ageable_mob();
                let can_age_up = self.can_age_up()
                    && ageable.map_or(age < 0, AgeableMob::can_age_up);
                if can_age_up {
                    item_stack.decrement_unless_creative(player.gamemode.load(), 1);
                    let seconds = speed_up_seconds_when_feeding(-age);
                    if let Some(ageable) = ageable {
                        ageable.age_up(seconds, true);
                    } else {
                        // No age state to route through (see `as_ageable_mob`): same arithmetic
                        // as `AgeableMob.ageUp` without the forced-age bookkeeping.
                        let new_age = (age + seconds * 20).min(0);
                        mob_entity
                            .living_entity
                            .entity
                            .age
                            .store(new_age, std::sync::atomic::Ordering::Relaxed);
                    }

                    let entity = &mob_entity.living_entity.entity;
                    let world = entity.world.load();
                    let pos = entity.pos.load();

                    world.spawn_particle(
                        pos + Vector3::new(0.0, f64::from(entity.height()), 0.0),
                        Vector3::new(0.5, 0.5, 0.5),
                        1.0,
                        7,
                        Particle::HappyVillager,
                    );
                    self.play_eating_sound(ambient_sound);
                    return true;
                }
            }

            mob_entity
                .mob_interact(player, item_stack, self.can_be_leashed())
                .await
        })
    }
}
