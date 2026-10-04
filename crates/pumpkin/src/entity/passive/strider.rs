use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};

use pumpkin_data::attributes::Attributes;
use pumpkin_data::fluid::Fluid;
use pumpkin_data::tag::{self, Taggable};
use pumpkin_data::tracked_data;
use pumpkin_protocol::java::client::play::Metadata;

use pumpkin_data::Block;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::Sound;
use pumpkin_data::{data_component_impl::EquipmentSlot, entity::EntityType, item::Item};
use pumpkin_util::math::boundingbox::{EntityAttachmentsBuilder, EntityDimensions};

use crate::entity::ai::pathfinder::node::PathType;
use crate::entity::attributes::{Modifier, ModifierOperation};
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

/// `Strider.SUFFOCATING_MODIFIER_ID` (`Strider.java:60`).
const SUFFOCATING_MODIFIER_ID: &str = "minecraft:suffocating";
/// `Strider.SUFFOCATING_MODIFIER` amount (`Strider.java:61-63`), `ADD_MULTIPLIED_BASE`.
const SUFFOCATING_MODIFIER_AMOUNT: f64 = -0.34;
/// `Strider.getLiquidCollisionShape` (`Strider.java:341-344`) is `Block.column(16.0, 0.0, 8.0)`.
const LIQUID_COLLISION_HEIGHT: f64 = 0.5;

/// Represents a Strider, a passive mob that walks on lava.
///
/// Wiki: <https://minecraft.wiki/w/Strider>
pub struct StriderEntity {
    pub mob_entity: MobEntity,
    pub ageable_data: crate::entity::ageable::AgeableData,
    pub steering: ItemBasedSteering,
    pub saddled: std::sync::atomic::AtomicBool,
    /// Vanilla `DATA_SUFFOCATING` (`Strider.java:67`), recomputed every tick and never saved.
    suffocating: AtomicBool,
    /// `Strider.temptGoal.isRunning()` for `isBeingTempted` (`Strider.java:320-322`).
    tempt_running: Arc<AtomicBool>,
}

impl StriderEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let mob_entity = MobEntity::new(entity);
        let strider = Self {
            mob_entity,
            ageable_data: crate::entity::ageable::AgeableData::default(),
            steering: ItemBasedSteering::default(),
            saddled: std::sync::atomic::AtomicBool::new(false),
            suffocating: AtomicBool::new(false),
            tempt_running: Arc::new(AtomicBool::new(false)),
        };
        let mob_arc = Arc::new(strider);
        // `Strider` constructor (`Strider.java:93-100`) supplies these pathfinding maluses.
        #[allow(clippy::semicolon_if_nothing_returned)]
        {
            let mut navigator = mob_arc.mob_entity.navigator.lock().unwrap();
            navigator.set_pathfinding_malus(PathType::Water, -1.0);
            navigator.set_pathfinding_malus(PathType::Lava, 0.0);
            navigator.set_pathfinding_malus(PathType::DangerFire, 0.0);
            navigator.set_pathfinding_malus(PathType::DamageFire, 0.0);
            // `Strider.createNavigation` (`Strider.java:380-383`).
            navigator.set_strider(true)
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
            goal_selector.add_goal(
                3,
                Box::new(
                    TemptGoal::new(1.4, STRIDER_TEMPT_ITEMS, false)
                        .with_running_flag(mob_arc.tempt_running.clone()),
                ),
            );
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

    /// Vanilla `Strider.isSuffocating` (`Strider.java:176-178`).
    pub fn is_suffocating(&self) -> bool {
        self.suffocating.load(Ordering::Relaxed)
    }

    /// Vanilla `Strider.setSuffocating` (`Strider.java:163-174`): publishes the synced flag and
    /// swaps the `minecraft:suffocating` movement-speed modifier. Both are idempotent in vanilla
    /// (an unchanged `entityData.set` sends nothing), so only a change does any work.
    async fn set_suffocating(&self, flag: bool) {
        if self.suffocating.swap(flag, Ordering::Relaxed) == flag {
            return;
        }
        let living = &self.mob_entity.living_entity;
        living.entity.send_meta_data(
            &[Metadata::new(tracked_data::strider::DATA_SUFFOCATING, flag)],
            None,
        );
        living.update_attribute(&Attributes::MOVEMENT_SPEED, |instance| {
            if flag {
                instance.add_or_update_transient_modifier(Modifier {
                    id: SUFFOCATING_MODIFIER_ID.to_string(),
                    amount: SUFFOCATING_MODIFIER_AMOUNT,
                    operation: ModifierOperation::MultiplyBase,
                });
            } else {
                instance.remove_modifier(SUFFOCATING_MODIFIER_ID);
            }
        });
        crate::entity::attributes::send_attribute_updates_for_living(
            living,
            vec![Attributes::MOVEMENT_SPEED],
        )
        .await;
    }

    /// Vanilla `Strider.isBeingTempted` (`Strider.java:320-322`).
    fn is_being_tempted(&self) -> bool {
        self.tempt_running.load(Ordering::Relaxed)
    }

    /// Vanilla `Strider.floatStrider` (`Strider.java:329-339`).
    fn float_strider(&self) {
        let entity = &self.mob_entity.living_entity.entity;
        if !entity.touching_lava.load(Ordering::SeqCst) {
            return;
        }
        let block_pos = entity.block_pos.load();
        let world = entity.world.load();
        // `CollisionContext.isAbove(getLiquidCollisionShape(), blockPosition(), true)`.
        let above_shape = entity.pos.load().y
            > f64::from(block_pos.0.y) + LIQUID_COLLISION_HEIGHT - f64::from(1.0E-5f32);
        if above_shape && world.get_fluid(&block_pos.up()).id != Fluid::FLOWING_LAVA.id {
            entity.on_ground.store(true, Ordering::SeqCst);
        } else {
            let velocity = entity.velocity.load();
            entity.velocity.store(pumpkin_util::math::vector3::Vector3::new(
                velocity.x * 0.5,
                velocity.y * 0.5 + 0.05,
                velocity.z * 0.5,
            ));
        }
    }
}

