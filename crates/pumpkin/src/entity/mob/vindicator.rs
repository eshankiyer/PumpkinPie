// Legacy invariant checks retained for vanilla behavior; migrate these paths before removing this allow.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::sync::{Arc, Weak};

use pumpkin_data::Enchantment;
use pumpkin_data::data_component_impl::EquipmentSlot;
use pumpkin_data::entity::EntityType;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_util::difficulty::Difficulty;
use rand::RngExt;

use crate::entity::{
    Entity, EntityBase, EntityBaseFuture, NBTStorage, NbtFuture,
    ai::goal::{
        active_target::ActiveTargetGoal,
        avoid_entity::AvoidEntityGoal,
        break_door::{self, BreakDoorGoal},
        interact_with_door::InteractWithDoorGoal,
        johnny_attack::JohnnyAttackGoal,
        look_at_entity::LookAtEntityGoal,
        melee_attack::MeleeAttackGoal,
        revenge::RevengeGoal,
        swim::SwimGoal,
        wander_around::WanderAroundGoal,
    },
    mob::{Mob, MobEntity, equipment::enchant_item_from_single_enchantment},
};
use crate::world::raid::num_groups_for_difficulty;

pub struct VindicatorEntity {
    pub mob_entity: MobEntity,
    /// Vanilla: `Vindicator.isJohnny`. One-way latch set (via `mob_tick`) once the vindicator is
    /// custom-named "Johnny".
    is_johnny: AtomicBool,
}

impl VindicatorEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let mob_entity = MobEntity::new(entity);
        let vindicator = Self {
            mob_entity,
            is_johnny: AtomicBool::new(false),
        };
        let mob_arc = Arc::new(vindicator);
        // Vanilla `populateDefaultEquipmentSlots`: only gives the axe `if getCurrentRaid() ==
        // null`; raid-spawned vindicators get their axe from `apply_raid_buffs` instead, called
        // by `Raid::spawn_group` right after construction, which overwrites this unconditionally.
        mob_arc
            .mob_entity
            .living_entity
            .entity_equipment
            .try_lock()
            .expect("new vindicator equipment is uncontended")
            .equipment
            .insert(EquipmentSlot::MAIN_HAND, ItemStack::new(1, &Item::IRON_AXE));
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
            // Vindicator.java:64: `AvoidEntityGoal<>(this, Creaking.class, 8.0F, 1.0, 1.2)`.
            goal_selector.add_goal(
                1,
                Box::new(AvoidEntityGoal::new(&EntityType::CREAKING, 8.0, 1.0, 1.2)),
            );
            goal_selector.add_goal(
                2,
                Box::new(BreakDoorGoal::new(break_door::normal_or_hard).raid_gated(true)),
            );
            goal_selector.add_goal(
                3,
                Box::new(InteractWithDoorGoal::new(false).raid_gated(true)),
            );
            // Vindicator.java:68: `MeleeAttackGoal(this, 1.0, false)`.
            goal_selector.add_goal(5, Box::new(MeleeAttackGoal::new(1.0, false)));
            // Vindicator.java:74-76: `RandomStrollGoal(this, 0.6)` at 8,
            // `LookAtPlayerGoal(this, Player.class, 3.0F, 1.0F)` at 9 and
            // `LookAtPlayerGoal(this, Mob.class, 8.0F)` at 10. Vanilla registers no
            // `RandomLookAroundGoal` for the vindicator.
            goal_selector.add_goal(8, Box::new(WanderAroundGoal::new(0.6)));
            goal_selector.add_goal(
                9,
                Box::new(LookAtEntityGoal::new(
                    mob_weak.clone(),
                    &EntityType::PLAYER,
                    3.0,
                    1.0,
                    false,
                )),
            );
            goal_selector.add_goal(
                10,
                LookAtEntityGoal::with_default_any_mob(mob_weak.clone(), 8.0),
            );

            let mut target_selector = mob_arc
                .mob_entity
                .target_selector
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            // Vindicator.java:69-73.
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
                    true,
                ),
            );
            target_selector.add_goal(
                3,
                ActiveTargetGoal::with_default(&mob_arc.mob_entity, &EntityType::IRON_GOLEM, true),
            );
            target_selector.add_goal(4, Box::new(JohnnyAttackGoal::new(&mob_arc.mob_entity)));
        };

        mob_arc
    }

    #[must_use]
    pub fn is_johnny(&self) -> bool {
        self.is_johnny.load(Relaxed)
    }
}

impl NBTStorage for VindicatorEntity {
    fn write_nbt<'a>(&'a self, nbt: &'a mut NbtCompound) -> NbtFuture<'a, ()> {
        Box::pin(async {
            self.mob_entity.living_entity.write_nbt(nbt).await;
            if self.is_johnny() {
                nbt.put_bool("Johnny", true);
            }
        })
    }

    fn read_nbt_non_mut<'a>(&'a self, nbt: &'a NbtCompound) -> NbtFuture<'a, ()> {
        Box::pin(async {
            self.mob_entity.living_entity.read_nbt_non_mut(nbt).await;
            if nbt.get_bool("Johnny") == Some(true) {
                self.is_johnny.store(true, Relaxed);
            }
        })
    }
}

