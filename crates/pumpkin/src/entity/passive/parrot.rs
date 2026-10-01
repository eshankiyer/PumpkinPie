use std::sync::Arc;
use std::sync::Weak;
use std::sync::atomic::{AtomicI32, Ordering};

use pumpkin_data::attributes::Attributes;
use pumpkin_data::damage::DamageType;
use pumpkin_data::entity::EntityType;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::particle::Particle;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_data::{
    effect::StatusEffect,
    tag::{self, Taggable},
};
use pumpkin_util::math::vector3::Vector3;
use rand::RngExt;

use pumpkin_protocol::codec::var_int::VarInt;
use pumpkin_protocol::java::client::play::Metadata;
use pumpkin_util::Difficulty;

use crate::entity::{
    Entity, EntityBase, EntityBaseFuture, NBTStorage, NbtFuture,
    ai::control::flying_move_control::FlyingMoveControl,
    ai::pathfinder::node::PathType,
    ai::goal::{
        escape_danger::EscapeDangerGoal, follow_mob::FollowMobGoal, follow_owner::FollowOwnerGoal,
        land_on_owners_shoulder::LandOnOwnersShoulderGoal, look_at_entity::LookAtEntityGoal,
        sit::SitGoal, swim::SwimGoal, wander_around::WanderAroundGoal,
    },
    mob::{Mob, MobEntity},
    player::Player,
};
use crate::world::World;

/// Duration in ticks of the poison a parrot gets from eating a cookie, matching
/// vanilla `Parrot.mobInteract`.
const COOKIE_POISON_DURATION: i32 = 900;

/// `ShoulderRidingEntity.RIDE_COOLDOWN` (`ShoulderRidingEntity.java:14`).
const RIDE_COOLDOWN: i32 = 100;

/// `Parrot.MOB_SOUND_MAP` (`Parrot.java:83-126`), keyed by entity type id. `None` is
/// `SoundEvents.EMPTY` (the happy ghast), which a parrot can pick but which makes no sound.
const MOB_SOUND_MAP: [(u16, Option<Sound>); 42] = [
    (EntityType::BLAZE.id, Some(Sound::EntityParrotImitateBlaze)),
    (EntityType::BOGGED.id, Some(Sound::EntityParrotImitateBogged)),
    (EntityType::BREEZE.id, Some(Sound::EntityParrotImitateBreeze)),
    (EntityType::CAMEL_HUSK.id, Some(Sound::EntityParrotImitateCamelHusk)),
    (EntityType::CAVE_SPIDER.id, Some(Sound::EntityParrotImitateSpider)),
    (EntityType::CREAKING.id, Some(Sound::EntityParrotImitateCreaking)),
    (EntityType::CREEPER.id, Some(Sound::EntityParrotImitateCreeper)),
    (EntityType::DROWNED.id, Some(Sound::EntityParrotImitateDrowned)),
    (EntityType::ELDER_GUARDIAN.id, Some(Sound::EntityParrotImitateElderGuardian)),
    (EntityType::ENDER_DRAGON.id, Some(Sound::EntityParrotImitateEnderDragon)),
    (EntityType::ENDERMITE.id, Some(Sound::EntityParrotImitateEndermite)),
    (EntityType::EVOKER.id, Some(Sound::EntityParrotImitateEvoker)),
    (EntityType::GHAST.id, Some(Sound::EntityParrotImitateGhast)),
    (EntityType::HAPPY_GHAST.id, None),
    (EntityType::GUARDIAN.id, Some(Sound::EntityParrotImitateGuardian)),
    (EntityType::HOGLIN.id, Some(Sound::EntityParrotImitateHoglin)),
    (EntityType::HUSK.id, Some(Sound::EntityParrotImitateHusk)),
    (EntityType::ILLUSIONER.id, Some(Sound::EntityParrotImitateIllusioner)),
    (EntityType::MAGMA_CUBE.id, Some(Sound::EntityParrotImitateMagmaCube)),
    (EntityType::PARCHED.id, Some(Sound::EntityParrotImitateParched)),
    (EntityType::PHANTOM.id, Some(Sound::EntityParrotImitatePhantom)),
    (EntityType::PIGLIN.id, Some(Sound::EntityParrotImitatePiglin)),
    (EntityType::PIGLIN_BRUTE.id, Some(Sound::EntityParrotImitatePiglinBrute)),
    (EntityType::PILLAGER.id, Some(Sound::EntityParrotImitatePillager)),
    (EntityType::RAVAGER.id, Some(Sound::EntityParrotImitateRavager)),
    (EntityType::SHULKER.id, Some(Sound::EntityParrotImitateShulker)),
    (EntityType::SILVERFISH.id, Some(Sound::EntityParrotImitateSilverfish)),
    (EntityType::SKELETON.id, Some(Sound::EntityParrotImitateSkeleton)),
    (EntityType::SLIME.id, Some(Sound::EntityParrotImitateSlime)),
    (EntityType::SPIDER.id, Some(Sound::EntityParrotImitateSpider)),
    (EntityType::STRAY.id, Some(Sound::EntityParrotImitateStray)),
    (EntityType::VEX.id, Some(Sound::EntityParrotImitateVex)),
    (EntityType::VINDICATOR.id, Some(Sound::EntityParrotImitateVindicator)),
    (EntityType::WARDEN.id, Some(Sound::EntityParrotImitateWarden)),
    (EntityType::WITCH.id, Some(Sound::EntityParrotImitateWitch)),
    (EntityType::WITHER.id, Some(Sound::EntityParrotImitateWither)),
    (EntityType::WITHER_SKELETON.id, Some(Sound::EntityParrotImitateWitherSkeleton)),
    (EntityType::ZOGLIN.id, Some(Sound::EntityParrotImitateZoglin)),
    (EntityType::ZOMBIE.id, Some(Sound::EntityParrotImitateZombie)),
    (EntityType::ZOMBIE_HORSE.id, Some(Sound::EntityParrotImitateZombieHorse)),
    (EntityType::ZOMBIE_NAUTILUS.id, Some(Sound::EntityParrotImitateZombieNautilus)),
    (EntityType::ZOMBIE_VILLAGER.id, Some(Sound::EntityParrotImitateZombieVillager)),
];

