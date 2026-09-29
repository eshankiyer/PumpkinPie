// Legacy invariant checks retained for vanilla behavior; migrate these paths before removing this allow.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use std::sync::{Arc, Weak, atomic::Ordering::Relaxed};

use pumpkin_data::attributes::Attributes;
use pumpkin_data::damage::DamageType;
use pumpkin_data::entity::EntityType;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::Sound;
use pumpkin_data::tag::{self, Taggable};
use pumpkin_data::{Block, BlockState};
use pumpkin_inventory::screen_handler::BoxFuture;
use pumpkin_util::math::vector3::Vector3;
use rand::RngExt;

use crate::entity::{
    Entity, EntityBase, EntityBaseFuture, NBTStorage,
    ai::goal::{
        ambient_stand::AmbientStandGoal, follow_parent::FollowParentGoal,
        look_around::RandomLookAroundGoal, look_at_entity::LookAtEntityGoal,
        run_around_like_crazy::RunAroundLikeCrazyGoal, swim::SwimGoal, tempt::TemptGoal,
        wander_around::WanderAroundGoal,
    },
    mob::{Mob, MobEntity},
    passive::{
        animal::Animal,
        equine::{AbstractHorse, AbstractHorseData},
    },
    player::Player,
};

/// `ZombieHorse.java`'s `isFood` tag (`ItemTags.ZOMBIE_HORSE_FOOD`) is red mushroom only;
/// `TemptGoal` only supports a static item list (not a tag/predicate), so this is narrowed the
/// same way `HappyGhastEntity`'s tempt items are.
const TEMPT_ITEMS: &[&Item] = &[&Item::RED_MUSHROOM];

/// Represents a Zombie Horse.
///
/// A passive mob that can be tamed and ridden but never carries a chest and never breeds
/// attribute-inherited offspring (`ZombieHorse.getBreedOffspring` spawns a plain default
/// `ZombieHorse`, unlike Horse/Donkey/Mule).
///
/// Zombie-jockey natural-spawn (`ZombieHorse.java:132-146`) is not ported: Pumpkin has no
/// natural-spawn "finalize" hook to inject a rider mob yet (no chicken/spider jockey precedent
/// exists either), so this is a known gap rather than folded into this framework change.
///
/// Wiki: <https://minecraft.wiki/w/Zombie_Horse>
pub struct ZombieHorseEntity {
    pub mob_entity: MobEntity,
    pub horse_data: AbstractHorseData,
}

impl ZombieHorseEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let mob_entity = MobEntity::new(entity);
        let horse = Self {
            mob_entity,
            horse_data: AbstractHorseData::default(),
        };
        let mob_arc = Arc::new(horse);
        // `ZombieHorse.createAttributes` (`ZombieHorse.java:53-55`) adds a fixed
        // MAX_HEALTH attribute of 25 to the base horse attributes.
        if let Some(attribute) = mob_arc
            .mob_entity
            .living_entity
            .attributes
            .write()
            .unwrap()
            .get_mut(&Attributes::MAX_HEALTH.id)
        {
            attribute.base_value = 25.0;
            attribute.dirty.store(true, Relaxed);
        }
        mob_arc.mob_entity.living_entity.health.store(25.0);
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

            // `ZombieHorse.java:125-129` (`addBehaviourGoals`): float + tempt only, no panic
            // goal. Wander/look/stand/run-around-like-crazy come from the base
            // `AbstractHorse.registerGoals` (`AbstractHorse.java:132-145`); its `BreedGoal` never
            // fires since `AbstractHorse.canMate` is `false`.
            goal_selector.add_goal(0, Box::new(SwimGoal::default()));
            goal_selector.add_goal(1, RunAroundLikeCrazyGoal::new(horse_weak.clone(), 1.2));
            goal_selector.add_goal(3, Box::new(TemptGoal::new(1.25, TEMPT_ITEMS, false)));
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

impl NBTStorage for ZombieHorseEntity {
    fn write_nbt<'a>(
        &'a self,
        nbt: &'a mut pumpkin_nbt::compound::NbtCompound,
    ) -> crate::entity::NbtFuture<'a, ()> {
        Box::pin(async {
            self.mob_entity.living_entity.write_nbt(nbt).await;
            self.write_horse_nbt(nbt);
        })
    }

    fn read_nbt_non_mut<'a>(
        &'a self,
        nbt: &'a pumpkin_nbt::compound::NbtCompound,
    ) -> crate::entity::NbtFuture<'a, ()> {
        Box::pin(async {
            self.mob_entity.living_entity.read_nbt_non_mut(nbt).await;
            self.read_horse_nbt(nbt);
        })
    }
}

