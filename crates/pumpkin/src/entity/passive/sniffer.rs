use std::sync::{
    Arc, Weak,
    atomic::{AtomicI32, Ordering},
};

use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_data::tag::{self, Taggable};
use pumpkin_data::{entity::EntityType, item::Item};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_nbt::tag::NbtTag;
use pumpkin_protocol::codec::var_int::VarInt;
use pumpkin_protocol::java::client::play::Metadata;
use pumpkin_util::math::position::BlockPos;
use rand::RngExt;

use crate::entity::{
    Entity, EntityBase, EntityBaseFuture, NBTStorage, NbtFuture,
    ageable::{AgeableData, AgeableMob},
    ai::goal::{
        breed::BreedGoal, follow_parent::FollowParentGoal, look_around::RandomLookAroundGoal,
        look_at_entity::LookAtEntityGoal, sniffer_dig::SnifferDigGoal, swim::SwimGoal,
        tempt::TemptGoal, wander_around::WanderAroundGoal,
    },
    ai::pathfinder::{Navigator, node::PathType},
    item::ItemEntity,
    mob::{Mob, MobEntity},
};
use crate::world::World;

const TEMPT_ITEMS: &[&Item] = &[&Item::TORCHFLOWER_SEEDS];

/// Vanilla `Sniffer.SNIFFER_BABY_START_AGE`: twice the default baby age, sniffers take twice as
/// long to grow up.
pub const SNIFFER_BABY_START_AGE: i32 = -48000;

const fn sniffer_ignores_water_malus(fire_ticks: i32, touching_water: bool) -> bool {
    // `Sniffer.onPathfindingStart` allows water while on fire or in water
    // (`Sniffer.java:104-110`).
    fire_ticks > 0 || touching_water
}

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

/// `Sniffer.storeExploredPosition` truncates the existing list to 20 entries and then
/// prepends the new one (`Sniffer.java:322-326`), so the list settles at 21 entries.
const MAX_EXPLORED_POSITIONS: usize = 20;

pub struct SnifferEntity {
    pub mob_entity: MobEntity,
    pub ageable_data: AgeableData,
    state: AtomicI32,
    /// `MemoryModuleType.SNIFFER_EXPLORED_POSITIONS` (`Sniffer.java:65,322-331`), the memory
    /// that stops a sniffer digging the same block over and over.
    ///
    /// Pumpkin has no `Brain` on the Sniffer, so the memory is carried as a plain field, as
    /// `warden.rs` does for its own brain-shaped state. Vanilla stores `GlobalPos`
    /// (dimension + position); a Pumpkin entity never changes `World` in place, so the
    /// dimension is only re-attached on serialisation and the in-memory comparison is by
    /// `BlockPos` alone.
    explored_positions: std::sync::Mutex<Vec<BlockPos>>,
}

