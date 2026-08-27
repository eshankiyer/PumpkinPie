use std::sync::Arc;
use std::sync::Weak;
use std::sync::atomic::{AtomicI32, Ordering};

use pumpkin_data::damage::DamageType;
use pumpkin_data::entity::EntityType;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::particle::Particle;
use pumpkin_data::{
    effect::StatusEffect,
    tag::{self, Taggable},
};
use pumpkin_util::math::vector3::Vector3;
use rand::RngExt;

use pumpkin_protocol::java::client::play::Metadata;

use crate::entity::{
    Entity, EntityBase, EntityBaseFuture, NBTStorage, NbtFuture,
    ai::goal::{
        escape_danger::EscapeDangerGoal, follow_mob::FollowMobGoal, follow_owner::FollowOwnerGoal,
        land_on_owners_shoulder::LandOnOwnersShoulderGoal, look_at_entity::LookAtEntityGoal,
        sit::SitGoal, swim::SwimGoal, wander_around::WanderAroundGoal,
    },
    mob::{Mob, MobEntity},
    player::Player,
};

/// Duration in ticks of the poison a parrot gets from eating a cookie, matching
/// vanilla `Parrot.mobInteract`.
const COOKIE_POISON_DURATION: i32 = 900;

/// `ShoulderRidingEntity.RIDE_COOLDOWN` (`ShoulderRidingEntity.java:14`).
const RIDE_COOLDOWN: i32 = 100;

/// Represents a Parrot, a passive flying mob that can mimic nearby mob sounds.
///
/// Wiki: <https://minecraft.wiki/w/Parrot>
pub struct ParrotEntity {
    pub mob_entity: MobEntity,
    /// `ShoulderRidingEntity.rideCooldownCounter` (`ShoulderRidingEntity.java:15,35-38`):
    /// vanilla increments this every tick from `ShoulderRidingEntity.tick()`; this codebase
    /// has no per-tick hook on `ParrotEntity` to mirror that exactly, so it is incremented
    /// once per `can_sit_on_shoulder` check instead (called from
    /// `LandOnOwnersShoulderGoal::can_start`, which the goal selector re-evaluates near every
    /// tick while the goal is inactive) -- a freshly spawned or respawned-from-shoulder parrot
    /// still starts at 0, giving the same net ~100-tick post-spawn cooldown.
    ride_cooldown_counter: AtomicI32,
}

