use std::sync::atomic::{AtomicBool, AtomicI32, AtomicI64, Ordering::Relaxed};
use std::sync::{Arc, Weak};

use pumpkin_data::{Block, BlockState};
use pumpkin_data::damage::DamageType;
use pumpkin_data::data_component_impl::EquipmentSlot;
use pumpkin_data::entity::{EntityPose, EntityType};
use pumpkin_data::game_event::GameEvent;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_data::tag::{self, Taggable};
use pumpkin_data::tracked_data;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_protocol::java::client::play::Metadata;
use pumpkin_util::math::boundingbox::BoundingBox;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_util::math::wrap_degrees;

use crate::entity::{
    Entity, EntityBase, EntityBaseFuture, NBTStorage, NbtFuture,
    ai::goal::{
        camel_sit::{CamelPanicGoal, CamelSitGoal, RefuseToMoveGate},
        look_around::RandomLookAroundGoal,
        look_at_entity::LookAtEntityGoal,
        swim::SwimGoal,
        wander_around::WanderAroundGoal,
    },
    mob::{Mob, MobEntity},
    passive::equine::{
        equip_saddle_item, is_valid_saddle_item, mount_player, saddle_equip_on_interact,
    },
    player::Player,
};
use crate::world::game_event::{GameEventContext, emit_game_event};

/// `Camel.DASH_COOLDOWN_TICKS` (`Camel.java:64`).
const DASH_COOLDOWN_TICKS: i32 = 55;
/// `Camel.java:187` (`isDashing() && dashCooldown < 50 && (onGround || isInLiquid || isPassenger)`).
const DASH_CLEAR_THRESHOLD: i32 = 50;
/// `Camel.java:397` (`this.getPassengers().size() < 2`): a camel seats two riders.
const MAX_PASSENGERS: usize = 2;
/// `Camel.SITDOWN_DURATION_TICKS` (`Camel.java:70`).
const SITDOWN_DURATION_TICKS: i64 = 40;
/// `Camel.STANDUP_DURATION_TICKS` (`Camel.java:71`).
const STANDUP_DURATION_TICKS: i64 = 52;
/// `Camel.getMaxHeadYRot` (`Camel.java:562-565`).
const MAX_HEAD_Y_ROT: f32 = 30.0;

/// `Camel.getPoseTime` (`Camel.java:630-632`): ticks since the last pose change, whichever sign
/// the synced tick carries.
const fn pose_time(game_time: i64, last_pose_change_tick: i64) -> i64 {
    game_time - last_pose_change_tick.abs()
}

/// `Camel.isInPoseTransition` (`Camel.java:580-583`).
const fn is_in_pose_transition_for(pose_time: i64, sitting: bool) -> bool {
    pose_time
        < if sitting {
            SITDOWN_DURATION_TICKS
        } else {
            STANDUP_DURATION_TICKS
        }
}

/// `Camel.resetLastPoseChangeTickToFullStand` (`Camel.java:626-628`).
fn full_stand_tick(game_time: i64) -> i64 {
    (game_time - STANDUP_DURATION_TICKS - 1).max(0)
}

