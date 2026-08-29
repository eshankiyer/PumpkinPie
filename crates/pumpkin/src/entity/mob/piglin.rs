use std::sync::{
    Arc, Mutex, Weak,
    atomic::{AtomicBool, AtomicI32, Ordering},
};

use pumpkin_data::tag::{self, Taggable};
use pumpkin_data::{
    Block, damage::DamageType, data_component_impl::EquipmentSlot, dimension::Dimension,
    entity::EntityType, item::Item, item_stack::ItemStack, sound::{Sound, SoundCategory},
    tracked_data,
};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_nbt::tag::NbtTag;
use pumpkin_protocol::java::client::play::Metadata;
use pumpkin_util::math::boundingbox::EntityDimensions;
use pumpkin_util::math::position::BlockPos;

use crate::entity::{
    Entity, EntityBase,
    ai::goal::{
        active_target::ActiveTargetGoal,
        avoid_entity::AvoidEntityGoal,
        back_up_if_too_close::BackUpIfTooCloseGoal,
        go_to_wanted_item::GoToWantedItemGoal,
        look_around::RandomLookAroundGoal,
        look_at_entity::LookAtEntityGoal,
        melee_attack::MeleeAttackGoal,
        open_door::OpenDoorGoal,
        piglin_admire::{ADMIRE_DURATION_TICKS, PiglinAdmireGoal},
        piglin_avoid_repellent::PiglinAvoidRepellentGoal,
        ranged_crossbow_attack::RangedCrossbowAttackGoal,
        spear_use::SpearUseGoal,
        swim::SwimGoal,
        wander_around::WanderAroundGoal,
    },
    mob::{
        Mob, MobEntity, crossbow_attack_mob::CrossbowAttackMob, equipment::RegionalDifficulty,
        piglin_ai::PiglinAi, zombification::ZombificationTimer,
    },
    player::Player,
};
use crate::world::World;
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

/// `GoToWantedItem.create(PiglinAi::isNotHoldingLovedItemInOffHand, 1.0F, true, 9)`
/// (`PiglinAi.java:218`): the piglin walks to a wanted item at speed 1.0, but only if it is
/// within nine blocks.
const WANTED_ITEM_WALK_SPEED: f64 = 1.0;
const WANTED_ITEM_MAX_DISTANCE: f64 = 9.0;

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
/// Ported: gold-armour pacification, bartering/admiring from both the direct-gift path
/// (`mobInteract`) and the ground path (`GoToWantedItem` -> `wantsToPickup` ->
/// `pickUpItem`), overworld zombification, the AVOID
/// activity for zombified piglins and zoglins (`avoidZombified`), the baby-only nemesis retreat
/// (`babyAvoidNemesis`), repellent avoidance (`avoidRepellent`), crossbow use (`CrossbowAttack`),
/// the fight-activity spear trio (`SpearApproach`/`SpearAttack`/`SpearRetreat`,
/// `PiglinAi.java:179-181`, flattened onto `SpearUseGoal`),
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
/// - The non-currency branches of `pickUpItem`/`wantsToPickup` (gold nuggets, food, armour
///   upgrades), each of which needs the mob inventory or the eating path this codebase
///   lacks; see the `wants_to_pick_up_item` override.
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
    /// An item taken off the ground by `Mob::mob_try_pick_up_items`, waiting to be moved into
    /// the offhand on the next `mob_tick`.
    ///
    /// `PiglinAi.pickUpItem` (`PiglinAi.java:336-360`) equips the item inside the pickup call,
    /// and is moved into the equipment slot by `mob_tick`, after the pickup path has accepted it.
    pending_offhand: std::sync::Mutex<Option<ItemStack>>,
    /// Whether the piglin currently has an attack target, sampled once per `mob_tick`.
    ///
    /// `wantsToPickup` consults `MemoryModuleType.ATTACK_TARGET` (`PiglinAi.java:464`), but it
    /// runs from the synchronous `Mob::wants_to_pick_up_item`, which cannot await the target
    /// mutex. The sample is at most one tick stale.
    has_attack_target: AtomicBool,
    // Upstream's public state surface is retained for callers and NBT compatibility. The
    // parity goal path above uses the dedicated countdown/timer fields, with the mirrored
    // values kept in sync by the accessors below.
    pub immune_to_zombification: AtomicBool,
    pub time_in_overworld: AtomicI32,
    pub is_baby: AtomicBool,
    pub cannot_hunt: AtomicBool,
    pub is_charging_crossbow: AtomicBool,
    pub is_dancing: AtomicBool,
    pub inventory: Mutex<Vec<ItemStack>>,
    pub admire_timer: AtomicI32,
    pub admiring_disabled_timer: AtomicI32,
    pub eat_cooldown_timer: AtomicI32,
    pub celebration_timer: AtomicI32,
    pub hunt_cooldown_timer: AtomicI32,
    pub admiring_item: Mutex<Option<ItemStack>>,
}

