//! Ports of vanilla's random-position helpers
//! (`net/minecraft/world/entity/ai/util/{RandomPos,DefaultRandomPos,LandRandomPos,GoalUtils}.java`).
//!
//! `RandomStrollGoal` and every goal that overrides its `getPosition()` picks a destination
//! through these, so a goal ported without them ends up with an ad-hoc offset that ignores
//! build height, home restriction, navmesh stability and pathfinding malus. The three iron
//! golem goals (`MoveTowardsTargetGoal`, `MoveBackToVillageGoal`,
//! `GolemRandomStrollInVillageGoal`) all need them, so they live here rather than being
//! re-approximated per goal.
//!
//! Deliberately *not* ported: `RandomPos.moveUpToAboveSolid` (flying mobs only) and
//! `getPosAway` (avoid-goals, which Pumpkin already approximates elsewhere).

use pumpkin_data::tag::Taggable;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use rand::RngExt;
use rand::rngs::ThreadRng;
use std::sync::atomic::Ordering;

use crate::entity::mob::Mob;

/// `RandomPos.RANDOM_POS_ATTEMPTS` (`RandomPos.java:16`).
const RANDOM_POS_ATTEMPTS: u32 = 10;

/// `Mth.SQRT_OF_TWO`, a `float` constant in vanilla - widened here so the `dist` product in
/// [`generate_random_direction_within_radians`] rounds the way `RandomPos.java:37` does.
const SQRT_OF_TWO: f64 = std::f32::consts::SQRT_2 as f64;

/// `GoalUtils.mobRestricted` (`GoalUtils.java:16-18`): the mob has a home *and* is currently
/// close enough to it that the home radius can actually constrain a candidate.
fn mob_restricted(mob: &dyn Mob, horizontal_dist: f64) -> bool {
    let mob_entity = mob.get_mob_entity();
    let radius = mob_entity.position_target_range.load(Ordering::Relaxed);
    if radius == -1 {
        return false;
    }
    let home = mob_entity.position_target.load();
    let pos = mob_entity.living_entity.entity.pos.load();
    let dx = f64::from(home.0.x) + 0.5 - pos.x;
    let dy = f64::from(home.0.y) + 0.5 - pos.y;
    let dz = f64::from(home.0.z) + 0.5 - pos.z;
    let limit = f64::from(radius) + horizontal_dist + 1.0;
    dx.mul_add(dx, dy.mul_add(dy, dz * dz)) < limit * limit
}

/// `RandomPos.generateRandomDirection` (`RandomPos.java:18-23`).
fn generate_random_direction(rng: &mut ThreadRng, horizontal: i32, vertical: i32) -> Vector3<i32> {
    Vector3::new(
        rng.random_range(-horizontal..=horizontal),
        rng.random_range(-vertical..=vertical),
        rng.random_range(-horizontal..=horizontal),
    )
}

/// `RandomPos.generateRandomDirectionWithinRadians` (`RandomPos.java:25-46`).
///
/// Returns `None` for the same reason vanilla does: the polar sample is drawn on a circle of
/// radius `dist * sqrt(2)`, so it can land outside the axis-aligned `max_horizontal` box, and
/// vanilla rejects rather than clamping (which is what makes the resulting distribution
/// roughly uniform over the box instead of piling up on its edges).
fn generate_random_direction_within_radians(
    rng: &mut ThreadRng,
    min_horizontal: f64,
    max_horizontal: f64,
    vertical: i32,
    x_dir: f64,
    z_dir: f64,
    max_xz_radians_from_dir: f64,
) -> Option<Vector3<i32>> {
    let y_radians_center = z_dir.atan2(x_dir) - std::f64::consts::FRAC_PI_2;
    let y_radians = f64::from(2.0f32.mul_add(rng.random::<f32>(), -1.0))
        .mul_add(max_xz_radians_from_dir, y_radians_center);
    let t = rng.random::<f64>().sqrt();
    let dist = t.mul_add(max_horizontal - min_horizontal, min_horizontal) * SQRT_OF_TWO;
    let xt = -dist * y_radians.sin();
    let zt = dist * y_radians.cos();
    if xt.abs() > max_horizontal || zt.abs() > max_horizontal {
        return None;
    }
    let yt = rng.random_range(-vertical..=vertical);
    Some(Vector3::new(xt.floor() as i32, yt, zt.floor() as i32))
}

