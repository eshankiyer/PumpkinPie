use super::{Entity, EntityBase, NBTStorage, living::LivingEntity};
use crate::block::BlockHitResult;
use crate::block::blocks::redstone::target_block::TargetBlock;
use crate::entity::player::advancement::trigger::AdvancementTrigger;
use crate::server::Server;
use crate::world::World;
use pumpkin_data::BlockDirection;
use pumpkin_data::entity::EntityType;
use pumpkin_data::tag::{self, Taggable};
use pumpkin_protocol::java::client::play::CEntityVelocity;
use pumpkin_util::math::boundingbox::BoundingBox;
use pumpkin_util::math::{position::BlockPos, vector3::Vector3};
use std::{
    sync::Arc,
    sync::atomic::{AtomicBool, Ordering},
};
pub mod arrow;
pub mod dragon_fireball;
pub mod egg;
pub mod ender_pearl;
pub mod evoker_fangs;
pub mod experience_bottle;
pub mod eye_of_ender;
pub mod fireball;
pub mod firework_rocket;
pub mod fishing_bobber;
pub mod lingering_potion;
pub mod llama_spit;
pub mod shulker_bullet;
pub mod small_fireball;
pub mod snowball;
pub mod splash_potion;
pub mod trident;
pub mod wind_charge;
pub mod wither_skull;

#[must_use]
pub fn is_projectile(entity_type: &EntityType) -> bool {
    *entity_type == EntityType::ARROW
        // Vanilla `SpectralArrow extends AbstractArrow extends Projectile`
        // (`projectile/arrow/SpectralArrow.java`) - a distinct `EntityType` from `ARROW`
        // (`ArrowEntity::entity_type_for_item` in arrow.rs picks it for spectral arrows), so it
        // needs its own arm here or a spectral arrow hit falls through every `is_projectile`
        // check in this module (base knockback direction, projectile-vs-projectile passthrough).
        || *entity_type == EntityType::SPECTRAL_ARROW
        || *entity_type == EntityType::TRIDENT
        || *entity_type == EntityType::EGG
        || *entity_type == EntityType::SNOWBALL
        || *entity_type == EntityType::FIREWORK_ROCKET
        || *entity_type == EntityType::WIND_CHARGE
        // Vanilla `BreezeWindCharge extends AbstractWindCharge extends Projectile`
        // (`projectile/hurtingprojectile/windcharge/BreezeWindCharge.java`) - a Breeze's own
        // wind charge is a distinct `EntityType` from the player-thrown `WIND_CHARGE`
        // (`entity/mob/breeze.rs` spawns it directly), so it needs the same arm.
        || *entity_type == EntityType::BREEZE_WIND_CHARGE
        || *entity_type == EntityType::SPLASH_POTION
        || *entity_type == EntityType::LINGERING_POTION
        || *entity_type == EntityType::ENDER_PEARL
        || *entity_type == EntityType::SHULKER_BULLET
        || *entity_type == EntityType::FIREBALL
        || *entity_type == EntityType::DRAGON_FIREBALL
        || *entity_type == EntityType::SMALL_FIREBALL
        || *entity_type == EntityType::FISHING_BOBBER
        || *entity_type == EntityType::EXPERIENCE_BOTTLE
        || *entity_type == EntityType::WITHER_SKULL
        || *entity_type == EntityType::LLAMA_SPIT
}

/// Applies `Projectile.shootFromRotation`'s known-motion inheritance
/// (`Projectile.java:157-159`).
fn add_known_movement(
    projectile_velocity: Vector3<f64>,
    source_movement: Vector3<f64>,
    source_on_ground: bool,
) -> Vector3<f64> {
    Vector3::new(
        projectile_velocity.x + source_movement.x,
        projectile_velocity.y
            + if source_on_ground {
                0.0
            } else {
                source_movement.y
            },
        projectile_velocity.z + source_movement.z,
    )
}

/// Minimum horizontal distance between the shooter and the impact for `adventure/bullseye`.
const BULLSEYE_MIN_HORIZONTAL_DISTANCE: f64 = 30.0;

