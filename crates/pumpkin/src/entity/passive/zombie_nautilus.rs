// Port of `net.minecraft.world.entity.animal.nautilus.ZombieNautilus`, plus the parts of
// `AbstractNautilus` it inherits.
//
// Vanilla drives this mob entirely from a Brain/Memory/Activity ladder
// (`ZombieNautilus.customServerAiStep`, ZombieNautilus.java:74-84, delegating to
// `ZombieNautilusAi.getActivities`/`updateActivity`). Pumpkin has no Brain/Memory/Activity
// system, so — following the precedent set by `entity/mob/warden.rs` — the state the brain
// would own is kept as plain fields on `ZombieNautilusEntity`, ticked directly from
// `mob_tick`, and targeting is driven through the existing Goal system.
//
// Ported here:
//
// - Attributes (ZombieNautilus.java:51-53). Nothing to write: the generated
//   `EntityType::ZOMBIE_NAUTILUS.attributes` table already carries MAX_HEALTH 15,
//   ATTACK_DAMAGE 3, KNOCKBACK_RESISTANCE 0.3 from `AbstractNautilus.createAttributes`
//   (AbstractNautilus.java:111-117) and MOVEMENT_SPEED 1.1 from this class's override.
// - The underwater/on-land sound pairs (ZombieNautilus.java:86-119), as pure functions with
//   tests. Unlike the living nautilus these have no baby branch, because `canBeABaby` is
//   false (ZombieNautilus.java:182-185). How much of that is REACHABLE differs per sound,
//   because this codebase resolves sounds through several unrelated paths:
//   - hurt: wired, via a new `ZOMBIE_NAUTILUS` arm in `LivingEntity::hurt_sound`
//     (`entity/living.rs`), alongside the existing slime/magma-cube/sulfur-cube arms. The
//     generated `ZOMBIE_NAUTILUS.hurt_sound` is `None`, so without it the mob would play
//     `entity.generic.hurt`.
//   - dash-ready: wired, played from `mob_tick` when the cooldown hits zero.
//   - eat: wired, played by the taming branch of `mob_interact`.
//   - ambient: wired, through the shared `Mob::tick_ambient_sound` cadence
//     (`entity/mob/mod.rs`, from `Mob.baseTick`) as well as the `animal_interact`
//     love-mode fallback.
//   - swim: wired, through `LivingEntity::tick_swim_sound` (`entity/living.rs`, from
//     `Entity.applyMovementEmissionAndPlaySound`).
//   - death: not played by the server. Pumpkin broadcasts `EntityStatus::Death` from
//     `LivingEntity::on_death`, and a vanilla client plays `getDeathSound` itself on that
//     entity event (LivingEntity.java:1477 broadcast, LivingEntity.java:2063-2067 handler),
//     so the sound does reach players. Vanilla additionally plays it server-side
//     (LivingEntity.java:1255); that second play is not replicated, because the generated
//     `EntityType` carries no `death_sound` field and doing it only for the mobs with a
//     hand-written death sound would make those alone double-play.
// - `sunProtectionSlot` = BODY (ZombieNautilus.java:59-62). This is live: `zombie_nautilus`
//   is in the `minecraft:burn_in_daylight` entity-type tag, which is what
//   `Mob::tick_sun_burn` gates on. Nothing overrode `Mob::sun_protection_slot` before this
//   file, so zombie horses (`ZombieHorse.java:186`) are still on the HEAD default — a
//   pre-existing gap left alone here.
// - `canBeLeashed` (ZombieNautilus.java:177-180), approximated below.
// - The `AbstractNautilus` tick: the rider's Breath of the Nautilus effect
//   (AbstractNautilus.java:260-268), the dash-cooldown countdown and dash-ready sound
//   (AbstractNautilus.java:301-310), and the bubble trail (AbstractNautilus.java:270-292).
// - Taming and riding (AbstractNautilus.java:396-444), and the tame/food split in `isFood`
//   (AbstractNautilus.java:97-100). Note the sibling `nautilus.rs` hardcodes a food list
//   that matches neither `#minecraft:nautilus_food` nor `#minecraft:nautilus_taming_items`
//   (it accepts a nautilus shell, which is in neither); this file reads the real tags
//   instead. Fixing the sibling is out of scope for closing this entity.
// - Breeding: deliberately none. `getBreedOffspring` returns null
//   (ZombieNautilus.java:55-58) and `ZombieNautilusAi.initIdleActivity`
//   (ZombieNautilusAi.java:50-68) has no `AnimalMakeLove`, unlike `NautilusAi`
//   (NautilusAi.java:85). Love mode itself is still reachable — vanilla's
//   `Animal.mobInteract` (Animal.java:139-148) sets it for any `isFood` item — so the
//   fallback to `animal_interact` is correct. No `BreedGoal` is registered, and the only
//   readers of `MobEntity::love_ticks` outside `ai/goal/breed.rs` are `horse_breed.rs`,
//   `turtle_lay_egg.rs`, `turtle_travel.rs` and the countdown in `mob/mod.rs:2223`, none of
//   which spawn offspring for this species — so the null offspring holds by construction.
// - Variant (ZombieNautilus.java:43-45, 121-145): temperate/warm, registry order taken from
//   `ZombieNautilusVariants.bootstrap` (ZombieNautilusVariants.java:25-28) and confirmed
//   against the generated `zombie_nautilus_variant` static registry.
//
// Explicitly NOT ported, with reasons:
//
// - `ZombieNautilusAi`'s FIGHT activity, `ChargeAttack(80, ..., 0.5F, 2.0F, 12.0, 11.0,
//   ZOMBIE_NAUTILUS_DASH)` (ZombieNautilusAi.java:70-81). Pumpkin has no charge/dash attack
//   behaviour with that shape (`vex_charge_attack.rs` is Vex-specific: it flies to a fixed
//   point and has no cooldown/knockback parameters). `MeleeAttackGoal` at the same 0.5 speed
//   is registered below as the honest approximation: the mob still closes and hits, but it
//   does not dash, does not apply the 2.0 knockback impulse, and has no 80-tick charge
//   cooldown or 12-block charge range.
// - Unprovoked hunting of `#minecraft:nautilus_hostiles`
//   (`NautilusAi.findNearestValidAttackTarget`, NautilusAi.java:115-138): it is gated on the
//   `ATTACK_TARGET_COOLDOWN` memory, seeded to a uniform 2400-3600 ticks in
//   `AbstractNautilus.finalizeSpawn` (AbstractNautilus.java:466-473 -> NautilusAi.java:45,
//   58-60) and counted down by a `CountDownCooldownTicks` behaviour in CORE. Pumpkin has no
//   memory-with-expiry primitive, and an ungated `ActiveTargetGoal` would make zombie
//   nautiluses attack roughly 2500x more often than vanilla, so this is left out rather
//   than approximated badly. The provoked half — `AbstractNautilus.hurtServer` calling
//   `NautilusAi.setAngerTarget` (AbstractNautilus.java:451-459) — IS ported, as
//   `RevengeGoal`, which is the goal-system analogue. The 400-tick anger expiry
//   (NautilusAi.java:143) has no equivalent: `RevengeGoal` drops the target on
//   `TrackTargetGoal`'s own timeout instead.
// - `AbstractNautilus.checkRestriction` (AbstractNautilus.java:245-252), which pins a tame,
//   unleashed, unridden nautilus to a 16- or 32-block home radius. Pumpkin's `MobEntity` has
//   no home-position/restriction concept at all, so there is nothing to set.
// - The custom mount inventory (`HasCustomInventoryScreen`,
//   AbstractNautilus.java:484-522). `getInventoryColumns()` is 0 on the base class, so the
//   screen is empty for both nautilus species anyway; the sibling carries an unused
//   inventory field, which is not copied here.
// - Natural spawning. `SpawnPlacements.java:136` registers `checkNautilusSpawnRules` for
//   `NAUTILUS` only — there is no `ZOMBIE_NAUTILUS` entry — so no spawn predicate arm is
//   added in `entity/type.rs`, unlike the living nautilus.
// - Biome-driven variant selection on spawn (`finalizeSpawn`, ZombieNautilus.java:169-175):
//   Pumpkin has no `SpawnContext`/`SpawnPrioritySelectors` equivalent, so a zombie nautilus
//   always starts temperate and only changes variant from NBT or `/summon`.