/// `RandomPos.generateRandomPosTowardDirection` (`RandomPos.java:114-133`): turns a relative
/// direction into an absolute block position, biased back towards the home position when the
/// mob is restricted.
fn generate_random_pos_toward_direction(
    mob: &dyn Mob,
    xz_dist: f64,
    rng: &mut ThreadRng,
    direction: Vector3<i32>,
) -> BlockPos {
    let mob_entity = mob.get_mob_entity();
    let pos = mob_entity.living_entity.entity.pos.load();
    let mut xt = f64::from(direction.x);
    let mut zt = f64::from(direction.z);
    let has_home = mob_entity.position_target_range.load(Ordering::Relaxed) != -1;
    if has_home && xz_dist > 1.0 {
        let home = mob_entity.position_target.load();
        let x_bias = rng.random::<f64>() * xz_dist / 2.0;
        let z_bias = rng.random::<f64>() * xz_dist / 2.0;
        if pos.x > f64::from(home.0.x) {
            xt -= x_bias;
        } else {
            xt += x_bias;
        }
        if pos.z > f64::from(home.0.z) {
            zt -= z_bias;
        } else {
            zt += z_bias;
        }
    }
    BlockPos::new(
        (xt + pos.x).floor() as i32,
        (f64::from(direction.y) + pos.y).floor() as i32,
        (zt + pos.z).floor() as i32,
    )
}

/// The `isOutsideLimits` / `isRestricted` / `isNotStable` triple shared by
/// `DefaultRandomPos.generateRandomPosTowardDirection` (`DefaultRandomPos.java:51-53`) and
/// `LandRandomPos.generateRandomPosTowardDirection` (`LandRandomPos.java:75`).
fn passes_common_checks(mob: &dyn Mob, restrict: bool, pos: BlockPos) -> bool {
    let mob_entity = mob.get_mob_entity();
    let world = mob_entity.living_entity.entity.world.load();
    if !(world.get_bottom_y()..=world.get_top_y()).contains(&pos.0.y) {
        return false;
    }
    if restrict && !mob_entity.is_in_position_target_range_pos(&pos) {
        return false;
    }
    let navigator = mob_entity
        .navigator
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    navigator.is_stable_destination(&world, &pos)
}

/// `GoalUtils.hasMalus` (`GoalUtils.java:40-42`).
fn has_malus(mob: &dyn Mob, pos: BlockPos) -> bool {
    let mob_entity = mob.get_mob_entity();
    let world = mob_entity.living_entity.entity.world.load();
    let navigator = mob_entity
        .navigator
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    navigator.has_pathfinding_malus(&world, &pos)
}

/// `LandRandomPos.movePosUpOutOfSolid` (`LandRandomPos.java:66-69`), including
/// `RandomPos.moveUpOutOfSolid` (`RandomPos.java:49-61`).
fn move_pos_up_out_of_solid(mob: &dyn Mob, pos: BlockPos) -> Option<BlockPos> {
    let world = mob.get_mob_entity().living_entity.entity.world.load();
    let max_y = world.get_top_y();
    let mut landing = pos;
    if world.get_block_state(&landing).is_solid() {
        landing = landing.up();
        while landing.0.y <= max_y && world.get_block_state(&landing).is_solid() {
            landing = landing.up();
        }
    }
    if world
        .get_fluid(&landing)
        .has_tag(&pumpkin_data::tag::Fluid::MINECRAFT_WATER)
        || has_malus(mob, landing)
    {
        return None;
    }
    Some(landing)
}