/// `Parrot.getImitatedSound` (`Parrot.java:340-342`): the mapped sound, or `PARROT_AMBIENT`
/// for a type outside the map.
fn imitated_sound(type_id: u16) -> Option<Sound> {
    MOB_SOUND_MAP
        .iter()
        .find(|(id, _)| *id == type_id)
        .map_or(Some(Sound::EntityParrotAmbient), |(_, sound)| *sound)
}

/// `Parrot.getPitch` (`Parrot.java:375-377`).
#[must_use]
pub fn get_pitch() -> f32 {
    (rand::random::<f32>() - rand::random::<f32>()).mul_add(0.2, 1.0)
}

/// `Parrot.getAmbient` (`Parrot.java:331-338`): outside peaceful, a 1/1000 roll imitates a
/// uniformly chosen map key; otherwise the parrot's own call. `None` is the silent happy ghast.
pub fn get_ambient(world: &World) -> Option<Sound> {
    if world.level_info.load().difficulty != Difficulty::Peaceful
        && rand::random_range(0..1000) == 0
    {
        let (type_id, _) = MOB_SOUND_MAP[rand::random_range(0..MOB_SOUND_MAP.len())];
        imitated_sound(type_id)
    } else {
        Some(Sound::EntityParrotAmbient)
    }
}

