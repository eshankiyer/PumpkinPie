use std::sync::{
    Arc, Weak,
    atomic::{AtomicBool, Ordering},
};

use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_data::tag::{self, Taggable};
use pumpkin_data::{entity::EntityType, item::Item};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_protocol::java::client::play::Metadata;
use pumpkin_util::GameMode;
use pumpkin_util::math::boundingbox::EntityDimensions;
use rand::RngExt;

use crate::entity::{
    Entity, EntityBaseFuture, NBTStorage, NbtFuture,
    ageable::{AgeableData, AgeableMob},
    ai::goal::{
        breed::BreedGoal, follow_parent::FollowParentGoal, goat_ram::GoatRamGoal,
        long_jump_to_random_pos::LongJumpToRandomPosGoal, look_around::RandomLookAroundGoal,
        look_at_entity::LookAtEntityGoal, swim::SwimGoal, tempt::TemptGoal,
        wander_around::WanderAroundGoal,
    },
    mob::{Mob, MobEntity},
    player::Player,
};

const TEMPT_ITEMS: &[&Item] = &[&Item::WHEAT];

/// Vanilla `Goat.BABY_DIMENSIONS` (`Goat.java:57-61`) supplies the baby width, height, and eye
/// height used by `Goat.getDefaultDimensions` (`Goat.java:250-254`).
const fn goat_baby_dimensions() -> EntityDimensions {
    EntityDimensions::new(0.45, 0.65, 0.59375)
}

/// `GoatAi.TIME_BETWEEN_LONG_JUMPS = UniformInt.of(600, 1200)` (`GoatAi.java:45`).
const TIME_BETWEEN_LONG_JUMPS: (i32, i32) = (600, 1200);
/// `GoatAi.MAX_LONG_JUMP_HEIGHT` (`GoatAi.java:46`).
const MAX_LONG_JUMP_HEIGHT: i32 = 5;
/// `GoatAi.MAX_LONG_JUMP_WIDTH` (`GoatAi.java:47`).
const MAX_LONG_JUMP_WIDTH: i32 = 5;
/// `GoatAi.MAX_JUMP_VELOCITY_MULTIPLIER` (`GoatAi.java:48`).
const MAX_JUMP_VELOCITY_MULTIPLIER: f64 = 3.571_428_8;

/// `GoatAi.java:119`: a screaming goat has its own long-jump sound.
fn goat_long_jump_sound(mob: &dyn Mob) -> Sound {
    let screaming = mob
        .cast_any()
        .downcast_ref::<GoatEntity>()
        .is_some_and(GoatEntity::is_screaming_goat);
    if screaming {
        Sound::EntityGoatScreamingLongJump
    } else {
        Sound::EntityGoatLongJump
    }
}

/// `Goat.GOAT_SCREAMING_CHANCE` (`Goat.java:76`).
const GOAT_SCREAMING_CHANCE: f64 = 0.02;
/// `Goat.UNIHORN_CHANCE` (`Goat.java:77`).
const UNIHORN_CHANCE: f32 = 0.1;

