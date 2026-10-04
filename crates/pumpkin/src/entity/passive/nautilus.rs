use crossbeam::atomic::AtomicCell;
use std::sync::{
    Arc, Weak,
    atomic::{AtomicBool, AtomicI32, Ordering},
};
use tokio::sync::Mutex;
use uuid::Uuid;

use pumpkin_data::{
    effect::StatusEffect,
    entity::{EntityStatus, EntityType},
    item::Item,
    item_stack::ItemStack,
    particle::Particle,
    potion::Effect,
    sound::{Sound, SoundCategory},
    tag,
};
use pumpkin_inventory::generic_container_screen_handler::create_generic_3x3;
use pumpkin_inventory::player::player_inventory::PlayerInventory;
use pumpkin_inventory::screen_handler::{
    BoxFuture, InventoryPlayer, ScreenHandlerFactory, SharedScreenHandler,
};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_protocol::java::client::play::Metadata;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_util::text::TextComponent;
use pumpkin_world::inventory::{Inventory, SimpleInventory};

use crate::entity::{
    Entity, EntityBase, EntityBaseFuture, NBTStorage, NbtFuture,
    ai::goal::{
        breed::BreedGoal, escape_danger::EscapeDangerGoal, look_around::RandomLookAroundGoal,
        look_at_entity::LookAtEntityGoal, tempt::TemptGoal,
        wander_around::{BrainStroll, WanderAroundGoal},
    },
    mob::{Mob, MobEntity},
    passive::animal::Animal,
    player::Player,
};
use crate::world::World;

/// `ItemTags.NAUTILUS_FOOD` (`NautilusAi.getTemptations`, `NautilusAi.java:155-157`).
const NAUTILUS_TEMPT_ITEMS: &[&Item] = &[
    &Item::COD,
    &Item::COOKED_COD,
    &Item::SALMON,
    &Item::COOKED_SALMON,
    &Item::PUFFERFISH,
    &Item::TROPICAL_FISH,
    &Item::PUFFERFISH_BUCKET,
    &Item::COD_BUCKET,
    &Item::SALMON_BUCKET,
    &Item::TROPICAL_FISH_BUCKET,
];

/// Extra blocks beyond the radius before `checkRestriction` re-anchors the home
/// (`AbstractNautilus.java:249`).
const RESTRICTION_RADIUS_BUFFER: i32 = 8;

/// `AbstractNautilus.getNautilusRestrictionRadius` (`AbstractNautilus.java:241-243`).
pub(crate) const fn restriction_radius(is_baby: bool, is_saddled: bool) -> i32 {
    if !is_baby && !is_saddled { 32 } else { 16 }
}

/// Re-anchor condition of `AbstractNautilus.checkRestriction` (`AbstractNautilus.java:249`):
/// `!hasHome() || !home.closerThan(pos, radius + 8) || radius != homeRadius`, where
/// `closerThan` (`Vec3i.java:193-195`) is a strict squared-distance comparison.
fn needs_rehome(has_home: bool, dist_sq: f64, radius: i32, home_radius: i32) -> bool {
    let limit = f64::from(radius + RESTRICTION_RADIUS_BUFFER);
    !has_home || dist_sq >= limit * limit || radius != home_radius
}

/// The re-anchor half of `AbstractNautilus.checkRestriction` (`AbstractNautilus.java:248-250`),
/// shared by both nautilus species once the tame/unleashed/unridden gate has passed.
pub(crate) fn rehome_if_needed(mob_entity: &MobEntity, radius: i32) {
    let entity = &mob_entity.living_entity.entity;
    let home_radius = mob_entity.position_target_range.load(Ordering::Relaxed);
    let home = mob_entity.position_target.load();
    let pos = entity.block_pos.load();
    let dx = f64::from(home.0.x) - f64::from(pos.0.x);
    let dy = f64::from(home.0.y) - f64::from(pos.0.y);
    let dz = f64::from(home.0.z) - f64::from(pos.0.z);
    // `hasHome()` is `homeRadius != -1` (`Mob.java:1225-1227`).
    if needs_rehome(
        home_radius != -1,
        dx.mul_add(dx, dy.mul_add(dy, dz * dz)),
        radius,
        home_radius,
    ) {
        mob_entity.position_target.store(pos);
        mob_entity
            .position_target_range
            .store(radius, Ordering::Relaxed);
    }
}

