use std::sync::{
    Arc, Weak,
    atomic::{AtomicBool, AtomicI32, AtomicU8, Ordering::Relaxed},
};

use crossbeam::atomic::AtomicCell;
use pumpkin_data::attributes::Attributes;
use pumpkin_data::data_component_impl::EquipmentSlot;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_data::tag::{self, Taggable};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_protocol::codec::var_int::VarInt;
use pumpkin_protocol::java::client::play::Metadata;
use pumpkin_util::math::boundingbox::EntityDimensions;
use pumpkin_util::math::vector3::Vector3;
use rand::{RngExt, rng};
use uuid::Uuid;

use crate::entity::{
    Entity, EntityBase, EntityBaseFuture, NBTStorage, NbtFuture,
    ageable::{AgeableData, AgeableMob},
    ai::goal::{
        follow_parent::FollowParentGoal, look_around::RandomLookAroundGoal,
        panda_attack::PandaAttackGoal, panda_avoid::PandaAvoidGoal, panda_breed::PandaBreedGoal,
        panda_hurt_by_target::PandaHurtByTargetGoal, panda_lie_on_back::PandaLieOnBackGoal,
        panda_look_at_player::PandaLookAtPlayerGoal, panda_panic::PandaPanicGoal,
        panda_roll::PandaRollGoal, panda_sit::PandaSitGoal, panda_sneeze::PandaSneezeGoal,
        swim::SwimGoal, tempt::TemptGoal, wander_around::WanderAroundGoal,
    },
    mob::{Mob, MobEntity},
    passive::animal::Animal,
    player::Player,
};
use crate::world::World;

/// `ItemTags.PANDA_FOOD` is bamboo-only (`tag.rs`: `MINECRAFT_PANDA_FOOD = ["bamboo"]`);
/// `TemptGoal` takes a static item list rather than a tag, so the tag's single member is spelled
/// out here. `is_food` still goes through the tag itself.
const TEMPT_ITEMS: &[&Item] = &[&Item::BAMBOO];

/// `Panda.FLAG_SNEEZE`.
const FLAG_SNEEZE: u8 = 2;
/// `Panda.FLAG_ROLL`.
const FLAG_ROLL: u8 = 4;
/// `Panda.FLAG_SIT`.
const FLAG_SIT: u8 = 8;
/// `Panda.FLAG_ON_BACK`.
const FLAG_ON_BACK: u8 = 16;
/// `Panda.TOTAL_ROLL_STEPS`.
const TOTAL_ROLL_STEPS: i32 = 32;
/// `Panda.TOTAL_UNHAPPY_TIME`.
pub const TOTAL_UNHAPPY_TIME: i32 = 32;

/// Sentinel for "genes not yet rolled". Same reasoning as `FoxEntity`'s `VARIANT_UNSET`: there is
/// no `finalizeSpawn` hook in this codebase, so the spawn roll happens in
/// `mob_init_data_tracker`, which has to tell a fresh spawn apart from a panda restored from NBT
/// as NORMAL (0).
const GENE_UNSET: u8 = 0xFF;

/// `Panda.Gene` (`Panda.java:729-807`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum PandaGene {
    Normal = 0,
    Lazy = 1,
    Worried = 2,
    Playful = 3,
    Brown = 4,
    Weak = 5,
    Aggressive = 6,
}

impl PandaGene {
    /// `Panda.Gene.BY_ID`, a `ByIdMap.continuous(..., OutOfBoundsStrategy.ZERO)`: an id outside
    /// 0..=6 falls back to NORMAL.
    #[must_use]
    pub const fn from_id(id: u8) -> Self {
        match id {
            1 => Self::Lazy,
            2 => Self::Worried,
            3 => Self::Playful,
            4 => Self::Brown,
            5 => Self::Weak,
            6 => Self::Aggressive,
            _ => Self::Normal,
        }
    }

    #[must_use]
    pub const fn id(self) -> u8 {
        self as u8
    }

    /// `Panda.Gene.getSerializedName`.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Normal => "normal",
            Self::Lazy => "lazy",
            Self::Worried => "worried",
            Self::Playful => "playful",
            Self::Brown => "brown",
            Self::Weak => "weak",
            Self::Aggressive => "aggressive",
        }
    }

    #[must_use]
    pub fn from_name(name: &str) -> Self {
        match name.strip_prefix("minecraft:").unwrap_or(name) {
            "lazy" => Self::Lazy,
            "worried" => Self::Worried,
            "playful" => Self::Playful,
            "brown" => Self::Brown,
            "weak" => Self::Weak,
            "aggressive" => Self::Aggressive,
            _ => Self::Normal,
        }
    }

    /// `Panda.Gene.isRecessive` (`Panda.java:730-736`): only BROWN and WEAK are recessive.
    #[must_use]
    pub const fn is_recessive(self) -> bool {
        matches!(self, Self::Brown | Self::Weak)
    }

    /// `Panda.Gene.getVariantFromGenes` (`Panda.java:762-768`): a recessive main gene only
    /// expresses when the hidden gene matches it, otherwise the panda looks NORMAL. A dominant
    /// main gene always expresses.
    #[must_use]
    pub const fn variant_from_genes(main: Self, hidden: Self) -> Self {
        if main.is_recessive() {
            if main as u8 == hidden as u8 {
                main
            } else {
                Self::Normal
            }
        } else {
            main
        }
    }

    /// `Panda.Gene.getRandom` (`Panda.java:774-789`), split out from the RNG so the weighting is
    /// testable.
    ///
    /// Note the branch order: `== 4` is checked *before* `< 9`, so roll 3 falls through into the
    /// WEAK bucket. WEAK is 5/16 ({3,5,6,7,8}), not 4/16.
    #[must_use]
    pub const fn from_roll(roll: i32) -> Self {
        match roll {
            0 => Self::Lazy,
            1 => Self::Worried,
            2 => Self::Playful,
            4 => Self::Aggressive,
            r if r < 9 => Self::Weak,
            r if r < 11 => Self::Brown,
            _ => Self::Normal,
        }
    }

    fn random() -> Self {
        Self::from_roll(rng().random_range(0..16))
    }
}