/// `Parrot.imitateNearbyMobs` (`Parrot.java:232-249`).
///
/// With a 1/2 roll, plays the imitation of a random mob in the map within 20 blocks of
/// `entity`. Returns whether a (non-silent) mob
/// was imitated; a happy ghast is chosen like any other and imitates silence.
pub fn imitate_nearby_mobs(world: &World, entity: &Entity, source: SoundCategory) -> bool {
    if !entity.is_alive() || entity.is_silent() || rand::random_range(0..2) != 0 {
        return false;
    }
    let mobs: Vec<_> = world
        .get_entities_at_box(&entity.bounding_box.load().expand(20.0, 20.0, 20.0))
        .into_iter()
        .filter(|candidate| {
            candidate.get_mob().is_some()
                && MOB_SOUND_MAP
                    .iter()
                    .any(|(id, _)| *id == candidate.get_entity().entity_type.id)
        })
        .collect();
    if mobs.is_empty() {
        return false;
    }
    let mob = &mobs[rand::random_range(0..mobs.len())];
    if mob.get_entity().is_silent() {
        return false;
    }
    if let Some(sound) = imitated_sound(mob.get_entity().entity_type.id) {
        world.play_sound_fine(sound, source, &entity.pos.load(), 0.7, get_pitch());
    }
    true
}

/// `Parrot.Variant` ids (`Parrot.java:517-522`): `red_blue`, `blue`, `green`, `yellow_blue`, `gray`.
const VARIANT_COUNT: i32 = 5;
/// Sentinel for a variant not rolled yet (`Parrot.finalizeSpawn` has not run); not a vanilla id.
const VARIANT_UNSET: i32 = -1;

/// Maps a `Parrot.Variant` serialized name to its id; an unknown name keeps `DEFAULT`.
fn variant_id_from_name(name: &str) -> i32 {
    match name.strip_prefix("minecraft:").unwrap_or(name) {
        "blue" => 1,
        "green" => 2,
        "yellow_blue" => 3,
        "gray" => 4,
        _ => 0,
    }
}

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
    /// `Parrot.DATA_VARIANT_ID` (`Parrot.java:77`); `VARIANT_UNSET` until `finalizeSpawn`'s
    /// random roll (done in `mob_init_data_tracker`, after any NBT read) or an NBT/egg value.
    variant: AtomicI32,
}

impl ParrotEntity {
    /// `Parrot` constructor (`Parrot.java:136-142`) and `Parrot.createNavigation`
    /// (`Parrot.java:181-187`). The fire maluses are omitted as for the bee: Pumpkin splits
    /// vanilla's fire path types into `DangerFire`/`DamageFire` with different semantics.
    fn install_flight(mob_entity: &MobEntity) {
        *mob_entity
            .move_control
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            Box::new(FlyingMoveControl::new(10.0, false));
        let mut navigator = mob_entity
            .navigator
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        navigator.set_flying(true);
        navigator.set_can_float(true);
        navigator.set_can_open_doors(false);
        navigator.set_pathfinding_malus(PathType::Cocoa, -1.0);
    }