/// Represents a Goat, a neutral mob that can jump high and ram players or other mobs.
///
/// Wiki: <https://minecraft.wiki/w/Goat>
///
/// Vanilla's goat is brain-driven (`Goat.BRAIN_PROVIDER`, `Goat.java:67-75`). No brain is built
/// here; the ram behaviour keeps running through `GoatRamGoal` and the state that vanilla stores
/// in `SynchedEntityData` -- `DATA_IS_SCREAMING_GOAT`, `DATA_HAS_LEFT_HORN`, `DATA_HAS_RIGHT_HORN`
/// (`Goat.java:78-80`) -- is carried as plain atomics, the same shape `warden.rs` uses.
///
/// Ported in this pass: the milking branch of `mobInteract` (`Goat.java:217-224`), the milking
/// sound selection (`getMilkingSound`, `Goat.java:147-149`), the screaming/unihorn rolls of
/// `finalizeSpawn` (`Goat.java:239-246`), their NBT round-trip (`addAdditionalSaveData` /
/// `readAdditionalSaveData`, `Goat.java:259-272`), and the `Animal` half of `mobInteract` that
/// vanilla reaches through `super.mobInteract` -- a goat previously had no `Animal` impl at all,
/// so it could neither be fed nor bred.
///
/// Also ported: `GoatAi`'s long-jump activity (`GoatAi.java:109-122`), through the new
/// `LongJumpToRandomPosGoal`. That goal's module doc lists the deviations forced by the missing
/// memory system and by unbounded per-tick pathfinding in the vanilla original.
///
/// NOT ported, with reasons:
///
/// - `createHorn` (`Goat.java:100-107`): needs the `Instrument` registry and
///   `InstrumentItem.create` to stamp a goat-horn variant, which does not exist here. Horn
///   presence is tracked and persisted, but a dropped horn item is never produced.
/// - `isLoweringHead`/`lowerHeadTick` (`Goat.java:283-292`) and the entity events 58/59 that
///   drive them: client-side animation state only.
/// - The `POWDER_SNOW` pathfinding malus (`Goat.java:92-95`), which lives outside this file.
///   `calculateFallDamage`'s `GOAT_FALL_DAMAGE_REDUCTION` (`Goat.java:121-125`) is now ported,
///   through `Mob::mob_calculate_fall_damage` below.
pub struct GoatEntity {
    pub mob_entity: MobEntity,
    pub ageable_data: AgeableData,
    /// `Goat.DATA_IS_SCREAMING_GOAT`.
    is_screaming: AtomicBool,
    /// `Goat.DATA_HAS_LEFT_HORN`, default `true`.
    has_left_horn: AtomicBool,
    /// `Goat.DATA_HAS_RIGHT_HORN`, default `true`.
    has_right_horn: AtomicBool,
}

impl GoatEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let is_baby = entity.age.load(Ordering::Relaxed) < 0;
        let mob_entity = MobEntity::new(entity);

        // `Goat.finalizeSpawn` (`Goat.java:236-246`). Pumpkin has no `finalizeSpawn` hook, and a
        // goat loaded from disk overwrites all three flags in `read_nbt_non_mut`, so rolling
        // them here is equivalent for freshly spawned goats and harmless for loaded ones.
        let mut rng = rand::rng();
        let is_screaming = rng.random::<f64>() < GOAT_SCREAMING_CHANCE;
        let (mut left_horn, mut right_horn) = (true, true);
        if !is_baby && rng.random::<f32>() < UNIHORN_CHANCE {
            if rng.random::<bool>() {
                left_horn = false;
            } else {
                right_horn = false;
            }
        }

        let goat = Self {
            mob_entity,
            ageable_data: AgeableData::default(),
            is_screaming: AtomicBool::new(is_screaming),
            has_left_horn: AtomicBool::new(left_horn),
            has_right_horn: AtomicBool::new(right_horn),
        };
        let mob_arc = Arc::new(goat);
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
            goal_selector.add_goal(1, Box::new(GoatRamGoal::new()));
            // `GoatAi.initLongJumpActivity` (`GoatAi.java:109-122`). Vanilla runs the long-jump
            // activity above IDLE and below RAM, which is what priority 2 buys here.
            goal_selector.add_goal(
                2,
                Box::new(LongJumpToRandomPosGoal::new(
                    TIME_BETWEEN_LONG_JUMPS.0,
                    TIME_BETWEEN_LONG_JUMPS.1,
                    MAX_LONG_JUMP_HEIGHT,
                    MAX_LONG_JUMP_WIDTH,
                    MAX_JUMP_VELOCITY_MULTIPLIER,
                    goat_long_jump_sound,
                    Sound::EntityGoatStep,
                )),
            );
            goal_selector.add_goal(3, BreedGoal::new(1.0));
            goal_selector.add_goal(4, Box::new(TemptGoal::new(1.25, TEMPT_ITEMS, false)));
            goal_selector.add_goal(5, Box::new(FollowParentGoal::new(1.25)));
            goal_selector.add_goal(6, Box::new(WanderAroundGoal::new(1.0)));
            goal_selector.add_goal(
                7,
                LookAtEntityGoal::with_default(mob_weak, &EntityType::PLAYER, 6.0),
            );
            goal_selector.add_goal(8, Box::new(RandomLookAroundGoal::default()));
        };

        mob_arc
    }

    #[must_use]
    pub fn is_screaming_goat(&self) -> bool {
        self.is_screaming.load(Ordering::Relaxed)
    }

    /// `Goat.setScreamingGoat`.
    pub fn set_screaming_goat(&self, screaming: bool) {
        self.is_screaming.store(screaming, Ordering::Relaxed);
        self.mob_entity.living_entity.entity.send_meta_data(
            &[Metadata::new(
                pumpkin_data::tracked_data::goat::IS_SCREAMING_GOAT,
                screaming,
            )],
            None,
        );
    }

    #[must_use]
    pub fn has_left_horn(&self) -> bool {
        self.has_left_horn.load(Ordering::Relaxed)
    }

    #[must_use]
    pub fn has_right_horn(&self) -> bool {
        self.has_right_horn.load(Ordering::Relaxed)
    }

    /// `Goat.getMilkingSound` (`Goat.java:147-149`).
    #[must_use]
    fn milking_sound(&self) -> Sound {
        if self.is_screaming.load(Ordering::Relaxed) {
            Sound::EntityGoatScreamingMilk
        } else {
            Sound::EntityGoatMilk
        }
    }

    /// `Goat.playEatingSound` (`Goat.java:196-209`).
    #[must_use]
    fn eating_sound(&self) -> Sound {
        if self.is_screaming.load(Ordering::Relaxed) {
            Sound::EntityGoatScreamingEat
        } else {
            Sound::EntityGoatEat
        }
    }

    /// `ItemUtils.createFilledResult(heldStack, player, filled, true)` (`ItemUtils.java:16-37`)
    /// specialised to the one caller here: `Goat.mobInteract` fills a bucket with milk.
    ///
    /// `item_stack` is the player's held stack, which `EntityBase::interact`'s caller writes
    /// back into the held slot (`net/java/play/interact.rs:122-129`), so the empty-bucket half
    /// of the swap is done by mutating it in place.
    async fn fill_bucket_with_milk(&self, player: &Arc<Player>, item_stack: &mut ItemStack) {
        let filled = ItemStack::new(1, &Item::MILK_BUCKET);

        // `limitCreativeStackSize` is `true` for this call site, so a creative player keeps the
        // bucket and only gains a milk bucket if they do not already have one.
        if player.gamemode.load() == GameMode::Creative {
            if !player.inventory.contains_item(&Item::MILK_BUCKET) {
                let mut filled = filled;
                player.inventory.insert_stack_anywhere(&mut filled).await;
            }
            return;
        }

        item_stack.decrement(1);
        if item_stack.is_empty() {
            *item_stack = filled;
        } else {
            player
                .inventory
                .offer_or_drop_stack(filled, &**player)
                .await;
        }
    }
}

