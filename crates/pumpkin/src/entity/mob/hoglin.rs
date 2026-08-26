use std::sync::{
    Arc, Weak,
    atomic::{AtomicBool, AtomicI32, Ordering::Relaxed},
};

use pumpkin_data::damage::DamageType;
use pumpkin_data::entity::EntityType;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_data::tag::{self, Taggable};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_protocol::java::client::play::Metadata;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;

use crate::entity::{
    Entity, EntityBase, EntityBaseFuture, NBTStorage, NbtFuture,
    ai::goal::{
        active_target::ActiveTargetGoal, avoid_entity::AvoidEntityGoal,
        look_around::RandomLookAroundGoal, look_at_entity::LookAtEntityGoal,
        melee_attack::MeleeAttackGoal, swim::SwimGoal, wander_around::WanderAroundGoal,
    },
    mob::{
        Mob, MobEntity, hoglin_gore,
        zoglin::ZoglinEntity,
        zombification::{self, ZombificationTimer},
    },
};
use crate::world::World;

/// `HoglinAi.REPELLENT_DETECTION_RANGE_HORIZONTAL/VERTICAL`.
const REPELLENT_RANGE_HORIZONTAL: i32 = 8;
const REPELLENT_RANGE_VERTICAL: i32 = 4;
/// `HoglinAi.REPELLENT_PACIFY_TIME`.
const REPELLENT_PACIFY_TICKS: i32 = 200;
/// Matches vanilla's `Sensor` default scan rate (`HoglinSpecificSensor` inherits it) --
/// re-scanning every tick would be needlessly expensive for an 8x4x8-block box check.
const REPELLENT_SCAN_INTERVAL_TICKS: i32 = 20;
/// Radius of the direct world query standing in for vanilla's pack-coordination memories
/// (`NEAREST_VISIBLE_ADULT_HOGLINS` / the visible-count sensors). Pumpkin has no
/// sensor-memory equivalent, so this follows the same 16-block direct-query pattern
/// `piglin_shared::retaliate_and_alert_piglins` established for `broadcastAngerTarget`.
const PACK_ALERT_RADIUS: f64 = 16.0;
/// `BehaviorUtils.isOtherTargetMuchFurtherAwayThanCurrentAttackTarget`'s margin
/// (`HoglinAi.java:198` passes `4.0`): a retaliation candidate more than four blocks past
/// the current target's distance is ignored.
const RETALIATE_DISTANCE_MARGIN: f64 = 4.0;

/// `HoglinSpecificSensor.findNearestRepellent`/`BlockPos.findClosestMatch(pos, 8, 4, ...)`:
/// true if any block within an 8-horizontal/4-vertical box of `center` carries the
/// `hoglin_repellents` tag.
fn repellent_nearby(world: &World, center: BlockPos) -> bool {
    for dy in -REPELLENT_RANGE_VERTICAL..=REPELLENT_RANGE_VERTICAL {
        for dx in -REPELLENT_RANGE_HORIZONTAL..=REPELLENT_RANGE_HORIZONTAL {
            for dz in -REPELLENT_RANGE_HORIZONTAL..=REPELLENT_RANGE_HORIZONTAL {
                let pos = BlockPos(center.0 + Vector3::new(dx, dy, dz));
                if world
                    .get_block(&pos)
                    .has_tag(&tag::Block::MINECRAFT_HOGLIN_REPELLENTS)
                {
                    return true;
                }
            }
        }
    }
    false
}

pub struct HoglinEntity {
    pub mob_entity: MobEntity,
    /// Ticks left pacified by a nearby repellent block (`HoglinAi.isPacified`).
    pacify_ticks: Arc<AtomicI32>,
    repellent_scan_countdown: AtomicI32,
    /// Whether the hoglin currently has an attack target, sampled once per `mob_tick`.
    /// `get_ambient_sound` is synchronous while the target lives behind a mutex, so the
    /// sample runs at most one tick stale -- the same shape `PiglinEntity` uses for its
    /// `wants_to_pick_up_item` check.
    has_attack_target: AtomicBool,
    /// `Hoglin.timeInOverworld`/`IsImmuneToZombification` (`Hoglin.java:69-71`).
    zombification: ZombificationTimer,
}

