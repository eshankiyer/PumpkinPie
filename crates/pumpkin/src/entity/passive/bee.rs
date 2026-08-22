// Legacy invariant checks retained for vanilla behavior; migrate these paths before removing this allow.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use std::sync::{
    Arc, Mutex as StdMutex, Weak,
    atomic::{AtomicBool, AtomicI32, AtomicU8, Ordering::Relaxed},
};

use crossbeam::atomic::AtomicCell;
use pumpkin_data::BlockStateId;
use pumpkin_data::block_properties::{
    BlockProperties, DoubleBlockHalf, TallSeagrassLikeProperties,
};
use pumpkin_data::block_state::BlockState;
use pumpkin_data::damage::DamageType;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_data::tag::{self, Taggable};
use pumpkin_data::{Block, effect::StatusEffect, potion::Effect};
use pumpkin_data::{entity::EntityType, tracked_data};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_nbt::tag::NbtTag;
use pumpkin_protocol::java::client::play::Metadata;
use pumpkin_util::Difficulty;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use rand::RngExt;

use crate::block::entities::BlockEntity;
use crate::block::entities::beehive::{BeehiveBlockEntity, bees_stay_in_hive, is_beehive};
use crate::entity::{
    Entity, EntityBase, EntityBaseFuture, NBTStorage, NbtFuture,
    ageable::{AgeableData, AgeableMob},
    ai::goal::{
        Controls, Goal, GoalFuture, active_target::ActiveTargetGoal, breed::BreedGoal,
        follow_parent::FollowParentGoal, look_around::RandomLookAroundGoal,
        look_at_entity::LookAtEntityGoal, melee_attack::MeleeAttackGoal,
        reset_universal_anger_target::ResetUniversalAngerTargetGoal, revenge::RevengeGoal,
        swim::SwimGoal, wander_around::WanderAroundGoal,
    },
    ai::pathfinder::NavigatorGoal,
    mob::{Mob, MobEntity},
    passive::animal::Animal,
    persistent_anger::PersistentAnger,
    player::Player,
};
use crate::world::World;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_world::world::BlockFlags;
use uuid::Uuid;

/// `Bee.FLAG_ROLL`, `Bee.FLAG_HAS_STUNG`, `Bee.FLAG_HAS_NECTAR`.
const FLAG_ROLL: u8 = 2;
const FLAG_HAS_STUNG: u8 = 4;
const FLAG_HAS_NECTAR: u8 = 8;

/// `Bee.STING_DEATH_COUNTDOWN`.
const STING_DEATH_COUNTDOWN: i32 = 1200;
/// `Bee.TICKS_WITHOUT_NECTAR_BEFORE_GOING_HOME`.
const TICKS_WITHOUT_NECTAR_BEFORE_GOING_HOME: i32 = 3600;
/// `Bee.COOLDOWN_BEFORE_LOCATING_NEW_FLOWER`.
const COOLDOWN_BEFORE_LOCATING_NEW_FLOWER: i32 = 200;
/// `Bee.MIN_FIND_FLOWER_RETRY_COOLDOWN` / `Bee.MAX_FIND_FLOWER_RETRY_COOLDOWN`.
const MIN_FIND_FLOWER_RETRY_COOLDOWN: i32 = 20;
const MAX_FIND_FLOWER_RETRY_COOLDOWN: i32 = 60;
/// `Bee.dropHive`: `remainingCooldownBeforeLocatingNewHive = 200`.
const COOLDOWN_BEFORE_LOCATING_NEW_HIVE: i32 = 200;
/// `Bee.isTooFarAway`: `!closerThan(pos, 48)`.
const TOO_FAR_FROM_HIVE: f64 = 48.0;
/// `BeeGoToHiveGoal.MAX_TRAVELLING_TICKS`.
const MAX_TRAVELLING_TICKS: i32 = 2400;
/// `BeeGoToHiveGoal.TICKS_BEFORE_HIVE_DROP`.
const TICKS_BEFORE_HIVE_DROP: i32 = 60;
/// `BeeGoToHiveGoal.MAX_BLACKLISTED_TARGETS`.
const MAX_BLACKLISTED_TARGETS: usize = 3;
/// `BeeLocateHiveGoal.findNearbyHivesWithSpace` searches the POI manager out to 20 blocks.
const HIVE_SEARCH_RADIUS: i32 = 20;
/// `BeeEnterHiveGoal.canBeeUse`: `hivePos.closerToCenterThan(position(), 2.0)`.
const HIVE_ENTER_DISTANCE: f64 = 2.0;

/// `Bee.doHurtTarget`: `POISON_SECONDS_NORMAL` / `POISON_SECONDS_HARD`.
const fn poison_duration(difficulty: Difficulty) -> Option<i32> {
    match difficulty {
        Difficulty::Normal => Some(10 * 20),
        Difficulty::Hard => Some(18 * 20),
        Difficulty::Peaceful | Difficulty::Easy => None,
    }
}

/// `Bee.customServerAiStep`: `random.nextInt(Mth.clamp(1200 - timeSinceSting, 1, 1200)) == 0`.
const fn sting_death_roll_bound(time_since_sting: i32) -> i32 {
    let remaining = STING_DEATH_COUNTDOWN - time_since_sting;
    if remaining < 1 {
        1
    } else if remaining > STING_DEATH_COUNTDOWN {
        STING_DEATH_COUNTDOWN
    } else {
        remaining
    }
}

/// `Bee.isTiredOfLookingForNectar`.
const fn is_tired_of_looking_for_nectar(ticks_without_nectar: i32) -> bool {
    ticks_without_nectar > TICKS_WITHOUT_NECTAR_BEFORE_GOING_HOME
}

/// Represents a Bee, a neutral flying mob that can pollinate crops and sting attackers.
///
/// Wiki: <https://minecraft.wiki/w/Bee>
pub struct BeeEntity {
    pub mob_entity: MobEntity,
    /// `Bee.DATA_FLAGS_ID`.
    flags: AtomicU8,
    /// `Bee.hivePos`.
    pub hive_pos: AtomicCell<Option<BlockPos>>,
    /// `Bee.savedFlowerPos`.
    pub flower_pos: AtomicCell<Option<BlockPos>>,
    /// `Bee.ticksWithoutNectarSinceExitingHive`.
    ticks_without_nectar: AtomicI32,
    /// `Bee.stayOutOfHiveCountdown`.
    stay_out_of_hive_countdown: AtomicI32,
    /// `Bee.numCropsGrownSincePollination`.
    crops_grown_since_pollination: AtomicI32,
    /// `Bee.timeSinceSting`.
    time_since_sting: AtomicI32,
    /// `Bee.underWaterTicks`.
    under_water_ticks: AtomicI32,
    /// `Bee.remainingCooldownBeforeLocatingNewFlower`.
    flower_cooldown: AtomicI32,
    /// `Bee.remainingCooldownBeforeLocatingNewHive`.
    hive_cooldown: AtomicI32,
    /// `Bee.beePollinateGoal.isPollinating()`, hoisted onto the entity because Pumpkin's goals
    /// are boxed and isolated while vanilla's are inner classes sharing the outer `Bee`.
    pollinating: AtomicBool,
    /// `BeeGoToHiveGoal.blacklistedTargets`, hoisted for the same reason: vanilla's
    /// `BeeLocateHiveGoal` reads and clears the go-to-hive goal's list directly.
    hive_blacklist: StdMutex<Vec<BlockPos>>,
    /// `Entity.tickCount`, needed only for the 20-tick hive-validity sweep in `Bee.aiStep`.
    tick_count: AtomicI32,
    /// `AgeableMob` state: `Bee extends Animal` (`Bee.java:96`), so bees age and breed.
    ageable_data: AgeableData,
    /// `Bee implements NeutralMob` (`Bee.java:96`): `angerEndTime`/`persistentAngerTarget`.
    pub persistent_anger: PersistentAnger,
}