/// Powers a target block that a projectile just hit and grants the `adventure/bullseye`
/// advancement to the shooter when the hit was worth a full signal from far enough away.
pub async fn on_target_block_hit(
    world: &Arc<World>,
    position: &BlockPos,
    face: BlockDirection,
    hit_pos: Vector3<f64>,
    owner_id: Option<i32>,
    delay: u8,
) {
    let power = TargetBlock::trigger(world, position, face, hit_pos, delay).await;
    if power != TargetBlock::MAX_POWER {
        return;
    }
    let Some(player) = owner_id.and_then(|id| world.get_player_by_id(id)) else {
        return;
    };
    let distance = player
        .living_entity
        .entity
        .pos
        .load()
        .sub(&hit_pos)
        .horizontal_length();
    if distance >= BULLSEYE_MIN_HORIZONTAL_DISTANCE {
        player
            .trigger_advancement(AdvancementTrigger::Bullseye)
            .await;
    }
}

/// Resolves the id of a projectile's owner, following it back to the shooter the way
/// vanilla's `DamageSource.getEntity()` does for projectile damage sources.
///
/// (`DamageSource.java:63` returns `causingEntity`, which is set to the shooter rather
/// than the projectile itself - see `directEntity` vs `causingEntity`,
/// `DamageSource.java:17-18`). Returns `None` if `source` is not a projectile type this
/// resolves, or it is one with no tracked owner (e.g. dispenser-fired).
///
/// Callers reproducing `source.getEntity() instanceof Player` must also check
/// `source.get_player().is_some()` themselves for the direct-hit case; this function only
/// covers the indirect, projectile-owner case.
#[must_use]
pub fn projectile_owner_id(source: &dyn EntityBase) -> Option<i32> {
    let any = source.cast_any();
    if let Some(e) = any.downcast_ref::<arrow::ArrowEntity>() {
        return e.owner_id;
    }
    if let Some(e) = any.downcast_ref::<trident::TridentEntity>() {
        return e.owner_id;
    }
    if let Some(e) = any.downcast_ref::<fishing_bobber::FishingBobberEntity>() {
        return Some(e.owner_id);
    }
    if let Some(e) = any.downcast_ref::<shulker_bullet::ShulkerBulletEntity>() {
        return Some(e.owner_id);
    }
    if let Some(e) = any.downcast_ref::<snowball::SnowballEntity>() {
        return e.thrown.owner_id;
    }
    if let Some(e) = any.downcast_ref::<egg::EggEntity>() {
        return e.thrown.owner_id;
    }
    if let Some(e) = any.downcast_ref::<splash_potion::SplashPotionEntity>() {
        return e.thrown.owner_id;
    }
    if let Some(e) = any.downcast_ref::<lingering_potion::LingeringPotionEntity>() {
        return e.thrown.owner_id;
    }
    if let Some(e) = any.downcast_ref::<experience_bottle::ExperienceBottleEntity>() {
        return e.thrown.owner_id;
    }
    if let Some(e) = any.downcast_ref::<ender_pearl::EnderPearlEntity>() {
        return e.thrown.owner_id;
    }
    if let Some(e) = any.downcast_ref::<fireball::FireballEntity>() {
        return e.thrown.owner_id;
    }
    if let Some(e) = any.downcast_ref::<small_fireball::SmallFireballEntity>() {
        return e.thrown.owner_id;
    }
    if let Some(e) = any.downcast_ref::<dragon_fireball::DragonFireballEntity>() {
        return e.thrown.owner_id;
    }
    if let Some(e) = any.downcast_ref::<wither_skull::WitherSkullEntity>() {
        return e.thrown.owner_id;
    }
    if let Some(e) = any.downcast_ref::<llama_spit::LlamaSpitEntity>() {
        return e.thrown.owner_id;
    }
    if let Some(e) = any.downcast_ref::<wind_charge::WindChargeEntity>() {
        return e.thrown_item_entity.owner_id;
    }
    if let Some(e) = any.downcast_ref::<firework_rocket::FireworkRocketEntity>() {
        return e.entity.owner_id;
    }

    None
}

