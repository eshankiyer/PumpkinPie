//! Port of `LongJumpToRandomPos.java` with `LongJumpMidJump.java` folded into the same goal.
//!
//! Vanilla splits the leap across two brain behaviors that hand off through the
//! `LONG_JUMP_MID_JUMP` / `LONG_JUMP_COOLDOWN_TICKS` memories: `LongJumpToRandomPos` picks a
//! landing spot, crouches for 40 ticks and launches; `LongJumpMidJump` holds the jumping pose
//! until the mob lands, then plays the landing sound and arms the cooldown. Pumpkin has no
//! expiring-memory primitive to route that handoff through, so both halves live in this one
//! `Goal` and the two memories become plain fields. `breeze_jump.rs` merges its own flight phase
//! the same way.
//!
//! Deliberate deviations from vanilla, all of them bounded-cost or missing-API decisions:
//!
//! - `pickCandidate` (`LongJumpToRandomPos.java:150-171`) loops until the candidate list is
//!   empty within a single tick, computing a path per surviving candidate. That is unbounded
//!   pathfinding inside one tick. Here at most `CANDIDATES_PER_TICK` candidates are examined per
//!   tick, still capped overall by `FIND_JUMP_TRIES` ticks.
//! - The reachability filter uses a throwaway `Navigator::default()` rather than the mob's own
//!   navigator, because the mob's navigator is behind a `std::sync::Mutex` that cannot be held
//!   across an await. `villager/mod.rs` does the same for the same reason; the cost is that the
//!   goat's own pathfinding maluses do not apply to the reachability probe.
//! - `setDiscardFriction(true)` has no equivalent in Pumpkin's movement pipeline (the same gap
//!   `breeze_jump.rs` documents), so the arc loses slightly more speed to drag than vanilla's.
//! - `getJumpBoostPower` is not applied to the launch vector: a Jump Boost effect does not
//!   lengthen the leap here.

use std::sync::atomic::Ordering::Relaxed;

use pumpkin_data::entity::EntityPose;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use rand::RngExt;

use super::{Controls, Goal, GoalFuture, breeze_jump::calculate_jump_vector_for_angle};
use crate::entity::EntityBase;
use crate::entity::ai::pathfinder::Navigator;
use crate::entity::mob::Mob;

/// `LongJumpToRandomPos.FIND_JUMP_TRIES` (`LongJumpToRandomPos.java:31`).
const FIND_JUMP_TRIES: i32 = 20;
/// `LongJumpToRandomPos.PREPARE_JUMP_DURATION` (`LongJumpToRandomPos.java:32`).
const PREPARE_JUMP_DURATION: i32 = 40;
/// `LongJumpToRandomPos.MIN_PATHFIND_DISTANCE_TO_VALID_JUMP` (`LongJumpToRandomPos.java:33`).
const MIN_PATHFIND_DISTANCE_TO_VALID_JUMP: f32 = 8.0;
/// `LongJumpMidJump.TIME_OUT_DURATION` (`LongJumpMidJump.java:14`).
const MID_JUMP_TIME_OUT: i32 = 100;
/// `LongJumpToRandomPos.ALLOWED_ANGLES` (`LongJumpToRandomPos.java:34`).
const ALLOWED_ANGLES: [i32; 4] = [65, 70, 75, 80];
/// See the module doc: bounds the per-tick pathfinding that vanilla leaves unbounded.
const CANDIDATES_PER_TICK: usize = 8;
/// A launched goat needs at least this many ticks in the air before `onGround` is believed,
/// since the flag is still set on the tick the launch velocity is applied.
const MIN_AIRBORNE_TICKS: i32 = 2;

/// One entry of `LongJumpToRandomPos.jumpCandidates`: a landing block and its selection weight
/// (`LongJumpToRandomPos.PossibleJump`, `LongJumpToRandomPos.java:196`).
struct PossibleJump {
    target: BlockPos,
    weight: i32,
}

enum Phase {
    /// Searching for a landing spot (`chosenJump == null`).
    Searching,
    /// A jump vector is chosen; crouching for `PREPARE_JUMP_DURATION` ticks.
    Preparing { velocity: Vector3<f64>, ticks: i32 },
    /// Launched; the `LongJumpMidJump` half.
    MidJump { ticks: i32 },
}

