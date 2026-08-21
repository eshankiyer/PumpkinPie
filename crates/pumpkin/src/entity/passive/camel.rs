use std::sync::atomic::{AtomicBool, AtomicI32, Ordering::Relaxed};
use std::sync::{Arc, Weak};

use pumpkin_data::data_component_impl::EquipmentSlot;
use pumpkin_data::entity::EntityType;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::tracked_data;
use pumpkin_protocol::java::client::play::Metadata;

use crate::entity::{
    Entity, EntityBase, EntityBaseFuture, NBTStorage,
    ai::goal::{
        camel_sit::CamelSitGoal, look_around::RandomLookAroundGoal,
        look_at_entity::LookAtEntityGoal, swim::SwimGoal, wander_around::WanderAroundGoal,
    },
    mob::{Mob, MobEntity},
    passive::equine::{
        equip_saddle_item, is_valid_saddle_item, mount_player, saddle_equip_on_interact,
    },
    player::Player,
};

/// `Camel.java:314` (`dash()`).
const DASH_COOLDOWN_TICKS: i32 = 55;
/// `Camel.java:187` (`isDashing() && dashCooldown < 50 && (onGround || isInLiquid || isPassenger)`).
const DASH_CLEAR_THRESHOLD: i32 = 50;
/// `Camel.java:397` (`this.getPassengers().size() < 2`): a camel seats two riders.
const MAX_PASSENGERS: usize = 2;

/// Represents a Camel, a passive mount that can carry two players and dash.
///
/// Wiki: <https://minecraft.wiki/w/Camel>
///
/// Vanilla's dash/rider-jump mechanic (`getRiddenInput`/`canJump`/`handleStartJump`,
/// `Camel.java:260-349`) is entirely rider-input driven -- a saddled, ridden camel dashes when its
/// controlling passenger double-taps jump. Pumpkin has no `PlayerRideableJumping`/mounted-input
/// routing system at all yet (confirmed absent from `entity/mod.rs` and `mob/mod.rs`, same gap
/// the equine framework's design doc flags as "Phase 2"), so nothing can ever call `dash()`/
/// `set_dashing(true)` here. What *is* implemented is the non-rider-dependent half of the state
/// machine: the `isDashing`/`DASH` synced flag and the cooldown countdown plus its
/// auto-clear-on-landing rule (`Camel.java:187-193`), so the plumbing is ready and correct for
/// whenever a future mounted-input system starts calling `set_dashing`/`dash`. Sitting/pose
/// (`CamelSitGoal`) is unaffected and already implemented separately.
///
/// Mounting itself IS implemented (`mob_interact`, `Camel.java:380-401`): right-clicking an adult
/// camel seats the player, up to two riders, and a saddle can be equipped by hand the same way the
/// equine framework does it. Steering a mounted camel is the same gap the equine module header
/// documents ("a horse can be saddled and mounted, but does not yet respond to WASD/jump while
/// ridden") -- the camel is no worse off than a horse here.
///
/// Deliberately not ported, each a distinct gap rather than an approximation:
/// - `openCustomInventoryScreen` on a sneaking interact (`Camel.java:383-385`). Camel is an
///   `AbstractHorse` in vanilla and shares its saddle/armor menu; `CamelEntity` is a plain
///   `MobEntity` here and is not in the equine framework, so there is no menu to open.
/// - `isFood`/`fedFood` (`Camel.java:394-396`, cactus feeding, breeding, baby growth).
///   `CamelEntity` implements neither `Animal` nor `AgeableMob`, so feeding and breeding are
///   absent for camels entirely; that gap is separate from rideability and untouched here.
/// - `LAST_POSE_CHANGE_TICK` sitting-pose sync: the vanilla field is a LONG and
///   `MetadataSerializer` has no `i64` impl, so a sitting camel still renders standing (the
///   caveat `CamelSitGoal`'s doc comment already carries).
pub struct CamelEntity {
    pub mob_entity: MobEntity,
    dashing: AtomicBool,
    dash_cooldown: AtomicI32,
}

impl CamelEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let mob_entity = MobEntity::new(entity);
        let camel = Self {
            mob_entity,
            dashing: AtomicBool::new(false),
            dash_cooldown: AtomicI32::new(0),
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

            goal_selector.add_goal(0, Box::new(SwimGoal::default()));
            goal_selector.add_goal(1, Box::new(CamelSitGoal::new()));
            goal_selector.add_goal(2, Box::new(WanderAroundGoal::new(1.0)));
            goal_selector.add_goal(
                3,
                LookAtEntityGoal::with_default(mob_weak, &EntityType::PLAYER, 6.0),
            );
            goal_selector.add_goal(4, Box::new(RandomLookAroundGoal::default()));
        };

        mob_arc
    }

    #[must_use]
    pub fn is_dashing(&self) -> bool {
        self.dashing.load(Relaxed)
    }

    /// `Camel.setDashing` (`Camel.java:323-325`).
    pub fn set_dashing(&self, dashing: bool) {
        self.dashing.store(dashing, Relaxed);
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

    /// `Camel.dash` (`Camel.java:312-316`): sets the cooldown and the dashing flag. Nothing
    /// currently calls this (see the struct doc), but it's kept ready for the future rider-input
    /// path.
    pub fn dash(&self) {
        self.dash_cooldown.store(DASH_COOLDOWN_TICKS, Relaxed);
        self.set_dashing(true);
    }
}

impl NBTStorage for CamelEntity {}

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
        })
    }

    /// `Camel.aiStep`'s dash-cooldown handling (`Camel.java:187-193`): once dashing, the flag
    /// clears as soon as the cooldown drops under 50 ticks and the camel is grounded, in liquid,
    /// or carrying a passenger; the cooldown itself always ticks down to zero regardless.
    fn mob_tick<'a>(&'a self, _caller: &'a Arc<dyn EntityBase>) -> EntityBaseFuture<'a, ()> {
        Box::pin(async move {
            let entity = self.get_entity();
            let cooldown = self.dash_cooldown.load(Relaxed);

            if self.is_dashing() && cooldown < DASH_CLEAR_THRESHOLD {
                let grounded =
                    entity.on_ground.load(Relaxed) || entity.touching_water.load(Relaxed);
                let carrying_passenger = !entity.passengers.lock().await.is_empty();
                if grounded || carrying_passenger {
                    self.set_dashing(false);
                }
            }

            if cooldown > 0 {
                self.dash_cooldown.store(cooldown - 1, Relaxed);
            }
        })
    }
}