/// `Projectile.mayInteract` (`Projectile.java:360-364`) as used by block projectile-hit callbacks.
///
/// Player-owned projectiles obey spawn protection and the world border; other owners are governed
/// by `mobGriefing`, while an unresolved owner has no restriction.
pub async fn projectile_may_interact(
    projectile: &dyn EntityBase,
    server: &Server,
    world: &Arc<World>,
    position: &BlockPos,
) -> bool {
    let Some(owner_id) = projectile_owner_id(projectile) else {
        return true;
    };
    let Some(owner) = world.get_entity_by_id(owner_id) else {
        return true;
    };
    if let Some(player) = owner.get_player() {
        return !player
            .is_under_spawn_protection(server, world, position)
            .await
            && world
                .worldborder
                .lock()
                .await
                .contains_block(position.0.x, position.0.z);
    }
    world.level_info.load().game_rules.mob_griefing
}

/// `Projectile.mayBreak` (`Projectile.java:366-368`).
#[must_use]
pub fn projectile_may_break(projectile: &dyn EntityBase, world: &World) -> bool {
    projectile
        .get_entity()
        .entity_type
        .has_tag(&tag::EntityType::MINECRAFT_IMPACT_PROJECTILES)
        && world
            .level_info
            .load()
            .game_rules
            .projectiles_can_break_blocks
}

/// Calls the block hook from `Projectile.onHitBlock` (`Projectile.java:312-315`).
pub async fn on_projectile_block_hit(
    world: &Arc<World>,
    server: &Server,
    projectile: &Arc<dyn EntityBase>,
    position: BlockPos,
    face: BlockDirection,
    hit_pos: Vector3<f64>,
) {
    let cursor_pos = Vector3::new(
        (hit_pos.x - f64::from(position.0.x)) as f32,
        (hit_pos.y - f64::from(position.0.y)) as f32,
        (hit_pos.z - f64::from(position.0.z)) as f32,
    );
    let hit = BlockHitResult {
        face: &face,
        cursor_pos: &cursor_pos,
    };
    let block = world.get_block(&position);
    let state = world.get_block_state(&position);
    world
        .block_registry
        .on_projectile_hit(
            block,
            server,
            world,
            projectile.as_ref(),
            &position,
            state,
            &hit,
        )
        .await;
}

/// The impact location vanilla passes to `GameEvent.PROJECTILE_LAND`.
///
/// `Projectile.java:300` uses the exact hit location for entity hits, `:305` uses the
/// hit block's position for block hits.
#[must_use]
pub fn projectile_land_pos(hit: &ProjectileHit) -> Vector3<f64> {
    match hit {
        ProjectileHit::Block { pos, .. } => Vector3::new(
            f64::from(pos.0.x) + 0.5,
            f64::from(pos.0.y) + 0.5,
            f64::from(pos.0.z) + 0.5,
        ),
        ProjectileHit::Entity { hit_pos, .. } => *hit_pos,
    }
}

/// The entity branch of `Projectile.hitTargetOrDeflectSelf` (`Projectile.java:244-255`).
///
/// Returns `true` when the target deflected the projectile, in which case the caller must skip
/// `on_hit` and must NOT despawn the projectile -- a deflected projectile keeps flying. The
/// `lastDeflectedBy` guard (`Projectile.java:250-251`) is what stops a projectile that is still
/// inside the deflector's expanded hitbox from being deflected again every tick, which would
/// otherwise leave it oscillating in place.
///
/// The world-border branch of the same method (`Projectile.java:256-262`) is not ported: nothing
/// here reports a world-border block hit.
pub fn try_deflect(hit: &ProjectileHit, projectile: &Arc<dyn EntityBase>) -> bool {
    let ProjectileHit::Entity { entity: target, .. } = hit else {
        return false;
    };

    let deflection = target.projectile_deflection(projectile.as_ref());
    if matches!(
        deflection,
        crate::entity::projectile_deflection::ProjectileDeflectionType::None
    ) {
        return false;
    }

    let target_id = target.get_entity().entity_id;
    let entity = projectile.get_entity();
    if entity.last_deflected_by.swap(target_id, Ordering::Relaxed) != target_id {
        deflection.deflect(projectile.as_ref(), Some(target.as_ref()));
    }
    true
}