/// Represents a Panda, a rare passive mob with a two-gene personality system.
///
/// `Panda.Gene`: a main and a hidden gene whose combination via
/// [`PandaGene::variant_from_genes`] yields the expressed variant (lazy / worried / playful /
/// brown / weak / aggressive / normal). Nine of `Panda.java`'s sixteen goals gate on that
/// variant, so the gene system is a hard prerequisite for the goal ladder rather than cosmetic.
///
/// This carries `Panda.registerGoals`' full ladder (`Panda.java:264-282`), the `DATA_ID_FLAGS`
/// bitfield (sneeze/roll/sit/on-back), the three synced counters (unhappy/sneeze/eat), the roll
/// physics, the eat loop, and the bamboo pick-up/hold path.
///
/// Documented simplifications:
///
/// * `Panda.addEatingParticles`' item-break particles and `afterSneeze`'s sneeze particle are not
///   spawned; the corresponding *sounds* and all server-side effects are. Entities here have no
///   `ItemParticleOption` equivalent surfaced to them.
/// * The `sitAmount`/`onBackAmount`/`rollAmount` interpolation floats are client-render-only
///   (`Panda.getSitAmount` and friends have no server-side caller) and are omitted.
/// * `dropFromGiftLootTable(BuiltInLootTables.PANDA_SNEEZE)` is inlined as its datapack contents
///   (`assets/datapacks/26_2/data/minecraft/loot_table/gameplay/panda_sneeze.json`: a slime ball
///   at weight 1 against an empty entry at weight 699, so 1/700); the `minecraft:gift` loot-table
///   type is not wired to entities in this codebase.
///
/// Wiki: <https://minecraft.wiki/w/Panda>
pub struct PandaEntity {
    pub mob_entity: MobEntity,
    pub ageable_data: AgeableData,
    /// `Panda.DATA_ID_FLAGS`.
    flags: AtomicU8,
    /// `Panda.MAIN_GENE_ID`.
    main_gene: AtomicU8,
    /// `Panda.HIDDEN_GENE_ID`.
    hidden_gene: AtomicU8,
    /// `Panda.UNHAPPY_COUNTER`.
    unhappy_counter: AtomicI32,
    /// `Panda.SNEEZE_COUNTER`.
    sneeze_counter: AtomicI32,
    /// `Panda.EAT_COUNTER`.
    eat_counter: AtomicI32,
    /// `Panda.rollCounter`.
    roll_counter: AtomicI32,
    /// Whether the main-hand slot currently holds something. `entity_equipment` is behind an
    /// async mutex, but `Mob::wants_to_pick_up_item`/`on_item_pickup` are synchronous hooks, so
    /// the emptiness they need is mirrored here and written by `set_held_stack`. Without it a
    /// panda that already holds bamboo would still claim a dropped stack, delete the `ItemEntity`
    /// and then discard the items.
    hand_occupied: AtomicBool,
    /// `Panda.rollDelta`.
    roll_delta: AtomicCell<Vector3<f64>>,
    /// `Panda.gotBamboo`: set when a player feeds a panda that currently has a target, so
    /// `PandaHurtByTargetGoal` drops the grudge.
    got_bamboo: AtomicBool,
    /// `Panda.didBite`: same, for a non-aggressive panda that has already retaliated once.
    did_bite: AtomicBool,
    /// `Panda.PandaLookAtPlayerGoal.setTarget`: `PandaBreedGoal` points the look goal at the
    /// nearest player when it fails to find bamboo. The look goal owns no state shared with the
    /// breed goal here, so the request is parked on the entity and consumed by the look goal.
    forced_look_target: AtomicCell<Option<Uuid>>,
}