/// A nautilus (`net/minecraft/world/entity/animal/nautilus/Nautilus.java`, behaviour in
/// `NautilusAi.java`, shared base in `AbstractNautilus.java`).
///
/// Vanilla drives this mob entirely from a `Brain`. The behaviours that map onto Pumpkin's
/// goal selector are wired in [`NautilusEntity::new`]: `AnimalPanic(1.6F)` and the idle
/// `RandomStroll.swim(1.0F)` / `SetWalkTargetFromLookTarget` pair from `NautilusAi`'s CORE
/// and IDLE activities.
pub struct NautilusEntity {
    pub mob_entity: MobEntity,
    pub is_tame: AtomicBool,
    pub owner: AtomicCell<Option<Uuid>>,
    pub is_dashing: AtomicBool,
    pub dash_cooldown: AtomicI32,
    pub is_saddled: AtomicBool,
    // Vanilla passes this mount inventory to the player screen (`AbstractNautilus.java:504-507`,
    // `ServerPlayer.java:1385-1395`).
    pub inventory: Arc<SimpleInventory>,
}

// The existing generic screen factory is the server-side inventory path used by this entity;
// vanilla selects its mount packet and menu in `ServerPlayer.openNautilusInventory`
// (`ServerPlayer.java:1385-1395`).
struct NautilusScreenFactory(Arc<SimpleInventory>);

impl ScreenHandlerFactory for NautilusScreenFactory {
    fn create_screen_handler<'a>(
        &'a self,
        sync_id: u8,
        player_inventory: &'a Arc<PlayerInventory>,
        _player: &'a dyn InventoryPlayer,
    ) -> BoxFuture<'a, Option<SharedScreenHandler>> {
        Box::pin(async move {
            let inventory: Arc<dyn Inventory> = self.0.clone();
            let handler = create_generic_3x3(sync_id, player_inventory, inventory).await;
            Some(Arc::new(Mutex::new(handler)) as SharedScreenHandler)
        })
    }

    fn get_display_name(&self) -> TextComponent {
        TextComponent::text("Nautilus")
    }
}

