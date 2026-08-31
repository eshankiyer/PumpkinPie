use std::sync::{Arc, Weak};

use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::Sound;
use pumpkin_data::{data_component_impl::EquipmentSlot, entity::EntityType, item::Item};
use pumpkin_util::math::boundingbox::EntityDimensions;

use crate::entity::ai::pathfinder::node::PathType;
use crate::entity::item_steerable::{ItemBasedSteering, ItemSteerable};
use crate::entity::{
    Entity, EntityBase, EntityBaseFuture, NBTStorage, NbtFuture,
    ageable::AgeableMob,
    ai::goal::{
        breed::BreedGoal, escape_danger::EscapeDangerGoal, follow_parent::FollowParentGoal,
        look_around::RandomLookAroundGoal, look_at_entity::LookAtEntityGoal,
        strider_go_to_lava::StriderGoToLavaGoal, tempt::TemptGoal, wander_around::WanderAroundGoal,
    },
    mob::{Mob, MobEntity},
    passive::animal::Animal,
    player::Player,
};
use pumpkin_nbt::compound::NbtCompound;

/// `strider_tempt_items` tag (`#strider_food` + `warped_fungus_on_a_stick`).
const STRIDER_TEMPT_ITEMS: &[&Item] = &[&Item::WARPED_FUNGUS, &Item::WARPED_FUNGUS_ON_A_STICK];

/// Represents a Strider, a passive mob that walks on lava.
///
/// Wiki: <https://minecraft.wiki/w/Strider>
pub struct StriderEntity {
    pub mob_entity: MobEntity,
    pub ageable_data: crate::entity::ageable::AgeableData,
    pub steering: ItemBasedSteering,
    pub saddled: std::sync::atomic::AtomicBool,
}

impl StriderEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let mob_entity = MobEntity::new(entity);
        let strider = Self {
            mob_entity,
            ageable_data: crate::entity::ageable::AgeableData::default(),
            steering: ItemBasedSteering::default(),
            saddled: std::sync::atomic::AtomicBool::new(false),
        };
        let mob_arc = Arc::new(strider);
        // `Strider` constructor (`Strider.java:93-100`) supplies these pathfinding maluses.
        #[allow(clippy::semicolon_if_nothing_returned)]
        {
            let mut navigator = mob_arc.mob_entity.navigator.lock().unwrap();
            navigator.set_pathfinding_malus(PathType::Water, -1.0);
            navigator.set_pathfinding_malus(PathType::Lava, 0.0);
            navigator.set_pathfinding_malus(PathType::DangerFire, 0.0);
            navigator.set_pathfinding_malus(PathType::DamageFire, 0.0)
        };
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

            // Vanilla `Strider.registerGoals` has no float/swim goal.
            goal_selector.add_goal(1, EscapeDangerGoal::new(1.65));
            goal_selector.add_goal(2, BreedGoal::new(1.0));
            goal_selector.add_goal(3, Box::new(TemptGoal::new(1.4, STRIDER_TEMPT_ITEMS, false)));
            goal_selector.add_goal(4, StriderGoToLavaGoal::new(1.0));
            goal_selector.add_goal(5, Box::new(FollowParentGoal::new(1.0)));
            goal_selector.add_goal(7, Box::new(WanderAroundGoal::new_with_interval(1.0, 60)));
            goal_selector.add_goal(
                8,
                LookAtEntityGoal::with_default(mob_weak.clone(), &EntityType::PLAYER, 8.0),
            );
            goal_selector.add_goal(8, Box::new(RandomLookAroundGoal::default()));
            goal_selector.add_goal(
                9,
                LookAtEntityGoal::with_default(mob_weak, &EntityType::STRIDER, 8.0),
            );
        };

        mob_arc
    }
}

impl AgeableMob for StriderEntity {
    fn get_ageable_data(&self) -> &crate::entity::ageable::AgeableData {
        &self.ageable_data
    }

    fn baby_dimensions(&self) -> Option<EntityDimensions> {
        Some(EntityDimensions::new(0.45, 0.85, 0.4375))
    }
}

impl NBTStorage for StriderEntity {
    fn write_nbt<'a>(&'a self, nbt: &'a mut NbtCompound) -> NbtFuture<'a, ()> {
        Box::pin(async move {
            self.mob_entity.living_entity.write_nbt(nbt).await;
            self.write_ageable_nbt(nbt);
            self.write_animal_nbt(nbt);
            nbt.put_bool("Saddle", self.is_saddled());
        })
    }