impl PandaEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let mob_entity = MobEntity::new(entity);
        let panda = Self {
            mob_entity,
            ageable_data: AgeableData::default(),
            flags: AtomicU8::new(0),
            main_gene: AtomicU8::new(GENE_UNSET),
            hidden_gene: AtomicU8::new(GENE_UNSET),
            unhappy_counter: AtomicI32::new(0),
            sneeze_counter: AtomicI32::new(0),
            eat_counter: AtomicI32::new(0),
            roll_counter: AtomicI32::new(0),
            hand_occupied: AtomicBool::new(false),
            roll_delta: AtomicCell::new(Vector3::new(0.0, 0.0, 0.0)),
            got_bamboo: AtomicBool::new(false),
            did_bite: AtomicBool::new(false),
            forced_look_target: AtomicCell::new(None),
        };
        let mob_arc = Arc::new(panda);
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

            // `Panda.registerGoals` (`Panda.java:264-282`), priorities kept 1:1.
            goal_selector.add_goal(0, Box::new(SwimGoal::default()));
            goal_selector.add_goal(2, PandaPanicGoal::new(2.0));
            goal_selector.add_goal(2, PandaBreedGoal::new(1.0));
            goal_selector.add_goal(3, PandaAttackGoal::new(1.2, true));
            goal_selector.add_goal(4, Box::new(TemptGoal::new(1.0, TEMPT_ITEMS, false)));
            goal_selector.add_goal(6, PandaAvoidGoal::from_player(8.0, 2.0, 2.0));
            goal_selector.add_goal(6, PandaAvoidGoal::from_monsters(4.0, 2.0, 2.0));
            goal_selector.add_goal(7, PandaSitGoal::new());
            goal_selector.add_goal(8, PandaLieOnBackGoal::new());
            goal_selector.add_goal(8, PandaSneezeGoal::new());
            goal_selector.add_goal(9, PandaLookAtPlayerGoal::new(mob_weak, 6.0));
            goal_selector.add_goal(10, Box::new(RandomLookAroundGoal::default()));
            goal_selector.add_goal(12, PandaRollGoal::new());
            goal_selector.add_goal(13, Box::new(FollowParentGoal::new(1.25)));
            // `WaterAvoidingRandomStrollGoal(this, 1.0)`.
            goal_selector.add_goal(14, Box::new(WanderAroundGoal::new_water_avoiding(1.0)));

            let mut target_selector = mob_arc
                .mob_entity
                .target_selector
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            target_selector.add_goal(1, PandaHurtByTargetGoal::new());
        };

        // `Panda.<init>`: an adult panda picks bamboo and cake up off the ground.
        if mob_arc.mob_entity.living_entity.entity.age.load(Relaxed) >= 0 {
            mob_arc.mob_entity.set_can_pick_up_loot(true);
        }

        mob_arc
    }

    fn flag(&self, mask: u8) -> bool {
        self.flags.load(Relaxed) & mask != 0
    }

    /// `Panda.setFlag`, resyncing `DATA_ID_FLAGS`.
    fn set_flag(&self, mask: u8, value: bool) {
        let old = if value {
            self.flags.fetch_or(mask, Relaxed)
        } else {
            self.flags.fetch_and(!mask, Relaxed)
        };
        let byte = if value { old | mask } else { old & !mask };
        self.mob_entity.living_entity.entity.send_meta_data(
            &[Metadata::new(
                pumpkin_data::tracked_data::panda::DATA_ID_FLAGS,
                byte as i8,
            )],
            None,
        );
    }

    #[must_use]
    pub fn is_sneezing(&self) -> bool {
        self.flag(FLAG_SNEEZE)
    }

    /// `Panda.sneeze`.
    pub fn sneeze(&self, value: bool) {
        self.set_flag(FLAG_SNEEZE, value);
        if !value {
            self.set_sneeze_counter(0);
        }
    }

    #[must_use]
    pub fn is_sitting_panda(&self) -> bool {
        self.flag(FLAG_SIT)
    }

    /// `Panda.sit`.
    pub fn sit(&self, value: bool) {
        self.set_flag(FLAG_SIT, value);
    }

    #[must_use]
    pub fn is_on_back(&self) -> bool {
        self.flag(FLAG_ON_BACK)
    }

    /// `Panda.setOnBack`.
    pub fn set_on_back(&self, value: bool) {
        self.set_flag(FLAG_ON_BACK, value);
    }

    #[must_use]
    pub fn is_rolling(&self) -> bool {
        self.flag(FLAG_ROLL)
    }

    /// `Panda.roll`.
    pub fn roll(&self, value: bool) {
        self.set_flag(FLAG_ROLL, value);
    }

    #[must_use]
    pub fn get_unhappy_counter(&self) -> i32 {
        self.unhappy_counter.load(Relaxed)
    }

    /// `Panda.setUnhappyCounter`.
    pub fn set_unhappy_counter(&self, value: i32) {
        self.unhappy_counter.store(value, Relaxed);
        self.mob_entity.living_entity.entity.send_meta_data(
            &[Metadata::new(
                pumpkin_data::tracked_data::panda::UNHAPPY_COUNTER,
                VarInt(value),
            )],
            None,
        );
    }

    #[must_use]
    pub fn get_sneeze_counter(&self) -> i32 {
        self.sneeze_counter.load(Relaxed)
    }

    fn set_sneeze_counter(&self, value: i32) {
        self.sneeze_counter.store(value, Relaxed);
        self.mob_entity.living_entity.entity.send_meta_data(
            &[Metadata::new(
                pumpkin_data::tracked_data::panda::SNEEZE_COUNTER,
                VarInt(value),
            )],
            None,
        );
    }

    #[must_use]
    pub fn is_eating(&self) -> bool {
        self.eat_counter.load(Relaxed) > 0
    }

    /// `Panda.eat`.
    pub fn eat(&self, value: bool) {
        self.set_eat_counter(i32::from(value));
    }

    fn set_eat_counter(&self, value: i32) {
        self.eat_counter.store(value, Relaxed);
        self.mob_entity.living_entity.entity.send_meta_data(
            &[Metadata::new(
                pumpkin_data::tracked_data::panda::EAT_COUNTER,
                VarInt(value),
            )],
            None,
        );
    }

    #[must_use]
    pub fn main_gene(&self) -> PandaGene {
        PandaGene::from_id(self.main_gene.load(Relaxed))
    }

    /// `Panda.setMainGene`.
    pub fn set_main_gene(&self, gene: PandaGene) {
        self.main_gene.store(gene.id(), Relaxed);
        self.mob_entity.living_entity.entity.send_meta_data(
            &[Metadata::new(
                pumpkin_data::tracked_data::panda::MAIN_GENE_ID,
                gene.id() as i8,
            )],
            None,
        );
    }

    #[must_use]
    pub fn hidden_gene(&self) -> PandaGene {
        PandaGene::from_id(self.hidden_gene.load(Relaxed))
    }

    /// `Panda.setHiddenGene`.
    pub fn set_hidden_gene(&self, gene: PandaGene) {
        self.hidden_gene.store(gene.id(), Relaxed);
        self.mob_entity.living_entity.entity.send_meta_data(
            &[Metadata::new(
                pumpkin_data::tracked_data::panda::HIDDEN_GENE_ID,
                gene.id() as i8,
            )],
            None,
        );
    }

    /// `Panda.getVariant`.
    #[must_use]
    pub fn variant(&self) -> PandaGene {
        PandaGene::variant_from_genes(self.main_gene(), self.hidden_gene())
    }

    #[must_use]
    pub fn is_lazy(&self) -> bool {
        self.variant() == PandaGene::Lazy
    }

    #[must_use]
    pub fn is_worried(&self) -> bool {
        self.variant() == PandaGene::Worried
    }

    #[must_use]
    pub fn is_playful(&self) -> bool {
        self.variant() == PandaGene::Playful
    }

    #[must_use]
    pub fn is_brown(&self) -> bool {
        self.variant() == PandaGene::Brown
    }

    #[must_use]
    pub fn is_weak(&self) -> bool {
        self.variant() == PandaGene::Weak
    }

    /// `Panda.isAggressive` override -- the expressed gene, not the generic `Mob.isAggressive`
    /// attacking flag.
    #[must_use]
    pub fn is_aggressive_gene(&self) -> bool {
        self.variant() == PandaGene::Aggressive
    }

    /// `Panda.isScared`: a worried panda cowers during a thunderstorm.
    ///
    /// Takes the weather state as an argument because `World::is_thundering` is async and every
    /// caller already has it to hand.
    #[must_use]
    pub fn is_scared_with(&self, thundering: bool) -> bool {
        self.is_worried() && thundering
    }

    /// `Panda.canPerformAction` (`Panda.java:684-686`): the gate nearly every panda-specific goal
    /// shares. The `isScared` half needs the weather, so it is passed in.
    #[must_use]
    pub fn can_perform_action_with(&self, thundering: bool) -> bool {
        !self.is_on_back()
            && !self.is_scared_with(thundering)
            && !self.is_eating()
            && !self.is_rolling()
            && !self.is_sitting_panda()
    }

    /// `Panda.canPerformAction`, resolving the weather itself. Goals call this one.
    pub async fn can_perform_action(&self) -> bool {
        let thundering = self
            .mob_entity
            .living_entity
            .entity
            .world
            .load()
            .is_thundering()
            .await;
        self.can_perform_action_with(thundering)
    }

    #[must_use]
    pub fn got_bamboo(&self) -> bool {
        self.got_bamboo.load(Relaxed)
    }

    #[must_use]
    pub fn did_bite(&self) -> bool {
        self.did_bite.load(Relaxed)
    }

    /// `Panda.doHurtTarget`: a non-aggressive panda that lands a bite stops holding the grudge.
    pub fn set_did_bite(&self, value: bool) {
        self.did_bite.store(value, Relaxed);
    }

    /// `Panda.PandaLookAtPlayerGoal.setTarget`, parked on the entity (see the field doc).
    pub fn set_forced_look_target(&self, uuid: Option<Uuid>) {
        self.forced_look_target.store(uuid);
    }

    #[must_use]
    pub fn take_forced_look_target(&self) -> Option<Uuid> {
        self.forced_look_target.swap(None)
    }

    /// `Panda.tryToSit`.
    pub fn try_to_sit(&self) {
        if !self.mob_entity.living_entity.is_in_water() {
            self.mob_entity
                .navigator
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .stop();
            self.sit(true);
        }
    }

    /// `Panda.getItemBySlot(MAINHAND)`.
    pub async fn held_stack(&self) -> ItemStack {
        self.mob_entity
            .living_entity
            .entity_equipment
            .lock()
            .await
            .get(&EquipmentSlot::MAIN_HAND)
    }

    /// `Panda.setItemSlot(MAINHAND, ...)` plus the equipment resync the client needs to render
    /// the held bamboo.
    pub async fn set_held_stack(&self, stack: ItemStack) {
        self.hand_occupied.store(!stack.is_empty(), Relaxed);
        self.mob_entity
            .living_entity
            .entity_equipment
            .lock()
            .await
            .put(&EquipmentSlot::MAIN_HAND, stack.clone());
        self.mob_entity
            .living_entity
            .send_equipment_changes(&[(EquipmentSlot::MAIN_HAND, stack)]);
    }

    /// `Panda.canPickUpAndEat`'s item half: `ItemTags.PANDA_EATS_FROM_GROUND` (bamboo and cake).
    #[must_use]
    pub fn can_pick_up_and_eat(stack: &ItemStack) -> bool {
        stack
            .item
            .has_tag(&tag::Item::MINECRAFT_PANDA_EATS_FROM_GROUND)
    }

    /// `Panda.setAttributes` (`Panda.java:594-602`): a weak panda has 10 max health, a lazy one
    /// moves at 0.07 instead of the species default 0.15.
    pub fn set_attributes(&self) {
        let living = &self.mob_entity.living_entity;
        if self.is_weak() {
            living.set_attribute_base(&Attributes::MAX_HEALTH, 10.0);
        }
        if self.is_lazy() {
            living.set_attribute_base(&Attributes::MOVEMENT_SPEED, 0.07);
        }
    }

    /// `Panda.setGeneFromParents` (`Panda.java:562-587`), including the two independent 1/32
    /// mutation re-rolls.
    pub fn set_gene_from_parents(&self, parent1: &Self, parent2: Option<&Self>) {
        let mut rng = rng();
        match parent2 {
            None => {
                if rng.random_bool(0.5) {
                    self.set_main_gene(parent1.one_of_genes_randomly());
                    self.set_hidden_gene(PandaGene::random());
                } else {
                    self.set_main_gene(PandaGene::random());
                    self.set_hidden_gene(parent1.one_of_genes_randomly());
                }
            }
            Some(parent2) => {
                if rng.random_bool(0.5) {
                    self.set_main_gene(parent1.one_of_genes_randomly());
                    self.set_hidden_gene(parent2.one_of_genes_randomly());
                } else {
                    self.set_main_gene(parent2.one_of_genes_randomly());
                    self.set_hidden_gene(parent1.one_of_genes_randomly());
                }
            }
        }

        if rng.random_range(0..32) == 0 {
            self.set_main_gene(PandaGene::random());
        }
        if rng.random_range(0..32) == 0 {
            self.set_hidden_gene(PandaGene::random());
        }
    }

    /// `Panda.getOneOfGenesRandomly`.
    fn one_of_genes_randomly(&self) -> PandaGene {
        if rng().random_bool(0.5) {
            self.main_gene()
        } else {
            self.hidden_gene()
        }
    }

    /// `Panda.handleRoll` (`Panda.java:483-503`).
    fn handle_roll(&self) {
        let entity = &self.mob_entity.living_entity.entity;
        let counter = self.roll_counter.load(Relaxed) + 1;
        self.roll_counter.store(counter, Relaxed);
        if counter > TOTAL_ROLL_STEPS {
            self.roll(false);
            return;
        }

        let movement = entity.velocity.load();
        if counter == 1 {
            let angle = f64::from(entity.yaw.load()).to_radians();
            let multiplier = if self.is_baby() { 0.1 } else { 0.2 };
            let delta = Vector3::new(
                movement.x + -angle.sin() * multiplier,
                0.0,
                movement.z + angle.cos() * multiplier,
            );
            self.roll_delta.store(delta);
            entity.set_velocity(Vector3::new(delta.x, delta.y + 0.27, delta.z));
        } else if counter == 7 || counter == 15 || counter == 23 {
            let y = if entity.on_ground.load(Relaxed) {
                0.27
            } else {
                movement.y
            };
            entity.set_velocity(Vector3::new(0.0, y, 0.0));
        } else {
            let delta = self.roll_delta.load();
            entity.set_velocity(Vector3::new(delta.x, movement.y, delta.z));
        }
    }

    /// `Panda.handleEating` (`Panda.java:424-450`) plus the sound half of
    /// `Panda.addEatingParticles`.
    async fn handle_eating(&self, thundering: bool) {
        let held = self.held_stack().await;
        let held_empty = held.is_empty();

        if !self.is_eating()
            && self.is_sitting_panda()
            && !self.is_scared_with(thundering)
            && !held_empty
            && rng().random_range(0..80) == 1
        {
            self.eat(true);
        } else if held_empty || !self.is_sitting_panda() {
            self.eat(false);
        }

        if !self.is_eating() {
            return;
        }

        let counter = self.eat_counter.load(Relaxed);
        if counter % 5 == 0 {
            let entity = &self.mob_entity.living_entity.entity;
            entity.world.load().play_sound(
                Sound::EntityPandaEat,
                SoundCategory::Neutral,
                &entity.pos.load(),
            );
        }

        if counter > 80 && rng().random_range(0..20) == 1 {
            if counter > 100 && Self::can_pick_up_and_eat(&held) {
                self.set_held_stack(ItemStack::EMPTY.clone()).await;
                self.sit(false);
            }
            self.eat(false);
            return;
        }

        self.set_eat_counter(counter + 1);
    }

    /// `Panda.afterSneeze` (`Panda.java:505-528`): startles nearby adult pandas into a jump and
    /// rolls the sneeze gift loot table.
    async fn after_sneeze(&self) {
        let entity = &self.mob_entity.living_entity.entity;
        let world = entity.world.load();
        let pos = entity.pos.load();
        world.play_sound(Sound::EntityPandaSneeze, SoundCategory::Neutral, &pos);

        let thundering = world.is_thundering().await;
        let area = entity.bounding_box.load().expand_all(10.0);
        for candidate in world.get_entities_at_box(&area) {
            let Some(other) = candidate.cast_any().downcast_ref::<Self>() else {
                continue;
            };
            let other_entity = &other.mob_entity.living_entity.entity;
            if other.is_baby()
                || !other_entity.on_ground.load(Relaxed)
                || other.mob_entity.living_entity.is_in_water()
                || !other.can_perform_action_with(thundering)
            {
                continue;
            }
            // `Mob.jumpFromGround` is driven through the `JumpControl` phase here.
            other.mob_entity.jump_requested.store(true, Relaxed);
        }

        // Inlined `gameplay/panda_sneeze` gift loot table (see the type doc): slime ball at
        // weight 1 against an empty entry at weight 699.
        if world.level_info.load().game_rules.mob_drops && rng().random_range(0..700) == 0 {
            world
                .drop_stack(
                    &entity.block_pos.load(),
                    ItemStack::new(1, &Item::SLIME_BALL),
                )
                .await;
        }
    }
}

