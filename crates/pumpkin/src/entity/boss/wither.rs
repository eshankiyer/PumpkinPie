// Legacy invariant checks retained for vanilla behavior; migrate these paths before removing this allow.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use std::sync::Mutex;
use std::sync::{
    Arc, Weak,
    atomic::{AtomicI32, Ordering},
};
use uuid::Uuid;

use pumpkin_data::{
    Block,
    damage::DamageType,
    entity::EntityType,
    item::Item,
    item_stack::ItemStack,
    tag::{self, Taggable},
    tracked_data,
    world::WorldEvent,
};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_protocol::{codec::var_int::VarInt, java::client::play::Metadata};
use pumpkin_util::{
    Difficulty,
    math::{position::BlockPos, vector3::Vector3},
    text::TextComponent,
};
use pumpkin_world::world::BlockFlags;
use rand::RngExt;

use crate::{
    entity::{
        Entity, EntityBase, NBTStorage,
        ai::control::flying_move_control::FlyingMoveControl,
        ai::goal::{
            Controls, Goal, look_around::RandomLookAroundGoal, look_at_entity::LookAtEntityGoal,
            revenge::RevengeGoal, track_target::TrackTargetGoal,
        },
        ai::pathfinder::NavigatorGoal,
        ai::target_predicate::TargetPredicate,
        item::ItemEntity,
        living::LivingEntity,
        mob::{Mob, MobEntity},
        projectile::wither_skull::WitherSkullEntity,
    },
    world::{
        ExplosionInteraction, World,
        bossbar::{Bossbar, BossbarColor, BossbarDivisions, BossbarFlags},
    },
};

/// `WitherBoss.INVULNERABLE_TICKS`.
const INVULNERABLE_TICKS: i32 = 220;

pub struct WitherEntity {
    pub mob_entity: MobEntity,
    /// `WitherBoss.DATA_ID_INV`.
    invulnerable_ticks: AtomicI32,
    /// `WitherBoss.destroyBlocksTick`.
    pub destroy_blocks_tick: AtomicI32,
    /// `WitherBoss.nextHeadUpdate`.
    pub next_head_update: [AtomicI32; 2],
    /// `WitherBoss.idleHeadUpdates`.
    pub idle_head_updates: [AtomicI32; 2],
    /// `WitherBoss.DATA_TARGET_A`/`B`/`C`.
    pub alternative_targets: [AtomicI32; 3],
    /// `WitherBoss.bossEvent` id.
    pub bossbar_uuid: Uuid,
    /// Players currently shown `bossEvent`.
    pub bossbar_players: Mutex<Vec<Uuid>>,
}