/// Mirrors `Projectile.checkLeftOwner` and `isOutsideOwnerCollisionRange`
/// (`Projectile.java:105-127`). The movement loops below call this before their hit scan;
/// the shared entity flag lets arrows and tridents retain the same state as thrown items.
pub(crate) async fn check_left_owner(
    entity: &Entity,
    owner_id: Option<i32>,
    movement: Vector3<f64>,
) {
    if entity.projectile_left_owner.load(Ordering::Relaxed) {
        return;
    }

    let Some(owner_id) = owner_id else {
        entity.projectile_left_owner.store(true, Ordering::Relaxed);
        return;
    };
    let world = entity.world.load();
    let Some(owner) = world.get_entity_by_id(owner_id) else {
        entity.projectile_left_owner.store(true, Ordering::Relaxed);
        return;
    };

    let root_id = owner.get_entity().root_vehicle_id().await;
    let root = world
        .get_entity_by_id(root_id)
        .unwrap_or_else(|| owner.clone());
    let collision_box = entity
        .bounding_box
        .load()
        .expand_towards(movement.x, movement.y, movement.z)
        .expand(1.0, 1.0, 1.0);

    let mut pending = vec![root];
    while let Some(candidate) = pending.pop() {
        // `EntitySelector.CAN_BE_PICKED` is exactly the entity pickability predicate
        // (`EntitySelector.java:19`; `Projectile.java:117-124`).
        if candidate.is_pickable()
            && collision_box.intersects(&candidate.get_entity().bounding_box.load())
        {
            return;
        }
        pending.extend(
            candidate
                .get_entity()
                .passengers
                .lock()
                .await
                .iter()
                .cloned(),
        );
    }

    entity.projectile_left_owner.store(true, Ordering::Relaxed);
}

/// Returns the vanilla projectile override of `Entity.getPickRadius`
/// (`Projectile.java:370-378`). Projectile targets that are pickable use a one-block margin;
/// ordinary entities retain the base entity radius of zero (`Entity.java:2563-2565`).
#[must_use]
pub(crate) fn projectile_target_pick_radius(target: &dyn EntityBase) -> f64 {
    if is_projectile(target.get_entity().entity_type) && target.is_pickable() {
        1.0
    } else {
        0.0
    }
}

/// Emits `GameEvent.PROJECTILE_LAND`; call after `on_hit` runs.
///
/// `Projectile.onHit`, lines 299-300/304-305: `onHitEntity`/`onHitBlock` runs first,
/// then `gameEvent(GameEvent.PROJECTILE_LAND, <impact location>, GameEvent.Context.of(this,
/// ...))` fires for both the entity-hit and block-hit branches.
pub async fn emit_projectile_land(
    world: &Arc<World>,
    caller: &Arc<dyn EntityBase>,
    land_pos: Vector3<f64>,
) {
    crate::world::game_event::emit_game_event(
        world,
        pumpkin_data::game_event::GameEvent::ProjectileLand,
        land_pos,
        crate::world::game_event::GameEventContext::of_entity(caller.clone()),
    )
    .await;
}

/// `AbstractWindCharge` overrides both `canHitEntity` and the inertia getters, and
/// `ThrownItemEntity` is the shared body those live on here, so the two wind-charge-only
/// branches identify their projectile by downcasting the caller.
fn is_wind_charge(caller: &Arc<dyn EntityBase>) -> bool {
    caller
        .cast_any()
        .downcast_ref::<wind_charge::WindChargeEntity>()
        .is_some()
}

