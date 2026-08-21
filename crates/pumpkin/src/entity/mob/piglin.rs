use std::sync::{
    Arc, Weak,
    atomic::{AtomicBool, AtomicI32, Ordering},
};

use crate::entity::{
    Entity, EntityBase, EntityBaseFuture, NBTStorage, NbtFuture,
    ai::goal::{
        active_target::ActiveTargetGoal,
        avoid_entity::AvoidEntityGoal,
        look_around::RandomLookAroundGoal,
        look_at_entity::LookAtEntityGoal,
        melee_attack::MeleeAttackGoal,
        piglin_admire::{ADMIRE_DURATION_TICKS, PiglinAdmireGoal},
        piglin_avoid_repellent::PiglinAvoidRepellentGoal,
        ranged_crossbow_attack::RangedCrossbowAttackGoal,
        swim::SwimGoal,
        wander_around::WanderAroundGoal,
    },
    mob::{
        Mob, MobEntity, piglin_shared,
        zombification::{self, ZombificationTimer},
        zombified_piglin::ZombifiedPiglinEntity,
    },
    player::Player,
};
use crate::world::World;
use pumpkin_data::tag::{self, Taggable};
use pumpkin_data::{
    damage::DamageType, data_component_impl::EquipmentSlot, entity::EntityType, item::Item,
    item_stack::ItemStack, sound::Sound,
};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_protocol::java::client::play::Metadata;
use pumpkin_util::Difficulty;
use rand::RngExt;

use crate::entity::ai::target_predicate::TargetData;

/// `PiglinAi.ADMIRING_DISABLED` lockout: `wasHurtBy` (PiglinAi.java:567) sets a
/// 400-tick expiry when the attacker is a player, blocking `canAdmire` for that long.
const ADMIRING_DISABLED_TICKS: i32 = 400;

/// `PiglinAi.TIME_BETWEEN_HUNTS = TimeUtil.rangeOfSeconds(30, 120)` (`PiglinAi.java:92`), in
/// ticks. Used both for `initMemories`'s initial delay (`PiglinAi.java:132-133`) and for
/// `dontKillAnyMoreHoglinsForAWhile`.
const TIME_BETWEEN_HUNTS_MIN: i32 = 30 * 20;
const TIME_BETWEEN_HUNTS_MAX: i32 = 120 * 20;

/// `PiglinAi.isNearZombified` (`PiglinAi.java:499-507`): a zombified piglin or zoglin within
/// six blocks makes a piglin drop its target and retreat. `AVOID_ZOMBIFIED_DURATION`
/// (`PiglinAi.java:108`) then keeps it fleeing for 5-7 seconds; `AvoidEntityGoal` re-evaluates
/// every tick instead of holding an expiring memory, which is equivalent while the zombified mob
/// is still in range and simply stops sooner once it is not.
const ZOMBIFIED_AVOID_DISTANCE: f64 = 6.0;

/// `SetWalkTargetAwayFrom.entity(AVOID_TARGET, 1.0F, 12, true)` (`PiglinAi.java:231`): the
/// distance the AVOID activity retreats to. Reused here as the detection radius, since
/// `AvoidEntityGoal` has one radius where vanilla has a sensor range plus a retreat distance.
const NEMESIS_AVOID_DISTANCE: f64 = 12.0;

/// `PiglinAi.SPEED_MULTIPLIER_WHEN_AVOIDING`/`SPEED_MULTIPLIER_WHEN_RETREATING`, both `1.0F`
/// (`PiglinAi.java:111-112`).
const AVOID_SPEED: f64 = 1.0;

/// `CrossbowItem.getDefaultProjectileRange` (`CrossbowItem.java:274-276`), which is the distance
/// `BehaviorUtils.isWithinAttackRange` (`BehaviorUtils.java:115-121`) checks for the piglin's
/// `CrossbowAttack` behavior (`CrossbowAttack.java:28`).
const CROSSBOW_RANGE: f64 = 8.0;