impl PiglinEntity {
    pub const CONVERSION_TIME: i32 = 300;
    pub const INVENTORY_SIZE: usize = 8;
    pub const XP_REWARD: u32 = 5;

    pub const ADULT_DIMENSIONS: EntityDimensions = EntityDimensions {
        width: 0.6,
        height: 1.95,
        eye_height: 1.79,
    };
    pub const BABY_DIMENSIONS: EntityDimensions = EntityDimensions {
        width: 0.49,
        height: 0.98,
        eye_height: 0.78,
    };

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
            pending_offhand: std::sync::Mutex::new(None),
            has_attack_target: AtomicBool::new(false),
            immune_to_zombification: AtomicBool::new(false),
            time_in_overworld: AtomicI32::new(0),
            is_baby: AtomicBool::new(false),
            cannot_hunt: AtomicBool::new(false),
            is_charging_crossbow: AtomicBool::new(false),
            is_dancing: AtomicBool::new(false),
            inventory: Mutex::new(Vec::new()),
            admire_timer: AtomicI32::new(0),
            admiring_disabled_timer: AtomicI32::new(0),
            eat_cooldown_timer: AtomicI32::new(0),
            celebration_timer: AtomicI32::new(0),
            hunt_cooldown_timer: AtomicI32::new(0),
            admiring_item: Mutex::new(None),
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
        goal_selector.add_goal(1, Box::new(OpenDoorGoal::new(true)));
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
        // `GoToWantedItem` in the IDLE activity (`PiglinAi.java:218`). This is the half of
        // bartering a player actually uses: gold thrown on the ground is walked to, then
        // picked up by `Mob::mob_try_pick_up_items`, which routes through the
        // `wants_to_pick_up_item`/`on_item_pickup` overrides below into the admire countdown
        // that `PiglinAdmireGoal` pays out. Ranked below the AVOID goals and the repellent
        // retreat, matching vanilla, where IDLE is the lowest-priority activity
        // (`PiglinAi.java:307-309`).
        goal_selector.add_goal(
            3,
            GoToWantedItemGoal::new(WANTED_ITEM_WALK_SPEED, WANTED_ITEM_MAX_DISTANCE),
        );
        // `BackUpIfTooClose.create(5, 0.75F)` (`PiglinAi.java:177`) is a one-shot Brain
        // behavior. It must outrank the crossbow goal so a close visible target can interrupt
        // an already-running ranged attack.
        goal_selector.add_goal(3, Box::new(BackUpIfTooCloseGoal::new(5.0, 0.75)));
        // `initFightActivity`'s spear trio (`PiglinAi.java:179-181`): `new
        // SpearApproach(1.0, 10.0F)`, `new SpearAttack(1.0, 1.0, 2.0F)`, `new
        // SpearRetreat(1.0)`. Vanilla runs these as three Brain behaviors that pass a
        // `SPEAR_STATUS` memory between them (approach -> charging -> retreat); this
        // codebase has no Brain memory plumbing, so the trio is flattened onto the
        // existing goal port of the behaviourally identical approach -> windup -> stab ->
        // reposition cycle (`SpearUseGoal`), exactly how the zombie registers the same
        // behaviour (`Zombie.java:120`). The parameters map one-to-one: approach distance
        // `10.0F` from `SpearApproach`, charge/reposition speeds `1.0`/`1.0` and stab range
        // `2.0F` from `SpearAttack`. The goal self-gates on the held item carrying the
        // `minecraft:kinetic_weapon` component, so it coexists with the crossbow/melee pair
        // below; registering ahead of them reproduces `initFightActivity`'s behavior order.
        goal_selector.add_goal(4, SpearUseGoal::new(1.0, 1.0, 10.0, 2.0));
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

    pub fn is_adult(&self) -> bool {
        !self.is_baby()
    }

    /// `PiglinAi.isNotHoldingLovedItemInOffHand` (`PiglinAi.java:849-851`).
    ///
    /// Read off the admire countdown rather than the offhand slot itself, which lives behind
    /// an async mutex this is called from a synchronous context. The two agree in this port:
    /// the offhand is only ever filled by `hold_in_offhand_and_admire`, which sets the
    /// countdown in the same call, and is only ever emptied by `PiglinAdmireGoal`'s payout or
    /// by `on_damage`'s `cancelAdmiring`, both of which clear the countdown. Vanilla's other
    /// offhand writers -- `equipItemIfPossible` for armour and the baby's held item -- have no
    /// analogue here.
    fn is_not_holding_loved_item_in_offhand(&self) -> bool {
        self.admiring_ticks.load(Ordering::Relaxed) <= 0
            && self
                .pending_offhand
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .is_none()
    }

