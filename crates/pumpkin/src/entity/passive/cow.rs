use std::sync::{
    Arc, Weak,
    atomic::{AtomicU8, Ordering},
};

use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::Sound;
use pumpkin_data::{entity::EntityType, item::Item};
use pumpkin_protocol::codec::var_int::VarInt;
use pumpkin_protocol::java::client::play::Metadata;
use pumpkin_util::math::boundingbox::EntityDimensions;
use rand::RngExt;

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
use crate::world::World;
use pumpkin_nbt::compound::NbtCompound;

const TEMPT_ITEMS: &[&Item] = &[&Item::WHEAT];

/// Sentinel meaning "no variant rolled yet" -- `mob_init_data_tracker` rolls a biome-based
/// variant the same way `SheepEntity`'s `COLOR_UNSET` does (sheep.rs:48-63): NBT restore runs
/// before `init_data_tracker` (entity/mod.rs:4916-4921), so a loaded cow keeps its stored
/// variant while a fresh spawn gets rolled.
const VARIANT_UNSET: u8 = 0xFF;

/// Vanilla registry ids of the `minecraft:cow_variant` entries (`CowVariants.java`, data files
/// `data/minecraft/cow_variant/{cold,temperate,warm}.json`; identical ordering in Pumpkin's
/// generated `registry.rs`): cold = 0, temperate = 1, warm = 2. This is the value vanilla
/// syncs through `DATA_VARIANT_ID` (Cow.java:33) and stores under the `variant` NBT key via
/// `VariantUtils.writeVariant`/`readVariant` (VariantUtils.java:26-32, Cow.java:57,66).
mod cow_variant {
    pub const COLD: u8 = 0;
    pub const TEMPERATE: u8 = 1;
    pub const WARM: u8 = 2;

    pub const fn name(id: u8) -> &'static str {
        match id {
            COLD => "minecraft:cold",
            WARM => "minecraft:warm",
            _ => "minecraft:temperate",
        }
    }

    /// Inverse of [`name`] for the keys vanilla actually writes; unknown ids fall back to
    /// temperate exactly like a missing/default entry.
    pub fn from_name(name: &str) -> Option<u8> {
        match name.strip_prefix("minecraft:").unwrap_or(name) {
            "cold" => Some(COLD),
            "warm" => Some(WARM),
            "temperate" => Some(TEMPERATE),
            _ => None,
        }
    }
}

/// Represents a Cow, a common passive mob that provides milk, leather, and beef.
///
/// Wiki: <https://minecraft.wiki/w/Cow>
pub struct CowEntity {
    pub mob_entity: MobEntity,
    /// Vanilla `DATA_VARIANT_ID` (Cow.java:33): the registry id of the cow's
    /// [`cow_variant`] entry, synced to clients so they pick the model/texture.
    pub variant: AtomicU8,
    pub ageable_data: crate::entity::ageable::AgeableData,
}

impl CowEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let mob_entity = MobEntity::new(entity);
        let cow = Self {
            mob_entity,
            variant: AtomicU8::new(VARIANT_UNSET),
            ageable_data: crate::entity::ageable::AgeableData::default(),
        };
        let mob_arc = Arc::new(cow);
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
            goal_selector.add_goal(1, EscapeDangerGoal::new(2.0));
            goal_selector.add_goal(2, BreedGoal::new(1.0));
            goal_selector.add_goal(3, Box::new(TemptGoal::new(1.25, TEMPT_ITEMS, false)));
            goal_selector.add_goal(4, Box::new(FollowParentGoal::new(1.25)));
            goal_selector.add_goal(5, Box::new(WanderAroundGoal::new_water_avoiding(1.0)));
            goal_selector.add_goal(
                6,
                LookAtEntityGoal::with_default(mob_weak, &EntityType::PLAYER, 6.0),
            );
            goal_selector.add_goal(7, Box::new(RandomLookAroundGoal::default()));
        };

        mob_arc
    }

    pub fn get_variant(&self) -> u8 {
        let variant = self.variant.load(Ordering::Relaxed);
        if variant == VARIANT_UNSET {
            cow_variant::TEMPERATE
        } else {
            variant
        }
    }

    /// Stores the variant and syncs `DATA_VARIANT_ID` (Cow.java:90-92), the way
    /// `CatEntity::set_variant` does for cats.
    pub fn set_variant(&self, variant: u8) {
        self.variant.store(variant, Ordering::Relaxed);
        self.mob_entity.living_entity.entity.send_meta_data(
            &[Metadata::new(
                pumpkin_data::tracked_data::cow::VARIANT,
                VarInt(i32::from(self.get_variant())),
            )],
            None,
        );
    }

    /// Vanilla `Cow.finalizeSpawn`'s `VariantUtils.selectVariantToSpawn` over
    /// `Registries.COW_VARIANT` (Cow.java:85): `cold.json`'s priority-1 selector gates on
    /// `#minecraft:spawns_cold_variant_farm_animals`, `warm.json`'s on
    /// `#minecraft:spawns_warm_variant_farm_animals`, and temperate is the unconditional
    /// priority-0 fallback. The two tags are disjoint in vanilla data, so a positive test of
    /// each in order reproduces `PriorityProvider.pick`; an unresolved biome is in neither,
    /// which selects temperate.
    fn select_spawn_variant(biome: Option<&'static pumpkin_data::biome::Biome>) -> u8 {
        use pumpkin_data::tag::Taggable;
        let has = |t: &'static pumpkin_data::tag::Tag| biome.is_some_and(|b| b.has_tag(t));
        if has(&pumpkin_data::tag::WorldgenBiome::MINECRAFT_SPAWNS_WARM_VARIANT_FARM_ANIMALS) {
            cow_variant::WARM
        } else if has(&pumpkin_data::tag::WorldgenBiome::MINECRAFT_SPAWNS_COLD_VARIANT_FARM_ANIMALS)
        {
            cow_variant::COLD
        } else {
            cow_variant::TEMPERATE
        }
    }
}