/// Vanilla's `LongJumpToRandomPos` + `LongJumpMidJump` pair. Used by the goat
/// (`GoatAi.initLongJumpActivity`, `GoatAi.java:109-122`).
pub struct LongJumpToRandomPosGoal {
    time_between_min: i32,
    time_between_max: i32,
    max_long_jump_height: i32,
    max_long_jump_width: i32,
    max_jump_velocity_multiplier: f64,
    jump_sound: fn(&dyn Mob) -> Sound,
    landing_sound: Sound,

    cooldown: i32,
    phase: Phase,
    candidates: Vec<PossibleJump>,
    total_weight: i32,
    initial_position: Option<Vector3<f64>>,
    find_jump_tries: i32,
}

impl LongJumpToRandomPosGoal {
    /// Mirrors vanilla's constructor argument order: the `UniformInt timeBetweenLongJumps`
    /// bounds, `maxLongJumpHeight`, `maxLongJumpWidth`, `maxJumpVelocityMultiplier` and the
    /// jump-sound selector, plus the landing sound `LongJumpMidJump` is built with.
    #[must_use]
    pub const fn new(
        time_between_min: i32,
        time_between_max: i32,
        max_long_jump_height: i32,
        max_long_jump_width: i32,
        max_jump_velocity_multiplier: f64,
        jump_sound: fn(&dyn Mob) -> Sound,
        landing_sound: Sound,
    ) -> Self {
        Self {
            time_between_min,
            time_between_max,
            max_long_jump_height,
            max_long_jump_width,
            max_jump_velocity_multiplier,
            jump_sound,
            landing_sound,
            cooldown: 0,
            phase: Phase::Searching,
            candidates: Vec::new(),
            total_weight: 0,
            initial_position: None,
            find_jump_tries: 0,
        }
    }

    fn sample_time_between(&self, mob: &dyn Mob) -> i32 {
        mob.get_random()
            .random_range(self.time_between_min..=self.time_between_max)
    }

    /// LongJumpToRandomPos.defaultAcceptableLandingSpot (LongJumpToRandomPos.java:60-64)
    /// plus the same-column rejection from isAcceptableLandingPosition
    /// (LongJumpToRandomPos.java:186-190).
    fn is_acceptable_landing_position(mob: &dyn Mob, target: &BlockPos) -> bool {
        let entity = mob.get_entity();
        let mob_pos = entity.block_pos.load();
        if mob_pos.0.x == target.0.x && mob_pos.0.z == target.0.z {
            return false;
        }

        let world = entity.world.load();
        world.get_block_state(&target.down()).is_solid_render()
            && !mob
                .get_mob_entity()
                .navigator
                .lock()
                .unwrap()
                .has_pathfinding_malus(&world, target)
    }

    /// `LongJumpToRandomPos.getJumpCandidate` (`LongJumpToRandomPos.java:173-179`): a
    /// weight-proportional draw that also removes the drawn entry.
    fn take_jump_candidate(&mut self, mob: &dyn Mob) -> Option<PossibleJump> {
        if self.candidates.is_empty() || self.total_weight <= 0 {
            self.candidates.clear();
            self.total_weight = 0;
            return None;
        }

        let mut roll = mob.get_random().random_range(0..self.total_weight);
        let mut index = self.candidates.len() - 1;
        for (i, candidate) in self.candidates.iter().enumerate() {
            roll -= candidate.weight;
            if roll < 0 {
                index = i;
                break;
            }
        }

        let candidate = self.candidates.swap_remove(index);
        self.total_weight -= candidate.weight;
        Some(candidate)
    }

