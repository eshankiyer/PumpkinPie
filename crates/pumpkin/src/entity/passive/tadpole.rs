use std::sync::atomic::{AtomicBool, AtomicI32, Ordering::Relaxed};
use std::sync::{Arc, Weak};

use pumpkin_data::entity::EntityType;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_data::tracked_data;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_protocol::java::client::play::Metadata;

use crate::entity::{
    Entity, EntityBase, EntityBaseFuture, NBTStorage, NbtFuture,
    ai::goal::{
        avoid_entity::AvoidEntityGoal, escape_danger::EscapeDangerGoal,
        look_around::RandomLookAroundGoal, look_at_entity::LookAtEntityGoal,
        wander_around::WanderAroundGoal,
    },
    mob::{Mob, MobEntity, zombification},
    passive::frog::FrogEntity,
};

/// `Tadpole.ticksToBeFrog = Math.abs(-24000)` (`Tadpole.java:49`).
const TICKS_TO_BE_FROG: i32 = 24000;

/// Represents a Tadpole, the juvenile form of the frog.
///
/// Wiki: <https://minecraft.wiki/w/Tadpole>
///
/// Vanilla's tadpole is brain-driven; no brain is built here, and the movement goals below stand
/// in for `TadpoleAi`. What this file does carry is the growth clock: `Tadpole.aiStep`
/// (`Tadpole.java:103-106`) advances a tadpole-private `age` counter every tick, and `setAge`
/// (`Tadpole.java:231-236`) converts the tadpole into a frog once it reaches `ticksToBeFrog`
/// (`ageUp`, `Tadpole.java:238-247`). That counter is a plain field here, exactly as vanilla
/// keeps it a plain field rather than the shared `AgeableMob` age.
///
/// Not ported:
/// - The age-lock interaction (`AgeableMob.setAgeLocked`, `Tadpole.java:161-170`), which needs an
///   item-side hook. `AGE_LOCKED` is synced and round-tripped, so a locked tadpole loaded from
///   disk still refuses to grow, but nothing in this codebase can set it yet.
/// - `finalizeSpawn` on the new frog, so the frog it becomes keeps `FrogEntity::new`'s default
///   temperate variant instead of picking one from the biome -- the same `finalizeSpawn` hook
///   gap `goat.rs` and `axolotl.rs` document.
/// - Feeding a tadpole to speed up growth (`Tadpole.java:210-217`).
pub struct TadpoleEntity {
    pub mob_entity: MobEntity,
    /// `Tadpole.age` (`Tadpole.java:52`), counting up from zero.
    age: AtomicI32,
    /// `Tadpole.AGE_LOCKED` (`Tadpole.java:131-133`).
    age_locked: AtomicBool,
}

impl TadpoleEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let mob_entity = MobEntity::new(entity);
        let tadpole = Self {
            mob_entity,
            age: AtomicI32::new(0),
            age_locked: AtomicBool::new(false),
        };
        let mob_arc = Arc::new(tadpole);
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

            // Vanilla `AbstractFish.registerGoals` (inherited by `Tadpole`) has no float/swim
            // goal.
            goal_selector.add_goal(0, EscapeDangerGoal::new(1.25));
            // Vanilla `AbstractFish.registerGoals`: flee players within 8 blocks.
            // The vanilla goal also skips spectators, which `AvoidEntityGoal` cannot do yet.
            goal_selector.add_goal(
                2,
                Box::new(AvoidEntityGoal::new(&EntityType::PLAYER, 8.0, 1.6, 1.4)),
            );
            goal_selector.add_goal(4, Box::new(WanderAroundGoal::new_with_interval(1.0, 40)));
            goal_selector.add_goal(
                2,
                LookAtEntityGoal::with_default(mob_weak, &EntityType::PLAYER, 6.0),
            );
            goal_selector.add_goal(3, Box::new(RandomLookAroundGoal::default()));
        };

        mob_arc
    }

    #[must_use]
    pub fn is_age_locked(&self) -> bool {
        self.age_locked.load(Relaxed)
    }

    /// `Tadpole.ageUp` (`Tadpole.java:238-247`).
    async fn grow_into_frog(&self) {
        let entity = self.get_entity();
        let world = entity.world.load();
        let pos = entity.pos.load();
        // Vanilla plays the sound inside the conversion callback, before the tadpole is
        // discarded; `convert_to` removes it, so the sound is played first at the same spot.
        world.play_sound(Sound::EntityTadpoleGrowUp, SoundCategory::Neutral, &pos);
        zombification::convert_to(&self.mob_entity, &EntityType::FROG, false, FrogEntity::new)
            .await;
    }
}

impl NBTStorage for TadpoleEntity {
    fn write_nbt<'a>(&'a self, nbt: &'a mut NbtCompound) -> NbtFuture<'a, ()> {
        Box::pin(async move {
            self.mob_entity.living_entity.write_nbt(nbt).await;
            // `Tadpole.addAdditionalSaveData` (`Tadpole.java:113-117`).
            nbt.put_int("Age", self.age.load(Relaxed));
            nbt.put_bool("AgeLocked", self.is_age_locked());
        })
    }

    fn read_nbt_non_mut<'a>(&'a self, nbt: &'a NbtCompound) -> NbtFuture<'a, ()> {
        Box::pin(async move {
            self.mob_entity.living_entity.read_nbt_non_mut(nbt).await;
            // `Tadpole.readAdditionalSaveData` (`Tadpole.java:119-123`).
            self.age.store(nbt.get_int("Age").unwrap_or(0), Relaxed);
            self.age_locked
                .store(nbt.get_bool("AgeLocked").unwrap_or(false), Relaxed);
        })
    }
}

impl Mob for TadpoleEntity {
    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    fn mob_init_data_tracker(&self) -> EntityBaseFuture<'_, ()> {
        Box::pin(async move {
            self.get_entity().send_meta_data(
                &[Metadata::new(
                    tracked_data::tadpole::AGE_LOCKED,
                    self.is_age_locked(),
                )],
                None,
            );
        })
    }

    /// `Tadpole.aiStep` (`Tadpole.java:103-106`) plus the growth check `setAge` performs
    /// (`Tadpole.java:231-236`).
    fn mob_tick<'a>(&'a self, _caller: &'a Arc<dyn EntityBase>) -> EntityBaseFuture<'a, ()> {
        Box::pin(async move {
            if !self.get_entity().is_alive() || self.is_age_locked() {
                return;
            }

            let age = self.age.fetch_add(1, Relaxed) + 1;
            if age >= TICKS_TO_BE_FROG {
                self.grow_into_frog().await;
            }
        })
    }
}