impl AgeableMob for GoatEntity {
    fn get_ageable_data(&self) -> &AgeableData {
        &self.ageable_data
    }

    /// Vanilla `Goat.getDefaultDimensions` (`Goat.java:250-254`) selects `BABY_DIMENSIONS`
    /// (`Goat.java:57-65`) for a baby before applying the long-jump pose scale.
    fn baby_dimensions(&self) -> Option<EntityDimensions> {
        Some(goat_baby_dimensions())
    }
}

impl NBTStorage for GoatEntity {
    fn write_nbt<'a>(&'a self, nbt: &'a mut NbtCompound) -> NbtFuture<'a, ()> {
        Box::pin(async move {
            use super::animal::Animal;
            self.mob_entity.living_entity.write_nbt(nbt).await;
            self.write_ageable_nbt(nbt);
            self.write_animal_nbt(nbt);
            // `Goat.addAdditionalSaveData` (`Goat.java:259-264`).
            nbt.put_bool("IsScreamingGoat", self.is_screaming_goat());
            nbt.put_bool("HasLeftHorn", self.has_left_horn());
            nbt.put_bool("HasRightHorn", self.has_right_horn());
        })
    }

    fn read_nbt_non_mut<'a>(&'a self, nbt: &'a NbtCompound) -> NbtFuture<'a, ()> {
        Box::pin(async move {
            use super::animal::Animal;
            self.mob_entity.living_entity.read_nbt_non_mut(nbt).await;
            self.read_ageable_nbt(nbt);
            self.read_animal_nbt(nbt);
            // `Goat.readAdditionalSaveData` (`Goat.java:266-272`): horns default to present,
            // screaming defaults to false.
            self.is_screaming.store(
                nbt.get_bool("IsScreamingGoat").unwrap_or(false),
                Ordering::Relaxed,
            );
            self.has_left_horn.store(
                nbt.get_bool("HasLeftHorn").unwrap_or(true),
                Ordering::Relaxed,
            );
            self.has_right_horn.store(
                nbt.get_bool("HasRightHorn").unwrap_or(true),
                Ordering::Relaxed,
            );
        })
    }
}