/// `PiglinAi.isWearingSafeArmor` (PiglinAi.java:648-656): true when ANY of the four armor
/// slots holds an item in the `minecraft:piglin_safe_armor` tag (the four gold pieces).
pub(crate) async fn is_wearing_safe_armor(living: &crate::entity::living::LivingEntity) -> bool {
    const ARMOR_SLOTS: [EquipmentSlot; 4] = [
        EquipmentSlot::HEAD,
        EquipmentSlot::CHEST,
        EquipmentSlot::LEGS,
        EquipmentSlot::FEET,
    ];
    let equipment = living.entity_equipment.lock().await;
    ARMOR_SLOTS.iter().any(|slot| {
        equipment
            .get(slot)
            .item
            .has_tag(&tag::Item::MINECRAFT_PIGLIN_SAFE_ARMOR)
    })
}

/// The `NEAREST_TARGETABLE_PLAYER_NOT_WEARING_GOLD` half of `PiglinSpecificSensor.doTick`
/// (PiglinSpecificSensor.java:82-85), which is the only player memory
/// `PiglinAi.findNearestValidAttackTarget` falls back on for an unprovoked piglin
/// (PiglinAi.java:531-532).
///
/// Anger is deliberately NOT routed through this predicate: vanilla checks the `ANGRY_AT`
/// memory first (PiglinAi.java:515-518), so a piglin that has been hit still retaliates
/// against a fully gold-armoured player. Here that path is `on_damage` ->
/// `piglin_shared::retaliate_and_alert_piglins`, which sets the target directly and never
/// consults this goal.
async fn player_not_wearing_gold(target: TargetData, world: Arc<World>) -> bool {
    let Some(entity) = world.get_entity_by_id(target.entity_id) else {
        return false;
    };
    let Some(living) = entity.get_living_entity() else {
        return false;
    };
    !is_wearing_safe_armor(living).await
}

/// Represents a Piglin.
///
/// Wiki: <https://minecraft.wiki/w/Piglin>
///
/// Vanilla's piglin is entirely brain-driven (`PiglinAi`). No brain is built here; each activity
/// is flattened onto the goal system and every expiring memory becomes a plain countdown field,
/// the shape `warden.rs` established. Each goal registration cites the vanilla behavior it
/// stands for.
///
/// Ported: gold-armour pacification, bartering/admiring, overworld zombification, the AVOID
/// activity for zombified piglins and zoglins (`avoidZombified`), the baby-only nemesis retreat
/// (`babyAvoidNemesis`), repellent avoidance (`avoidRepellent`), crossbow use (`CrossbowAttack`),
/// hoglin hunting (`StartHuntingHoglin`) with its `HUNTED_RECENTLY` cooldown, and the adult-only
/// gate on target acquisition (`StartAttacking`, `PiglinAi.java:160`).
///
/// NOT ported, each needing machinery this codebase lacks:
/// - Anger broadcasting between piglins (`broadcastAngerTarget`, `setAngerTarget`) and the pack
///   half of `StartHuntingHoglin` -- both need a nearby-adult-piglin scan and a shared anger
///   memory. A hunt is a solo affair here; see the comment on the hoglin target goal.
/// - The CELEBRATE and RIDE activities (`initCelebrateActivity`, `initRideHoglinActivity`),
///   including baby piglins riding baby hoglins.
/// - `BackUpIfTooClose.create(5, 0.75F)` (`PiglinAi.java:177`): a crossbow piglin does not back
///   away from a target that closes to within five blocks.
/// - `getSoundForCurrentActivity` (`PiglinAi.java:614-632`), which picks the ambient sound from
///   the active activity; there are no activities to read here.
pub struct PiglinEntity {
    pub mob_entity: MobEntity,
    /// Ticks left staring at a gold ingot given by a player, shared with
    /// `PiglinAdmireGoal`. See `pumpkin/src/entity/ai/goal/piglin_admire.rs`.
    pub admiring_ticks: Arc<AtomicI32>,
    /// Mirrors vanilla's `ADMIRING_DISABLED` memory expiry (see `ADMIRING_DISABLED_TICKS`).
    admiring_disabled_ticks: AtomicI32,
    /// `AbstractPiglin.timeInOverworld`/`IsImmuneToZombification`
    /// (`AbstractPiglin.java:26-33`).
    zombification: ZombificationTimer,
    /// `MemoryModuleType.HUNTED_RECENTLY`, the expiring memory `dontKillAnyMoreHoglinsForAWhile`
    /// sets (`StartHuntingHoglin.java:23`). A plain countdown here; shared with the hunt target
    /// predicate through an `Arc`, the way `hoglin.rs` shares `pacify_ticks`.
    hunted_recently_ticks: Arc<AtomicI32>,
    /// Whether the current target is a hoglin, so `mob_tick` can tell a hunt ending from a hunt
    /// that never started. See the cooldown handling there for why the cooldown is armed on hunt
    /// end rather than on hunt start.
    hunting_hoglin: AtomicBool,
}