impl WitherEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let mob_entity = MobEntity::new(entity);
        // `WitherBoss` installs `new FlyingMoveControl<>(this, 10, false)` in its
        // constructor. The controller is responsible for FLYING_SPEED and vertical input.
        *mob_entity.move_control.lock().unwrap() = Box::new(FlyingMoveControl::new(10.0, false));
        {
            let mut navigator = mob_entity.navigator.lock().unwrap();
            navigator.set_flying(true);
            navigator.set_can_float(true);
            navigator.set_can_open_doors(false);
        };
        let wither = Self {
            mob_entity,
            invulnerable_ticks: AtomicI32::new(0),
            destroy_blocks_tick: AtomicI32::new(0),
            next_head_update: [AtomicI32::new(0), AtomicI32::new(0)],
            idle_head_updates: [AtomicI32::new(0), AtomicI32::new(0)],
            alternative_targets: [AtomicI32::new(0), AtomicI32::new(0), AtomicI32::new(0)],
            bossbar_uuid: Uuid::new_v4(),
            bossbar_players: Mutex::new(Vec::new()),
        };
        let mob_arc = Arc::new(wither);
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
            let mut target_selector = mob_arc
                .mob_entity
                .target_selector
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);

            // `WitherBoss.registerGoals` (`boss/wither/WitherBoss.java:98-107`).
            goal_selector.add_goal(0, Box::new(WitherDoNothingGoal));
            goal_selector.add_goal(2, Box::new(WitherRangedAttackGoal::new()));
            goal_selector.add_goal(5, Box::new(WitherRandomFlightGoal::new()));
            goal_selector.add_goal(
                6,
                LookAtEntityGoal::with_default(mob_weak, &EntityType::PLAYER, 8.0),
            );
            goal_selector.add_goal(7, Box::new(RandomLookAroundGoal::default()));

            // `HurtByTargetGoal(this)` -> `TargetGoal(mob, true)`: must see the attacker.
            target_selector.add_goal(1, Box::new(RevengeGoal::new(true)));
            target_selector.add_goal(2, Box::new(WitherNearestTargetGoal::new()));
        };

        mob_arc
    }

    #[must_use]
    pub fn invulnerable_ticks(&self) -> i32 {
        self.invulnerable_ticks.load(Ordering::Relaxed)
    }

    /// Upstream-named alias of [`Self::invulnerable_ticks`].
    #[must_use]
    pub fn get_invulnerable_ticks(&self) -> i32 {
        self.invulnerable_ticks()
    }

    /// `WitherBoss.setInvulnerableTicks`: updates the synced `DATA_ID_INV`.
    pub fn set_invulnerable_ticks(&self, ticks: i32) {
        let ticks = ticks.max(0);
        self.invulnerable_ticks.store(ticks, Ordering::Relaxed);
        self.mob_entity.living_entity.entity.send_meta_data(
            &[Metadata::new(
                tracked_data::wither::DATA_ID_INV,
                VarInt(ticks),
            )],
            None,
        );
    }

    /// `WitherBoss.getAlternativeTarget`.
    #[must_use]
    pub fn get_alternative_target(&self, head: usize) -> i32 {
        if head < 3 {
            self.alternative_targets[head].load(Ordering::Relaxed)
        } else {
            0
        }
    }

    /// `WitherBoss.setAlternativeTarget`: updates the synced `DATA_TARGET_A`/`B`/`C`.
    pub fn set_alternative_target(&self, head: usize, entity_id: i32) {
        if head < 3 {
            let old = self.alternative_targets[head].swap(entity_id, Ordering::Relaxed);
            if old != entity_id {
                let tracker_id = match head {
                    0 => tracked_data::wither::DATA_TARGET_A,
                    1 => tracked_data::wither::DATA_TARGET_B,
                    _ => tracked_data::wither::DATA_TARGET_C,
                };
                self.mob_entity
                    .living_entity
                    .entity
                    .send_meta_data(&[Metadata::new(tracker_id, VarInt(entity_id))], None);
            }
        }
    }

    /// `WitherBoss.isPowered`.
    #[must_use]
    pub fn is_powered(&self) -> bool {
        let living = &self.mob_entity.living_entity;
        living.health.load() <= living.get_max_health() / 2.0
    }

    /// `WitherBoss.makeInvulnerable` (`WitherBoss.java:350-354`).
    pub fn make_invulnerable(&self) {
        self.set_invulnerable_ticks(INVULNERABLE_TICKS);
        self.mob_entity
            .living_entity
            .set_health(self.mob_entity.living_entity.get_max_health() / 3.0);
    }

    /// `WitherBoss.canDestroy`: `!state.isAir() && !state.is(BlockTags.WITHER_IMMUNE)`
    /// (air, cave air and void air all count as air).
    #[must_use]
    pub fn can_destroy(block: &Block) -> bool {
        !block.is_air() && !block.has_tag(&tag::Block::MINECRAFT_WITHER_IMMUNE)
    }

    /// `WitherBoss.getHeadX` (`WitherBoss.java:372-381`), at scale 1.
    #[must_use]
    pub fn get_head_x(&self, head: usize) -> f64 {
        let entity = &self.mob_entity.living_entity.entity;
        if head == 0 {
            return entity.pos.load().x;
        }
        let angle = (entity.body_yaw.load() + 180.0 * (head as f32 - 1.0)).to_radians();
        entity.pos.load().x + f64::from(angle.cos()) * 1.3
    }

    /// `WitherBoss.getHeadY` (`WitherBoss.java:383-386`), at scale 1.
    #[must_use]
    pub fn get_head_y(&self, head: usize) -> f64 {
        let base_y = self.mob_entity.living_entity.entity.pos.load().y;
        if head == 0 {
            base_y + 3.0
        } else {
            base_y + 2.2
        }
    }

    /// `WitherBoss.getHeadZ` (`WitherBoss.java:388-397`), at scale 1.
    #[must_use]
    pub fn get_head_z(&self, head: usize) -> f64 {
        let entity = &self.mob_entity.living_entity.entity;
        if head == 0 {
            return entity.pos.load().z;
        }
        let angle = (entity.body_yaw.load() + 180.0 * (head as f32 - 1.0)).to_radians();
        entity.pos.load().z + f64::from(angle.sin()) * 1.3
    }

    /// `WitherBoss.performRangedAttack(int, LivingEntity)` (`WitherBoss.java:413-415`): aims at
    /// the target's mid-body; only the main head rolls the 0.1% dangerous skull.
    pub fn perform_ranged_attack_at(&self, head: usize, target: &dyn EntityBase) {
        let target_entity = target.get_entity();
        let target_pos = target_entity.pos.load();
        let dangerous = head == 0 && self.get_random().random::<f32>() < 0.001;
        self.perform_ranged_attack(
            head,
            target_pos.x,
            target_pos.y + target_entity.get_eye_height() * 0.5,
            target_pos.z,
            dangerous,
        );
    }

    /// `WitherBoss.performRangedAttack(int, double, double, double, boolean)`
    /// (`WitherBoss.java:417-442`).
    pub fn perform_ranged_attack(
        &self,
        head: usize,
        target_x: f64,
        target_y: f64,
        target_z: f64,
        dangerous: bool,
    ) {
        let entity = &self.mob_entity.living_entity.entity;
        let world = entity.world.load_full();

        if !entity.is_silent() {
            world.sync_world_event(WorldEvent::SoundWitherBossShoot, entity.block_pos.load(), 0);
        }

        let head_pos = Vector3::new(
            self.get_head_x(head),
            self.get_head_y(head),
            self.get_head_z(head),
        );
        let direction = Vector3::new(
            target_x - head_pos.x,
            target_y - head_pos.y,
            target_z - head_pos.z,
        );

        let skull_entity = Entity::new(world.clone(), head_pos, &EntityType::WITHER_SKULL);
        let skull = WitherSkullEntity::new_shot(skull_entity, entity, dangerous, direction);
        world.spawn_entity(Arc::new(skull));
    }

    fn make_bossbar(&self) -> Bossbar {
        let title = self
            .mob_entity
            .living_entity
            .entity
            .custom_name
            .load()
            .as_ref()
            .clone()
            .unwrap_or_else(|| {
                TextComponent::translate_cross(
                    "entity.minecraft.wither",
                    "entity.minecraft.wither",
                    [],
                )
            });

        // `ServerBossEvent(..., PURPLE, PROGRESS)` with `setDarkenScreen(true)`.
        Bossbar {
            uuid: self.bossbar_uuid,
            title,
            health: 1.0,
            color: BossbarColor::Purple,
            division: BossbarDivisions::NoDivision,
            flags: BossbarFlags::DARKEN_SKY,
        }
    }

    /// `bossEvent.setProgress`, plus the `startSeenByPlayer`/`stopSeenByPlayer` membership,
    /// approximated by a 50-block radius around the wither.
    fn update_bossbar(&self, world: &Arc<World>, progress: f32) {
        let pos = self.mob_entity.living_entity.entity.pos.load();
        let tracking_radius_sq = 50.0 * 50.0;
        let players = world.players.load();

        let current: Vec<Uuid> = players
            .iter()
            .filter(|p| {
                let p_pos = p.living_entity.entity.pos.load();
                (p_pos - pos).length_squared() < tracking_radius_sq
            })
            .map(|p| p.gameprofile.id)
            .collect();

        let mut bossbar_players = self
            .bossbar_players
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        for &uid in &current {
            if !bossbar_players.contains(&uid) {
                if let Some(p) = players.iter().find(|p| p.gameprofile.id == uid) {
                    let mut bar = self.make_bossbar();
                    bar.health = progress;
                    p.send_bossbar(&bar);
                }
                bossbar_players.push(uid);
            }
        }

        let to_remove: Vec<Uuid> = bossbar_players
            .iter()
            .filter(|uid| !current.contains(uid))
            .copied()
            .collect();

        for uid in &to_remove {
            if let Some(p) = players.iter().find(|p| &p.gameprofile.id == uid) {
                p.remove_bossbar(self.bossbar_uuid);
            }
            bossbar_players.retain(|u| u != uid);
        }

        for player in players.iter() {
            if bossbar_players.contains(&player.gameprofile.id) {
                player.update_bossbar_health(&self.bossbar_uuid, progress);
            }
        }
    }

    fn remove_all_bossbar(&self, world: &Arc<World>) {
        let mut bossbar_players = self
            .bossbar_players
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let players = world.players.load();
        for player in players.iter() {
            if bossbar_players.contains(&player.gameprofile.id) {
                player.remove_bossbar(self.bossbar_uuid);
            }
        }
        bossbar_players.clear();
    }

    /// The side-head block of `WitherBoss.customServerAiStep` (`WitherBoss.java:268-298`).
    fn tick_side_heads(&self, world: &Arc<World>, tick_count: i32) {
        let entity = &self.mob_entity.living_entity.entity;
        let difficulty = world.level_info.load().difficulty;

        for i in 1..3 {
            if tick_count < self.next_head_update[i - 1].load(Ordering::Relaxed) {
                continue;
            }
            let delay = self.get_random().random_range(0..10);
            self.next_head_update[i - 1].store(tick_count + 10 + delay, Ordering::Relaxed);

            if (difficulty == Difficulty::Normal || difficulty == Difficulty::Hard)
                && self.idle_head_updates[i - 1].fetch_add(1, Ordering::Relaxed) > 15
            {
                let pos = entity.pos.load();
                let (xt, yt, zt) = {
                    let mut rng = self.get_random();
                    (
                        rng.random_range(pos.x - 10.0..pos.x + 10.0),
                        rng.random_range(pos.y - 5.0..pos.y + 5.0),
                        rng.random_range(pos.z - 10.0..pos.z + 10.0),
                    )
                };
                // Vanilla fires side head `i` from head index `i + 1`.
                self.perform_ranged_attack(i + 1, xt, yt, zt, true);
                self.idle_head_updates[i - 1].store(0, Ordering::Relaxed);
            }

            let head_target = self.get_alternative_target(i);
            if head_target > 0 {
                let current = world.get_entity_by_id(head_target);
                if let Some(current) = current
                    && current
                        .get_living_entity()
                        .is_some_and(LivingEntity::can_be_seen_as_enemy)
                    && self.can_attack(current.get_entity())
                    && entity
                        .pos
                        .load()
                        .squared_distance_to_vec(&current.get_entity().pos.load())
                        <= 900.0
                    && self.mob_entity.has_line_of_sight(current.as_ref())
                {
                    self.perform_ranged_attack_at(i + 1, current.as_ref());
                    let delay = self.get_random().random_range(0..20);
                    self.next_head_update[i - 1].store(tick_count + 40 + delay, Ordering::Relaxed);
                    self.idle_head_updates[i - 1].store(0, Ordering::Relaxed);
                } else {
                    self.set_alternative_target(i, 0);
                }
            } else {
                // `level.getNearbyEntities(LivingEntity.class, TARGETING_CONDITIONS, this,
                // getBoundingBox().inflate(20.0, 8.0, 20.0))`.
                let predicate = TargetPredicate::create_attackable().set_base_max_distance(20.0);
                let search_box = entity.bounding_box.load().expand(20.0, 8.0, 20.0);
                let mut candidates = Vec::new();
                for candidate in world.get_entities_at_box(&search_box) {
                    if candidate.get_entity().entity_id == entity.entity_id
                        || candidate
                            .get_entity()
                            .entity_type
                            .has_tag(&tag::EntityType::MINECRAFT_WITHER_FRIENDS)
                    {
                        continue;
                    }
                    let Some(living) = candidate.get_living_entity() else {
                        continue;
                    };
                    if living.is_valid_ai_target()
                        && predicate.test(world, Some(&self.mob_entity.living_entity), living)
                    {
                        candidates.push(candidate);
                    }
                }
                if !candidates.is_empty() {
                    let idx = self.get_random().random_range(0..candidates.len());
                    self.set_alternative_target(i, candidates[idx].get_entity().entity_id);
                }
            }
        }
    }

    /// The `destroyBlocksTick` block of `WitherBoss.customServerAiStep`
    /// (`WitherBoss.java:306-331`).
    fn tick_destroy_blocks(&self, world: &Arc<World>) {
        let destroy_tick = self.destroy_blocks_tick.load(Ordering::Relaxed);
        if destroy_tick <= 0 {
            return;
        }
        let next_destroy = destroy_tick - 1;
        self.destroy_blocks_tick
            .store(next_destroy, Ordering::Relaxed);
        if next_destroy != 0 || !world.level_info.load().game_rules.mob_griefing {
            return;
        }

        let entity = &self.mob_entity.living_entity.entity;
        let bb = entity.bounding_box.load();
        let width = ((bb.max.x - bb.min.x) as f32 / 2.0 + 1.0).floor() as i32;
        let height = ((bb.max.y - bb.min.y) as f32).floor() as i32;
        let origin = entity.block_pos.load();
        let mut destroyed = false;

        for dx in -width..=width {
            for dy in 0..=height {
                for dz in -width..=width {
                    let pos = BlockPos::new(origin.0.x + dx, origin.0.y + dy, origin.0.z + dz);
                    if Self::can_destroy(world.get_block(&pos)) {
                        // `level.destroyBlock(blockPos, true, this)`: drops the block.
                        destroyed |= world
                            .break_block(&pos, None, BlockFlags::NOTIFY_ALL)
                            .is_some();
                    }
                }
            }
        }

        if destroyed {
            world.sync_world_event(WorldEvent::SoundWitherBlockBreak, origin, 0);
        }
    }

    /// `WitherBoss.aiStep` (`WitherBoss.java:155-203`) follows its main target directly rather
    /// than relying on ground navigation. The generic living movement tick supplies collision
    /// handling and drag after this updates the velocity.
    fn ai_step_movement(&self) {
        let entity = &self.mob_entity.living_entity.entity;
        let Some(target) = self
            .mob_entity
            .target
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
        else {
            return;
        };
        if !target.get_entity().is_alive() {
            return;
        }

        let target_pos = target.get_entity().pos.load();
        let pos = entity.pos.load();
        let mut velocity = entity.velocity.load().multiply(1.0, 0.6, 1.0);
        if pos.y < target_pos.y || (!self.is_powered() && pos.y < target_pos.y + 5.0) {
            velocity.y = velocity.y.max(0.0);
            velocity.y += 0.3 - velocity.y * 0.6;
        }

        let horizontal = Vector3::new(target_pos.x - pos.x, 0.0, target_pos.z - pos.z);
        if horizontal.length_squared() > 9.0 {
            let direction = horizontal.normalize();
            velocity.x += direction.x * 0.3 - velocity.x * 0.6;
            velocity.z += direction.z * 0.3 - velocity.z * 0.6;
        }
        entity.set_velocity(velocity);
        if velocity.horizontal_length() > 0.05 {
            entity
                .yaw
                .store((velocity.z.atan2(velocity.x).to_degrees() - 90.0) as f32);
        }
    }
}