impl AgeableMob for PandaEntity {
    fn get_ageable_data(&self) -> &AgeableData {
        &self.ageable_data
    }

    /// `Panda.BABY_DIMENSIONS`: the adult box (1.3 x 1.25) scaled by 0.5, with a 0.28125 eye
    /// height.
    fn baby_dimensions(&self) -> Option<EntityDimensions> {
        Some(EntityDimensions::new(0.65, 0.625, 0.28125))
    }
}

impl Animal for PandaEntity {
    /// `Panda.isFood`.
    fn is_food(&self, item_stack: &ItemStack) -> bool {
        item_stack.item.has_tag(&tag::Item::MINECRAFT_PANDA_FOOD)
    }
}

impl NBTStorage for PandaEntity {
    fn write_nbt<'a>(&'a self, nbt: &'a mut NbtCompound) -> NbtFuture<'a, ()> {
        Box::pin(async move {
            self.mob_entity.living_entity.write_nbt(nbt).await;
            self.write_ageable_nbt(nbt);
            self.write_animal_nbt(nbt);
            // `Panda.addAdditionalSaveData`.
            nbt.put_string("MainGene", self.main_gene().name().to_string());
            nbt.put_string("HiddenGene", self.hidden_gene().name().to_string());
        })
    }

    fn read_nbt_non_mut<'a>(&'a self, nbt: &'a NbtCompound) -> NbtFuture<'a, ()> {
        Box::pin(async move {
            self.mob_entity.living_entity.read_nbt_non_mut(nbt).await;
            self.read_ageable_nbt(nbt);
            self.read_animal_nbt(nbt);
            // `Panda.readAdditionalSaveData`: an absent or unrecognised name is NORMAL.
            let main = nbt
                .get_string("MainGene")
                .map_or(PandaGene::Normal, PandaGene::from_name);
            let hidden = nbt
                .get_string("HiddenGene")
                .map_or(PandaGene::Normal, PandaGene::from_name);
            self.set_main_gene(main);
            self.set_hidden_gene(hidden);
        })
    }
}