impl HoglinEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let mob_entity = MobEntity::new(entity);
        let pacify_ticks = Arc::new(AtomicI32::new(0));
        let hoglin = Self {
            mob_entity,
            pacify_ticks: pacify_ticks.clone(),
            repellent_scan_countdown: AtomicI32::new(0),
            has_attack_target: AtomicBool::new(false),
            zombification: ZombificationTimer::new(),
        };
        let mob_arc = Arc::new(hoglin);
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

            goal_selector.add_goal(0, Box::new(SwimGoal::default()));
            goal_selector.add_goal(4, Box::new(MeleeAttackGoal::new(1.0, true)));
            // Vanilla: adult hoglins flee visible piglins within 8 blocks while idle
            // (HoglinAi.java:74, DESIRED_DISTANCE_FROM_PIGLIN_WHEN_IDLING=8, speed
            // 0.4F) and flee harder once actually hit (initRetreatActivity, distance
            // 15, speed SPEED_MULTIPLIER_WHEN_RETREATING=1.3F). Pumpkin's
            // `AvoidEntityGoal` has a single close/far speed model rather than two
            // separate Brain activities, so this merges both into one goal; the
            // adult/baby and `isPacified`/repellent gating from vanilla are also not
            // reproduced here.
            goal_selector.add_goal(
                4,
                Box::new(AvoidEntityGoal::new(&EntityType::PIGLIN, 8.0, 0.4, 1.3)),
            );
            goal_selector.add_goal(5, Box::new(WanderAroundGoal::new(1.0)));
            goal_selector.add_goal(
                6,
                LookAtEntityGoal::with_default(mob_weak.clone(), &EntityType::PLAYER, 8.0),
            );
            goal_selector.add_goal(7, Box::new(RandomLookAroundGoal::default()));

            let mut target_selector = mob_arc
                .mob_entity
                .target_selector
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let pacify_check = pacify_ticks;
            target_selector.add_goal(
                1,
                Box::new(ActiveTargetGoal::new(
                    &mob_arc.mob_entity,
                    &EntityType::PLAYER,
                    10,
                    true,
                    false,
                    Some(
                        move |_target: crate::entity::ai::target_predicate::TargetData,
                              _world: Arc<World>| {
                            let pacify_check = pacify_check.clone();
                            async move { pacify_check.load(Relaxed) <= 0 }
                        },
                    ),
                )),
            );
        };

        mob_arc
    }

    #[must_use]
    pub fn is_adult(&self) -> bool {
        self.mob_entity.living_entity.entity.age.load(Relaxed) >= 0
    }

    /// `HoglinAi.isPacified` (200 ticks after a nearby repellent was last seen).
    #[must_use]
    pub fn is_pacified(&self) -> bool {
        self.pacify_ticks.load(Relaxed) > 0
    }

    /// All other adult hoglins within `PACK_ALERT_RADIUS`, standing in for vanilla's
    /// `NEAREST_VISIBLE_ADULT_HOGLINS` memory read by `getVisibleAdultHoglins`
    /// (`HoglinAi.java:241-243`).
    fn nearby_adult_hoglins(&self) -> Vec<Arc<dyn EntityBase>> {
        let entity = &self.mob_entity.living_entity.entity;
        let world = entity.world.load();
        world
            .get_nearby_entities(entity.pos.load(), PACK_ALERT_RADIUS)
            .into_values()
            .filter(|nearby| {
                let nearby_entity = nearby.get_entity();
                nearby_entity.entity_type.id == EntityType::HOGLIN.id
                    && nearby_entity.entity_id != entity.entity_id
                    && nearby_entity.age.load(Relaxed) >= 0
            })
            .collect()
    }

    /// `HoglinAi.piglinsOutnumberHoglins` (`HoglinAi.java:174-182`): true when visible
    /// adult piglins outnumber the visible adult hoglins plus this one. Vanilla reads the
    /// counts from sensor memories; this scans the same radius directly. Babies always
    /// report false (L175-177).
    fn piglins_outnumber_hoglins(&self) -> bool {
        if !self.is_adult() {
            return false;
        }

        let entity = &self.mob_entity.living_entity.entity;
        let world = entity.world.load();

        let mut piglins = 0;
        // L180: vanilla adds one for the asking hoglin itself.
        let mut hoglins = 1;
        for nearby in world
            .get_nearby_entities(entity.pos.load(), PACK_ALERT_RADIUS)
            .into_values()
        {
            let type_id = nearby.get_entity().entity_type.id;
            if type_id != EntityType::PIGLIN.id && type_id != EntityType::HOGLIN.id {
                continue;
            }
            let Some(mob) = nearby.get_mob() else {
                continue;
            };
            if mob.get_mob_entity().living_entity.entity.age.load(Relaxed) < 0 {
                continue;
            }
            if type_id == EntityType::PIGLIN.id {
                piglins += 1;
            } else if nearby.get_entity().entity_id != entity.entity_id {
                hoglins += 1;
            }
        }

        piglins > hoglins
    }

    /// `HoglinAi.broadcastAttackTarget` -> `setAttackTargetIfCloserThanCurrent`
    /// (`HoglinAi.java:215-225`): every non-pacified nearby adult hoglin joins in on
    /// `target`, keeping whichever of its current/new target is closer, as
    /// `BehaviorUtils.getNearestTarget` decides. The 200-tick expiry vanilla puts on the
    /// memory (`setAttackTarget`, L208-213) has no equivalent here; the target goals
    /// re-validate their target every tick, which bounds stale targets instead.
    async fn broadcast_attack_target(&self, target: &Arc<dyn EntityBase>) {
        for hoglin in self.nearby_adult_hoglins() {
            let Some(hoglin_mob) = hoglin.get_mob() else {
                continue;
            };
            // L220: pacified hoglins never join a fight.
            if hoglin_mob
                .cast_any()
                .downcast_ref::<Self>()
                .is_some_and(Self::is_pacified)
            {
                continue;
            }

            let pos = hoglin_mob.get_mob_entity().living_entity.entity.pos.load();
            let new_dist_sq = pos.squared_distance_to_vec(&target.get_entity().pos.load());
            let current = hoglin_mob.get_mob_entity().target.lock().await.clone();
            if let Some(current) = current.as_ref() {
                let current_dist_sq = pos.squared_distance_to_vec(&current.get_entity().pos.load());
                if current_dist_sq <= new_dist_sq {
                    continue;
                }
            }

            hoglin_mob.set_mob_target(Some(target.clone())).await;
        }
    }

    /// `HoglinAi.onHitTarget` (`HoglinAi.java:132-141`), called from the attack path the
    /// same way vanilla's `Hoglin.doHurtTarget` calls it (`Hoglin.java:110`). Adults only.
    ///
    /// Against an outnumbering piglin pack, vanilla sets an expiring `AVOID_TARGET` on
    /// this hoglin and broadcasts a retreat. There is no mechanism here to hand
    /// `AvoidEntityGoal` an explicit flee target -- the exact limitation that kept the
    /// piglin's own outnumbered-retreat branch unimplemented -- so the disengagement half
    /// is ported instead: this hoglin drops its target and so does every nearby adult
    /// hoglin currently attacking that same piglin, which hands the fight back to the
    /// existing flee-from-piglin goal. Any other gore victim is broadcast as an attack
    /// target for the pack to pile onto.
    async fn on_hit_target(&self, target: &dyn EntityBase) {
        if !self.is_adult() {
            return;
        }

        if target.get_entity().entity_type.id == EntityType::PIGLIN.id
            && self.piglins_outnumber_hoglins()
        {
            self.set_mob_target(None).await;
            let target_id = target.get_entity().entity_id;
            for hoglin in self.nearby_adult_hoglins() {
                let Some(hoglin_mob) = hoglin.get_mob() else {
                    continue;
                };
                let same_target = hoglin_mob
                    .get_mob_entity()
                    .target
                    .lock()
                    .await
                    .as_ref()
                    .is_some_and(|t| t.get_entity().entity_id == target_id);
                if same_target {
                    hoglin_mob.set_mob_target(None).await;
                }
            }
        } else {
            let world = self.mob_entity.living_entity.entity.world.load();
            let Some(target_arc) = world.get_entity_by_id(target.get_entity().entity_id) else {
                return;
            };
            self.broadcast_attack_target(&target_arc).await;
        }
    }
}