use crossbeam::atomic::AtomicCell;
use std::sync::{
    Arc, Weak,
    atomic::{AtomicBool, AtomicI32, Ordering},
};
use uuid::Uuid;

use pumpkin_data::data_component_impl::EquipmentSlot;
use pumpkin_data::effect::StatusEffect;
use pumpkin_data::entity::{EntityStatus, EntityType};
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::particle::Particle;
use pumpkin_data::potion::Effect;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_data::tag::{self, Taggable};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_protocol::codec::var_int::VarInt;
use pumpkin_protocol::java::client::play::Metadata;
use pumpkin_util::math::vector3::Vector3;

use crate::entity::{
    Entity, EntityBase, EntityBaseFuture, NBTStorage, NbtFuture,
    ai::goal::{
        look_around::RandomLookAroundGoal, look_at_entity::LookAtEntityGoal,
        melee_attack::MeleeAttackGoal, revenge::RevengeGoal, tempt::TemptGoal,
        wander_around::WanderAroundGoal,
    },
    mob::{Mob, MobEntity},
    passive::animal::Animal,
    player::Player,
};

/// `SensorType.NAUTILUS_TEMPTATIONS` -> `NautilusAi.getTemptations` (NautilusAi.java:155-157)
/// is the `#minecraft:nautilus_food` tag. `TemptGoal` only takes a static item list, not a
/// tag, so the tag's ten members are spelled out the same way `zombie_horse.rs` does it.
const TEMPT_ITEMS: &[&Item] = &[
    &Item::COD,
    &Item::COOKED_COD,
    &Item::SALMON,
    &Item::COOKED_SALMON,
    &Item::PUFFERFISH,
    &Item::TROPICAL_FISH,
    &Item::PUFFERFISH_BUCKET,
    &Item::COD_BUCKET,
    &Item::SALMON_BUCKET,
    &Item::TROPICAL_FISH_BUCKET,
];

