//! Port of `Cat.CatRelaxOnOwnerGoal` (`Cat.java:519-645`).

use std::sync::{Arc, Weak};

use pumpkin_data::block_properties::{BlockProperties, WhiteBedLikeProperties};
use pumpkin_data::tag::{self, Taggable};
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;

use super::{Controls, Goal, GoalFuture, to_goal_ticks};
use crate::entity::EntityBase;
use crate::entity::ai::pathfinder::NavigatorGoal;
use crate::entity::mob::Mob;
use crate::entity::passive::cat::CatEntity;
use crate::entity::passive::tamable::TamableAnimal;
use crate::entity::player::Player;

/// `Cat.java:546`: `this.cat.distanceToSqr(this.ownerPlayer) > 100.0`.
const MAX_OWNER_DISTANCE_SQ: f64 = 100.0;
/// `Cat.java:632`: `this.cat.distanceToSqr(this.ownerPlayer) < 2.5`.
const ON_BED_DISTANCE_SQ: f64 = 2.5;
/// `Cat.java:634`: `this.onBedTicks > this.adjustedTickDelay(16)`.
const ON_BED_TICKS_BEFORE_LYING: i32 = 16;
/// `Cat.java:587`/`Cat.java:631`: the navigation speed used to reach the bedside.
const APPROACH_SPEED: f64 = 1.1;
/// `Cat.java:564`: `new AABB(this.goalPos).inflate(2.0)`.
const OCCUPANCY_RADIUS: f64 = 2.0;

/// A tamed cat walks to the bedside of its sleeping owner and curls up there.
///
/// Divergences from `Cat.java:519-645`, both in `stop`:
///
/// * The morning gift (`giveMorningGift`, lines 605-625) is not implemented. It needs
///   `randomTeleport` plus a `minecraft:gameplay/cat_morning_gift` loot-table roll, neither of
///   which exists here; the cat simply gets up. Everything the goal is normally observed for --
///   walking to the bed, curling up, blocking the bed slot -- still happens.
/// * `EnvironmentAttributes.CAT_WAKING_UP_GIFT_CHANCE` is only read by the gift branch, so it is
///   not consulted either.
///
/// `spaceIsOccupied` (lines 563-571) IS ported: vanilla's box query is approximated with a
/// radius-2 sphere around the goal position, so a corner case at the box's diagonal reads as
/// unoccupied where vanilla reads occupied.
pub struct CatRelaxOnOwnerGoal {
    cat: Weak<CatEntity>,
    owner: Option<Arc<Player>>,
    goal_pos: Option<BlockPos>,
    on_bed_ticks: i32,
}

impl CatRelaxOnOwnerGoal {
    #[must_use]
    pub fn new(cat: Weak<CatEntity>) -> Box<Self> {
        Box::new(Self {
            cat,
            owner: None,
            goal_pos: None,
            on_bed_ticks: 0,
        })
    }

    fn cat_is_relaxable(cat: &CatEntity) -> bool {
        // `canUse` lines 530-537.
        cat.is_tame() && !cat.mob_entity.is_ordered_to_sit()
    }

    /// `Cat.java:542`: `owner.isSleeping()`. `Player::is_sleeping` is private to `entity::player`,
    /// so the field it reads is used directly -- it is set on bed entry and cleared on wake
    /// (`player.rs:2442`/`player.rs:2585`).
    fn owner_is_sleeping(owner: &Player) -> bool {
        owner.sleeping_since.load().is_some()
    }

    /// `Cat.CatRelaxOnOwnerGoal.spaceIsOccupied` (lines 563-571).
    fn space_is_occupied(cat: &CatEntity, goal_pos: BlockPos) -> bool {
        let world = cat.get_entity().world.load_full();
        let center = Vector3::new(
            f64::from(goal_pos.0.x) + 0.5,
            f64::from(goal_pos.0.y) + 0.5,
            f64::from(goal_pos.0.z) + 0.5,
        );
        world
            .get_nearby_entities(center, OCCUPANCY_RADIUS)
            .values()
            .any(|entity| {
                entity
                    .cast_any()
                    .downcast_ref::<CatEntity>()
                    .is_some_and(|other| {
                        !std::ptr::eq(std::ptr::from_ref(other), std::ptr::from_ref(cat))
                            && (other.is_lying() || other.is_relax_state_one())
                    })
            })
    }

    /// `canUse` lines 550-557: the block the owner is standing/lying in must be a bed; the goal
    /// position is the cell behind the bed's facing direction.
    fn bedside_of(cat: &CatEntity, owner: &Player) -> Option<BlockPos> {
        let world = cat.get_entity().world.load_full();
        let owner_pos = owner.get_entity().block_pos.load();
        let (block, state_id) = world.get_block_and_state_id(&owner_pos);
        if !block.has_tag(&tag::Block::MINECRAFT_BEDS) {
            return None;
        }
        let facing = WhiteBedLikeProperties::from_state_id(state_id, block).facing;
        Some(owner_pos.offset(facing.opposite().to_offset()))
    }
}