    /// `LongJumpToRandomPos.calculateOptimalJumpVector` (`LongJumpToRandomPos.java:189-202`):
    /// tries each allowed launch angle in a random order until one has a ballistic solution.
    fn calculate_optimal_jump_vector(
        &self,
        mob: &dyn Mob,
        target_pos: Vector3<f64>,
    ) -> Option<Vector3<f64>> {
        let living = &mob.get_mob_entity().living_entity;
        let gravity = EntityBase::get_gravity(mob);
        let max_jump_velocity = living
            .get_attribute_value(&pumpkin_data::attributes::Attributes::JUMP_STRENGTH)
            * self.max_jump_velocity_multiplier;

        let mut angles = ALLOWED_ANGLES;
        // `Collections.shuffle`, Fisher-Yates. Same shape as `breeze_jump.rs`.
        for i in (1..angles.len()).rev() {
            let j = mob.get_random().random_range(0..=i);
            angles.swap(i, j);
        }

        let mob_pos = mob.get_entity().pos.load();
        for angle in angles {
            if let Some(velocity) = calculate_jump_vector_for_angle(
                mob_pos,
                target_pos,
                gravity,
                max_jump_velocity,
                angle,
            ) {
                return Some(velocity);
            }
        }
        None
    }

    /// `LongJumpToRandomPos.pickCandidate` (`LongJumpToRandomPos.java:150-171`), bounded to
    /// `CANDIDATES_PER_TICK` candidates per tick (see the module doc).
    async fn pick_candidate(&mut self, mob: &dyn Mob) {
        for _ in 0..CANDIDATES_PER_TICK {
            let Some(candidate) = self.take_jump_candidate(mob) else {
                return;
            };
            if !Self::is_acceptable_landing_position(mob, &candidate.target) {
                continue;
            }

            let target_pos = candidate.target.to_centered_f64();
            let Some(velocity) = self.calculate_optimal_jump_vector(mob, target_pos) else {
                continue;
            };

            // `navigation.createPath(targetPos, 0, 8)`: only leap where walking will not do.
            let mut navigator = Navigator::default();
            // `Mob.onPathfindingStart/Done` wrap evaluator preparation and cleanup
            // (`Mob.java:194-198`, `WalkNodeEvaluator.java:39-49`).
            let walkable = navigator
                .can_reach_within_for_mob(mob, target_pos, MIN_PATHFIND_DISTANCE_TO_VALID_JUMP)
                .await;
            if walkable {
                continue;
            }

            mob.get_mob_entity()
                .look_control
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .look_at_position(mob, target_pos);
            self.phase = Phase::Preparing { velocity, ticks: 0 };
            return;
        }
    }
}