/// `AbstractNautilus.DASH_COOLDOWN_TICKS` (AbstractNautilus.java:75).
const DASH_COOLDOWN_TICKS: i32 = 40;
/// `AbstractNautilus.tick` clears the dash flag once the cooldown drops below this
/// (AbstractNautilus.java:301-303); 40 - 35 = the 5-tick `DASH_MINIMUM_DURATION_TICKS`.
const DASH_MINIMUM_HOLD_THRESHOLD: i32 = 35;
/// `AbstractNautilus.EFFECT_DURATION` / `EFFECT_REFRESH_RATE` (AbstractNautilus.java:68-69).
const EFFECT_DURATION: i32 = 60;
const EFFECT_REFRESH_RATE: i64 = 40;
/// `ZombieNautilusAi.SPEED_WHEN_ATTACKING` (ZombieNautilusAi.java:26).
const SPEED_WHEN_ATTACKING: f64 = 0.5;
/// `ZombieNautilusAi.SPEED_MULTIPLIER_WHEN_TEMPTED` (ZombieNautilusAi.java:25).
const SPEED_MULTIPLIER_WHEN_TEMPTED: f64 = 0.9;
/// `ZombieNautilusAi.SPEED_MULTIPLIER_WHEN_IDLING_IN_WATER` (ZombieNautilusAi.java:24).
const SPEED_MULTIPLIER_WHEN_IDLING_IN_WATER: f64 = 1.0;

/// Registry order of `zombie_nautilus_variant`, from `ZombieNautilusVariants.bootstrap`
/// (ZombieNautilusVariants.java:25-28): temperate first (also `DEFAULT`, line 19), then warm.
pub const VARIANT_TEMPERATE: i32 = 0;
pub const VARIANT_WARM: i32 = 1;

/// `ZombieNautilus.getAmbientSound` (ZombieNautilus.java:86-89).
#[must_use]
pub const fn ambient_sound_for(underwater: bool) -> Sound {
    if underwater {
        Sound::EntityZombieNautilusAmbient
    } else {
        Sound::EntityZombieNautilusAmbientLand
    }
}

/// `ZombieNautilus.getHurtSound` (ZombieNautilus.java:91-94).
#[must_use]
pub const fn hurt_sound_for(underwater: bool) -> Sound {
    if underwater {
        Sound::EntityZombieNautilusHurt
    } else {
        Sound::EntityZombieNautilusHurtLand
    }
}

/// `ZombieNautilus.getDeathSound` (ZombieNautilus.java:96-99).
#[must_use]
pub const fn death_sound_for(underwater: bool) -> Sound {
    if underwater {
        Sound::EntityZombieNautilusDeath
    } else {
        Sound::EntityZombieNautilusDeathLand
    }
}

/// `ZombieNautilus.getDashSound` (ZombieNautilus.java:101-104).
#[must_use]
pub const fn dash_sound_for(underwater: bool) -> Sound {
    if underwater {
        Sound::EntityZombieNautilusDash
    } else {
        Sound::EntityZombieNautilusDashLand
    }
}

/// `ZombieNautilus.getDashReadySound` (ZombieNautilus.java:106-109).
#[must_use]
pub const fn dash_ready_sound_for(underwater: bool) -> Sound {
    if underwater {
        Sound::EntityZombieNautilusDashReady
    } else {
        Sound::EntityZombieNautilusDashReadyLand
    }
}

