// Legacy invariant checks retained for vanilla behavior; migrate these paths before removing this allow.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicI32, Ordering, Ordering::Relaxed},
};

use pumpkin_data::data_component_impl::{EquipmentSlot, EquippableImpl, IDSet, IdOr};
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_data::tag::{self, Taggable};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_protocol::java::client::play::Metadata;
use pumpkin_util::math::boundingbox::EntityDimensions;

use crate::entity::{
    Entity, EntityBase, EntityBaseFuture, NBTStorage, NbtFuture,
    ageable::{AgeableData, AgeableMob},
    ai::goal::{ghast_random_float::GhastRandomFloatAroundGoal, swim::SwimGoal, tempt::TemptGoal},
    mob::{Mob, MobEntity},
    passive::animal::Animal,
    player::Player,
};

/// What tempts a happy ghast: the snowball.
///
/// Vanilla's tempt set is the harness-color tags plus the snowball
/// (`ItemTags.HAPPY_GHAST_TEMPT_ITEMS`); Pumpkin's `TemptGoal` only accepts a
/// static item list, not a tag/predicate, so this is narrowed to the snowball,
/// matching `ItemTags.HAPPY_GHAST_FOOD` (the baby feed/growth tag) exactly.
pub const HAPPY_GHAST_FOOD: &[&Item] = &[&Item::SNOWBALL];

const HEAL_INTERVAL_TICKS: i32 = 600;

/// `HappyGhast.java:448-450`.
#[must_use]
pub const fn restriction_radius(is_baby: bool, has_body_armor: bool) -> i32 {
    if !is_baby && !has_body_armor { 64 } else { 32 }
}

/// Represents a Happy Ghast, a passive flying mob that can be equipped with a
/// dyed Harness and ridden.
///
/// Wiki: <https://minecraft.wiki/w/Happy_Ghast>
pub struct HappyGhastEntity {
    pub mob_entity: MobEntity,
    pub ageable_data: AgeableData,
    heal_ticks: AtomicI32,
    pub server_still_timeout: AtomicI32,
    pub leash_holder_time: AtomicI32,
    pub is_leash_holder: AtomicBool,
    pub stays_still: AtomicBool,
}

impl HappyGhastEntity {
    pub(crate) fn can_breathe_underwater(&self) -> bool {
        self.is_baby()
    }
    pub fn new(entity: Entity) -> Arc<Self> {
        let mob_entity = MobEntity::new(entity);
        let happy_ghast = Self {
            mob_entity,
            ageable_data: AgeableData::default(),
            heal_ticks: AtomicI32::new(0),
            server_still_timeout: AtomicI32::new(0),
            leash_holder_time: AtomicI32::new(0),
            is_leash_holder: AtomicBool::new(false),
            stays_still: AtomicBool::new(false),
        };

        let mob_arc = Arc::new(happy_ghast);

        {
            let mut goal_selector = mob_arc
                .mob_entity
                .goals_selector
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);

            // `HappyGhast.registerGoals` (`HappyGhast.java:102-118`) registers exactly three
            // goals, at priorities 3/4/5. No look-at-player or random-look-around goal appears
            // in that list -- the head yaw is driven by `HappyGhastLookControl`
            // (`HappyGhast.java:639-660`) instead, so both are dropped here.
            //
            // Divergence: vanilla registers this set only for adults. `babyGhastSetup`
            // (`HappyGhast.java:132-138`) calls `removeAllGoals(goal -> true)` and registers
            // nothing, and `adultGhastSetup` (`HappyGhast.java:120-129`) clears and re-registers
            // on the age boundary. This codebase registers once at construction, so a baby happy
            // ghast still runs the adult goal list.
            //
            // `HappyGhast.HappyGhastFloatGoal` (`HappyGhast.java:628-637`) is a `FloatGoal`
            // whose `canUse` additionally requires `!isOnStillTimeout()`; that extra gate is not
            // modelled by this codebase's `SwimGoal`.
            goal_selector.add_goal(3, Box::new(SwimGoal::default()));
            // `TemptGoal.ForNonPathfinders(this, 1.0, <tag predicate>, false, 7.0)`
            // (`HappyGhast.java:104-118`). Vanilla picks `HAPPY_GHAST_TEMPT_ITEMS` for an
            // unarmored adult and `HAPPY_GHAST_FOOD` otherwise; see `HAPPY_GHAST_FOOD` above for
            // why the single-list approximation is used.
            goal_selector.add_goal(
                4,
                Box::new(TemptGoal::with_stop_distance(
                    1.0,
                    HAPPY_GHAST_FOOD,
                    false,
                    7.0,
                )),
            );
            // `Ghast.RandomFloatAroundGoal(this, 16)` (`HappyGhast.java:117`). Happy ghasts fly,
            // so the previous ground-pathing `WanderAroundGoal` was wrong. Divergence: this
            // codebase's goal is the `distanceToBlocks == 0` form (see
            // `ghast_random_float.rs`), so vanilla's 16-block clearance check on the chosen
            // target position is not applied.
            goal_selector.add_goal(5, Box::new(GhastRandomFloatAroundGoal::new()));
        };