impl PiglinEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let mob_entity = MobEntity::new(entity);
        let admiring_ticks = Arc::new(AtomicI32::new(0));
        // `PiglinAi.initMemories` (`PiglinAi.java:131-134`): a fresh piglin already counts as
        // having hunted recently, so it waits out one interval before its first hunt.
        let hunted_recently_ticks = Arc::new(AtomicI32::new(
            rand::rng().random_range(TIME_BETWEEN_HUNTS_MIN..=TIME_BETWEEN_HUNTS_MAX),
        ));
        let piglin = Self {
            mob_entity,
            admiring_ticks: admiring_ticks.clone(),
            admiring_disabled_ticks: AtomicI32::new(0),
            zombification: ZombificationTimer::new(),
            hunted_recently_ticks: hunted_recently_ticks.clone(),
            hunting_hoglin: AtomicBool::new(false),
        };
        let mob_arc = Arc::new(piglin);
        let mob_weak: Weak<dyn Mob> = {
            let mob_arc: Arc<dyn Mob> = mob_arc.clone();
            Arc::downgrade(&mob_arc)
        };
        // `Piglin.isBaby`, shared by the baby-flees-nemesis goal and the adult-only target
        // goals. A `Weak` keeps a goal from holding its own mob alive.
        let is_baby = {
            let piglin_weak = Arc::downgrade(&mob_arc);
            move || piglin_weak.upgrade().is_some_and(|p| !p.is_adult())
        };

        Self::register_goals(&mob_arc, mob_weak, admiring_ticks, is_baby.clone());
        Self::register_target_goals(&mob_arc, is_baby, hunted_recently_ticks);

        mob_arc
    }

    /// The FIGHT/AVOID/IDLE movement behaviours of `PiglinAi`, flattened into the goal system.
    fn register_goals<F>(
        mob_arc: &Arc<Self>,
        mob_weak: Weak<dyn Mob>,
        admiring_ticks: Arc<AtomicI32>,
        is_baby: F,
    ) where
        F: Fn() -> bool + Clone + Send + Sync + 'static,
    {
        let mut goal_selector = mob_arc
            .mob_entity
            .goals_selector
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        goal_selector.add_goal(0, Box::new(SwimGoal::default()));
        goal_selector.add_goal(1, PiglinAdmireGoal::new(admiring_ticks));
        // `PiglinAi.avoidZombified` (`PiglinAi.java:298-302`). Note FIGHT actually outranks
        // AVOID in `updateActivity`'s first-valid list (`PiglinAi.java:307-309`); fleeing wins
        // by target erasure instead, because the FIGHT activity's `EraseMemoryIf` on
        // `isNearZombified` drops `ATTACK_TARGET` (`PiglinAi.java:185`) and
        // `findNearestValidAttackTarget` refuses to pick a new one while near a zombified mob
        // (`PiglinAi.java:511-513`). Placing these above the attack goals reproduces that
        // outcome through goal priority. `PiglinAi.isZombified` (`PiglinAi.java:853-855`)
        // counts the zoglin as well as the zombified piglin.
        for zombified in [&EntityType::ZOMBIFIED_PIGLIN, &EntityType::ZOGLIN] {
            goal_selector.add_goal(
                2,
                Box::new(AvoidEntityGoal::new(
                    zombified,
                    ZOMBIFIED_AVOID_DISTANCE,
                    AVOID_SPEED,
                    AVOID_SPEED,
                )),
            );
        }
        // `PiglinAi.babyAvoidNemesis` (`PiglinAi.java:294-296`): only babies flee, and the
        // nemesis set is what `PiglinSpecificSensor` collects -- wither skeletons and the
        // wither (`PiglinSpecificSensor.java:88-95`).
        for nemesis in [&EntityType::WITHER_SKELETON, &EntityType::WITHER] {
            let baby_gate = is_baby.clone();
            goal_selector.add_goal(
                2,
                Box::new(
                    AvoidEntityGoal::new(nemesis, NEMESIS_AVOID_DISTANCE, AVOID_SPEED, AVOID_SPEED)
                        .with_predicate(Arc::new(move |_| baby_gate())),
                ),
            );
        }
        // `PiglinAi.avoidRepellent` (`PiglinAi.java:290-292`).
        goal_selector.add_goal(3, Box::new(PiglinAvoidRepellentGoal::new()));
        // `initFightActivity` runs `MeleeAttack.create(20)` and `new CrossbowAttack()` side by
        // side (`PiglinAi.java:182-183`); the crossbow goal gates itself on the piglin actually
        // holding one, which `mob/equipment.rs` already gives it a chance of.
        goal_selector.add_goal(4, Box::new(RangedCrossbowAttackGoal::new(CROSSBOW_RANGE)));
        goal_selector.add_goal(4, Box::new(MeleeAttackGoal::new(1.0, true)));
        goal_selector.add_goal(5, Box::new(WanderAroundGoal::new(1.0)));
        goal_selector.add_goal(
            6,
            LookAtEntityGoal::with_default(mob_weak, &EntityType::PLAYER, 8.0),
        );
        goal_selector.add_goal(7, Box::new(RandomLookAroundGoal::default()));
    }

    /// `PiglinAi.findNearestValidAttackTarget` (`PiglinAi.java:509-...`) and
    /// `StartHuntingHoglin`, expressed as target-selector goals.
    fn register_target_goals<F>(
        mob_arc: &Arc<Self>,
        is_baby: F,
        hunted_recently_ticks: Arc<AtomicI32>,
    ) where
        F: Fn() -> bool + Clone + Send + Sync + 'static,
    {
        let mut target_selector = mob_arc
            .mob_entity
            .target_selector
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        // `StartAttacking.create((level, piglin) -> piglin.isAdult(), ...)`
        // (`PiglinAi.java:160`): only adults pick attack targets, which is the other half of
        // the baby's flee behaviour.
        let player_gate = is_baby.clone();
        target_selector.add_goal(
            1,
            Box::new(ActiveTargetGoal::new(
                &mob_arc.mob_entity,
                &EntityType::PLAYER,
                10,
                true,
                false,
                Some(move |target: TargetData, world: Arc<World>| {
                    let player_gate = player_gate.clone();
                    async move { !player_gate() && player_not_wearing_gold(target, world).await }
                }),
            )),
        );

        for nemesis in [&EntityType::WITHER_SKELETON, &EntityType::WITHER] {
            let nemesis_gate = is_baby.clone();
            target_selector.add_goal(
                2,
                Box::new(ActiveTargetGoal::new(
                    &mob_arc.mob_entity,
                    nemesis,
                    10,
                    true,
                    false,
                    Some(move |_target: TargetData, _world: Arc<World>| {
                        let nemesis_gate = nemesis_gate.clone();
                        async move { !nemesis_gate() }
                    }),
                )),
            );
        }

        // `StartHuntingHoglin.create()` (`StartHuntingHoglin.java:10-30`), gated on
        // `Piglin.canHunt` (`Piglin.java:264-266`), on the piglin being an adult, and on
        // `HUNTED_RECENTLY` being absent. The hoglin must be an adult
        // (`PiglinSpecificSensor.java:66-72`); `TargetData.age` carries the baby flag.
        //
        // NOT ported: `broadcastAngerTarget` and the loop that also puts every visible adult
        // piglin on hunt cooldown (`StartHuntingHoglin.java:22-25`), together with the "no
        // nearby piglin has hunted recently" precondition. All three need a
        // nearby-adult-piglin scan no goal here performs, so a hunt stays a solo affair.
        target_selector.add_goal(
            3,
            Box::new(ActiveTargetGoal::new(
                &mob_arc.mob_entity,
                &EntityType::HOGLIN,
                10,
                true,
                false,
                Some(move |target: TargetData, _world: Arc<World>| {
                    let hunt_gate = is_baby.clone();
                    let hunted = hunted_recently_ticks.clone();
                    async move {
                        !hunt_gate() && target.age >= 0 && hunted.load(Ordering::Relaxed) <= 0
                    }
                }),
            )),
        );
    }

    fn is_adult(&self) -> bool {
        self.mob_entity
            .living_entity
            .entity
            .age
            .load(Ordering::Relaxed)
            >= 0
    }
}