    /// `PiglinAi.holdInOffhand` (`PiglinAi.java:362-368`) followed by `admireGoldItem`
    /// (`PiglinAi.java:805-807`): drop whatever the offhand already held, equip the new stack,
    /// and start the 119-tick admire countdown that `PiglinAdmireGoal` pays out.
    fn hold_in_offhand_and_admire(&self, stack: ItemStack) {
        self.admiring_ticks
            .store(ADMIRE_DURATION_TICKS, Ordering::Relaxed);
        self.admire_timer
            .store(ADMIRE_DURATION_TICKS, Ordering::Relaxed);
        *self
            .admiring_item
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(stack.clone());
        let mut equipment = self
            .mob_entity
            .living_entity
            .entity_equipment
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let previous = equipment.put(&EquipmentSlot::OFF_HAND, stack.clone());
        drop(equipment);
        if !previous.is_empty() {
            let pos = self.mob_entity.living_entity.entity.block_pos.load();
            self.mob_entity
                .living_entity
                .entity
                .world
                .load()
                .drop_stack(&pos, previous);
        }
        self.mob_entity
            .living_entity
            .send_equipment_changes(&[(EquipmentSlot::OFF_HAND, stack)]);
    }
}

impl PiglinEntity {
    #[must_use]
    pub fn is_immune_to_zombification(&self) -> bool {
        self.zombification.is_immune()
    }

    /// `AbstractPiglin` stores this flag in `DATA_IMMUNE_TO_ZOMBIFICATION`
    /// (`AbstractPiglin.java:26-28`) and persists it through the additional save data.
    pub fn set_immune_to_zombification(&self, immune: bool) {
        self.immune_to_zombification.store(immune, Ordering::Relaxed);
        self.zombification.set_immune(immune);
        self.mob_entity.living_entity.entity.send_meta_data(
            &[Metadata::new(
                tracked_data::piglin::DATA_IMMUNE_TO_ZOMBIFICATION,
                immune,
            )],
            None,
        );
    }

    #[must_use]
    pub fn is_converting(&self, world: &World) -> bool {
        self.zombification.is_converting(&self.mob_entity)
            && world.dimension.minecraft_name != Dimension::THE_NETHER.minecraft_name
    }

    #[must_use]
    pub fn is_baby(&self) -> bool {
        self.is_baby.load(Ordering::Relaxed)
    }

    pub fn set_baby(&self, baby: bool) {
        self.is_baby.store(baby, Ordering::Relaxed);
        let entity = &self.mob_entity.living_entity.entity;
        entity.send_meta_data(
            &[Metadata::new(tracked_data::piglin::DATA_BABY_ID, baby)],
            None,
        );
        if baby {
            entity.entity_dimension.store(Self::BABY_DIMENSIONS);
        } else {
            entity.entity_dimension.store(Self::ADULT_DIMENSIONS);
        }
    }

    #[must_use]
    pub fn is_charging_crossbow(&self) -> bool {
        self.is_charging_crossbow.load(Ordering::Relaxed)
    }

    pub fn set_charging_crossbow(&self, is_charging: bool) {
        self.is_charging_crossbow
            .store(is_charging, Ordering::Relaxed);
        self.mob_entity.living_entity.entity.send_meta_data(
            &[Metadata::new(
                tracked_data::piglin::DATA_IS_CHARGING_CROSSBOW,
                is_charging,
            )],
            None,
        );
    }

    #[must_use]
    pub fn is_dancing(&self) -> bool {
        self.is_dancing.load(Ordering::Relaxed)
    }

    pub fn set_dancing(&self, is_dancing: bool) {
        self.is_dancing.store(is_dancing, Ordering::Relaxed);
        self.mob_entity.living_entity.entity.send_meta_data(
            &[Metadata::new(
                tracked_data::piglin::DATA_IS_DANCING,
                is_dancing,
            )],
            None,
        );
    }

    #[must_use]
    pub fn can_hunt(&self) -> bool {
        !self.cannot_hunt.load(Ordering::Relaxed)
            && self.hunt_cooldown_timer.load(Ordering::Relaxed) <= 0
    }

    pub fn set_cannot_hunt(&self, cannot_hunt: bool) {
        self.cannot_hunt.store(cannot_hunt, Ordering::Relaxed);
    }