        mob_arc
    }

    async fn body_armor_stack(&self) -> ItemStack {
        self.mob_entity
            .living_entity
            .entity_equipment
            .lock()
            .await
            .get(&EquipmentSlot::BODY)
    }

    async fn is_wearing_body_armor(&self) -> bool {
        !self.body_armor_stack().await.is_empty()
    }

    fn is_harness(item_stack: &ItemStack) -> Option<&EquippableImpl> {
        let equippable = item_stack.get_data_component::<EquippableImpl>()?;
        (*equippable.slot == EquipmentSlot::BODY
            && equippable.equip_on_interact
            && matches!(&equippable.allowed_entities, Some(IDSet::Tag(tag)) if tag.as_ref() == "can_equip_harness"))
        .then_some(equippable)
    }

    async fn try_equip_harness(&self, player: &Arc<Player>, item_stack: &mut ItemStack) -> bool {
        let Some(equippable) = Self::is_harness(item_stack) else {
            return false;
        };

        if self.is_wearing_body_armor().await {
            return false;
        }

        let equip_sound = match &equippable.equip_sound {
            IdOr::Id(sound) => *sound,
            IdOr::Value(_) => Sound::ItemArmorEquipGeneric,
        };

        let new_stack = item_stack.split_unless_creative(player.gamemode.load(), 1);
        {
            let mut equipment = self.mob_entity.living_entity.entity_equipment.lock().await;
            equipment.put(&EquipmentSlot::BODY, new_stack.clone())
        };
        self.mob_entity
            .living_entity
            .send_equipment_changes(&[(EquipmentSlot::BODY, new_stack)]);

        let entity = &self.mob_entity.living_entity.entity;
        let world = entity.world.load();
        world.play_sound(equip_sound, SoundCategory::Neutral, &entity.pos.load());
        true
    }

    async fn try_mount(&self, player: &Arc<Player>) -> bool {
        if !self.is_wearing_body_armor().await || player.get_entity().is_sneaking() {
            return false;
        }

        if player.get_entity().has_vehicle().await {
            return false;
        }

        let entity = &self.mob_entity.living_entity.entity;
        let passengers_len = entity.passengers.lock().await.len();
        // HappyGhast.java:339, MAX_PASSENGERS (vanilla misspells the constant name).
        if passengers_len >= 4 {
            return false;
        }

        let world = entity.world.load();
        let Some(vehicle) = world.get_entity_by_id(entity.entity_id) else {
            return false;
        };
        let Some(passenger) = world.get_player_by_id(player.entity_id()) else {
            return false;
        };

        if passengers_len == 0 {
            world.play_sound(
                Sound::EntityHappyGhastHarnessGogglesDown,
                SoundCategory::Neutral,
                &entity.pos.load(),
            );
        }

        entity
            .add_passenger(vehicle, passenger as Arc<dyn EntityBase>)
            .await;
        true
    }

    fn continuous_heal(&self) {
        let living = &self.mob_entity.living_entity;
        if living.dead.load(Relaxed) {
            return;
        }

        if living.health.load() >= living.get_max_health() {
            self.heal_ticks.store(0, Relaxed);
            return;
        }

        let ticks = self.heal_ticks.fetch_add(1, Relaxed) + 1;
        if ticks >= HEAL_INTERVAL_TICKS {
            self.heal_ticks.store(0, Relaxed);
            living.heal(1.0);
        }
    }

    // HappyGhast.java:452-459. Only called from `mob_tick` while not a vehicle
    // (`this.isVehicle()` there).
    async fn check_restriction(&self) {
        let entity = &self.mob_entity.living_entity.entity;
        if entity.leashed_to.lock().await.is_some() {
            return;
        }

        let is_baby = self.is_baby();
        let has_body_armor = self.is_wearing_body_armor().await;

        let radius = restriction_radius(is_baby, has_body_armor);
        self.mob_entity
            .position_target
            .store(entity.block_pos.load());
        self.mob_entity.position_target_range.store(radius, Relaxed);
    }

    pub fn set_server_still_timeout(&self, timeout: i32) {
        self.server_still_timeout.store(timeout, Ordering::Relaxed);
        self.sync_stay_still_flag();
    }

    #[must_use]
    pub fn is_on_still_timeout(&self) -> bool {
        self.stays_still.load(Ordering::Relaxed)
            || self.server_still_timeout.load(Ordering::Relaxed) > 0
    }

    fn set_leash_holder(&self, is_leash_holder: bool) {
        self.is_leash_holder
            .store(is_leash_holder, Ordering::Relaxed);
        let entity = self.get_entity();
        entity.send_meta_data(
            &[Metadata::new(
                pumpkin_data::tracked_data::happy_ghast::IS_LEASH_HOLDER,
                is_leash_holder,
            )],
            None,
        );
    }

    fn sync_stay_still_flag(&self) {
        let stays_still = self.server_still_timeout.load(Ordering::Relaxed) > 0;
        self.stays_still.store(stays_still, Ordering::Relaxed);
        let entity = self.get_entity();
        entity.send_meta_data(
            &[Metadata::new(
                pumpkin_data::tracked_data::happy_ghast::STAYS_STILL,
                stays_still,
            )],
            None,
        );
    }
}