impl NBTStorage for PiglinEntity {
    /// `AbstractPiglin.addAdditionalSaveData` (`AbstractPiglin.java:65-70`).
    fn write_nbt<'a>(&'a self, nbt: &'a mut NbtCompound) -> NbtFuture<'a, ()> {
        Box::pin(async {
            self.mob_entity.living_entity.write_nbt(nbt).await;
            self.zombification.write_nbt(nbt);
        })
    }

    /// `AbstractPiglin.readAdditionalSaveData` (`AbstractPiglin.java:72-78`). The
    /// immunity flag is synced because vanilla holds it in `DATA_IMMUNE_TO_ZOMBIFICATION`
    /// (`AbstractPiglin.java:26-28`), which `setImmuneToZombification` writes through.
    fn read_nbt_non_mut<'a>(&'a self, nbt: &'a NbtCompound) -> NbtFuture<'a, ()> {
        Box::pin(async {
            self.mob_entity.living_entity.read_nbt_non_mut(nbt).await;
            self.zombification.read_nbt(nbt);
            self.mob_entity.living_entity.entity.send_meta_data(
                &[Metadata::new(
                    pumpkin_data::tracked_data::piglin::DATA_IMMUNE_TO_ZOMBIFICATION,
                    self.zombification.is_immune(),
                )],
                None,
            );
        })
    }
}