    #[must_use]
    pub fn is_admiring(&self) -> bool {
        self.admiring_ticks.load(Ordering::Relaxed) > 0
    }

    #[must_use]
    pub fn is_admiring_disabled(&self) -> bool {
        self.admiring_disabled_ticks.load(Ordering::Relaxed) > 0
    }

    #[must_use]
    pub fn has_eaten_recently(&self) -> bool {
        self.eat_cooldown_timer.load(Ordering::Relaxed) > 0
    }

    pub fn start_admiring(&self, item: ItemStack) {
        self.admiring_ticks
            .store(ADMIRE_DURATION_TICKS, Ordering::Relaxed);
        self.admire_timer
            .store(PiglinAi::ADMIRE_DURATION, Ordering::Relaxed);
        *self
            .admiring_item
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(item.clone());

        let mut equip = self
            .mob_entity
            .living_entity
            .entity_equipment
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        equip.put(&EquipmentSlot::OFF_HAND, item);

        let entity = &self.mob_entity.living_entity.entity;
        let pos = entity.pos.load();
        entity.world.load().play_sound(
            Sound::EntityPiglinAdmiringItem,
            SoundCategory::Hostile,
            &pos,
        );
    }

    pub fn stop_holding_off_hand_item(&self, bartering_enabled: bool) {
        self.admiring_ticks.store(0, Ordering::Relaxed);
        let admired_item = {
            let mut guard = self
                .admiring_item
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            guard.take()
        };

        let _ = {
            let mut equip = self
                .mob_entity
                .living_entity
                .entity_equipment
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            equip.put(&EquipmentSlot::OFF_HAND, ItemStack::EMPTY.clone())
        };

        let Some(item) = admired_item else {
            return;
        };

        if self.is_adult() {
            let is_barter = PiglinAi::is_barter_currency(&item);
            if bartering_enabled && is_barter {
                let outcomes = PiglinAi::get_barter_response_items();
                let entity = &self.mob_entity.living_entity.entity;

                let mut event =
                    crate::plugin::api::events::entity::piglin_barter::PiglinBarterEvent::new(
                        entity.entity_id,
                        item,
                        outcomes,
                    );
                if let Some(server) = entity.world.load().server.upgrade() {
                    server.plugin_manager.fire_blocking(&server, &mut event);
                }

                if !event.cancelled {
                    PiglinAi::throw_items(self, event.outcome, None);
                }
            } else if !is_barter {
                let remainder = self.add_to_inventory(item);
                if let Some(rem) = remainder {
                    PiglinAi::throw_items(self, vec![rem], None);
                }
            }
        } else {
            let remainder = self.add_to_inventory(item);
            if let Some(rem) = remainder {
                PiglinAi::throw_items(self, vec![rem], None);
            }
        }
    }

    pub fn cancel_admiring(&self) {
        if self.is_admiring() {
            self.admiring_ticks.store(0, Ordering::Relaxed);
            self.admire_timer.store(0, Ordering::Relaxed);
            let item = {
                let mut guard = self
                    .admiring_item
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                guard.take()
            };
            if let Some(item) = item {
                let _ = {
                    let mut equip = self
                        .mob_entity
                        .living_entity
                        .entity_equipment
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    equip.put(&EquipmentSlot::OFF_HAND, ItemStack::EMPTY.clone())
                };
                PiglinAi::throw_items(self, vec![item], None);
            }
        }
    }

    pub fn was_hurt_by(&self, attacker: Option<&dyn EntityBase>) {
        self.cancel_admiring();
        self.set_dancing(false);
        self.celebration_timer.store(0, Ordering::Relaxed);

        if let Some(attacker_entity) = attacker
            && attacker_entity.get_entity().entity_type.id == EntityType::PLAYER.id
        {
            self.admiring_disabled_ticks
                .store(ADMIRING_DISABLED_TICKS, Ordering::Relaxed);
            self.admiring_disabled_timer
                .store(PiglinAi::ADMIRING_DISABLED_DURATION, Ordering::Relaxed);
        }
    }

    pub fn add_to_inventory(&self, item: ItemStack) -> Option<ItemStack> {
        let mut inv = self
            .inventory
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if inv.len() < Self::INVENTORY_SIZE {
            inv.push(item);
            None
        } else {
            Some(item)
        }
    }

    pub fn drop_inventory(&self) {
        let items = self
            .inventory
            .try_lock()
            .map_or_else(|_| Vec::new(), |mut inv| std::mem::take(&mut *inv));
        let entity = &self.mob_entity.living_entity.entity;
        let world = entity.world.load();
        let pos = entity.pos.load();
        for item in items {
            if !item.is_empty() {
                let item_entity = crate::entity::item::ItemEntity::new(
                    Entity::new(world.clone(), pos, &EntityType::ITEM),
                    item,
                );
                world.spawn_entity(Arc::new(item_entity));
            }
        }
    }