/// Represents a Camel, a passive mount that can carry two players and dash.
///
/// Wiki: <https://minecraft.wiki/w/Camel>
///
/// The sit state is vanilla's single synced `LAST_POSE_CHANGE_TICK` LONG (`Camel.java:79`): its
/// sign is the state and its magnitude the game time of the last change. A player-ridden camel
/// is client-authoritative, so the dash impulse itself never runs on the server; the server half
/// is `START_RIDING_JUMP` -> `canJump`/`handleStartJump` (`Camel.java:287-290,327-332`), which
/// plays the dash sound and sets `DASH`, and the synced-data hook that arms the cooldown.
///
/// Mounting itself IS implemented (`mob_interact`, `Camel.java:380-401`): right-clicking an adult
/// camel seats the player, up to two riders, and a saddle can be equipped by hand the same way the
/// equine framework does it.
///
/// Deliberately not ported, each a distinct gap rather than an approximation:
/// - `openCustomInventoryScreen` on a sneaking interact (`Camel.java:383-385`). Camel is an
///   `AbstractHorse` in vanilla and shares its saddle/armor menu; `CamelEntity` is a plain
///   `MobEntity` here and is not in the equine framework, so there is no menu to open.
/// - `isFood`/`fedFood` (`Camel.java:394-396`, cactus feeding, breeding, baby growth).
///   `CamelEntity` implements neither `Animal` nor `AgeableMob`, so feeding and breeding are
///   absent for camels entirely; that gap is separate from rideability and untouched here.
/// - `finalizeSpawn`'s `resetLastPoseChangeTickToFullStand` (`Camel.java:135-142`): a fresh camel
///   starts with tick `0` (standing, not in transition) instead of `max(0, now - 53)`, which
///   only lets it sit down sooner than vanilla's first 400 ticks.
pub struct CamelEntity {
    pub mob_entity: MobEntity,
    dashing: AtomicBool,
    dash_cooldown: AtomicI32,
    /// `Camel.LAST_POSE_CHANGE_TICK`. Signed: negative means sitting (see [`pose_time`]).
    last_pose_change_tick: AtomicI64,
    /// Set by `on_elastic_leash_pull`, which is a synchronous hook, and consumed in `mob_tick`
    /// where the stand-up (needing the world age) can run (`Camel.onElasticLeashPull`).
    leash_pulled: AtomicBool,
}