impl BeeEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let mob_entity = MobEntity::new(entity);
        let bee = Self {
            mob_entity,
            flags: AtomicU8::new(0),
            hive_pos: AtomicCell::new(None),
            flower_pos: AtomicCell::new(None),
            ticks_without_nectar: AtomicI32::new(0),
            stay_out_of_hive_countdown: AtomicI32::new(0),
            crops_grown_since_pollination: AtomicI32::new(0),
            time_since_sting: AtomicI32::new(0),
            under_water_ticks: AtomicI32::new(0),
            flower_cooldown: AtomicI32::new(
                rand::rng()
                    .random_range(MIN_FIND_FLOWER_RETRY_COOLDOWN..=MAX_FIND_FLOWER_RETRY_COOLDOWN),
            ),
            hive_cooldown: AtomicI32::new(0),
            pollinating: AtomicBool::new(false),
            hive_blacklist: StdMutex::new(Vec::new()),
            tick_count: AtomicI32::new(0),
            ageable_data: AgeableData::default(),
            persistent_anger: PersistentAnger::default(),
        };
        let mob_arc = Arc::new(bee);
        let bee_weak = Arc::downgrade(&mob_arc);
        let mob_weak: Weak<dyn Mob> = {
            let mob_arc: Arc<dyn Mob> = mob_arc.clone();
            Arc::downgrade(&mob_arc)
        };
        let mob_weak_target = mob_weak.clone();

        {
            let mut goal_selector = mob_arc
                .mob_entity
                .goals_selector
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);

            goal_selector.add_goal(0, Box::new(BeeAttackGoal::new(bee_weak.clone())));
            goal_selector.add_goal(1, Box::new(BeeEnterHiveGoal::new(bee_weak.clone())));
            // `new BreedGoal(this, 1.0)` (`Bee.java:177`). Not ported alongside it:
            // `TemptGoal(this, 1.25, i -> i.is(ItemTags.BEE_FOOD), false)` (`Bee.java:178`) --
            // `tempt.rs` takes a `&'static [&'static Item]`, not an item tag.
            goal_selector.add_goal(2, BreedGoal::new(1.0));
            goal_selector.add_goal(3, Box::new(BeeValidateHiveGoal::new(bee_weak.clone())));
            goal_selector.add_goal(3, Box::new(BeeValidateFlowerGoal::new(bee_weak.clone())));
            goal_selector.add_goal(4, Box::new(BeePollinateGoal::new(bee_weak.clone())));
            // `new FollowParentGoal(this, 1.25)` (`Bee.java:183`).
            goal_selector.add_goal(5, Box::new(FollowParentGoal::new(1.25)));
            goal_selector.add_goal(5, Box::new(BeeLocateHiveGoal::new(bee_weak.clone())));
            goal_selector.add_goal(5, Box::new(BeeGoToHiveGoal::new(bee_weak.clone())));
            goal_selector.add_goal(6, Box::new(BeeGoToKnownFlowerGoal::new(bee_weak.clone())));
            goal_selector.add_goal(7, Box::new(BeeGrowCropGoal::new(bee_weak)));
            goal_selector.add_goal(8, Box::new(WanderAroundGoal::new(1.0)));
            goal_selector.add_goal(9, Box::new(SwimGoal::default()));
            goal_selector.add_goal(
                5,
                LookAtEntityGoal::with_default(mob_weak, &EntityType::PLAYER, 6.0),
            );
            goal_selector.add_goal(6, Box::new(RandomLookAroundGoal::default()));

            let mut target_selector = mob_arc.mob_entity.target_selector.lock().unwrap();
            // `new Bee.BeeHurtByOtherGoal(this).setAlertOthers()` (`Bee.java:192`).
            target_selector.add_goal(1, Box::new(RevengeGoal::new(true)));
            // `new Bee.BeeBecomeAngryTargetGoal(this)` (`Bee.java:193`), which is
            // `NearestAttackableTargetGoal<Player>(bee, Player.class, 10, true, false,
            // bee::isAngryAt)` (`Bee.java:714-717`). The predicate only sees the candidate, so
            // it closes over a weak handle back to this bee to consult its own
            // `PersistentAnger` - the shape `polar_bear.rs:85-118` established.
            let angry_weak = mob_weak_target.clone();
            target_selector.add_goal(
                2,
                Box::new(ActiveTargetGoal::new(
                    &mob_arc.mob_entity,
                    &EntityType::PLAYER,
                    10,
                    true,
                    false,
                    Some(
                        move |target: crate::entity::ai::target_predicate::TargetData,
                              world: Arc<World>| {
                            let angry_weak = angry_weak.clone();
                            async move {
                                let Some(mob) = angry_weak.upgrade() else {
                                    return false;
                                };
                                let Some(anger) = mob.persistent_anger() else {
                                    return false;
                                };
                                if anger.is_angry_at(target.entity_uuid).await {
                                    return true;
                                }
                                let universal_anger =
                                    world.level_info.load().game_rules.universal_anger;
                                anger.is_angry_at_all_players(universal_anger).await
                            }
                        },
                    ),
                )),
            );
            // `new ResetUniversalAngerTargetGoal<>(this, true)` (`Bee.java:194`).
            target_selector.add_goal(3, ResetUniversalAngerTargetGoal::new(true));
        };

        mob_arc
    }

    /// `Bee.setFlag`: read-modify-write on the shared flag byte; vanilla's synched data only
    /// broadcasts when the value actually changes.
    fn set_flag(&self, flag: u8, value: bool) {
        let previous = if value {
            self.flags.fetch_or(flag, Relaxed)
        } else {
            self.flags.fetch_and(!flag, Relaxed)
        };
        let new = if value {
            previous | flag
        } else {
            previous & !flag
        };
        if previous == new {
            return;
        }
        self.mob_entity.living_entity.entity.send_meta_data(
            &[Metadata::new(tracked_data::bee::FLAGS_ID, new as i8)],
            None,
        );
    }

    fn get_flag(&self, flag: u8) -> bool {
        self.flags.load(Relaxed) & flag != 0
    }

    #[must_use]
    pub fn has_nectar(&self) -> bool {
        self.get_flag(FLAG_HAS_NECTAR)
    }

    /// `Bee.setHasNectar`.
    pub fn set_has_nectar(&self, has_nectar: bool) {
        if has_nectar {
            self.ticks_without_nectar.store(0, Relaxed);
        }
        self.set_flag(FLAG_HAS_NECTAR, has_nectar);
    }

    #[must_use]
    pub fn has_stung(&self) -> bool {
        self.get_flag(FLAG_HAS_STUNG)
    }

    fn set_has_stung(&self, has_stung: bool) {
        self.set_flag(FLAG_HAS_STUNG, has_stung);
    }

    #[must_use]
    pub fn is_rolling(&self) -> bool {
        self.get_flag(FLAG_ROLL)
    }

    /// `Bee.dropOffNectar`, called by the hive when a bee is released.
    pub fn drop_off_nectar(&self) {
        self.set_has_nectar(false);
        self.crops_grown_since_pollination.store(0, Relaxed);
    }

    /// `Bee.isTiredOfLookingForNectar`.
    #[must_use]
    pub fn is_tired_of_looking_for_nectar(&self) -> bool {
        is_tired_of_looking_for_nectar(self.ticks_without_nectar.load(Relaxed))
    }

    /// `Bee.setStayOutOfHiveCountdown`.
    pub fn set_stay_out_of_hive_countdown(&self, ticks: i32) {
        self.stay_out_of_hive_countdown.store(ticks, Relaxed);
    }

    /// `Bee.ticksWithoutNectarSinceExitingHive`, for the goals that gate on it.
    #[must_use]
    pub fn ticks_without_nectar(&self) -> i32 {
        self.ticks_without_nectar.load(Relaxed)
    }

    /// `Bee.getCropsGrownSincePollination`.
    #[must_use]
    pub fn crops_grown_since_pollination(&self) -> i32 {
        self.crops_grown_since_pollination.load(Relaxed)
    }

    /// `Bee.incrementNumCropsGrownSincePollination`.
    pub fn increment_crops_grown_since_pollination(&self) {
        self.crops_grown_since_pollination.fetch_add(1, Relaxed);
    }

    /// `Bee.dropFlower`.
    fn drop_flower(&self) {
        self.flower_pos.store(None);
        self.flower_cooldown.store(
            rand::rng()
                .random_range(MIN_FIND_FLOWER_RETRY_COOLDOWN..=MAX_FIND_FLOWER_RETRY_COOLDOWN),
            Relaxed,
        );
    }

    /// `Bee.resetTicksWithoutNectarSinceExitingHive`.
    fn reset_ticks_without_nectar(&self) {
        self.ticks_without_nectar.store(0, Relaxed);
    }

    /// `Bee.setHivePos`.
    pub fn set_hive_pos(&self, hive_pos: Option<BlockPos>) {
        self.hive_pos.store(hive_pos);
    }

    /// `Bee.hasHive`.
    #[must_use]
    pub fn has_hive(&self) -> bool {
        self.hive_pos.load().is_some()
    }

    /// `Bee.dropHive`.
    fn drop_hive(&self) {
        self.hive_pos.store(None);
        self.hive_cooldown
            .store(COOLDOWN_BEFORE_LOCATING_NEW_HIVE, Relaxed);
    }

    /// `BeeGoToHiveGoal.blacklistTarget`, capped at `MAX_BLACKLISTED_TARGETS`.
    fn blacklist_hive(&self, pos: BlockPos) {
        let mut blacklist = self
            .hive_blacklist
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        blacklist.push(pos);
        while blacklist.len() > MAX_BLACKLISTED_TARGETS {
            blacklist.remove(0);
        }
    }

    /// `BeeGoToHiveGoal.isTargetBlacklisted`.
    fn is_hive_blacklisted(&self, pos: BlockPos) -> bool {
        self.hive_blacklist
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .contains(&pos)
    }

    /// `BeeGoToHiveGoal.clearBlacklist`.
    fn clear_hive_blacklist(&self) {
        self.hive_blacklist
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
    }

    /// `BeeGoToHiveGoal.dropAndBlacklistHive`.
    fn drop_and_blacklist_hive(&self) {
        if let Some(hive_pos) = self.hive_pos.load() {
            self.blacklist_hive(hive_pos);
        }
        self.drop_hive();
    }

    /// `Bee.isTooFarAway`.
    #[must_use]
    fn is_too_far_away(&self, pos: BlockPos) -> bool {
        !self.closer_than(pos, TOO_FAR_FROM_HIVE)
    }

    /// `Entity.closerThan(BlockPos, double)`, measured against the block centre as vanilla's
    /// `BlockPos.closerToCenterThan` does.
    #[must_use]
    fn closer_than(&self, pos: BlockPos, distance: f64) -> bool {
        let entity_pos = self.mob_entity.living_entity.entity.pos.load();
        let centre = Vector3::new(
            f64::from(pos.0.x) + 0.5,
            f64::from(pos.0.y) + 0.5,
            f64::from(pos.0.z) + 0.5,
        );
        entity_pos.squared_distance_to_vec(&centre) < distance * distance
    }

    /// `Bee.getBeehiveBlockEntity`. Returns the erased handle because `Arc<dyn BlockEntity>`
    /// cannot be downcast in place here; pair it with [`as_hive`].
    #[must_use]
    pub fn get_beehive_block_entity(&self) -> Option<Arc<dyn BlockEntity>> {
        let hive_pos = self.hive_pos.load()?;
        if self.is_too_far_away(hive_pos) {
            return None;
        }
        let world = self.mob_entity.living_entity.entity.world.load();
        let block_entity = world.get_block_entity(&hive_pos)?;
        // `getBlockEntity(pos, BlockEntityTypes.BEEHIVE)` is a typed lookup: a non-beehive
        // block entity at that position is not a valid hive.
        (block_entity.resource_location() == BeehiveBlockEntity::ID).then_some(block_entity)
    }

    /// `Bee.isHiveValid`.
    #[must_use]
    pub fn is_hive_valid(&self) -> bool {
        self.get_beehive_block_entity().is_some()
    }

    /// `Bee.isHiveNearFire`.
    fn is_hive_near_fire(&self) -> bool {
        let world = self.mob_entity.living_entity.entity.world.load();
        let Some(handle) = self.get_beehive_block_entity() else {
            return false;
        };
        as_hive(&handle).is_some_and(|hive| hive.is_fire_nearby(&world))
    }

    /// `Bee.wantsToEnterHive`.
    async fn wants_to_enter_hive(&self) -> bool {
        if self.stay_out_of_hive_countdown.load(Relaxed) > 0
            || self.pollinating.load(Relaxed)
            || self.has_stung()
            || self.mob_entity.get_target().await.is_some()
        {
            return false;
        }
        let world = self.mob_entity.living_entity.entity.world.load();
        let wants = self.has_nectar()
            || self.is_tired_of_looking_for_nectar()
            || bees_stay_in_hive(&world).await;
        wants && !self.is_hive_near_fire()
    }
}