pub struct ThrownItemEntity {
    pub entity: Entity,
    pub owner_id: Option<i32>,
    pub collides_with_projectiles: bool,
    pub has_hit: AtomicBool,
    pub gravity: f64,
}

impl ThrownItemEntity {
    pub fn new(entity: Entity, owner: &Entity, gravity: f64) -> Self {
        let mut owner_pos = owner.pos.load();
        owner_pos.y += owner.get_eye_height() - 0.1;
        entity.pos.store(owner_pos);
        // `Projectile.getAddEntityPacket` (`Projectile.java:346-349`): the spawn packet's
        // generic "data" int carries the owner's entity id (0 with no owner), which the
        // client uses to attribute the projectile to its shooter.
        entity.data.store(owner.entity_id, Ordering::Relaxed);
        Self {
            entity,
            owner_id: Some(owner.entity_id),
            collides_with_projectiles: false,
            has_hit: AtomicBool::new(false),
            gravity,
        }
    }

    pub fn set_velocity_from(
        &self,
        shooter: &Entity,
        pitch: f32,
        yaw: f32,
        roll: f32,
        speed: f32,
        divergence: f32,
    ) {
        let yaw_rad = yaw.to_radians();
        let pitch_rad = pitch.to_radians();
        let roll_rad = (pitch + roll).to_radians();

        let x = -yaw_rad.sin() * pitch_rad.cos();
        let y = -roll_rad.sin();
        let z = yaw_rad.cos() * pitch_rad.cos();

        self.set_velocity(
            f64::from(x),
            f64::from(y),
            f64::from(z),
            f64::from(speed),
            f64::from(divergence),
        );

        // Vanilla `Projectile.shootFromRotation` inherits the shooter's known movement
        // after aiming, omitting vertical movement while grounded (`Projectile.java:157-159`).
        let source_movement = shooter.get_known_movement();
        let velocity = add_known_movement(
            self.entity.velocity.load(),
            source_movement,
            shooter.on_ground.load(Ordering::Relaxed),
        );
        self.entity.velocity.store(velocity);
    }

    pub fn set_velocity(&self, x: f64, y: f64, z: f64, power: f64, uncertainty: f64) {
        fn next_triangular(mode: f64, deviation: f64) -> f64 {
            deviation.mul_add(rand::random::<f64>() - rand::random::<f64>(), mode)
        }
        let velocity = Vector3::new(x, y, z)
            .normalize()
            .add_raw(
                next_triangular(0.0, 0.017_227_5 * uncertainty),
                next_triangular(0.0, 0.017_227_5 * uncertainty),
                next_triangular(0.0, 0.017_227_5 * uncertainty),
            )
            .multiply(power, power, power);

        self.entity.velocity.store(velocity);
        let len = velocity.horizontal_length();
        self.entity.set_rotation(
            velocity.x.atan2(velocity.z) as f32 * 57.295_776,
            velocity.y.atan2(len) as f32 * 57.295_776,
        );
    }
}

