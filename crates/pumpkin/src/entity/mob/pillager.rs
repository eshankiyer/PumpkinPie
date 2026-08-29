// Legacy invariant checks retained for vanilla behavior; migrate these paths before removing this allow.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use std::sync::{
    Arc, Mutex, Weak,
    atomic::{AtomicBool, Ordering},
};

use pumpkin_data::Enchantment;
use pumpkin_data::data_component_impl::EquipmentSlot;
use pumpkin_data::entity::EntityType;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::Sound;
use pumpkin_data::tracked_data;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_nbt::tag::NbtTag;
use pumpkin_protocol::java::client::play::Metadata;
use pumpkin_util::difficulty::Difficulty;
use rand::RngExt;

use crate::entity::{
    Entity, EntityBase,
    ai::goal::{
        active_target::ActiveTargetGoal, avoid_entity::AvoidEntityGoal,
        look_around::RandomLookAroundGoal,
        look_at_entity::LookAtEntityGoal, ranged_crossbow_attack::RangedCrossbowAttackGoal,
        revenge::RevengeGoal,
        swim::SwimGoal, wander_around::WanderAroundGoal,
    },
    mob::{
        Mob, MobEntity,
        equipment::enchant_item_from_single_enchantment,
        crossbow_attack_mob::CrossbowAttackMob,
        patrol::{LongDistancePatrolGoal, PatrolData, PatrollingMonster},
        raider::{
            HoldGroundAttackGoal, ObtainRaidLeaderBannerGoal, PathfindToRaidGoal, Raider,
            RaiderCelebrationGoal, RaiderData, RaiderMoveThroughVillageGoal,
        },
    },
};
use crate::world::raid::num_groups_for_difficulty;

pub struct PillagerEntity {
    pub mob_entity: MobEntity,
    pub raider_data: RaiderData,
    pub is_charging_crossbow: AtomicBool,
    pub inventory: Mutex<Vec<ItemStack>>,
}

impl PillagerEntity {
    pub const INVENTORY_SIZE: usize = 5;

    #[must_use]
    pub fn new(entity: Entity) -> Arc<Self> {
        let mob_arc = Arc::new(Self {
            mob_entity: MobEntity::new(entity),
            raider_data: RaiderData::default(),
            is_charging_crossbow: AtomicBool::new(false),
            inventory: Mutex::new(Vec::new()),
        });
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
            goal_selector.add_goal(1, Box::new(ObtainRaidLeaderBannerGoal));
            goal_selector.add_goal(2, Box::new(HoldGroundAttackGoal::new(10.0)));
            goal_selector.add_goal(3, Box::new(RangedCrossbowAttackGoal::new(1.0, 8.0)));
            goal_selector.add_goal(4, Box::new(LongDistancePatrolGoal::new(0.7, 0.595)));
            goal_selector.add_goal(4, Box::new(RaiderMoveThroughVillageGoal::new(1.05)));
            goal_selector.add_goal(4, Box::new(PathfindToRaidGoal::default()));
            goal_selector.add_goal(5, Box::new(RaiderCelebrationGoal));
            goal_selector.add_goal(5, Box::new(WanderAroundGoal::new(1.0)));
            goal_selector.add_goal(
                6,
                LookAtEntityGoal::with_default(mob_weak, &EntityType::PLAYER, 8.0),
            );
            goal_selector.add_goal(7, Box::new(RandomLookAroundGoal::default()));
            goal_selector.add_goal(10, LookAtEntityGoal::with_default_any_mob(
                {
                    let mob_arc: Arc<dyn Mob> = mob_arc.clone();
                    Arc::downgrade(&mob_arc)
                },
                15.0,
            ));

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

    #[must_use]
    pub fn is_charging_crossbow(&self) -> bool {
        self.is_charging_crossbow.load(Ordering::Relaxed)
    }

    pub fn set_charging_crossbow(&self, is_charging: bool) {
        self.is_charging_crossbow
            .store(is_charging, Ordering::Relaxed);
        self.mob_entity.living_entity.entity.send_meta_data(
            &[Metadata::new(
                tracked_data::pillager::IS_CHARGING_CROSSBOW,
                is_charging,
            )],
            None,
        );
    }

    pub fn add_to_inventory(&self, item: ItemStack) -> Option<ItemStack> {
        let mut inv = self
            .inventory
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if inv.len() < Self::INVENTORY_SIZE {
            inv.push(item);
            None
        } else {
            Some(item)
        }
    }

    pub fn drop_inventory(&self) {
        let items = self
            .inventory
            .try_lock()
            .map_or_else(|_| Vec::new(), |mut inv| std::mem::take(&mut *inv));
        let entity = &self.mob_entity.living_entity.entity;
        let world = entity.world.load();
        let pos = entity.pos.load();
        for item in items {
            if !item.is_empty() {
                let item_entity = crate::entity::item::ItemEntity::new(
                    Entity::new(world.clone(), pos, &EntityType::ITEM),
                    item,
                );
                world.spawn_entity(Arc::new(item_entity));
            }
        }
    }
}

impl Mob for PillagerEntity {
    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
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
    fn as_patrolling_monster(&self) -> Option<&dyn PatrollingMonster> {
        Some(self)
    }

    fn as_raider(&self) -> Option<&dyn Raider> {
        Some(self)
    }

    fn as_crossbow_attack_mob(&self) -> Option<&dyn CrossbowAttackMob> {
        Some(self)
    }

    fn mob_init_data_tracker(&self) {
        self.set_charging_crossbow(self.is_charging_crossbow.load(Ordering::Relaxed));
    }

    fn mob_write_nbt(&self, nbt: &mut NbtCompound) {
        self.write_raider_nbt(nbt);
        nbt.put_bool("CanPickUpLoot", true);

        let inv = self
            .inventory
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !inv.is_empty() {
            let mut items_tag = Vec::new();
            for item in inv.iter() {
                if !item.is_empty() {
                    let mut item_nbt = NbtCompound::new();
                    item.write_item_stack(&mut item_nbt);
                    items_tag.push(NbtTag::Compound(item_nbt));
                }
            }
            if !items_tag.is_empty() {
                nbt.put_list("Inventory", items_tag);
            }
        }
    }

    fn mob_read_nbt(&self, nbt: &NbtCompound) {
        self.read_raider_nbt(nbt);

        if let Some(inv_list) = nbt.get_list("Inventory") {
            let mut inv = self
                .inventory
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            inv.clear();
            for tag in inv_list {
                if let Some(compound) = tag.extract_compound()
                    && let Some(stack) = ItemStack::read_item_stack(compound)
                {
                    inv.push(stack);
                }
            }
        }
    }

    fn on_damage(
        &self,
        _damage_type: pumpkin_data::damage::DamageType,
        _source: Option<&dyn EntityBase>,
    ) {
        if self.mob_entity.living_entity.dead.load(Ordering::Relaxed) {
            self.drop_inventory();
        }
    }
}

impl PatrollingMonster for PillagerEntity {
    fn get_patrol_data(&self) -> &PatrolData {
        &self.raider_data.patrol_data
    }
}

impl Raider for PillagerEntity {
    fn get_raider_data(&self) -> &RaiderData {
        &self.raider_data
    }

    fn get_celebrate_sound(&self) -> Sound {
        Sound::EntityPillagerCelebrate
    }
}

impl CrossbowAttackMob for PillagerEntity {
    fn set_charging_crossbow(&self, is_charging: bool) {
        self.set_charging_crossbow(is_charging);
    }

    fn is_charging_crossbow(&self) -> bool {
        self.is_charging_crossbow()
    }
}