impl NBTStorage for WitherEntity {
    fn write_nbt<'a>(&'a self, nbt: &'a mut NbtCompound) -> crate::entity::NbtFuture<'a, ()> {
        Box::pin(async move {
            self.mob_entity.living_entity.write_nbt(nbt);
            nbt.put_int("Invul", self.invulnerable_ticks());
        })
    }

    fn read_nbt_non_mut<'a>(&'a self, nbt: &'a NbtCompound) -> crate::entity::NbtFuture<'a, ()> {
        Box::pin(async move {
            self.mob_entity.living_entity.read_nbt_non_mut(nbt);
            self.set_invulnerable_ticks(nbt.get_int("Invul").unwrap_or(0));
        })
    }
}

impl Mob for WitherEntity {
    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    /// `WitherBoss.defineSynchedData`: `DATA_TARGET_A`/`B`/`C` and `DATA_ID_INV`.
    fn mob_init_data_tracker(&self) {
        let entity = &self.mob_entity.living_entity.entity;
        entity.send_meta_data(
            &[
                Metadata::new(
                    tracked_data::wither::DATA_TARGET_A,
                    VarInt(self.get_alternative_target(0)),
                ),
                Metadata::new(
                    tracked_data::wither::DATA_TARGET_B,
                    VarInt(self.get_alternative_target(1)),
                ),
                Metadata::new(
                    tracked_data::wither::DATA_TARGET_C,
                    VarInt(self.get_alternative_target(2)),
                ),
                Metadata::new(
                    tracked_data::wither::DATA_ID_INV,
                    VarInt(self.invulnerable_ticks()),
                ),
            ],
            None,
        );
    }

