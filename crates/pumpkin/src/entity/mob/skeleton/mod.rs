// Legacy invariant checks retained for vanilla behavior; migrate these paths before removing this allow.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use std::sync::{
    Arc, Weak,
    atomic::{AtomicBool, Ordering::Relaxed},
};

use pumpkin_data::{
    data_component_impl::{EquipmentSlot, StatusEffectInstance},
    entity::EntityType,
    item::Item,
    item_stack::ItemStack,
};
use pumpkin_util::Difficulty;

use crate::entity::{
    Entity, NBTStorage, NbtFuture,
    ai::goal::{
        active_target::ActiveTargetGoal, avoid_entity::AvoidEntityGoal, flee_sun::FleeSunGoal,
        look_around::RandomLookAroundGoal, look_at_entity::LookAtEntityGoal,
        melee_attack::MeleeAttackGoal, ranged_bow_attack::RangedBowAttackGoal,
        revenge::RevengeGoal, wander_around::WanderAroundGoal,
    },
    mob::{Mob, MobEntity},
};
use pumpkin_nbt::compound::NbtCompound;

pub mod bogged;
pub mod parched;
#[allow(clippy::module_inception)]
pub mod skeleton;
pub mod stray;
pub mod wither;

/// `AbstractSkeleton#getHardAttackInterval` (AbstractSkeleton.java).
pub const SKELETON_ATTACK_INTERVAL: i32 = 20;
const SKELETON_ATTACK_INTERVAL_NORMAL: i32 = 40;
/// `Parched#getHardAttackInterval` (Parched.java).
pub const PARCHED_ATTACK_INTERVAL: i32 = 50;
const PARCHED_ATTACK_INTERVAL_NORMAL: i32 = 70;

/// `Bogged#getHardAttackInterval` (Bogged.java).
pub const BOGGED_ATTACK_INTERVAL: i32 = 50;
const BOGGED_ATTACK_INTERVAL_NORMAL: i32 = 70;

fn attack_interval(entity_type: &'static EntityType, difficulty: Difficulty) -> i32 {
    let hard = difficulty == Difficulty::Hard;
    if entity_type == &EntityType::PARCHED {
        if hard {
            PARCHED_ATTACK_INTERVAL
        } else {
            PARCHED_ATTACK_INTERVAL_NORMAL
        }
    } else if entity_type == &EntityType::BOGGED {
        if hard {
            BOGGED_ATTACK_INTERVAL
        } else {
            BOGGED_ATTACK_INTERVAL_NORMAL
        }
    } else if hard {
        SKELETON_ATTACK_INTERVAL
    } else {
        SKELETON_ATTACK_INTERVAL_NORMAL
    }
}

/// `Parched#getArrow` (Parched.java) attaches `new MobEffectInstance(MobEffects.WEAKNESS, 600)`
/// to every arrow it fires; that constructor defaults to amplifier 0, non-ambient, with
/// particles and icon shown.
const PARCHED_ARROW_EFFECTS: &[StatusEffectInstance] = &[StatusEffectInstance {
    effect_id: std::borrow::Cow::Borrowed("minecraft:weakness"),
    amplifier: 0,
    duration: 600,
    ambient: false,
    show_particles: true,
    show_icon: true,
}];

/// `Bogged#getArrow` (Bogged.java) attaches `new MobEffectInstance(MobEffects.POISON, 100)` to
/// every arrow it fires.
pub const BOGGED_ARROW_EFFECTS: &[StatusEffectInstance] = &[StatusEffectInstance {
    effect_id: std::borrow::Cow::Borrowed("minecraft:poison"),
    amplifier: 0,
    duration: 100,
    ambient: false,
    show_particles: true,
    show_icon: true,
}];

/// `Stray.getArrow` adds `new MobEffectInstance(MobEffects.SLOWNESS, 600)` to ordinary arrows.
/// Vanilla: `Stray.java:60-64`.
pub const STRAY_ARROW_EFFECTS: &[StatusEffectInstance] = &[StatusEffectInstance {
    effect_id: std::borrow::Cow::Borrowed("minecraft:slowness"),
    amplifier: 0,
    duration: 600,
    ambient: false,
    show_particles: true,
    show_icon: true,
}];

pub struct SkeletonEntityBase {
    pub mob_entity: MobEntity,
    weapon_goal_is_bow: AtomicBool,
}

