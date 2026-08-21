use std::sync::{
    Arc, Weak,
    atomic::{AtomicI32, Ordering},
};

use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_data::tag::{self, Taggable};
use pumpkin_data::{entity::EntityType, item::Item};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_protocol::codec::var_int::VarInt;
use pumpkin_protocol::java::client::play::Metadata;

use crate::entity::{
    Entity, EntityBase, EntityBaseFuture, NBTStorage, NbtFuture,
    ageable::{AgeableData, AgeableMob},
    ai::goal::{
        breed::BreedGoal, follow_parent::FollowParentGoal, look_around::RandomLookAroundGoal,
        look_at_entity::LookAtEntityGoal, sniffer_dig::SnifferDigGoal, swim::SwimGoal,
        tempt::TemptGoal, wander_around::WanderAroundGoal,
    },
    item::ItemEntity,
    mob::{Mob, MobEntity},
};
use crate::world::World;

const TEMPT_ITEMS: &[&Item] = &[&Item::TORCHFLOWER_SEEDS];

/// Vanilla `Sniffer.SNIFFER_BABY_START_AGE`: twice the default baby age, sniffers take twice as
/// long to grow up.
pub const SNIFFER_BABY_START_AGE: i32 = -48000;

/// Vanilla `Sniffer.State`.
///
/// Currently reflects only `IDLING`/`DIGGING`, driven by `SnifferDigGoal`'s existing
/// start/stop boundaries (see its `transition_to` call sites) --
/// `SCENTING`/`SNIFFING`/`SEARCHING`/`RISING`/`FEELING_HAPPY` belong to `SnifferAi` Brain
/// behaviors that are not ported at all yet and are left for a follow-up.
#[repr(i32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SnifferState {
    #[default]
    Idling = 0,
    FeelingHappy = 1,
    Scenting = 2,
    Sniffing = 3,
    Searching = 4,
    Digging = 5,
    Rising = 6,
}

impl SnifferState {
    #[must_use]
    pub const fn from_id(id: i32) -> Self {
        match id {
            1 => Self::FeelingHappy,
            2 => Self::Scenting,
            3 => Self::Sniffing,
            4 => Self::Searching,
            5 => Self::Digging,
            6 => Self::Rising,
            _ => Self::Idling,
        }
    }

    #[must_use]
    pub const fn id(self) -> i32 {
        self as i32
    }
}

pub struct SnifferEntity {
    pub mob_entity: MobEntity,
    pub ageable_data: AgeableData,
    state: AtomicI32,
}

impl SnifferEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let mob_entity = MobEntity::new(entity);
        let sniffer = Self {
            mob_entity,
            ageable_data: AgeableData::default(),
            state: AtomicI32::new(SnifferState::Idling.id()),
        };
        let mob_arc = Arc::new(sniffer);
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
            goal_selector.add_goal(1, SnifferDigGoal::new(1.0));
            goal_selector.add_goal(2, BreedGoal::new(1.0));
            goal_selector.add_goal(3, Box::new(TemptGoal::new(1.0, TEMPT_ITEMS, false)));
            goal_selector.add_goal(4, Box::new(FollowParentGoal::new(1.0)));
            goal_selector.add_goal(5, Box::new(WanderAroundGoal::new(1.0)));
            goal_selector.add_goal(
                6,
                LookAtEntityGoal::with_default(mob_weak, &EntityType::PLAYER, 6.0),
            );
            goal_selector.add_goal(7, Box::new(RandomLookAroundGoal::default()));
        };

        mob_arc
    }

    #[must_use]
    pub fn get_state(&self) -> SnifferState {
        SnifferState::from_id(self.state.load(Ordering::Relaxed))
    }

    /// `Sniffer.DATA_STATE`: the animation state has to reach the client or the sniffer
    /// keeps its idle pose while it digs.
    pub fn set_state(&self, state: SnifferState) {
        self.state.store(state.id(), Ordering::Relaxed);
        self.mob_entity.living_entity.entity.send_meta_data(
            &[Metadata::new(
                pumpkin_data::tracked_data::sniffer::STATE,
                VarInt(state.id()),
            )],
            None,
        );
    }

    /// Vanilla `Sniffer.transitionTo`. Only `IDLING`/`DIGGING` are driven anywhere today (by
    /// `SnifferDigGoal`); the sound-per-state switch mirrors vanilla but most states are
    /// currently unreachable, so most arms never fire yet.
    pub fn transition_to(&self, state: SnifferState) {
        self.set_state(state);

        let entity = &self.mob_entity.living_entity.entity;
        let world = entity.world.load();
        let pos = entity.pos.load();
        let sound = match state {
            SnifferState::FeelingHappy => Some(Sound::EntitySnifferHappy),
            SnifferState::Scenting => Some(Sound::EntitySnifferScenting),
            SnifferState::Sniffing => Some(Sound::EntitySnifferSniffing),
            // DIGGING's sound is played by `SnifferDigGoal` itself at its own dig-start point
            // (entity event 63 in vanilla); IDLING/SEARCHING/RISING play no sound here.
            SnifferState::Idling
            | SnifferState::Searching
            | SnifferState::Digging
            | SnifferState::Rising => None,
        };
        if let Some(sound) = sound {
            world.play_sound(sound, SoundCategory::Neutral, &pos);
        }
    }
}