impl CamelEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let mob_entity = MobEntity::new(entity);
        let camel = Self {
            mob_entity,
            dashing: AtomicBool::new(false),
            dash_cooldown: AtomicI32::new(0),
            last_pose_change_tick: AtomicI64::new(0),
            leash_pulled: AtomicBool::new(false),
        };
        let mob_arc = Arc::new(camel);
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

            let camel_weak = Arc::downgrade(&mob_arc);
            goal_selector.add_goal(0, Box::new(SwimGoal::default()));
            // `CamelAi.CamelPanic(4.0F)` (`CamelAi.java:40`).
            goal_selector.add_goal(1, CamelPanicGoal::new(camel_weak.clone(), 4.0));
            goal_selector.add_goal(2, Box::new(CamelSitGoal::new(camel_weak.clone())));
            // `RandomStroll.stroll(2.0F)` behind `triggerIf(!refuseToMove)` (`CamelAi.java:84`).
            goal_selector.add_goal(
                3,
                RefuseToMoveGate::new(camel_weak, Box::new(WanderAroundGoal::new(2.0))),
            );
            goal_selector.add_goal(
                4,
                LookAtEntityGoal::with_default(mob_weak, &EntityType::PLAYER, 6.0),
            );
            goal_selector.add_goal(5, Box::new(RandomLookAroundGoal::default()));
        };

        mob_arc
    }

    #[must_use]
    pub fn is_dashing(&self) -> bool {
        self.dashing.load(Relaxed)
    }

    /// `Camel.setDashing` (`Camel.java:323-325`) plus `Camel.onSyncedDataUpdated`
    /// (`Camel.java:643-650`): a changed `DASH` value, in either direction, arms the cooldown
    /// when it is idle. `SynchedEntityData.set` does nothing for an unchanged value.
    pub fn set_dashing(&self, dashing: bool) {
        if self.dashing.swap(dashing, Relaxed) == dashing {
            return;
        }
        let _ = self
            .dash_cooldown
            .compare_exchange(0, DASH_COOLDOWN_TICKS, Relaxed, Relaxed);
        self.get_entity()
            .send_meta_data(&[Metadata::new(tracked_data::camel::DASH, dashing)], None);
    }

    /// `AgeableMob.isBaby`. `CamelEntity` is not an `AgeableMob` (see the struct doc), so the age
    /// field is read directly -- the same test `mob_init_data_tracker` below already uses.
    fn is_baby(&self) -> bool {
        self.get_entity().age.load(Relaxed) < 0
    }

    /// `AbstractHorse.isSaddled`, which Camel inherits (`AbstractHorse.java:961-962` reads the
    /// same slot to decide who controls the mob).
    async fn is_saddled(&self) -> bool {
        let saddle = {
            let equipment = self.mob_entity.living_entity.entity_equipment.lock().await;
            equipment.get(&EquipmentSlot::SADDLE)
        };
        is_valid_saddle_item(&saddle, self.get_entity().entity_type)
    }

    /// `Camel.isCamelSitting` (`Camel.java:572-574`).
    #[must_use]
    pub fn is_camel_sitting(&self) -> bool {
        self.last_pose_change_tick.load(Relaxed) < 0
    }

    /// `Camel.getPoseTime` (`Camel.java:630-632`).
    pub async fn get_pose_time(&self) -> i64 {
        let world = self.get_entity().world.load_full();
        pose_time(
            world.get_world_age().await,
            self.last_pose_change_tick.load(Relaxed),
        )
    }

    /// `Camel.isInPoseTransition` (`Camel.java:580-583`).
    pub async fn is_in_pose_transition(&self) -> bool {
        is_in_pose_transition_for(self.get_pose_time().await, self.is_camel_sitting())
    }

    /// `Camel.refuseToMove` (`Camel.java:267-269`).
    pub async fn refuse_to_move(&self) -> bool {
        self.is_camel_sitting() || self.is_in_pose_transition().await
    }

    /// `Camel.canCamelChangePose` (`Camel.java:417-419`): the box of the pose it would switch to
    /// must be free at its current position. Same expression `Entity::set_pose` uses.
    pub fn can_camel_change_pose(&self) -> bool {
        let entity = self.get_entity();
        let target = if self.is_camel_sitting() {
            EntityPose::Standing
        } else {
            EntityPose::Sitting
        };
        let pos = entity.pos.load();
        let aabb = BoundingBox::new_from_pos(pos.x, pos.y, pos.z, &entity.get_dimensions(target));
        entity.world.load().is_space_empty(aabb.contract_all(1.0E-7))
    }

    /// `Camel.resetLastPoseChangeTick` (`Camel.java:621-624`).
    fn reset_last_pose_change_tick(&self, synced_pose_tick_time: i64) {
        self.last_pose_change_tick
            .store(synced_pose_tick_time, Relaxed);
        self.get_entity().send_meta_data(
            &[Metadata::new(
                tracked_data::camel::LAST_POSE_CHANGE_TICK,
                synced_pose_tick_time,
            )],
            None,
        );
    }

    async fn emit_entity_action(&self) {
        let entity = self.get_entity();
        let world = entity.world.load_full();
        emit_game_event(
            &world,
            GameEvent::EntityAction,
            entity.pos.load(),
            GameEventContext::none(),
        )
        .await;
    }

    /// `Camel.sitDown` (`Camel.java:589-596`).
    pub async fn sit_down(&self) {
        if self.is_camel_sitting() {
            return;
        }
        let entity = self.get_entity();
        let world = entity.world.load_full();
        // `LivingEntity.makeSound` (`LivingEntity.java:1431-1435`).
        world.play_sound_fine(
            Sound::EntityCamelSit,
            self.get_sound_source(),
            &entity.pos.load(),
            1.0,
            self.get_sound_pitch(),
        );
        entity.set_pose_ignoring_space(EntityPose::Sitting);
        self.emit_entity_action().await;
        self.reset_last_pose_change_tick(-world.get_world_age().await);
    }

    /// `Camel.standUp` (`Camel.java:598-605`).
    pub async fn stand_up(&self) {
        if !self.is_camel_sitting() {
            return;
        }
        let entity = self.get_entity();
        let world = entity.world.load_full();
        world.play_sound_fine(
            Sound::EntityCamelStand,
            self.get_sound_source(),
            &entity.pos.load(),
            1.0,
            self.get_sound_pitch(),
        );
        entity.set_pose_ignoring_space(EntityPose::Standing);
        self.emit_entity_action().await;
        self.reset_last_pose_change_tick(world.get_world_age().await);
    }

    /// `Camel.standUpInstantly` (`Camel.java:615-619`): unconditional and silent.
    pub async fn stand_up_instantly(&self) {
        let entity = self.get_entity();
        entity.set_pose_ignoring_space(EntityPose::Standing);
        self.emit_entity_action().await;
        let now = entity.world.load_full().get_world_age().await;
        self.reset_last_pose_change_tick(full_stand_tick(now));
    }

    /// `Camel.tick`'s `clampHeadRotationToBody` (`Mob.java:767-774`).
    fn clamp_head_rotation_to_body(&self) {
        let entity = self.get_entity();
        let head = entity.head_yaw.load();
        let delta = wrap_degrees(entity.body_yaw.load() - head);
        let target = delta.clamp(-MAX_HEAD_Y_ROT, MAX_HEAD_Y_ROT);
        entity.head_yaw.store(head + delta - target);
    }
}