/// Downcasts an erased block-entity handle to a beehive.
#[must_use]
pub fn as_hive(handle: &Arc<dyn BlockEntity>) -> Option<&BeehiveBlockEntity> {
    handle.as_any().downcast_ref::<BeehiveBlockEntity>()
}

/// Downcasts an `EntityBase` handle to a bee, for callers that only hold `Arc<dyn EntityBase>`
/// (the hive block entity releasing occupants, above all).
#[must_use]
pub fn as_bee(entity: &Arc<dyn EntityBase>) -> Option<&BeeEntity> {
    entity.get_mob().and_then(Mob::get_bee)
}

/// `Bee.attractsBees`.
fn attracts_bees(block: &Block, state: &BlockState) -> bool {
    if !block.has_tag(&tag::Block::MINECRAFT_BEE_ATTRACTIVE) {
        return false;
    }
    if state.is_waterlogged() {
        return false;
    }
    if block.id == Block::SUNFLOWER.id {
        return TallSeagrassLikeProperties::from_state_id(state.id, block).half
            == DoubleBlockHalf::Upper;
    }
    true
}

/// `Bee.BeeAttackGoal`: a `MeleeAttackGoal` that stops once the bee has stung.
///
/// `canUse`/`canContinueToUse` also require `isAngry()` (`Bee.java:705`/`710`), now backed by
/// `PersistentAnger`.
pub struct BeeAttackGoal {
    bee: Weak<BeeEntity>,
    melee: MeleeAttackGoal,
}

impl BeeAttackGoal {
    #[must_use]
    pub fn new(bee: Weak<BeeEntity>) -> Self {
        Self {
            bee,
            melee: MeleeAttackGoal::new(1.4, true),
        }
    }

    fn can_sting(&self) -> bool {
        self.bee
            .upgrade()
            .is_some_and(|bee| !bee.has_stung() && bee.persistent_anger.is_angry())
    }
}

impl Goal for BeeAttackGoal {
    fn can_start<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        Box::pin(async move { self.can_sting() && self.melee.can_start(mob).await })
    }

    fn should_continue<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        Box::pin(async move { self.can_sting() && self.melee.should_continue(mob).await })
    }

    fn start<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        self.melee.start(mob)
    }

    fn stop<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        self.melee.stop(mob)
    }

    fn tick<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        self.melee.tick(mob)
    }

    fn should_run_every_tick(&self) -> bool {
        self.melee.should_run_every_tick()
    }

    fn controls(&self) -> Controls {
        self.melee.controls()
    }
}

/// `Bee.BeePollinateGoal`.
///
/// Ported with two documented reductions, both forced by engine gaps rather than choice:
///
/// - Vanilla filters candidate flowers through `navigation.createPath(pos, 1).canReach()` and
///   remembers unreachable ones in `unreachableFlowerCache` for 600 ticks. Pumpkin's `Navigator`
///   exposes no path-reachability query, so the reachability filter and its cache are dropped;
///   the retry cooldowns (`MIN`/`MAX_FIND_FLOWER_RETRY_COOLDOWN` on failure,
///   `COOLDOWN_BEFORE_LOCATING_NEW_FLOWER` on stop) are kept and are what bound the scan cost.
/// - Vanilla hovers inside the flower with `MoveControl.setWantedPosition` at
///   `HOVER_HEIGHT_WITHIN_FLOWER`, re-jittering by `HOVER_POS_OFFSET` on a 1-in-25 roll.
///   Pumpkin's `MoveControl` only produces yaw plus forward input and cannot hold a Y target,
///   so the hover jitter is dropped; the bee stops navigating on arrival and pollinates in
///   place. The pollination timings (`MIN_POLLINATION_TICKS`, `MAX_POLLINATING_TICKS`, the
///   1-in-5 continue roll and the pollinate sound throttle) are unchanged.
pub struct BeePollinateGoal {
    bee: Weak<BeeEntity>,
    goal_control: Controls,
    successful_pollinating_ticks: i32,
    last_sound_played_tick: i32,
    pollinating: bool,
    pollinating_ticks: i32,
}

/// `BeePollinateGoal.MIN_POLLINATION_TICKS`.
const MIN_POLLINATION_TICKS: i32 = 400;
/// `BeePollinateGoal.MAX_POLLINATING_TICKS`.
const MAX_POLLINATING_TICKS: i32 = 600;
/// `BeePollinateGoal.FLOWER_SEARCH_RADIUS`.
const FLOWER_SEARCH_RADIUS: i32 = 5;
/// `BeePollinateGoal.HOVER_HEIGHT_WITHIN_FLOWER`.
const HOVER_HEIGHT_WITHIN_FLOWER: f64 = 0.6;

impl BeePollinateGoal {
    #[must_use]
    pub const fn new(bee: Weak<BeeEntity>) -> Self {
        Self {
            bee,
            goal_control: Controls::MOVE,
            successful_pollinating_ticks: 0,
            last_sound_played_tick: 0,
            pollinating: false,
            pollinating_ticks: 0,
        }
    }

    const fn has_pollinated_long_enough(&self) -> bool {
        self.successful_pollinating_ticks > MIN_POLLINATION_TICKS
    }