impl Mob for PandaEntity {
    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    /// `Panda.doHurtTarget` (`Panda.java:365-372`): a panda that is not AGGRESSIVE-gened records
    /// that it has bitten, which makes `PandaHurtByTargetGoal` drop the grudge. This runs on a
    /// landed hit rather than on the swing, which is what vanilla keys off.
    fn on_successful_attack<'a>(&'a self, _target: &'a dyn EntityBase) -> EntityBaseFuture<'a, ()> {
        Box::pin(async move {
            if !self.is_aggressive_gene() {
                self.set_did_bite(true);
            }
            let entity = &self.mob_entity.living_entity.entity;
            // `Panda.playAttackSound`.
            entity.world.load().play_sound(
                Sound::EntityPandaBite,
                SoundCategory::Neutral,
                &entity.pos.load(),
            );
        })
    }

    /// `Panda.canBeLeashed`.
    fn can_be_leashed(&self) -> bool {
        false
    }

    /// `Panda.getAmbientSound`.
    fn get_ambient_sound(&self) -> Option<Sound> {
        Some(if self.is_aggressive_gene() {
            Sound::EntityPandaAggressiveAmbient
        } else if self.is_worried() {
            Sound::EntityPandaWorriedAmbient
        } else {
            Sound::EntityPandaAmbient
        })
    }

