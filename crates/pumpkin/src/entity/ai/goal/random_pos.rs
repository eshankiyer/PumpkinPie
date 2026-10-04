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
//! Deliberately *not* ported: `getPosAway` (avoid-goals, which Pumpkin already approximates
//! elsewhere).

use pumpkin_data::tag::Taggable;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use rand::RngExt;
use rand::rngs::ThreadRng;
use std::sync::atomic::Ordering;

use crate::block::pathfindable::{PathComputationType, is_pathfindable};
use crate::entity::mob::Mob;
use pumpkin_data::fluid::Fluid;

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

/// `RandomPos.moveUpToAboveSolid` (`RandomPos.java:63-90`): from a solid `pos`, climb out of
/// the solid column, then up to `above_solid_amount` more blocks, stopping one short of the next
/// solid block above. A non-solid `pos` is returned unchanged.
fn move_up_to_above_solid(
    pos: BlockPos,
    above_solid_amount: i32,
    max_y: i32,
    solid: impl Fn(BlockPos) -> bool,
) -> BlockPos {
    debug_assert!(
        above_solid_amount >= 0,
        "aboveSolidAmount was {above_solid_amount}, expected >= 0"
    );
    if !solid(pos) {
        return pos;
    }
    let mut landing = pos.up();
    while landing.0.y <= max_y && solid(landing) {
        landing = landing.up();
    }
    let first_non_solid_y = landing.0.y;
    while landing.0.y <= max_y && landing.0.y - first_non_solid_y < above_solid_amount {
        landing = landing.up();
        if solid(landing) {
            landing = landing.down();
            break;
        }
    }
    landing
}

/// `HoverRandomPos.getPos` (`HoverRandomPos.java:9-44`): a ground candidate in the view cone,
/// lifted `hover_min_height..=hover_max_height` blocks above the solid block it lands on.
#[allow(clippy::too_many_arguments, reason = "mirrors vanilla's signature")]
pub fn hover_get_pos(
    mob: &dyn Mob,
    horizontal: i32,
    vertical: i32,
    x_dir: f64,
    z_dir: f64,
    max_xz_radians_from_dir: f64,
    hover_max_height: i32,
    hover_min_height: i32,
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
        let candidate =
            generate_random_pos_toward_direction(mob, f64::from(horizontal), rng, direction);
        // `LandRandomPos.generateRandomPosTowardDirection` (`LandRandomPos.java:71-78`).
        if !passes_common_checks(mob, restrict, candidate) {
            return None;
        }
        // Drawn only after the candidate passed, as in vanilla.
        let amount =
            rng.random_range(0..=(hover_max_height - hover_min_height)) + hover_min_height;
        let world = mob.get_mob_entity().living_entity.entity.world.load();
        let landing = move_up_to_above_solid(candidate, amount, world.get_top_y(), |p| {
            world.get_block_state(&p).is_solid()
        });
        if world
            .get_fluid(&landing)
            .has_tag(&pumpkin_data::tag::Fluid::MINECRAFT_WATER)
            || has_malus(mob, landing)
        {
            return None;
        }
        Some(landing)
    })
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