    /// `BeePollinateGoal.findNearbyFlower`, iterating `BlockPos.withinManhattan(pos, 5, 5, 5)`
    /// in vanilla's increasing-Manhattan-distance order so the closest flower wins.
    fn find_nearby_flower(mob: &dyn Mob) -> Option<BlockPos> {
        let origin = mob.get_entity().block_pos.load();
        let world = mob.get_entity().world.load();
        let max_depth = FLOWER_SEARCH_RADIUS * 3;

        for depth in 0..=max_depth {
            let max_x = FLOWER_SEARCH_RADIUS.min(depth);
            for x in -max_x..=max_x {
                let max_y = FLOWER_SEARCH_RADIUS.min(depth - x.abs());
                for y in -max_y..=max_y {
                    let z = depth - x.abs() - y.abs();
                    if z > FLOWER_SEARCH_RADIUS {
                        continue;
                    }
                    for z in [z, -z] {
                        let pos = BlockPos::new(origin.0.x + x, origin.0.y + y, origin.0.z + z);
                        let (block, state) = world.get_block_and_state(&pos);
                        if attracts_bees(block, state) {
                            return Some(pos);
                        }
                        if z == 0 {
                            break;
                        }
                    }
                }
            }
        }

        None
    }

    /// `Vec3.atBottomCenterOf(savedFlowerPos).add(0.0, HOVER_HEIGHT_WITHIN_FLOWER, 0.0)`.
    fn flower_target(flower_pos: BlockPos) -> Vector3<f64> {
        Vector3::new(
            f64::from(flower_pos.0.x) + 0.5,
            f64::from(flower_pos.0.y) + HOVER_HEIGHT_WITHIN_FLOWER,
            f64::from(flower_pos.0.z) + 0.5,
        )
    }

    async fn is_raining(mob: &dyn Mob) -> bool {
        mob.get_entity().world.load().weather.lock().await.raining
    }
}

impl Goal for BeePollinateGoal {
    fn can_start<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        Box::pin(async move {
            let Some(bee) = self.bee.upgrade() else {
                return false;
            };
            if bee.flower_cooldown.load(Relaxed) > 0 || bee.has_nectar() {
                return false;
            }
            if Self::is_raining(mob).await {
                return false;
            }

            let Some(flower_pos) = Self::find_nearby_flower(mob) else {
                bee.flower_cooldown.store(
                    mob.get_random().random_range(
                        MIN_FIND_FLOWER_RETRY_COOLDOWN..=MAX_FIND_FLOWER_RETRY_COOLDOWN,
                    ),
                    Relaxed,
                );
                return false;
            };

            bee.flower_pos.store(Some(flower_pos));
            let pos = mob.get_entity().pos.load();
            let mut navigator = mob.get_mob_entity().navigator.lock().unwrap();
            navigator.set_progress(NavigatorGoal::new(
                pos,
                Self::flower_target(flower_pos),
                1.2,
            ));
            true
        })
    }

    fn should_continue<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        Box::pin(async move {
            let Some(bee) = self.bee.upgrade() else {
                return false;
            };
            if !self.pollinating || bee.flower_pos.load().is_none() {
                return false;
            }
            if Self::is_raining(mob).await {
                return false;
            }
            if self.has_pollinated_long_enough() {
                return mob.get_random().random::<f32>() < 0.2;
            }
            true
        })
    }

    fn start<'a>(&'a mut self, _mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async move {
            self.successful_pollinating_ticks = 0;
            self.pollinating_ticks = 0;
            self.last_sound_played_tick = 0;
            self.pollinating = true;
            if let Some(bee) = self.bee.upgrade() {
                bee.pollinating.store(true, Relaxed);
                bee.reset_ticks_without_nectar();
            }
        })
    }

    fn stop<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async move {
            if let Some(bee) = self.bee.upgrade() {
                if self.has_pollinated_long_enough() {
                    bee.set_has_nectar(true);
                }
                bee.flower_cooldown
                    .store(COOLDOWN_BEFORE_LOCATING_NEW_FLOWER, Relaxed);
                bee.pollinating.store(false, Relaxed);
            }
            self.pollinating = false;
            mob.get_mob_entity().navigator.lock().unwrap().stop();
        })
    }

    fn tick<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async move {
            let Some(bee) = self.bee.upgrade() else {
                return;
            };
            let Some(flower_pos) = bee.flower_pos.load() else {
                return;
            };

            self.pollinating_ticks += 1;
            if self.pollinating_ticks > MAX_POLLINATING_TICKS {
                bee.drop_flower();
                self.pollinating = false;
                bee.pollinating.store(false, Relaxed);
                bee.flower_cooldown
                    .store(COOLDOWN_BEFORE_LOCATING_NEW_FLOWER, Relaxed);
                return;
            }

            let target = Self::flower_target(flower_pos);
            let pos = mob.get_entity().pos.load();
            if (target - pos).length() > 1.0 {
                let mut navigator = mob.get_mob_entity().navigator.lock().unwrap();
                if navigator.is_idle() {
                    navigator.set_progress(NavigatorGoal::new(pos, target, 1.2));
                }
                return;
            }

            mob.get_mob_entity().navigator.lock().unwrap().stop();
            mob.get_mob_entity()
                .look_control
                .lock()
                .unwrap()
                .look_at(mob, target.x, target.y, target.z);

            self.successful_pollinating_ticks += 1;
            if mob.get_random().random::<f32>() < 0.05
                && self.successful_pollinating_ticks > self.last_sound_played_tick + 60
            {
                self.last_sound_played_tick = self.successful_pollinating_ticks;
                let entity = mob.get_entity();
                entity.world.load().play_sound(
                    Sound::EntityBeePollinate,
                    SoundCategory::Neutral,
                    &entity.pos.load(),
                );
            }
        })
    }

    fn should_run_every_tick(&self) -> bool {
        true
    }

    fn controls(&self) -> Controls {
        self.goal_control
    }
}

/// `Bee.BeeEnterHiveGoal`.
pub struct BeeEnterHiveGoal {
    bee: Weak<BeeEntity>,
}

impl BeeEnterHiveGoal {
    #[must_use]
    pub const fn new(bee: Weak<BeeEntity>) -> Self {
        Self { bee }
    }
}

impl Goal for BeeEnterHiveGoal {
    fn can_start<'a>(&'a mut self, _mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        Box::pin(async move {
            let Some(bee) = self.bee.upgrade() else {
                return false;
            };
            let Some(hive_pos) = bee.hive_pos.load() else {
                return false;
            };
            if !bee.wants_to_enter_hive().await || !bee.closer_than(hive_pos, HIVE_ENTER_DISTANCE) {
                return false;
            }
            let Some(handle) = bee.get_beehive_block_entity() else {
                return false;
            };
            let Some(hive) = as_hive(&handle) else {
                return false;
            };
            if hive.is_full().await {
                // Vanilla forgets a full hive outright rather than hovering at its mouth.
                bee.hive_pos.store(None);
                return false;
            }
            true
        })
    }

    /// `BeeEnterHiveGoal.canBeeContinueToUse` is unconditionally false: entering is a one-shot.
    fn should_continue<'a>(&'a mut self, _mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        Box::pin(async { false })
    }

    fn start<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async move {
            let Some(bee) = self.bee.upgrade() else {
                return;
            };
            let Some(handle) = bee.get_beehive_block_entity() else {
                return;
            };
            let Some(hive) = as_hive(&handle) else {
                return;
            };
            let world = mob.get_entity().world.load();
            let flower_pos = bee.flower_pos.load();
            let entity: Arc<dyn EntityBase> = bee;
            hive.add_occupant(&world, &entity, flower_pos).await;
        })
    }
}

/// `Bee.BeeLocateHiveGoal`.
///
/// Documented reduction: vanilla queries the `PoiManager` for `PoiTypeTags.BEE_HOME` records
/// within 20 blocks. Pumpkin has no POI manager, so this scans the same 20-block radius for
/// blocks in `BlockTags.BEEHIVES` directly. The scan runs at most once every
/// `COOLDOWN_BEFORE_LOCATING_NEW_HIVE` ticks, and only for a bee that already wants to go home,
/// which is what bounds its cost; it also stops at the first `MAX_OCCUPANTS`-worth of
/// candidates so a bee in a large apiary does not walk the whole volume.
pub struct BeeLocateHiveGoal {
    bee: Weak<BeeEntity>,
}

/// How many candidate hives the reduced scan collects before it stops looking.
const MAX_HIVE_CANDIDATES: usize = 8;

impl BeeLocateHiveGoal {
    #[must_use]
    pub const fn new(bee: Weak<BeeEntity>) -> Self {
        Self { bee }
    }