impl crate::entity::ageable::AgeableMob for CowEntity {
    fn get_ageable_data(&self) -> &crate::entity::ageable::AgeableData {
        &self.ageable_data
    }

    fn baby_dimensions(&self) -> Option<EntityDimensions> {
        Some(EntityDimensions::new(0.45, 0.7, 0.69))
    }
}

impl NBTStorage for CowEntity {
    fn write_nbt<'a>(&'a self, nbt: &'a mut NbtCompound) -> NbtFuture<'a, ()> {
        Box::pin(async move {
            self.mob_entity.living_entity.write_nbt(nbt).await;
            self.write_ageable_nbt(nbt);
            self.write_animal_nbt(nbt);
            // `VariantUtils.writeVariant` (VariantUtils.java:26-28, Cow.java:57): the variant
            // resource key under the `variant` key. The pre-roll sentinel normalizes to
            // temperate, the registry default `define`d in the constructor (Cow.java:49) --
            // same unavoidable-default reasoning as sheep.rs:57-63.
            nbt.put_string("variant", cow_variant::name(self.get_variant()).to_string());
        })
    }

    fn read_nbt_non_mut<'a>(&'a self, nbt: &'a NbtCompound) -> NbtFuture<'a, ()> {
        Box::pin(async move {
            self.mob_entity.living_entity.read_nbt_non_mut(nbt).await;
            self.read_ageable_nbt(nbt);
            self.read_animal_nbt(nbt);
            // `VariantUtils.readVariant` (VariantUtils.java:30-32, Cow.java:66): an absent or
            // unknown id leaves the sentinel so `mob_init_data_tracker` still rolls a
            // biome-based variant for legacy saves that never stored one.
            if let Some(name) = nbt.get_string("variant")
                && let Some(variant) = cow_variant::from_name(name)
            {
                self.variant.store(variant, Ordering::Relaxed);
            }
        })
    }
}

impl super::animal::Animal for CowEntity {
    fn is_food(&self, item_stack: &ItemStack) -> bool {
        use pumpkin_data::tag::Taggable;
        item_stack
            .item
            .has_tag(&pumpkin_data::tag::Item::MINECRAFT_COW_FOOD)
            || TEMPT_ITEMS.iter().any(|i| i.id == item_stack.item.id)
    }
}

impl Mob for CowEntity {
    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    /// Vanilla `Cow.setVariant` accepts any registered cow variant key (Cow.java:90-92); the
    /// spawn-egg data-component path resolves names through this hook.
    fn mob_set_variant_name(&self, name: &str) {
        if let Some(variant) = cow_variant::from_name(name) {
            self.variant.store(variant, Ordering::Relaxed);
        }
    }

    /// Sends the tracked variant (Cow.java:33, defined to temperate at Cow.java:49) and the
    /// baby flag, rolling a biome-based variant first for fresh spawns (`Cow.finalizeSpawn`,
    /// Cow.java:85). This override replaces `Mob::mob_init_data_tracker`'s default body, so
    /// the `BABY_ID` send from mob/mod.rs:2276-2294 is replicated here exactly like
    /// sheep.rs:378-404 does.
    fn mob_init_data_tracker(&self) -> EntityBaseFuture<'_, ()> {
        Box::pin(async move {
            let entity = &self.mob_entity.living_entity.entity;

            if self.variant.load(Ordering::Relaxed) == VARIANT_UNSET {
                let world = entity.world.load();
                let pos = entity.block_pos.load();
                let variant = Self::select_spawn_variant(world.get_biome(&pos));
                self.variant.store(variant, Ordering::Relaxed);
            }

            entity.send_meta_data(
                &[Metadata::new(
                    pumpkin_data::tracked_data::cow::VARIANT,
                    VarInt(i32::from(self.get_variant())),
                )],
                None,
            );

            if entity.age.load(Ordering::Relaxed) < 0 {
                entity.send_meta_data(
                    &[Metadata::new(
                        pumpkin_data::tracked_data::ageable_mob::DATA_BABY_ID,
                        true,
                    )],
                    None,
                );
            }
        })
    }

    fn get_walk_target_value(&self, pos: &pumpkin_util::math::position::BlockPos) -> f64 {
        super::animal::Animal::get_walk_target_value(self, pos)
    }

    /// Vanilla `Cow.getBreedOffspring` (Cow.java:75): the calf takes one of its parents'
    /// variants at random.
    fn create_offspring<'a>(
        &'a self,
        mate: &'a dyn EntityBase,
        world: &'a Arc<World>,
    ) -> EntityBaseFuture<'a, Option<Arc<dyn EntityBase>>> {
        Box::pin(async move {
            let entity = self.get_entity();
            let baby = crate::entity::r#type::from_type(
                entity.entity_type,
                entity.pos.load(),
                world,
                uuid::Uuid::new_v4(),
            );

            if let Some(mate_cow) = mate.cast_any().downcast_ref::<Self>() {
                let picked = if rand::rng().random_bool(0.5) {
                    self.get_variant()
                } else {
                    mate_cow.get_variant()
                };
                if let Some(calf) = baby.cast_any().downcast_ref::<Self>() {
                    calf.variant.store(picked, Ordering::Relaxed);
                }
            }

            Some(baby)
        })
    }

    fn mob_interact<'a>(
        &'a self,
        player: &'a Arc<Player>,
        item_stack: &'a mut ItemStack,
    ) -> EntityBaseFuture<'a, bool> {
        use super::animal::Animal;
        self.animal_interact(player, item_stack, Sound::EntityCowAmbient)
    }
}