    /// `Panda.pickUpItem`'s two gates: bamboo or cake (`canPickUpAndEat`), and an empty main
    /// hand.
    fn wants_to_pick_up_item(&self, _world: &World, stack: &ItemStack) -> bool {
        !self.hand_occupied.load(Relaxed) && Self::can_pick_up_and_eat(stack)
    }

    /// `Panda.pickUpItem`: the whole stack goes into the main hand, so the returned count is the
    /// full stack size and the caller then removes the emptied `ItemEntity`.
    fn on_item_pickup(&self, stack: &ItemStack) -> u8 {
        if !Self::can_pick_up_and_eat(stack) {
            return 0;
        }
        // Claim the hand synchronously, before returning a nonzero count: the caller destroys the
        // `ItemEntity` on that count, so a second stack must not also be claimed while the
        // equipment write below is still queued.
        if self.hand_occupied.swap(true, Relaxed) {
            return 0;
        }
        let count = stack.item_count;
        let stack = stack.clone();
        // The equipment write is async and this hook is not, so it is spawned. The count is
        // decided synchronously from the same stack the caller shrinks, so the two cannot
        // disagree even if the spawned write lands a tick later.
        let e = &self.mob_entity.living_entity.entity;
        let world = e.world.load();
        let Some(entity) = world.get_entity_by_uuid(e.entity_uuid) else {
            // Release the claim taken above: nothing will write the equipment slot.
            self.hand_occupied.store(false, Relaxed);
            return 0;
        };
        tokio::spawn(async move {
            if let Some(panda) = entity.cast_any().downcast_ref::<Self>() {
                panda.set_held_stack(stack).await;
            }
        });
        count
    }