impl Goal for CatRelaxOnOwnerGoal {
    fn can_start<'a>(&'a mut self, _mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        Box::pin(async {
            let Some(cat) = self.cat.upgrade() else {
                return false;
            };
            if !Self::cat_is_relaxable(&cat) {
                return false;
            }
            let Some(owner_uuid) = cat.get_owner() else {
                return false;
            };
            let world = cat.get_entity().world.load_full();
            let Some(owner) = world.get_player_by_uuid(owner_uuid) else {
                return false;
            };
            // Vanilla assigns `ownerPlayer` before the sleep check (line 541), so a non-sleeping
            // owner is still remembered for `canContinueToUse`.
            self.owner = Some(owner.clone());
            if !Self::owner_is_sleeping(&owner) {
                return false;
            }
            let cat_pos = cat.get_entity().pos.load();
            let owner_pos = owner.get_entity().pos.load();
            if cat_pos.squared_distance_to_vec(&owner_pos) > MAX_OWNER_DISTANCE_SQ {
                return false;
            }
            let Some(goal_pos) = Self::bedside_of(&cat, &owner) else {
                return false;
            };
            self.goal_pos = Some(goal_pos);
            !Self::space_is_occupied(&cat, goal_pos)
        })
    }

    fn should_continue<'a>(&'a mut self, _mob: &'a dyn Mob) -> GoalFuture<'a, bool> {
        Box::pin(async {
            // `canContinueToUse` lines 575-581.
            let Some(cat) = self.cat.upgrade() else {
                return false;
            };
            let (Some(owner), Some(goal_pos)) = (self.owner.as_ref(), self.goal_pos) else {
                return false;
            };
            Self::cat_is_relaxable(&cat)
                && Self::owner_is_sleeping(owner)
                && !Self::space_is_occupied(&cat, goal_pos)
        })
    }

    fn start<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async {
            // `start` lines 584-589.
            let Some(goal_pos) = self.goal_pos else {
                return;
            };
            if let Some(cat) = self.cat.upgrade()
                && cat.is_sitting()
            {
                cat.set_sitting(false);
            }
            let entity = mob.get_entity();
            mob.get_mob_entity()
                .navigator
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .set_progress(NavigatorGoal::new(
                    entity.pos.load(),
                    goal_pos.to_f64(),
                    APPROACH_SPEED,
                ));
        })
    }

    fn stop<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async {
            // `stop` lines 592-603, minus the morning gift (see the type-level note).
            if let Some(cat) = self.cat.upgrade() {
                if cat.is_lying() {
                    cat.set_lying(false);
                }
                if cat.is_relax_state_one() {
                    cat.set_relax_state_one(false);
                }
            }
            self.on_bed_ticks = 0;
            mob.get_mob_entity()
                .navigator
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .stop();
        })
    }

    fn tick<'a>(&'a mut self, mob: &'a dyn Mob) -> GoalFuture<'a, ()> {
        Box::pin(async {
            // `tick` lines 628-644.
            let (Some(owner), Some(goal_pos)) = (self.owner.clone(), self.goal_pos) else {
                return;
            };
            let Some(cat) = self.cat.upgrade() else {
                return;
            };
            if cat.is_sitting() {
                cat.set_sitting(false);
            }
            let entity = mob.get_entity();
            let cat_pos = entity.pos.load();
            mob.get_mob_entity()
                .navigator
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .set_progress(NavigatorGoal::new(
                    cat_pos,
                    goal_pos.to_f64(),
                    APPROACH_SPEED,
                ));

            let owner_pos = owner.get_entity().pos.load();
            if cat_pos.squared_distance_to_vec(&owner_pos) < ON_BED_DISTANCE_SQ {
                self.on_bed_ticks += 1;
                if self.on_bed_ticks > to_goal_ticks(ON_BED_TICKS_BEFORE_LYING) {
                    if !cat.is_lying() {
                        cat.set_lying(true);
                    }
                    if cat.is_relax_state_one() {
                        cat.set_relax_state_one(false);
                    }
                } else {
                    entity.look_at(owner_pos);
                    if !cat.is_relax_state_one() {
                        cat.set_relax_state_one(true);
                    }
                }
            } else if cat.is_lying() {
                cat.set_lying(false);
            }
        })
    }

    // No `should_run_every_tick` override: `Cat.CatRelaxOnOwnerGoal` does not override
    // `requiresUpdateEveryTick`, so it ticks on the goal cadence and `to_goal_ticks(16)` above
    // lands on vanilla's `adjustedTickDelay(16)` in game ticks.

    fn controls(&self) -> Controls {
        // `Cat.CatRelaxOnOwnerGoal` sets no flags, so it runs alongside the movement goals;
        // it drives the navigator directly instead.
        Controls::empty()
    }
}