/// `AbstractNautilus.isFood` (AbstractNautilus.java:97-100).
///
/// The `isBaby` half of vanilla's condition is dropped because `ZombieNautilus.canBeABaby` is
/// false (ZombieNautilus.java:182-185), so a zombie nautilus is never a baby.
#[must_use]
pub fn is_food_for(item: &Item, tame: bool) -> bool {
    if tame {
        item.has_tag(&tag::Item::MINECRAFT_NAUTILUS_FOOD)
    } else {
        item.has_tag(&tag::Item::MINECRAFT_NAUTILUS_TAMING_ITEMS)
    }
}

#[must_use]
pub fn variant_id_from_name(name: &str) -> i32 {
    match name.strip_prefix("minecraft:").unwrap_or(name) {
        "warm" => VARIANT_WARM,
        _ => VARIANT_TEMPERATE,
    }
}

#[must_use]
pub const fn variant_name_from_id(id: i32) -> &'static str {
    if id == VARIANT_WARM {
        "minecraft:warm"
    } else {
        "minecraft:temperate"
    }
}

/// A zombie nautilus (`ZombieNautilus.java`, behaviour in `ZombieNautilusAi.java`, shared base
/// in `AbstractNautilus.java`). See the module header for the ported/not-ported split.
///
/// Wiki: <https://minecraft.wiki/w/Zombie_Nautilus>
pub struct ZombieNautilusEntity {
    pub mob_entity: MobEntity,
    pub is_tame: AtomicBool,
    pub owner: AtomicCell<Option<Uuid>>,
    pub is_dashing: AtomicBool,
    pub dash_cooldown: AtomicI32,
    pub is_saddled: AtomicBool,
    pub variant: AtomicI32,
    /// Stand-in for `AbstractNautilus.isAggravated` (AbstractNautilus.java:528-530), which
    /// reads the `ANGRY_AT`/`ATTACK_TARGET` brain memories. Refreshed from `mob_tick` because
    /// `MobEntity::target` sits behind an async mutex while `Mob::can_be_leashed` is sync.
    aggravated: AtomicBool,
    /// Stand-in for `AbstractNautilus.isMobControlled` (AbstractNautilus.java:524-526), for
    /// the same reason.
    mob_controlled: AtomicBool,
}