/// `AirAndWaterRandomPos.getPos` (`AirAndWaterRandomPos.java:8-13`).
pub fn air_and_water_get_pos(
    mob: &dyn Mob,
    horizontal: i32,
    vertical: i32,
    flying_height: i32,
    x_dir: f64,
    z_dir: f64,
    max_xz_radians_from_dir: f64,
) -> Option<Vector3<f64>> {
    let restrict = mob_restricted(mob, f64::from(horizontal));
    generate_random_pos(mob, |rng| {
        let direction = generate_random_direction_within_radians(
            rng,
            0.0,
            f64::from(horizontal),
            vertical,
            x_dir,
            z_dir,
            max_xz_radians_from_dir,
        )?;
        let candidate = generate_random_pos_toward_direction(
            mob,
            f64::from(horizontal),
            rng,
            Vector3::new(direction.x, direction.y + flying_height, direction.z),
        );
        let mob_entity = mob.get_mob_entity();
        let world = mob_entity.living_entity.entity.world.load();
        if !(world.get_bottom_y()..=world.get_top_y()).contains(&candidate.0.y)
            || (restrict && !mob_entity.is_in_position_target_range_pos(&candidate))
        {
            return None;
        }
        let landing = move_up_out_of_solid(mob, candidate);
        (!has_malus(mob, landing)).then_some(landing)
    })
}

fn move_up_out_of_solid(mob: &dyn Mob, pos: BlockPos) -> BlockPos {
    let world = mob.get_mob_entity().living_entity.entity.world.load();
    let mut landing = pos;
    if world.get_block_state(&landing).is_solid() {
        landing = landing.up();
        while landing.0.y <= world.get_top_y() && world.get_block_state(&landing).is_solid() {
            landing = landing.up();
        }
    }
    landing
}

/// `RandomPos.generateRandomPos` (`RandomPos.java:96-112`): ten independent draws, keeping the
/// one with the highest `getWalkTargetValue`, then `Vec3.atBottomCenterOf` on the winner.
fn generate_random_pos(
    mob: &dyn Mob,
    mut supplier: impl FnMut(&mut ThreadRng) -> Option<BlockPos>,
) -> Option<Vector3<f64>> {
    let mut rng = mob.get_random();
    let mut best_weight = f64::NEG_INFINITY;
    let mut best_pos = None;
    for _ in 0..RANDOM_POS_ATTEMPTS {
        if let Some(pos) = supplier(&mut rng) {
            let weight = mob.get_walk_target_value(&pos);
            if weight > best_weight {
                best_weight = weight;
                best_pos = Some(pos);
            }
        }
    }
    best_pos.map(|pos| {
        Vector3::new(
            f64::from(pos.0.x) + 0.5,
            f64::from(pos.0.y),
            f64::from(pos.0.z) + 0.5,
        )
    })
}

/// `DefaultRandomPos.getPosTowards` (`DefaultRandomPos.java:17-31`).
///
/// Used by `MoveTowardsTargetGoal` (`MoveTowardsTargetGoal.java:37`) and
/// `MoveBackToVillageGoal` (`MoveBackToVillageGoal.java:34`).
pub fn default_get_pos_towards(
    mob: &dyn Mob,
    horizontal: i32,
    vertical: i32,
    towards: Vector3<f64>,
    max_xz_radians_from_dir: f64,
) -> Option<Vector3<f64>> {
    let pos = mob.get_mob_entity().living_entity.entity.pos.load();
    let dir_x = towards.x - pos.x;
    let dir_z = towards.z - pos.z;
    let restrict = mob_restricted(mob, f64::from(horizontal));
    generate_random_pos(mob, |rng| {
        let direction = generate_random_direction_within_radians(
            rng,
            0.0,
            f64::from(horizontal),
            vertical,
            dir_x,
            dir_z,
            max_xz_radians_from_dir,
        )?;
        let candidate =
            generate_random_pos_toward_direction(mob, f64::from(horizontal), rng, direction);
        // `DefaultRandomPos` also rejects on malus, which `LandRandomPos` defers to
        // `movePosUpOutOfSolid` instead (`DefaultRandomPos.java:54`).
        (passes_common_checks(mob, restrict, candidate) && !has_malus(mob, candidate))
            .then_some(candidate)
    })
}