impl SnifferEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let mob_entity = MobEntity::new(entity);
        // `Sniffer` enables floating and sets its base path maluses in the constructor
        // (`Sniffer.java:89-95`).
        #[allow(clippy::semicolon_if_nothing_returned)]
        {
            let mut navigator = mob_entity.navigator.lock().unwrap();
            navigator.set_can_float(true);
            navigator.set_pathfinding_malus(PathType::Water, -1.0);
            navigator.set_pathfinding_malus(PathType::PowderSnow, -1.0);
            navigator.set_pathfinding_malus(PathType::DamageCautious, -1.0)
        };
        let sniffer = Self {
            mob_entity,
            ageable_data: AgeableData::default(),
            state: AtomicI32::new(SnifferState::Idling.id()),
            explored_positions: std::sync::Mutex::new(Vec::new()),
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
            goal_selector.add_goal(1, SnifferDigGoal::new(1.0, Arc::downgrade(&mob_arc)));
            goal_selector.add_goal(2, BreedGoal::with_mate_predicate(1.0, sniffer_can_mate));
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

    /// `Sniffer.storeExploredPosition` (`Sniffer.java:322-326`).
    pub fn store_explored_position(&self, pos: BlockPos) {
        let mut explored = self
            .explored_positions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        explored.truncate(MAX_EXPLORED_POSITIONS);
        explored.insert(0, pos);
    }

    /// The `getExploredPositions().noneMatch(...)` half of `Sniffer.canDig(BlockPos)`
    /// (`Sniffer.java:281`), inverted.
    #[must_use]
    pub fn has_explored(&self, pos: BlockPos) -> bool {
        self.explored_positions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .contains(&pos)
    }

    /// Serialises `SNIFFER_EXPLORED_POSITIONS` at vanilla's own NBT path so a sniffer that
    /// is unloaded and reloaded does not forget which blocks it already dug.
    ///
    /// Shape verified against the 26.2 decompile: `Brain.Packed.CODEC` writes a `memories`
    /// map (`Brain.java:463-467`), `MemoryMap.CODEC` keys it by registry name
    /// (`MemoryMap.java:19-22`), each entry is an `ExpirableValue` record with a `value`
    /// field and an optional `ttl` (`ExpirableValue.java:21-28`), and this memory's own
    /// codec is `Codec.list(GlobalPos.CODEC)` (`MemoryModuleType.java:141`), i.e. a list of
    /// `{dimension, pos}` with `pos` a three-int array. No `ttl` is written: vanilla stores
    /// this memory without an expiry.
    fn write_explored_positions_nbt(&self, nbt: &mut NbtCompound) {
        let explored = self
            .explored_positions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        if explored.is_empty() {
            return;
        }

        let dimension = self
            .mob_entity
            .living_entity
            .entity
            .world
            .load()
            .dimension
            .minecraft_name
            .to_string();
        let entries: Vec<NbtTag> = explored
            .iter()
            .map(|pos| {
                let mut global_pos = NbtCompound::new();
                global_pos.put_string("dimension", dimension.clone());
                global_pos.put("pos", NbtTag::IntArray(vec![pos.0.x, pos.0.y, pos.0.z]));
                NbtTag::Compound(global_pos)
            })
            .collect();

        let mut memory = NbtCompound::new();
        memory.put_list("value", entries);
        let mut memories = NbtCompound::new();
        memories.put_compound("minecraft:sniffer_explored_positions", memory);
        let mut brain = NbtCompound::new();
        brain.put_compound("memories", memories);
        nbt.put_compound("Brain", brain);
    }

    fn read_explored_positions_nbt(&self, nbt: &NbtCompound) {
        let Some(entries) = nbt
            .get_compound("Brain")
            .and_then(|brain| brain.get_compound("memories"))
            .and_then(|memories| memories.get_compound("minecraft:sniffer_explored_positions"))
            .and_then(|memory| memory.get_list("value"))
        else {
            return;
        };

        let positions: Vec<BlockPos> = entries
            .iter()
            .filter_map(|entry| {
                let compound = entry.extract_compound()?;
                let pos = compound.get_int_array("pos")?;
                match pos {
                    [x, y, z] => Some(BlockPos::new(*x, *y, *z)),
                    _ => None,
                }
            })
            .collect();
        *self
            .explored_positions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = positions;
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

fn sniffer_can_mate(mob: &dyn Mob, partner: &dyn EntityBase) -> bool {
    let Some(sniffer) = mob.cast_any().downcast_ref::<SnifferEntity>() else {
        return false;
    };
    let Some(partner) = partner.cast_any().downcast_ref::<SnifferEntity>() else {
        return false;
    };

    sniffer_state_can_mate(sniffer.get_state()) && sniffer_state_can_mate(partner.get_state())
}

const fn sniffer_state_can_mate(state: SnifferState) -> bool {
    matches!(
        state,
        SnifferState::Idling | SnifferState::Scenting | SnifferState::FeelingHappy
    )
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
            self.write_explored_positions_nbt(nbt);
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
            self.read_explored_positions_nbt(nbt);
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

    fn on_pathfinding_start(&self, navigator: &mut Navigator) {
        // Vanilla temporarily makes water traversable for a burning or submerged sniffer
        // during evaluator preparation (`Sniffer.java:104-110`).
        let entity = &self.mob_entity.living_entity.entity;
        if sniffer_ignores_water_malus(
            entity.fire_ticks.load(Ordering::Relaxed),
            entity.touching_water.load(Ordering::Relaxed),
        ) {
            navigator.set_pathfinding_malus(PathType::Water, 0.0);
        }
    }

    fn on_pathfinding_done(&self, navigator: &mut Navigator) {
        // Vanilla restores the sniffer's water malus when evaluator state is released
        // (`Sniffer.java:112-115`).
        navigator.set_pathfinding_malus(PathType::Water, -1.0);
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
    /// a live baby. The item is emitted by `spawn_breeding_result` after the shared breeding XP
    /// path, matching vanilla's `finalizeSpawnChildFromBreeding` ordering.
    ///
    /// Vanilla `Sniffer.canMate` gates breeding on both sniffers being in
    /// `{IDLING, SCENTING, FEELING_HAPPY}`; the predicate supplied to this sniffer's
    /// `BreedGoal` enforces that state check before selecting a partner.
    fn create_offspring<'a>(
        &'a self,
        _mate: &'a dyn EntityBase,
        _world: &'a Arc<World>,
    ) -> EntityBaseFuture<'a, Option<Arc<dyn EntityBase>>> {
        Box::pin(async { None })
    }

    fn spawn_breeding_result<'a>(
        &'a self,
        _offspring: Option<Arc<dyn EntityBase>>,
        world: &'a Arc<World>,
        parent_pos: pumpkin_util::math::vector3::Vector3<f64>,
    ) -> EntityBaseFuture<'a, ()> {
        Box::pin(async move {
            let item_entity = Arc::new(ItemEntity::new(
                Entity::new(world.clone(), parent_pos, &EntityType::ITEM),
                ItemStack::new(1, &Item::SNIFFER_EGG),
            ));
            let pitch =
                (self.get_random().random::<f32>() - self.get_random().random::<f32>()) * 0.2 + 0.5;
            world.play_sound_fine(
                Sound::BlockSnifferEggPlop,
                SoundCategory::Neutral,
                &parent_pos,
                1.0,
                pitch,
            );
            world.spawn_entity(item_entity).await;
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{SnifferState, sniffer_ignores_water_malus, sniffer_state_can_mate};

    #[test]
    fn sniffer_ignores_water_malus_only_when_burning_or_submerged() {
        // `Sniffer.onPathfindingStart` checks fire and water state
        // (`Sniffer.java:104-110`).
        assert!(sniffer_ignores_water_malus(1, false));
        assert!(sniffer_ignores_water_malus(0, true));
        assert!(sniffer_ignores_water_malus(1, true));
        assert!(!sniffer_ignores_water_malus(0, false));
    }

    #[test]
    fn sniffer_can_mate_matches_vanilla_state_allowlist() {
        for state in [
            SnifferState::Idling,
            SnifferState::Scenting,
            SnifferState::FeelingHappy,
        ] {
            assert!(sniffer_state_can_mate(state));
        }

        for state in [
            SnifferState::Sniffing,
            SnifferState::Searching,
            SnifferState::Digging,
            SnifferState::Rising,
        ] {
            assert!(!sniffer_state_can_mate(state));
        }
    }
}