impl NBTStorage for HoglinEntity {
    /// `Hoglin.addAdditionalSaveData` (`Hoglin.java:273-277`). `CannotBeHunted`
    /// (`Hoglin.java:275`) is not persisted: nothing in this codebase sets it, because the
    /// piglin hunting behaviour it gates is not ported (see `PiglinAi.StartHuntingHoglin`).
    fn write_nbt<'a>(&'a self, nbt: &'a mut NbtCompound) -> NbtFuture<'a, ()> {
        Box::pin(async {
            self.mob_entity.living_entity.write_nbt(nbt).await;
            self.zombification.write_nbt(nbt);
        })
    }

    /// `Hoglin.readAdditionalSaveData` (`Hoglin.java:280-285`).
    fn read_nbt_non_mut<'a>(&'a self, nbt: &'a NbtCompound) -> NbtFuture<'a, ()> {
        Box::pin(async {
            self.mob_entity.living_entity.read_nbt_non_mut(nbt).await;
            self.zombification.read_nbt(nbt);
            self.mob_entity.living_entity.entity.send_meta_data(
                &[Metadata::new(
                    pumpkin_data::tracked_data::hoglin::DATA_IMMUNE_TO_ZOMBIFICATION,
                    self.zombification.is_immune(),
                )],
                None,
            );
        })
    }
}