impl SkeletonEntityBase {
    pub fn new(entity: Entity) -> Arc<Self> {
        let uses_bow = entity.entity_type != &EntityType::WITHER_SKELETON;
        let is_parched = entity.entity_type == &EntityType::PARCHED;
        let is_bogged = entity.entity_type == &EntityType::BOGGED;
        let is_stray = entity.entity_type == &EntityType::STRAY;
        let mob_entity = MobEntity::new(entity);
        let mob = Self {
            mob_entity,
            weapon_goal_is_bow: AtomicBool::new(uses_bow),
        };
        let mob_arc = Arc::new(mob);
        let mob_weak: Weak<dyn Mob> = {
            let mob_arc: Arc<dyn Mob> = mob_arc.clone();
            Arc::downgrade(&mob_arc)
        };
        {
            // Vanilla `AbstractSkeleton#populateDefaultEquipmentSlots` equips a bow;
            // WitherSkeleton overrides that hook with a stone sword.
            let main_hand = if uses_bow {
                &Item::BOW
            } else {
                &Item::STONE_SWORD
            };
            mob_arc
                .mob_entity
                .living_entity
                .entity_equipment
                .try_lock()
                .expect("new skeleton equipment is uncontended")
                .put(&EquipmentSlot::MAIN_HAND, ItemStack::new(1, main_hand));

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

            goal_selector.add_goal(3, FleeSunGoal::new(1.0));
            // AbstractSkeleton.java:79: `AvoidEntityGoal<>(this, Wolf.class, 6.0F, 1.0, 1.2)`.
            goal_selector.add_goal(
                3,
                Box::new(AvoidEntityGoal::new(&EntityType::WOLF, 6.0, 1.0, 1.2)),
            );
            if uses_bow {
                // Vanilla `AbstractSkeleton#reassessWeaponGoal` selects this at priority 4.
                let interval = attack_interval(
                    mob_arc.mob_entity.living_entity.entity.entity_type,
                    mob_arc
                        .mob_entity
                        .living_entity
                        .entity
                        .world
                        .load()
                        .level_info
                        .load()
                        .difficulty,
                );
                let arrow_effects = if is_parched {
                    PARCHED_ARROW_EFFECTS
                } else if is_bogged {
                    BOGGED_ARROW_EFFECTS
                } else if is_stray {
                    STRAY_ARROW_EFFECTS
                } else {
                    &[][..]
                };
                goal_selector.add_goal(
                    4,
                    Box::new(RangedBowAttackGoal::with_arrow_effects(
                        interval,
                        15.0,
                        arrow_effects,
                    )),
                );
            } else {
                goal_selector.add_goal(4, Box::new(MeleeAttackGoal::new(1.2, false)));
            }
            // AbstractSkeleton.java:80-82: `WaterAvoidingRandomStrollGoal(this, 1.0)` at 5,
            // `LookAtPlayerGoal(this, Player.class, 8.0F)` and `RandomLookAroundGoal` at 6.
            goal_selector.add_goal(5, Box::new(WanderAroundGoal::new_water_avoiding(1.0)));
            goal_selector.add_goal(
                6,
                LookAtEntityGoal::with_default(mob_weak, &EntityType::PLAYER, 8.0),
            );
            goal_selector.add_goal(6, Box::new(RandomLookAroundGoal::default()));

            target_selector.add_goal(1, Box::new(RevengeGoal::new(true)));
            target_selector.add_goal(
                2,
                ActiveTargetGoal::with_default(&mob_arc.mob_entity, &EntityType::PLAYER, true),
            );
            target_selector.add_goal(
                3,
                ActiveTargetGoal::with_default(&mob_arc.mob_entity, &EntityType::IRON_GOLEM, true),
            );
        };

        mob_arc
    }

    pub async fn reassess_weapon_goal(&self, mob: &dyn Mob) {
        let main_hand = {
            let equipment = self.mob_entity.living_entity.entity_equipment.lock().await;
            equipment.get(&EquipmentSlot::MAIN_HAND)
        };
        let main_is_bow = main_hand.item.registry_key == Item::BOW.registry_key;
        let off_hand = {
            let equipment = self.mob_entity.living_entity.entity_equipment.lock().await;
            equipment.get(&EquipmentSlot::OFF_HAND)
        };
        let is_bow = main_is_bow || off_hand.item.registry_key == Item::BOW.registry_key;

        if self.weapon_goal_is_bow.swap(is_bow, Relaxed) == is_bow {
            return;
        }

        let mut goals = crate::entity::mob::MutexTakeGuard::new(&self.mob_entity.goals_selector);
        goals.remove_goal::<RangedBowAttackGoal>(mob).await;
        goals.remove_goal::<MeleeAttackGoal>(mob).await;
        if is_bow {
            let entity = &self.mob_entity.living_entity.entity;
            let interval = attack_interval(
                entity.entity_type,
                entity.world.load().level_info.load().difficulty,
            );
            let arrow_effects = if entity.entity_type == &EntityType::PARCHED {
                PARCHED_ARROW_EFFECTS
            } else if entity.entity_type == &EntityType::BOGGED {
                BOGGED_ARROW_EFFECTS
            } else if entity.entity_type == &EntityType::STRAY {
                STRAY_ARROW_EFFECTS
            } else {
                &[][..]
            };
            goals.add_goal(
                4,
                Box::new(RangedBowAttackGoal::with_arrow_effects(
                    interval,
                    15.0,
                    arrow_effects,
                )),
            );
        } else {
            goals.add_goal(4, Box::new(MeleeAttackGoal::new(1.2, false)));
        }
    }
}

