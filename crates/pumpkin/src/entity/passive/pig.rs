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
    mob::{Mob, MobEntity, zombified_piglin::ZombifiedPiglinEntity},
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

use crate::entity::item_steerable::{ItemBasedSteering, ItemSteerable};

/// Represents a Pig, a common passive mob that provides porkchops.
///
/// Wiki: <https://minecraft.wiki/w/Pig>
pub struct PigEntity {
    pub mob_entity: MobEntity,
    pub ageable_data: crate::entity::ageable::AgeableData,
    pub steering: ItemBasedSteering,
    pub saddled: std::sync::atomic::AtomicBool,
}

impl PigEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let mob_entity = MobEntity::new(entity);
        let pig = Self {
            mob_entity,
            ageable_data: crate::entity::ageable::AgeableData::default(),
            steering: ItemBasedSteering::default(),
            saddled: std::sync::atomic::AtomicBool::new(false),
        };
        let mob_arc = Arc::new(pig);
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

impl super::animal::Animal for PigEntity {
    fn is_food(&self, item_stack: &ItemStack) -> bool {
        use pumpkin_data::tag::Taggable;
        item_stack
            .item
            .has_tag(&pumpkin_data::tag::Item::MINECRAFT_PIG_FOOD)
            || PIG_FOOD.iter().any(|i| i.id == item_stack.item.id)
    }
}

impl Mob for PigEntity {
    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    fn get_item_steerable(&self) -> Option<&dyn ItemSteerable> {
        Some(self)
    }

    fn is_saddled(&self) -> bool {
        self.saddled.load(std::sync::atomic::Ordering::Relaxed)
    }

    fn can_be_saddled(&self) -> bool {
        use crate::entity::ageable::AgeableMob;
        self.mob_entity.living_entity.entity.is_alive() && !self.is_baby()
    }

    fn set_saddled(&self, saddled: bool) {
        self.saddled
            .store(saddled, std::sync::atomic::Ordering::Relaxed);
    }

    /// Vanilla `Pig.getControllingPassenger` (`Pig.java:96-101`): a saddled pig is controlled
    /// only by its first player passenger while that player holds a carrot on a stick.
    fn has_controlling_passenger(&self) -> EntityBaseFuture<'_, bool> {
        Box::pin(async move {
            let saddle = {
                let equipment = self.mob_entity.living_entity.entity_equipment.lock().await;
                equipment.get(&EquipmentSlot::SADDLE)
            };
            let saddled = self.get_entity().is_alive()
                && !self.is_baby()
                && super::equine::is_valid_saddle_item(&saddle, self.get_entity().entity_type);
            if !saddled {
                return self.default_has_controlling_passenger().await;
            }

            let passenger = self.get_entity().passengers.lock().await.first().cloned();
            let Some(passenger) = passenger else {
                return self.default_has_controlling_passenger().await;
            };
            let Some(player) = passenger.get_player() else {
                return self.default_has_controlling_passenger().await;
            };
            let main_hand = player.inventory().held_item().await.item.id;
            if main_hand == Item::CARROT_ON_A_STICK.id {
                return true;
            }
            player.inventory().off_hand_item().await.item.id == Item::CARROT_ON_A_STICK.id
                || self.default_has_controlling_passenger().await
        })
    }

    /// `Pig.thunderHit` (`Pig.java:198-210`): outside Peaceful the pig becomes a zombified
    /// piglin (its saddle is not kept, `keepEquipment = false`) holding a golden sword, or
    /// rarely a golden spear, instead of taking lightning damage.
    fn mob_on_lightning_strike<'a>(
        &'a self,
        caller: &'a dyn EntityBase,
        lightning: &'a crate::entity::lightning::LightningBoltEntity,
    ) -> EntityBaseFuture<'a, ()> {
        use crate::entity::mob::zombification;
        use rand::RngExt;
        Box::pin(async move {
            let world = self.get_entity().world.load_full();
            if world.level_info.load().difficulty != pumpkin_util::Difficulty::Peaceful
                && !self.get_entity().is_removed()
            {
                let zombified = zombification::prepare_conversion_with_equipment(
                    &self.mob_entity,
                    &EntityType::ZOMBIFIED_PIGLIN,
                    false,
                    ZombifiedPiglinEntity::new,
                )
                .await;
                zombified.set_persistence_required();
                // `ZombifiedPiglin.populateDefaultEquipmentSlots` (`ZombifiedPiglin.java:224-226`).
                let weapon = if rand::rng().random_range(0..20) == 0 {
                    &Item::GOLDEN_SPEAR
                } else {
                    &Item::GOLDEN_SWORD
                };
                if let Some(living) = zombified.get_living_entity() {
                    living
                        .entity_equipment
                        .lock()
                        .await
                        .put(&EquipmentSlot::MAIN_HAND, ItemStack::new(1, weapon));
                }
                zombification::complete_conversion(&self.mob_entity, zombified).await;
                return;
            }
            self.mob_entity
                .living_entity
                .on_lightning_strike(caller, lightning)
                .await;
        })
    }

    /// `Pig.playStepSound` (`Pig.java:145-148`).
    fn get_step_sound(&self) -> Option<Sound> {
        use crate::entity::ageable::AgeableMob;
        Some(if self.is_baby() {
            Sound::EntityBabyPigStep
        } else {
            Sound::EntityPigStep
        })
    }

    fn mob_tick<'a>(&'a self, _caller: &'a Arc<dyn EntityBase>) -> EntityBaseFuture<'a, ()> {
        Box::pin(async move {
            self.steering.tick_ridden(self).await;
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

impl ItemSteerable for PigEntity {
    fn boost(&self) -> bool {
        let Some(total) = self.steering.boost() else {
            return false;
        };
        // Vanilla syncs the new length through `DATA_BOOST_TIME`; the riding client reads it
        // to start its own boost timer (`Pig.onSyncedDataUpdated`).
        self.mob_entity.living_entity.entity.send_meta_data(
            &[pumpkin_protocol::java::client::play::Metadata::new(
                pumpkin_data::tracked_data::pig::DATA_BOOST_TIME,
                pumpkin_protocol::codec::var_int::VarInt(total),
            )],
            None,
        );
        true
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}