    /// Vanilla `WitherBoss.checkDespawn` only removes the wither in Peaceful;
    /// otherwise it resets `noActionTime` instead of applying distance despawn.
    fn check_despawn(&self) {
        let entity = self.get_entity();
        if entity.is_removed() {
            return;
        }

        let world = entity.world.load_full();
        if world.level_info.load().difficulty == Difficulty::Peaceful
            && !entity.entity_type.allowed_in_peaceful
        {
            // `stopSeenByPlayer` on discard removes every viewer from `bossEvent`.
            self.remove_all_bossbar(&world);
            entity.remove();
        } else {
            self.mob_entity.no_action_time.store(0, Ordering::Relaxed);
        }
    }

    fn can_attack(&self, target: &Entity) -> bool {
        !target
            .entity_type
            .has_tag(&tag::EntityType::MINECRAFT_WITHER_FRIENDS)
    }

    fn get_mob_gravity(&self) -> f64 {
        0.0
    }

    /// `WitherBoss.hurtServer` (`WitherBoss.java:450-484`) ignores damage while the summon
    /// countdown is active. The wither-friends tag is also used for the boss's blanket immunity
    /// to friendly undead.
    fn pre_damage(&self, damage_type: DamageType, source: Option<&dyn EntityBase>) -> bool {
        if damage_type.has_tag(&tag::DamageType::MINECRAFT_WITHER_IMMUNE_TO)
            || source.is_some_and(|source| source.get_entity().entity_type == &EntityType::WITHER)
            || (self.invulnerable_ticks() > 0
                && !damage_type.has_tag(&tag::DamageType::MINECRAFT_BYPASSES_INVULNERABILITY))
        {
            return false;
        }

        if self.is_powered()
            && source.is_some_and(|source| {
                let source_type = source.get_entity().entity_type;
                source_type == &EntityType::ARROW
                    || source_type == &EntityType::SPECTRAL_ARROW
                    || source_type == &EntityType::WIND_CHARGE
                    || source_type == &EntityType::BREEZE_WIND_CHARGE
            })
        {
            return false;
        }

        !source.is_some_and(|source| {
            source
                .get_entity()
                .entity_type
                .has_tag(&tag::EntityType::MINECRAFT_WITHER_FRIENDS)
        })
    }