    fn mob_init_data_tracker(&self) -> EntityBaseFuture<'_, ()> {
        Box::pin(async move {
            // `Panda.finalizeSpawn`: both genes rolled independently at spawn, then
            // `setAttributes`. An NBT restore has already written real ids, so the sentinel check
            // keeps a loaded panda's genes instead of re-rolling them.
            if self.main_gene.load(Relaxed) == GENE_UNSET {
                self.set_main_gene(PandaGene::random());
            } else {
                self.set_main_gene(self.main_gene());
            }
            if self.hidden_gene.load(Relaxed) == GENE_UNSET {
                self.set_hidden_gene(PandaGene::random());
            } else {
                self.set_hidden_gene(self.hidden_gene());
            }
            self.set_attributes();
            // `entity_equipment` is restored by `LivingEntity::read_nbt`, which does not go
            // through `set_held_stack`, so the mirror has to be re-derived once here.
            self.hand_occupied
                .store(!self.held_stack().await.is_empty(), Relaxed);

            let entity = &self.mob_entity.living_entity.entity;
            if entity.age.load(Relaxed) < 0 {
                entity.send_meta_data(
                    &[Metadata::new(
                        pumpkin_data::tracked_data::panda::DATA_BABY_ID,
                        true,
                    )],
                    None,
                );
            }
            entity.send_meta_data(
                &[Metadata::new(
                    pumpkin_data::tracked_data::panda::DATA_ID_FLAGS,
                    self.flags.load(Relaxed) as i8,
                )],
                None,
            );
        })
    }

    /// `Panda.tick` (`Panda.java:379-422`).
    fn mob_tick<'a>(&'a self, _caller: &'a Arc<dyn EntityBase>) -> EntityBaseFuture<'a, ()> {
        Box::pin(async move {
            let entity = &self.mob_entity.living_entity.entity;
            let world = entity.world.load();
            let thundering = world.is_thundering().await;

            if self.is_worried() {
                if thundering && !self.mob_entity.living_entity.is_in_water() {
                    self.sit(true);
                    self.eat(false);
                } else if !self.is_eating() {
                    self.sit(false);
                }
            }

            let target = self.mob_entity.target.lock().await.clone();
            if target.is_none() {
                self.got_bamboo.store(false, Relaxed);
                self.did_bite.store(false, Relaxed);
            }

            let unhappy = self.get_unhappy_counter();
            if unhappy > 0 {
                if let Some(target) = &target {
                    entity.look_at(target.get_entity().pos.load());
                }
                if unhappy == 29 || unhappy == 14 {
                    world.play_sound(
                        Sound::EntityPandaCantBreed,
                        SoundCategory::Neutral,
                        &entity.pos.load(),
                    );
                }
                self.set_unhappy_counter(unhappy - 1);
            }

            if self.is_sneezing() {
                let counter = self.get_sneeze_counter() + 1;
                self.set_sneeze_counter(counter);
                if counter > 20 {
                    self.sneeze(false);
                    self.after_sneeze().await;
                } else if counter == 1 {
                    world.play_sound(
                        Sound::EntityPandaPreSneeze,
                        SoundCategory::Neutral,
                        &entity.pos.load(),
                    );
                }
            }

            if self.is_rolling() {
                self.handle_roll();
            } else {
                self.roll_counter.store(0, Relaxed);
            }

            if self.is_sitting_panda() {
                // `this.setXRot(0.0F)`.
                entity.set_rotation(entity.yaw.load(), 0.0);
            }

            self.handle_eating(thundering).await;
        })
    }

    /// `Panda.mobInteract` (`Panda.java:611-663`).
    fn mob_interact<'a>(
        &'a self,
        player: &'a Arc<Player>,
        item_stack: &'a mut ItemStack,
    ) -> EntityBaseFuture<'a, bool> {
        Box::pin(async move {
            let entity = &self.mob_entity.living_entity.entity;
            let world = entity.world.load();
            if self.is_scared_with(world.is_thundering().await) {
                return false;
            }

            if self.is_on_back() {
                self.set_on_back(false);
                return true;
            }

            if !self.is_food(item_stack) {
                return false;
            }

            // `if (this.getTarget() != null) this.gotBamboo = true;`
            if self.mob_entity.target.lock().await.is_some() {
                self.got_bamboo.store(true, Relaxed);
            }

            // Growing a cub up and entering love mode both live in `animal_interact`, which
            // returns true when it consumed the item for either.
            if self
                .animal_interact(player, item_stack, Sound::EntityPandaAmbient)
                .await
            {
                return true;
            }

            // Vanilla's remaining branch: an adult that can neither age up nor fall in love sits
            // down and starts eating the offered bamboo, dropping whatever it already held.
            if self.is_baby()
                || self.is_sitting_panda()
                || self.mob_entity.living_entity.is_in_water()
            {
                return false;
            }

            self.try_to_sit();
            self.eat(true);
            let current = self.held_stack().await;
            // `!player.hasInfiniteMaterials()`: a creative player's feeding does not spit the old
            // stack back onto the ground.
            if !current.is_empty() && player.gamemode.load() != pumpkin_util::GameMode::Creative {
                world.drop_stack(&entity.block_pos.load(), current).await;
            }
            self.set_held_stack(ItemStack::new(1, item_stack.item))
                .await;
            item_stack.decrement_unless_creative(player.gamemode.load(), 1);
            true
        })
    }

    /// `Panda.getBreedOffspring`: the cub's genes come from both parents via
    /// `setGeneFromParents`, then `setAttributes` applies the weak/lazy stat overrides.
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
                Uuid::new_v4(),
            );

            if let Some(cub) = baby.cast_any().downcast_ref::<Self>() {
                let mate_panda = mate.cast_any().downcast_ref::<Self>();
                cub.set_gene_from_parents(self, mate_panda);
                cub.set_attributes();
            }

            Some(baby)
        })
    }
}