impl AgeableMob for SnifferEntity {
    fn get_ageable_data(&self) -> &AgeableData {
        &self.ageable_data
    }

    fn get_baby_start_age(&self) -> i32 {
        SNIFFER_BABY_START_AGE
    }
}

impl NBTStorage for SnifferEntity {
    fn write_nbt<'a>(&'a self, nbt: &'a mut NbtCompound) -> NbtFuture<'a, ()> {
        Box::pin(async move {
            use super::animal::Animal;
            self.mob_entity.living_entity.write_nbt(nbt).await;
            self.write_ageable_nbt(nbt);
            self.write_animal_nbt(nbt);
            nbt.put_int("State", self.get_state().id());
        })
    }

    fn read_nbt_non_mut<'a>(&'a self, nbt: &'a NbtCompound) -> NbtFuture<'a, ()> {
        Box::pin(async move {
            use super::animal::Animal;
            self.mob_entity.living_entity.read_nbt_non_mut(nbt).await;
            self.read_ageable_nbt(nbt);
            self.read_animal_nbt(nbt);
            if let Some(state_id) = nbt.get_int("State") {
                self.state.store(state_id, Ordering::Relaxed);
            }
        })
    }
}

impl super::animal::Animal for SnifferEntity {
    fn is_food(&self, item_stack: &ItemStack) -> bool {
        item_stack.item.has_tag(&tag::Item::MINECRAFT_SNIFFER_FOOD)
    }
}

impl Mob for SnifferEntity {
    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    fn mob_init_data_tracker(&self) -> EntityBaseFuture<'_, ()> {
        Box::pin(async move {
            let entity = &self.mob_entity.living_entity.entity;
            if entity.age.load(Ordering::Relaxed) < 0 {
                entity.send_meta_data(
                    &[Metadata::new(
                        pumpkin_data::tracked_data::sniffer::BABY_ID,
                        true,
                    )],
                    None,
                );
            }
            entity.send_meta_data(
                &[Metadata::new(
                    pumpkin_data::tracked_data::sniffer::STATE,
                    VarInt(self.get_state().id()),
                )],
                None,
            );
        })
    }

    fn can_breed_with(&self, mate: &dyn EntityBase) -> bool {
        let allowed = |state| {
            matches!(
                state,
                SnifferState::Idling | SnifferState::Scenting | SnifferState::FeelingHappy
            )
        };
        allowed(self.get_state())
            && mate
                .get_mob()
                .and_then(|mob| mob.cast_any().downcast_ref::<Self>())
                .is_some_and(|mate| allowed(mate.get_state()))
    }

    fn mob_interact<'a>(
        &'a self,
        player: &'a Arc<crate::entity::player::Player>,
        item_stack: &'a mut ItemStack,
    ) -> EntityBaseFuture<'a, bool> {
        use super::animal::Animal;
        self.animal_interact(player, item_stack, Sound::EntitySnifferEat)
    }

    /// Vanilla `Sniffer.spawnChildFromBreeding`: drops a `SNIFFER_EGG` item instead of spawning
    /// a live baby. The item is emitted by `spawn_breeding_item` after the shared finalization.
    ///
    fn create_offspring<'a>(
        &'a self,
        _mate: &'a dyn EntityBase,
        _world: &'a Arc<World>,
    ) -> EntityBaseFuture<'a, Option<Arc<dyn EntityBase>>> {
        Box::pin(async move { None })
    }

    fn spawn_breeding_item<'a>(&'a self, world: &'a Arc<World>) -> EntityBaseFuture<'a, ()> {
        Box::pin(async move {
            let entity = &self.mob_entity.living_entity.entity;
            let pos = entity.pos.load();
            let item_entity = Arc::new(ItemEntity::new(
                Entity::new(world.clone(), pos, &EntityType::ITEM),
                ItemStack::new(1, &Item::SNIFFER_EGG),
            ));
            world.spawn_entity(item_entity).await;
            world.play_sound(Sound::BlockSnifferEggPlop, SoundCategory::Neutral, &pos);
        })
    }
}