impl ZombieNautilusEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let mob_entity = MobEntity::new(entity);
        let zombie_nautilus = Self {
            mob_entity,
            is_tame: AtomicBool::new(false),
            owner: AtomicCell::new(None),
            is_dashing: AtomicBool::new(false),
            dash_cooldown: AtomicI32::new(0),
            is_saddled: AtomicBool::new(false),
            variant: AtomicI32::new(VARIANT_TEMPERATE),
            aggravated: AtomicBool::new(false),
            mob_controlled: AtomicBool::new(false),
        };

        let mob_arc = Arc::new(zombie_nautilus);
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

            // `ZombieNautilusAi.initCoreActivity` (ZombieNautilusAi.java:36-48) has NO
            // `AnimalPanic`, unlike `NautilusAi.initCoreActivity` (NautilusAi.java:71), so no
            // `EscapeDangerGoal` is registered here. There is no float/swim goal either:
            // nautiluses live in water.
            // `ZombieNautilusAi.initFightActivity`'s `ChargeAttack` speed (ZombieNautilusAi.java:73),
            // approximated by melee — see the module header.
            goal_selector.add_goal(
                1,
                Box::new(MeleeAttackGoal::new(SPEED_WHEN_ATTACKING, true)),
            );
            // `ZombieNautilusAi.initIdleActivity` priority 1: `FollowTemptation(0.9F)`.
            goal_selector.add_goal(
                2,
                Box::new(TemptGoal::new(
                    SPEED_MULTIPLIER_WHEN_TEMPTED,
                    TEMPT_ITEMS,
                    false,
                )),
            );
            // `ZombieNautilusAi.initIdleActivity` priority 3: `RandomStroll.swim(1.0F)`.
            goal_selector.add_goal(
                3,
                Box::new(WanderAroundGoal::new(SPEED_MULTIPLIER_WHEN_IDLING_IN_WATER)),
            );
            // `LookAtTargetSink(45, 90)` in CORE (ZombieNautilusAi.java:41).
            goal_selector.add_goal(
                4,
                LookAtEntityGoal::with_default(mob_weak, &EntityType::PLAYER, 8.0),
            );
            goal_selector.add_goal(5, Box::new(RandomLookAroundGoal::default()));

            let mut target_selector = mob_arc
                .mob_entity
                .target_selector
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            // `AbstractNautilus.hurtServer` -> `NautilusAi.setAngerTarget`
            // (AbstractNautilus.java:451-459). `check_visibility` is false because
            // `setAngerTarget` gates on `Sensor.isEntityAttackableIgnoringLineOfSight`
            // (NautilusAi.java:140-145), not on line of sight.
            target_selector.add_goal(1, Box::new(RevengeGoal::new(false)));
        };

        mob_arc
    }

    /// Vanilla `Entity.isUnderWater` (Entity.java:1608-1610): the eye must be submerged, not
    /// just the feet. The sibling `nautilus.rs` checks only `touching_water`, which is the
    /// looser `isInWater`.
    fn is_underwater(&self) -> bool {
        let entity = &self.mob_entity.living_entity.entity;
        entity.was_eye_in_water.load(Ordering::Relaxed)
            && entity.touching_water.load(Ordering::Relaxed)
    }

    #[must_use]
    pub fn is_dashing(&self) -> bool {
        self.is_dashing.load(Ordering::Relaxed)
    }

    pub fn set_dashing(&self, dashing: bool) {
        self.is_dashing.store(dashing, Ordering::Relaxed);
        self.mob_entity.living_entity.entity.send_meta_data(
            &[Metadata::new(
                pumpkin_data::tracked_data::zombie_nautilus::DASH,
                dashing,
            )],
            None,
        );
    }

    #[must_use]
    pub fn is_tame(&self) -> bool {
        self.is_tame.load(Ordering::Relaxed)
    }

    pub fn set_tame(&self, tame: bool, owner: Option<Uuid>) {
        self.is_tame.store(tame, Ordering::Relaxed);
        self.owner.store(owner);
    }

    #[must_use]
    pub fn get_variant(&self) -> i32 {
        self.variant.load(Ordering::Relaxed)
    }

    pub fn set_variant(&self, variant: i32) {
        self.variant.store(variant, Ordering::Relaxed);
        self.mob_entity.living_entity.entity.send_meta_data(
            &[Metadata::new(
                pumpkin_data::tracked_data::zombie_nautilus::DATA_VARIANT_ID,
                VarInt(variant),
            )],
            None,
        );
    }

    #[must_use]
    pub fn ambient_sound(&self) -> Sound {
        ambient_sound_for(self.is_underwater())
    }

    #[must_use]
    pub fn get_hurt_sound(&self) -> Sound {
        hurt_sound_for(self.is_underwater())
    }

    #[must_use]
    pub fn get_death_sound(&self) -> Sound {
        death_sound_for(self.is_underwater())
    }

    #[must_use]
    pub fn get_dash_sound(&self) -> Sound {
        dash_sound_for(self.is_underwater())
    }

    #[must_use]
    pub fn get_dash_ready_sound(&self) -> Sound {
        dash_ready_sound_for(self.is_underwater())
    }

    /// `ZombieNautilus.playEatingSound` (ZombieNautilus.java:111-114): a single sound, with no
    /// underwater pair and no baby variant.
    #[must_use]
    pub const fn get_eat_sound(&self) -> Sound {
        Sound::EntityZombieNautilusEat
    }

    /// `ZombieNautilus.getSwimSound` (ZombieNautilus.java:116-119).
    #[must_use]
    pub const fn get_swim_sound(&self) -> Sound {
        Sound::EntityZombieNautilusSwim
    }

    /// `AbstractNautilus.executeRidersJump` (AbstractNautilus.java:343-351), minus the delta
    /// movement impulse, which is applied by the rider-input path this codebase does not have.
    pub fn start_dash_cooldown(&self) {
        self.dash_cooldown
            .store(DASH_COOLDOWN_TICKS, Ordering::Relaxed);
        self.set_dashing(true);
    }
}

impl NBTStorage for ZombieNautilusEntity {
    fn write_nbt<'a>(&'a self, nbt: &'a mut NbtCompound) -> NbtFuture<'a, ()> {
        Box::pin(async move {
            self.mob_entity.living_entity.write_nbt(nbt).await;
            self.write_animal_nbt(nbt);
            nbt.put_bool("IsTame", self.is_tame.load(Ordering::Relaxed));
            nbt.put_bool("Saddled", self.is_saddled.load(Ordering::Relaxed));
            nbt.put_int("DashCooldown", self.dash_cooldown.load(Ordering::Relaxed));
            nbt.put_string(
                "variant",
                variant_name_from_id(self.get_variant()).to_string(),
            );
            if let Some(owner) = self.owner.load() {
                nbt.put_uuid("Owner", owner);
            }
        })
    }