impl NautilusEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let mob_entity = MobEntity::new(entity);
        let nautilus = Self {
            mob_entity,
            is_tame: AtomicBool::new(false),
            owner: AtomicCell::new(None),
            is_dashing: AtomicBool::new(false),
            dash_cooldown: AtomicI32::new(0),
            is_saddled: AtomicBool::new(false),
            inventory: Arc::new(SimpleInventory::new(9)),
        };

        let mob_arc = Arc::new(nautilus);
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

            // `NautilusAi`/`AbstractNautilus` has no float/swim behaviour: nautiluses are
            // always in water.
            // `NautilusAi.initCoreActivity`: `new AnimalPanic(1.6F)`.
            goal_selector.add_goal(0, EscapeDangerGoal::new(1.6));
            // `NautilusAi.initIdleActivity`: `AnimalMakeLove(NAUTILUS, 0.4F, 2)`.
            goal_selector.add_goal(1, BreedGoal::new(0.4));
            // `FollowTemptation(mob -> 1.3F, mob -> mob.isBaby() ? 2.5 : 3.5)`.
            goal_selector.add_goal(
                2,
                Box::new(TemptGoal::for_nautilus(1.3, NAUTILUS_TEMPT_ITEMS)),
            );
            // `NautilusAi.initIdleActivity`: `RandomStroll.swim(1.0F)` inside an
            // `ORDERED`/`TRY_ALL` gate with no `DoNothing` (`NautilusAi.java:88-96`), so it is
            // retried every tick while there is no walk target (interval 1). DEVIATION: the goal
            // selector only evaluates `can_start` every other tick, so retries run at half the
            // vanilla brain cadence.
            goal_selector.add_goal(
                4,
                Box::new(WanderAroundGoal::new_brain_stroll(
                    const { &[BrainStroll::swim(1.0)] },
                    1,
                )),
            );
            goal_selector.add_goal(
                5,
                LookAtEntityGoal::with_default(mob_weak, &EntityType::PLAYER, 8.0),
            );
            goal_selector.add_goal(6, Box::new(RandomLookAroundGoal::default()));
        };

        mob_arc
    }

    /// `AbstractNautilus.checkRestriction` (`AbstractNautilus.java:245-252`).
    async fn check_restriction(&self) {
        let entity = &self.mob_entity.living_entity.entity;
        if !self.is_tame() || entity.is_leashed().await {
            return;
        }
        if !entity.passengers.lock().await.is_empty() {
            return;
        }

        let is_baby = entity.age.load(Ordering::Relaxed) < 0;
        rehome_if_needed(
            &self.mob_entity,
            restriction_radius(is_baby, self.is_saddled.load(Ordering::Relaxed)),
        );
    }

    /// `AbstractNautilus.usePlayerItem` (`AbstractNautilus.java:102-108`): bucket foods become
    /// a water bucket through `ItemUtils.createFilledResult` (`ItemUtils.java:16-37`, with the
    /// creative stack-size limit); anything else is consumed normally.
    async fn use_player_item(player: &Arc<Player>, item_stack: &mut ItemStack) {
        if !tag::Item::MINECRAFT_NAUTILUS_BUCKET_FOOD
            .1
            .contains(&item_stack.item.id)
        {
            MobEntity::use_player_item(item_stack, player.gamemode.load());
            return;
        }

        super::animal::fill_water_bucket_result(player, item_stack).await;
    }

    /// `AbstractNautilus.tryToTame` (`AbstractNautilus.java:434-444`).
    fn try_to_tame(&self, player: &Arc<Player>) {
        let entity = &self.mob_entity.living_entity.entity;
        let world = entity.world.load();
        if rand::random::<u32>().is_multiple_of(3) {
            self.set_tame(true, Some(player.gameprofile.id));
            self.mob_entity
                .navigator
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .stop();
            world.send_entity_status(entity, EntityStatus::TamingSucceeded, None);
        } else {
            world.send_entity_status(entity, EntityStatus::TamingFailed, None);
        }
        world.play_sound(
            self.get_eat_sound(),
            SoundCategory::Neutral,
            &entity.pos.load(),
        );
    }

    pub fn is_dashing(&self) -> bool {
        self.is_dashing.load(Ordering::Relaxed)
    }

    pub fn set_dashing(&self, dashing: bool) {
        self.is_dashing.store(dashing, Ordering::Relaxed);
        self.mob_entity.living_entity.entity.send_meta_data(
            &[Metadata::new(
                pumpkin_data::tracked_data::nautilus::DASH,
                dashing,
            )],
            None,
        );
    }

    pub fn is_tame(&self) -> bool {
        self.is_tame.load(Ordering::Relaxed)
    }

    pub fn set_tame(&self, tame: bool, owner: Option<Uuid>) {
        self.is_tame.store(tame, Ordering::Relaxed);
        self.owner.store(owner);
    }

    pub fn ambient_sound(&self) -> Sound {
        let is_baby = self
            .mob_entity
            .living_entity
            .entity
            .age
            .load(Ordering::Relaxed)
            < 0;
        let is_water = self
            .mob_entity
            .living_entity
            .entity
            .touching_water
            .load(Ordering::Relaxed);
        if is_baby {
            if is_water {
                Sound::EntityBabyNautilusAmbient
            } else {
                Sound::EntityBabyNautilusAmbientLand
            }
        } else if is_water {
            Sound::EntityNautilusAmbient
        } else {
            Sound::EntityNautilusAmbientLand
        }
    }

    pub fn get_hurt_sound(&self) -> Sound {
        let is_baby = self
            .mob_entity
            .living_entity
            .entity
            .age
            .load(Ordering::Relaxed)
            < 0;
        let is_water = self
            .mob_entity
            .living_entity
            .entity
            .touching_water
            .load(Ordering::Relaxed);
        if is_baby {
            if is_water {
                Sound::EntityBabyNautilusHurt
            } else {
                Sound::EntityBabyNautilusHurtLand
            }
        } else if is_water {
            Sound::EntityNautilusHurt
        } else {
            Sound::EntityNautilusHurtLand
        }
    }

    pub fn get_death_sound(&self) -> Sound {
        let is_baby = self
            .mob_entity
            .living_entity
            .entity
            .age
            .load(Ordering::Relaxed)
            < 0;
        let is_water = self
            .mob_entity
            .living_entity
            .entity
            .touching_water
            .load(Ordering::Relaxed);
        if is_baby {
            if is_water {
                Sound::EntityBabyNautilusDeath
            } else {
                Sound::EntityBabyNautilusDeathLand
            }
        } else if is_water {
            Sound::EntityNautilusDeath
        } else {
            Sound::EntityNautilusDeathLand
        }
    }

    pub fn get_dash_sound(&self) -> Sound {
        let is_water = self
            .mob_entity
            .living_entity
            .entity
            .touching_water
            .load(Ordering::Relaxed);
        if is_water {
            Sound::EntityNautilusDash
        } else {
            Sound::EntityNautilusDashLand
        }
    }

    pub fn get_dash_ready_sound(&self) -> Sound {
        let is_water = self
            .mob_entity
            .living_entity
            .entity
            .touching_water
            .load(Ordering::Relaxed);
        if is_water {
            Sound::EntityNautilusDashReady
        } else {
            Sound::EntityNautilusDashReadyLand
        }
    }

    pub fn get_eat_sound(&self) -> Sound {
        let is_baby = self
            .mob_entity
            .living_entity
            .entity
            .age
            .load(Ordering::Relaxed)
            < 0;
        if is_baby {
            Sound::EntityBabyNautilusEat
        } else {
            Sound::EntityNautilusEat
        }
    }

    pub fn get_swim_sound(&self) -> Sound {
        let is_baby = self
            .mob_entity
            .living_entity
            .entity
            .age
            .load(Ordering::Relaxed)
            < 0;
        if is_baby {
            Sound::EntityBabyNautilusSwim
        } else {
            Sound::EntityNautilusSwim
        }
    }
}

