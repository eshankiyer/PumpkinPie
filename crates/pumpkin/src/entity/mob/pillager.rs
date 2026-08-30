// Legacy invariant checks retained for vanilla behavior; migrate these paths before removing this allow.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::sync::{Arc, Weak};

use pumpkin_data::Enchantment;
use pumpkin_data::data_component_impl::EquipmentSlot;
use pumpkin_data::entity::EntityType;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::tracked_data;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_nbt::tag::NbtTag;
use pumpkin_protocol::java::client::play::Metadata;
use pumpkin_util::difficulty::Difficulty;
use rand::RngExt;
use tokio::sync::Mutex;

use crate::entity::{
    Entity, EntityBaseFuture, NBTStorage, NbtFuture,
    ai::goal::{
        active_target::ActiveTargetGoal, avoid_entity::AvoidEntityGoal,
        look_at_entity::LookAtEntityGoal, pathfind_to_raid::PathfindToRaidGoal,
        ranged_crossbow_attack::RangedCrossbowAttackGoal, revenge::RevengeGoal, swim::SwimGoal,
        wander_around::WanderAroundGoal,
    },
    mob::{Mob, MobEntity, equipment::enchant_item_from_single_enchantment},
};
use crate::world::raid::num_groups_for_difficulty;

const BANNER_INVENTORY_SIZE: usize = 5;

pub struct PillagerEntity {
    pub mob_entity: MobEntity,
    /// Vanilla: `Pillager.IS_CHARGING_CROSSBOW` synced data.
    is_charging_crossbow: AtomicBool,
    /// Vanilla: `Pillager.inventory` (a private 5-slot `SimpleContainer` that only ever holds
    /// plain white banners picked up during an active raid, for the "ominous banner leader"
    /// hand-off mechanic).
    ///
    /// Scope reduction: `Pillager.pickUpItem`'s "else" branch (the actual banner-pickup logic
    /// this inventory exists for) is not wired up -- Pumpkin has no generic mob item-pickup
    /// pipeline anywhere yet (confirmed by a repo-wide search finding no `pick_up_item`/
    /// `on_item_pickup` hook on `Mob`/`MobEntity`), so nothing currently populates this. The
    /// field and its NBT round-trip exist so a future item-pickup pass has a slot to fill.
    banner_inventory: Mutex<[Option<ItemStack>; BANNER_INVENTORY_SIZE]>,
}

impl PillagerEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let mob_entity = MobEntity::new(entity);
        let pillager = Self {
            mob_entity,
            is_charging_crossbow: AtomicBool::new(false),
            banner_inventory: Mutex::new(std::array::from_fn(|_| None)),
        };
        let mob_arc = Arc::new(pillager);
        mob_arc
            .mob_entity
            .living_entity
            .entity_equipment
            .try_lock()
            .expect("new pillager equipment is uncontended")
            .equipment
            .insert(EquipmentSlot::MAIN_HAND, ItemStack::new(1, &Item::CROSSBOW));
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
            // Pillager.java:72: `AvoidEntityGoal<>(this, Creaking.class, 8.0F, 1.0, 1.2)`.
            goal_selector.add_goal(
                1,
                Box::new(AvoidEntityGoal::new(&EntityType::CREAKING, 8.0, 1.0, 1.2)),
            );
            // Raider.java:65, via `super.registerGoals()`: `PathfindToRaidGoal<>(this)`.
            goal_selector.add_goal(3, PathfindToRaidGoal::new());
            // Pillager.java:74: `RangedCrossbowAttackGoal<>(this, 1.0, 8.0F)`.
            goal_selector.add_goal(3, Box::new(RangedCrossbowAttackGoal::new(8.0)));
            // Pillager.java:75-77: `RandomStrollGoal(this, 0.6)` at 8,
            // `LookAtPlayerGoal(this, Player.class, 15.0F, 1.0F)` at 9 and
            // `LookAtPlayerGoal(this, Mob.class, 15.0F)` at 10. Vanilla registers no
            // `RandomLookAroundGoal` for the pillager.
            goal_selector.add_goal(8, Box::new(WanderAroundGoal::new(0.6)));
            goal_selector.add_goal(
                9,
                Box::new(LookAtEntityGoal::new(
                    mob_weak.clone(),
                    &EntityType::PLAYER,
                    15.0,
                    1.0,
                    false,
                )),
            );
            goal_selector.add_goal(
                10,
                LookAtEntityGoal::with_default_any_mob(mob_weak.clone(), 15.0),
            );

            let mut target_selector = mob_arc
                .mob_entity
                .target_selector
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            // Pillager.java:78: `HurtByTargetGoal(this, Raider.class).setAlertOthers()`.
            target_selector.add_goal(1, Box::new(RevengeGoal::new(true).exclude_raiders()));
            target_selector.add_goal(
                2,
                ActiveTargetGoal::with_default(&mob_arc.mob_entity, &EntityType::PLAYER, true),
            );
            target_selector.add_goal(
                3,
                ActiveTargetGoal::with_default_types(
                    &mob_arc.mob_entity,
                    &[&EntityType::VILLAGER, &EntityType::WANDERING_TRADER],
                    false,
                ),
            );
            target_selector.add_goal(
                3,
                ActiveTargetGoal::with_default(&mob_arc.mob_entity, &EntityType::IRON_GOLEM, true),
            );
        };

        mob_arc
    }
}