    fn read_nbt_non_mut<'a>(&'a self, nbt: &'a NbtCompound) -> NbtFuture<'a, ()> {
        Box::pin(async move {
            self.mob_entity.living_entity.read_nbt_non_mut(nbt).await;
            self.read_animal_nbt(nbt);
            if let Some(is_tame) = nbt.get_bool("IsTame") {
                self.is_tame.store(is_tame, Ordering::Relaxed);
            }
            if let Some(saddled) = nbt.get_bool("Saddled") {
                self.is_saddled.store(saddled, Ordering::Relaxed);
            }
            if let Some(dash) = nbt.get_int("DashCooldown") {
                self.dash_cooldown.store(dash, Ordering::Relaxed);
            }
            if let Some(variant) = nbt.get_string("variant") {
                self.variant
                    .store(variant_id_from_name(variant), Ordering::Relaxed);
            }
            if let Some(owner) = nbt.get_uuid("Owner") {
                self.owner.store(Some(owner));
            }
        })
    }
}

impl Animal for ZombieNautilusEntity {
    fn is_food(&self, item_stack: &ItemStack) -> bool {
        is_food_for(item_stack.item, self.is_tame())
    }
}

impl Mob for ZombieNautilusEntity {
    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    /// `ZombieNautilus.getAmbientSound` (ZombieNautilus.java:87-89), reached through the shared
    /// `Mob.baseTick` idle-sound cadence.
    fn get_ambient_sound(&self) -> Option<Sound> {
        Some(self.ambient_sound())
    }

    /// `ZombieNautilus.sunProtectionSlot` (ZombieNautilus.java:59-62).
    fn sun_protection_slot(&self) -> EquipmentSlot {
        EquipmentSlot::BODY
    }

    /// `ZombieNautilus.canBeLeashed` (ZombieNautilus.java:177-180).
    fn can_be_leashed(&self) -> bool {
        !self.aggravated.load(Ordering::Relaxed) && !self.mob_controlled.load(Ordering::Relaxed)
    }

    fn mob_set_variant_name(&self, name: &str) {
        self.variant
            .store(variant_id_from_name(name), Ordering::Relaxed);
    }