impl Mob for HoglinEntity {
    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    /// `HoglinAi.wasHurtBy` (`HoglinAi.java:184-193`), reached from vanilla
    /// `Hoglin.hurtServer` (`Hoglin.java:120-128`) after a successful hurt carrying a
    /// living attacker.
    ///
    /// Being hit clears repellent pacification (`eraseMemory(PACIFIED)`, L186); the
    /// `BREED_TARGET` erasure at L187 needs no counterpart because breeding here is
    /// goal-driven rather than memory-driven. Babies retreat from the attacker in vanilla
    /// (`retreatFromNearestTarget`, L189); that is not portable for the same reason the
    /// piglin's baby-flee branch was skipped (`PiglinEntity::on_damage`) -- no way to hand
    /// `AvoidEntityGoal` an explicit target -- so they keep the pacification reset but do
    /// not switch behaviour. Adults run `maybeRetaliate` (L195-206).
    fn on_damage<'a>(
        &'a self,
        _damage_type: DamageType,
        source: Option<&'a dyn EntityBase>,
    ) -> EntityBaseFuture<'a, ()> {
        Box::pin(async move {
            let Some(source) = source else {
                return;
            };

            self.pacify_ticks.store(0, Relaxed);

            if !self.is_adult() {
                return;
            }

            // maybeRetaliate (L195-206). L196's guard (`!isActive(AVOID)` or the attacker
            // is not a piglin) has no observable activity to test in the flattened model,
            // so retaliation can preempt a flee; the remaining guards still apply.
            if source.get_entity().entity_type.id == EntityType::HOGLIN.id {
                return; // L197
            }

            let entity = &self.mob_entity.living_entity.entity;
            let pos = entity.pos.load();
            let source_pos = source.get_entity().pos.load();

            // L198: don't switch away from a current target already meaningfully closer
            // than the attacker.
            let source_dist_sq = pos.squared_distance_to_vec(&source_pos);
            let current = self.mob_entity.target.lock().await.clone();
            if let Some(current) = current.as_ref() {
                let current_dist = pos
                    .squared_distance_to_vec(&current.get_entity().pos.load())
                    .sqrt();
                if source_dist_sq.sqrt() > current_dist + RETALIATE_DISTANCE_MARGIN {
                    return;
                }
            }

            // L199: `Sensor.isEntityAttackable` ≈ `Mob.can_attack`.
            if !self.can_attack(source.get_entity()) {
                return;
            }

            let world = entity.world.load();
            let Some(source_arc) = world.get_entity_by_id(source.get_entity().entity_id) else {
                return;
            };
            // L200: `setAttackTarget` stores with a 200-tick expiry; Pumpkin targets do
            // not expire, and the target goals re-validate every tick instead.
            self.set_mob_target(Some(source_arc.clone())).await;
            // L201: `broadcastAttackTarget`.
            self.broadcast_attack_target(&source_arc).await;
        })
    }

    /// `Hoglin.doHurtTarget` (`Hoglin.java:105-115`): the swing-animation event and
    /// `attackAnimationRemainingTicks` (L106-107) are client-side only; the server effects
    /// are the `HOGLIN_ATTACK` sound (L108), the `HoglinAi.onHitTarget` coordination hook
    /// (L110), and then the `HoglinBase.hurtAndThrowTarget` damage roll/knockback, which
    /// replaces the generic flat-damage melee path.
    fn try_attack<'a>(&'a self, target: &'a dyn EntityBase) -> EntityBaseFuture<'a, bool> {
        Box::pin(async move {
            let entity = &self.mob_entity.living_entity.entity;
            entity.world.load().play_sound(
                Sound::EntityHoglinAttack,
                SoundCategory::Hostile,
                &entity.pos.load(),
            );
            self.on_hit_target(target).await;
            hoglin_gore::try_gore_attack(self, target).await
        })
    }

    /// `Hoglin.getAmbientSound` (`Hoglin.java:331-333`) delegates straight to
    /// `HoglinAi.getSoundForCurrentActivity`; its per-activity mapping
    /// (`getSoundForActivity`, `HoglinAi.java:231-239`) is reproduced here against the
    /// flattened activity model: FIGHT ≈ this hoglin currently holds an attack target
    /// (`has_attack_target`), AVOID/converting map to their observable states, and "near
    /// repellent" ≈ `is_pacified` (the pacify timer is exactly the lifetime of vanilla's
    /// repellent-driven passivity).
    fn get_ambient_sound(&self) -> Option<Sound> {
        if self.zombification.is_converting(&self.mob_entity) {
            return Some(Sound::EntityHoglinRetreat);
        }
        if self.has_attack_target.load(Relaxed) {
            return Some(Sound::EntityHoglinAngry);
        }
        if self.is_pacified() {
            return Some(Sound::EntityHoglinRetreat);
        }
        Some(Sound::EntityHoglinAmbient)
    }

    /// `HoglinSpecificSensor.findNearestRepellent` + `BecomePassiveIfMemoryPresent`:
    /// re-scans for a nearby repellent block every 20 ticks, refreshing the pacify
    /// timer and clearing the current attack target when one is found.
    fn mob_tick<'a>(&'a self, _caller: &'a Arc<dyn EntityBase>) -> EntityBaseFuture<'a, ()> {
        Box::pin(async move {
            // `Hoglin.customServerAiStep` (`Hoglin.java:149-157`). Unlike `AbstractPiglin`,
            // the hoglin plays its converted sound unconditionally -- there is no
            // peaceful-difficulty guard on this branch.
            if self.zombification.tick(&self.mob_entity) {
                zombification::play_converted_sound(
                    &self.mob_entity,
                    Sound::EntityHoglinConvertedToZombified,
                );
                zombification::convert_to(
                    &self.mob_entity,
                    &EntityType::ZOGLIN,
                    true,
                    ZoglinEntity::new,
                )
                .await;
                return;
            }

            // Sampled once per tick for `get_ambient_sound`'s FIGHT branch; see the
            // field doc for why the sample is not read inline.
            self.has_attack_target
                .store(self.mob_entity.target.lock().await.is_some(), Relaxed);

            let countdown = self.repellent_scan_countdown.fetch_sub(1, Relaxed);
            if countdown > 0 {
                if self.pacify_ticks.load(Relaxed) > 0 {
                    self.pacify_ticks.fetch_sub(1, Relaxed);
                }
                return;
            }
            self.repellent_scan_countdown
                .store(REPELLENT_SCAN_INTERVAL_TICKS, Relaxed);

            let pos = self.mob_entity.living_entity.entity.block_pos.load();
            let world = self.mob_entity.living_entity.entity.world.load();
            if repellent_nearby(&world, pos) {
                self.pacify_ticks.store(REPELLENT_PACIFY_TICKS, Relaxed);
                self.set_mob_target(None).await;
            } else if self.pacify_ticks.load(Relaxed) > 0 {
                self.pacify_ticks.fetch_sub(1, Relaxed);
            }
        })
    }
}