    /// The tail of `WitherBoss.hurtServer` (`WitherBoss.java:476-482`), reached once the
    /// immunity checks in [`Self::pre_damage`] have passed.
    fn on_damage(&self, _damage_type: DamageType, _source: Option<&dyn EntityBase>) {
        if self.destroy_blocks_tick.load(Ordering::Relaxed) <= 0 {
            self.destroy_blocks_tick.store(20, Ordering::Relaxed);
        }

        for idle in &self.idle_head_updates {
            idle.fetch_add(3, Ordering::Relaxed);
        }
    }

    /// `WitherBoss.dropCustomDeathLoot` (`WitherBoss.java:486-493`): a Nether Star with an
    /// extended lifetime, gated like the rest of `dropAllDeathLoot` by `mob_drops`. The boss
    /// bar is also torn down here, as the dying wither stops being tracked.
    fn on_mob_death(&self, _cause: Option<&dyn EntityBase>) {
        let entity = &self.mob_entity.living_entity.entity;
        let world = entity.world.load_full();
        self.remove_all_bossbar(&world);
        if world.level_info.load().game_rules.mob_drops {
            let item_entity = Entity::new(world.clone(), entity.pos.load(), &EntityType::ITEM);
            let nether_star = Arc::new(ItemEntity::new(
                item_entity,
                ItemStack::new(1, &Item::NETHER_STAR),
            ));
            nether_star.set_extended_lifetime();
            world.spawn_entity(nether_star);
        }
    }