impl NBTStorage for PillagerEntity {
    fn write_nbt<'a>(&'a self, nbt: &'a mut NbtCompound) -> NbtFuture<'a, ()> {
        Box::pin(async {
            self.mob_entity.living_entity.write_nbt(nbt).await;

            let inventory = self.banner_inventory.lock().await;
            let mut items = Vec::with_capacity(BANNER_INVENTORY_SIZE);
            for slot in inventory.iter() {
                let mut item_nbt = NbtCompound::new();
                if let Some(stack) = slot {
                    stack.write_item_stack(&mut item_nbt);
                }
                items.push(NbtTag::Compound(item_nbt));
            }
            nbt.put("Inventory", NbtTag::List(items));
        })
    }

    fn read_nbt_non_mut<'a>(&'a self, nbt: &'a NbtCompound) -> NbtFuture<'a, ()> {
        Box::pin(async {
            self.mob_entity.living_entity.read_nbt_non_mut(nbt).await;

            if let Some(NbtTag::List(items)) = nbt.get("Inventory") {
                let mut inventory = self.banner_inventory.lock().await;
                for (slot, tag) in inventory.iter_mut().zip(items.iter()) {
                    *slot = tag
                        .extract_compound()
                        .and_then(ItemStack::read_item_stack)
                        .filter(|stack| !stack.is_empty());
                }
            }
        })
    }
}

impl Mob for PillagerEntity {
    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    /// Vanilla `Pillager.canUseNonMeleeWeapon` (`Pillager.java:97-101`) permits its crossbow
    /// attack behavior to select a crossbow as a non-melee weapon.
    fn can_use_non_melee_weapon(&self, item: &ItemStack) -> bool {
        item.item.id == Item::CROSSBOW.id
    }

    /// Vanilla: `Pillager.enchantSpawnedWeapon` (`Pillager.java:172-181`). After the generic
    /// `MOB_SPAWN_EQUIPMENT` weapon roll has run on the freshly equipped crossbow,
    /// `this.random.nextInt(300) == 0` enchants it through the `pillager_spawn_crossbow`
    /// provider (`VanillaEnchantmentProviders.java:25`) — a `SingleEnchantment(Piercing,
    /// ConstantInt.of(1))` (`VanillaEnchantmentProviders.java:28`, `piercing` at level 1), so
    /// the crossbow carries exactly Piercing I.
    fn enchant_spawned_weapon(&self, main_hand: &mut ItemStack) {
        if self.get_random().random_range(0..300) == 0 {
            enchant_item_from_single_enchantment(main_hand, &Enchantment::PIERCING, 1);
        }
    }

    /// Vanilla: `Pillager.applyRaidBuffs` (`Pillager.java:234-256`). Rolls
    /// `random.nextFloat() <= raid.getEnchantOdds()` against the raider's own raid, and only
    /// on success builds a fresh crossbow enchanted through the wave-selected provider
    /// (`VanillaEnchantmentProviders.java:26-27`): waves past `getNumGroups(NORMAL)` use
    /// `raid/pillager_post_wave_5` (Quick Charge II), waves past `getNumGroups(EASY)` use
    /// `raid/pillager_post_wave_3` (Quick Charge I); below those thresholds no provider
    /// applies and the pillager keeps its current crossbow. Unlike a vindicator's axe, an
    /// unenchanted roll leaves the slot untouched (`Pillager.java:248-253`).
    fn apply_raid_buffs(&self, wave: i32, _is_captain: bool) -> EntityBaseFuture<'_, ()> {
        Box::pin(async move {
            // `this.getCurrentRaid()` / `raid.getEnchantOdds()` (`Pillager.java:238-239`).
            // Pumpkin keeps only the raid membership cached on the entity, so a missing
            // membership/raid skips the roll entirely (vanilla can never reach this method
            // without an active raid).
            let living = &self.mob_entity.living_entity;
            let should_enchant = {
                let world = living.entity.world.load();
                let raids = world.raids.lock().await;
                living
                    .raid_membership
                    .load()
                    .and_then(|membership| raids.raid(membership.raid_id))
                    .is_some_and(|raid| self.get_random().random::<f32>() <= raid.enchant_odds())
            };
            if !should_enchant {
                return;
            }

            // `Pillager.java:240-245`: provider selection by wave threshold; `null` keeps
            // the existing crossbow.
            let quick_charge_level = if wave > num_groups_for_difficulty(Difficulty::Normal) {
                2
            } else if wave > num_groups_for_difficulty(Difficulty::Easy) {
                1
            } else {
                return;
            };

            let mut crossbow = ItemStack::new(1, &Item::CROSSBOW);
            enchant_item_from_single_enchantment(
                &mut crossbow,
                &Enchantment::QUICK_CHARGE,
                quick_charge_level,
            );
            // `this.setItemSlot(EquipmentSlot.MAINHAND, crossbow)` (`Pillager.java:252`),
            // synced to observers the same way other direct equipment writes broadcast.
            living
                .entity_equipment
                .lock()
                .await
                .put(&EquipmentSlot::MAIN_HAND, crossbow.clone());
            living.send_equipment_changes(&[(EquipmentSlot::MAIN_HAND, crossbow)]);
        })
    }

    /// Vanilla: `Pillager.setChargingCrossbow`. Drives `getArmPose()`'s `CROSSBOW_CHARGE` state
    /// client-side (not ported -- see the illager-family "no client rendering" scope cut), but is
    /// still a real synced-data write other clients observe (visible crossbow draw animation).
    fn set_charging_crossbow(&self, charging: bool) {
        if self.is_charging_crossbow.swap(charging, Relaxed) != charging {
            self.mob_entity.living_entity.entity.send_meta_data(
                &[Metadata::new(
                    tracked_data::pillager::IS_CHARGING_CROSSBOW,
                    charging,
                )],
                None,
            );
        }
    }
}