#[cfg(test)]
mod gene_tests {
    use super::PandaGene;

    /// `Panda.Gene.getRandom`'s exact bucket layout (`Panda.java:774-789`). The `== 4` branch runs
    /// before the `< 9` branch, so roll 3 is WEAK rather than part of a contiguous 5..9 run.
    #[test]
    fn get_random_bucket_layout() {
        let expected = [
            PandaGene::Lazy,       // 0
            PandaGene::Worried,    // 1
            PandaGene::Playful,    // 2
            PandaGene::Weak,       // 3 -- falls past `== 4` into `< 9`
            PandaGene::Aggressive, // 4
            PandaGene::Weak,       // 5
            PandaGene::Weak,       // 6
            PandaGene::Weak,       // 7
            PandaGene::Weak,       // 8
            PandaGene::Brown,      // 9
            PandaGene::Brown,      // 10
            PandaGene::Normal,     // 11
            PandaGene::Normal,     // 12
            PandaGene::Normal,     // 13
            PandaGene::Normal,     // 14
            PandaGene::Normal,     // 15
        ];
        for (roll, want) in expected.into_iter().enumerate() {
            assert_eq!(
                PandaGene::from_roll(roll as i32),
                want,
                "roll {roll} mapped wrong"
            );
        }
    }

    #[test]
    fn weak_is_five_sixteenths_and_brown_two() {
        let weak = (0..16)
            .filter(|r| PandaGene::from_roll(*r) == PandaGene::Weak)
            .count();
        let brown = (0..16)
            .filter(|r| PandaGene::from_roll(*r) == PandaGene::Brown)
            .count();
        assert_eq!(weak, 5);
        assert_eq!(brown, 2);
    }

    #[test]
    fn only_brown_and_weak_are_recessive() {
        for gene in [
            PandaGene::Normal,
            PandaGene::Lazy,
            PandaGene::Worried,
            PandaGene::Playful,
            PandaGene::Aggressive,
        ] {
            assert!(!gene.is_recessive(), "{gene:?} should be dominant");
        }
        assert!(PandaGene::Brown.is_recessive());
        assert!(PandaGene::Weak.is_recessive());
    }

    #[test]
    fn recessive_main_gene_needs_a_matching_hidden_gene() {
        assert_eq!(
            PandaGene::variant_from_genes(PandaGene::Brown, PandaGene::Brown),
            PandaGene::Brown
        );
        assert_eq!(
            PandaGene::variant_from_genes(PandaGene::Brown, PandaGene::Lazy),
            PandaGene::Normal
        );
        assert_eq!(
            PandaGene::variant_from_genes(PandaGene::Weak, PandaGene::Brown),
            PandaGene::Normal
        );
    }

    #[test]
    fn dominant_main_gene_always_expresses() {
        assert_eq!(
            PandaGene::variant_from_genes(PandaGene::Lazy, PandaGene::Brown),
            PandaGene::Lazy
        );
        assert_eq!(
            PandaGene::variant_from_genes(PandaGene::Aggressive, PandaGene::Weak),
            PandaGene::Aggressive
        );
    }

    #[test]
    fn id_and_name_round_trip_and_bad_input_falls_back_to_normal() {
        for gene in [
            PandaGene::Normal,
            PandaGene::Lazy,
            PandaGene::Worried,
            PandaGene::Playful,
            PandaGene::Brown,
            PandaGene::Weak,
            PandaGene::Aggressive,
        ] {
            assert_eq!(PandaGene::from_id(gene.id()), gene);
            assert_eq!(PandaGene::from_name(gene.name()), gene);
        }
        assert_eq!(PandaGene::from_id(7), PandaGene::Normal);
        assert_eq!(PandaGene::from_id(255), PandaGene::Normal);
        assert_eq!(PandaGene::from_name("nonsense"), PandaGene::Normal);
    }
}