    /// `BeeLocateHiveGoal.findNearbyHivesWithSpace`, sorted by squared distance as vanilla does.
    async fn find_nearby_hives_with_space(bee: &BeeEntity) -> Vec<BlockPos> {
        let origin = bee.mob_entity.living_entity.entity.block_pos.load();
        let world = bee.mob_entity.living_entity.entity.world.load();

        let mut candidates: Vec<(i64, BlockPos)> = Vec::new();
        'scan: for x in -HIVE_SEARCH_RADIUS..=HIVE_SEARCH_RADIUS {
            for y in -HIVE_SEARCH_RADIUS..=HIVE_SEARCH_RADIUS {
                for z in -HIVE_SEARCH_RADIUS..=HIVE_SEARCH_RADIUS {
                    let pos = BlockPos::new(origin.0.x + x, origin.0.y + y, origin.0.z + z);
                    if !is_beehive(world.get_block(&pos)) {
                        continue;
                    }
                    let Some(handle) = world.get_block_entity(&pos) else {
                        continue;
                    };
                    let Some(hive) = as_hive(&handle) else {
                        continue;
                    };
                    if hive.is_full().await {
                        continue;
                    }
                    let distance = i64::from(x) * i64::from(x)
                        + i64::from(y) * i64::from(y)
                        + i64::from(z) * i64::from(z);
                    candidates.push((distance, pos));
                    if candidates.len() >= MAX_HIVE_CANDIDATES {
                        break 'scan;
                    }
                }
            }
        }

        candidates.sort_unstable_by_key(|(distance, _)| *distance);
        candidates.into_iter().map(|(_, pos)| pos).collect()
    }
}

impl Goal for BeeLocateHiveGoal {
    fn can_start<'a>(&'a mut self, _mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        Box::pin(async move {
            let Some(bee) = self.bee.upgrade() else {
                return false;
            };
            bee.hive_cooldown.load(Relaxed) == 0
                && !bee.has_hive()
                && bee.wants_to_enter_hive().await
        })
    }

    fn should_continue<'a>(&'a mut self, _mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        Box::pin(async { false })
    }

    fn start<'a>(&'a mut self, _mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async move {
            let Some(bee) = self.bee.upgrade() else {
                return;
            };
            bee.hive_cooldown
                .store(COOLDOWN_BEFORE_LOCATING_NEW_HIVE, Relaxed);
            let hives = Self::find_nearby_hives_with_space(&bee).await;
            let Some(&first) = hives.first() else {
                return;
            };
            for pos in &hives {
                if !bee.is_hive_blacklisted(*pos) {
                    bee.hive_pos.store(Some(*pos));
                    return;
                }
            }
            bee.clear_hive_blacklist();
            bee.hive_pos.store(Some(first));
        })
    }
}

/// `Bee.BeeGoToHiveGoal`.
///
/// Documented reductions, both forced by `Navigator` rather than choice:
///
/// - Vanilla splits travel into `pathfindRandomlyTowards` beyond 16 blocks and
///   `pathfindDirectlyTowards` inside it, and drops-and-blacklists the hive when
///   `path.canReach()` is false. Pumpkin's `Navigator` is a direct-line mover with no path
///   object and no reachability query, so a single direct goal is used at both ranges and the
///   unreachable-hive case falls through to the stuck detector below.
/// - `ticksStuck` compares the freshly computed path against the previous one. With no path to
///   compare, this counts ticks during which the bee's squared distance to the hive has not
///   improved, and drops the hive after the same `TICKS_BEFORE_HIVE_DROP`.
///
/// The `MAX_TRAVELLING_TICKS` budget, `isTooFarAway` abandonment, the `BlockTags.BEEHIVES`
/// check and the blacklist are unchanged.
pub struct BeeGoToHiveGoal {
    bee: Weak<BeeEntity>,
    travelling_ticks: i32,
    ticks_stuck: i32,
    best_distance: f64,
}

impl BeeGoToHiveGoal {
    #[must_use]
    pub const fn new(bee: Weak<BeeEntity>) -> Self {
        Self {
            bee,
            travelling_ticks: 0,
            ticks_stuck: 0,
            best_distance: f64::MAX,
        }
    }

    /// `BeeGoToHiveGoal.hasReachedTarget`, reduced to the distance half.
    fn has_reached_target(bee: &BeeEntity, hive_pos: BlockPos) -> bool {
        bee.closer_than(hive_pos, HIVE_ENTER_DISTANCE)
    }

    async fn can_use(bee: &BeeEntity) -> bool {
        let Some(hive_pos) = bee.hive_pos.load() else {
            return false;
        };
        if bee.is_too_far_away(hive_pos) || Self::has_reached_target(bee, hive_pos) {
            return false;
        }
        if !bee.wants_to_enter_hive().await {
            return false;
        }
        let world = bee.mob_entity.living_entity.entity.world.load();
        is_beehive(world.get_block(&hive_pos))
    }

    fn hive_target(hive_pos: BlockPos) -> Vector3<f64> {
        Vector3::new(
            f64::from(hive_pos.0.x) + 0.5,
            f64::from(hive_pos.0.y) + 0.5,
            f64::from(hive_pos.0.z) + 0.5,
        )
    }
}

impl Goal for BeeGoToHiveGoal {
    fn can_start<'a>(&'a mut self, _mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        Box::pin(async move {
            let Some(bee) = self.bee.upgrade() else {
                return false;
            };
            Self::can_use(&bee).await
        })
    }

    fn start<'a>(&'a mut self, _mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async move {
            self.travelling_ticks = 0;
            self.ticks_stuck = 0;
            self.best_distance = f64::MAX;
        })
    }

    fn stop<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async move {
            self.travelling_ticks = 0;
            self.ticks_stuck = 0;
            self.best_distance = f64::MAX;
            mob.get_mob_entity().navigator.lock().unwrap().stop();
        })
    }

    fn tick<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async move {
            let Some(bee) = self.bee.upgrade() else {
                return;
            };
            let Some(hive_pos) = bee.hive_pos.load() else {
                return;
            };

            self.travelling_ticks += 1;
            if self.travelling_ticks > MAX_TRAVELLING_TICKS {
                bee.drop_and_blacklist_hive();
                return;
            }

            if bee.is_too_far_away(hive_pos) {
                bee.drop_hive();
                return;
            }

            let pos = mob.get_entity().pos.load();
            let target = Self::hive_target(hive_pos);
            let distance = (target - pos).length_squared();
            if distance < self.best_distance {
                self.best_distance = distance;
                self.ticks_stuck = 0;
            } else {
                self.ticks_stuck += 1;
                if self.ticks_stuck > TICKS_BEFORE_HIVE_DROP {
                    bee.drop_and_blacklist_hive();
                    return;
                }
            }

            let mut navigator = mob.get_mob_entity().navigator.lock().unwrap();
            if navigator.is_idle() {
                navigator.set_progress(NavigatorGoal::new(pos, target, 1.0));
            }
        })
    }

    fn should_run_every_tick(&self) -> bool {
        true
    }

    fn controls(&self) -> Controls {
        Controls::MOVE
    }
}

/// `BlockPos.CODEC` serializes as an `[x, y, z]` int array.
fn block_pos_to_nbt(pos: BlockPos) -> NbtTag {
    NbtTag::IntArray(vec![pos.0.x, pos.0.y, pos.0.z])
}

fn block_pos_from_nbt(nbt: &NbtCompound, name: &str) -> Option<BlockPos> {
    let &[x, y, z] = nbt.get_int_array(name)? else {
        return None;
    };
    Some(BlockPos::new(x, y, z))
}

impl AgeableMob for BeeEntity {
    fn get_ageable_data(&self) -> &AgeableData {
        &self.ageable_data
    }
}

impl Animal for BeeEntity {
    /// `Bee.isFood` (`Bee.java:576-578`): `itemStack.is(ItemTags.BEE_FOOD)`.
    fn is_food(&self, item_stack: &ItemStack) -> bool {
        item_stack.item.has_tag(&tag::Item::MINECRAFT_BEE_FOOD)
    }
}

impl NBTStorage for BeeEntity {
    fn write_nbt<'a>(&'a self, nbt: &'a mut NbtCompound) -> NbtFuture<'a, ()> {
        Box::pin(async move {
            self.mob_entity.living_entity.write_nbt(nbt).await;
            self.write_ageable_nbt(nbt);
            self.write_animal_nbt(nbt);
            self.persistent_anger.write_nbt(nbt).await;
            if let Some(hive_pos) = self.hive_pos.load() {
                nbt.put("hive_pos", block_pos_to_nbt(hive_pos));
            }
            if let Some(flower_pos) = self.flower_pos.load() {
                nbt.put("flower_pos", block_pos_to_nbt(flower_pos));
            }
            nbt.put_bool("HasNectar", self.has_nectar());
            nbt.put_bool("HasStung", self.has_stung());
            nbt.put_int(
                "TicksSincePollination",
                self.ticks_without_nectar.load(Relaxed),
            );
            nbt.put_int(
                "CannotEnterHiveTicks",
                self.stay_out_of_hive_countdown.load(Relaxed),
            );
            nbt.put_int(
                "CropsGrownSincePollination",
                self.crops_grown_since_pollination.load(Relaxed),
            );
        })
    }

