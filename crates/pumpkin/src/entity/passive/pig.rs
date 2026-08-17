use std::sync::{Arc, Weak};

use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::Sound;
use pumpkin_data::{data_component_impl::EquipmentSlot, entity::EntityType, item::Item};
use pumpkin_util::math::boundingbox::EntityDimensions;

use crate::entity::{
    Entity, EntityBase, EntityBaseFuture, NBTStorage, NbtFuture,
    ageable::AgeableMob,
    ai::goal::{
        breed::BreedGoal, escape_danger::EscapeDangerGoal, follow_parent::FollowParentGoal,
        look_around::RandomLookAroundGoal, look_at_entity::LookAtEntityGoal, swim::SwimGoal,
        tempt::TemptGoal, wander_around::WanderAroundGoal,
    },
    mob::{Mob, MobEntity},
    passive::animal::Animal,
    player::Player,
};
use pumpkin_nbt::compound::NbtCompound;

const PIG_FOOD: &[&Item] = &[
    &Item::CARROT,
    &Item::POTATO,
    &Item::BEETROOT,
    &Item::CARROT_ON_A_STICK,
];

/// Represents a Pig, a common passive mob that provides porkchops.
///
/// Wiki: <https://minecraft.wiki/w/Pig>
pub struct PigEntity {
    pub mob_entity: MobEntity,
    pub ageable_data: crate::entity::ageable::AgeableData,
}

impl PigEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let mob_entity = MobEntity::new(entity);
        let pig = Self {
            mob_entity,
            ageable_data: crate::entity::ageable::AgeableData::default(),
        };
        let mob_arc = Arc::new(pig);
        let mob_weak: Weak<dyn Mob> = {
            let mob_arc: Arc<dyn Mob> = mob_arc.clone();
            Arc::downgrade(&mob_arc)
        };

        {
            let mut goal_selector = mob_arc.mob_entity.goals_selector.lock().unwrap();

            goal_selector.add_goal(0, Box::new(SwimGoal::default()));
            goal_selector.add_goal(1, EscapeDangerGoal::new(1.25));
            goal_selector.add_goal(2, BreedGoal::new(1.0));
            goal_selector.add_goal(3, Box::new(TemptGoal::new(1.2, PIG_FOOD, false)));
            goal_selector.add_goal(4, Box::new(FollowParentGoal::new(1.1)));
            goal_selector.add_goal(5, Box::new(WanderAroundGoal::new_water_avoiding(1.0)));
            goal_selector.add_goal(
                6,
                LookAtEntityGoal::with_default(mob_weak, &EntityType::PLAYER, 6.0),
            );
            goal_selector.add_goal(7, Box::new(RandomLookAroundGoal::default()));
        };

        mob_arc
    }
}

impl crate::entity::ageable::AgeableMob for PigEntity {
    fn get_ageable_data(&self) -> &crate::entity::ageable::AgeableData {
        &self.ageable_data
    }

    fn baby_dimensions(&self) -> Option<EntityDimensions> {
        Some(EntityDimensions::new(0.45, 0.45, 0.40625))
    }
}

impl NBTStorage for PigEntity {
    fn write_nbt<'a>(&'a self, nbt: &'a mut NbtCompound) -> NbtFuture<'a, ()> {
        Box::pin(async move {
            self.mob_entity.living_entity.write_nbt(nbt).await;
            self.write_ageable_nbt(nbt);
            self.write_animal_nbt(nbt);
        })
    }

    fn read_nbt_non_mut<'a>(&'a self, nbt: &'a NbtCompound) -> NbtFuture<'a, ()> {
        Box::pin(async move {
            self.mob_entity.living_entity.read_nbt_non_mut(nbt).await;
            self.read_ageable_nbt(nbt);
            self.read_animal_nbt(nbt);
        })
    }
}

impl super::animal::Animal for PigEntity {
    fn is_food(&self, item_stack: &ItemStack) -> bool {
        PIG_FOOD.iter().any(|i| i.id == item_stack.item.id)
    }
}

impl Mob for PigEntity {
    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    /// Vanilla `Pig.getControllingPassenger`: a saddled pig is controlled only by
    /// its first player passenger while that player holds a carrot on a stick.
    fn has_controlling_passenger(&self) -> EntityBaseFuture<'_, bool> {
        Box::pin(async move {
            let equipment = self.mob_entity.living_entity.entity_equipment.lock().await;
            let saddle = equipment.get(&EquipmentSlot::SADDLE);
            let saddle = saddle.lock().await;
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
            let main_hand = player.inventory().held_item().lock().await.item.id;
            if main_hand == Item::CARROT_ON_A_STICK.id {
                return true;
            }
            player
                .inventory()
                .off_hand_item()
                .await
                .lock()
                .await
                .item
                .id
                == Item::CARROT_ON_A_STICK.id
                || Mob::has_controlling_passenger(self).await
        })
    }

    fn mob_interact<'a>(
        &'a self,
        player: &'a Arc<Player>,
        item_stack: &'a mut ItemStack,
    ) -> EntityBaseFuture<'a, bool> {
        use super::animal::Animal;
        Box::pin(async move {
            let has_food = self.is_food(item_stack);
            let is_saddled = {
                let equipment = self.mob_entity.living_entity.entity_equipment.lock().await;
                let saddle = equipment.get(&EquipmentSlot::SADDLE);
                let saddle = saddle.lock().await;
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
                .animal_interact(player, item_stack, Sound::EntityPigAmbient)
                .await
            {
                return true;
            }

            let can_equip = {
                let equipment = self.mob_entity.living_entity.entity_equipment.lock().await;
                let saddle = equipment.get(&EquipmentSlot::SADDLE);
                saddle.lock().await.is_empty()
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