impl Animal for ZombieHorseEntity {
    /// `ZombieHorse.isFood`: the distinct `ZOMBIE_HORSE_FOOD` tag (red mushroom only), not the
    /// generic `HORSE_FOOD` tag Horse/Donkey/Mule use.
    fn is_food(&self, item_stack: &ItemStack) -> bool {
        item_stack
            .item
            .has_tag(&tag::Item::MINECRAFT_ZOMBIE_HORSE_FOOD)
    }

    /// `ZombieHorse.canAgeUp` -- babies never grow up from food.
    fn can_age_up(&self) -> bool {
        false
    }
}

impl AbstractHorse for ZombieHorseEntity {
    fn horse_data(&self) -> &AbstractHorseData {
        &self.horse_data
    }

    /// `ZombieHorse.getQuadLeashOffsets` (`ZombieHorse.java:190-193`) uses the shorter
    /// horse-body offsets.
    fn get_quad_leash_offsets(&self) -> [Vector3<f64>; 4] {
        let width = f64::from(self.get_entity().width());
        let height = f64::from(self.get_entity().height());
        let front_offset = 0.04 * width;
        let front_back = 0.41 * width;
        let left_right = 0.18 * width;
        let y = 0.73 * height;
        [
            Vector3::new(-left_right, y, front_back + front_offset),
            Vector3::new(-left_right, y, -front_back + front_offset),
            Vector3::new(left_right, y, -front_back + front_offset),
            Vector3::new(left_right, y, front_back + front_offset),
        ]
    }

    fn angry_sound(&self) -> Option<Sound> {
        Some(Sound::EntityZombieHorseAngry)
    }

    fn eating_sound(&self) -> Option<Sound> {
        Some(Sound::EntityZombieHorseEat)
    }

    /// `ZombieHorse.canFallInLove` -- always false.
    fn can_fall_in_love(&self) -> bool {
        false
    }

    /// `ZombieHorse.isMobControlled`: `getFirstPassenger() instanceof Mob` -- a non-player
    /// mob riding (e.g. a zombie jockey) counts as being "in control", which keeps
    /// `RunAroundLikeCrazyGoal` from bucking it.
    fn is_mob_controlled(&self) -> BoxFuture<'_, bool> {
        Box::pin(async {
            let passengers = self.get_entity().passengers.lock().await;
            passengers
                .first()
                .is_some_and(|passenger| passenger.get_mob().is_some())
        })
    }

    /// `ZombieHorse.randomizeAttributes`: only jump-strength and speed, using
    /// `generateZombieHorseJumpStrength`/`generateZombieHorseSpeed` (different constants from
    /// the base `AbstractHorse` formula) -- max-health stays the fixed 25 from `createAttributes`.
    fn randomize_attributes(&self, random: &mut impl RngExt)
    where
        Self: Sized,
    {
        let mut attrs = self.mob_entity.living_entity.attributes.write().unwrap();
        if let Some(a) = attrs.get_mut(&Attributes::JUMP_STRENGTH.id) {
            a.base_value =
                crate::entity::passive::equine::generate_zombie_horse_jump_strength(random);
            a.dirty.store(true, Relaxed);
        }
        if let Some(a) = attrs.get_mut(&Attributes::MOVEMENT_SPEED.id) {
            a.base_value = crate::entity::passive::equine::generate_zombie_horse_speed(random);
            a.dirty.store(true, Relaxed);
        }
    }
}

impl Mob for ZombieHorseEntity {
    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    /// Vanilla `ZombieHorse.chargeSpeedModifier` (`ZombieHorse.java:200-203`).
    fn charge_speed_modifier(&self) -> f32 {
        1.4
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

    /// `AbstractHorse.isImmobile` (`AbstractHorse.java:504-507`): a grazing or rearing horse
    /// runs no AI.
    fn suppress_ai_goals(&self) -> bool {
        AbstractHorse::is_immobile(self)
    }

    /// `AbstractHorse.playStepSound` (`AbstractHorse.java:341-363`).
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

    fn mob_init_data_tracker(&self) -> EntityBaseFuture<'_, ()> {
        Box::pin(async move {
            self.send_horse_init_metadata();
        })
    }

    /// `ZombieHorse.getAmbientSound` (`ZombieHorse.java:90-93`).
    fn get_ambient_sound(&self) -> Option<Sound> {
        Some(Sound::EntityZombieHorseAmbient)
    }

    fn has_controlling_passenger(&self) -> EntityBaseFuture<'_, bool> {
        AbstractHorse::has_saddled_player_passenger(self)
    }

    fn mob_interact<'a>(
        &'a self,
        player: &'a Arc<Player>,
        item_stack: &'a mut ItemStack,
    ) -> EntityBaseFuture<'a, bool> {
        self.abstract_horse_mob_interact(player, item_stack)
    }
}
