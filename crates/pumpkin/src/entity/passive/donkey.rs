// Legacy invariant checks retained for vanilla behavior; migrate these paths before removing this allow.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use std::sync::{Arc, Weak, atomic::Ordering::Relaxed};

use pumpkin_data::attributes::Attributes;
use pumpkin_data::damage::DamageType;
use pumpkin_data::entity::EntityType;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_data::tag::{self, Taggable};
use pumpkin_data::{Block, BlockState};
use rand::RngExt;
use uuid::Uuid;

use crate::entity::{
    Entity, EntityBase, EntityBaseFuture, NBTStorage,
    ai::goal::{
        ambient_stand::AmbientStandGoal, follow_parent::FollowParentGoal,
        horse_breed::HorseBreedGoal, look_around::RandomLookAroundGoal,
        look_at_entity::LookAtEntityGoal, run_around_like_crazy::RunAroundLikeCrazyGoal,
        swim::SwimGoal, tempt::TemptGoal, wander_around::WanderAroundGoal,
    },
    mob::{Mob, MobEntity},
    passive::{
        animal::Animal,
        equine::{
            AbstractChestedHorse, AbstractHorse, AbstractHorseData, ChestedHorseData,
            HORSE_TEMPT_ITEMS, MountPanicGoal, set_offspring_attributes,
        },
    },
    player::Player,
};

/// Donkey.java#canMate: a Donkey may breed with another Donkey or a Horse.
const COMPATIBLE_MATES: &[&EntityType] = &[&EntityType::DONKEY, &EntityType::HORSE];

/// Represents a Donkey, a passive mob that can be tamed and equipped with a chest.
///
/// Wiki: <https://minecraft.wiki/w/Donkey>
pub struct DonkeyEntity {
    pub mob_entity: MobEntity,
    pub horse_data: AbstractHorseData,
    pub chested_data: ChestedHorseData,
}

impl DonkeyEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let mob_entity = MobEntity::new(entity);
        let donkey = Self {
            mob_entity,
            horse_data: AbstractHorseData::default(),
            chested_data: ChestedHorseData::default(),
        };
        let mob_arc = Arc::new(donkey);
        AbstractHorse::randomize_attributes(mob_arc.as_ref(), &mut rand::rng());

        let mob_weak: Weak<dyn Mob> = {
            let mob_arc: Arc<dyn Mob> = mob_arc.clone();
            Arc::downgrade(&mob_arc)
        };
        let horse_weak: Weak<Self> = Arc::downgrade(&mob_arc);

        {
            let mut goal_selector = mob_arc
                .mob_entity
                .goals_selector
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);

            // See `horse.rs` for the priority citations (`AbstractHorse.java:134-151`);
            // Donkey uses the same base `addBehaviourGoals`.
            goal_selector.add_goal(0, Box::new(SwimGoal::default()));
            goal_selector.add_goal(1, RunAroundLikeCrazyGoal::new(horse_weak.clone(), 1.2));
            goal_selector.add_goal(1, MountPanicGoal::new(horse_weak.clone(), 1.2));
            goal_selector.add_goal(2, HorseBreedGoal::new(1.0, COMPATIBLE_MATES));
            goal_selector.add_goal(3, Box::new(TemptGoal::new(1.25, HORSE_TEMPT_ITEMS, false)));
            goal_selector.add_goal(4, Box::new(FollowParentGoal::new(1.0)));
            goal_selector.add_goal(6, Box::new(WanderAroundGoal::new_water_avoiding(0.7)));
            goal_selector.add_goal(
                7,
                LookAtEntityGoal::with_default(mob_weak, &EntityType::PLAYER, 6.0),
            );
            goal_selector.add_goal(8, Box::new(RandomLookAroundGoal::default()));
            goal_selector.add_goal(9, AmbientStandGoal::new(horse_weak));
        };

        mob_arc
    }
}

impl NBTStorage for DonkeyEntity {
    fn write_nbt<'a>(
        &'a self,
        nbt: &'a mut pumpkin_nbt::compound::NbtCompound,
    ) -> crate::entity::NbtFuture<'a, ()> {
        Box::pin(async {
            self.mob_entity.living_entity.write_nbt(nbt).await;
            self.write_animal_nbt(nbt);
            self.write_horse_nbt(nbt);
            self.write_chested_horse_nbt(nbt).await;
        })
    }

    fn read_nbt_non_mut<'a>(
        &'a self,
        nbt: &'a pumpkin_nbt::compound::NbtCompound,
    ) -> crate::entity::NbtFuture<'a, ()> {
        Box::pin(async {
            self.mob_entity.living_entity.read_nbt_non_mut(nbt).await;
            self.read_animal_nbt(nbt);
            self.read_horse_nbt(nbt);
            self.read_chested_horse_nbt(nbt).await;
        })
    }
}

impl Animal for DonkeyEntity {
    fn is_food(&self, item_stack: &ItemStack) -> bool {
        item_stack.item.has_tag(&tag::Item::MINECRAFT_HORSE_FOOD)
    }
}

impl AbstractHorse for DonkeyEntity {
    fn horse_data(&self) -> &AbstractHorseData {
        &self.horse_data
    }

    fn angry_sound(&self) -> Option<Sound> {
        Some(Sound::EntityDonkeyAngry)
    }

    fn eating_sound(&self) -> Option<Sound> {
        Some(Sound::EntityDonkeyEat)
    }

    /// `AbstractChestedHorse` clears `canGallop` (`AbstractChestedHorse.java:38`).
    fn can_gallop(&self) -> bool {
        false
    }