impl NBTStorage for CamelEntity {
    /// `Camel.addAdditionalSaveData` (`Camel.java:103-107`).
    fn write_nbt<'a>(&'a self, nbt: &'a mut NbtCompound) -> NbtFuture<'a, ()> {
        Box::pin(async move {
            self.mob_entity.living_entity.write_nbt(nbt).await;
            nbt.put_long("LastPoseTick", self.last_pose_change_tick.load(Relaxed));
        })
    }

    /// `Camel.readAdditionalSaveData` (`Camel.java:109-118`).
    fn read_nbt_non_mut<'a>(&'a self, nbt: &'a NbtCompound) -> NbtFuture<'a, ()> {
        Box::pin(async move {
            self.mob_entity.living_entity.read_nbt_non_mut(nbt).await;
            let pose_tick = nbt.get_long("LastPoseTick").unwrap_or(0);
            if pose_tick < 0 {
                self.get_entity()
                    .set_pose_ignoring_space(EntityPose::Sitting);
            }
            self.last_pose_change_tick.store(pose_tick, Relaxed);
        })
    }
}

impl Mob for CamelEntity {
    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    /// `AbstractHorse.getControllingPassenger` (`AbstractHorse.java:961-962`), which Camel
    /// inherits unchanged: a saddled camel is controlled by its first player passenger.
    fn has_controlling_passenger(&self) -> EntityBaseFuture<'_, bool> {
        Box::pin(async move {
            if !self.is_saddled().await {
                return Mob::has_controlling_passenger(self).await;
            }
            let passenger = self.get_entity().passengers.lock().await.first().cloned();
            if passenger.is_some_and(|passenger| passenger.get_player().is_some()) {
                return true;
            }
            Mob::has_controlling_passenger(self).await
        })
    }

    /// `Camel.mobInteract` (`Camel.java:380-401`). Vanilla needs no saddle to mount and seats two
    /// riders. The saddle-equip branch stands in for vanilla's generic
    /// `ItemStack.interactLivingEntity` dispatch, exactly as `abstract_horse_mob_interact` does
    /// (see the equine module header for why that dispatch is inlined here).
    fn mob_interact<'a>(
        &'a self,
        player: &'a Arc<Player>,
        item_stack: &'a mut ItemStack,
    ) -> EntityBaseFuture<'a, bool> {
        Box::pin(async move {
            if self
                .mob_entity
                .mob_interact(player, item_stack, self.can_be_leashed())
                .await
            {
                return true;
            }

            // `Camel.java:400`: a baby camel never rides and never equips.
            if self.is_baby() {
                return false;
            }

            if !item_stack.is_empty()
                && saddle_equip_on_interact(item_stack, self.get_entity().entity_type)
                && !self.is_saddled().await
            {
                equip_saddle_item(&self.mob_entity, player, item_stack).await;
                return true;
            }

            if self.get_entity().passengers.lock().await.len() < MAX_PASSENGERS {
                mount_player(&self.mob_entity, player).await;
                return true;
            }

            false
        })
    }

    fn mob_init_data_tracker(&self) -> EntityBaseFuture<'_, ()> {
        Box::pin(async move {
            let entity = self.get_entity();
            // Re-sends `BABY_ID` (dropped by overriding `mob_init_data_tracker`), matching the
            // blanket `Mob` `EntityBase` impl's default behavior (`mob/mod.rs`) -- same reason
            // `CatEntity` re-sends it manually.
            if entity.age.load(std::sync::atomic::Ordering::Relaxed) < 0 {
                entity.send_meta_data(&[Metadata::new(tracked_data::camel::BABY_ID, true)], None);
            }
            entity.send_meta_data(&[Metadata::new(tracked_data::camel::DASH, false)], None);
            entity.send_meta_data(
                &[Metadata::new(
                    tracked_data::camel::LAST_POSE_CHANGE_TICK,
                    self.last_pose_change_tick.load(Relaxed),
                )],
                None,
            );
        })
    }

    /// `Camel.canJump` (`Camel.java:287-290`): `!refuseToMove() && AbstractHorse.canJump`
    /// (`isSaddled`). The player-command handler calls it for `START_RIDING_JUMP`
    /// (`ServerGamePacketListenerImpl.java:1721-1727`).
    fn can_jump(&self) -> EntityBaseFuture<'_, bool> {
        Box::pin(async move { !self.refuse_to_move().await && self.is_saddled().await })
    }

    /// `Camel.handleStartJump` (`Camel.java:327-332`). `onPlayerJump` stays the inert default:
    /// its only caller is the rider's client (`Camel.java:293-297`).
    fn handle_start_jump(&self, _jump_scale: i32) {
        let entity = self.get_entity();
        let world = entity.world.load_full();
        let pos = entity.pos.load();
        world.play_sound_fine(
            Sound::EntityCamelDash,
            self.get_sound_source(),
            &pos,
            1.0,
            self.get_sound_pitch(),
        );
        // `handleStartJump` is a synchronous hook; the game event needs the async listener scan.
        tokio::spawn(async move {
            emit_game_event(&world, GameEvent::EntityAction, pos, GameEventContext::none()).await;
        });
        self.set_dashing(true);
    }

    /// `Camel.onElasticLeashPull` (`Camel.java:404-410`); the stand-up itself runs in `mob_tick`.
    fn on_elastic_leash_pull(&self) {
        self.default_on_elastic_leash_pull();
        self.leash_pulled.store(true, Relaxed);
    }

    /// `Camel.getMaxHeadYRot` (`Camel.java:562-565`).
    fn get_max_head_rotation(&self) -> f32 {
        MAX_HEAD_Y_ROT
    }

    /// `Camel.travel` (`Camel.java:249-257`): a sitting or transitioning camel on the ground
    /// loses its horizontal velocity and input, and the generic travel still runs the vertical
    /// part.
    fn custom_travel<'a>(&'a self, _caller: &'a Arc<dyn EntityBase>) -> EntityBaseFuture<'a, bool> {
        Box::pin(async move {
            let entity = self.get_entity();
            if entity.on_ground.load(Relaxed) && self.refuse_to_move().await {
                let velocity = entity.velocity.load();
                entity
                    .velocity
                    .store(Vector3::new(0.0, velocity.y, 0.0));
                let living = &self.mob_entity.living_entity;
                let input = living.movement_input.load();
                living
                    .movement_input
                    .store(Vector3::new(0.0, input.y, 0.0));
            }
            false
        })
    }

    /// `Camel.actuallyHurt` (`Camel.java:489-493`) stands the camel before the damage is applied;
    /// this hook only runs for accepted damage, which is when vanilla reaches `actuallyHurt`.
    fn on_damage<'a>(
        &'a self,
        _damage_type: DamageType,
        _source: Option<&'a dyn EntityBase>,
    ) -> EntityBaseFuture<'a, ()> {
        Box::pin(async move { self.stand_up_instantly().await })
    }

    /// `Camel.playStepSound` (`Camel.java:366-373`): sand-like blocks use the sand step, at full
    /// volume and pitch.
    fn ground_step_sounds(
        &self,
        supporting_block: &Block,
        _supporting_state: &BlockState,
        _above_block: &Block,
    ) -> Option<Vec<(Sound, f32, f32)>> {
        let sound = if supporting_block.has_tag(&tag::Block::MINECRAFT_CAMEL_SAND_STEP_SOUND_BLOCKS)
        {
            Sound::EntityCamelStepSand
        } else {
            Sound::EntityCamelStep
        };
        Some(vec![(sound, 1.0, 1.0)])
    }

    /// `Camel.tick` (`Camel.java:184-209`) and `Camel.CamelMoveControl.tick`
    /// (`Camel.java:700-711`).
    fn mob_tick<'a>(&'a self, _caller: &'a Arc<dyn EntityBase>) -> EntityBaseFuture<'a, ()> {
        Box::pin(async move {
            let entity = self.get_entity();

            if self.is_dashing() && self.dash_cooldown.load(Relaxed) < DASH_CLEAR_THRESHOLD {
                // `isInLiquid() = isInWater() || isInLava()`; `isPassenger()` is having a vehicle.
                let grounded = entity.on_ground.load(Relaxed)
                    || entity.is_in_liquid()
                    || entity.has_vehicle().await;
                if grounded {
                    self.set_dashing(false);
                }
            }

            // Re-read: clearing `DASH` at an idle cooldown re-arms it, and vanilla's decrement
            // below sees the armed value.
            let cooldown = self.dash_cooldown.load(Relaxed);
            if cooldown > 0 {
                self.dash_cooldown.store(cooldown - 1, Relaxed);
                if cooldown == 1 {
                    let block_pos = entity.block_pos.load().0;
                    entity.world.load().play_sound(
                        Sound::EntityCamelDashReady,
                        SoundCategory::Neutral,
                        &Vector3::new(
                            f64::from(block_pos.x) + 0.5,
                            f64::from(block_pos.y) + 0.5,
                            f64::from(block_pos.z) + 0.5,
                        ),
                    );
                }
            }

            if self.refuse_to_move().await {
                self.clamp_head_rotation_to_body();
            }

            if self.is_camel_sitting() && entity.touching_water.load(Relaxed) {
                self.stand_up_instantly().await;
            }

            // `Camel.tickRidden` (`Camel.java:260-265`): forward input from the controlling
            // rider stands a sitting camel that has finished its transition.
            if self.is_camel_sitting() && !self.is_in_pose_transition().await {
                let rider = entity.passengers.lock().await.first().cloned();
                if let Some(player) = rider.as_ref().and_then(|rider| rider.get_player())
                    && self.is_saddled().await
                    && player.last_input.load(Relaxed)
                        & pumpkin_protocol::java::server::play::SPlayerInput::FORWARD
                        != 0
                {
                    self.stand_up().await;
                }
            }

            // A walk target or an elastic leash pull stands a sitting camel that has finished
            // its transition and has room (`CamelMoveControl.tick`, `Camel.onElasticLeashPull`).
            let leash_pulled = self.leash_pulled.swap(false, Relaxed);
            let leashed = entity.leashed_to.lock().await.is_some();
            let moving = !self
                .mob_entity
                .navigator
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .is_idle();
            if (leash_pulled || (moving && !leashed))
                && self.is_camel_sitting()
                && !self.is_in_pose_transition().await
                && self.can_camel_change_pose()
            {
                self.stand_up().await;
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{full_stand_tick, is_in_pose_transition_for, pose_time};

    #[test]
    fn pose_time_ignores_the_sign_of_the_synced_tick() {
        assert_eq!(pose_time(1000, -900), 100);
        assert_eq!(pose_time(1000, 900), 100);
    }

    #[test]
    fn transition_lasts_forty_ticks_sitting_and_fifty_two_standing() {
        assert!(is_in_pose_transition_for(39, true));
        assert!(!is_in_pose_transition_for(40, true));
        assert!(is_in_pose_transition_for(51, false));
        assert!(!is_in_pose_transition_for(52, false));
    }

    #[test]
    fn full_stand_tick_is_out_of_transition_and_never_negative() {
        assert_eq!(full_stand_tick(1000), 947);
        assert!(!is_in_pose_transition_for(pose_time(1000, 947), false));
        assert_eq!(full_stand_tick(10), 0);
    }
}
