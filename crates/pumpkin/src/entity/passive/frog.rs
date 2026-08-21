use std::sync::{
    Arc, Weak,
    atomic::{AtomicBool, AtomicI32, Ordering},
};

use pumpkin_data::entity::EntityType;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::Sound;
use pumpkin_data::tag::{self, Taggable};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_protocol::codec::var_int::VarInt;
use pumpkin_protocol::java::client::play::Metadata;

use crate::entity::{
    Entity, EntityBase, EntityBaseFuture, NBTStorage, NbtFuture,
    ageable::{AgeableData, AgeableMob},
    ai::goal::{
        breed::BreedGoal,
        frog_lay_spawn::FrogLaySpawnGoal,
        frog_tongue_attack::{FrogFindFoodGoal, FrogTongueAttackGoal},
        look_around::RandomLookAroundGoal,
        look_at_entity::LookAtEntityGoal,
        swim::SwimGoal,
        tempt::TemptGoal,
        wander_around::WanderAroundGoal,
    },
    mob::{Mob, MobEntity},
    passive::animal::Animal,
    player::Player,
};

pub const FROG_FOOD: &[&Item] = &[&Item::SLIME_BALL];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(i32)]
pub enum FrogVariant {
    Cold = 0,
    #[default]
    Temperate = 1,
    Warm = 2,
}

impl FrogVariant {
    #[must_use]
    pub const fn from_id(id: i32) -> Self {
        match id {
            0 => Self::Cold,
            2 => Self::Warm,
            _ => Self::Temperate,
        }
    }

    #[must_use]
    pub const fn id(self) -> i32 {
        self as i32
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Cold => "minecraft:cold",
            Self::Temperate => "minecraft:temperate",
            Self::Warm => "minecraft:warm",
        }
    }

    #[must_use]
    pub fn from_name(s: &str) -> Self {
        match s {
            "minecraft:cold" | "cold" => Self::Cold,
            "minecraft:warm" | "warm" => Self::Warm,
            _ => Self::Temperate,
        }
    }
}

/// Represents a Frog, an amphibious mob that can eat small slimes and magma cubes.
///
/// Wiki: <https://minecraft.wiki/w/Frog>
pub struct FrogEntity {
    pub mob_entity: MobEntity,
    pub ageable_data: AgeableData,
    pub variant: AtomicI32,
    pub tongue_target_id: AtomicI32,
    /// `MemoryModuleType.IS_PREGNANT` (`Frog.spawnChildFromBreeding`, `Frog.java:256-259`).
    /// A breeding frog produces no baby: it becomes pregnant and later lays frogspawn, which is
    /// what hatches into tadpoles. Pumpkin has no memory system, so the memory is a plain flag,
    /// the same shape `warden.rs` uses for the warden's activity state.
    is_pregnant: AtomicBool,
}

impl FrogEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let mob_entity = MobEntity::new(entity);
        let frog = Self {
            mob_entity,
            ageable_data: AgeableData::default(),
            variant: AtomicI32::new(FrogVariant::Temperate.id()),
            tongue_target_id: AtomicI32::new(-1),
            is_pregnant: AtomicBool::new(false),
        };
        let mob_arc = Arc::new(frog);
        let frog_weak = Arc::downgrade(&mob_arc);
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
            let mut target_selector = mob_arc
                .mob_entity
                .target_selector
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);

            goal_selector.add_goal(0, Box::new(SwimGoal::default()));
            goal_selector.add_goal(1, Box::new(FrogTongueAttackGoal::new()));
            goal_selector.add_goal(1, Box::new(TemptGoal::new(1.0, FROG_FOOD, false)));
            // `FrogAi.initLaySpawnActivity` (`FrogAi.java:143-168`) outranks the idle bundle,
            // and `AnimalMakeLove(FROG)` (`FrogAi.java:90`) is what `BreedGoal` stands in for.
            goal_selector.add_goal(1, FrogLaySpawnGoal::new(frog_weak, 1.0));
            goal_selector.add_goal(2, BreedGoal::new(1.0));
            goal_selector.add_goal(2, Box::new(WanderAroundGoal::new(1.0)));
            goal_selector.add_goal(
                3,
                LookAtEntityGoal::with_default(mob_weak, &EntityType::PLAYER, 6.0),
            );
            goal_selector.add_goal(4, Box::new(RandomLookAroundGoal::default()));

            target_selector.add_goal(1, Box::new(FrogFindFoodGoal::default()));
        };

        mob_arc
    }

    #[must_use]
    pub fn get_variant(&self) -> FrogVariant {
        FrogVariant::from_id(self.variant.load(Ordering::Relaxed))
    }

    pub fn set_variant(&self, variant: FrogVariant) {
        self.variant.store(variant.id(), Ordering::Relaxed);
        let entity = self.get_entity();
        entity.send_meta_data(
            &[Metadata::new(
                pumpkin_data::tracked_data::frog::VARIANT,
                VarInt(variant.id()),
            )],
            None,
        );
    }

    /// Whether `MemoryModuleType.IS_PREGNANT` is present (`FrogAi.java:163-165` gates the
    /// `LAY_SPAWN` activity on it).
    #[must_use]
    pub fn is_pregnant(&self) -> bool {
        self.is_pregnant.load(Ordering::Relaxed)
    }

    pub fn set_pregnant(&self, pregnant: bool) {
        self.is_pregnant.store(pregnant, Ordering::Relaxed);
    }
}