    fn read_nbt_non_mut<'a>(&'a self, nbt: &'a NbtCompound) -> NbtFuture<'a, ()> {
        Box::pin(async move {
            self.mob_entity.living_entity.read_nbt_non_mut(nbt).await;
            self.read_ageable_nbt(nbt);
            self.read_animal_nbt(nbt);
            if let Some(saddle) = nbt.get_byte("Saddle") {
                self.set_saddled(saddle == 1);
            }
        })
    }
}

impl Animal for StriderEntity {
    /// `strider_food` tag: warped fungus only (the tempt-item tag is wider, adding
    /// warped-fungus-on-a-stick, but that item isn't food for breeding purposes).
    fn is_food(&self, item_stack: &ItemStack) -> bool {
        item_stack.item.id == Item::WARPED_FUNGUS.id
    }
}

impl Mob for StriderEntity {
    /// `Strider.shouldPassengersInheritMalus` (`Strider.java:324-327`) lets controlled mobs use
    /// the strider's lava-safe pathfinding costs.
    fn should_passengers_inherit_malus(&self) -> bool {
        true
    }

    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    /// `Strider.isSensitiveToWater` (`Strider.java:371-373`).
    fn mob_is_sensitive_to_water(&self) -> bool {
        true
    }

    fn get_item_steerable(&self) -> Option<&dyn ItemSteerable> {
        Some(self)
    }

    fn is_saddled(&self) -> bool {
        self.saddled.load(std::sync::atomic::Ordering::Relaxed)
    }

    fn can_be_saddled(&self) -> bool {
        self.mob_entity.living_entity.entity.is_alive()
    }

    fn set_saddled(&self, saddled: bool) {
        self.saddled
            .store(saddled, std::sync::atomic::Ordering::Relaxed);
    }

    /// Vanilla `Strider.getControllingPassenger`: a saddled strider is controlled
    /// only by its first player passenger while holding warped fungus on a stick.
    fn has_controlling_passenger(&self) -> EntityBaseFuture<'_, bool> {
        Box::pin(async move {
            let equipment = self.mob_entity.living_entity.entity_equipment.lock().await;
            let saddle = equipment.get(&EquipmentSlot::SADDLE);
            let saddled = self.get_entity().is_alive()
                && !self.is_baby()
                && super::equine::is_valid_saddle_item(&saddle, self.get_entity().entity_type);
            drop(equipment);
            if !saddled {
                return Mob::has_controlling_passenger(self).await;
            }

            let passenger = self.get_entity().passengers.lock().await.first().cloned();
            let Some(passenger) = passenger else {
                return Mob::has_controlling_passenger(self).await;
            };
            let Some(player) = passenger.get_player() else {
                return Mob::has_controlling_passenger(self).await;
            };
            let main_hand = player.inventory().held_item().await.item.id;
            if main_hand == Item::WARPED_FUNGUS_ON_A_STICK.id {
                return true;
            }
            player.inventory().off_hand_item().await.item.id == Item::WARPED_FUNGUS_ON_A_STICK.id
                || Mob::has_controlling_passenger(self).await
        })
    }

    fn mob_interact<'a>(
        &'a self,
        player: &'a Arc<Player>,
        item_stack: &'a mut ItemStack,
    ) -> EntityBaseFuture<'a, bool> {
        Box::pin(async move {
            let has_food = self.is_food(item_stack);
            let is_saddled = {
                let equipment = self.mob_entity.living_entity.entity_equipment.lock().await;
                let saddle = equipment.get(&EquipmentSlot::SADDLE);
                self.get_entity().is_alive()
                    && !self.is_baby()
                    && super::equine::is_valid_saddle_item(&saddle, self.get_entity().entity_type)
            };
            if !has_food
                && is_saddled
                && self.get_entity().passengers.lock().await.is_empty()
                && !player.get_entity().is_sneaking()
            {
                super::equine::mount_player(&self.mob_entity, player).await;
                return true;
            }

            if self
                .animal_interact(player, item_stack, Sound::EntityStriderEat)
                .await
            {
                return true;
            }

            let can_equip = {
                let equipment = self.mob_entity.living_entity.entity_equipment.lock().await;
                let saddle = equipment.get(&EquipmentSlot::SADDLE);
                saddle.is_empty()
                    && self.get_entity().is_alive()
                    && !self.is_baby()
                    && super::equine::saddle_equip_on_interact(
                        item_stack,
                        self.get_entity().entity_type,
                    )
            };
            if can_equip {
                super::equine::equip_saddle_item(&self.mob_entity, player, item_stack).await;
                return true;
            }
            false
        })
    }
}

impl ItemSteerable for StriderEntity {
    fn boost(&self) -> bool {
        self.steering.boost()
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}