    fn read_nbt_non_mut<'a>(&'a self, nbt: &'a NbtCompound) -> NbtFuture<'a, ()> {
        Box::pin(async move {
            self.mob_entity.living_entity.read_nbt_non_mut(nbt).await;
            self.read_ageable_nbt(nbt);
            self.read_animal_nbt(nbt);
            self.persistent_anger.read_nbt(nbt).await;
            // Store the flag bits directly: the entity has no viewers yet during load, so the
            // byte is published by `mob_init_data_tracker` instead of broadcast from here.
            let mut flags = 0u8;
            if nbt.get_bool("HasNectar").unwrap_or(false) {
                flags |= FLAG_HAS_NECTAR;
            }
            if nbt.get_bool("HasStung").unwrap_or(false) {
                flags |= FLAG_HAS_STUNG;
            }
            self.flags.store(flags, Relaxed);
            self.ticks_without_nectar
                .store(nbt.get_int("TicksSincePollination").unwrap_or(0), Relaxed);
            self.stay_out_of_hive_countdown
                .store(nbt.get_int("CannotEnterHiveTicks").unwrap_or(0), Relaxed);
            self.crops_grown_since_pollination.store(
                nbt.get_int("CropsGrownSincePollination").unwrap_or(0),
                Relaxed,
            );
            self.hive_pos.store(block_pos_from_nbt(nbt, "hive_pos"));
            self.flower_pos.store(block_pos_from_nbt(nbt, "flower_pos"));
        })
    }
}

impl Mob for BeeEntity {
    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    fn get_bee(&self) -> Option<&Self> {
        Some(self)
    }

    fn persistent_anger(&self) -> Option<&PersistentAnger> {
        Some(&self.persistent_anger)
    }

    fn mob_interact<'a>(
        &'a self,
        player: &'a Arc<Player>,
        item_stack: &'a mut ItemStack,
    ) -> EntityBaseFuture<'a, bool> {
        self.animal_interact(player, item_stack, Sound::EntityBeeLoop)
    }

    /// `Bee.getBreedOffspring` (`Bee.java:604`): a plain new bee, no inherited state.
    fn create_offspring<'a>(
        &'a self,
        _mate: &'a dyn EntityBase,
        world: &'a Arc<World>,
    ) -> EntityBaseFuture<'a, Option<Arc<dyn EntityBase>>> {
        Box::pin(async move {
            let entity = self.get_entity();
            Some(crate::entity::r#type::from_type(
                entity.entity_type,
                entity.pos.load(),
                world,
                Uuid::new_v4(),
            ))
        })
    }

    fn mob_init_data_tracker(&self) -> EntityBaseFuture<'_, ()> {
        Box::pin(async move {
            let entity = &self.mob_entity.living_entity.entity;
            entity.send_meta_data(
                &[Metadata::new(
                    tracked_data::bee::FLAGS_ID,
                    self.flags.load(Relaxed) as i8,
                )],
                None,
            );
            if entity.age.load(Relaxed) < 0 {
                entity.send_meta_data(
                    &[Metadata::new(tracked_data::bee::DATA_BABY_ID, true)],
                    None,
                );
            }
        })
    }

    /// `Bee.doHurtTarget`: the poison, the stung flag and the sting sound are all gated on the
    /// hit actually landing, which is what `Mob::try_attack` gates this hook on.
    fn on_successful_attack<'a>(&'a self, target: &'a dyn EntityBase) -> EntityBaseFuture<'a, ()> {
        Box::pin(async move {
            if let Some(living) = target.get_living_entity() {
                living.add_stinger();
                if let Some(duration) =
                    poison_duration(self.get_entity().world.load().level_info.load().difficulty)
                {
                    living
                        .add_effect(Effect {
                            effect_type: &StatusEffect::POISON,
                            duration,
                            amplifier: 0,
                            ambient: false,
                            show_particles: true,
                            show_icon: true,
                            blend: false,
                        })
                        .await;
                }
            }

            self.set_has_stung(true);
            // `Bee.doHurtTarget` calls `stopBeingAngry`.
            self.persistent_anger.stop_being_angry().await;
            self.set_mob_target(None).await;
            let entity = &self.mob_entity.living_entity.entity;
            entity.world.load().play_sound(
                Sound::EntityBeeSting,
                SoundCategory::Neutral,
                &entity.pos.load(),
            );
        })
    }

    /// `Bee.aiStep` and `Bee.customServerAiStep`.
    fn mob_tick<'a>(&'a self, caller: &'a Arc<dyn EntityBase>) -> EntityBaseFuture<'a, ()> {
        Box::pin(async move {
            let living = &self.mob_entity.living_entity;
            if living.dead.load(Relaxed) {
                return;
            }

            self.persistent_anger.tick().await;
            // Simplified `NeutralMob.updatePersistentAnger(level, true)`, the same shape
            // `polar_bear.rs` uses: adopt whatever target the revenge goal set as the anger
            // target and (re)start the timer.
            if let Some(target) = self.mob_entity.get_target().await {
                let target_uuid = target.get_entity().entity_uuid;
                if !self.persistent_anger.is_angry_at(target_uuid).await {
                    self.persistent_anger.set_angry_at(Some(target_uuid)).await;
                    self.persistent_anger.start_timer();
                }
            }

            if self.stay_out_of_hive_countdown.load(Relaxed) > 0 {
                self.stay_out_of_hive_countdown.fetch_sub(1, Relaxed);
            }

            if self.flower_cooldown.load(Relaxed) > 0 {
                self.flower_cooldown.fetch_sub(1, Relaxed);
            }

            if self.hive_cooldown.load(Relaxed) > 0 {
                self.hive_cooldown.fetch_sub(1, Relaxed);
            }

            // `Bee.aiStep`: `if (this.tickCount % 20 == 0 && !this.isHiveValid()) hivePos = null`.
            let tick_count = self.tick_count.fetch_add(1, Relaxed) + 1;
            if tick_count % 20 == 0 && self.hive_pos.load().is_some() && !self.is_hive_valid() {
                self.hive_pos.store(None);
            }

            if living.is_in_water() {
                self.under_water_ticks.fetch_add(1, Relaxed);
            } else {
                self.under_water_ticks.store(0, Relaxed);
            }

            if self.under_water_ticks.load(Relaxed) > 20 {
                caller.damage(caller.as_ref(), 1.0, DamageType::DROWN).await;
            }

            if self.has_stung() {
                let time_since_sting = self.time_since_sting.fetch_add(1, Relaxed) + 1;
                if time_since_sting % 5 == 0
                    && rand::rng().random_range(0..sting_death_roll_bound(time_since_sting)) == 0
                {
                    let health = living.health.load();
                    caller
                        .damage(caller.as_ref(), health, DamageType::GENERIC)
                        .await;
                }
            }

            if !self.has_nectar() {
                self.ticks_without_nectar.fetch_add(1, Relaxed);
            }
        })
    }
}

/// `ValidateHiveGoal.VALIDATE_HIVE_COOLDOWN` / `ValidateFlowerGoal.validateFlowerCooldown`:
/// `Mth.nextInt(random, 20, 40)`, inclusive on both ends.
const VALIDATE_COOLDOWN_MIN: i64 = 20;
const VALIDATE_COOLDOWN_MAX: i64 = 40;

/// `BeeGoToKnownFlowerGoal.wantsToGoToKnownFlower`: `ticksWithoutNectarSinceExitingHive > 600`.
const TICKS_WITHOUT_NECTAR_BEFORE_SEEKING_KNOWN_FLOWER: i32 = 600;
/// `BeeGoToKnownFlowerGoal.MAX_TRAVELLING_TICKS`.
const MAX_FLOWER_TRAVELLING_TICKS: i32 = 2400;
/// `BeeGrowCropGoal.GROW_CHANCE`.
const GROW_CHANCE: i32 = 30;
/// `BeeGrowCropGoal.canBeeUse`: a bee stops fertilising after ten crops per load of nectar.
const MAX_CROPS_GROWN_PER_POLLINATION: i32 = 10;

/// Builds the state id of `block` with its `age` property set to `age`, or `None` when the block
/// has no such state. Used by the max-age table's test.
#[cfg(test)]
fn state_id_with_age(block: &Block, age: u8) -> Option<BlockStateId> {
    let props = block.properties(block.default_state.id)?.to_props();
    let age = age.to_string();
    let updated: Vec<(&str, &str)> = props
        .iter()
        .map(|(name, value)| {
            if *name == "age" {
                (*name, age.as_str())
            } else {
                (*name, *value)
            }
        })
        .collect();
    Some(block.from_properties(&updated).to_state_id(block))
}