impl NBTStorage for SkeletonEntityBase {
    fn write_nbt<'a>(&'a self, nbt: &'a mut NbtCompound) -> NbtFuture<'a, ()> {
        self.mob_entity.living_entity.write_nbt(nbt)
    }

    fn read_nbt_non_mut<'a>(&'a self, nbt: &'a NbtCompound) -> NbtFuture<'a, ()> {
        self.mob_entity.living_entity.read_nbt_non_mut(nbt)
    }
}

impl Mob for SkeletonEntityBase {
    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }
}

#[cfg(test)]
mod tests {
    use super::{
        BOGGED_ARROW_EFFECTS, BOGGED_ATTACK_INTERVAL, PARCHED_ARROW_EFFECTS,
        PARCHED_ATTACK_INTERVAL, SKELETON_ATTACK_INTERVAL,
    };
    use crate::item::potion::PotionContents;
    use pumpkin_data::data_component::DataComponent;
    use pumpkin_data::data_component_impl::{DataComponentImpl, PotionContentsImpl};
    use pumpkin_data::effect::StatusEffect;
    use pumpkin_data::item::Item;
    use pumpkin_data::item_stack::ItemStack;

    #[test]
    fn parched_shoots_slower_than_a_plain_skeleton() {
        assert_eq!(SKELETON_ATTACK_INTERVAL, 20);
        assert_eq!(PARCHED_ATTACK_INTERVAL, 50);
    }

    #[test]
    fn parched_arrows_carry_ten_second_weakness() {
        let mut arrow = ItemStack::new(1, &Item::ARROW);
        arrow.patch.push((
            DataComponent::PotionContents,
            Some(
                PotionContentsImpl {
                    potion_id: None,
                    custom_color: None,
                    custom_effects: PARCHED_ARROW_EFFECTS.to_vec(),
                    custom_name: None,
                }
                .to_dyn(),
            ),
        ));

        // `ArrowEntity::new_shot` stores `item_stack.copy_with_count(1)`, so the patch has
        // to survive that copy for the fired arrow to carry the effect.
        let arrow = arrow.copy_with_count(1);

        let effects = PotionContents::read_potion_effects(&arrow);
        assert_eq!(effects.len(), 1);
        let (effect_type, duration, amplifier, ambient, particles, icon) = effects[0];
        assert_eq!(effect_type.id, StatusEffect::WEAKNESS.id);
        assert_eq!(duration, 600);
        assert_eq!(amplifier, 0);
        assert!(!ambient);
        assert!(particles);
        assert!(icon);
    }

    #[test]
    fn bogged_shoots_at_the_hard_attack_interval() {
        assert_eq!(BOGGED_ATTACK_INTERVAL, 50);
    }

    #[test]
    fn bogged_arrows_carry_five_second_poison() {
        let mut arrow = ItemStack::new(1, &Item::ARROW);
        arrow.patch.push((
            DataComponent::PotionContents,
            Some(
                PotionContentsImpl {
                    potion_id: None,
                    custom_color: None,
                    custom_effects: BOGGED_ARROW_EFFECTS.to_vec(),
                    custom_name: None,
                }
                .to_dyn(),
            ),
        ));
        let arrow = arrow.copy_with_count(1);

        let effects = PotionContents::read_potion_effects(&arrow);
        assert_eq!(effects.len(), 1);
        let (effect_type, duration, amplifier, ambient, particles, icon) = effects[0];
        assert_eq!(effect_type.id, StatusEffect::POISON.id);
        assert_eq!(duration, 100);
        assert_eq!(amplifier, 0);
        assert!(!ambient);
        assert!(particles);
        assert!(icon);
    }
}
