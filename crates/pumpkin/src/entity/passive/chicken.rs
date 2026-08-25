use std::sync::{
    Arc, Weak,
    atomic::{AtomicI32, AtomicU8, Ordering, Ordering::Relaxed},
};

use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::Sound;
use pumpkin_data::{entity::EntityType, item::Item};
use pumpkin_protocol::codec::var_int::VarInt;
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

const TEMPT_ITEMS: &[&Item] = &[
    &Item::WHEAT_SEEDS,
    &Item::MELON_SEEDS,
    &Item::PUMPKIN_SEEDS,
    &Item::BEETROOT_SEEDS,
    &Item::TORCHFLOWER_SEEDS,
    &Item::PITCHER_POD,
];

/// Sentinel meaning "no variant rolled yet" -- `mob_init_data_tracker` rolls a biome-based
/// variant the same way `SheepEntity`'s `COLOR_UNSET` does (sheep.rs:48-63): NBT restore runs
/// before `init_data_tracker` (entity/mod.rs:4916-4921), so a loaded chicken keeps its stored
/// variant while a fresh spawn gets rolled.
const VARIANT_UNSET: u8 = 0xFF;

/// Vanilla registry ids of the `minecraft:chicken_variant` entries
/// (`data/minecraft/chicken_variant/{cold,temperate,warm}.json`; identical ordering in
/// Pumpkin's generated `registry.rs`): cold = 0, temperate = 1, warm = 2. This is the value
/// vanilla syncs through `DATA_VARIANT_ID` (Chicken.java:62) and stores under the `variant`
/// NBT key via `VariantUtils.writeVariant`/`readVariant` (VariantUtils.java:26-32,
/// Chicken.java:216,227). The existing cold/temperate/warm mapping below predates this
/// module-level naming and matches it exactly.
mod chicken_variant {
    pub const COLD: u8 = 0;
    pub const TEMPERATE: u8 = 1;
    pub const WARM: u8 = 2;

    /// Vanilla `Chicken.finalizeSpawn`'s `VariantUtils.selectVariantToSpawn` over
    /// `Registries.CHICKEN_VARIANT` (Chicken.java:188): `cold.json`'s priority-1 selector
    /// gates on `#minecraft:spawns_cold_variant_farm_animals`, `warm.json`'s on
    /// `#minecraft:spawns_warm_variant_farm_animals`, and temperate is the unconditional
    /// priority-0 fallback. The two tags are disjoint in vanilla data, so a positive test of
    /// each in order reproduces `PriorityProvider.pick`; an unresolved biome is in neither,
    /// which selects temperate.
    pub fn select_spawn_variant(biome: Option<&'static pumpkin_data::biome::Biome>) -> u8 {
        use pumpkin_data::tag::{Taggable, WorldgenBiome};
        let has = |t: &'static pumpkin_data::tag::Tag| biome.is_some_and(|b| b.has_tag(t));
        if has(&WorldgenBiome::MINECRAFT_SPAWNS_WARM_VARIANT_FARM_ANIMALS) {
            WARM
        } else if has(&WorldgenBiome::MINECRAFT_SPAWNS_COLD_VARIANT_FARM_ANIMALS) {
            COLD
        } else {
            TEMPERATE
        }
    }
}

/// Represents a Chicken, a passive mob that lays eggs and is immune to fall damage.
///
/// Wiki: <https://minecraft.wiki/w/Chicken>
pub struct ChickenEntity {
    pub mob_entity: MobEntity,
    pub variant: AtomicU8,
    egg_lay_time: AtomicI32,
    pub ageable_data: crate::entity::ageable::AgeableData,
}

impl ChickenEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let mob_entity = MobEntity::new(entity);
        let egg_lay_time = rand::rng().random_range(6000..12000);
        let chicken = Self {
            mob_entity,
            variant: AtomicU8::new(VARIANT_UNSET), // rolled from the spawn biome
            egg_lay_time: AtomicI32::new(egg_lay_time),
            ageable_data: crate::entity::ageable::AgeableData::default(),
        };
        let mob_arc = Arc::new(chicken);
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
            goal_selector.add_goal(1, EscapeDangerGoal::new(1.4));
            goal_selector.add_goal(2, BreedGoal::new(1.0));
            goal_selector.add_goal(3, Box::new(TemptGoal::new(1.0, TEMPT_ITEMS, false)));
            goal_selector.add_goal(4, Box::new(FollowParentGoal::new(1.1)));
            goal_selector.add_goal(5, Box::new(WanderAroundGoal::new(1.0)));
            goal_selector.add_goal(
                6,
                LookAtEntityGoal::with_default(mob_weak, &EntityType::PLAYER, 6.0),
            );
            goal_selector.add_goal(7, Box::new(RandomLookAroundGoal::default()));
        };

        mob_arc
    }
}

impl crate::entity::ageable::AgeableMob for ChickenEntity {
    fn get_ageable_data(&self) -> &crate::entity::ageable::AgeableData {
        &self.ageable_data
    }

    fn baby_dimensions(&self) -> Option<EntityDimensions> {
        Some(EntityDimensions::new(0.3, 0.4, 0.28125))
    }
}

impl NBTStorage for ChickenEntity {
    fn write_nbt<'a>(&'a self, nbt: &'a mut NbtCompound) -> NbtFuture<'a, ()> {
        Box::pin(async {
            self.mob_entity.living_entity.write_nbt(nbt).await;
            self.write_ageable_nbt(nbt);
            self.write_animal_nbt(nbt);
            nbt.put_int("EggLayTime", self.egg_lay_time.load(Ordering::Relaxed));
            let variant_str = match self.variant.load(Ordering::Relaxed) {
                0 => "minecraft:cold",
                2 => "minecraft:warm",
                _ => "minecraft:temperate",
            };
            nbt.put_string("variant", variant_str.to_string());
        })
    }