    pub fn new(entity: Entity) -> Arc<Self> {
        let mob_entity = MobEntity::new(entity);
        Self::install_flight(&mob_entity);
        let parrot = Self {
            mob_entity,
            ride_cooldown_counter: AtomicI32::new(0),
            variant: AtomicI32::new(VARIANT_UNSET),
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
            goal_selector.add_goal(0, EscapeDangerGoal::new_tamable(1.25));
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
            // `Parrot.addAdditionalSaveData` (`Parrot.java:441-445`): `Variant` as a legacy int.
            nbt.put_int("Variant", self.variant.load(Ordering::Relaxed).max(0));
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
            // `Parrot.readAdditionalSaveData` (`Parrot.java:447-451`): clamped, default `RED_BLUE`.
            self.variant.store(
                nbt.get_int("Variant").unwrap_or(0).clamp(0, VARIANT_COUNT - 1),
                Ordering::Relaxed,
            );
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

    /// `Parrot.getAmbientSound` (`Parrot.java:326-329`).
    fn get_ambient_sound(&self) -> Option<Sound> {
        get_ambient(&self.mob_entity.living_entity.entity.world.load())
    }

    /// `Parrot.playStepSound` (`Parrot.java:354-357`): `PARROT_STEP` at 0.15 volume, 1.0 pitch,
    /// regardless of the block.
    fn get_step_sound(&self) -> Option<Sound> {
        Some(Sound::EntityParrotStep)
    }

    /// `Parrot.getVoicePitch` (`Parrot.java:370-373`).
    fn get_sound_pitch(&self) -> f32 {
        get_pitch()
    }

    /// `Parrot.omnidirectionalAirMover` (`Parrot.java:457-460`): `LivingEntity.travelInAir`
    /// (`LivingEntity.java:2483`) damps vertical velocity with the horizontal air drag.
    fn get_mob_y_velocity_drag(&self) -> Option<f64> {
        Some(crate::entity::living::modified_friction(
            0.91,
            self.mob_entity
                .living_entity
                .get_attribute_value(&Attributes::AIR_DRAG_MODIFIER),
        ))
    }

    /// `Parrot.aiStep` (`Parrot.java:189-202`): a 1/400 chance per tick to imitate nearby mobs,
    /// and `calculateFlapping`'s server-visible part (`Parrot.java:225-227`), which slows a
    /// falling airborne parrot's descent to 0.6 of its vertical velocity.
    fn mob_tick<'a>(&'a self, _caller: &'a Arc<dyn EntityBase>) -> EntityBaseFuture<'a, ()> {
        Box::pin(async move {
            let entity = &self.mob_entity.living_entity.entity;
            if self.get_random().random_range(0..400) == 0 {
                imitate_nearby_mobs(&entity.world.load(), entity, self.get_sound_source());
            }
            if !entity.on_ground.load(Ordering::Relaxed) {
                let velocity = entity.velocity.load();
                if velocity.y < 0.0 {
                    entity
                        .velocity
                        .store(Vector3::new(velocity.x, velocity.y * 0.6, velocity.z));
                }
            }
        })
    }

    /// `Parrot.applyImplicitComponent` (`Parrot.java:425-433`) for `PARROT_VARIANT`.
    fn mob_set_variant_name(&self, name: &str) {
        self.variant
            .store(variant_id_from_name(name), Ordering::Relaxed);
    }

    fn mob_init_data_tracker(&self) -> EntityBaseFuture<'_, ()> {
        Box::pin(async move {
            // `Parrot.finalizeSpawn` (`Parrot.java:148`): a uniformly random variant, only for
            // a parrot that was neither read from NBT nor given one by a spawn egg.
            let _ = self.variant.compare_exchange(
                VARIANT_UNSET,
                self.get_random().random_range(0..VARIANT_COUNT),
                Ordering::Relaxed,
                Ordering::Relaxed,
            );
            self.mob_entity.living_entity.entity.send_meta_data(
                &[Metadata::new(
                    pumpkin_data::tracked_data::parrot::DATA_VARIANT_ID,
                    VarInt(self.variant.load(Ordering::Relaxed)),
                )],
                None,
            );
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
    use super::{
        COOKIE_POISON_DURATION, MOB_SOUND_MAP, get_pitch, imitated_sound, tame_flags_byte,
        variant_id_from_name,
    };
    use pumpkin_data::entity::EntityType;
    use pumpkin_data::sound::Sound;
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

    #[test]
    fn variant_names_map_to_vanilla_ids() {
        assert_eq!(variant_id_from_name("minecraft:red_blue"), 0);
        assert_eq!(variant_id_from_name("blue"), 1);
        assert_eq!(variant_id_from_name("minecraft:green"), 2);
        assert_eq!(variant_id_from_name("yellow_blue"), 3);
        assert_eq!(variant_id_from_name("gray"), 4);
    }

    #[test]
    fn imitation_table_matches_vanilla() {
        assert_eq!(MOB_SOUND_MAP.len(), 42);
        // Both spiders share one sound, the happy ghast imitates silence, and an unmapped
        // type falls back to the parrot's own call.
        assert_eq!(
            imitated_sound(EntityType::CAVE_SPIDER.id),
            imitated_sound(EntityType::SPIDER.id)
        );
        assert_eq!(imitated_sound(EntityType::HAPPY_GHAST.id), None);
        assert_eq!(
            imitated_sound(EntityType::COW.id),
            Some(Sound::EntityParrotAmbient)
        );
    }

    #[test]
    fn pitch_stays_within_vanilla_range() {
        for _ in 0..100 {
            assert!((0.8..=1.2).contains(&get_pitch()));
        }
    }
}