impl AgeableMob for StriderEntity {
    fn get_ageable_data(&self) -> &crate::entity::ageable::AgeableData {
        &self.ageable_data
    }

    fn baby_dimensions(&self) -> Option<EntityDimensions> {
        Some(
            EntityDimensions::new(0.45, 0.85, 0.4375)
                .with_attachments(EntityAttachmentsBuilder::new().passenger_y(0.65625)),
        )
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
    fn as_ageable_mob(&self) -> Option<&dyn crate::entity::ageable::AgeableMob> {
        Some(self)
    }

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
            if main_hand == Item::WARPED_FUNGUS_ON_A_STICK.id {
                return true;
            }
            player.inventory().off_hand_item().await.item.id == Item::WARPED_FUNGUS_ON_A_STICK.id
                || self.default_has_controlling_passenger().await
        })
    }

    /// `Strider.tick` before `super.tick()` (`Strider.java:298-312`); `mob_tick` is skipped for
    /// `NoAI` mobs, which is exactly the `!isNoAi()` gate on the suffocation update.
    fn mob_tick<'a>(&'a self, _caller: &'a Arc<dyn EntityBase>) -> EntityBaseFuture<'a, ()> {
        Box::pin(async move {
            let entity = &self.mob_entity.living_entity.entity;
            let sound = if self.is_being_tempted() && rand::random_range(0..140) == 0 {
                Some(Sound::EntityStriderHappy)
            } else if self.is_panicking() && rand::random_range(0..60) == 0 {
                Some(Sound::EntityStriderRetreat)
            } else {
                None
            };
            if let Some(sound) = sound {
                entity.world.load().play_sound_fine(
                    sound,
                    self.get_sound_source(),
                    &entity.pos.load(),
                    1.0,
                    self.get_sound_pitch(),
                );
            }

            let in_warm_blocks = Block::from_state_id(entity.get_in_block_state().id)
                .has_tag(&tag::Block::MINECRAFT_STRIDER_WARM_BLOCKS)
                || Block::from_state_id(entity.get_block_state_on_legacy().id)
                    .has_tag(&tag::Block::MINECRAFT_STRIDER_WARM_BLOCKS)
                || entity.lava_height.load() > 0.0;
            let vehicle = entity.vehicle.lock().await.clone();
            let on_warm_strider = vehicle.is_some_and(|vehicle| {
                vehicle
                    .cast_any()
                    .downcast_ref::<Self>()
                    .is_some_and(|strider| !strider.is_suffocating())
            });
            self.set_suffocating(!in_warm_blocks && !on_warm_strider)
                .await;

            self.steering.tick_ridden(self).await;
        })
    }

    /// `Strider.tick` after `super.tick()` (`Strider.java:317`).
    fn post_tick(&self) -> EntityBaseFuture<'_, ()> {
        Box::pin(async move {
            self.float_strider();
        })
    }

    fn can_stand_on_fluid(&self, fluid: &Fluid) -> bool {
        // `Strider.canStandOnFluid` (`Strider.java:179-182`): `fluid.is(FluidTags.LAVA)`.
        fluid.has_tag(&tag::Fluid::MINECRAFT_LAVA)
    }

    fn liquid_collision_height(&self) -> Option<f64> {
        Some(LIQUID_COLLISION_HEIGHT)
    }

    /// `Strider.getAmbientSound` (`Strider.java:351-353`).
    fn get_ambient_sound(&self) -> Option<Sound> {
        (!self.is_panicking() && !self.is_being_tempted()).then_some(Sound::EntityStriderAmbient)
    }

    /// `Strider.nextStep` (`Strider.java:274-277`).
    fn get_next_step(&self, move_dist: f32) -> f32 {
        move_dist + 0.6
    }

    /// `Strider.playStepSound` (`Strider.java:279-282`) plays at volume and pitch 1.0.
    fn get_step_sound(&self) -> Option<Sound> {
        Some(
            if self
                .mob_entity
                .living_entity
                .entity
                .touching_lava
                .load(Ordering::Relaxed)
            {
                Sound::EntityStriderStepLava
            } else {
                Sound::EntityStriderStep
            },
        )
    }

    fn get_step_sound_volume(&self) -> f32 {
        1.0
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
        let Some(total) = self.steering.boost() else {
            return false;
        };
        // Vanilla syncs the new length through `DATA_BOOST_TIME`; the riding client reads it
        // to start its own boost timer (`Pig.onSyncedDataUpdated`).
        self.mob_entity.living_entity.entity.send_meta_data(
            &[pumpkin_protocol::java::client::play::Metadata::new(
                pumpkin_data::tracked_data::strider::DATA_BOOST_TIME,
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