    fn read_nbt_non_mut<'a>(&'a self, nbt: &'a NbtCompound) -> NbtFuture<'a, ()> {
        Box::pin(async {
            self.mob_entity.living_entity.read_nbt_non_mut(nbt).await;
            self.read_ageable_nbt(nbt);
            self.read_animal_nbt(nbt);
            self.egg_lay_time
                .store(nbt.get_int("EggLayTime").unwrap_or(6000), Ordering::Relaxed);
            if let Some(variant_str) = nbt.get_string("variant") {
                let variant = match variant_str
                    .strip_prefix("minecraft:")
                    .unwrap_or(variant_str)
                {
                    "cold" => 0,
                    "warm" => 2,
                    _ => 1,
                };
                self.variant.store(variant, Ordering::Relaxed);
            }
        })
    }
}

impl super::animal::Animal for ChickenEntity {
    fn is_food(&self, item_stack: &ItemStack) -> bool {
        use pumpkin_data::tag::Taggable;
        item_stack
            .item
            .has_tag(&pumpkin_data::tag::Item::MINECRAFT_CHICKEN_FOOD)
            || TEMPT_ITEMS.iter().any(|i| i.id == item_stack.item.id)
    }
}

impl Mob for ChickenEntity {
    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    fn mob_set_variant_name(&self, name: &str) {
        let variant = match name.strip_prefix("minecraft:").unwrap_or(name) {
            "cold" => 0,
            "warm" => 2,
            _ => 1,
        };
        self.variant.store(variant, Ordering::Relaxed);
    }

    /// Vanilla `Chicken.getBreedOffspring` (Chicken.java:178): the chick takes one of its
    /// parents' variants at random.
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

            if let Some(mate_chicken) = mate.cast_any().downcast_ref::<Self>() {
                // Normalize through the same table write_nbt uses so an unset parent
                // contributes temperate rather than the raw sentinel.
                let normalize = |v: u8| match v {
                    0 => chicken_variant::COLD,
                    2 => chicken_variant::WARM,
                    _ => chicken_variant::TEMPERATE,
                };
                let picked = if rand::rng().random_bool(0.5) {
                    normalize(self.variant.load(Ordering::Relaxed))
                } else {
                    normalize(mate_chicken.variant.load(Ordering::Relaxed))
                };
                if let Some(chick) = baby.cast_any().downcast_ref::<Self>() {
                    chick.variant.store(picked, Ordering::Relaxed);
                }
            }

            Some(baby)
        })
    }

    fn mob_init_data_tracker(&self) -> EntityBaseFuture<'_, ()> {
        Box::pin(async move {
            let entity = self.get_entity();

            // `Chicken.finalizeSpawn` (Chicken.java:188): roll the variant from the spawn
            // biome once, before first sync. A value already settled by NBT restore,
            // `mob_set_variant_name` or `create_offspring` is kept as-is.
            if self.variant.load(Ordering::Relaxed) == VARIANT_UNSET {
                let world = entity.world.load();
                let pos = entity.block_pos.load();
                let variant = chicken_variant::select_spawn_variant(world.get_biome(&pos));
                self.variant.store(variant, Ordering::Relaxed);
            }

            let is_baby = entity.age.load(Ordering::Relaxed) < 0;
            if is_baby {
                entity.send_meta_data(
                    &[pumpkin_protocol::java::client::play::Metadata::new(
                        pumpkin_data::tracked_data::chicken::BABY_ID,
                        true,
                    )],
                    None,
                );
            }
            entity.send_meta_data(
                &[pumpkin_protocol::java::client::play::Metadata::new(
                    pumpkin_data::tracked_data::chicken::VARIANT,
                    VarInt(self.variant.load(Ordering::Relaxed) as i32),
                )],
                None,
            );
        })
    }

    fn mob_tick<'a>(&'a self, _caller: &'a Arc<dyn EntityBase>) -> EntityBaseFuture<'a, ()> {
        Box::pin(async {
            if self.mob_entity.living_entity.dead.load(Relaxed) {
                return;
            }
            let entity = &self.mob_entity.living_entity.entity;
            let current_velocity = entity.velocity.load();
            let on_ground = entity.on_ground.load(Ordering::Relaxed);

            // TODO: move velocity logic to physics tick when implemented
            if (!on_ground) && current_velocity.y < 0.0 {
                entity.set_velocity(current_velocity.multiply(1.0, 0.6, 1.0));
            }
            if self.egg_lay_time.fetch_sub(1, Ordering::Relaxed) <= 1 {
                let next_time = rand::rng().random_range(6000..12000);
                let world = entity.world.load_full();
                let pos = entity.block_pos.load();
                let mut drop_event =
                    crate::plugin::api::events::entity::entity_drop_item::EntityDropItemEvent::new(
                        entity.entity_id,
                        "minecraft:egg".to_string(),
                        1,
                    );
                if let Some(server) = world.server.upgrade() {
                    server.plugin_manager.fire(&server, &mut drop_event).await;
                }
                if !drop_event.cancelled {
                    world.drop_stack(&pos, ItemStack::new(1, &Item::EGG)).await;
                }
                self.egg_lay_time.store(next_time, Ordering::Relaxed);
            }
        })
    }

    fn mob_interact<'a>(
        &'a self,
        player: &'a Arc<Player>,
        item_stack: &'a mut ItemStack,
    ) -> EntityBaseFuture<'a, bool> {
        use super::animal::Animal;
        self.animal_interact(player, item_stack, Sound::EntityChickenAmbient)
    }
}