/// `Bee.BaseBeeGoal.canUse`/`canContinueToUse`'s shared `!Bee.this.isAngry()` gate.
///
/// Backed by `PersistentAnger` (`entity/persistent_anger.rs`), Pumpkin's `NeutralMob`
/// equivalent, so this is now vanilla's `isAngry()` rather than a target-presence stand-in.
fn bee_is_angry(bee: &BeeEntity) -> bool {
    bee.persistent_anger.is_angry()
}

/// `BeeGrowCropGoal.tick`'s per-block max age, which vanilla reads off each block class
/// (`CropBlock.getMaxAge`, `StemBlock.AGE`'s range, `SweetBerryBushBlock.AGE`'s range). There is
/// no generic max-age query on `Block` here, so the five families are tabulated.
///
/// Two tag members are deliberately absent. `cave_vines`/`cave_vines_plant` are grown by vanilla
/// through `BonemealableBlock.performBonemeal`, and no bone-meal entry point is reachable from mob
/// AI in this codebase yet. `pitcher_crop` is grown by vanilla's goal not at all:
/// `PitcherCropBlock extends DoublePlantBlock` (`PitcherCropBlock.java:33`), so it matches none of
/// `BeeGrowCropGoal.tick`'s four `instanceof`/`is` branches even though it is in the tag. A bee
/// flying over either simply grows nothing, rather than growing something wrong -- bumping only
/// the lower half of a pitcher crop would desynchronise the double block.
const fn bee_growable_max_age(block: &Block) -> Option<u8> {
    match block.id.as_u16() {
        // wheat, pumpkin_stem, melon_stem, carrots, potatoes
        207 | 364 | 365 | 441 | 442 => Some(7),
        // beetroots (`BeetrootBlock.MAX_AGE = 3`), sweet_berry_bush
        // (`SweetBerryBushBlock.MAX_AGE = 3`)
        665 | 861 => Some(3),
        // torchflower_crop (`TorchflowerCropBlock.MAX_AGE = 1`)
        662 => Some(1),
        _ => None,
    }
}

/// `Bee.ValidateHiveGoal` (`Bee.java:1320-1344`): every 20-40 ticks, forget a hive position whose
/// block is no longer a valid beehive.
pub struct BeeValidateHiveGoal {
    bee: Weak<BeeEntity>,
    cooldown: i64,
    last_validate_tick: i64,
}

impl BeeValidateHiveGoal {
    #[must_use]
    pub fn new(bee: Weak<BeeEntity>) -> Self {
        Self {
            bee,
            cooldown: rand::rng().random_range(VALIDATE_COOLDOWN_MIN..=VALIDATE_COOLDOWN_MAX),
            last_validate_tick: -1,
        }
    }
}

impl Goal for BeeValidateHiveGoal {
    fn can_start<'a>(&'a mut self, _mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        Box::pin(async move {
            let Some(bee) = self.bee.upgrade() else {
                return false;
            };
            if bee_is_angry(&bee) {
                return false;
            }
            let world = bee.mob_entity.living_entity.entity.world.load();
            let game_time = world.level_time.lock().await.world_age;
            game_time > self.last_validate_tick + self.cooldown
        })
    }

    fn should_continue<'a>(&'a mut self, _mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        Box::pin(async { false })
    }

    fn start<'a>(&'a mut self, _mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async move {
            let Some(bee) = self.bee.upgrade() else {
                return;
            };
            let world = bee.mob_entity.living_entity.entity.world.load();
            // `ValidateHiveGoal.start`'s `level().isLoaded(hivePos)` guard: an unloaded chunk
            // reads as having no block entity, which would make the bee forget a perfectly good
            // hive it has simply flown out of render distance of.
            if let Some(hive_pos) = bee.hive_pos.load()
                && world.is_loaded(&hive_pos)
                && !bee.is_hive_valid()
            {
                bee.drop_hive();
            }
            self.last_validate_tick = world.level_time.lock().await.world_age;
        })
    }
}

/// `Bee.ValidateFlowerGoal` (`Bee.java:1292-1318`): the same sweep for the saved flower position,
/// dropping it once the block there stops attracting bees (picked, replaced, waterlogged).
pub struct BeeValidateFlowerGoal {
    bee: Weak<BeeEntity>,
    cooldown: i64,
    last_validate_tick: i64,
}

impl BeeValidateFlowerGoal {
    #[must_use]
    pub fn new(bee: Weak<BeeEntity>) -> Self {
        Self {
            bee,
            cooldown: rand::rng().random_range(VALIDATE_COOLDOWN_MIN..=VALIDATE_COOLDOWN_MAX),
            last_validate_tick: -1,
        }
    }
}

impl Goal for BeeValidateFlowerGoal {
    fn can_start<'a>(&'a mut self, _mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        Box::pin(async move {
            let Some(bee) = self.bee.upgrade() else {
                return false;
            };
            if bee_is_angry(&bee) {
                return false;
            }
            let world = bee.mob_entity.living_entity.entity.world.load();
            let game_time = world.level_time.lock().await.world_age;
            game_time > self.last_validate_tick + self.cooldown
        })
    }

    fn should_continue<'a>(&'a mut self, _mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        Box::pin(async { false })
    }

    fn start<'a>(&'a mut self, _mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async move {
            let Some(bee) = self.bee.upgrade() else {
                return;
            };
            let world = bee.mob_entity.living_entity.entity.world.load();
            // `ValidateFlowerGoal.start`'s `level().isLoaded(savedFlowerPos)` guard, for the
            // same reason as the hive validator above.
            if let Some(flower_pos) = bee.flower_pos.load()
                && world.is_loaded(&flower_pos)
            {
                let (block, state) = world.get_block_and_state(&flower_pos);
                if !attracts_bees(block, state) {
                    bee.drop_flower();
                }
            }
            self.last_validate_tick = world.level_time.lock().await.world_age;
        })
    }
}

/// `Bee.BeeGoToKnownFlowerGoal` (`Bee.java:888-938`).
///
/// A homeless bee that has gone 600 ticks without nectar flies back to the flower it remembers,
/// giving up (and forgetting the flower) after 2400 ticks of travel or if the flower turns out to
/// be too far away.
pub struct BeeGoToKnownFlowerGoal {
    bee: Weak<BeeEntity>,
    travelling_ticks: i32,
}

impl BeeGoToKnownFlowerGoal {
    #[must_use]
    pub const fn new(bee: Weak<BeeEntity>) -> Self {
        Self {
            bee,
            travelling_ticks: 0,
        }
    }

    /// `BeeGoToKnownFlowerGoal.canBeeUse`.
    fn can_use(bee: &BeeEntity) -> bool {
        let Some(flower_pos) = bee.flower_pos.load() else {
            return false;
        };
        if bee.has_hive() {
            return false;
        }
        if bee.ticks_without_nectar() <= TICKS_WITHOUT_NECTAR_BEFORE_SEEKING_KNOWN_FLOWER {
            return false;
        }
        !bee.closer_than(flower_pos, 2.0) && !bee_is_angry(bee)
    }
}

impl Goal for BeeGoToKnownFlowerGoal {
    fn can_start<'a>(&'a mut self, _mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        Box::pin(async move {
            let Some(bee) = self.bee.upgrade() else {
                return false;
            };
            Self::can_use(&bee)
        })
    }

    fn should_continue<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        self.can_start(mob)
    }

    fn start<'a>(&'a mut self, _mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async move {
            self.travelling_ticks = 0;
        })
    }

    fn stop<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async move {
            self.travelling_ticks = 0;
            mob.get_mob_entity()
                .navigator
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .stop();
        })
    }

    fn tick<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async move {
            let Some(bee) = self.bee.upgrade() else {
                return;
            };
            let Some(flower_pos) = bee.flower_pos.load() else {
                return;
            };

            self.travelling_ticks += 1;
            if self.travelling_ticks > MAX_FLOWER_TRAVELLING_TICKS {
                bee.drop_flower();
                return;
            }

            let mut navigator = mob
                .get_mob_entity()
                .navigator
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if !navigator.is_idle() {
                return;
            }
            if bee.is_too_far_away(flower_pos) {
                drop(navigator);
                bee.drop_flower();
                return;
            }
            let pos = mob.get_entity().pos.load();
            let target = Vector3::new(
                f64::from(flower_pos.0.x) + 0.5,
                f64::from(flower_pos.0.y) + 0.5,
                f64::from(flower_pos.0.z) + 0.5,
            );
            navigator.set_progress(NavigatorGoal::new(pos, target, 1.0));
        })
    }

    fn should_run_every_tick(&self) -> bool {
        true
    }

    fn controls(&self) -> Controls {
        Controls::MOVE
    }
}