/// The ten candidate positions of `LandRandomPos.getPos(mob, horizontal, vertical, weight)`
/// (`LandRandomPos.java:15-22`), before `RandomPos.generateRandomPos` (`RandomPos.java:96-112`)
/// keeps the one with the highest weight.
///
/// [`land_get_pos`] scores candidates with `getWalkTargetValue`, which is synchronous. A goal
/// with its own weight, such as `MoveThroughVillageGoal`'s village-distance one, needs asynchronous
/// world lookups, so it takes the candidates and scores them itself. Candidates rejected by the
/// stability, restriction, water or malus checks are simply absent, as `null` suppliers are in
/// vanilla.
pub(crate) fn land_get_candidates(mob: &dyn Mob, horizontal: i32, vertical: i32) -> Vec<BlockPos> {
    let restrict = mob_restricted(mob, f64::from(horizontal));
    let mut rng = mob.get_random();
    let mut candidates = Vec::with_capacity(RANDOM_POS_ATTEMPTS as usize);
    for _ in 0..RANDOM_POS_ATTEMPTS {
        let direction = generate_random_direction(&mut rng, horizontal, vertical);
        let candidate =
            generate_random_pos_toward_direction(mob, f64::from(horizontal), &mut rng, direction);
        if !passes_common_checks(mob, restrict, candidate) {
            continue;
        }
        if let Some(landing) = move_pos_up_out_of_solid(mob, candidate) {
            candidates.push(landing);
        }
    }
    candidates
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

/// `RandomStroll.SWIM_XY_DISTANCE_TIERS` (`RandomStroll.java:21`): the `(horizontal, vertical)`
/// reach of each successive swim-target attempt.
const SWIM_XY_DISTANCE_TIERS: [(i32, i32); 6] = [(1, 1), (3, 3), (5, 5), (6, 5), (7, 7), (10, 7)];

/// `BehaviorUtils.getRandomSwimmablePos` (`BehaviorUtils.java:159-168`): re-rolls
/// `DefaultRandomPos.getPos` up to ten times until the block is pathfindable for WATER, and
/// returns the last roll even if it never was.
fn get_random_swimmable_pos(mob: &dyn Mob, horizontal: i32, vertical: i32) -> Option<Vector3<f64>> {
    let world = mob.get_mob_entity().living_entity.entity.world.load();
    let mut target = default_get_pos(mob, horizontal, vertical);
    let mut count = 0;
    while let Some(pos) = target {
        if count >= 10
            || is_pathfindable(
                world.get_block_state(&BlockPos::floored_v(pos)),
                PathComputationType::Water,
            )
        {
            break;
        }
        count += 1;
        target = default_get_pos(mob, horizontal, vertical);
    }
    target
}

/// `Mob.isWithinHome(Vec3)` (`Mob.java:1204-1206`), measured from the home block's center
/// (`Vec3i.distToCenterSqr`) with vanilla's wrapping `int` radius product.
#[allow(
    clippy::suboptimal_flops,
    reason = "vanilla rounds each multiply and add separately; fusing can flip the `<` at the radius"
)]
fn is_within_home_vec(mob: &dyn Mob, pos: Vector3<f64>) -> bool {
    let mob_entity = mob.get_mob_entity();
    let radius = mob_entity.position_target_range.load(Ordering::Relaxed);
    if radius == -1 {
        return true;
    }
    let home = mob_entity.position_target.load();
    let dx = f64::from(home.0.x) + 0.5 - pos.x;
    let dy = f64::from(home.0.y) + 0.5 - pos.y;
    let dz = f64::from(home.0.z) + 0.5 - pos.z;
    dx * dx + dy * dy + dz * dz < f64::from(radius.wrapping_mul(radius))
}

/// `position + normalize(vectorTo(fallback)) * (horizontal, vertical, horizontal)`
/// (`RandomStroll.java:65`), with `Vec3.normalize` collapsing to zero below `1.0E-5F`
/// (`Vec3.java:83-86`).
#[allow(
    clippy::suboptimal_flops,
    reason = "vanilla `Vec3` rounds each multiply and add separately; the result is floored to a block"
)]
fn extend_swim_target(
    origin: Vector3<f64>,
    fallback: Vector3<f64>,
    horizontal: i32,
    vertical: i32,
) -> Vector3<f64> {
    let dx = fallback.x - origin.x;
    let dy = fallback.y - origin.y;
    let dz = fallback.z - origin.z;
    let dist = (dx * dx + dy * dy + dz * dz).sqrt();
    let (nx, ny, nz) = if dist < f64::from(1.0e-5f32) {
        (0.0, 0.0, 0.0)
    } else {
        (dx / dist, dy / dist, dz / dist)
    };
    let horizontal = f64::from(horizontal);
    Vector3::new(
        origin.x + nx * horizontal,
        origin.y + ny * f64::from(vertical),
        origin.z + nz * horizontal,
    )
}