impl NBTStorage for NautilusEntity {
    fn write_nbt<'a>(&'a self, nbt: &'a mut NbtCompound) -> NbtFuture<'a, ()> {
        Box::pin(async move {
            self.mob_entity.living_entity.write_nbt(nbt).await;
            self.write_animal_nbt(nbt);
            nbt.put_bool("IsTame", self.is_tame.load(Ordering::Relaxed));
            nbt.put_bool("Saddled", self.is_saddled.load(Ordering::Relaxed));
            nbt.put_int("DashCooldown", self.dash_cooldown.load(Ordering::Relaxed));
            if let Some(owner) = self.owner.load() {
                nbt.put_uuid("Owner", owner);
            }
        })
    }

    fn read_nbt_non_mut<'a>(&'a self, nbt: &'a NbtCompound) -> NbtFuture<'a, ()> {
        Box::pin(async move {
            self.mob_entity.living_entity.read_nbt_non_mut(nbt).await;
            self.read_animal_nbt(nbt);
            if let Some(is_tame) = nbt.get_bool("IsTame") {
                self.is_tame.store(is_tame, Ordering::Relaxed);
            }
            if let Some(saddled) = nbt.get_bool("Saddled") {
                self.is_saddled.store(saddled, Ordering::Relaxed);
            }
            if let Some(dash) = nbt.get_int("DashCooldown") {
                self.dash_cooldown.store(dash, Ordering::Relaxed);
            }
            if let Some(owner) = nbt.get_uuid("Owner") {
                self.owner.store(Some(owner));
            }
        })
    }
}

impl Animal for NautilusEntity {
    /// `AbstractNautilus.isFood` (`AbstractNautilus.java:97-100`).
    fn is_food(&self, item_stack: &ItemStack) -> bool {
        let is_baby = self.mob_entity.living_entity.entity.age.load(Ordering::Relaxed) < 0;
        let tag = if !self.is_tame() && !is_baby {
            tag::Item::MINECRAFT_NAUTILUS_TAMING_ITEMS
        } else {
            tag::Item::MINECRAFT_NAUTILUS_FOOD
        };
        tag.1.contains(&item_stack.item.id)
    }

    /// `AbstractNautilus.usePlayerItem` also covers `Animal.mobInteract`'s feeding branches.
    fn animal_use_player_item<'a>(
        &'a self,
        player: &'a Arc<Player>,
        item_stack: &'a mut ItemStack,
    ) -> EntityBaseFuture<'a, ()> {
        Box::pin(Self::use_player_item(player, item_stack))
    }
}