impl Goal for LongJumpToRandomPosGoal {
    /// `LongJumpToRandomPos.checkExtraStartConditions` (`LongJumpToRandomPos.java:92-99`), with
    /// `LONG_JUMP_COOLDOWN_TICKS` as a plain counter.
    fn can_start<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        Box::pin(async move {
            if self.cooldown > 0 {
                self.cooldown -= 1;
                return false;
            }

            let entity = mob.get_entity();
            let on_honey = {
                let world = entity.world.load();
                let (block, _) = world.get_block_and_state(&entity.block_pos.load());
                block.id == pumpkin_data::Block::HONEY_BLOCK.id
            };
            let can_start = entity.on_ground.load(Relaxed)
                && !entity.touching_water.load(Relaxed)
                && !entity.touching_lava.load(Relaxed)
                && !on_honey;

            if !can_start {
                self.cooldown = self.sample_time_between(mob) / 2;
            }
            can_start
        })
    }

    /// `LongJumpToRandomPos.start` (`LongJumpToRandomPos.java:112-133`): enumerate every block
    /// in the search box, weighted by squared distance.
    fn start<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async move {
            let entity = mob.get_entity();
            self.phase = Phase::Searching;
            self.find_jump_tries = FIND_JUMP_TRIES;
            self.initial_position = Some(entity.pos.load());

            let mob_pos = entity.block_pos.load();
            let width = self.max_long_jump_width;
            let height = self.max_long_jump_height;
            self.candidates.clear();
            self.total_weight = 0;
            for dx in -width..=width {
                for dy in -height..=height {
                    for dz in -width..=width {
                        if dx == 0 && dy == 0 && dz == 0 {
                            continue;
                        }
                        // `Mth.ceil(mobPos.distSqr(pos))` over integer offsets is exact.
                        let weight = dx * dx + dy * dy + dz * dz;
                        self.candidates.push(PossibleJump {
                            target: BlockPos::new(
                                mob_pos.0.x + dx,
                                mob_pos.0.y + dy,
                                mob_pos.0.z + dz,
                            ),
                            weight,
                        });
                        self.total_weight += weight;
                    }
                }
            }
        })
    }

    /// `LongJumpToRandomPos.canStillUse` (`LongJumpToRandomPos.java:101-110`) while searching or
    /// preparing; `LongJumpMidJump.canStillUse` (`LongJumpMidJump.java:24-26`) once launched.
    fn should_continue<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        Box::pin(async move {
            let entity = mob.get_entity();
            if let Phase::MidJump { ticks } = self.phase {
                return ticks < MID_JUMP_TIME_OUT
                    && (ticks < MIN_AIRBORNE_TICKS || !entity.on_ground.load(Relaxed));
            }

            let still_put = self.initial_position == Some(entity.pos.load());
            let has_work =
                matches!(self.phase, Phase::Preparing { .. }) || !self.candidates.is_empty();
            still_put
                && self.find_jump_tries > 0
                && !entity.touching_water.load(Relaxed)
                && has_work
        })
    }

    fn tick<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async move {
            let entity = mob.get_entity();
            match self.phase {
                Phase::Searching => {
                    self.find_jump_tries -= 1;
                    self.pick_candidate(mob).await;
                }
                Phase::Preparing { velocity, ticks } => {
                    if ticks < PREPARE_JUMP_DURATION {
                        self.phase = Phase::Preparing {
                            velocity,
                            ticks: ticks + 1,
                        };
                        return;
                    }

                    // `body.setYRot(body.yBodyRot)` then the launch itself.
                    entity.yaw.store(entity.head_yaw.load());
                    entity.velocity.store(velocity);
                    // `LongJumpToRandomPos` enables friction discard for the launched arc
                    // (`LongJumpToRandomPos.java:137-146`).
                    mob.get_mob_entity()
                        .living_entity
                        .set_discard_friction(true);
                    entity.set_pose(EntityPose::LongJumping);
                    mob.get_mob_entity()
                        .living_entity
                        .set_discard_friction(true);
                    entity.world.load().play_sound(
                        (self.jump_sound)(mob),
                        SoundCategory::Neutral,
                        &entity.pos.load(),
                    );
                    self.phase = Phase::MidJump { ticks: 0 };
                }
                Phase::MidJump { ticks } => {
                    self.phase = Phase::MidJump { ticks: ticks + 1 };
                }
            }
        })
    }

    /// `LongJumpMidJump.stop` (`LongJumpMidJump.java:33-43`) for a completed leap, and
    /// `LongJumpToRandomPos.canStillUse`'s half-cooldown for an abandoned search
    /// (`LongJumpToRandomPos.java:106`).
    fn stop<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async move {
            let entity = mob.get_entity();
            let launched = matches!(self.phase, Phase::MidJump { .. });

            if launched {
                if entity.on_ground.load(Relaxed) {
                    let velocity = entity.velocity.load();
                    entity.velocity.store(velocity.multiply(0.1, 1.0, 0.1));
                    entity.world.load().play_sound(
                        self.landing_sound,
                        SoundCategory::Neutral,
                        &entity.pos.load(),
                    );
                }
                mob.get_mob_entity()
                    .living_entity
                    .set_discard_friction(false);
                self.cooldown = self.sample_time_between(mob);
                // `LongJumpMidJump.stop` clears friction discard after landing
                // (`LongJumpMidJump.java:33-40`).
                mob.get_mob_entity()
                    .living_entity
                    .set_discard_friction(false);
            } else {
                self.cooldown = self.sample_time_between(mob) / 2;
            }

            if entity.pose.load() == EntityPose::LongJumping {
                entity.set_pose(EntityPose::Standing);
            }
            mob.get_mob_entity()
                .living_entity
                .set_discard_friction(false);

            self.phase = Phase::Searching;
            self.candidates.clear();
            self.total_weight = 0;
            self.initial_position = None;
            self.find_jump_tries = 0;
        })
    }

    fn should_run_every_tick(&self) -> bool {
        true
    }

    fn controls(&self) -> Controls {
        Controls::MOVE | Controls::LOOK | Controls::JUMP
    }
}