/// `RandomStroll.getTargetSwimPos` (`RandomStroll.java:57-77`).
///
/// A swimmable random position within the smallest tier, then pushed outward along the same
/// direction tier by tier for as long as each extension stays in a fluid and inside the home
/// restriction.
pub fn get_target_swim_pos(mob: &dyn Mob) -> Option<Vector3<f64>> {
    let entity = &mob.get_mob_entity().living_entity.entity;
    let world = entity.world.load();
    let mut fallback: Option<Vector3<f64>> = None;
    let mut target = None;
    for (horizontal, vertical) in SWIM_XY_DISTANCE_TIERS {
        target = fallback.map_or_else(
            || get_random_swimmable_pos(mob, horizontal, vertical),
            |fallback| {
                Some(extend_swim_target(
                    entity.pos.load(),
                    fallback,
                    horizontal,
                    vertical,
                ))
            },
        );
        let restrict = mob_restricted(mob, f64::from(horizontal));
        let Some(pos) = target else {
            return fallback;
        };
        if world.get_fluid(&BlockPos::floored_v(pos)).id == Fluid::EMPTY.id
            || (restrict && !is_within_home_vec(mob, pos))
        {
            return fallback;
        }
        fallback = Some(pos);
    }
    target
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

    #[test]
    fn move_up_to_above_solid_passes_non_solid_through() {
        let pos = BlockPos::new(0, 64, 0);
        assert_eq!(move_up_to_above_solid(pos, 3, 320, |_| false), pos);
    }

    #[test]
    fn move_up_to_above_solid_climbs_column_then_hovers() {
        // Solid at y 64..=66; first non-solid is 67, then up to 3 more.
        let solid = |p: BlockPos| (64..=66).contains(&p.0.y);
        let landing = move_up_to_above_solid(BlockPos::new(0, 64, 0), 3, 320, solid);
        assert_eq!(landing.0.y, 70);
        let landing = move_up_to_above_solid(BlockPos::new(0, 64, 0), 0, 320, solid);
        assert_eq!(landing.0.y, 67);
    }

    #[test]
    fn move_up_to_above_solid_stops_below_ceiling() {
        // Ground at 64, ceiling at 67: hovering stops at 66, one below the ceiling.
        let solid = |p: BlockPos| p.0.y == 64 || p.0.y == 67;
        let landing = move_up_to_above_solid(BlockPos::new(0, 64, 0), 3, 320, solid);
        assert_eq!(landing.0.y, 66);
    }

    #[test]
    fn move_up_to_above_solid_respects_max_y() {
        let solid = |p: BlockPos| p.0.y == 64;
        let landing = move_up_to_above_solid(BlockPos::new(0, 64, 0), 3, 66, solid);
        assert_eq!(landing.0.y, 67);
        // A solid column reaching past max_y stops one above max_y.
        let landing = move_up_to_above_solid(BlockPos::new(0, 64, 0), 3, 66, |_| true);
        assert_eq!(landing.0.y, 67);
    }

    #[test]
    fn swim_tiers_match_vanilla() {
        assert_eq!(
            SWIM_XY_DISTANCE_TIERS,
            [(1, 1), (3, 3), (5, 5), (6, 5), (7, 7), (10, 7)]
        );
    }

    #[test]
    fn swim_target_extends_along_the_fallback_direction() {
        let origin = Vector3::new(0.5, 64.0, 0.5);
        let extended = extend_swim_target(origin, Vector3::new(3.5, 68.0, 0.5), 5, 5);
        assert!((extended.x - 3.5).abs() < 1e-9);
        assert!((extended.y - 68.0).abs() < 1e-9);
        assert!((extended.z - 0.5).abs() < 1e-9);
        // The vertical tier scales only y: (6, 5) keeps x and z at 6 and y at 5.
        let extended = extend_swim_target(origin, Vector3::new(0.5, 64.0, 2.5), 6, 5);
        assert!((extended.z - 6.5).abs() < 1e-9);
        assert!((extended.y - 64.0).abs() < 1e-9);
    }

    #[test]
    fn swim_target_with_zero_direction_stays_put() {
        let origin = Vector3::new(1.0, 2.0, 3.0);
        let extended = extend_swim_target(origin, origin, 10, 7);
        assert_eq!((extended.x, extended.y, extended.z), (1.0, 2.0, 3.0));
    }
}