impl ParrotEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let mob_entity = MobEntity::new(entity);
        let parrot = Self {
            mob_entity,
            ride_cooldown_counter: AtomicI32::new(0),
        };
        let mob_arc = Arc::new(parrot);
        let mob_weak: Weak<dyn Mob> = {
            let mob_arc: Arc<dyn Mob> = mob_arc.clone();
            Arc::downgrade(&mob_arc)
        };

        {
            let mut goal_selector = mob_arc
                .mob_entity
                .goals_selector
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);

            // `Parrot.registerGoals` (`Parrot.java:162-171`).
            goal_selector.add_goal(0, EscapeDangerGoal::new(1.25));
            goal_selector.add_goal(0, Box::new(SwimGoal::default()));
            goal_selector.add_goal(
                1,
                LookAtEntityGoal::with_default(mob_weak, &EntityType::PLAYER, 8.0),
            );
            goal_selector.add_goal(2, SitGoal::new());
            goal_selector.add_goal(2, FollowOwnerGoal::new(1.0, 5.0, 1.0));
            // `Parrot.ParrotWanderGoal` only overrides the flying-navigation position search;
            // this codebase has no flying-stroll variant, so the water-avoiding stroll stands in.
            goal_selector.add_goal(2, Box::new(WanderAroundGoal::new_water_avoiding(1.0)));
            // `Parrot.java:168`.
            goal_selector.add_goal(3, LandOnOwnersShoulderGoal::new());
            // `Parrot.java:169` -- priority 3 `FollowMobGoal(this, 1.0, 3.0F, 7.0F)`.
            goal_selector.add_goal(3, FollowMobGoal::new(1.0, 3.0, 7.0));
        };

        mob_arc
    }

    /// `ShoulderRidingEntity.canSitOnShoulder` (`ShoulderRidingEntity.java:41-43`).
    pub fn can_sit_on_shoulder(&self) -> bool {
        self.ride_cooldown_counter.fetch_add(1, Ordering::Relaxed) > RIDE_COOLDOWN
    }

    /// `ShoulderRidingEntity.setEntityOnShoulder` (`ShoulderRidingEntity.java:45-56`): saves
    /// this parrot into the given player's shoulder slot and discards the live entity.
    pub async fn set_entity_on_shoulder(&self, player: &Player) -> bool {
        let mut nbt = pumpkin_nbt::compound::NbtCompound::new();
        self.write_nbt(&mut nbt).await;
        nbt.put_string(
            "id",
            format!(
                "minecraft:{}",
                self.mob_entity
                    .living_entity
                    .entity
                    .entity_type
                    .resource_name
            ),
        );

        if player.set_entity_on_shoulder(nbt).await {
            self.mob_entity.living_entity.entity.remove().await;
            true
        } else {
            false
        }
    }

    /// Feeds the parrot a cookie: it is poisoned and then killed, as in vanilla
    /// `Parrot.mobInteract`.
    async fn eat_cookie(&self, player: &Arc<Player>, item_stack: &mut ItemStack) {
        item_stack.decrement_unless_creative(player.gamemode.load(), 1);

        self.mob_entity
            .living_entity
            .add_effect(pumpkin_data::potion::Effect {
                effect_type: &StatusEffect::POISON,
                duration: COOKIE_POISON_DURATION,
                amplifier: 0,
                ambient: false,
                show_particles: true,
                show_icon: true,
                blend: true,
            })
            .await;

        // Vanilla guards this call with `player.isCreative() || !this.isInvulnerable()`,
        // but `hurt` re-checks invulnerability itself and `player_attack` doesn't bypass
        // it, so the guard only skips a call that would do nothing anyway.
        self.damage_with_context(
            self,
            f32::MAX,
            DamageType::PLAYER_ATTACK,
            None,
            Some(player.as_ref()),
            Some(player.as_ref()),
        )
        .await;
    }
}

impl NBTStorage for ParrotEntity {
    /// `TamableAnimal.addAdditionalSaveData`: owner UUID plus the ordered-to-sit flag.
    /// Without these a tamed parrot reverted to wild on reload.
    fn write_nbt<'a>(
        &'a self,
        nbt: &'a mut pumpkin_nbt::compound::NbtCompound,
    ) -> NbtFuture<'a, ()> {
        Box::pin(async move {
            self.mob_entity.living_entity.write_nbt(nbt).await;
            if let Some(owner) = self.mob_entity.owner.load() {
                nbt.put_uuid("Owner", owner);
            }
            nbt.put_bool("Sitting", self.mob_entity.is_ordered_to_sit());
        })
    }

    fn read_nbt_non_mut<'a>(
        &'a self,
        nbt: &'a pumpkin_nbt::compound::NbtCompound,
    ) -> NbtFuture<'a, ()> {
        Box::pin(async move {
            self.mob_entity.living_entity.read_nbt_non_mut(nbt).await;
            if let Some(owner) = nbt.get_uuid("Owner") {
                self.mob_entity.set_owner(owner);
            }
            if let Some(sitting) = nbt.get_bool("Sitting") {
                self.mob_entity.set_ordered_to_sit(sitting);
            }
        })
    }
}

/// Vanilla `TamableAnimal` flag byte: bit 0 sitting, bit 2 tame.
const fn tame_flags_byte(sitting: bool, tamed: bool) -> u8 {
    let mut flags = 0u8;
    if sitting {
        flags |= 0x01;
    }
    if tamed {
        flags |= 0x04;
    }
    flags
}

impl ParrotEntity {
    fn tame_flags(&self) -> u8 {
        tame_flags_byte(
            self.mob_entity.is_ordered_to_sit(),
            self.mob_entity.is_tamed(),
        )
    }

    fn sync_tame_flags(&self) {
        self.mob_entity.living_entity.entity.send_meta_data(
            &[Metadata::new(
                pumpkin_data::tracked_data::parrot::TAMEABLE_FLAGS,
                self.tame_flags(),
            )],
            None,
        );
    }
}