impl AgeableMob for FrogEntity {
    fn get_ageable_data(&self) -> &AgeableData {
        &self.ageable_data
    }
}

impl Animal for FrogEntity {
    fn is_food(&self, item_stack: &ItemStack) -> bool {
        item_stack.item.has_tag(&tag::Item::MINECRAFT_FROG_FOOD)
            || FROG_FOOD.iter().any(|i| i.id == item_stack.item.id)
    }
}

impl NBTStorage for FrogEntity {
    fn write_nbt<'a>(&'a self, nbt: &'a mut NbtCompound) -> NbtFuture<'a, ()> {
        Box::pin(async move {
            self.mob_entity.living_entity.write_nbt(nbt).await;
            self.write_ageable_nbt(nbt);
            self.write_animal_nbt(nbt);
            nbt.put_string("variant", self.get_variant().as_str().to_string());
            // Vanilla stores `IS_PREGNANT` inside the serialized brain; there is no brain here,
            // so it gets its own key. A frog written by vanilla therefore loads as not pregnant.
            nbt.put_bool("IsPregnant", self.is_pregnant());
        })
    }

    fn read_nbt_non_mut<'a>(&'a self, nbt: &'a NbtCompound) -> NbtFuture<'a, ()> {
        Box::pin(async move {
            self.mob_entity.living_entity.read_nbt_non_mut(nbt).await;
            self.read_ageable_nbt(nbt);
            self.read_animal_nbt(nbt);
            if let Some(variant_str) = nbt.get_string("variant") {
                self.set_variant(FrogVariant::from_name(variant_str));
            }
            self.is_pregnant.store(
                nbt.get_bool("IsPregnant").unwrap_or(false),
                Ordering::Relaxed,
            );
        })
    }
}

impl Mob for FrogEntity {
    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    /// `Frog.calculateFallDamage` (`Frog.java:304-306`): frogs take 5 less fall damage.
    fn mob_calculate_fall_damage(&self, fall_distance: f64, damage_modifier: f32) -> i32 {
        self.mob_entity
            .living_entity
            .default_calculate_fall_damage(fall_distance, damage_modifier)
            - 5
    }

    fn mob_set_variant_name(&self, name: &str) {
        self.set_variant(FrogVariant::from_name(name));
    }

    /// `Frog.spawnChildFromBreeding` (`Frog.java:255-259`): breeding frogs produce no child.
    /// Vanilla still calls `getBreedOffspring` (`Frog.java:241-248`) through
    /// `finalizeSpawnChildFromBreeding(level, partner, null)` with a null child, i.e. the
    /// offspring is discarded; only the XP orb and the love-mode reset survive. Returning `None`
    /// here reproduces that: `BreedGoal::breed` still applies both cooldowns and the XP drop.
    fn create_offspring<'a>(
        &'a self,
        _mate: &'a dyn EntityBase,
        _world: &'a Arc<crate::world::World>,
    ) -> EntityBaseFuture<'a, Option<Arc<dyn EntityBase>>> {
        Box::pin(async move { None })
    }

    /// The other half of `Frog.spawnChildFromBreeding`: the frog becomes pregnant instead, and
    /// `FrogLaySpawnGoal` turns that into a frogspawn block.
    fn spawn_breeding_result<'a>(
        &'a self,
        _offspring: Option<Arc<dyn EntityBase>>,
        _world: &'a Arc<crate::world::World>,
        _parent_pos: pumpkin_util::math::vector3::Vector3<f64>,
    ) -> EntityBaseFuture<'a, ()> {
        Box::pin(async move {
            self.set_pregnant(true);
        })
    }

    fn mob_tick<'a>(&'a self, _caller: &'a Arc<dyn EntityBase>) -> EntityBaseFuture<'a, ()> {
        Box::pin(async move {
            self.ageable_ai_step();
        })
    }

    fn mob_init_data_tracker(&self) -> EntityBaseFuture<'_, ()> {
        Box::pin(async move {
            let entity = self.get_entity();
            let is_baby = entity.age.load(Ordering::Relaxed) < 0;
            if is_baby {
                entity.send_meta_data(
                    &[Metadata::new(
                        pumpkin_data::tracked_data::frog::BABY_ID,
                        true,
                    )],
                    None,
                );
            }
            entity.send_meta_data(
                &[Metadata::new(
                    pumpkin_data::tracked_data::frog::VARIANT,
                    VarInt(self.get_variant().id()),
                )],
                None,
            );
        })
    }

    fn mob_interact<'a>(
        &'a self,
        player: &'a Arc<Player>,
        item_stack: &'a mut ItemStack,
    ) -> EntityBaseFuture<'a, bool> {
        Box::pin(async move {
            self.animal_interact(player, item_stack, Sound::EntityFrogAmbient)
                .await
        })
    }
}