    /// `WitherBoss.customServerAiStep` (`WitherBoss.java:240-340`) followed by the movement half
    /// of `WitherBoss.aiStep`.
    fn mob_tick(&self, _caller: &Arc<dyn EntityBase>) {
        let entity = &self.mob_entity.living_entity.entity;
        let world = entity.world.load_full();
        let age = entity.age.load(Ordering::Relaxed);
        let invulnerable = self.invulnerable_ticks();
        if invulnerable > 0 {
            let next = invulnerable - 1;
            self.update_bossbar(&world, 1.0 - next as f32 / INVULNERABLE_TICKS as f32);
            if next <= 0 {
                world.explode(entity.get_eye_pos(), 7.0, ExplosionInteraction::Mob);
                if !entity.is_silent() {
                    // Vanilla uses `globalLevelEvent(1023, ...)`.
                    world.sync_world_event(
                        WorldEvent::SoundWitherBossSpawn,
                        entity.block_pos.load(),
                        0,
                    );
                }
            }
            self.set_invulnerable_ticks(next);
            if age % 10 == 0 {
                self.mob_entity.living_entity.heal(10.0);
            }
            return;
        }

        self.tick_side_heads(&world, age);

        let main_target = self
            .mob_entity
            .target
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .map_or(0, |target| target.get_entity().entity_id);
        self.set_alternative_target(0, main_target);

        self.tick_destroy_blocks(&world);

        if age % 20 == 0 {
            self.mob_entity.living_entity.heal(1.0);
        }

        let living = &self.mob_entity.living_entity;
        let max_health = living.get_max_health();
        let progress = if max_health > 0.0 {
            (living.health.load() / max_health).clamp(0.0, 1.0)
        } else {
            0.0
        };
        self.update_bossbar(&world, progress);

        self.ai_step_movement();
    }
}