    #[must_use]
    pub fn check_piglin_spawn_rules(world: &World, pos: &BlockPos) -> bool {
        let below = BlockPos::new(pos.0.x, pos.0.y - 1, pos.0.z);
        let state = world.get_block_state(&below);
        state.id != Block::NETHER_WART_BLOCK.default_state.id
    }

    fn convert_to_zombified(&self) {
        let entity = &self.mob_entity.living_entity.entity;
        let world = entity.world.load();
        let pos = entity.pos.load();

        if world.level_info.load().difficulty != pumpkin_util::Difficulty::Peaceful {
            world.play_sound(
                Sound::EntityPiglinConvertedToZombified,
                SoundCategory::Hostile,
                &pos,
            );
        }

        self.drop_inventory();

        let zombified = crate::entity::r#type::from_type(
            &EntityType::ZOMBIFIED_PIGLIN,
            pos,
            &world,
            uuid::Uuid::new_v4(),
        );

        let zombified_base = zombified.get_entity();
        zombified_base.set_rotation(entity.yaw.load(), entity.pitch.load());
        zombified_base.head_yaw.store(entity.head_yaw.load());
        zombified_base.velocity.store(entity.velocity.load());

        if let Some(living) = zombified.get_living_entity() {
            living.set_health(self.mob_entity.living_entity.health.load());
        }

        if let Some(custom_name) = &**entity.custom_name.load() {
            zombified_base.set_custom_name(custom_name.clone());
        }

        {
            let src_equip = self
                .mob_entity
                .living_entity
                .entity_equipment
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(living) = zombified.get_living_entity() {
                let mut dst_equip = living
                    .entity_equipment
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                for (slot, item) in &src_equip.equipment {
                    dst_equip.put(slot, item.clone());
                }
            }
        }

        world.spawn_entity(zombified);
        entity.remove();
    }
}

impl Mob for PiglinEntity {
    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    /// `AbstractPiglin`'s constructor calls `setCanPickUpLoot(true)` unconditionally
    /// (`AbstractPiglin.java:37`), and `readAdditionalSaveData` defaults the saved flag to
    /// `true` as well (`AbstractPiglin.java:75`, `DEFAULT_PICK_UP_LOOT` at line 31). The
    /// generic spawn-time roll in `mob/mod.rs` (0.55 x regional difficulty) is the wrong rule
    /// for this mob, so it is overridden rather than configured -- without this a piglin
    /// mostly cannot be bartered with at all.
    fn can_pick_up_loot(&self) -> bool {
        true
    }

    /// `PiglinAi.wantsToPickup` (`PiglinAi.java:460-478`).
    ///
    /// Only the barter-currency branch can ever return `true` here. The four other branches
    /// all end in a capability this codebase does not have: `canAddToInventory` and
    /// `putInInventory` need the mob inventory `InventoryCarrier` provides (gold nuggets and
    /// non-currency loved items), `hasEatenRecently`/`eat` need the `ATE_RECENTLY` memory and
    /// a mob eating path (`ItemTags.PIGLIN_FOOD`), and `canReplaceCurrentItem`
    /// (`Mob.java`'s equipment-upgrade comparison) is not ported. Each is rejected here
    /// rather than accepted and silently dropped, so a piglin never destroys an item it
    /// cannot actually use.
    fn wants_to_pick_up_item(&self, _world: &World, stack: &ItemStack) -> bool {
        // `body.isBaby() && itemStack.is(ItemTags.IGNORED_BY_PIGLIN_BABIES)` (line 461).
        if !self.is_adult()
            && stack
                .item
                .has_tag(&tag::Item::MINECRAFT_IGNORED_BY_PIGLIN_BABIES)
        {
            return false;
        }
        // `itemStack.is(ItemTags.PIGLIN_REPELLENTS)` (line 463).
        if stack.item.has_tag(&tag::Item::MINECRAFT_PIGLIN_REPELLENTS) {
            return false;
        }
        // `isAdmiringDisabled(body) && body.getBrain().hasMemoryValue(ATTACK_TARGET)`
        // (line 464).
        if self.admiring_disabled_ticks.load(Ordering::Relaxed) > 0
            && self.has_attack_target.load(Ordering::Relaxed)
        {
            return false;
        }
        // `isBarterCurrency(itemStack)` -> `isNotHoldingLovedItemInOffHand(body)`
        // (lines 467-468). `PiglinAi.BARTERING_ITEM = Items.GOLD_INGOT` (`PiglinAi.java:83`).
        //
        // The `is_adult` term is a deliberate divergence. Vanilla lets a baby pick gold up,
        // but `stopHoldingOffHandItem` gates the barter payout on `body.isAdult()`
        // (`PiglinAi.java:385`); the baby branch (lines 395-406) moves the ingot into the main
        // hand or the mob inventory and never throws loot. `PiglinAdmireGoal` here implements
        // only the adult branch, so letting a baby pick gold up would pay out a barter no
        // vanilla baby ever pays. Refusing the pickup keeps the wrong behaviour from firing at
        // the cost of a baby not carrying gold around; the faithful baby branch needs the mob
        // inventory this codebase lacks. `mob_interact` already refuses babies the same way.
        self.is_adult()
            && stack.item.id == Item::GOLD_INGOT.id
            && self.is_not_holding_loved_item_in_offhand()
    }