impl ThrownItemEntity {
    /// Process a tick for projectile movement and collisions
    #[expect(clippy::too_many_lines)]
    pub async fn process_tick<'a>(&'a self, caller: &'a Arc<dyn EntityBase>, server: &'a Server) {
        let entity = self.get_entity();
        let world = entity.world.load();

        entity.update_last_pos();

        // Apply gravity and inertia
        let mut velocity = entity.velocity.load();
        velocity.y -= self.get_gravity();

        let inertia = if is_wind_charge(caller) {
            // `AbstractWindCharge.getInertia`/`getLiquidInertia`
            // (`AbstractWindCharge.java:132-140`) both return 1.0F, overriding
            // `AbstractHurtingProjectile`'s 0.95F/0.8F: a wind charge never decelerates.
            1.0
        } else if entity.touching_water.load(Ordering::Relaxed) {
            0.8
        } else {
            0.99
        };
        velocity = velocity.multiply(inertia, inertia, inertia);

        // Store velocity
        entity.velocity.store(velocity);

        // `Projectile.checkLeftOwner` runs before the projectile hit scan
        // (`Projectile.java:105-127`).
        check_left_owner(entity, self.owner_id, velocity).await;

        let start_pos = entity.pos.load();
        let delta = velocity;

        // Update position
        let new_pos = start_pos.add(&delta);
        entity.set_pos(new_pos);

        // `Projectile.updateRotation` follows movement for throwable projectiles
        // (`Projectile.java:326-343`).
        update_rotation(entity, velocity);

        // Send updated velocity to clients
        let packet = CEntityVelocity::new(entity.entity_id.into(), velocity);
        let chunk_pos = entity.chunk_pos.load();
        world.broadcast_to_chunk(chunk_pos, &packet);

        // Calculate search box for collisions
        let search_box = BoundingBox::new(
            Vector3::new(
                start_pos.x.min(new_pos.x),
                start_pos.y.min(new_pos.y),
                start_pos.z.min(new_pos.z),
            ),
            Vector3::new(
                start_pos.x.max(new_pos.x),
                start_pos.y.max(new_pos.y),
                start_pos.z.max(new_pos.z),
            ),
        )
        .expand(0.3, 0.3, 0.3);

        let mut closest_t = 1.0f64;
        let mut hit = None;

        // Block collisions
        let (block_cols, block_positions) = world
            .get_block_collisions(search_box, caller.as_ref())
            .await;
        for (idx, bb) in block_cols.iter().enumerate() {
            if let Some(t) = calculate_ray_intersection(&start_pos, &delta, bb)
                && t < closest_t
            {
                closest_t = t;
                // Map back to block pos
                let mut curr = 0;
                for (len, pos) in &block_positions {
                    curr += len;
                    if idx < curr {
                        let hit_pos = start_pos.add(&delta.multiply(t, t, t));
                        hit = Some(ProjectileHit::Block {
                            pos: *pos,
                            face: get_hit_face(hit_pos, *pos),
                            hit_pos,
                            normal: delta.normalize().multiply(-1.0, -1.0, -1.0),
                        });
                        break;
                    }
                }
            }
        }

        // Entity collisions
        let candidates = world.get_all_at_box(&search_box);
        for cand in candidates {
            if self.should_skip_collision(caller, entity, &cand) {
                continue;
            }

            // `ProjectileUtil.getEntityHitResult` inflates the target by its pick radius
            // (`ProjectileUtil.java:109-120`).
            let pick_radius = projectile_target_pick_radius(cand.as_ref());
            let ebb =
                cand.get_entity()
                    .bounding_box
                    .load()
                    .expand(pick_radius, pick_radius, pick_radius);
            if let Some(t) = calculate_ray_intersection(&start_pos, &delta, &ebb)
                && t < closest_t
            {
                closest_t = t;
                let hit_pos = start_pos.add(&delta.multiply(t, t, t));
                hit = Some(ProjectileHit::Entity {
                    entity: cand.clone(),
                    hit_pos,
                    normal: delta.normalize().multiply(-1.0, -1.0, -1.0),
                });
            }
        }

        // Handle hit or continue
        if let Some(h) = hit {
            // `Projectile.hitTargetOrDeflectSelf`: a deflected projectile is not consumed.
            if try_deflect(&h, caller) {
                return;
            }

            // Ensure hit is only processed once per projectile
            if self.has_hit.swap(true, Ordering::SeqCst) {
                return;
            }

            // Just trigger hit effects and remove
            let land_pos = projectile_land_pos(&h);
            if let ProjectileHit::Block {
                pos, face, hit_pos, ..
            } = &h
            {
                on_projectile_block_hit(&world, server, caller, *pos, *face, *hit_pos).await;
            }
            caller.on_hit(h).await;
            emit_projectile_land(&world, caller, land_pos).await;

            entity.remove().await;
        }
    }

    /// Returns if collision should be skipped (e.g. owner or projectile vs projectile)
    fn should_skip_collision(
        &self,
        caller: &Arc<dyn EntityBase>,
        self_ent: &Entity,
        other: &Arc<dyn EntityBase>,
    ) -> bool {
        let other_ent = other.get_entity();

        // `AbstractWindCharge.canHitEntity` (`AbstractWindCharge.java:69-75`) rejects end
        // crystals outright. The other half of that override - never hitting another wind
        // charge - is already covered by the `is_projectile` skip below, since
        // `collides_with_projectiles` is false at every wind-charge construction site.
        if *other_ent.entity_type == EntityType::END_CRYSTAL && is_wind_charge(caller) {
            return true;
        }
        if other_ent.entity_id == self_ent.entity_id {
            return true;
        }

        if !other.can_be_hit_by_projectile() {
            return true;
        }

        // `Projectile.canHitEntity` excludes the owner until `leftOwner` is set
        // (`Projectile.java:317-324`).
        if Some(other_ent.entity_id) == self.owner_id
            && !self_ent.projectile_left_owner.load(Ordering::Relaxed)
        {
            return true;
        }

        // The ender dragon body is never a projectile target in vanilla:
        // `EnderDragon.isPickable()` is false (EnderDragon.java:758-761), so
        // `Entity.canBeHitByProjectile` (Entity.java:2005-2007) rejects it. The `age < 5`
        // owner skip above is tuned for player-sized owners and expires while a
        // `DragonFireball` is still inside the dragon's own 16-wide bounding box.
        if Some(other_ent.entity_id) == self.owner_id
            && other
                .cast_any()
                .downcast_ref::<crate::entity::boss::ender_dragon::EnderDragonEntity>()
                .is_some()
        {
            return true;
        }

        // An ender dragon's own body parts are separate world entities here, each carrying
        // the *dragon's* 16x8 bounding box, so a `DragonFireball` spawned at the head
        // (`DragonStrafePlayerPhase.java:64-77`) would collide with them on its first tick.
        // Vanilla never sees this: `Level.getEntities` (Level.java:782-786) is the only place
        // parts enter a hit scan, and `EnderDragonPart.is` (EnderDragonPart.java:57-60) treats
        // a part as its parent mob. Skip parts belonging to this projectile's own shooter.
        if let Some(owner_id) = self.owner_id
            && let Some(part) = other
                .cast_any()
                .downcast_ref::<crate::entity::boss::ender_dragon::EnderDragonPart>()
        {
            let world = self_ent.world.load();
            if world
                .get_entity_by_id(owner_id)
                .is_some_and(|owner| owner.get_entity().entity_uuid == part.dragon_uuid)
            {
                return true;
            }
        }

        // Projectiles should pass through lingering clouds
        if *other_ent.entity_type == EntityType::AREA_EFFECT_CLOUD {
            return true;
        }

        // Projectile vs projectile logic
        if !self.collides_with_projectiles && is_projectile(other_ent.entity_type) {
            return true;
        }

        false
    }

    const fn get_entity(&self) -> &Entity {
        &self.entity
    }

    #[allow(dead_code, clippy::unused_self)]
    const fn get_living_entity(&self) -> Option<&LivingEntity> {
        None
    }
    const fn get_gravity(&self) -> f64 {
        self.gravity
    }
}