/// `WitherBoss.WitherDoNothingGoal` (`WitherBoss.java:586-599`).
struct WitherDoNothingGoal;

impl Goal for WitherDoNothingGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        mob.cast_any()
            .downcast_ref::<WitherEntity>()
            .is_some_and(|wither| wither.invulnerable_ticks() > 0)
    }

    fn controls(&self) -> Controls {
        Controls::MOVE | Controls::LOOK | Controls::JUMP
    }
}

/// `RangedAttackGoal(this, 1.0, 40, 20.0F)` from `WitherBoss.registerGoals`.
struct WitherRangedAttackGoal {
    target: Option<Arc<dyn EntityBase>>,
    attack_time: i32,
    see_time: i32,
}

impl WitherRangedAttackGoal {
    const fn new() -> Self {
        Self {
            target: None,
            attack_time: -1,
            see_time: 0,
        }
    }

    fn has_line_of_sight(mob: &dyn Mob, target: &dyn EntityBase) -> bool {
        mob.get_mob_entity().has_line_of_sight(target)
    }

    /// `WitherBoss.performRangedAttack(LivingEntity, float)` (`WitherBoss.java:444-447`):
    /// always fires from the main head (index 0).
    fn shoot(mob: &dyn Mob, target: &dyn EntityBase) {
        if let Some(wither) = mob.cast_any().downcast_ref::<WitherEntity>() {
            wither.perform_ranged_attack_at(0, target);
        }
    }
}

impl Goal for WitherRangedAttackGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        let target = mob
            .get_mob_entity()
            .target
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        if target
            .as_ref()
            .is_some_and(|target| target.get_entity().is_alive())
        {
            self.target = target;
            true
        } else {
            false
        }
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        if let Some(target) = mob
            .get_mob_entity()
            .target
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
            && target.get_entity().is_alive()
        {
            self.target = Some(target);
            return true;
        }

        self.target.as_ref().is_some_and(|target| {
            target.get_entity().is_alive()
                && !mob.get_mob_entity().navigator.lock().unwrap().is_idle()
        })
    }

    fn start(&mut self, mob: &dyn Mob) {
        self.attack_time = -1;
        self.see_time = 0;
        mob.get_mob_entity().set_attacking(true);
    }

    fn stop(&mut self, mob: &dyn Mob) {
        self.target = None;
        self.see_time = 0;
        self.attack_time = -1;
        mob.get_mob_entity().set_attacking(false);
    }

    fn tick(&mut self, mob: &dyn Mob) {
        let Some(target) = self.target.clone() else {
            return;
        };
        let entity = mob.get_entity();
        let target_pos = target.get_entity().pos.load();
        let distance_squared = entity.pos.load().squared_distance_to_vec(&target_pos);
        let has_line_of_sight = Self::has_line_of_sight(mob, target.as_ref());
        if has_line_of_sight {
            self.see_time += 1;
        } else {
            self.see_time = 0;
        }

        if distance_squared > 400.0 || self.see_time < 5 {
            mob.get_mob_entity()
                .navigator
                .lock()
                .unwrap()
                .set_progress(NavigatorGoal::new(entity.pos.load(), target_pos, 1.0));
        } else {
            mob.get_mob_entity().navigator.lock().unwrap().stop();
        }

        mob.get_mob_entity()
            .look_control
            .lock()
            .unwrap()
            .look_at_entity_with_range(&target, 30.0, 30.0);

        self.attack_time -= 1;
        if self.attack_time == 0 {
            if !has_line_of_sight {
                return;
            }
            let distance = distance_squared.sqrt() / 20.0;
            let _power = distance.clamp(0.1, 1.0);
            Self::shoot(mob, target.as_ref());
            self.attack_time = 40;
        } else if self.attack_time < 0 {
            self.attack_time = 40;
        }
    }

    fn should_run_every_tick(&self) -> bool {
        true
    }

    fn controls(&self) -> Controls {
        Controls::MOVE | Controls::LOOK
    }
}

