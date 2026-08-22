// Legacy invariant checks retained for vanilla behavior; migrate these paths before removing this allow.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::sync::{Arc, Weak};

use pumpkin_data::data_component_impl::EquipmentSlot;
use pumpkin_data::entity::EntityType;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::tracked_data;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_nbt::tag::NbtTag;
use pumpkin_protocol::java::client::play::Metadata;
use tokio::sync::Mutex;

use crate::entity::{
    Entity, NBTStorage, NbtFuture,
    ai::goal::{
        active_target::ActiveTargetGoal, avoid_entity::AvoidEntityGoal,
        look_at_entity::LookAtEntityGoal, ranged_crossbow_attack::RangedCrossbowAttackGoal,
        revenge::RevengeGoal, swim::SwimGoal, wander_around::WanderAroundGoal,
    },
    mob::{Mob, MobEntity},
};

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