    /// The taking half of `PiglinAi.pickUpItem` (`PiglinAi.java:336-347`): everything but a
    /// gold nugget is taken one item at a time, and the nugget branch is unreachable because
    /// `wants_to_pick_up_item` above never accepts one.
    ///
    /// `body.take(itemEntity, 1)` -- the client-side pickup animation -- is not sent: the
    /// caller in `mob/mod.rs` owns the `ItemEntity` and does not offer a hook for it.
    fn on_item_pickup<'a>(
        &'a self,
        stack: &'a ItemStack,
    ) -> crate::entity::EntityBaseFuture<'a, u8> {
        Box::pin(async move {
            if stack.item.id != Item::GOLD_INGOT.id {
                return 0;
            }
            let mut pending = self
                .pending_offhand
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if pending.is_some() {
                return 0;
            }
            *pending = Some(ItemStack::new(1, &Item::GOLD_INGOT));
            1
        })
    }

    /// `PiglinAi.mobInteract`/`canAdmire` (PiglinAi.java:539-554): a player directly
    /// handing an adult, non-admiring, non-locked-out piglin a gold ingot starts
    /// admiring. Ground-item pickup (`isLovedItem`/broader `piglin_loved` tag) is not
    /// implemented -- see `piglin_admire.rs` module doc.
    fn mob_interact(&self, player: &Arc<Player>, item_stack: &mut ItemStack) -> bool {
        let is_gold_ingot = item_stack.item.id == Item::GOLD_INGOT.id;
        let admiring_disabled = self.admiring_disabled_ticks.load(Ordering::Relaxed) > 0;
        let already_admiring = self.admiring_ticks.load(Ordering::Relaxed) > 0;

        if !is_gold_ingot || admiring_disabled || already_admiring || !self.is_adult() {
            return self.mob_entity.mob_interact(player, item_stack);
        }

        let taken = ItemStack::new(1, &Item::GOLD_INGOT);
        item_stack.decrement_unless_creative(player.gamemode.load(), 1);
        self.hold_in_offhand_and_admire(taken);
        true
    }

    /// `PiglinAi.wasHurtBy` (PiglinAi.java:556-587), simplified: cancels admiring,
    /// applies the player-attacker lockout, then retaliates and alerts nearby piglins
    /// (see `piglin_shared::retaliate_and_alert_piglins`). Not implemented: baby-flee
    /// (100-tick flee instead of retaliating) and the hoglin-outnumbered retreat
    /// branch -- both need per-mob-type special-casing beyond this goal-based
    /// approximation of `maybeRetaliate`.
    fn on_damage(&self, _damage_type: DamageType, source: Option<&dyn EntityBase>) {
        if source.is_some_and(|source| {
            source.get_entity().entity_type.id == EntityType::PIGLIN.id
        }) {
            return;
        }

        self.admiring_ticks.store(0, Ordering::Relaxed);
        self.admire_timer.store(0, Ordering::Relaxed);
        self.admiring_item
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        self.pending_offhand
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();

        let mut equipment = self
            .mob_entity
            .living_entity
            .entity_equipment
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let dropped = equipment.put(&EquipmentSlot::OFF_HAND, ItemStack::EMPTY.clone());
        drop(equipment);
        if !dropped.is_empty() {
            let pos = self.mob_entity.living_entity.entity.block_pos.load();
            self.mob_entity.living_entity.entity.world.load().drop_stack(&pos, dropped);
            self.mob_entity
                .living_entity
                .send_equipment_changes(&[(EquipmentSlot::OFF_HAND, ItemStack::EMPTY.clone())]);
        }

        if source.is_some_and(|s| s.get_entity().entity_type.id == EntityType::PLAYER.id) {
            self.admiring_disabled_ticks
                .store(ADMIRING_DISABLED_TICKS, Ordering::Relaxed);
            self.admiring_disabled_timer
                .store(ADMIRING_DISABLED_TICKS, Ordering::Relaxed);
        }
        if let Some(source) = source {
            let entity_id = source.get_entity().entity_id;
            if let Some(attacker) = self.mob_entity.living_entity.entity.world.load().get_entity_by_id(entity_id) {
                *self.mob_entity.target.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(attacker);
            }
        }
    }

    /// Decrements the admiring-disabled lockout every tick, matching the passive
    /// expiry of vanilla's `ADMIRING_DISABLED` memory. Must run independently of
    /// `PiglinAdmireGoal` since the goal isn't running while admiring is disabled.
    fn mob_tick(&self, _caller: &dyn EntityBase) {
        let entity = &self.mob_entity.living_entity.entity;
        if !entity.is_alive() {
            return;
        }

        let world = entity.world.load();
        if self.is_converting(&world) {
            self.time_in_overworld.fetch_add(1, Ordering::Relaxed);
        } else {
            self.time_in_overworld.store(0, Ordering::Relaxed);
        }

        let remaining = self.admiring_disabled_ticks.load(Ordering::Relaxed);
        if remaining > 0 {
            self.admiring_disabled_ticks
                .store(remaining - 1, Ordering::Relaxed);
        }

        // Keep upstream's public countdown state ticking alongside the parity countdowns.
        if self.admiring_disabled_timer.load(Ordering::Relaxed) > 0 {
            self.admiring_disabled_timer.fetch_sub(1, Ordering::Relaxed);
        }
        if self.eat_cooldown_timer.load(Ordering::Relaxed) > 0 {
            self.eat_cooldown_timer.fetch_sub(1, Ordering::Relaxed);
        }
        if self.hunt_cooldown_timer.load(Ordering::Relaxed) > 0 {
            self.hunt_cooldown_timer.fetch_sub(1, Ordering::Relaxed);
        }
        if self.celebration_timer.load(Ordering::Relaxed) > 0 {
            let remaining = self.celebration_timer.fetch_sub(1, Ordering::Relaxed) - 1;
            if remaining <= 0 {
                self.set_dancing(false);
            }
        }
        if self.admire_timer.load(Ordering::Relaxed) > 0 {
            let remaining = self.admire_timer.fetch_sub(1, Ordering::Relaxed) - 1;
            if remaining <= 0 {
                self.stop_holding_off_hand_item(true);
            }
        }

        // The equip half of `PiglinAi.pickUpItem`'s `isLovedItem` branch
        // (`PiglinAi.java:348-351`), deferred out of the synchronous `on_item_pickup`.
        let picked_up = self
            .pending_offhand
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        if let Some(stack) = picked_up {
            self.hold_in_offhand_and_admire(stack);
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
        let (has_target, targeting_hoglin) = {
            let target = self
                .mob_entity
                .target
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            (
                target.is_some(),
                target
                    .as_ref()
                    .is_some_and(|t| t.get_entity().entity_type.id == EntityType::HOGLIN.id),
            )
        };
        // Sampled for `wants_to_pick_up_item`'s `ATTACK_TARGET` check; see the field doc.
        self.has_attack_target.store(has_target, Ordering::Relaxed);
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
            self.convert_to_zombified();
        }
    }

    fn populate_default_equipment_slots(
        &self,
        _world: &Arc<World>,
        _difficulty: &RegionalDifficulty,
    ) {
        if !self.is_baby.load(Ordering::Relaxed) {
            let living = &self.mob_entity.living_entity;
            let mut equipment = living
                .entity_equipment
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);

            // Spawn weapon: 50% crossbow, 5% golden spear (10% of remaining 50%), 45% golden sword
            let weapon = if rand::random::<f32>() < 0.5 {
                &Item::CROSSBOW
            } else if rand::random_range(0..10) == 0 {
                &Item::GOLDEN_SPEAR
            } else {
                &Item::GOLDEN_SWORD
            };
            equipment.put(&EquipmentSlot::MAIN_HAND, ItemStack::new(1, weapon));

            // Armor: 10% chance per piece for golden armor
            if rand::random::<f32>() < 0.1 {
                equipment.put(
                    &EquipmentSlot::HEAD,
                    ItemStack::new(1, &Item::GOLDEN_HELMET),
                );
            }
            if rand::random::<f32>() < 0.1 {
                equipment.put(
                    &EquipmentSlot::CHEST,
                    ItemStack::new(1, &Item::GOLDEN_CHESTPLATE),
                );
            }
            if rand::random::<f32>() < 0.1 {
                equipment.put(
                    &EquipmentSlot::LEGS,
                    ItemStack::new(1, &Item::GOLDEN_LEGGINGS),
                );
            }
            if rand::random::<f32>() < 0.1 {
                equipment.put(&EquipmentSlot::FEET, ItemStack::new(1, &Item::GOLDEN_BOOTS));
            }
        }
    }

    fn mob_init_data_tracker(&self) {
        let entity = self.get_entity();
        let mut meta = Vec::new();
        if self.is_immune_to_zombification() {
            meta.push(Metadata::new(
                tracked_data::piglin::DATA_IMMUNE_TO_ZOMBIFICATION,
                true,
            ));
        }
        if self.is_baby() {
            meta.push(Metadata::new(tracked_data::piglin::DATA_BABY_ID, true));
        }
        if self.is_charging_crossbow() {
            meta.push(Metadata::new(
                tracked_data::piglin::DATA_IS_CHARGING_CROSSBOW,
                true,
            ));
        }
        if self.is_dancing() {
            meta.push(Metadata::new(tracked_data::piglin::DATA_IS_DANCING, true));
        }
        if !meta.is_empty() {
            entity.send_meta_data(&meta, None);
        }
    }

    fn mob_write_nbt(&self, nbt: &mut NbtCompound) {
        // `AbstractPiglin.addAdditionalSaveData` (`AbstractPiglin.java:65-70`) persists the
        // shared zombification timer before the mob-specific fields.
        self.zombification.write_nbt(nbt);
        if self.is_immune_to_zombification() {
            nbt.put_bool("IsImmuneToZombification", true);
        }
        let time_in_overworld = self.time_in_overworld.load(Ordering::Relaxed);
        if time_in_overworld > 0 {
            nbt.put_int("TimeInOverworld", time_in_overworld);
        }
        nbt.put_bool("CanPickUpLoot", true);
        if self.is_baby() {
            nbt.put_bool("IsBaby", true);
        }
        if !self.can_hunt() {
            nbt.put_bool("CannotHunt", true);
        }
        if self.is_charging_crossbow() {
            nbt.put_bool("IsChargingCrossbow", true);
        }
        if self.is_dancing() {
            nbt.put_bool("IsDancing", true);
        }

        let inv = self
            .inventory
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !inv.is_empty() {
            let mut items_tag = Vec::new();
            for item in inv.iter() {
                if !item.is_empty() {
                    let mut item_nbt = NbtCompound::new();
                    item.write_item_stack(&mut item_nbt);
                    items_tag.push(NbtTag::Compound(item_nbt));
                }
            }
            if !items_tag.is_empty() {
                nbt.put_list("Inventory", items_tag);
            }
        }
    }

    fn mob_read_nbt(&self, nbt: &NbtCompound) {
        // `AbstractPiglin.readAdditionalSaveData` (`AbstractPiglin.java:72-78`) restores the
        // shared timer, including vanilla's false/zero defaults for absent fields.
        self.zombification.read_nbt(nbt);
        if let Some(immune) = nbt.get_bool("IsImmuneToZombification") {
            self.set_immune_to_zombification(immune);
        }
        if let Some(time) = nbt.get_int("TimeInOverworld") {
            self.time_in_overworld.store(time, Ordering::Relaxed);
        }
        if let Some(baby) = nbt.get_bool("IsBaby") {
            self.set_baby(baby);
        }
        if let Some(cannot_hunt) = nbt.get_bool("CannotHunt") {
            self.set_cannot_hunt(cannot_hunt);
        }
        if let Some(charging) = nbt.get_bool("IsChargingCrossbow") {
            self.set_charging_crossbow(charging);
        }
        if let Some(dancing) = nbt.get_bool("IsDancing") {
            self.set_dancing(dancing);
        }
        if let Some(inv_list) = nbt.get_list("Inventory") {
            let mut inv = self
                .inventory
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            inv.clear();
            for tag in inv_list {
                if let Some(compound) = tag.extract_compound()
                    && let Some(stack) = ItemStack::read_item_stack(compound)
                {
                    inv.push(stack);
                }
            }
        }
    }

    fn as_crossbow_attack_mob(&self) -> Option<&dyn CrossbowAttackMob> {
        Some(self)
    }

    fn get_base_experience_reward(&self) -> u32 {
        Self::XP_REWARD
    }
}

impl CrossbowAttackMob for PiglinEntity {
    fn set_charging_crossbow(&self, is_charging: bool) {
        self.set_charging_crossbow(is_charging);
    }

    fn is_charging_crossbow(&self) -> bool {
        self.is_charging_crossbow()
    }
}