impl Mob for VindicatorEntity {
    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    /// Vanilla: `Vindicator.setCustomName`'s one-way "Johnny" latch. Pumpkin has no per-mob
    /// custom-name-changed hook, so this lazily checks-and-latches every tick instead (up to one
    /// tick of latency versus vanilla's setter-time latch, unobservable to a player).
    /// Also `customServerAiStep`: `getNavigation().setCanOpenDoors(level.isRaided(pos))`,
    /// approximated with `has_active_raid()` (this mob's own raid membership) rather than
    /// re-querying the level for any raid covering this position -- same approximation
    /// `InteractWithDoorGoal::raid_gated`/`BreakDoorGoal` already use.
    fn mob_tick<'a>(&'a self, _caller: &'a Arc<dyn EntityBase>) -> EntityBaseFuture<'a, ()> {
        Box::pin(async move {
            if !self.is_johnny.load(Relaxed)
                && let Some(name) = &**self.mob_entity.living_entity.entity.custom_name.load()
                && name.clone().get_text() == "Johnny"
            {
                self.is_johnny.store(true, Relaxed);
            }

            let can_open_doors = self.mob_entity.living_entity.has_active_raid();
            self.mob_entity
                .navigator
                .lock()
                .unwrap()
                .set_can_open_doors(can_open_doors);
        })
    }

    /// Vanilla: `Vindicator.applyRaidBuffs` (`Vindicator.java:168-180`). Builds a fresh
    /// iron axe, rolls `random.nextFloat() <= raid.getEnchantOdds()` on the raider's own
    /// raid, and on success enchants the axe through the exact `SingleEnchantment`
    /// provider for the wave: `raid/vindicator` (Sharpness I) at waves
    /// `<= getNumGroups(Normal)` and `raid/vindicator_post_wave_5` (Sharpness II) after.
    /// The `DifficultyInstance` argument is threaded through by vanilla but unused by
    /// `SingleEnchantment`, so no regional difficulty is consulted here.
    fn apply_raid_buffs(&self, wave: i32, _is_captain: bool) -> EntityBaseFuture<'_, ()> {
        Box::pin(async move {
            let living = &self.mob_entity.living_entity;
            let mut axe = ItemStack::new(1, &Item::IRON_AXE);

            // Vindicator.java:170-171: `shouldEnchant =
            // this.random.nextFloat() <= raid.getEnchantOdds()` against
            // `getCurrentRaid()`. Pumpkin keeps only the raid membership cached on the
            // entity, so a missing membership/raid skips the roll entirely (vanilla can
            // never reach this method without an active raid).
            let should_enchant = {
                let world = living.entity.world.load();
                let raids = world.raids.lock().await;
                living
                    .raid_membership
                    .load()
                    .and_then(|membership| raids.raid(membership.raid_id))
                    .is_some_and(|raid| self.get_random().random::<f32>() <= raid.enchant_odds())
            };

            // Vindicator.java:173-176: provider selection is
            // `wave > raid.getNumGroups(Difficulty.NORMAL)`.
            if should_enchant {
                enchant_item_from_single_enchantment(
                    &mut axe,
                    &Enchantment::SHARPNESS,
                    raid_vindicator_sharpness_level(wave),
                );
            }

            // Vindicator.java:179: `setItemSlot(EquipmentSlot.MAINHAND, axe)` --
            // unconditional, enchanted or not.
            living
                .entity_equipment
                .lock()
                .await
                .put(&EquipmentSlot::MAIN_HAND, axe);
        })
    }
}

/// Vindicator.java:173-175: waves past `Raid.getNumGroups(Difficulty.NORMAL)`
/// (`Raid.java:786-793`, 5 on Normal) use `RAID_VINDICATOR_POST_WAVE_5`
/// (Sharpness II); all others use `RAID_VINDICATOR` (Sharpness I).
const fn raid_vindicator_sharpness_level(wave: i32) -> i32 {
    if wave > num_groups_for_difficulty(Difficulty::Normal) {
        2
    } else {
        1
    }
}

#[cfg(test)]
mod tests {
    use super::{Enchantment, Item, ItemStack, raid_vindicator_sharpness_level};
    use crate::entity::mob::equipment::enchant_item_from_single_enchantment;
    use crate::world::raid::num_groups_for_difficulty;
    use pumpkin_data::data_component_impl::EnchantmentsImpl;
    use pumpkin_util::difficulty::Difficulty;

    #[test]
    fn vindicator_provider_level_follows_vanilla_wave_threshold() {
        // Vindicator.java:173-175 with Raid.getNumGroups(NORMAL) == 5.
        assert_eq!(
            raid_vindicator_sharpness_level(num_groups_for_difficulty(Difficulty::Normal)),
            1
        );
        assert_eq!(
            raid_vindicator_sharpness_level(num_groups_for_difficulty(Difficulty::Normal) + 1),
            2
        );
        for wave in 1..=5 {
            assert_eq!(raid_vindicator_sharpness_level(wave), 1, "wave {wave}");
        }
        for wave in 6..=9 {
            assert_eq!(raid_vindicator_sharpness_level(wave), 2, "wave {wave}");
        }
    }

    #[test]
    fn raid_buffs_enchant_is_exact_single_sharpness() {
        // The providers are `SingleEnchantment(SHARPNESS, ConstantInt(1|2))`
        // (`VanillaEnchantmentProviders.java:28-29`), so a buffed axe carries exactly
        // Sharpness I or II -- never extra enchantments or other levels.
        let sharpness_id = u16::from(Enchantment::SHARPNESS.id);
        for (wave, expected) in [(3, 1), (7, 2)] {
            let mut axe = ItemStack::new(1, &Item::IRON_AXE);
            enchant_item_from_single_enchantment(
                &mut axe,
                &Enchantment::SHARPNESS,
                raid_vindicator_sharpness_level(wave),
            );
            let applied: Vec<(u16, i32)> = axe
                .get_data_component::<EnchantmentsImpl>()
                .map(|data| {
                    data.enchantment
                        .iter()
                        .map(|(enchantment, level)| (u16::from(enchantment.id), *level))
                        .collect()
                })
                .unwrap_or_default();
            assert_eq!(applied, vec![(sharpness_id, expected)]);
        }
    }
}