/// Vanilla `Projectile.updateRotation` uses a wrapped angular interpolation before setting the
/// projectile rotation (`Projectile.java:326-343`).
fn update_rotation(entity: &Entity, movement: Vector3<f64>) {
    let horizontal = movement.horizontal_length();
    let target_pitch = movement.y.atan2(horizontal).to_degrees() as f32;
    let target_yaw = movement.x.atan2(movement.z).to_degrees() as f32;
    let pitch = lerp_rotation(entity.pitch.load(), target_pitch);
    let yaw = lerp_rotation(entity.yaw.load(), target_yaw);
    entity.set_rotation(yaw, pitch);
}

fn lerp_rotation(mut old: f32, rotation: f32) -> f32 {
    while rotation - old < -180.0 {
        old -= 360.0;
    }
    while rotation - old >= 180.0 {
        old += 360.0;
    }
    old + (rotation - old) * 0.2
}

/// Ray intersection algorithm for AABBs, returning a t value
fn calculate_ray_intersection(
    start: &Vector3<f64>,
    dir: &Vector3<f64>,
    bb: &BoundingBox,
) -> Option<f64> {
    let mut t_min = 0.0f64;
    let mut t_max = 1.0f64;

    let b_min = [bb.min.x, bb.min.y, bb.min.z];
    let b_max = [bb.max.x, bb.max.y, bb.max.z];
    let s = [start.x, start.y, start.z];
    let d = [dir.x, dir.y, dir.z];

    for i in 0..3 {
        if d[i].abs() < 1e-9 {
            if s[i] < b_min[i] || s[i] > b_max[i] {
                return None;
            }
        } else {
            let t1 = (b_min[i] - s[i]) / d[i];
            let t2 = (b_max[i] - s[i]) / d[i];
            t_min = t_min.max(t1.min(t2));
            t_max = t_max.min(t1.max(t2));
        }
    }

    (0.0..=1.0).contains(&t_min).then_some(t_min)
}