/// The `WaterAvoidingRandomFlyingGoal` slot from `WitherBoss.registerGoals`.
/// Wither flight is driven by `WitherBoss.aiStep`; this goal supplies the idle drift when there
/// is no combat target instead of handing the flying mob to the ground navigator.
struct WitherRandomFlightGoal {
    cooldown: i32,
}

impl WitherRandomFlightGoal {
    const fn new() -> Self {
        Self { cooldown: 0 }
    }
}

impl Goal for WitherRandomFlightGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        mob.get_mob_entity()
            .target
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_none()
            && mob
                .cast_any()
                .downcast_ref::<WitherEntity>()
                .is_some_and(|wither| wither.invulnerable_ticks() == 0)
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        self.can_start(mob)
    }

    fn tick(&mut self, mob: &dyn Mob) {
        self.cooldown -= 1;
        if self.cooldown > 0 {
            return;
        }
        let mut rng = mob.get_random();
        let direction = Vector3::new(
            rng.random_range(-1.0..=1.0),
            rng.random_range(-0.5..=0.5),
            rng.random_range(-1.0..=1.0),
        )
        .normalize()
            * 0.1;
        let velocity = mob.get_entity().velocity.load().multiply(0.8, 0.8, 0.8) + direction;
        mob.get_entity().set_velocity(velocity);
        self.cooldown = rng.random_range(20..60);
    }

    fn should_run_every_tick(&self) -> bool {
        true
    }
}

/// `NearestAttackableTargetGoal<LivingEntity>` with the `WITHER_FRIENDS` exclusion from
/// `WitherBoss.LIVING_ENTITY_SELECTOR` (`WitherBoss.java:74-79`).
struct WitherNearestTargetGoal {
    tracker: TrackTargetGoal,
    target: Option<Arc<dyn EntityBase>>,
    target_predicate: TargetPredicate,
}

impl WitherNearestTargetGoal {
    fn new() -> Self {
        Self {
            tracker: TrackTargetGoal::with_default(false),
            target: None,
            target_predicate: TargetPredicate::create_attackable().set_base_max_distance(40.0),
        }
    }

    fn find_target(&mut self, mob: &dyn Mob) {
        let entity = mob.get_entity();
        let world = entity.world.load();
        let mut search_pos = entity.pos.load();
        search_pos.y += entity.get_eye_height();
        let mut candidates: Vec<Arc<dyn EntityBase>> = world
            .get_nearby_entities(search_pos, 40.0)
            .into_values()
            .filter(|candidate| {
                candidate.get_entity().entity_id != entity.entity_id
                    && !candidate
                        .get_entity()
                        .entity_type
                        .has_tag(&tag::EntityType::MINECRAFT_WITHER_FRIENDS)
                    && candidate
                        .get_living_entity()
                        .is_some_and(LivingEntity::is_valid_ai_target)
            })
            .collect();
        candidates.sort_by(|a, b| {
            let distance = |candidate: &Arc<dyn EntityBase>| {
                candidate
                    .get_entity()
                    .pos
                    .load()
                    .squared_distance_to_vec(&search_pos)
            };
            distance(a).partial_cmp(&distance(b)).unwrap()
        });

        self.target = None;
        for candidate in candidates {
            let Some(living) = candidate.get_living_entity() else {
                continue;
            };
            if !TrackTargetGoal::is_allied(mob, candidate.as_ref())
                && mob.can_attack(candidate.get_entity())
                && self.target_predicate.test(
                    &world,
                    Some(&mob.get_mob_entity().living_entity),
                    living,
                )
            {
                self.target = Some(candidate);
                break;
            }
        }
    }
}

impl Goal for WitherNearestTargetGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        self.find_target(mob);
        self.target.is_some()
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        self.tracker.should_continue(mob)
    }

    fn start(&mut self, mob: &dyn Mob) {
        mob.set_mob_target(self.target.clone());
        self.tracker.start(mob);
    }

    fn stop(&mut self, mob: &dyn Mob) {
        self.target = None;
        self.tracker.stop(mob);
    }

    fn controls(&self) -> Controls {
        Controls::TARGET
    }
}