impl AgeableMob for HappyGhastEntity {
    fn get_ageable_data(&self) -> &AgeableData {
        &self.ageable_data
    }

    fn baby_dimensions(&self) -> Option<EntityDimensions> {
        Some(EntityDimensions::new(0.95, 0.95, 0.46875))
    }
}

impl NBTStorage for HappyGhastEntity {
    fn write_nbt<'a>(&'a self, nbt: &'a mut NbtCompound) -> NbtFuture<'a, ()> {
        Box::pin(async move {
            self.mob_entity.living_entity.write_nbt(nbt).await;
            self.write_ageable_nbt(nbt);
            self.write_animal_nbt(nbt);
            nbt.put_int(
                "still_timeout",
                self.server_still_timeout.load(Ordering::Relaxed),
            );
        })
    }

    fn read_nbt_non_mut<'a>(&'a self, nbt: &'a NbtCompound) -> NbtFuture<'a, ()> {
        Box::pin(async move {
            self.mob_entity.living_entity.read_nbt_non_mut(nbt).await;
            self.read_ageable_nbt(nbt);
            self.read_animal_nbt(nbt);
            if let Some(timeout) = nbt.get_int("still_timeout") {
                self.set_server_still_timeout(timeout);
            }
        })
    }
}

impl Animal for HappyGhastEntity {
    fn is_food(&self, item_stack: &ItemStack) -> bool {
        item_stack
            .item
            .has_tag(&tag::Item::MINECRAFT_HAPPY_GHAST_FOOD)
            || HAPPY_GHAST_FOOD.iter().any(|i| i.id == item_stack.item.id)
    }
}