/// Get the face of the block that was hit
fn get_hit_face(hit_pos: Vector3<f64>, block_pos: BlockPos) -> BlockDirection {
    let local = hit_pos.sub(&block_pos.0.to_f64());
    let eps = 1.0e-4;

    if local.x <= eps {
        BlockDirection::West
    } else if local.x >= 1.0 - eps {
        BlockDirection::East
    } else if local.y <= eps {
        BlockDirection::Down
    } else if local.y >= 1.0 - eps {
        BlockDirection::Up
    } else if local.z <= eps {
        BlockDirection::North
    } else {
        BlockDirection::South
    }
}

pub enum ProjectileHit {
    Block {
        pos: BlockPos,
        face: BlockDirection,
        hit_pos: Vector3<f64>,
        normal: Vector3<f64>,
    },
    Entity {
        entity: Arc<dyn EntityBase>,
        hit_pos: Vector3<f64>,
        normal: Vector3<f64>,
    },
}

impl ProjectileHit {
    /// Returns the exact impact coordinates regardless of what was hit.
    #[must_use]
    pub const fn hit_pos(&self) -> Vector3<f64> {
        match self {
            Self::Block { hit_pos, .. } | Self::Entity { hit_pos, .. } => *hit_pos,
        }
    }

    /// Returns the surface normal of the impact.
    #[must_use]
    pub const fn normal(&self) -> Vector3<f64> {
        match self {
            Self::Block { normal, .. } | Self::Entity { normal, .. } => *normal,
        }
    }

    /// Safely returns the face hit if it was a block, otherwise None.
    #[must_use]
    pub const fn face(&self) -> Option<BlockDirection> {
        match self {
            Self::Block { face, .. } => Some(*face),
            Self::Entity { .. } => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{add_known_movement, lerp_rotation};
    use pumpkin_util::math::vector3::Vector3;

    /// `Projectile.shootFromRotation` keeps horizontal source motion in both cases, but
    /// only keeps vertical source motion while airborne (`Projectile.java:157-159`).
    #[test]
    fn known_source_motion_is_added_with_grounded_vertical_gate() {
        let projectile = Vector3::new(1.0, 2.0, 3.0);
        let source = Vector3::new(0.25, 0.5, -0.75);

        assert_eq!(
            add_known_movement(projectile, source, true),
            Vector3::new(1.25, 2.0, 2.25)
        );
        assert_eq!(
            add_known_movement(projectile, source, false),
            Vector3::new(1.25, 2.5, 2.25)
        );
    }

    #[test]
    fn update_rotation_interpolates_and_wraps_angles() {
        // `Projectile.lerpRotation` wraps before applying the 0.2 interpolation
        // (`Projectile.java:333-343`).
        assert_eq!(lerp_rotation(0.0, 90.0), 18.0);
        assert_eq!(lerp_rotation(170.0, -170.0), -186.0);
    }
}