    /// `AbstractChestedHorse.randomizeAttributes`: only max-health is rolled.
    fn randomize_attributes(&self, random: &mut impl RngExt)
    where
        Self: Sized,
    {
        let mut attrs = self.mob_entity.living_entity.attributes.write().unwrap();
        if let Some(a) = attrs.get_mut(&Attributes::MAX_HEALTH.id) {
            a.base_value = crate::entity::passive::equine::generate_max_health(random);
            a.dirty.store(true, Relaxed);
        }
        drop(attrs);
        let max_health = self.mob_entity.living_entity.get_max_health();
        self.mob_entity.living_entity.health.store(max_health);
    }
}

impl AbstractChestedHorse for DonkeyEntity {
    fn chested_data(&self) -> &ChestedHorseData {
        &self.chested_data
    }

    fn play_chest_equips_sound(&self) {
        let entity = self.get_entity();
        let world = entity.world.load();
        world.play_sound(
            Sound::EntityDonkeyChest,
            SoundCategory::Neutral,
            &entity.pos.load(),
        );
    }
}

impl Mob for DonkeyEntity {
    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    /// `ServerPlayer.openHorseInventory` receives the chested horse container
    /// (`ServerPlayer.java:1372-1382`) after the ridden-vehicle inventory command.
    fn open_custom_inventory_screen<'a>(
        &'a self,
        player: &'a Arc<Player>,
    ) -> EntityBaseFuture<'a, ()> {
        Box::pin(async move {
            if self.is_tamed() {
                AbstractChestedHorse::open_chest_inventory(self, player).await;
            }
        })
    }

    // `AbstractHorse` rider, breeding, and leash hooks (`AbstractHorse.java:189-205,878-905`).
    fn can_jump(&self) -> EntityBaseFuture<'_, bool> {
        AbstractHorse::can_jump_now(self)
    }

    fn on_player_jump(&self, jump_amount: i32) {
        AbstractHorse::on_player_jump(self, jump_amount);
    }

    fn handle_start_jump(&self, jump_scale: i32) {
        AbstractHorse::handle_start_jump(self, jump_scale);
    }

    fn handle_stop_jump(&self) {
        AbstractHorse::handle_stop_jump(self);
    }

    fn on_elastic_leash_pull(&self) {
        AbstractHorse::on_elastic_leash_pull(self);
    }

    fn custom_travel<'a>(&'a self, caller: &'a Arc<dyn EntityBase>) -> EntityBaseFuture<'a, bool> {
        AbstractHorse::custom_travel(self, caller)
    }

    /// `AbstractHorse.tick`/`aiStep` run for `NoAI` horses too, so this is `post_tick`, which
    /// `Mob::tick` runs every tick, rather than `mob_tick`.
    fn post_tick(&self) -> EntityBaseFuture<'_, ()> {
        AbstractHorse::tick_horse_ai(self)
    }

    /// `AbstractHorse.hurtServer` (`AbstractHorse.java:319-327`).
    fn on_damage<'a>(
        &'a self,
        _damage_type: DamageType,
        _source: Option<&'a dyn EntityBase>,
    ) -> EntityBaseFuture<'a, ()> {
        Box::pin(async move {
            AbstractHorse::horse_on_damage(self);
        })
    }

    /// `AbstractHorse.isImmobile` (`AbstractHorse.java:504-507`): a grazing or rearing donkey
    /// runs no AI.
    fn suppress_ai_goals(&self) -> bool {
        AbstractHorse::is_immobile(self)
    }

    /// `AbstractHorse.playStepSound` (`AbstractHorse.java:341-363`); a donkey never gallops.
    fn ground_step_sounds(
        &self,
        supporting_block: &Block,
        supporting_state: &BlockState,
        above_block: &Block,
    ) -> Option<Vec<(Sound, f32, f32)>> {
        Some(AbstractHorse::horse_step_sounds(
            self,
            supporting_block,
            supporting_state,
            above_block,
        ))
    }

    /// `Donkey.getAmbientSound`.
    fn get_ambient_sound(&self) -> Option<Sound> {
        Some(Sound::EntityDonkeyAmbient)
    }

    fn drop_death_inventory(&self) -> EntityBaseFuture<'_, ()> {
        AbstractChestedHorse::drop_chest_contents(self)
    }

    fn mob_init_data_tracker(&self) -> EntityBaseFuture<'_, ()> {
        Box::pin(async move {
            self.send_horse_init_metadata();
            self.send_initial_chest();
        })
    }

    fn has_controlling_passenger(&self) -> EntityBaseFuture<'_, bool> {
        AbstractHorse::has_saddled_player_passenger(self)
    }

    fn mob_interact<'a>(
        &'a self,
        player: &'a Arc<Player>,
        item_stack: &'a mut ItemStack,
    ) -> EntityBaseFuture<'a, bool> {
        self.chested_mob_interact(player, item_stack)
    }

    /// `Donkey.getBreedOffspring`: Donkey+Horse -> Mule, Donkey+Donkey -> Donkey (both handled
    /// generically by `HorseBreedGoal`/`horse_family_offspring`) plus attribute inheritance.
    fn create_offspring<'a>(
        &'a self,
        mate: &'a dyn EntityBase,
        world: &'a Arc<crate::world::World>,
    ) -> EntityBaseFuture<'a, Option<Arc<dyn EntityBase>>> {
        Box::pin(async move {
            let entity = self.get_entity();
            let baby = crate::entity::r#type::from_type(
                entity.entity_type,
                entity.pos.load(),
                world,
                Uuid::new_v4(),
            );

            if let Some(baby_mob) = baby.get_mob() {
                set_offspring_attributes(self, mate, baby_mob);
            }

            Some(baby)
        })
    }
}