impl Mob for HappyGhastEntity {
    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    fn should_follow_leash(&self) -> bool {
        false
    }

    fn can_be_collided_with(&self) -> bool {
        // HappyGhast.canBeCollidedWith only exposes adult, alive ghasts to
        // collision queries. Babies must remain non-collidable while growing.
        !self.is_baby() && self.get_entity().is_alive()
    }

    fn get_mob_gravity(&self) -> f64 {
        0.0
    }

    fn mob_init_data_tracker(&self) -> EntityBaseFuture<'_, ()> {
        Box::pin(async move {
            let entity = self.get_entity();
            let is_baby = entity.age.load(Ordering::Relaxed) < 0;
            if is_baby {
                entity.send_meta_data(
                    &[Metadata::new(
                        pumpkin_data::tracked_data::happy_ghast::BABY_ID,
                        true,
                    )],
                    None,
                );
            }
            entity.send_meta_data(
                &[
                    Metadata::new(
                        pumpkin_data::tracked_data::happy_ghast::IS_LEASH_HOLDER,
                        self.is_leash_holder.load(Ordering::Relaxed),
                    ),
                    Metadata::new(
                        pumpkin_data::tracked_data::happy_ghast::STAYS_STILL,
                        self.stays_still.load(Ordering::Relaxed),
                    ),
                ],
                None,
            );
        })
    }

    fn mob_interact<'a>(
        &'a self,
        player: &'a Arc<Player>,
        item_stack: &'a mut ItemStack,
    ) -> EntityBaseFuture<'a, bool> {
        Box::pin(async move {
            // HappyGhast.java:281-283: babies never equip/ride, only eat to grow up.
            if self.is_baby() {
                use crate::entity::passive::animal::Animal as _;
                return self
                    .animal_interact(player, item_stack, Sound::EntityGhastlingAmbient)
                    .await;
            }

            if !item_stack.is_empty() && self.try_equip_harness(player, item_stack).await {
                return true;
            }

            if self.try_mount(player).await {
                return true;
            }

            self.mob_entity
                .mob_interact(player, item_stack, self.can_be_leashed())
                .await
        })
    }

    fn mob_tick<'a>(&'a self, _caller: &'a Arc<dyn EntityBase>) -> EntityBaseFuture<'a, ()> {
        Box::pin(async move {
            if !self.mob_entity.living_entity.entity.is_alive() {
                return;
            }

            self.ageable_ai_step();

            let leash_time = self.leash_holder_time.load(Relaxed);
            if leash_time > 0 {
                self.leash_holder_time.fetch_sub(1, Relaxed);
            }
            self.set_leash_holder(leash_time > 0);

            if self.server_still_timeout.load(Relaxed) > 0 {
                if self.get_entity().age.load(Relaxed) > 60 {
                    self.server_still_timeout.fetch_sub(1, Relaxed);
                }
                self.sync_stay_still_flag();
            }

            self.continuous_heal();

            let is_vehicle = !self
                .mob_entity
                .living_entity
                .entity
                .passengers
                .lock()
                .await
                .is_empty();
            if !is_vehicle {
                self.check_restriction().await;
            }
        })
    }
}

#[cfg(test)]
mod test {
    use super::restriction_radius;

    #[test]
    fn restriction_radius_matches_vanilla() {
        assert_eq!(restriction_radius(false, false), 64);
        assert_eq!(restriction_radius(false, true), 32);
        assert_eq!(restriction_radius(true, false), 32);
        assert_eq!(restriction_radius(true, true), 32);
    }
}