/// `DefaultRandomPos.getPos` (`DefaultRandomPos.java:10-15`).
pub fn default_get_pos(mob: &dyn Mob, horizontal: i32, vertical: i32) -> Option<Vector3<f64>> {
    let restrict = mob_restricted(mob, f64::from(horizontal));
    generate_random_pos(mob, |rng| {
        let direction = generate_random_direction(rng, horizontal, vertical);
        let candidate =
            generate_random_pos_toward_direction(mob, f64::from(horizontal), rng, direction);
        (passes_common_checks(mob, restrict, candidate) && !has_malus(mob, candidate))
            .then_some(candidate)
    })
}

/// `LandRandomPos.getPos` (`LandRandomPos.java:10-23`), i.e.
/// `GolemRandomStrollInVillageGoal.getPositionTowardsAnywhere`
/// (`GolemRandomStrollInVillageGoal.java:51-53`).
pub fn land_get_pos(mob: &dyn Mob, horizontal: i32, vertical: i32) -> Option<Vector3<f64>> {
    let restrict = mob_restricted(mob, f64::from(horizontal));
    generate_random_pos(mob, |rng| {
        let direction = generate_random_direction(rng, horizontal, vertical);
        let candidate =
            generate_random_pos_toward_direction(mob, f64::from(horizontal), rng, direction);
        if !passes_common_checks(mob, restrict, candidate) {
            return None;
        }
        move_pos_up_out_of_solid(mob, candidate)
    })
}

/// `LandRandomPos.getPosTowards` (`LandRandomPos.java:25-29`), which routes through
/// `getPosInDirection` with `minHorizontalDist = 0` and a fixed `PI/2` cone
/// (`LandRandomPos.java:54`).
pub fn land_get_pos_towards(
    mob: &dyn Mob,
    horizontal: i32,
    vertical: i32,
    towards: Vector3<f64>,
) -> Option<Vector3<f64>> {
    let pos = mob.get_mob_entity().living_entity.entity.pos.load();
    let dir_x = towards.x - pos.x;
    let dir_z = towards.z - pos.z;
    let restrict = mob_restricted(mob, f64::from(horizontal));
    generate_random_pos(mob, |rng| {
        let direction = generate_random_direction_within_radians(
            rng,
            0.0,
            f64::from(horizontal),
            vertical,
            dir_x,
            dir_z,
            std::f64::consts::FRAC_PI_2,
        )?;
        let candidate =
            generate_random_pos_toward_direction(mob, f64::from(horizontal), rng, direction);
        if !passes_common_checks(mob, restrict, candidate) {
            return None;
        }
        move_pos_up_out_of_solid(mob, candidate)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn direction_within_radians_stays_in_the_axis_aligned_box() {
        let mut rng = rand::rng();
        for _ in 0..2000 {
            if let Some(direction) = generate_random_direction_within_radians(
                &mut rng,
                0.0,
                10.0,
                7,
                1.0,
                0.0,
                std::f64::consts::FRAC_PI_2,
            ) {
                assert!(direction.x.abs() <= 10, "x={}", direction.x);
                assert!(direction.z.abs() <= 10, "z={}", direction.z);
                assert!(direction.y.abs() <= 7, "y={}", direction.y);
            }
        }
    }

    #[test]
    fn direction_within_radians_respects_the_cone() {
        // Pointing at +X with a PI/2 half-cone: every accepted sample must have a
        // non-negative X component, since the sampled angle stays within +/-PI/2 of the
        // direction. Guards the `-PI/2` phase shift in `RandomPos.java:35`, which is easy
        // to drop and silently sends the mob sideways.
        let mut rng = rand::rng();
        let mut accepted = 0;
        for _ in 0..2000 {
            if let Some(direction) = generate_random_direction_within_radians(
                &mut rng,
                0.0,
                10.0,
                7,
                1.0,
                0.0,
                std::f64::consts::FRAC_PI_2,
            ) {
                accepted += 1;
                assert!(direction.x >= -1, "x={}", direction.x);
            }
        }
        assert!(accepted > 0);
    }

    #[test]
    fn plain_direction_is_bounded() {
        let mut rng = rand::rng();
        for _ in 0..500 {
            let direction = generate_random_direction(&mut rng, 10, 7);
            assert!(direction.x.abs() <= 10);
            assert!(direction.y.abs() <= 7);
            assert!(direction.z.abs() <= 10);
        }
    }
}