impl Mob for PiglinEntity {
    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    /// `PiglinAi.mobInteract`/`canAdmire` (PiglinAi.java:539-554): a player directly
    /// handing an adult, non-admiring, non-locked-out piglin a gold ingot starts
    /// admiring. Ground-item pickup (`isLovedItem`/broader `piglin_loved` tag) is not
    /// implemented -- see `piglin_admire.rs` module doc.
    fn mob_interact<'a>(
        &'a self,
        player: &'a Arc<Player>,
        item_stack: &'a mut ItemStack,
    ) -> EntityBaseFuture<'a, bool> {
        Box::pin(async move {
            let is_gold_ingot = item_stack.item.id == Item::GOLD_INGOT.id;
            let admiring_disabled = self.admiring_disabled_ticks.load(Ordering::Relaxed) > 0;
            let already_admiring = self.admiring_ticks.load(Ordering::Relaxed) > 0;

            if !is_gold_ingot || admiring_disabled || already_admiring || !self.is_adult() {
                return self
                    .mob_entity
                    .mob_interact(player, item_stack, self.can_be_leashed())
                    .await;
            }

            let taken = ItemStack::new(1, &Item::GOLD_INGOT);
            item_stack.decrement_unless_creative(player.gamemode.load(), 1);

            let mut equipment = self.mob_entity.living_entity.entity_equipment.lock().await;
            let previous = equipment.put(&EquipmentSlot::OFF_HAND, taken.clone());
            drop(equipment);
            if !previous.is_empty() {
                // `holdInOffhand` (PiglinAi.java:353-359): drop whatever was already
                // held in the offhand before replacing it.
                let pos = self.mob_entity.living_entity.entity.block_pos.load();
                self.mob_entity
                    .living_entity
                    .entity
                    .world
                    .load()
                    .drop_stack(&pos, previous)
                    .await;
            }
            self.mob_entity
                .living_entity
                .send_equipment_changes(&[(EquipmentSlot::OFF_HAND, taken)]);

            self.admiring_ticks
                .store(ADMIRE_DURATION_TICKS, Ordering::Relaxed);
            true
        })
    }

    /// `PiglinAi.wasHurtBy` (PiglinAi.java:556-587), simplified: cancels admiring,
    /// applies the player-attacker lockout, then retaliates and alerts nearby piglins
    /// (see `piglin_shared::retaliate_and_alert_piglins`). Not implemented: baby-flee
    /// (100-tick flee instead of retaliating) and the hoglin-outnumbered retreat
    /// branch -- both need per-mob-type special-casing beyond this goal-based
    /// approximation of `maybeRetaliate`.
    fn on_damage<'a>(
        &'a self,
        _damage_type: DamageType,
        source: Option<&'a dyn EntityBase>,
    ) -> EntityBaseFuture<'a, ()> {
        Box::pin(async move {
            if let Some(source) = source
                && source.get_entity().entity_type.id == EntityType::PIGLIN.id
            {
                // vanilla: `if (!(attacker instanceof Piglin))` guards the whole method.
                return;
            }

            self.admiring_ticks.store(0, Ordering::Relaxed);

            // `cancelAdmiring` (PiglinAi.java:401-406): drop the offhand item instead
            // of silently voiding it.
            let mut equipment = self.mob_entity.living_entity.entity_equipment.lock().await;
            let dropped = equipment.put(&EquipmentSlot::OFF_HAND, ItemStack::EMPTY.clone());
            drop(equipment);
            if !dropped.is_empty() {
                let pos = self.mob_entity.living_entity.entity.block_pos.load();
                self.mob_entity
                    .living_entity
                    .entity
                    .world
                    .load()
                    .drop_stack(&pos, dropped)
                    .await;
                self.mob_entity
                    .living_entity
                    .send_equipment_changes(&[(EquipmentSlot::OFF_HAND, ItemStack::EMPTY.clone())]);
            }

            let attacker_is_player =
                source.is_some_and(|s| s.get_entity().entity_type.id == EntityType::PLAYER.id);
            if attacker_is_player {
                self.admiring_disabled_ticks
                    .store(ADMIRING_DISABLED_TICKS, Ordering::Relaxed);
            }

            if let Some(source) = source {
                piglin_shared::retaliate_and_alert_piglins(self, source).await;
            }
        })
    }

    /// Decrements the admiring-disabled lockout every tick, matching the passive
    /// expiry of vanilla's `ADMIRING_DISABLED` memory. Must run independently of
    /// `PiglinAdmireGoal` since the goal isn't running while admiring is disabled.
    fn mob_tick<'a>(&'a self, _caller: &'a Arc<dyn EntityBase>) -> EntityBaseFuture<'a, ()> {
        Box::pin(async move {
            let remaining = self.admiring_disabled_ticks.load(Ordering::Relaxed);
            if remaining > 0 {
                self.admiring_disabled_ticks
                    .store(remaining - 1, Ordering::Relaxed);
            }

            // `HUNTED_RECENTLY`'s expiry, plus `PiglinAi.dontKillAnyMoreHoglinsForAWhile`
            // (`StartHuntingHoglin.java:23`).
            //
            // Vanilla arms the cooldown the moment the hunt starts: `StartHuntingHoglin` is a
            // one-shot behavior, and the hunt then lives on in the `ATTACK_TARGET` memory. That
            // does not transfer. `TrackTargetGoal::should_continue` re-runs the target predicate
            // every tick, so arming on acquisition would fail the predicate on the very next
            // tick and drop the hoglin immediately. The cooldown is therefore armed when the
            // hunt ENDS -- the hoglin dies, or the target is lost -- which still produces the
            // 30-120 second spacing between hunts the vanilla constant exists for.
            let targeting_hoglin = self
                .mob_entity
                .target
                .lock()
                .await
                .as_ref()
                .is_some_and(|t| t.get_entity().entity_type.id == EntityType::HOGLIN.id);
            let was_hunting = self
                .hunting_hoglin
                .swap(targeting_hoglin, Ordering::Relaxed);

            let hunted = self.hunted_recently_ticks.load(Ordering::Relaxed);
            if hunted > 0 {
                self.hunted_recently_ticks
                    .store(hunted - 1, Ordering::Relaxed);
            } else if was_hunting && !targeting_hoglin {
                self.hunted_recently_ticks.store(
                    self.get_random()
                        .random_range(TIME_BETWEEN_HUNTS_MIN..=TIME_BETWEEN_HUNTS_MAX),
                    Ordering::Relaxed,
                );
            }

            // `AbstractPiglin.customServerAiStep` (`AbstractPiglin.java:80-96`).
            if self.zombification.tick(&self.mob_entity) {
                if self
                    .mob_entity
                    .living_entity
                    .entity
                    .world
                    .load()
                    .level_info
                    .load()
                    .difficulty
                    != Difficulty::Peaceful
                {
                    zombification::play_converted_sound(
                        &self.mob_entity,
                        Sound::EntityPiglinConvertedToZombified,
                    );
                }
                zombification::convert_to(
                    &self.mob_entity,
                    &EntityType::ZOMBIFIED_PIGLIN,
                    true,
                    ZombifiedPiglinEntity::new,
                )
                .await;
            }
        })
    }
}