impl Mob for NautilusEntity {
    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    /// `AbstractNautilus.getControllingPassenger` (`AbstractNautilus.java:170-171`): a saddled
    /// nautilus is controlled by its first player passenger.
    fn has_controlling_passenger(&self) -> EntityBaseFuture<'_, bool> {
        Box::pin(async move {
            let first = self.get_entity().passengers.lock().await.first().cloned();
            if self.is_saddled() && first.is_some_and(|passenger| passenger.get_player().is_some()) {
                return true;
            }
            self.default_has_controlling_passenger().await
        })
    }

    /// `AbstractNautilus.openCustomInventoryScreen` gates the ridden inventory on taming and
    /// the controlling passenger (`AbstractNautilus.java:503-507`), then calls
    /// `ServerPlayer.openNautilusInventory` (`ServerPlayer.java:1385-1395`).
    fn open_custom_inventory_screen<'a>(
        &'a self,
        player: &'a Arc<Player>,
    ) -> EntityBaseFuture<'a, ()> {
        Box::pin(async move {
            if !self.is_tame() {
                return;
            }
            let passengers = self.mob_entity.living_entity.entity.passengers.lock().await;
            if passengers.is_empty()
                || !passengers
                    .iter()
                    .any(|passenger| passenger.get_entity().entity_id == player.entity_id())
            {
                return;
            }
            drop(passengers);
            player
                .open_handled_screen(&NautilusScreenFactory(self.inventory.clone()), None)
                .await;
        })
    }

    /// `Nautilus.getAmbientSound` (Nautilus.java:78-84), reached through the shared
    /// `Mob.baseTick` idle-sound cadence.
    fn get_ambient_sound(&self) -> Option<Sound> {
        Some(self.ambient_sound())
    }

    fn mob_init_data_tracker(&self) -> EntityBaseFuture<'_, ()> {
        Box::pin(async move {
            self.mob_entity.living_entity.entity.send_meta_data(
                &[Metadata::new(
                    pumpkin_data::tracked_data::nautilus::DASH,
                    self.is_dashing(),
                )],
                None,
            );
        })
    }

    fn mob_tick<'a>(&'a self, _caller: &'a Arc<dyn EntityBase>) -> EntityBaseFuture<'a, ()> {
        Box::pin(async move {
            let entity = &self.mob_entity.living_entity.entity;

            self.check_restriction().await;

            let passengers = entity.passengers.lock().await;
            if let Some(passenger) = passengers.first()
                && let Some(player) = passenger.cast_any().downcast_ref::<Player>()
            {
                let world = entity.world.load();
                let game_time = world.level_time.lock().await.world_age;
                if game_time % 40 == 0 {
                    player
                        .living_entity
                        .add_effect(Effect {
                            effect_type: &StatusEffect::BREATH_OF_THE_NAUTILUS,
                            duration: 60,
                            amplifier: 0,
                            ambient: true,
                            show_particles: true,
                            show_icon: true,
                            blend: true,
                        })
                        .await;
                }
            }

            if self.is_dashing() && self.dash_cooldown.load(Ordering::Relaxed) < 35 {
                self.set_dashing(false);
            }

            let cooldown = self.dash_cooldown.load(Ordering::Relaxed);
            if cooldown > 0 {
                let next = cooldown - 1;
                self.dash_cooldown.store(next, Ordering::Relaxed);
                if next == 0 {
                    let world = entity.world.load();
                    world.play_sound(
                        self.get_dash_ready_sound(),
                        SoundCategory::Neutral,
                        &entity.pos.load(),
                    );
                }
            }

            if entity.touching_water.load(Ordering::Relaxed) {
                let velo = entity.velocity.load();
                let speed = velo.length();
                let prob = (speed * 2.0).clamp(0.15, 1.0);
                if rand::random::<f64>() < prob {
                    let world = entity.world.load();
                    let pos = entity.pos.load();
                    world.spawn_particle(
                        pos + Vector3::new(0.0, 0.25, 0.0),
                        Vector3::new(0.4, 0.4, 0.4),
                        0.5,
                        2,
                        Particle::Bubble,
                    );
                }
            }
        })
    }

    /// `Nautilus.getBreedOffspring` (`Nautilus.java:50-58`): a tame parent passes on its owner.
    fn create_offspring<'a>(
        &'a self,
        _mate: &'a dyn EntityBase,
        world: &'a Arc<World>,
    ) -> EntityBaseFuture<'a, Option<Arc<dyn EntityBase>>> {
        Box::pin(async move {
            let entity = self.get_entity();
            let baby = crate::entity::r#type::from_type(
                entity.entity_type,
                entity.pos.load(),
                world,
                Uuid::new_v4(),
            );
            if self.is_tame()
                && let Some(baby_nautilus) = baby.cast_any().downcast_ref::<Self>()
            {
                baby_nautilus.set_tame(true, self.owner.load());
            }
            Some(baby)
        })
    }

    /// `AbstractNautilus.mobInteract` (`AbstractNautilus.java:395-432`).
    fn mob_interact<'a>(
        &'a self,
        player: &'a Arc<Player>,
        item_stack: &'a mut ItemStack,
    ) -> EntityBaseFuture<'a, bool> {
        Box::pin(async move {
            let mob_entity = &self.mob_entity;
            let entity = &mob_entity.living_entity.entity;

            if entity.age.load(Ordering::Relaxed) < 0 {
                return self
                    .animal_interact(player, item_stack, self.ambient_sound())
                    .await;
            }

            if self.is_tame() && player.get_entity().is_sneaking() {
                self.open_custom_inventory_screen(player).await;
                return true;
            }

            if !item_stack.is_empty() {
                let is_food = self.is_food(item_stack);
                if !self.is_tame() && is_food {
                    Self::use_player_item(player, item_stack).await;
                    self.try_to_tame(player);
                    return true;
                }

                if is_food
                    && mob_entity.living_entity.health.load()
                        < mob_entity.living_entity.get_max_health()
                {
                    // `feed(player, hand, stack, 2.0F, 1.0F)`: the bucket foods carry no food
                    // component, so they heal the default 1.0.
                    if tag::Item::MINECRAFT_NAUTILUS_BUCKET_FOOD
                        .1
                        .contains(&item_stack.item.id)
                    {
                        Self::use_player_item(player, item_stack).await;
                        mob_entity.living_entity.heal(1.0);
                        let world = entity.world.load();
                        world.play_sound(
                            self.get_eat_sound(),
                            SoundCategory::Neutral,
                            &entity.pos.load(),
                        );
                    } else {
                        crate::entity::passive::tamable::feed(
                            player,
                            item_stack,
                            &mob_entity.living_entity,
                            2.0,
                            1.0,
                            Some(self.get_eat_sound()),
                        );
                    }
                    return true;
                }

                // `itemStack.interactLivingEntity`: the saddle equips onto a tame nautilus.
                if self.is_tame()
                    && !self.is_saddled.load(Ordering::Relaxed)
                    && item_stack.item == &Item::SADDLE
                {
                    item_stack.decrement_unless_creative(player.gamemode.load(), 1);
                    self.is_saddled.store(true, Ordering::Relaxed);
                    let world = entity.world.load();
                    world.play_sound(
                        Sound::ItemNautilusSaddleEquip,
                        SoundCategory::Neutral,
                        &entity.pos.load(),
                    );
                    return true;
                }
            }

            if self.is_tame() && !self.is_food(item_stack) {
                // `doPlayerRide`: start riding, then drop the home if nothing is aboard.
                if player.get_entity().can_start_riding().await {
                    let world = player.world();
                    if let Some(vehicle) = world.get_entity_by_id(entity.entity_id)
                        && let Some(passenger) = world.get_player_by_id(player.entity_id())
                    {
                        entity
                            .add_passenger(vehicle, passenger as Arc<dyn EntityBase>)
                            .await;
                    }
                }
                if entity.passengers.lock().await.is_empty() {
                    mob_entity.clear_home();
                }
                return true;
            }

            self.animal_interact(player, item_stack, self.ambient_sound())
                .await
        })
    }

    fn is_saddled(&self) -> bool {
        self.is_saddled.load(Ordering::Relaxed)
    }

    fn can_be_saddled(&self) -> bool {
        self.mob_entity.living_entity.entity.is_alive()
    }

    fn set_saddled(&self, saddled: bool) {
        self.is_saddled.store(saddled, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::{needs_rehome, restriction_radius};

    #[test]
    fn restriction_radius_is_32_only_for_unsaddled_adults() {
        assert_eq!(restriction_radius(false, false), 32);
        assert_eq!(restriction_radius(true, false), 16);
        assert_eq!(restriction_radius(false, true), 16);
    }

    #[test]
    fn rehome_when_missing_far_or_radius_changed() {
        assert!(needs_rehome(false, 0.0, 32, -1));
        assert!(!needs_rehome(true, 39.0 * 39.0, 32, 32));
        assert!(needs_rehome(true, 40.0 * 40.0, 32, 32));
        assert!(needs_rehome(true, 0.0, 16, 32));
    }
}