impl Mob for ParrotEntity {
    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    fn mob_init_data_tracker(&self) -> EntityBaseFuture<'_, ()> {
        Box::pin(async move {
            self.sync_tame_flags();
            self.mob_entity.living_entity.entity.send_meta_data(
                &[Metadata::new(
                    pumpkin_data::tracked_data::parrot::OWNER_UUID,
                    self.mob_entity.owner.load(),
                )],
                None,
            );
        })
    }

    fn mob_interact<'a>(
        &'a self,
        player: &'a Arc<Player>,
        item_stack: &'a mut ItemStack,
    ) -> EntityBaseFuture<'a, bool> {
        Box::pin(async move {
            if item_stack
                .item
                .has_tag(&tag::Item::MINECRAFT_PARROT_POISONOUS_FOOD)
            {
                self.eat_cookie(player, item_stack).await;
                return true;
            }

            let entity = &self.mob_entity.living_entity.entity;
            if !self.mob_entity.is_tamed()
                && item_stack.item.has_tag(&tag::Item::MINECRAFT_PARROT_FOOD)
            {
                item_stack.decrement_unless_creative(player.gamemode.load(), 1);

                let world = entity.world.load();
                let pos = entity.pos.load() + Vector3::new(0.0, f64::from(entity.height()), 0.0);

                if self.get_random().random_range(0..10) == 0 {
                    self.mob_entity.set_owner(player.gameprofile.id);
                    self.sync_tame_flags();
                    world.spawn_particle(pos, Vector3::new(0.5, 0.5, 0.5), 1.0, 7, Particle::Heart);
                } else {
                    world.spawn_particle(pos, Vector3::new(0.5, 0.5, 0.5), 1.0, 7, Particle::Smoke);
                }

                return true;
            }

            // `Parrot.mobInteract` (`Parrot.java:281-286`): a grounded, tamed parrot owned by
            // this player toggles its ordered-to-sit flag on an empty-handed/other-item click.
            // `Parrot.isFlying` (`Parrot.java:453-455`) is `!onGround()`.
            if entity.on_ground.load(std::sync::atomic::Ordering::Relaxed)
                && self.mob_entity.is_tamed()
                && self.mob_entity.owner.load() == Some(player.gameprofile.id)
            {
                let sitting = !self.mob_entity.is_ordered_to_sit();
                self.mob_entity.set_ordered_to_sit(sitting);
                self.sync_tame_flags();
                return true;
            }

            self.mob_entity
                .mob_interact(player, item_stack, self.can_be_leashed())
                .await
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{COOKIE_POISON_DURATION, tame_flags_byte};
    use pumpkin_data::item::Item;
    use pumpkin_data::tag::{self, Taggable};

    /// The interaction is gated on the vanilla `parrot_poisonous_food` tag rather than
    /// on a hardcoded cookie id, so check the tag actually resolves the way the
    /// interaction assumes.
    #[test]
    fn cookie_is_poisonous_parrot_food() {
        assert!(Item::COOKIE.has_tag(&tag::Item::MINECRAFT_PARROT_POISONOUS_FOOD));
    }

    /// Seeds tame a parrot in vanilla and must not reach the poison branch.
    #[test]
    fn parrot_food_is_not_poisonous() {
        assert!(!Item::WHEAT_SEEDS.has_tag(&tag::Item::MINECRAFT_PARROT_POISONOUS_FOOD));
        assert!(!Item::COOKED_CHICKEN.has_tag(&tag::Item::MINECRAFT_PARROT_POISONOUS_FOOD));
    }

    #[test]
    fn poison_lasts_45_seconds() {
        assert_eq!(COOKIE_POISON_DURATION, 900);
    }

    /// `TamableAnimal` packs sitting into bit 0 and tame into bit 2 of the same byte, so a
    /// sitting tamed parrot must send `0x05`, not `0x03`.
    #[test]
    fn tame_flag_bits() {
        assert_eq!(tame_flags_byte(false, false), 0x00);
        assert_eq!(tame_flags_byte(true, false), 0x01);
        assert_eq!(tame_flags_byte(false, true), 0x04);
        assert_eq!(tame_flags_byte(true, true), 0x05);
    }
}