    fn mob_init_data_tracker(&self) -> EntityBaseFuture<'_, ()> {
        Box::pin(async move {
            self.mob_entity.living_entity.entity.send_meta_data(
                &[Metadata::new(
                    pumpkin_data::tracked_data::zombie_nautilus::DASH,
                    self.is_dashing(),
                )],
                None,
            );
            self.mob_entity.living_entity.entity.send_meta_data(
                &[Metadata::new(
                    pumpkin_data::tracked_data::zombie_nautilus::DATA_VARIANT_ID,
                    VarInt(self.get_variant()),
                )],
                None,
            );
        })
    }

    fn mob_tick<'a>(&'a self, _caller: &'a Arc<dyn EntityBase>) -> EntityBaseFuture<'a, ()> {
        Box::pin(async move {
            let entity = &self.mob_entity.living_entity.entity;

            // `AbstractNautilus.isAggravated` / `isMobControlled`
            // (AbstractNautilus.java:524-530), cached for the sync `can_be_leashed`.
            self.aggravated.store(
                self.mob_entity.target.lock().await.is_some(),
                Ordering::Relaxed,
            );

            // `AbstractNautilus.applyEffects` (AbstractNautilus.java:260-268).
            {
                let passengers = entity.passengers.lock().await;
                let first = passengers.first();
                self.mob_controlled.store(
                    first.is_some_and(|p| p.cast_any().downcast_ref::<Player>().is_none()),
                    Ordering::Relaxed,
                );
                if let Some(passenger) = first
                    && let Some(player) = passenger.cast_any().downcast_ref::<Player>()
                {
                    let world = entity.world.load();
                    let game_time = world.level_time.lock().await.world_age;
                    if game_time % EFFECT_REFRESH_RATE == 0 {
                        player
                            .living_entity
                            .add_effect(Effect {
                                effect_type: &StatusEffect::BREATH_OF_THE_NAUTILUS,
                                duration: EFFECT_DURATION,
                                amplifier: 0,
                                ambient: true,
                                show_particles: true,
                                show_icon: true,
                                blend: true,
                            })
                            .await;
                    }
                }
            }

            // `AbstractNautilus.tick` (AbstractNautilus.java:301-310).
            if self.is_dashing()
                && self.dash_cooldown.load(Ordering::Relaxed) < DASH_MINIMUM_HOLD_THRESHOLD
            {
                self.set_dashing(false);
            }

            let cooldown = self.dash_cooldown.load(Ordering::Relaxed);
            if cooldown > 0 {
                let next = cooldown - 1;
                self.dash_cooldown.store(next, Ordering::Relaxed);
                if next == 0 {
                    let world = entity.world.load();
                    world.play_sound(
                        self.get_dash_ready_sound(),
                        SoundCategory::Neutral,
                        &entity.pos.load(),
                    );
                }
            }

            // `AbstractNautilus.spawnBubbles` (AbstractNautilus.java:270-292).
            if entity.touching_water.load(Ordering::Relaxed) {
                let speed = entity.velocity.load().length();
                let prob = (speed * 2.0).clamp(0.15, 1.0);
                if rand::random::<f64>() < prob {
                    let world = entity.world.load();
                    let pos = entity.pos.load();
                    world.spawn_particle(
                        pos + Vector3::new(0.0, 0.25, 0.0),
                        Vector3::new(0.4, 0.4, 0.4),
                        0.5,
                        2,
                        Particle::Bubble,
                    );
                }
            }
        })
    }

    /// `AbstractNautilus.mobInteract` (AbstractNautilus.java:396-432) plus `tryToTame`
    /// (AbstractNautilus.java:434-444). The baby branch is dropped (`canBeABaby` is false)
    /// and the inventory-screen branch is not ported — see the module header.
    fn mob_interact<'a>(
        &'a self,
        player: &'a Arc<Player>,
        item_stack: &'a mut ItemStack,
    ) -> EntityBaseFuture<'a, bool> {
        Box::pin(async move {
            let entity = &self.mob_entity.living_entity.entity;

            if !self.is_tame() && self.is_food(item_stack) {
                item_stack.decrement_unless_creative(player.gamemode.load(), 1);
                let world = entity.world.load();
                if rand::random::<u32>().is_multiple_of(3) {
                    self.set_tame(true, Some(player.gameprofile.id));
                    world.send_entity_status(entity, EntityStatus::TamingSucceeded, None);
                } else {
                    world.send_entity_status(entity, EntityStatus::TamingFailed, None);
                }
                world.play_sound(
                    self.get_eat_sound(),
                    SoundCategory::Neutral,
                    &entity.pos.load(),
                );
                return true;
            }

            if self.is_tame() && !player.get_entity().is_sneaking() {
                if !self.is_saddled.load(Ordering::Relaxed) && item_stack.item == &Item::SADDLE {
                    item_stack.decrement_unless_creative(player.gamemode.load(), 1);
                    self.is_saddled.store(true, Ordering::Relaxed);
                    let world = entity.world.load();
                    // `AbstractNautilus.getEquipSound` (AbstractNautilus.java:475-482): the
                    // saddle sounds are shared with the living nautilus, not zombie-specific.
                    world.play_sound(
                        if self.is_underwater() {
                            Sound::ItemNautilusSaddleUnderwaterEquip
                        } else {
                            Sound::ItemNautilusSaddleEquip
                        },
                        SoundCategory::Neutral,
                        &entity.pos.load(),
                    );
                    return true;
                }

                let world = player.world();
                if let Some(vehicle) = world.get_entity_by_id(entity.entity_id)
                    && let Some(passenger) = world.get_player_by_id(player.entity_id())
                {
                    entity
                        .add_passenger(vehicle, passenger as Arc<dyn EntityBase>)
                        .await;
                    return true;
                }
            }

            self.animal_interact(player, item_stack, self.ambient_sound())
                .await
        })
    }

    fn is_saddled(&self) -> bool {
        self.is_saddled.load(Ordering::Relaxed)
    }

    /// `AbstractNautilus.canUseSlot` (AbstractNautilus.java:154-157): the SADDLE and BODY
    /// slots need a live, non-baby, tamed nautilus. The baby term is always false here.
    fn can_be_saddled(&self) -> bool {
        self.mob_entity.living_entity.entity.is_alive() && self.is_tame()
    }

    fn set_saddled(&self, saddled: bool) {
        self.is_saddled.store(saddled, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sounds_split_on_underwater_exactly_like_vanilla() {
        // ZombieNautilus.java:86-109 -- every pair is `isUnderWater() ? X : X_ON_LAND`.
        assert_eq!(ambient_sound_for(true), Sound::EntityZombieNautilusAmbient);
        assert_eq!(
            ambient_sound_for(false),
            Sound::EntityZombieNautilusAmbientLand
        );
        assert_eq!(hurt_sound_for(true), Sound::EntityZombieNautilusHurt);
        assert_eq!(hurt_sound_for(false), Sound::EntityZombieNautilusHurtLand);
        assert_eq!(death_sound_for(true), Sound::EntityZombieNautilusDeath);
        assert_eq!(death_sound_for(false), Sound::EntityZombieNautilusDeathLand);
        assert_eq!(dash_sound_for(true), Sound::EntityZombieNautilusDash);
        assert_eq!(dash_sound_for(false), Sound::EntityZombieNautilusDashLand);
        assert_eq!(
            dash_ready_sound_for(true),
            Sound::EntityZombieNautilusDashReady
        );
        assert_eq!(
            dash_ready_sound_for(false),
            Sound::EntityZombieNautilusDashReadyLand
        );
    }

    #[test]
    fn untamed_accepts_only_taming_items_while_tamed_accepts_all_nautilus_food() {
        // AbstractNautilus.java:97-100. `#minecraft:nautilus_taming_items` is pufferfish and
        // the pufferfish bucket; `#minecraft:nautilus_food` is the wider fish list.
        assert!(is_food_for(&Item::PUFFERFISH, false));
        assert!(is_food_for(&Item::PUFFERFISH_BUCKET, false));
        assert!(!is_food_for(&Item::COD, false));
        assert!(!is_food_for(&Item::SALMON, false));

        assert!(is_food_for(&Item::COD, true));
        assert!(is_food_for(&Item::COOKED_SALMON, true));
        assert!(is_food_for(&Item::PUFFERFISH, true));

        // The sibling `nautilus.rs` accepts a nautilus shell; neither vanilla tag contains it.
        assert!(!is_food_for(&Item::NAUTILUS_SHELL, false));
        assert!(!is_food_for(&Item::NAUTILUS_SHELL, true));
    }

    #[test]
    fn every_tempt_item_is_in_the_nautilus_food_tag() {
        // `NautilusAi.getTemptations` (NautilusAi.java:155-157) is exactly
        // `#minecraft:nautilus_food`, so the hardcoded list must not drift from the tag.
        for item in TEMPT_ITEMS {
            assert!(
                item.has_tag(&tag::Item::MINECRAFT_NAUTILUS_FOOD),
                "{} is not in #minecraft:nautilus_food",
                item.registry_key
            );
        }
        assert_eq!(
            TEMPT_ITEMS.len(),
            tag::Item::MINECRAFT_NAUTILUS_FOOD.0.len()
        );
    }

    #[test]
    fn variant_names_round_trip_in_registry_order() {
        // ZombieNautilusVariants.java:17-19, 25-28: temperate is the default and is
        // registered first.
        assert_eq!(
            variant_id_from_name("minecraft:temperate"),
            VARIANT_TEMPERATE
        );
        assert_eq!(variant_id_from_name("warm"), VARIANT_WARM);
        assert_eq!(variant_id_from_name("minecraft:warm"), VARIANT_WARM);
        assert_eq!(variant_id_from_name("nonsense"), VARIANT_TEMPERATE);
        assert_eq!(
            variant_id_from_name(variant_name_from_id(VARIANT_WARM)),
            VARIANT_WARM
        );
        assert_eq!(
            variant_id_from_name(variant_name_from_id(VARIANT_TEMPERATE)),
            VARIANT_TEMPERATE
        );
    }

    #[test]
    fn generated_attributes_already_carry_the_vanilla_overrides() {
        // ZombieNautilus.java:51-53 (`MOVEMENT_SPEED` 1.1) on top of
        // AbstractNautilus.java:111-117. Nothing in this file sets attributes, so this test
        // is what proves the generated table is doing it.
        let find = |attribute: &pumpkin_data::attributes::Attributes| {
            EntityType::ZOMBIE_NAUTILUS
                .attributes
                .iter()
                .find(|(key, _)| key == attribute)
                .map(|(_, value)| *value)
        };
        assert!(
            (find(&pumpkin_data::attributes::Attributes::MOVEMENT_SPEED).unwrap() - 1.1).abs()
                < 1e-6
        );
        assert!(
            (find(&pumpkin_data::attributes::Attributes::MAX_HEALTH).unwrap() - 15.0).abs() < 1e-6
        );
        assert!(
            (find(&pumpkin_data::attributes::Attributes::ATTACK_DAMAGE).unwrap() - 3.0).abs()
                < 1e-6
        );
        assert!(
            (find(&pumpkin_data::attributes::Attributes::KNOCKBACK_RESISTANCE).unwrap() - 0.3)
                .abs()
                < 1e-6
        );
    }

    #[test]
    fn zombie_nautilus_burns_in_daylight_so_the_body_slot_override_is_reachable() {
        // ZombieNautilus.java:59-62 is only observable through `Mob::tick_sun_burn`, which
        // gates on this tag.
        assert!(
            EntityType::ZOMBIE_NAUTILUS.has_tag(&tag::EntityType::MINECRAFT_BURN_IN_DAYLIGHT),
            "sun_protection_slot() would be dead code without this tag"
        );
    }
}