impl super::animal::Animal for GoatEntity {
    /// `Goat.isFood` (`Goat.java:212-214`).
    fn is_food(&self, item_stack: &ItemStack) -> bool {
        item_stack.item.has_tag(&tag::Item::MINECRAFT_GOAT_FOOD)
    }
}

impl Mob for GoatEntity {
    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    /// `Goat.calculateFallDamage` (`Goat.java:123-125`): goats take 10 less fall damage.
    fn mob_calculate_fall_damage(&self, fall_distance: f64, damage_modifier: f32) -> i32 {
        self.mob_entity
            .living_entity
            .default_calculate_fall_damage(fall_distance, damage_modifier)
            - 10
    }

    fn get_walk_target_value(&self, pos: &pumpkin_util::math::position::BlockPos) -> f64 {
        super::animal::Animal::get_walk_target_value(self, pos)
    }

    fn mob_init_data_tracker(&self) -> EntityBaseFuture<'_, ()> {
        Box::pin(async move {
            self.mob_entity.living_entity.entity.send_meta_data(
                &[
                    Metadata::new(
                        pumpkin_data::tracked_data::goat::IS_SCREAMING_GOAT,
                        self.is_screaming_goat(),
                    ),
                    Metadata::new(
                        pumpkin_data::tracked_data::goat::HAS_LEFT_HORN,
                        self.has_left_horn(),
                    ),
                    Metadata::new(
                        pumpkin_data::tracked_data::goat::HAS_RIGHT_HORN,
                        self.has_right_horn(),
                    ),
                ],
                None,
            );
        })
    }

    /// `Goat.mobInteract` (`Goat.java:216-232`): an empty bucket on a non-baby goat is filled
    /// with milk, otherwise the interaction falls through to `Animal.mobInteract` (feeding and
    /// breeding), which plays the eating sound when it consumes food.
    fn mob_interact<'a>(
        &'a self,
        player: &'a Arc<Player>,
        item_stack: &'a mut ItemStack,
    ) -> EntityBaseFuture<'a, bool> {
        Box::pin(async move {
            use super::animal::Animal;

            let entity = &self.mob_entity.living_entity.entity;
            let is_baby = entity.age.load(Ordering::Relaxed) < 0;
            if item_stack.item.id == Item::BUCKET.id && !is_baby {
                let world = entity.world.load();
                world.play_sound(
                    self.milking_sound(),
                    SoundCategory::Neutral,
                    &entity.pos.load(),
                );
                self.fill_bucket_with_milk(player, item_stack).await;
                return true;
            }

            self.animal_interact(player, item_stack, self.eating_sound())
                .await
        })
    }

    /// `Goat.getBreedOffspring` (`Goat.java:151-161`): the kid inherits screaming from one
    /// randomly chosen parent, or rolls the 2% chance independently.
    fn create_offspring<'a>(
        &'a self,
        mate: &'a dyn crate::entity::EntityBase,
        world: &'a Arc<crate::world::World>,
    ) -> EntityBaseFuture<'a, Option<Arc<dyn crate::entity::EntityBase>>> {
        Box::pin(async move {
            let entity = &self.mob_entity.living_entity.entity;
            let baby = crate::entity::r#type::from_type(
                entity.entity_type,
                entity.pos.load(),
                world,
                uuid::Uuid::new_v4(),
            );

            let mut rng = rand::rng();
            let parent_screaming = if rng.random::<bool>() {
                self.is_screaming_goat()
            } else {
                mate.cast_any()
                    .downcast_ref::<Self>()
                    .is_some_and(Self::is_screaming_goat)
            };
            if let Some(kid) = baby.cast_any().downcast_ref::<Self>() {
                kid.set_screaming_goat(
                    parent_screaming || rng.random::<f64>() < GOAT_SCREAMING_CHANCE,
                );
            }

            Some(baby)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::goat_baby_dimensions;

    #[test]
    fn baby_dimensions_match_vanilla_constants() {
        // Vanilla `Goat.BABY_DIMENSIONS` (`Goat.java:57-61`) supplies these dimensions.
        let dimensions = goat_baby_dimensions();
        assert_eq!(dimensions.width, 0.45);
        assert_eq!(dimensions.height, 0.65);
        assert_eq!(dimensions.eye_height, 0.59375);
    }
}