/// `Bee.BeeGrowCropGoal` (`Bee.java:940-1000`).
///
/// A nectar-carrying bee with a valid hive advances the age of up to two
/// `#minecraft:bee_growables` blocks directly beneath it, at most ten per load of nectar.
pub struct BeeGrowCropGoal {
    bee: Weak<BeeEntity>,
}

impl BeeGrowCropGoal {
    #[must_use]
    pub const fn new(bee: Weak<BeeEntity>) -> Self {
        Self { bee }
    }

    /// `BeeGrowCropGoal.canBeeUse`. The 0.3 roll is vanilla's per-check jitter, so the goal only
    /// engages about 70% of the time it otherwise could.
    fn can_use(bee: &BeeEntity) -> bool {
        if bee.crops_grown_since_pollination() >= MAX_CROPS_GROWN_PER_POLLINATION {
            return false;
        }
        if rand::rng().random::<f32>() < 0.3 {
            return false;
        }
        bee.has_nectar() && bee.is_hive_valid() && !bee_is_angry(bee)
    }

    /// The state-mutation half of `BeeGrowCropGoal.tick`, kept generic over the age property via
    /// `Block::properties`/`Block::from_properties` rather than per-block property structs.
    /// Returns the grown state id, or `None` when the block is not growable or is already ripe.
    fn grown_state(block: &Block, state_id: BlockStateId) -> Option<BlockStateId> {
        let max_age = bee_growable_max_age(block)?;
        let props = block.properties(state_id)?.to_props();
        let age: u8 = props
            .iter()
            .find(|(name, _)| *name == "age")?
            .1
            .parse()
            .ok()?;
        if age >= max_age {
            return None;
        }
        let next = (age + 1).to_string();
        let updated: Vec<(&str, &str)> = props
            .iter()
            .map(|(name, value)| {
                if *name == "age" {
                    (*name, next.as_str())
                } else {
                    (*name, *value)
                }
            })
            .collect();
        Some(block.from_properties(&updated).to_state_id(block))
    }
}

impl Goal for BeeGrowCropGoal {
    fn can_start<'a>(&'a mut self, _mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        Box::pin(async move {
            let Some(bee) = self.bee.upgrade() else {
                return false;
            };
            Self::can_use(&bee)
        })
    }

    fn should_continue<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        self.can_start(mob)
    }

    fn tick<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async move {
            let Some(bee) = self.bee.upgrade() else {
                return;
            };
            if rand::rng().random_range(0..self.get_tick_count(GROW_CHANCE)) != 0 {
                return;
            }

            let entity = mob.get_entity();
            let world = entity.world.load_full();
            let origin = entity.block_pos.load();
            for i in 1..=2 {
                let below = origin.down_height(i);
                let (block, state_id) = world.get_block_and_state_id(&below);
                if !block.has_tag(&tag::Block::MINECRAFT_BEE_GROWABLES) {
                    continue;
                }
                let Some(grown) = Self::grown_state(block, state_id) else {
                    continue;
                };
                world
                    .set_block_state(&below, grown, BlockFlags::NOTIFY_ALL)
                    .await;
                bee.increment_crops_grown_since_pollination();
            }
        })
    }

    fn should_run_every_tick(&self) -> bool {
        true
    }

    fn controls(&self) -> Controls {
        Controls::empty()
    }
}

#[cfg(test)]
mod tests {
    use super::{is_tired_of_looking_for_nectar, poison_duration, sting_death_roll_bound};
    use pumpkin_util::Difficulty;

    #[test]
    fn bee_sting_poison_matches_vanilla_difficulty_durations() {
        assert_eq!(poison_duration(Difficulty::Peaceful), None);
        assert_eq!(poison_duration(Difficulty::Easy), None);
        assert_eq!(poison_duration(Difficulty::Normal), Some(200));
        assert_eq!(poison_duration(Difficulty::Hard), Some(360));
    }

    #[test]
    fn bee_sting_death_roll_bound_shrinks_and_clamps() {
        assert_eq!(sting_death_roll_bound(0), 1200);
        assert_eq!(sting_death_roll_bound(600), 600);
        assert_eq!(sting_death_roll_bound(1199), 1);
        assert_eq!(sting_death_roll_bound(1200), 1);
        assert_eq!(sting_death_roll_bound(5000), 1);
    }

    #[test]
    fn bee_attracts_bees_matches_vanilla_predicate() {
        use pumpkin_data::Block;
        use pumpkin_data::block_properties::{
            BlockProperties, DoubleBlockHalf, TallSeagrassLikeProperties,
        };
        use pumpkin_data::block_state::BlockState;

        assert!(super::attracts_bees(
            &Block::DANDELION,
            Block::DANDELION.default_state
        ));
        assert!(!super::attracts_bees(
            &Block::STONE,
            Block::STONE.default_state
        ));

        // `Bee.attractsBees` only accepts the upper half of a sunflower.
        let mut props = TallSeagrassLikeProperties::default(&Block::SUNFLOWER);
        props.half = DoubleBlockHalf::Lower;
        let lower = BlockState::from_id(props.to_state_id(&Block::SUNFLOWER));
        props.half = DoubleBlockHalf::Upper;
        let upper = BlockState::from_id(props.to_state_id(&Block::SUNFLOWER));
        assert!(!super::attracts_bees(&Block::SUNFLOWER, lower));
        assert!(super::attracts_bees(&Block::SUNFLOWER, upper));
    }

    #[test]
    fn bee_is_tired_of_looking_for_nectar_after_3600_ticks() {
        assert!(!is_tired_of_looking_for_nectar(3600));
        assert!(is_tired_of_looking_for_nectar(3601));
    }

    /// `BeeGrowCropGoal.tick` advances the age property by exactly one and stops at the block's
    /// own max age, which differs per crop family.
    #[test]
    fn bee_grows_a_crop_by_one_age_step_and_stops_when_ripe() {
        use pumpkin_data::Block;
        use pumpkin_data::block_properties::{BlockProperties, WheatLikeProperties};

        let mut props = WheatLikeProperties::default(&Block::WHEAT);
        props.age = 3;
        let grown =
            super::BeeGrowCropGoal::grown_state(&Block::WHEAT, props.to_state_id(&Block::WHEAT))
                .expect("age 3 wheat is growable");
        assert_eq!(
            WheatLikeProperties::from_state_id(grown, &Block::WHEAT).age,
            4
        );

        props.age = 7;
        assert_eq!(
            super::BeeGrowCropGoal::grown_state(&Block::WHEAT, props.to_state_id(&Block::WHEAT)),
            None,
            "fully grown wheat must not advance"
        );
    }

    /// The tabulated max ages, checked against each block's own state space: growing from
    /// `max_age - 1` must succeed and from `max_age` must not.
    #[test]
    fn tabulated_max_ages_match_each_blocks_state_space() {
        use pumpkin_data::Block;
        for block in [
            &Block::WHEAT,
            &Block::CARROTS,
            &Block::POTATOES,
            &Block::MELON_STEM,
            &Block::PUMPKIN_STEM,
            &Block::BEETROOTS,
            &Block::SWEET_BERRY_BUSH,
            &Block::TORCHFLOWER_CROP,
        ] {
            let max = super::bee_growable_max_age(block)
                .unwrap_or_else(|| panic!("{} should be tabulated", block.name));
            let ripe = super::state_id_with_age(block, max)
                .unwrap_or_else(|| panic!("{} should have an age {max} state", block.name));
            assert_eq!(
                super::BeeGrowCropGoal::grown_state(block, ripe),
                None,
                "{} at max age must not grow",
                block.name
            );
            let almost = super::state_id_with_age(block, max - 1)
                .unwrap_or_else(|| panic!("{} should have an age {} state", block.name, max - 1));
            assert!(
                super::BeeGrowCropGoal::grown_state(block, almost).is_some(),
                "{} one below max age must grow",
                block.name
            );
        }
    }

    /// The two deliberate exclusions must not be tabulated: cave vines (vanilla grows them
    /// via bone meal, unreachable from mob AI here) and pitcher crop (a `DoublePlantBlock`,
    /// which vanilla's own goal never matches).
    #[test]
    fn cave_vines_are_not_tabulated() {
        use pumpkin_data::Block;
        assert_eq!(super::bee_growable_max_age(&Block::CAVE_VINES), None);
        assert_eq!(super::bee_growable_max_age(&Block::CAVE_VINES_PLANT), None);
        // `PitcherCropBlock extends DoublePlantBlock`, so vanilla's goal never grows it either.
        assert_eq!(super::bee_growable_max_age(&Block::PITCHER_CROP), None);
        assert_eq!(super::bee_growable_max_age(&Block::STONE), None);
    }
}
