// Legacy invariant checks retained for vanilla behavior; migrate these paths before removing this allow.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use super::{
    Entity, EntityBase, NBTStorage,
    ai::pathfinder::{NavigationKind, Navigator, NavigatorGoal},
    living::LivingEntity,
};
use crate::entity::EntityBaseFuture;
use crate::entity::ai::brain::Brain;
use crate::entity::ai::control::MoveControlTrait;
use crate::entity::ai::control::look_control::LookControl;
use crate::entity::ai::control::move_control::MoveControl;
use crate::entity::ai::goal::Controls;
use crate::entity::ai::goal::goal_selector::GoalSelector;
use crate::entity::passive::wolf::WolfEntity;
use crate::entity::player::Player;
use crate::entity::r#type::from_type;
use crate::item::items::spawn_egg::apply_entity_variant;
use crate::server::Server;
use crate::world::World;
use crate::world::game_event::{GameEventContext, emit_game_event};
use crossbeam::atomic::AtomicCell;
use pumpkin_data::attributes::Attributes;
use pumpkin_data::damage::DamageType;
use pumpkin_data::data_component_impl::{EquipmentSlot, EquippableImpl, IDSet, WeaponImpl};
use pumpkin_data::effect::StatusEffect;
use pumpkin_data::enchantment::Enchantment;
use pumpkin_data::entity::entity_from_egg;
use pumpkin_data::entity::{EntityType, MobCategory};
use pumpkin_data::item_stack::{DamageResult, ItemStack};
use pumpkin_data::potion::Effect;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_data::tag::{self, Taggable};
use pumpkin_data::tracked_data;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_protocol::java::client::play::{CHeadRot, CUpdateEntityRot, Metadata};
use pumpkin_util::Difficulty;
use pumpkin_util::math::boundingbox::BoundingBox;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_util::random::xoroshiro128::Xoroshiro;
use pumpkin_util::random::{RandomGenerator, get_seed};
use pumpkin_util::version::JavaMinecraftVersion;
use rand::RngExt;
use std::collections::HashSet;
use std::ops::{Deref, DerefMut};
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::Ordering::Relaxed;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU8, Ordering};
use uuid::Uuid;

/// Resolves generic `minecraft:post_attack` mob effects against the landed melee target.
/// Today this is Bane of Arthropods' Slowness payload; the effect list is captured from the
/// attacker's held item before damage is attempted, matching vanilla's weapon snapshot.
async fn apply_post_hit_mob_effects(
    target: &dyn EntityBase,
    post_hit_effects: Vec<(crate::enchantment::EnchantmentEffect, i32)>,
) {
    let Some(living) = target.get_living_entity() else {
        return;
    };

    for (effect, level) in post_hit_effects {
        let crate::enchantment::EnchantmentEffect::ApplyMobEffectOnHit {
            min_duration_seconds,
            max_duration_seconds,
            min_amplifier,
            max_amplifier,
            ..
        } = effect
        else {
            continue;
        };
        let (duration, amplifier) = crate::enchantment::apply_mob_effect_on_hit(
            min_duration_seconds,
            max_duration_seconds,
            min_amplifier,
            max_amplifier,
            level,
            rand::random::<f32>(),
            rand::random::<f32>(),
        );
        living
            .add_effect(Effect {
                effect_type: &StatusEffect::SLOWNESS,
                duration,
                amplifier,
                ambient: false,
                show_particles: true,
                show_icon: true,
                blend: false,
            })
            .await;
    }
}

pub mod bat;
pub mod blaze;
pub mod breeze;
pub mod cave_spider;
pub mod creaking;
pub mod creeper;
pub mod elder_guardian;
pub mod enderman;
pub mod endermite;
pub mod equipment;
pub mod evoker;
pub mod ghast;
pub mod giant;
pub mod guardian;
pub mod hoglin;
pub mod hoglin_gore;
pub mod illusioner;
pub mod magma_cube;
pub mod phantom;
pub mod piglin;
pub mod piglin_brute;
pub mod piglin_shared;
pub mod pillager;
pub mod ravager;
pub mod shulker;
pub mod silverfish;
pub mod skeleton;
pub mod slime;
pub mod spider;
pub mod sulfur_cube;
pub mod vex;
pub mod vindicator;
pub mod warden;
pub mod warden_anger;
pub mod witch;
pub mod zoglin;
pub mod zombie;
pub mod zombification;
pub mod zombified_piglin;

/// Vanilla `Mob.getAmbientSoundInterval` (Mob.java:274-276).
pub const DEFAULT_AMBIENT_SOUND_INTERVAL: i32 = 80;
/// Vanilla `AbstractGolem.getAmbientSoundInterval` (`AbstractGolem.java:29-31`).
const GOLEM_AMBIENT_SOUND_INTERVAL: i32 = 120;
/// Vanilla `Mob.ITEM_PICKUP_REACH` (`Mob.java:104-105`).
const DEFAULT_ITEM_PICKUP_REACH: (f64, f64, f64) = (1.0, 0.0, 1.0);

/// Vanilla `Animal.getAmbientSoundInterval` (`Animal.java:121-124`). The Java method is
/// inherited by every animal, while Pumpkin dispatches the sound cadence through `Mob`; keep
/// the inherited 120-tick value here for the concrete types implementing `passive::animal::Animal`.
const fn is_animal_entity(id: u16) -> bool {
    id == EntityType::ARMADILLO.id
        || id == EntityType::AXOLOTL.id
        || id == EntityType::BEE.id
        || id == EntityType::CAMEL.id
        || id == EntityType::CAT.id
        || id == EntityType::CHICKEN.id
        || id == EntityType::COW.id
        || id == EntityType::DONKEY.id
        || id == EntityType::FOX.id
        || id == EntityType::FROG.id
        || id == EntityType::GOAT.id
        || id == EntityType::HAPPY_GHAST.id
        || id == EntityType::HOGLIN.id
        || id == EntityType::HORSE.id
        || id == EntityType::LLAMA.id
        || id == EntityType::MOOSHROOM.id
        || id == EntityType::MULE.id
        || id == EntityType::NAUTILUS.id
        || id == EntityType::OCELOT.id
        || id == EntityType::PANDA.id
        || id == EntityType::PARROT.id
        || id == EntityType::PIG.id
        || id == EntityType::POLAR_BEAR.id
        || id == EntityType::RABBIT.id
        || id == EntityType::SHEEP.id
        || id == EntityType::SKELETON_HORSE.id
        || id == EntityType::SNIFFER.id
        || id == EntityType::STRIDER.id
        || id == EntityType::TRADER_LLAMA.id
        || id == EntityType::TURTLE.id
        || id == EntityType::WOLF.id
        || id == EntityType::ZOMBIE_HORSE.id
        || id == EntityType::ZOMBIE_NAUTILUS.id
}

pub struct MobEntity {
    pub living_entity: LivingEntity,
    /// Vanilla `Mob.sensing`; the per-tick visibility caches are cleared at the start of
    /// `serverAiStep` before selectors query them.
    pub sensing: std::sync::Mutex<Sensing>,
    /// Pending request consumed by the vanilla-equivalent `JumpControl` phase.
    pub jump_requested: AtomicBool,
    /// `Mob.brain` -- present only for mobs migrated to the Brain/Memory/Activity system
    /// (`crate::entity::ai::brain`). `None` for every Goal-driven mob, which is still the vast
    /// majority; vanilla likewise holds a `goalSelector` and a `brain` on the same `Mob` and
    /// lets each mob use whichever it needs. A Brain-having mob keeps its `goals_selector`, and
    /// the two are ticked independently and do not know about each other.
    pub brain: Option<Brain>,
    pub goals_selector: std::sync::Mutex<GoalSelector>,
    pub target_selector: std::sync::Mutex<GoalSelector>,
    pub navigator: std::sync::Mutex<Navigator>,
    strafe_navigation_kind: AtomicU8,
    pub target: tokio::sync::Mutex<Option<Arc<dyn EntityBase>>>,
    pub look_control: std::sync::Mutex<LookControl>,
    pub move_control: std::sync::Mutex<Box<dyn MoveControlTrait>>,
    pub position_target: AtomicCell<BlockPos>,
    pub position_target_range: AtomicI32,
    /// Whether the shared home restriction currently comes from a leash. Vanilla clears that
    /// restriction from `Mob.onLeashRemoved` (`Mob.java:1279-1284`).
    leash_home_active: AtomicBool,
    pub love_ticks: AtomicI32,
    pub breeding_cooldown: AtomicI32,
    /// Vanilla `AbstractSchoolingFish.leader` reference.
    ///
    /// The reference is kept on the mob rather than on the follow goal so that the
    /// follower state is shared by `FollowFlockLeaderGoal`, random swimming, and
    /// future spawn/lifecycle hooks.
    schooling_leader: std::sync::Mutex<Option<Arc<dyn EntityBase>>>,
    /// Vanilla `AbstractSchoolingFish.schoolSize`, including the leader itself.
    schooling_size: AtomicI32,
    /// Vanilla `Mob.noActionTime`, used by the random despawn check.
    pub no_action_time: AtomicI32,
    /// Vanilla `Entity.tickCount`, used by species-specific despawn rules.
    pub tick_count: AtomicI32,
    pub breeder: AtomicCell<Option<Uuid>>,
    pub owner: AtomicCell<Option<Uuid>>,
    pub ordered_to_sit: AtomicBool,
    mob_flags: AtomicU8,
    /// Vanilla `Mob.ambientSoundTime` (Mob.java:127): counted up in `Mob.baseTick`
    /// (Mob.java:283-292) and reset to `-getAmbientSoundInterval()` by
    /// `Mob.resetAmbientSoundTime` (Mob.java:301-303).
    pub ambient_sound_time: AtomicI32,
    last_sent_yaw: AtomicU8,
    last_sent_pitch: AtomicU8,
    last_sent_head_yaw: AtomicU8,
}

#[derive(Default)]
pub struct Sensing {
    seen: HashSet<i32>,
    unseen: HashSet<i32>,
}

impl Sensing {
    fn tick(&mut self) {
        self.seen.clear();
        self.unseen.clear();
    }
}

/// The integer calculation used by `Mob.getMaxFallDistance` (`Mob.java:835-845`).
const fn max_fall_distance_for_state(
    has_target: bool,
    health: f32,
    max_health: f32,
    difficulty_id: i32,
) -> i32 {
    if !has_target {
        return 3;
    }

    let mut sacrifice = (health - max_health * 0.33) as i32;
    sacrifice -= (3 - difficulty_id) * 4;
    if sacrifice < 0 {
        sacrifice = 0;
    }
    sacrifice + 3
}

/// The per-type overrides of `Mob.getMaxSpawnClusterSize` (`Mob.java:825-827`) used by the
/// natural-spawn cluster limit.
const fn max_spawn_cluster_size_for(entity_type_id: u16) -> i32 {
    if entity_type_id == EntityType::GHAST.id
        || entity_type_id == EntityType::HAPPY_GHAST.id
        || entity_type_id == EntityType::PILLAGER.id
    {
        1
    } else if entity_type_id == EntityType::WOLF.id
        || entity_type_id == EntityType::PUFFERFISH.id
        || entity_type_id == EntityType::COD.id
        || entity_type_id == EntityType::TROPICAL_FISH.id
    {
        8
    } else if entity_type_id == EntityType::SALMON.id {
        5
    } else if entity_type_id == EntityType::CAMEL.id
        || entity_type_id == EntityType::CAMEL_HUSK.id
        || entity_type_id == EntityType::DONKEY.id
        || entity_type_id == EntityType::HORSE.id
        || entity_type_id == EntityType::LLAMA.id
        || entity_type_id == EntityType::MULE.id
        || entity_type_id == EntityType::SKELETON_HORSE.id
        || entity_type_id == EntityType::TRADER_LLAMA.id
        || entity_type_id == EntityType::ZOMBIE_HORSE.id
    {
        6
    } else {
        4
    }
}

/// Tick boundaries (both inclusive) when monsters do not burn in sunlight (26.1).
///
/// Sourced from `data/minecraft/timeline/day.json` — `monsters_burn` keyframes:
/// `value=false` at tick 12542 (dusk), `value=true` at tick 23460 (dawn).
///
/// TODO: Replace with `EnvironmentAttributes::MONSTERS_BURN` lookup once the
/// `EnvironmentAttributeSystem` is implemented in `pumpkin-data`.
const NIGHT_START: i64 = 12542;
const NIGHT_END: i64 = 23459;

impl MobEntity {
    const AI_DISABLED_FLAG: u8 = 1;
    const LEFT_HANDED_FLAG: u8 = 2;
    const ATTACKING_FLAG: u8 = 4;
    const CAN_PICK_UP_LOOT_FLAG: u8 = 8;

    #[must_use]
    pub fn new(entity: Entity) -> Self {
        let mut navigator = Navigator::default();
        navigator.set_mob_dimensions(
            entity.entity_type.dimension[0],
            entity.entity_type.dimension[1],
        );
        let id = entity.entity_type.id;
        if id == pumpkin_data::entity::EntityType::AXOLOTL.id
            || id == pumpkin_data::entity::EntityType::TURTLE.id
            || id == pumpkin_data::entity::EntityType::DROWNED.id
        {
            navigator.set_amphibious(true);
        } else if id == pumpkin_data::entity::EntityType::FROG.id {
            navigator.set_amphibious(true);
            navigator.set_frog(true);
        } else if id == pumpkin_data::entity::EntityType::COD.id
            || id == pumpkin_data::entity::EntityType::DOLPHIN.id
            || id == pumpkin_data::entity::EntityType::ELDER_GUARDIAN.id
            || id == pumpkin_data::entity::EntityType::GLOW_SQUID.id
            || id == pumpkin_data::entity::EntityType::GUARDIAN.id
            || id == pumpkin_data::entity::EntityType::NAUTILUS.id
            || id == pumpkin_data::entity::EntityType::ZOMBIE_NAUTILUS.id
            || id == pumpkin_data::entity::EntityType::PUFFERFISH.id
            || id == pumpkin_data::entity::EntityType::SALMON.id
            || id == pumpkin_data::entity::EntityType::SQUID.id
            || id == pumpkin_data::entity::EntityType::TADPOLE.id
            || id == pumpkin_data::entity::EntityType::TROPICAL_FISH.id
        {
            navigator.set_water_bound(true);
            if id == pumpkin_data::entity::EntityType::DOLPHIN.id {
                navigator.set_allow_breaching(true);
            }
        }
        Self {
            living_entity: LivingEntity::new(entity),
            sensing: std::sync::Mutex::new(Sensing::default()),
            jump_requested: AtomicBool::new(false),
            brain: None,
            goals_selector: std::sync::Mutex::new(GoalSelector::default()),
            target_selector: std::sync::Mutex::new(GoalSelector::default()),
            navigator: std::sync::Mutex::new(navigator),
            strafe_navigation_kind: AtomicU8::new(0),
            target: tokio::sync::Mutex::new(None),
            look_control: std::sync::Mutex::new(LookControl::default()),
            move_control: std::sync::Mutex::new(Box::new(MoveControl::default())),
            position_target: AtomicCell::new(BlockPos::ZERO),
            position_target_range: AtomicI32::new(-1),
            leash_home_active: AtomicBool::new(false),
            love_ticks: AtomicI32::new(0),
            breeding_cooldown: AtomicI32::new(0),
            schooling_leader: std::sync::Mutex::new(None),
            schooling_size: AtomicI32::new(1),
            no_action_time: AtomicI32::new(0),
            tick_count: AtomicI32::new(0),
            breeder: AtomicCell::new(None),
            owner: AtomicCell::new(None),
            ordered_to_sit: AtomicBool::new(false),
            mob_flags: AtomicU8::new(0),
            ambient_sound_time: AtomicI32::new(-DEFAULT_AMBIENT_SOUND_INTERVAL),
            last_sent_yaw: AtomicU8::new(0),
            last_sent_pitch: AtomicU8::new(0),
            last_sent_head_yaw: AtomicU8::new(0),
        }
    }

    /// Vanilla `Sensing.hasLineOfSight`: cache the result for this mob until the next
    /// `serverAiStep`, and only perform the collision raycast on a cache miss.
    pub async fn has_line_of_sight(&self, target: &dyn EntityBase) -> bool {
        let target_id = target.get_entity().entity_id;
        {
            let sensing = self.sensing.lock().unwrap();
            if sensing.seen.contains(&target_id) {
                return true;
            }
            if sensing.unseen.contains(&target_id) {
                return false;
            }
        }

        let entity = &self.living_entity.entity;
        let target_entity = target.get_entity();
        let from = entity.get_eye_pos();
        let to = target_entity.get_eye_pos();
        let has_line_of_sight = if from.squared_distance_to_vec(&to) > 128.0 * 128.0 {
            false
        } else {
            let world = entity.world.load_full();
            Arc::ptr_eq(&world, &target_entity.world.load_full())
                && world
                    .raycast_collision(from, to, async |block_pos, world| {
                        !world.get_block_state(block_pos).collision_shapes.is_empty()
                    })
                    .await
                    .is_none()
        };

        let mut sensing = self.sensing.lock().unwrap();
        if has_line_of_sight {
            sensing.seen.insert(target_id);
        } else {
            sensing.unseen.insert(target_id);
        }
        has_line_of_sight
    }

    pub(crate) fn set_strafe_navigation_kind(&self, kind: NavigationKind) {
        let encoded = match kind {
            NavigationKind::Ground => 0,
            NavigationKind::Water => 1,
            NavigationKind::Flying => 2,
            NavigationKind::Amphibious => 3,
        };
        self.strafe_navigation_kind.store(encoded, Relaxed);
    }

    pub(crate) fn strafe_navigation_kind(&self) -> NavigationKind {
        match self.strafe_navigation_kind.load(Relaxed) {
            1 => NavigationKind::Water,
            2 => NavigationKind::Flying,
            3 => NavigationKind::Amphibious,
            _ => NavigationKind::Ground,
        }
    }

    pub fn is_in_position_target_range(&self) -> bool {
        self.is_in_position_target_range_pos(&self.living_entity.entity.block_pos.load())
    }

    pub fn is_in_position_target_range_pos(&self, block_pos: &BlockPos) -> bool {
        let position_target_range = self.position_target_range.load(Relaxed);
        if position_target_range == -1 {
            true
        } else {
            let target = self.position_target.load();
            let dx = f64::from(target.0.x) - f64::from(block_pos.0.x);
            let dy = f64::from(target.0.y) - f64::from(block_pos.0.y);
            let dz = f64::from(target.0.z) - f64::from(block_pos.0.z);
            // Java evaluates homeRadius * homeRadius as an int before comparing it to the
            // double distance, so preserve its two's-complement overflow behavior.
            let range_squared = position_target_range.wrapping_mul(position_target_range);
            dx.mul_add(dx, dy.mul_add(dy, dz * dz)) < f64::from(range_squared)
        }
    }

    /// Vanilla `Mob.getMaxFallDistance` (`Mob.java:834-846`) allows a mob with a target to
    /// spend some of its health and the world's difficulty on a larger safe fall.
    pub async fn max_fall_distance(&self) -> i32 {
        max_fall_distance_for_state(
            self.get_target().await.is_some(),
            self.living_entity.health.load(),
            self.living_entity.get_max_health(),
            self.living_entity
                .entity
                .world
                .load()
                .level_info
                .load()
                .difficulty as i32,
        )
    }

    /// Vanilla `Mob.getMaxSpawnClusterSize` (`Mob.java:825-827`) and the concrete overrides in
    /// `AbstractFish.java:59-61`, `AbstractSchoolingFish.java:29-35`, `Wolf.java:551-553`,
    /// `AbstractHorse.java:388-390`, `Ghast.java:156-158`, `Pillager.java:152-154`, and
    /// `HappyGhast.java:241-243`.
    pub const fn max_spawn_cluster_size(&self) -> i32 {
        max_spawn_cluster_size_for(self.living_entity.entity.entity_type.id)
    }

    /// Vanilla `Mob.canShearEquipment` and `Mob.attemptToShearEquipment`
    /// (`Mob.java:568-585`): equipment shearing is available only when this mob is not a
    /// vehicle, and the first equipped item with a shearing-enabled equippable component is
    /// removed. The item component is the server-side source of both eligibility and sound.
    pub async fn can_shear_equipment(&self, _player: &Player) -> bool {
        !self.living_entity.entity.has_vehicle().await
    }

    pub async fn attempt_to_shear_equipment(&self, player: &Player) -> bool {
        if !self.can_shear_equipment(player).await {
            return false;
        }

        let slots = [
            EquipmentSlot::MAIN_HAND,
            EquipmentSlot::OFF_HAND,
            EquipmentSlot::FEET,
            EquipmentSlot::LEGS,
            EquipmentSlot::CHEST,
            EquipmentSlot::HEAD,
            EquipmentSlot::BODY,
            EquipmentSlot::SADDLE,
        ];
        let creative = player.gamemode.load() == pumpkin_util::GameMode::Creative;
        let Some((slot, item, shearing_sound)) = ({
            let equipment = self.living_entity.entity_equipment.lock().await;
            slots.into_iter().find_map(|slot| {
                let item = equipment.get(&slot);
                let equippable = item.get_data_component::<EquippableImpl>()?;
                if !equippable.can_be_sheared
                    || (!creative && item.get_enchantment_level(&Enchantment::BINDING_CURSE) != 0)
                {
                    return None;
                }
                let shearing_sound = equippable.shearing_sound.clone();
                Some((slot, item, shearing_sound))
            })
        }) else {
            return false;
        };

        let mut equipment = self.living_entity.entity_equipment.lock().await;
        equipment.put(&slot, ItemStack::EMPTY.clone());
        drop(equipment);
        self.living_entity
            .send_equipment_changes(&[(slot, ItemStack::EMPTY.clone())]);

        let entity = &self.living_entity.entity;
        let world = entity.world.load();
        player.damage_held_item(1).await;
        let event_context = world
            .get_player_by_id(player.entity_id())
            .map_or_else(GameEventContext::none, |player| {
                GameEventContext::of_entity(player as Arc<dyn EntityBase>)
            });
        emit_game_event(
            &world,
            pumpkin_data::game_event::GameEvent::Shear,
            entity.pos.load(),
            event_context,
        )
        .await;
        world.drop_stack(&entity.block_pos.load(), item).await;
        world.play_sound_event(&shearing_sound, SoundCategory::Neutral, &entity.pos.load());
        true
    }

    pub fn set_attacking(&self, attacking: bool) {
        self.set_mob_flag(Self::ATTACKING_FLAG, attacking);
    }

    pub fn is_attacking(&self) -> bool {
        (self.mob_flags.load(Relaxed) & Self::ATTACKING_FLAG) != 0
    }

    pub fn set_left_handed(&self, left_handed: bool) {
        self.set_mob_flag(Self::LEFT_HANDED_FLAG, left_handed);
    }

    pub fn can_pick_up_loot(&self) -> bool {
        (self.mob_flags.load(Relaxed) & Self::CAN_PICK_UP_LOOT_FLAG) != 0
    }

    pub fn set_can_pick_up_loot(&self, value: bool) {
        self.set_mob_flag(Self::CAN_PICK_UP_LOOT_FLAG, value);
    }

    pub fn is_left_handed(&self) -> bool {
        (self.mob_flags.load(Relaxed) & Self::LEFT_HANDED_FLAG) != 0
    }

    pub fn set_persistence_required(&self) {
        self.living_entity
            .entity
            .persistence_required
            .store(true, Relaxed);
    }

    pub fn is_persistence_required(&self) -> bool {
        self.living_entity.entity.persistence_required.load(Relaxed)
    }

    pub fn set_no_ai(&self, no_ai: bool) {
        self.living_entity.entity.no_ai.store(no_ai, Relaxed);
        let old_flags = self.mob_flags.load(Relaxed);
        let new_flags = if no_ai {
            old_flags | Self::AI_DISABLED_FLAG
        } else {
            old_flags & !Self::AI_DISABLED_FLAG
        };
        self.mob_flags.store(new_flags, Relaxed);
        self.living_entity.entity.send_meta_data(
            &[Metadata::new(
                tracked_data::mob::DATA_MOB_FLAGS_ID,
                new_flags,
            )],
            None,
        );
    }

    pub fn sync_no_ai_flag(&self) {
        let no_ai = self.living_entity.entity.no_ai.load(Relaxed);
        let old_flags = self.mob_flags.load(Relaxed);
        let new_flags = if no_ai {
            old_flags | Self::AI_DISABLED_FLAG
        } else {
            old_flags & !Self::AI_DISABLED_FLAG
        };
        if new_flags != old_flags {
            self.mob_flags.store(new_flags, Relaxed);
            self.living_entity.entity.send_meta_data(
                &[Metadata::new(
                    tracked_data::mob::DATA_MOB_FLAGS_ID,
                    new_flags,
                )],
                None,
            );
        }
    }

    pub fn is_no_ai(&self) -> bool {
        self.living_entity.entity.no_ai.load(Relaxed)
    }

    pub async fn clear_ai_goals(&self, mob: &dyn Mob) {
        let running_goals = self
            .goals_selector
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
        for mut goal in running_goals {
            goal.goal.stop(mob).await;
        }

        let running_target_goals = self
            .target_selector
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
        for mut goal in running_target_goals {
            goal.goal.stop(mob).await;
        }
    }

    pub fn write_mob_nbt(&self, nbt: &mut NbtCompound) {
        if self.is_no_ai() {
            nbt.put_bool("NoAI", true);
        }
        if self.is_left_handed() {
            nbt.put_bool("LeftHanded", true);
        }
        if self.can_pick_up_loot() {
            nbt.put_bool("CanPickUpLoot", true);
        }
    }

    pub fn read_mob_nbt(&self, nbt: &NbtCompound) {
        if let Some(no_ai) = nbt.get_bool("NoAI") {
            self.set_no_ai(no_ai);
        }
        if let Some(left_handed) = nbt.get_bool("LeftHanded") {
            self.set_left_handed(left_handed);
        }
        if let Some(can_pick_up_loot) = nbt.get_bool("CanPickUpLoot") {
            self.set_can_pick_up_loot(can_pick_up_loot);
        }
    }

    pub fn add_goal<G: crate::entity::ai::goal::Goal + 'static>(&self, priority: u8, goal: G) {
        self.goals_selector
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .add_goal(priority, Box::new(goal));
    }

    pub fn add_target_goal<G: crate::entity::ai::goal::Goal + 'static>(
        &self,
        priority: u8,
        goal: G,
    ) {
        self.target_selector
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .add_goal(priority, Box::new(goal));
    }

    pub async fn set_target(&self, target: Option<Arc<dyn EntityBase>>) {
        let mut t = self.target.lock().await;
        *t = target;
    }

    pub async fn get_target(&self) -> Option<Arc<dyn EntityBase>> {
        self.target.lock().await.clone()
    }

    fn set_mob_flag(&self, flag: u8, value: bool) {
        let old_b = self.mob_flags.load(Ordering::Relaxed);

        let new_b = if value { old_b | flag } else { old_b & !flag };

        if new_b != old_b {
            self.mob_flags.store(new_b, Ordering::Relaxed);

            self.living_entity.entity.send_meta_data(
                &[Metadata::new(tracked_data::mob::DATA_MOB_FLAGS_ID, new_b)],
                None,
            );
        }
    }

    pub fn is_in_love(&self) -> bool {
        self.love_ticks.load(Relaxed) > 0
    }

    pub fn is_schooling_follower(&self) -> bool {
        self.schooling_leader
            .lock()
            .expect("schooling leader mutex poisoned")
            .as_ref()
            .is_some_and(|leader| leader.get_entity().is_alive())
    }

    pub fn schooling_leader(&self) -> Option<Arc<dyn EntityBase>> {
        self.schooling_leader
            .lock()
            .expect("schooling leader mutex poisoned")
            .clone()
    }

    #[must_use]
    pub fn has_schooling_followers(&self) -> bool {
        self.schooling_size.load(Relaxed) > 1
    }

    #[must_use]
    pub const fn max_school_size(&self) -> i32 {
        let entity_type = self.living_entity.entity.entity_type;
        if entity_type.id == EntityType::SALMON.id {
            5
        } else if entity_type.id == EntityType::COD.id
            || entity_type.id == EntityType::TROPICAL_FISH.id
        {
            8
        } else {
            0
        }
    }

    #[must_use]
    pub fn can_be_followed_by_schooling_fish(&self) -> bool {
        self.has_schooling_followers() && self.schooling_size.load(Relaxed) < self.max_school_size()
    }

    #[must_use]
    pub fn schooling_followers_remaining(&self) -> usize {
        self.max_school_size()
            .saturating_sub(self.schooling_size.load(Relaxed)) as usize
    }

    /// Vanilla `AbstractSchoolingFish.tick`: occasionally release a stale leader cap after the
    /// school has become isolated. The world query includes this fish, so one nearby fish means
    /// there are no remaining school members to account for.
    fn reset_schooling_if_isolated(&self, isolation_roll: u32) {
        if !self.has_schooling_followers() || isolation_roll != 1 {
            return;
        }

        let entity = &self.living_entity.entity;
        let world = entity.world.load();
        let search_box = entity.bounding_box.load().expand(8.0, 8.0, 8.0);
        let entity_type = entity.entity_type;
        let nearby = world
            .get_entities_at_box(&search_box)
            .into_iter()
            .filter(|candidate| {
                candidate.get_entity().entity_type == entity_type
                    && candidate.get_entity().is_alive()
            })
            .count();
        if nearby <= 1 {
            self.schooling_size.store(1, Relaxed);
        }
    }

    /// Vanilla `AbstractSchoolingFish.startFollowing` plus the leader's follower count update.
    /// Callers must only pass a different, currently non-following fish.
    pub fn start_schooling_following(&self, leader: &Arc<dyn EntityBase>) -> bool {
        if leader.get_entity().entity_id == self.living_entity.entity.entity_id {
            return false;
        }

        let Some(leader_mob) = leader.get_mob() else {
            return false;
        };

        let previous_leader = {
            let mut current = self
                .schooling_leader
                .lock()
                .expect("schooling leader mutex poisoned");
            if current.as_ref().is_some_and(|current| {
                current.get_entity().entity_id == leader.get_entity().entity_id
            }) {
                return false;
            }
            if !leader_mob.get_mob_entity().try_add_schooling_follower() {
                return false;
            }
            current.replace(leader.clone())
        };

        if let Some(previous_leader) = previous_leader
            && let Some(previous_mob) = previous_leader.get_mob()
        {
            previous_mob
                .get_mob_entity()
                .schooling_size
                .fetch_update(Relaxed, Relaxed, |size| Some(size.saturating_sub(1)))
                .ok();
        }

        true
    }

    /// Vanilla `AbstractSchoolingFish.stopFollowing`.
    pub fn stop_schooling_following(&self) {
        let mut current = self
            .schooling_leader
            .lock()
            .expect("schooling leader mutex poisoned");
        if let Some(leader) = current.take()
            && let Some(leader_mob) = leader.get_mob()
        {
            leader_mob
                .get_mob_entity()
                .schooling_size
                .fetch_update(Relaxed, Relaxed, |size| Some(size.saturating_sub(1)))
                .ok();
        }
    }

    /// Stop only if the leader is still the one observed by the goal being stopped.
    /// Entity goals stop asynchronously, so an unconditional clear could erase a newer
    /// assignment made by another fish between the goal stop and this lock acquisition.
    pub fn stop_schooling_following_if(&self, expected: &Arc<dyn EntityBase>) {
        let mut current = self
            .schooling_leader
            .lock()
            .expect("schooling leader mutex poisoned");
        if current
            .as_ref()
            .is_none_or(|leader| leader.get_entity().entity_id != expected.get_entity().entity_id)
        {
            return;
        }
        if let Some(leader) = current.take()
            && let Some(leader_mob) = leader.get_mob()
        {
            leader_mob
                .get_mob_entity()
                .schooling_size
                .fetch_update(Relaxed, Relaxed, |size| Some(size.saturating_sub(1)))
                .ok();
        }
    }

    fn try_add_schooling_follower(&self) -> bool {
        self.schooling_size
            .fetch_update(Relaxed, Relaxed, |size| {
                (size < self.max_school_size()).then_some(size + 1)
            })
            .is_ok()
    }

    pub fn set_love_ticks(&self, ticks: i32, breeder: Option<Uuid>) {
        self.love_ticks.store(ticks, Relaxed);
        self.breeder.store(breeder);
    }

    pub fn reset_love_ticks(&self) {
        self.love_ticks.store(0, Relaxed);
    }

    pub fn try_claim_love(&self) -> bool {
        self.love_ticks
            .fetch_update(Relaxed, Relaxed, |ticks| (ticks > 0).then_some(0))
            .is_ok()
    }

    pub fn is_tamed(&self) -> bool {
        self.owner.load().is_some()
    }

    pub fn set_owner(&self, owner: Uuid) {
        self.owner.store(Some(owner));
    }

    pub fn clear_owner(&self) {
        self.owner.store(None);
    }

    pub fn is_ordered_to_sit(&self) -> bool {
        self.ordered_to_sit.load(Relaxed)
    }

    pub fn set_ordered_to_sit(&self, value: bool) {
        self.ordered_to_sit.store(value, Relaxed);
    }

    pub fn is_breeding_ready(&self) -> bool {
        self.living_entity.entity.age.load(Relaxed) >= 0
            && self.breeding_cooldown.load(Relaxed) <= 0
    }

    pub async fn is_in_attack_range(&self, target: &dyn EntityBase) -> bool {
        const DEFAULT_ATTACK_RANGE: f64 = 0.828_427_12; // sqrt(2.04) - 0.6

        let held_item = self
            .living_entity
            .held_item(&self.living_entity.entity)
            .await;
        let (max_range, min_range) = held_item
            .get_data_component::<pumpkin_data::data_component_impl::AttackRangeImpl>()
            .map_or((DEFAULT_ATTACK_RANGE, 0.0), |attack_range| {
                (
                    f64::from(attack_range.max_reach * attack_range.mob_factor),
                    f64::from(attack_range.min_reach * attack_range.mob_factor),
                )
            });

        let target_hitbox = target.get_entity().bounding_box.load();

        if !self
            .get_attack_box(max_range)
            .await
            .intersects(&target_hitbox)
        {
            return false;
        }

        min_range <= 0.0
            || !self
                .get_attack_box(min_range)
                .await
                .intersects(&target_hitbox)
    }

    pub fn is_dark_enough_to_spawn(world: &World, pos: &BlockPos, is_thundering: bool) -> bool {
        let sky_light = world.get_sky_light_level(pos);
        if sky_light > rand::random_range(0..32) {
            return false;
        }

        let dimension = &world.dimension;
        let block_light_limit = dimension.monster_spawn_block_light_limit;

        let block_light = world.get_block_light_level(pos).unwrap_or(0);
        if block_light_limit < 15 && block_light > block_light_limit {
            return false;
        }

        let current_brightness = if is_thundering {
            world.get_raw_brightness(pos, 10)
        } else {
            world.get_max_local_raw_brightness(pos)
        };

        // TODO
        let mut random = RandomGenerator::Xoroshiro(Xoroshiro::from_seed(get_seed()));
        current_brightness <= dimension.monster_spawn_light_level.get(&mut random) as u8
    }

    pub fn check_monster_spawn_rules(world: &World, pos: &BlockPos, is_thundering: bool) -> bool {
        if world.level_info.load().difficulty == Difficulty::Peaceful {
            return false;
        }

        if !Self::is_dark_enough_to_spawn(world, pos, is_thundering) {
            return false;
        }

        //TODO:check_mob_spawn_rules(entity_type, world, spawn_reason, pos).await
        true
    }

    pub const fn check_any_light_monster_spawn_rules(_world: &World, _pos: &BlockPos) -> bool {
        // Vanilla delegates this predicate to Mob.checkMobSpawnRules. The
        // natural-spawn caller has already run is_spawn_position_ok, which is
        // Pumpkin's equivalent of that block-state predicate.
        true
    }

    #[expect(clippy::too_many_lines)]
    pub async fn try_attack(&self, target: &dyn EntityBase) -> bool {
        if self.living_entity.dead.load(Relaxed) {
            return false;
        }

        let mut attack_damage = self
            .living_entity
            .get_attribute_value(&Attributes::ATTACK_DAMAGE);
        let mut fire_aspect_level = 0u32;
        let base_attack_knockback = self
            .living_entity
            .get_attribute_value(&Attributes::ATTACK_KNOCKBACK);
        let mut knockback_level = 0u32;
        let mut post_hit_effects = Vec::new();
        let held_item = self
            .living_entity
            .held_item(&self.living_entity.entity)
            .await;
        if let Some(enchantments) =
            held_item.get_data_component::<pumpkin_data::data_component_impl::EnchantmentsImpl>()
        {
            let target_type = target.get_entity().entity_type;
            for (enchantment, level) in enchantments.enchantment.iter() {
                for effect in crate::enchantment::effects_for(enchantment) {
                    match effect {
                        crate::enchantment::EnchantmentEffect::Damage(condition, value)
                            if condition.applies(target_type) =>
                        {
                            attack_damage += f64::from(value.calculate(*level));
                        }
                        crate::enchantment::EnchantmentEffect::IgniteOnHit(value) => {
                            fire_aspect_level = (value.calculate(*level) * 20.0) as u32 / 80;
                        }
                        crate::enchantment::EnchantmentEffect::Knockback(condition, value)
                            if *condition == crate::enchantment::KnockbackCondition::Always =>
                        {
                            knockback_level = value.calculate(*level).max(0.0) as u32;
                        }
                        effect @ crate::enchantment::EnchantmentEffect::ApplyMobEffectOnHit {
                            condition,
                            ..
                        } if condition.applies(target_type) => {
                            post_hit_effects.push((*effect, *level));
                        }
                        _ => {}
                    }
                }
            }
        }
        drop(held_item);

        // `Mob.getKnockback` begins with the attacker's attribute, then adds any Knockback
        // enchantment contribution. Most mobs have zero here, but equipment, data packs, and
        // attributes can all make the base value meaningful.
        let knockback_strength = attack_knockback_strength(base_attack_knockback, knockback_level);

        let caller = self
            .living_entity
            .entity
            .world
            .load()
            .get_entity_by_id(self.living_entity.entity.entity_id);

        let damaged = target
            .damage_with_context(
                target,
                attack_damage as f32,
                DamageType::MOB_ATTACK,
                Some(self.living_entity.entity.pos.load()),
                caller.as_deref(),
                caller.as_deref(),
            )
            .await;

        if damaged {
            if fire_aspect_level != 0 {
                target
                    .get_entity()
                    .set_on_fire_for_ticks(fire_aspect_ticks(fire_aspect_level as i32));
            }
            apply_post_hit_mob_effects(target, post_hit_effects).await;
            if knockback_strength > 0.0 {
                let yaw = self.living_entity.entity.yaw.load().to_radians();
                let x = f64::from(yaw.sin());
                let z = f64::from(-yaw.cos());
                if let Some(living) = target.get_living_entity() {
                    living.knockback_with_resistance(knockback_strength, x, z);
                } else {
                    target.get_entity().knockback(knockback_strength, x, z);
                }

                // `Mob.causeExtraKnockback` slows the attacker horizontally after a landed
                // knockback hit. Keep vertical motion intact, as vanilla does.
                let velocity = self.living_entity.entity.velocity.load();
                self.living_entity
                    .entity
                    .velocity
                    .store(velocity.multiply(0.6, 1.0, 0.6));
            }
            self.living_entity
                .last_attacking_id
                .store(target.get_entity().entity_id, Relaxed);
            self.living_entity
                .last_attack_time
                .store(self.living_entity.entity.age.load(Relaxed), Relaxed);
            self.damage_main_hand_weapon_after_hit().await;
        }

        if let Some(caller) = caller.as_deref() {
            self.living_entity.post_piercing_attack(caller).await;
        }

        damaged
    }

    /// `Mob.doHurtTarget` delegates successful weapon strikes to `ItemStack.hurtEnemy`.
    /// Pumpkin's weapon component carries the equivalent durability cost, so mutate and publish
    /// the equipped main-hand stack only after the target actually accepted the hit.
    pub(crate) async fn damage_main_hand_weapon_after_hit(&self) {
        let living = &self.living_entity;
        let slot = EquipmentSlot::MAIN_HAND;
        let mut equipment = living.entity_equipment.lock().await;
        let Some(stack) = equipment.equipment.get_mut(&slot) else {
            return;
        };
        let broken_item = stack.clone();
        let cost = mob_weapon_durability_cost(stack);
        let result = stack.damage_item(cost);
        if result == DamageResult::Untouched {
            return;
        }
        let updated_stack = stack.clone();
        drop(equipment);

        if result == DamageResult::Broken {
            // Vanilla `ItemStack.hurtEnemy` reaches `onEquippedItemBroken` before the broken
            // weapon is sent to tracking clients (`LivingEntity.java:3845-3848`), which
            // broadcasts the break status and removes attribute modifiers; the client then plays
            // `breakItem`'s particles (`LivingEntity.java:1439-1448`) in response.
            living.on_equipped_item_broken(&broken_item, &slot).await;
            living.spawn_item_particles(&broken_item, 5);
        }
        living.send_equipment_changes(&[(slot, updated_stack)]);
    }

    async fn get_attack_box(&self, attack_range: f64) -> BoundingBox {
        let vehicle_lock = self.living_entity.entity.vehicle.lock().await;

        let base_box = vehicle_lock.as_ref().map_or_else(
            || self.living_entity.entity.bounding_box.load(),
            |vehicle| {
                let vehicle_box = vehicle.get_entity().bounding_box.load();
                let my_box = self.living_entity.entity.bounding_box.load();

                BoundingBox {
                    min: Vector3::new(
                        my_box.min.x.min(vehicle_box.min.x),
                        my_box.min.y,
                        my_box.min.z.min(vehicle_box.min.z),
                    ),
                    max: Vector3::new(
                        my_box.max.x.max(vehicle_box.max.x),
                        my_box.max.y,
                        my_box.max.z.max(vehicle_box.max.z),
                    ),
                }
            },
        );

        base_box.expand(attack_range, 0.0, attack_range)
    }

    async fn is_sun_burn_tick(&self, brightness: f32) -> bool {
        let entity = &self.living_entity.entity;

        let world_arc = entity.world.load();
        let world = world_arc.as_ref();

        // Night boundary from data/minecraft/timeline/day.json — monsters_burn keyframes:
        // value=false at tick 12542 (dusk), value=true at tick 23460 (dawn).
        // TODO: read directly from EnvironmentAttributes::MONSTERS_BURN once implemented.

        let day_time = world.get_time_of_day().await % 24000;
        if (NIGHT_START..=NIGHT_END).contains(&day_time) {
            return false;
        }

        if brightness <= 0.5 {
            return false;
        }

        let pos = entity.pos.load();
        let block_pos = BlockPos::floored(pos.x, pos.y, pos.z);
        let head_pos = BlockPos::floored(pos.x, entity.bounding_box.load().max.y, pos.z);
        let is_in_rain =
            world.is_raining_at(&block_pos).await || world.is_raining_at(&head_pos).await;
        let is_in_non_burnable = entity.touching_water.load(Relaxed)
            || is_in_rain
            || entity.is_in_powder_snow()
            || entity.was_in_powder_snow.load(Relaxed);

        if is_in_non_burnable {
            return false;
        }

        let eye_block_pos = BlockPos::floored(pos.x, entity.get_eye_y(), pos.z);
        if !world.can_see_sky(&eye_block_pos) {
            return false;
        }

        let mut rng = rand::rng();
        rng.random::<f32>() * 30.0 < (brightness - 0.4) * 2.0
    }

    fn apply_sun_burn(&self) {
        let entity = &self.living_entity.entity;
        entity.set_on_fire_for(8.0);
    }

    pub async fn mob_interact(
        &self,
        player: &Arc<Player>,
        item_stack: &mut ItemStack,
        can_be_leashed: bool,
    ) -> bool {
        let entity = &self.living_entity.entity;

        // If already leashed to player, right-clicking unleashes the mob
        let currently_leashed = {
            let guard = entity.leashed_to.lock().await;
            guard.is_some()
        };

        if currently_leashed {
            entity.unleash().await;
            let lead_item =
                pumpkin_data::item_stack::ItemStack::new(1, &pumpkin_data::item::Item::LEAD);
            entity
                .world
                .load()
                .drop_stack(&entity.block_pos.load(), lead_item)
                .await;
            return true;
        }

        // If holding a lead, leash the mob to the player
        if can_be_leashed
            && (item_stack.item.registry_key == "lead"
                || item_stack.item.registry_key == "minecraft:lead")
        {
            let diff = entity.pos.load() - player.get_entity().pos.load();
            let dist_sq = diff.length_squared();
            let leash_snap_distance = entity.leash_snap_distance();
            if dist_sq <= leash_snap_distance * leash_snap_distance {
                entity.leash_to(player.clone() as Arc<dyn EntityBase>).await;
                if player.gamemode.load() != pumpkin_util::GameMode::Creative {
                    item_stack.decrement(1);
                }
                return true;
            }
        }

        false
    }
}

/// Vanilla `SpawnEggItem.spawnOffspringFromSpawnEgg` only produces a baby when the clicked mob
/// is an `AgeableMob` whose `getBreedOffspring` returns an entity. These are the registered
/// types that satisfy both. Parrots, wandering traders, camel husks, zombie nautiluses, slimes
/// and magma cubes are ageable but always breed to null, so a matching egg used on them falls
/// through to the mob's regular interaction instead.
const fn spawns_offspring_from_egg(entity_type: &EntityType) -> bool {
    let id = entity_type.id;
    id == EntityType::ARMADILLO.id
        || id == EntityType::AXOLOTL.id
        || id == EntityType::BEE.id
        || id == EntityType::CAMEL.id
        || id == EntityType::CAT.id
        || id == EntityType::CHICKEN.id
        || id == EntityType::COW.id
        || id == EntityType::DOLPHIN.id
        || id == EntityType::DONKEY.id
        || id == EntityType::FOX.id
        || id == EntityType::FROG.id
        || id == EntityType::GLOW_SQUID.id
        || id == EntityType::GOAT.id
        || id == EntityType::HAPPY_GHAST.id
        || id == EntityType::HOGLIN.id
        || id == EntityType::HORSE.id
        || id == EntityType::LLAMA.id
        || id == EntityType::MOOSHROOM.id
        || id == EntityType::MULE.id
        || id == EntityType::NAUTILUS.id
        || id == EntityType::OCELOT.id
        || id == EntityType::PANDA.id
        || id == EntityType::PIG.id
        || id == EntityType::POLAR_BEAR.id
        || id == EntityType::RABBIT.id
        || id == EntityType::SHEEP.id
        || id == EntityType::SKELETON_HORSE.id
        || id == EntityType::SNIFFER.id
        || id == EntityType::SQUID.id
        || id == EntityType::STRIDER.id
        || id == EntityType::SULFUR_CUBE.id
        || id == EntityType::TRADER_LLAMA.id
        || id == EntityType::TURTLE.id
        || id == EntityType::VILLAGER.id
        || id == EntityType::WOLF.id
        || id == EntityType::ZOMBIE_HORSE.id
}

pub trait Mob: EntityBase + Send + Sync {
    /// Vanilla `HasCustomInventoryScreen.openCustomInventoryScreen` is dispatched by the
    /// ridden-vehicle inventory command (`ServerGamePacketListenerImpl.java:1734-1737`).
    fn open_custom_inventory_screen<'a>(
        &'a self,
        _player: &'a Arc<crate::entity::player::Player>,
    ) -> crate::entity::EntityBaseFuture<'a, ()> {
        Box::pin(async {})
    }

    /// Vanilla `Mob.canDispenserEquipIntoSlot` (`Mob.java:1117-1118`) lets a mob
    /// accept dispenser equipment only when its loot-pickup flag is enabled.
    fn can_dispenser_equip_into_slot(&self, _slot: &EquipmentSlot) -> bool {
        self.get_mob_entity().can_pick_up_loot()
    }

    /// `Entity.deflection` (`Entity.java:3491-3493`) for mobs. The blanket `EntityBase` impl
    /// below forwards to this so that individual mobs can override it; only the breeze does
    /// (`Breeze.java:196-202`).
    fn mob_projectile_deflection(
        &self,
        _projectile: &dyn EntityBase,
    ) -> crate::entity::projectile_deflection::ProjectileDeflectionType {
        crate::entity::projectile_deflection::ProjectileDeflectionType::None
    }

    /// Vanilla `Mob.hasControllingPassenger` and `Mob.getControllingPassenger`.
    /// Rideable mobs with player-specific controls override this (for example, Pig).
    fn has_controlling_passenger(&self) -> EntityBaseFuture<'_, bool> {
        self.default_has_controlling_passenger()
    }

    /// Server-side `PlayerRideableJumping` hooks (`PlayerRideableJumping.java:3-16`). Rideable
    /// mobs override these; keeping the defaults inert preserves ordinary mob command handling.
    fn can_jump(&self) -> EntityBaseFuture<'_, bool> {
        Box::pin(async { false })
    }

    fn on_player_jump(&self, _jump_amount: i32) {}

    fn handle_start_jump(&self, _jump_scale: i32) {}

    fn handle_stop_jump(&self) {}

    /// `AbstractHorse.isBred` is consumed by its cross-species follow-mommy goal. Other mobs do
    /// not have that flag and therefore remain ineligible as horse parents.
    fn is_bred(&self) -> bool {
        false
    }

    /// Vanilla `Leashable.onElasticLeashPull` (`Leashable.java:176-178`) delegates to
    /// `Entity.checkFallDistanceAccumulation` before entity-specific leash behavior.
    fn default_on_elastic_leash_pull(&self) {
        self.get_mob_entity()
            .living_entity
            .check_fall_distance_accumulation();
    }

    /// Called when the shared leash solver applies an elastic pull. Most mobs have no
    /// pull-specific state to clear.
    fn on_elastic_leash_pull(&self) {
        self.default_on_elastic_leash_pull();
    }

    /// Forwards the holder-side leash callback to entity-specific mob behavior
    /// (`Entity.java:3836`; `Leashable.java:198`).
    fn notify_leash_holder(&self, _entity: &dyn EntityBase) {}

    /// The `Mob`-level behaviour, callable from an override without recursing.
    ///
    /// `Mob::has_controlling_passenger(self)` inside an override resolves back to that same
    /// override, so a pig that fell through to it recursed until the tokio worker's stack died.
    fn default_has_controlling_passenger(&self) -> EntityBaseFuture<'_, bool> {
        Box::pin(async move {
            if self.get_mob_entity().is_no_ai() {
                return false;
            }
            let Some(passenger) = self.get_entity().passengers.lock().await.first().cloned() else {
                return false;
            };
            let Some(mob) = passenger.get_mob() else {
                return false;
            };
            !mob.get_entity()
                .entity_type
                .has_tag(&tag::EntityType::MINECRAFT_NON_CONTROLLING_RIDER)
        })
    }

    /// The `Mob` half of `LivingEntity.isSensitiveToWater`; overridden by Strider, Blaze and the
    /// snow golem.
    fn mob_is_sensitive_to_water(&self) -> bool {
        false
    }

    /// The `Mob` half of `EntityBase::can_use_portal`; overridden by the ender dragon.
    fn mob_can_use_portal(&self) -> bool {
        true
    }

    /// Called from the blanket `damage_with_context` after `modify_incoming_damage`, before the
    /// amount is applied to the underlying `LivingEntity`. Lets a mob clamp incoming damage
    /// (e.g. the ender dragon refusing to let a killing blow drop its health below 1 while not
    /// sitting, `EnderDragon.handleKillingBlow`, `EnderDragon.java:484-490`). The bool return
    /// says whether `mob_on_lethal_rescue` should run once the hit is confirmed to have landed.
    /// Default: pass the amount through unchanged, no rescue.
    fn mob_pre_apply_damage(&self, _health: f32, amount: f32) -> EntityBaseFuture<'_, (f32, bool)> {
        Box::pin(async move { (amount, false) })
    }

    /// Runs once a hit `mob_pre_apply_damage` flagged for rescue is confirmed to have landed.
    /// Default: no-op.
    fn mob_on_lethal_rescue(&self) -> EntityBaseFuture<'_, ()> {
        Box::pin(async move {})
    }

    /// The `Mob` half of `LivingEntity.calculateFallDamage`. The blanket `EntityBase` impl below
    /// routes to this, so a mob overrides here rather than on `EntityBase`.
    fn mob_calculate_fall_damage(&self, fall_distance: f64, damage_modifier: f32) -> i32 {
        self.get_mob_entity()
            .living_entity
            .default_calculate_fall_damage(fall_distance, damage_modifier)
    }

    /// Vanilla `Drowned.wantsToSwim`; ordinary mobs do not have a swimming controller state.
    fn wants_to_swim(&self) -> bool {
        false
    }

    /// Vanilla `Drowned.isSearchingForLand`.
    fn is_searching_for_land(&self) -> bool {
        false
    }

    /// Vanilla target-height check used by `DrownedMoveControl`.
    fn target_is_above(&self) -> bool {
        false
    }

    /// Vanilla `Drowned.setSearchingForLand`.
    fn set_searching_for_land(&self, _searching: bool) {}

    /// Vanilla `Entity.isAffectedByFluids`; ordinary mobs use the base `true` behavior.
    fn is_affected_by_fluids(&self) -> bool {
        true
    }

    /// Vanilla `Mob` entities are pickable unless a concrete entity overrides it.
    fn is_pickable(&self) -> bool {
        self.get_entity().is_alive()
    }

    /// Vanilla `Entity.canBeCollidedWith`; ordinary mobs do not collide with a
    /// null-source collision query, while a few concrete mob types do.
    fn can_be_collided_with(&self) -> bool {
        false
    }

    fn get_random(&self) -> rand::rngs::ThreadRng {
        rand::rng()
    }

    /// Vanilla `Mob.getAmbientSound` (Mob.java:363-365), which returns null for a mob with no
    /// idle sound. The base `Mob.baseTick` roll still happens for those mobs; it just makes no
    /// sound, so a mob without an entry here stays silent rather than borrowing another's.
    fn get_ambient_sound(&self) -> Option<Sound> {
        None
    }

    /// Vanilla `NeutralMob.isPreventingPlayerRest`: a neutral mob may block sleep
    /// based on the player-specific anger state rather than merely its entity type.
    fn is_preventing_player_rest(
        &self,
        _player_uuid: Uuid,
        _universal_anger: bool,
    ) -> EntityBaseFuture<'_, bool> {
        Box::pin(async { false })
    }

    /// Vanilla `Mob.getSoundSource`; neutral is the existing default for ordinary mobs.
    fn get_sound_source(&self) -> SoundCategory {
        SoundCategory::Neutral
    }

    /// Sound emitted by the mob's `playStepSound` hook, or `None` when the mob uses the generic
    /// block step path. `AbstractHorse.playStepSound` (`AbstractHorse.java:342-360`) uses the
    /// horse step sound for ordinary ground movement; the generic sound hook supplies that
    /// server-visible part for horse-family entities.
    fn get_step_sound(&self) -> Option<Sound> {
        let entity_type = self.get_entity().entity_type;
        let is_horse = matches!(
            entity_type.id,
            id if id == pumpkin_data::entity::EntityType::HORSE.id
                || id == pumpkin_data::entity::EntityType::DONKEY.id
                || id == pumpkin_data::entity::EntityType::MULE.id
                || id == pumpkin_data::entity::EntityType::SKELETON_HORSE.id
                || id == pumpkin_data::entity::EntityType::ZOMBIE_HORSE.id
        );
        if !is_horse {
            return None;
        }
        let entity = self.get_entity();
        let ridden = entity
            .passengers
            .try_lock()
            .is_ok_and(|passengers| !passengers.is_empty());
        let tick_count = self.get_mob_entity().tick_count.load(Relaxed);
        if ridden && tick_count > 5 && tick_count % 3 == 0 {
            // `AbstractHorse.playGallopSound` (`AbstractHorse.java:350-375`) is selected after
            // the initial ridden steps; `tick_swim_sound` supplies the shared step dispatch.
            Some(Sound::EntityHorseGallop)
        } else if entity.age.load(Relaxed) < 0 {
            Some(Sound::EntityBabyHorseStep)
        } else {
            Some(Sound::EntityHorseStep)
        }
    }

    /// Vanilla `LivingEntity.getVoicePitch` is consumed by `makeSound` for mob sounds
    /// (`LivingEntity.java:1431-1434, 2321-2325`).
    fn get_sound_pitch(&self) -> f32 {
        let is_baby = self.get_entity().age.load(Relaxed) < 0;
        let base = if is_baby { 1.5 } else { 1.0 };
        (rand::random::<f32>() - rand::random::<f32>()).mul_add(0.2, base)
    }

    /// Instance-specific override of the server-played hurt sound, or `None` to keep the
    /// static `EntityType::hurt_sound` table consulted by `LivingEntity::hurt_sound`.
    /// Only mobs whose hurt sound depends on instance state override this, e.g. the copper
    /// golem's oxidation stage (`CopperGolem.getHurtSound`, `CopperGolem.java:389-391`).
    fn get_hurt_sound(&self) -> Option<Sound> {
        None
    }

    /// Vanilla `Mob.getAmbientSoundInterval` (Mob.java:274-276), with the
    /// `AbstractGolem` override (`AbstractGolem.java:29-31`) for every concrete golem.
    fn get_ambient_sound_interval(&self) -> i32 {
        match self.get_entity().entity_type.id {
            id if id == pumpkin_data::entity::EntityType::IRON_GOLEM.id
                || id == pumpkin_data::entity::EntityType::SNOW_GOLEM.id
                || id == pumpkin_data::entity::EntityType::COPPER_GOLEM.id =>
            {
                GOLEM_AMBIENT_SOUND_INTERVAL
            }
            id if is_animal_entity(id) => 120,
            _ => DEFAULT_AMBIENT_SOUND_INTERVAL,
        }
    }

    /// `Mob.baseTick` (Mob.java:282-292): while alive, `ambientSoundTime` counts up once per
    /// tick and a `nextInt(1000)` roll below it fires `playAmbientSound` (Mob.java:278-280).
    /// The timer is reset by `resetAmbientSoundTime` (Mob.java:301-303) whenever the roll wins,
    /// including when `getAmbientSound` is null -- `makeSound` (LivingEntity.java:1431-1435)
    /// simply does nothing for a null sound.
    fn tick_ambient_sound(&self) {
        let mob_entity = self.get_mob_entity();
        let entity = &mob_entity.living_entity.entity;
        if !entity.is_alive() {
            return;
        }
        let ambient_time = mob_entity.ambient_sound_time.fetch_add(1, Relaxed);
        if self.get_random().random_range(0..1000) < ambient_time {
            mob_entity
                .ambient_sound_time
                .store(-self.get_ambient_sound_interval(), Relaxed);
            if let Some(sound) = self.get_ambient_sound() {
                entity.world.load().play_sound_fine(
                    sound,
                    self.get_sound_source(),
                    &entity.pos.load(),
                    1.0,
                    self.get_sound_pitch(),
                );
            }
        }
    }

    /// Vanilla `Mob.isMaxGroupSizeReached`; most mobs accept the configured group size.
    fn is_max_group_size_reached(&self, _group_size: i32) -> bool {
        false
    }

    /// Vanilla `Entity.getLightLevelDependentMagicValue` used by
    /// `Monster.updateNoActionTime`.
    fn light_level_dependent_magic_value(&self, world: &World) -> f32 {
        let eye_pos = BlockPos::floored_v(self.get_entity().get_eye_pos());
        if !world.level.is_chunk_loaded(&eye_pos.chunk_position()) {
            return 0.0;
        }

        let brightness = f32::from(world.get_max_local_raw_brightness(&eye_pos)) / 15.0;
        let curved_brightness = brightness / (4.0 - 3.0 * brightness);
        curved_brightness + world.dimension.ambient_light * (1.0 - curved_brightness)
    }

    fn get_max_look_yaw_change(&self) -> f32 {
        10.0
    }

    fn get_max_look_pitch_change(&self) -> f32 {
        40.0
    }

    fn get_max_head_rotation(&self) -> f32 {
        75.0
    }

    fn get_mob_entity(&self) -> &MobEntity;

    /// Passenger-specific attachment overrides are dispatched by the existing rider position
    /// update, matching `Entity.positionRider` (`Entity.java:2387-2394`).
    fn get_vehicle_attachment_point(&self, _vehicle: &Entity) -> Option<Vector3<f64>> {
        None
    }

    /// Vanilla `Mob.sunProtectionSlot`; zombie horses use their body slot.
    fn sun_protection_slot(&self) -> EquipmentSlot {
        EquipmentSlot::HEAD
    }

    /// Vanilla `Mob.burnUndead`, called after `LivingEntity.aiStep`.
    fn tick_sun_burn(&self) -> EntityBaseFuture<'_, ()> {
        Box::pin(async move {
            let mob_entity = self.get_mob_entity();
            let living = &mob_entity.living_entity;
            let entity = &living.entity;
            if living.dead.load(Relaxed)
                || living.health.load() <= 0.0
                || entity.is_removed()
                || !entity
                    .entity_type
                    .has_tag(&tag::EntityType::MINECRAFT_BURN_IN_DAYLIGHT)
                || !mob_entity
                    .is_sun_burn_tick(self.light_level_dependent_magic_value(&entity.world.load()))
                    .await
            {
                return;
            }
            if living.dead.load(Relaxed) || living.health.load() <= 0.0 || entity.is_removed() {
                return;
            }

            let slot = self.sun_protection_slot();
            let mut stack = {
                let equipment = living.entity_equipment.lock().await;
                equipment.get(&slot)
            };
            if living.dead.load(Relaxed) || living.health.load() <= 0.0 || entity.is_removed() {
                return;
            }
            if !stack.is_empty() {
                let broken_item = stack.clone();
                if stack.is_damageable() && !stack.is_unbreakable() && rand::random_range(0..2) != 0
                {
                    let new_damage = stack.get_damage() + 1;
                    let broken = stack
                        .get_max_damage()
                        .is_some_and(|max_damage| new_damage >= max_damage);
                    if broken {
                        stack = ItemStack::EMPTY.clone();
                    } else {
                        stack.set_damage(new_damage);
                    }
                    let updated_stack = stack.clone();
                    if broken {
                        // Vanilla `Mob.burnUndead` invokes `LivingEntity.onEquippedItemBroken`
                        // before clearing the slot (`Mob.java:485-496`; `LivingEntity.java:3845-3848`).
                        living.on_equipped_item_broken(&broken_item, &slot).await;
                    }
                    living.entity_equipment.lock().await.put(&slot, stack);
                    living.send_equipment_changes(&[(slot, updated_stack)]);
                }
                return;
            }

            mob_entity.apply_sun_burn();
        })
    }

    /// Vanilla `Mob.updateControlFlags`: a mob riding another controlling mob gives up its
    /// movement/look/jump goals, while a mob in a boat only gives up jump goals.
    fn update_control_flags(&self) -> EntityBaseFuture<'_, ()> {
        Box::pin(async move {
            let entity = self.get_entity();
            let controlled_by_mob = {
                let passengers = entity.passengers.lock().await;
                !self.get_mob_entity().is_no_ai()
                    && passengers
                        .first()
                        .and_then(|passenger| passenger.get_mob())
                        .is_some_and(|mob| {
                            !mob.get_entity()
                                .entity_type
                                .has_tag(&tag::EntityType::MINECRAFT_NON_CONTROLLING_RIDER)
                        })
            };
            let not_in_boat = {
                let vehicle = entity.vehicle.lock().await;
                vehicle.as_ref().is_none_or(|vehicle| {
                    !vehicle
                        .get_entity()
                        .entity_type
                        .has_tag(&tag::EntityType::C_BOATS)
                })
            };

            let mut goals = self.get_mob_entity().goals_selector.lock().unwrap();
            goals.set_control_enabled(Controls::MOVE, !controlled_by_mob);
            goals.set_control_enabled(Controls::JUMP, !controlled_by_mob && not_in_boat);
            goals.set_control_enabled(Controls::LOOK, !controlled_by_mob);
        })
    }

    /// Vanilla `Mob.setPersistenceRequired`.
    fn set_persistence_required(&self) {
        self.get_mob_entity().set_persistence_required();
    }

    /// Vanilla `Mob.isPersistenceRequired`.
    fn is_persistence_required(&self) -> bool {
        self.get_mob_entity().is_persistence_required()
    }

    /// Vanilla `Mob.removeWhenFarAway`, including the `AbstractGolem` override
    /// (`AbstractGolem.java:34-36`) and the current species overrides whose state is
    /// represented by Pumpkin.
    fn remove_when_far_away(&self, _dist_sqr: f64) -> bool {
        let category = self.get_entity().entity_type.category;
        let mob_entity = self.get_mob_entity();
        match self.get_entity().entity_type.id {
            // Cat.java: untamed cats become removable after 120 seconds.
            id if id == pumpkin_data::entity::EntityType::CAT.id => {
                !mob_entity.is_tamed() && mob_entity.tick_count.load(Relaxed) > 2400
            }
            // Ocelot.java: Pumpkin has no trust state yet, so its spawned ocelots
            // follow the vanilla untamed branch.
            id if id == pumpkin_data::entity::EntityType::OCELOT.id => {
                mob_entity.tick_count.load(Relaxed) > 2400
            }
            // AbstractFish.java: tadpoles are bucketable fish despite their
            // CREATURE category.
            id if id == pumpkin_data::entity::EntityType::TADPOLE.id => true,
            // AbstractFish and Axolotl override Animal's non-despawning default.
            // Vanilla also keeps custom-named fish, even when they were named by NBT
            // rather than through an interaction that sets PersistenceRequired.
            id if id == pumpkin_data::entity::EntityType::AXOLOTL.id
                || id == pumpkin_data::entity::EntityType::COD.id
                || id == pumpkin_data::entity::EntityType::NAUTILUS.id
                || id == pumpkin_data::entity::EntityType::PUFFERFISH.id
                || id == pumpkin_data::entity::EntityType::SALMON.id
                || id == pumpkin_data::entity::EntityType::TROPICAL_FISH.id =>
            {
                (**mob_entity.living_entity.entity.custom_name.load()).is_none()
            }
            id if id == pumpkin_data::entity::EntityType::ZOMBIE_HORSE.id => true,
            // AbstractGolem.java:34-36: golems never despawn because of distance.
            id if id == pumpkin_data::entity::EntityType::IRON_GOLEM.id
                || id == pumpkin_data::entity::EntityType::SNOW_GOLEM.id
                || id == pumpkin_data::entity::EntityType::COPPER_GOLEM.id =>
            {
                false
            }
            // Animal and non-despawning MISC mob implementations in the
            // generated registry use the persistent far-away behavior.
            _ if category == &MobCategory::CREATURE || category == &MobCategory::MISC => false,
            _ => true,
        }
    }

    /// Vanilla `Mob.requiresCustomPersistence`: passengers and leashed mobs
    /// must not be removed by the normal despawn checks.
    fn requires_custom_persistence_cached(&self) -> bool {
        let entity = self.get_entity();
        self.has_custom_persistence_state()
            || entity.vehicle_persistence_required.load(Relaxed)
            || entity.leash_persistence_required.load(Relaxed)
            || (entity
                .entity_type
                .has_tag(&tag::EntityType::MINECRAFT_RAIDERS)
                && self.get_mob_entity().living_entity.has_active_raid())
    }

    /// Species-specific state that keeps a mob persistent, such as an Enderman's carried block.
    fn has_custom_persistence_state(&self) -> bool {
        false
    }

    fn requires_custom_persistence(&self) -> EntityBaseFuture<'_, bool> {
        Box::pin(async move { self.requires_custom_persistence_cached() })
    }

    /// Vanilla `Mob.checkDespawn`, called by the server entity tick loop.
    fn check_despawn(&self) -> EntityBaseFuture<'_, ()> {
        Box::pin(async move {
            let mob_entity = self.get_mob_entity();
            let entity = self.get_entity();
            if entity.is_removed() {
                return;
            }
            let world = entity.world.load();

            if world.level_info.load().difficulty == Difficulty::Peaceful
                && !entity.entity_type.allowed_in_peaceful
            {
                entity.remove().await;
                return;
            }

            if self.is_persistence_required() || self.requires_custom_persistence().await {
                mob_entity.no_action_time.store(0, Relaxed);
                return;
            }

            let position = entity.pos.load();
            let nearest_player_distance = world
                .players
                .load()
                .iter()
                .filter(|player| !player.is_spectator())
                .map(|player| {
                    player
                        .get_entity()
                        .pos
                        .load()
                        .squared_distance_to_vec(&position)
                })
                .min_by(f64::total_cmp);

            let Some(distance_sqr) = nearest_player_distance else {
                return;
            };

            let despawn_distance = f64::from(entity.entity_type.category.despawn_distance);
            if distance_sqr > despawn_distance * despawn_distance
                && self.remove_when_far_away(distance_sqr)
            {
                entity.remove().await;
                return;
            }

            let no_despawn_distance = f64::from(MobCategory::NO_DESPAWN_DISTANCE);
            let no_despawn_distance_sqr = no_despawn_distance * no_despawn_distance;
            let no_action_time = mob_entity.no_action_time.load(Relaxed);
            if no_action_time > 600
                && rand::random_range(0..800) == 0
                && distance_sqr > no_despawn_distance_sqr
                && self.remove_when_far_away(distance_sqr)
            {
                entity.remove().await;
            } else if distance_sqr < no_despawn_distance_sqr {
                mob_entity.no_action_time.store(0, Relaxed);
            }
        })
    }

    /// `Raider.canBeLeader` default (all raiders except `Ravager`, which overrides to `false`).
    fn can_be_raid_leader(&self) -> bool {
        true
    }

    /// `Raider.applyRaidBuffs` default no-op. Raid-participant mobs (Vindicator, Pillager,
    /// Witch, Evoker, Illusioner) override this to enchant gear or grant potion effects;
    /// those overrides are separate work and are not implemented here.
    fn apply_raid_buffs(&self, _wave: i32, _is_captain: bool) -> EntityBaseFuture<'_, ()> {
        Box::pin(async {})
    }

    /// Vanilla `Mob.enchantSpawnedWeapon` override seam (`Mob.java:1065-1067`). The generic
    /// spawn-equipment pass (`equipment::equip_mob_on_spawn`) already runs the base roll
    /// (`0.25F * special multiplier` via the `MOB_SPAWN_EQUIPMENT` provider); this hook is then
    /// invoked with the freshly rolled main-hand stack so subclasses can chain their own
    /// provider roll exactly as vanilla's virtual dispatch does. Default no-op; Pillager
    /// overrides this with its 1-in-300 `PILLAGER_SPAWN_CROSSBOW` Piercing roll
    /// (`Pillager.java:172-181`).
    fn enchant_spawned_weapon(&self, _main_hand: &mut ItemStack) {}

    /// Vanilla `LivingEntity.blockedByItem`: called on the attacker (`self`) when `defender`
    /// successfully shield-blocks one of `self`'s attacks. Default no-op; Ravager overrides this
    /// to sometimes stun itself, Hoglin/Zoglin have their own vanilla overrides not yet ported.
    fn blocked_by_item<'a>(
        &'a self,
        _defender: &'a dyn EntityBase,
        _damage: f32,
    ) -> EntityBaseFuture<'a, ()> {
        Box::pin(async {})
    }

    /// Vanilla `CrossbowAttackMob.setChargingCrossbow` (synced data, drives `getArmPose`'s
    /// `CROSSBOW_CHARGE` state client-side). Default no-op; crossbow-wielding mobs (Pillager)
    /// override this to store and broadcast the flag.
    fn set_charging_crossbow(&self, _charging: bool) {}

    /// Vanilla `Mob.chargeSpeedModifier` (`Mob.java:1480`) scales the mounted spear
    /// approach/reposition speeds; the base mob has no modifier.
    fn charge_speed_modifier(&self) -> f32 {
        1.0
    }

    fn try_attack<'a>(&'a self, target: &'a dyn EntityBase) -> EntityBaseFuture<'a, bool> {
        Box::pin(async move {
            let damaged = self.get_mob_entity().try_attack(target).await;
            if damaged {
                self.on_successful_attack(target).await;
            }
            damaged
        })
    }

    fn on_successful_attack<'a>(&'a self, _target: &'a dyn EntityBase) -> EntityBaseFuture<'a, ()> {
        Box::pin(async {})
    }

    fn mob_bedrock_identifier(&self) -> Option<&'static str> {
        None
    }

    /// Metadata which must accompany this mob whenever it is spawned for a Java client.
    fn mob_java_spawn_metadata(
        &self,
        _version: JavaMinecraftVersion,
    ) -> EntityBaseFuture<'_, Option<Box<[u8]>>> {
        Box::pin(async { None })
    }

    /// Metadata which must accompany this mob whenever it is spawned for a Bedrock client.
    fn mob_bedrock_spawn_metadata(
        &self,
    ) -> EntityBaseFuture<
        '_,
        Option<pumpkin_protocol::bedrock::client::set_actor_data::EntityMetadata>,
    > {
        Box::pin(async { None })
    }

    fn get_job_site(&self) -> Option<BlockPos> {
        None
    }

    fn is_job_site_pending(&self) -> EntityBaseFuture<'_, bool> {
        Box::pin(async { false })
    }

    fn release_pending_job_site(&self, _position: BlockPos) -> EntityBaseFuture<'_, ()> {
        Box::pin(async {})
    }

    fn get_trading_player(&self) -> Option<Arc<Player>> {
        None
    }

    fn get_home(&self) -> Option<BlockPos> {
        None
    }

    /// Vanilla `PathfinderMob.getWalkTargetValue`; concrete mobs may override the position
    /// weight used by `DefaultRandomPos`.
    fn get_walk_target_value(&self, _pos: &BlockPos) -> f64 {
        0.0
    }

    fn get_meeting_point(&self) -> Option<BlockPos> {
        None
    }

    fn get_path_aware_entity(&self) -> Option<&dyn PathAwareEntity> {
        None
    }

    fn get_item_steerable(&self) -> Option<&dyn crate::entity::item_steerable::ItemSteerable> {
        None
    }

    fn is_saddled(&self) -> bool {
        false
    }

    fn can_be_saddled(&self) -> bool {
        false
    }

    fn set_saddled(&self, _saddled: bool) {}

    /// Vanilla `PathfinderMob.whenLeashedTo` (`PathfinderMob.java:100-104`): while leashed, the
    /// home restriction is retargeted to the holder's block position with radius
    /// `(int)leashElasticDistance() - 1` = 5 (`Leashable.java:191-193`). Vanilla re-runs this
    /// every leashed tick from `Leashable.tickLeash` (`Leashable.java:155-160`); pumpkin does
    /// the same from the Mob tick that consumes `tick_leash`. Plain-Mob leashees in vanilla
    /// (ghast, phantom, ender dragon) override nothing here, and their goal selectors never
    /// consult the restriction, so storing it unconditionally is observably equivalent.
    fn when_leashed_to(&self, holder_block_pos: BlockPos) {
        let mob_entity = self.get_mob_entity();
        mob_entity.position_target.store(holder_block_pos);
        mob_entity.position_target_range.store(
            self.get_entity().leash_elastic_distance() as i32 - 1,
            Relaxed,
        );
        mob_entity.leash_home_active.store(true, Relaxed);
    }

    /// Vanilla `Mob.onLeashRemoved` clears the home radius when no leash data remains
    /// (`Mob.java:1279-1284`). The flag preserves unrelated home restrictions represented by the
    /// same shared fields until the leash is actually removed.
    fn on_leash_removed(&self) {
        let mob_entity = self.get_mob_entity();
        if mob_entity.leash_home_active.swap(false, Relaxed) {
            mob_entity.position_target.store(BlockPos::ZERO);
            mob_entity.position_target_range.store(-1, Relaxed);
        }
    }

    /// Vanilla `PathfinderMob.closeRangeLeashBehaviour`: keep a non-panicking mob
    /// navigating toward its leash holder while preserving a two-block gap.
    fn close_range_leash_behavior(&self, holder_pos: Vector3<f64>, distance: f64) {
        if !self.should_follow_leash() || self.is_panicking() {
            return;
        }

        self.get_mob_entity()
            .goals_selector
            .lock()
            .unwrap()
            .enable_control(Controls::MOVE);

        let mob_pos = self.get_mob_entity().living_entity.entity.pos.load();
        let delta = (holder_pos - mob_pos).normalize() * (distance - 2.0).max(0.0);
        let target = mob_pos + delta;
        self.get_mob_entity()
            .navigator
            .lock()
            .unwrap()
            .set_progress_if_changed(NavigatorGoal::new(
                mob_pos,
                target,
                f64::from(self.get_follow_leash_speed()),
            ));
    }

    /// Vanilla `Mob.leashTooFarBehaviour` drops the leash and disables MOVE goals
    /// (`Mob.java:1287-1290`). The shared entity leash tick has already played the break sound
    /// at the holder, matching `Leashable.tickLeash` (`Leashable.java:160-163`).
    fn leash_too_far_behavior(&self) -> EntityBaseFuture<'_, ()> {
        Box::pin(async move {
            let entity = self.get_entity();
            entity.unleash().await;
            entity
                .world
                .load()
                .drop_stack(
                    &entity.block_pos.load(),
                    ItemStack::new(1, &pumpkin_data::item::Item::LEAD),
                )
                .await;
            self.get_mob_entity()
                .goals_selector
                .lock()
                .unwrap()
                .disable_control(Controls::MOVE);
        })
    }

    /// Vanilla `PathfinderMob.shouldStayCloseToLeashHolder`.
    fn should_follow_leash(&self) -> bool {
        true
    }

    /// Vanilla `PathfinderMob.followLeashSpeed`.
    fn get_follow_leash_speed(&self) -> f32 {
        1.0
    }

    /// Per-mob tick hook called after selectors and navigation, before movement controls.
    /// This is vanilla `Mob.customServerAiStep`'s position in `Mob.serverAiStep`.
    fn mob_tick<'a>(&'a self, _caller: &'a Arc<dyn EntityBase>) -> EntityBaseFuture<'a, ()> {
        Box::pin(async {})
    }

    /// Runs immediately before the vanilla mob goal selectors.
    fn pre_ai_tick(&self) -> EntityBaseFuture<'_, ()> {
        Box::pin(async {})
    }

    /// Vanilla `LivingEntity.updateSwimming`, called from the base tick before mob AI runs.
    fn update_swimming(&self) -> EntityBaseFuture<'_, ()> {
        Box::pin(async {})
    }

    /// Vanilla `JumpControl.tick`; specialized mobs may preserve or translate the published
    /// state, as `RabbitJumpControl` does.
    fn jump_control_tick(&self, jump_requested: bool) {
        self.get_mob_entity()
            .living_entity
            .jumping
            .store(jump_requested, Relaxed);
    }

    /// Hook for mobs whose vanilla `travel` implementation replaces the generic living-mob
    /// movement path (for example `Squid.travel`, which moves with its current movement vector).
    /// Returning `true` means the hook has already moved the entity for this tick.
    fn custom_travel<'a>(&'a self, _caller: &'a Arc<dyn EntityBase>) -> EntityBaseFuture<'a, bool> {
        Box::pin(async { false })
    }

    /// Whether this mob's `customServerAiStep` should tick its Brain this tick. Goal-driven mobs
    /// have no Brain and retain the default; Happy Ghast overrides this for babies
    /// (`HappyGhast.java:400-409`).
    fn should_tick_brain(&self) -> bool {
        true
    }

    /// Vanilla custom-travel goals can publish a movement vector without taking over the
    /// navigator. The default is inert for ordinary mobs.
    fn set_movement_vector(&self, _movement: Vector3<f64>) {}

    #[must_use]
    fn get_movement_vector(&self) -> Option<Vector3<f64>> {
        None
    }

    fn post_tick(&self) -> EntityBaseFuture<'_, ()> {
        Box::pin(async {})
    }

    /// Called before damage is applied. Return `false` to cancel the damage entirely.
    /// Used by endermen to dodge projectiles via teleportation.
    fn pre_damage<'a>(
        &'a self,
        _damage_type: DamageType,
        _source: Option<&'a dyn EntityBase>,
    ) -> EntityBaseFuture<'a, bool> {
        Box::pin(async { true })
    }

    /// Whether the sensing, target/goal selectors and navigation must be skipped this tick,
    /// while `mob_tick` still runs.
    ///
    /// This is the shape Pumpkin has for vanilla's *exclusive* brain activities: an activity
    /// like `Activity.EMERGE` sits at the top of the ladder (`WardenAi.java:79`) and, while its
    /// gating memory is present, is the only non-core activity running, so nothing can pick a
    /// walk target - `Emerging` itself demands `WALK_TARGET` absent
    /// (`ai/behavior/warden/Emerging.java`, its `Behavior` memory map). Returning `true`
    /// reproduces that freeze without `set_no_ai`, which is player-visible state and persists
    /// to NBT.
    fn suppress_ai_goals(&self) -> bool {
        false
    }

    fn on_damage<'a>(
        &'a self,
        _damage_type: DamageType,
        _source: Option<&'a dyn EntityBase>,
    ) -> EntityBaseFuture<'a, ()> {
        Box::pin(async {})
    }

    /// Called on the killed mob once its death is confirmed, with `cause` as the
    /// killer entity (mirrors `LivingEntity::on_death`'s `cause` parameter). Used by
    /// villagers to notify nearby witnesses of a murder.
    fn on_mob_death<'a>(&'a self, _cause: Option<&'a dyn EntityBase>) -> EntityBaseFuture<'a, ()> {
        Box::pin(async {})
    }

    fn on_eating_grass(&self) -> EntityBaseFuture<'_, ()> {
        Box::pin(async {})
    }

    fn modify_incoming_damage(&self, amount: f32, _damage_type: DamageType) -> f32 {
        amount
    }

    fn can_attack_with_owner(&self, _target: &dyn EntityBase, _owner: &dyn EntityBase) -> bool {
        true
    }

    /// Vanilla `Mob.canAttack`, consulted by `TargetingConditions.test`'s combat branch
    /// (`TargetingConditions.java:78`). Defaults to `true`; species with a blanket "never
    /// target this" rule (Iron Golem's player-created and creeper exclusions) override it.
    ///
    /// Consulted at initial acquisition by the active, hostile, witch, ghast, and non-tame target
    /// goals, and at continuation by `RevengeGoal` and `TrackTargetGoal`.
    fn can_attack(&self, _target: &Entity) -> bool {
        true
    }

    /// Exposes this mob's `NeutralMob`-equivalent grudge state (Wolf, `ZombifiedPiglin`),
    /// if it has one, for shared goals (`ActiveTargetGoal`'s angry-at-player predicate).
    fn persistent_anger(&self) -> Option<&crate::entity::persistent_anger::PersistentAnger> {
        None
    }

    fn get_mob_gravity(&self) -> f64 {
        self.get_mob_entity().living_entity.get_gravity()
    }

    fn get_mob_y_velocity_drag(&self) -> Option<f64> {
        None
    }

    /// Vanilla `Entity.isPushedByFluid`: whether currents apply push velocity to this mob.
    /// Turtle overrides this to `false`.
    fn mob_is_pushed_by_fluids(&self) -> bool {
        true
    }

    /// Vanilla `Entity.isInvulnerableToPiercingWeapon` defaults to `isInvulnerable`; mob
    /// families with a piercing-specific exception override this hook.
    fn mob_is_invulnerable_to_piercing_weapon(&self) -> bool {
        self.get_entity().invulnerable.load(Relaxed)
    }

    fn as_ageable(&self) -> Option<&dyn crate::entity::ageable::AgeableMob> {
        None
    }

    fn as_animal(&self) -> Option<&dyn crate::entity::passive::animal::Animal> {
        None
    }

    fn as_tamable(&self) -> Option<&dyn crate::entity::passive::tamable::TamableAnimal> {
        None
    }

    /// Set or clear the mob's target. Override to add side effects when targeting changes.
    fn set_mob_target(&self, target: Option<Arc<dyn EntityBase>>) -> EntityBaseFuture<'_, ()> {
        Box::pin(async move {
            let target_id = target.as_ref().map(|t| t.get_entity().entity_id);
            let mob = self.get_mob_entity();
            let mut event =
                crate::plugin::api::events::entity::entity_target::EntityTargetEvent::new(
                    mob.living_entity.entity.entity_id,
                    target_id,
                );
            if let Some(server) = mob.living_entity.entity.world.load().server.upgrade() {
                server.plugin_manager.fire(&server, &mut event).await;
            }
            if event.cancelled {
                return;
            }
            let mut mob_target = mob.target.lock().await;
            *mob_target = target;
        })
    }

    fn mob_interact<'a>(
        &'a self,
        player: &'a Arc<Player>,
        item_stack: &'a mut ItemStack,
    ) -> EntityBaseFuture<'a, bool> {
        Box::pin(async move {
            self.get_mob_entity()
                .mob_interact(player, item_stack, self.can_be_leashed())
                .await
        })
    }

    /// Vanilla `Mob.checkAndHandleImportantInteractions` handles matching spawn eggs before
    /// dispatching to each mob's regular interaction method.
    fn spawn_egg_interact<'a>(
        &'a self,
        player: &'a Arc<Player>,
        item_stack: &'a mut ItemStack,
    ) -> EntityBaseFuture<'a, bool>
    where
        Self: Sized,
    {
        Box::pin(async move {
            let Some(egg_type) = entity_from_egg(item_stack.item.id) else {
                return false;
            };
            let entity = self.get_entity();
            if egg_type.id != entity.entity_type.id || !spawns_offspring_from_egg(egg_type) {
                return false;
            }

            let world = entity.world.load();
            // Vanilla passes the clicked mob as its own breeding partner. `create_offspring`
            // returns `None` for species whose breeding drops an item instead of a baby (the
            // sniffer), but `Sniffer.getBreedOffspring` still returns a sniffer, so an egg used
            // on one spawns a baby the same as every other ageable mob.
            let offspring = self.create_offspring(self, &world).await.or_else(|| {
                Some(from_type(
                    egg_type,
                    entity.pos.load(),
                    &world,
                    Uuid::new_v4(),
                ))
            });
            let Some(offspring) = offspring else {
                return false;
            };

            offspring.get_entity().set_age(-24000);
            apply_entity_variant(item_stack, offspring.as_ref());
            world.spawn_entity(offspring.clone()).await;
            self.on_offspring_spawned_from_egg(player, offspring.as_ref());
            item_stack.decrement_unless_creative(player.gamemode.load(), 1);
            emit_game_event(
                &world,
                pumpkin_data::game_event::GameEvent::EntityInteract,
                entity.pos.load(),
                GameEventContext::of_entity(player.clone() as Arc<dyn EntityBase>),
            )
            .await;
            true
        })
    }

    /// Vanilla `Mob.onOffspringSpawnedFromEgg` hook.
    fn on_offspring_spawned_from_egg(&self, _player: &Arc<Player>, _offspring: &dyn EntityBase) {}

    /// Vanilla `Mob.canBeLeashed`: whether a lead can be attached to this mob at all.
    /// Defaults to `true`; species that are never leashable (e.g. Turtle) override this.
    fn can_be_leashed(&self) -> bool {
        true
    }

    fn tame<'a>(&'a self, player: &'a Arc<Player>) -> EntityBaseFuture<'a, ()> {
        Box::pin(async move {
            let mob = self.get_mob_entity();
            let mut event = crate::plugin::api::events::entity::entity_tame::EntityTameEvent::new(
                mob.living_entity.entity.entity_id,
                player.clone(),
            );
            if let Some(server) = mob.living_entity.entity.world.load().server.upgrade() {
                server.plugin_manager.fire(&server, &mut event).await;
            }
        })
    }

    fn breed(&self, father_id: i32, mother_id: i32, child_id: i32) -> EntityBaseFuture<'_, ()> {
        Box::pin(async move {
            let mob = self.get_mob_entity();
            let mut event = crate::plugin::api::events::entity::entity_breed::EntityBreedEvent::new(
                father_id, mother_id, child_id,
            );
            if let Some(server) = mob.living_entity.entity.world.load().server.upgrade() {
                server.plugin_manager.fire(&server, &mut event).await;
            }
        })
    }

    fn dye<'a>(
        &'a self,
        color: crate::plugin::api::events::entity::entity_dye::DyeColor,
        player: Option<&'a Arc<Player>>,
    ) -> EntityBaseFuture<'a, ()> {
        Box::pin(async move {
            let mob = self.get_mob_entity();
            let mut event = crate::plugin::api::events::entity::entity_dye::EntityDyeEvent::new(
                mob.living_entity.entity.entity_id,
                color,
                player.cloned(),
            );
            if let Some(server) = mob.living_entity.entity.world.load().server.upgrade() {
                server.plugin_manager.fire(&server, &mut event).await;
            }
        })
    }

    fn enter_love_mode(
        &self,
        human_entity_id: Option<i32>,
        ticks_in_love: i32,
    ) -> EntityBaseFuture<'_, ()> {
        Box::pin(async move {
            let mob = self.get_mob_entity();
            let mut event = crate::plugin::api::events::entity::entity_enter_love_mode::EntityEnterLoveModeEvent::new(
                mob.living_entity.entity.entity_id,
                human_entity_id,
                ticks_in_love,
            );
            if let Some(server) = mob.living_entity.entity.world.load().server.upgrade() {
                server.plugin_manager.fire(&server, &mut event).await;
            }
        })
    }

    fn transform(&self, new_entity_id: i32, transform_reason: String) -> EntityBaseFuture<'_, ()> {
        Box::pin(async move {
            let mob = self.get_mob_entity();
            let mut event =
                crate::plugin::api::events::entity::entity_transform::EntityTransformEvent::new(
                    mob.living_entity.entity.entity_id,
                    new_entity_id,
                    transform_reason,
                );
            if let Some(server) = mob.living_entity.entity.world.load().server.upgrade() {
                server.plugin_manager.fire(&server, &mut event).await;
            }
        })
    }

    fn break_door(&self, block_pos: BlockPos) -> EntityBaseFuture<'_, ()> {
        Box::pin(async move {
            let mob = self.get_mob_entity();
            let mut event =
                crate::plugin::api::events::entity::entity_break_door::EntityBreakDoorEvent::new(
                    mob.living_entity.entity.entity_id,
                    block_pos,
                );
            if let Some(server) = mob.living_entity.entity.world.load().server.upgrade() {
                server.plugin_manager.fire(&server, &mut event).await;
            }
        })
    }

    fn enter_block(&self, block_pos: BlockPos) -> EntityBaseFuture<'_, ()> {
        Box::pin(async move {
            let mob = self.get_mob_entity();
            let mut event =
                crate::plugin::api::events::entity::entity_enter_block::EntityEnterBlockEvent::new(
                    mob.living_entity.entity.entity_id,
                    block_pos,
                );
            if let Some(server) = mob.living_entity.entity.world.load().server.upgrade() {
                server.plugin_manager.fire(&server, &mut event).await;
            }
        })
    }

    fn interact(&self, block_pos: BlockPos) -> EntityBaseFuture<'_, ()> {
        Box::pin(async move {
            let mob = self.get_mob_entity();
            let mut event =
                crate::plugin::api::events::entity::entity_interact::EntityInteractEvent::new(
                    mob.living_entity.entity.entity_id,
                    block_pos,
                );
            if let Some(server) = mob.living_entity.entity.world.load().server.upgrade() {
                server.plugin_manager.fire(&server, &mut event).await;
            }
        })
    }

    fn place_block(&self, block_pos: BlockPos, block_name: String) -> EntityBaseFuture<'_, ()> {
        Box::pin(async move {
            let mob = self.get_mob_entity();
            let mut event = crate::plugin::api::events::entity::entity_place::EntityPlaceEvent::new(
                mob.living_entity.entity.entity_id,
                block_pos,
                block_name,
            );
            if let Some(server) = mob.living_entity.entity.world.load().server.upgrade() {
                server.plugin_manager.fire(&server, &mut event).await;
            }
        })
    }

    fn mob_player_collision<'a>(&'a self, _player: &'a Arc<Player>) -> EntityBaseFuture<'a, ()> {
        Box::pin(async {})
    }

    /// Vanilla `Mob.wantsToPickUp` default: delegates to `canHoldItem`, which defaults to
    /// `true`. Whether picking up is ever attempted at all is gated separately by
    /// `can_pick_up_loot`.
    fn wants_to_pick_up_item(&self, _world: &World, _stack: &ItemStack) -> bool {
        true
    }

    /// Vanilla `Mob.getPickupReach` returns `ITEM_PICKUP_REACH`, `(1, 0, 1)`.
    // `Mob.java:104-105, 517-518`.
    fn get_pickup_reach(&self) -> (f64, f64, f64) {
        DEFAULT_ITEM_PICKUP_REACH
    }

    /// `net.minecraft.world.entity.npc.InventoryCarrier.getInventory().isEmpty()`, reduced to
    /// the single question `GoAndGiveItemsToTarget` asks (`GoAndGiveItemsToTarget.java:81`).
    /// `true` for every mob that is not an `InventoryCarrier`, which is all but the Allay
    /// today.
    fn carried_inventory_is_empty(&self) -> bool {
        true
    }

    /// `InventoryCarrier.getInventory().removeItem(0, 1)`
    /// (`GoAndGiveItemsToTarget.java:71`): take a single item out of the carried inventory,
    /// returning an empty stack when there is nothing to take.
    fn remove_one_carried_item(&self) -> ItemStack {
        ItemStack::EMPTY.clone()
    }

    /// Vanilla `Mob.canPickUpLoot`: whether this mob is allowed to pick up dropped items at
    /// all. Backed by the mob's `CanPickUpLoot` tracked-data flag, which defaults to `false`
    /// and is set at spawn time for a few mob types (see `equipment.rs`).
    fn can_pick_up_loot(&self) -> bool {
        self.get_mob_entity().can_pick_up_loot()
    }

    /// Vanilla `Mob.getEquipmentSlotForItem`. Items with an `equippable` component use its
    /// declared slot; ordinary items are candidate main-hand equipment.
    fn get_equipment_slot_for_item(&self, stack: &ItemStack) -> EquipmentSlot {
        stack
            .get_data_component::<EquippableImpl>()
            .map_or(EquipmentSlot::MAIN_HAND, |equippable| {
                equippable.slot.clone()
            })
    }

    /// Vanilla `Mob.isEquippableInSlot`. A concrete allow-list is checked directly; an
    /// unresolved data-tag is conservatively refused rather than equipping restricted animal
    /// armor or saddles onto arbitrary mobs.
    fn is_equippable_in_slot(&self, stack: &ItemStack, slot: &EquipmentSlot) -> bool {
        stack
            .get_data_component::<EquippableImpl>()
            .is_none_or(|equippable| {
                equippable.slot == slot
                    && equippable
                        .allowed_entities
                        .as_ref()
                        .is_none_or(|allowed| match allowed {
                            IDSet::IDs(entities) => entities.iter().any(|entity_type| {
                                entity_type.id == self.get_entity().entity_type.id
                            }),
                            IDSet::Tag(_) => false,
                        })
            })
    }

    /// Vanilla `Mob.canHoldItem`; specialized mobs narrow this through
    /// `wants_to_pick_up_item` before the generic equipment path runs.
    fn can_hold_item(&self, _stack: &ItemStack) -> bool {
        true
    }

    /// Vanilla `Mob.canReplaceCurrentItem`'s safe base case. Full armor/weapon attribute
    /// comparison needs the missing item-attribute evaluator; equal stacks still use the
    /// source-faithful tie-breaker below.
    fn can_replace_current_item(
        &self,
        new_stack: &ItemStack,
        current_stack: &ItemStack,
        _slot: &EquipmentSlot,
    ) -> bool {
        // `Mob.canReplaceCurrentItem` uses this equal-item tie-breaker (`Mob.java:605-613, 654-666`).
        current_stack.is_empty()
            || (new_stack.item.id == current_stack.item.id
                && can_replace_equal_item(new_stack, current_stack))
    }

    /// `Mob.setItemSlotAndDropWhenKilled` (`Mob.java:563-566`): update persistent equipment,
    /// mark the slot with `DropChances.withGuaranteedDrop` (`DropChances.java:28-29`), and
    /// publish the change before the ground item is decremented.
    fn set_item_slot_and_drop_when_killed(
        &self,
        slot: EquipmentSlot,
        stack: ItemStack,
    ) -> EntityBaseFuture<'_, ()> {
        Box::pin(async move {
            let living = &self.get_mob_entity().living_entity;
            living
                .entity_equipment
                .lock()
                .await
                .put(&slot, stack.clone());
            living
                .equipment_drop_chances
                .lock()
                .await
                .insert(slot.clone(), 2.0);
            living.send_equipment_changes(&[(slot, stack)]);
        })
    }

    /// Vanilla `Mob.equipItemIfPossible`, constrained to an empty target slot until the shared
    /// attribute comparison machinery is ported. The returned stack is exactly the amount that
    /// was equipped, so callers can shrink the live `ItemEntity` without inventing or losing
    /// items.
    fn equip_item_if_possible<'a>(
        &'a self,
        stack: &'a ItemStack,
    ) -> EntityBaseFuture<'a, ItemStack> {
        Box::pin(async move {
            let slot = self.get_equipment_slot_for_item(stack);
            if !self.is_equippable_in_slot(stack, &slot) || !self.can_hold_item(stack) {
                return ItemStack::EMPTY.clone();
            }

            let living = &self.get_mob_entity().living_entity;
            let current = living.entity_equipment.lock().await.get(&slot);
            if !self.can_replace_current_item(stack, &current, &slot) {
                return ItemStack::EMPTY.clone();
            }

            let count = match slot {
                EquipmentSlot::MainHand(_) | EquipmentSlot::OffHand(_) => stack.item_count,
                _ => 1,
            };
            let equipped = stack.copy_with_count(count);
            self.set_item_slot_and_drop_when_killed(slot, equipped.clone())
                .await;
            equipped
        })
    }

    /// Vanilla `Mob.onItemPickup`/`equipItemIfPossible`: called once a candidate item stack
    /// has passed `wants_to_pick_up_item`, to actually take it. Returns the number of items
    /// taken from the stack; the caller only shrinks/removes the `ItemEntity` by that count.
    fn on_item_pickup<'a>(&'a self, stack: &'a ItemStack) -> EntityBaseFuture<'a, u8> {
        Box::pin(async move { self.equip_item_if_possible(stack).await.item_count })
    }

    /// Vanilla `Mob.aiStep`'s pickup-loot loop: scans nearby dropped items within pickup
    /// reach and offers each one to `on_item_pickup` if it passes `wants_to_pick_up_item`
    /// and the item entity's own pickup-delay gate. Gated on `can_pick_up_loot` and the
    /// `mobGriefing` gamerule.
    fn mob_try_pick_up_items(&self) -> EntityBaseFuture<'_, ()> {
        Box::pin(async move {
            if !self.can_pick_up_loot() {
                return;
            }

            let mob_entity = self.get_mob_entity();
            let entity = &mob_entity.living_entity.entity;
            if !entity.is_alive() {
                return;
            }

            let world = entity.world.load();
            if !world.level_info.load().game_rules.mob_griefing {
                return;
            }

            // `Mob.java:468-475, 517-518`.
            let (reach_x, reach_y, reach_z) = self.get_pickup_reach();
            let reach = entity.bounding_box.load().expand(reach_x, reach_y, reach_z);
            for candidate in world.get_entities_at_box(&reach) {
                let Some(item_entity) = candidate.clone().get_item_entity() else {
                    continue;
                };

                if !item_entity.get_entity().is_alive() || item_entity.has_pickup_delay() {
                    continue;
                }

                let stack_snapshot = { item_entity.get_item_stack().lock().await.clone() };
                if stack_snapshot.is_empty() || !self.wants_to_pick_up_item(&world, &stack_snapshot)
                {
                    continue;
                }

                let taken = self
                    .on_item_pickup(&stack_snapshot)
                    .await
                    .min(stack_snapshot.item_count);
                if taken == 0 {
                    continue;
                }

                self.set_persistence_required();

                let is_empty = {
                    let mut stack = item_entity.get_item_stack().lock().await;
                    stack.decrement(taken);
                    stack.is_empty()
                };

                if is_empty {
                    item_entity.get_entity().remove().await;
                } else {
                    item_entity.init_data_tracker().await;
                }
            }
        })
    }

    fn get_owner_uuid(&self) -> Option<Uuid> {
        self.as_tamable()
            .and_then(crate::entity::passive::tamable::TamableAnimal::get_owner)
    }

    fn is_sitting(&self) -> bool {
        self.as_tamable()
            .is_some_and(crate::entity::passive::tamable::TamableAnimal::is_in_sitting_pose)
    }

    fn get_base_experience_reward(&self) -> u32 {
        self.get_entity().entity_type.experience_reward
    }

    fn mob_init_data_tracker(&self) -> EntityBaseFuture<'_, ()> {
        Box::pin(async move {
            let entity = self.get_entity();
            // PENDING-INDEX-FIX: this default sits on `Mob`, but `DATA_BABY_ID` at index 16
            // belongs to `AgeableMob`. Index 16 is a different field on mobs that are not
            // ageable -- `DATA_SWELL_DIR` on a creeper, for one -- so a non-ageable mob that
            // ever went to a negative age would publish that field instead. Nothing sets a
            // negative age outside the ageable path today, which is why this is latent rather
            // than live. The real fix is to move this send onto the `AgeableMob` trait; doing
            // that safely means auditing every type that relies on this default first.
            let is_baby = entity.age.load(std::sync::atomic::Ordering::Relaxed) < 0;
            if is_baby {
                entity.send_meta_data(
                    &[Metadata::new(tracked_data::ageable_mob::DATA_BABY_ID, true)],
                    None,
                );
            }
        })
    }

    fn mob_set_variant_name(&self, _name: &str) {}

    /// Species-specific gate used by the generic animal breeding goal.
    fn can_breed(&self) -> bool {
        true
    }

    /// Species-specific partner gate used by the generic animal breeding goal.
    fn can_breed_with(&self, _mate: &dyn EntityBase) -> bool {
        true
    }

    /// Vanilla `Animal.getBreedOffspring`: builds the baby entity to spawn after a successful
    /// breed with `mate`. Override to customize the offspring (e.g. inherited color/variant)
    /// before it enters the world. Returning `None` skips spawning a baby entity entirely,
    /// matching `Sniffer`'s override, which drops a `SNIFFER_EGG` item instead.
    fn create_offspring<'a>(
        &'a self,
        _mate: &'a dyn EntityBase,
        world: &'a Arc<World>,
    ) -> EntityBaseFuture<'a, Option<Arc<dyn EntityBase>>> {
        Box::pin(async move {
            let entity = self.get_entity();
            Some(crate::entity::r#type::from_type(
                entity.entity_type,
                entity.pos.load(),
                world,
                Uuid::new_v4(),
            ))
        })
    }

    /// Spawns the prepared vanilla breeding result after `Animal.finalizeSpawnChildFromBreeding`
    /// awards experience. Concrete animals can override this when breeding produces a non-mob
    /// result, such as Sniffer's egg item.
    fn spawn_breeding_result<'a>(
        &'a self,
        offspring: Option<Arc<dyn EntityBase>>,
        world: &'a Arc<World>,
        _parent_pos: Vector3<f64>,
    ) -> EntityBaseFuture<'a, ()> {
        Box::pin(async move {
            if let Some(baby) = offspring {
                world.spawn_entity(baby).await;
            }
        })
    }

    /// Called once a breed has been claimed (both parents' love ticks reset) and offspring is
    /// about to be created. Override for side effects vanilla ties to a specific `BreedGoal`
    /// subclass rather than the generic breed path, e.g. `Turtle.TurtleBreedGoal.breed` setting
    /// `hasEgg = true` (`Turtle.java:300-326`).
    fn on_bred(&self, _mate: &dyn EntityBase) {}

    /// Vanilla sends entity event 18 after a successful generic breed. Custom breeding goals
    /// that replace that event can disable it explicitly.
    fn sends_breed_event(&self) -> bool {
        true
    }

    fn get_sheep(&self) -> Option<&crate::entity::passive::sheep::SheepEntity> {
        None
    }

    fn get_bee(&self) -> Option<&crate::entity::passive::bee::BeeEntity> {
        None
    }

    fn mob_on_lightning_strike<'a>(
        &'a self,
        caller: &'a dyn EntityBase,
        lightning: &'a crate::entity::lightning::LightningBoltEntity,
    ) -> EntityBaseFuture<'a, ()> {
        Box::pin(async move {
            self.get_mob_entity()
                .living_entity
                .on_lightning_strike(caller, lightning)
                .await;
        })
    }
}

pub(crate) struct MutexTakeGuard<'a, T> {
    mutex: &'a std::sync::Mutex<T>,
    value: Option<T>,
}

impl<'a, T: Default> MutexTakeGuard<'a, T> {
    fn new(mutex: &'a std::sync::Mutex<T>) -> Self {
        let value = std::mem::take(&mut *mutex.lock().unwrap());
        Self {
            mutex,
            value: Some(value),
        }
    }
}

impl<T> Deref for MutexTakeGuard<'_, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        self.value.as_ref().unwrap()
    }
}

impl<T> DerefMut for MutexTakeGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.value.as_mut().unwrap()
    }
}

impl<T> Drop for MutexTakeGuard<'_, T> {
    fn drop(&mut self) {
        if let Some(value) = self.value.take() {
            *self.mutex.lock().unwrap() = value;
        }
    }
}

/// Runs `Mob.serverAiStep` at the `LivingEntity` AI/movement boundary.
///
/// Vanilla reaches this after `LivingEntity.aiStep` has prepared input and before jump,
/// travel, and collision effects. Keeping the selector/navigation/controller phase here
/// lets the generic living tick place it correctly for every mob implementation.
pub(crate) fn tick_mob_ai<'a>(
    mob: &'a dyn Mob,
    caller: &'a Arc<dyn EntityBase>,
) -> EntityBaseFuture<'a, ()> {
    Box::pin(async move {
        let mob_entity = mob.get_mob_entity();
        if mob_entity.is_no_ai() {
            mob_entity.living_entity.jumping.store(false, Relaxed);
            mob_entity.jump_requested.store(false, Relaxed);
            return;
        }

        // An exclusive brain activity (see `Mob::suppress_ai_goals`) still ticks the brain, so
        // `mob_tick` - which is where such a state counts itself down - runs, but nothing that
        // could produce movement does.
        if mob.suppress_ai_goals() {
            mob_entity.living_entity.jumping.store(false, Relaxed);
            mob_entity.jump_requested.store(false, Relaxed);
            mob.mob_tick(caller).await;
            return;
        }

        // Mob.getNavigation delegates to a controlled Mob vehicle. Resolve that once at the
        // async AI boundary so the synchronous MoveControl tick can use the same evaluator.
        let mut strafe_navigation_kind = mob_entity.navigator.lock().unwrap().navigation_kind();
        let vehicle = mob_entity.living_entity.entity.vehicle.lock().await.clone();
        if let Some(vehicle) = vehicle
            && let Some(vehicle_mob) = vehicle.get_mob()
            && !vehicle_mob.get_mob_entity().is_no_ai()
        {
            let first_passenger = vehicle
                .get_entity()
                .passengers
                .lock()
                .await
                .first()
                .cloned();
            if first_passenger.is_some_and(|passenger| {
                passenger.get_entity().entity_id == mob_entity.living_entity.entity.entity_id
                    && !passenger
                        .get_entity()
                        .entity_type
                        .has_tag(&tag::EntityType::MINECRAFT_NON_CONTROLLING_RIDER)
            }) {
                strafe_navigation_kind = vehicle_mob
                    .get_mob_entity()
                    .navigator
                    .lock()
                    .unwrap()
                    .navigation_kind();
            }
        }
        mob_entity.set_strafe_navigation_kind(strafe_navigation_kind);

        // `Mob.getMaxFallDistance` is consumed by `WalkNodeEvaluator` while the navigation path
        // is built (`Mob.java:834-846`; `WalkNodeEvaluator.java:352`). Refresh it before goals
        // can request a path so target and difficulty changes affect this tick's navigation.
        let max_fall_distance = mob_entity.max_fall_distance().await;
        mob_entity
            .navigator
            .lock()
            .unwrap()
            .set_max_fall_distance(max_fall_distance as f32);

        mob.pre_ai_tick().await;

        mob_entity.sensing.lock().unwrap().tick();

        let mut target_selector = MutexTakeGuard::new(&mob_entity.target_selector);
        let mut goals_selector = MutexTakeGuard::new(&mob_entity.goals_selector);

        let tick_count = mob_entity.tick_count.load(Relaxed);
        let run_all_goals = tick_count <= 1
            || (tick_count.wrapping_add(mob_entity.living_entity.entity.entity_id)) % 2 == 0;
        if run_all_goals {
            target_selector.tick(mob).await;
            goals_selector.tick(mob).await;
        } else {
            target_selector.tick_goals(mob, false).await;
            goals_selector.tick_goals(mob, false).await;
        }

        drop(goals_selector);
        drop(target_selector);

        let mut navigator = MutexTakeGuard::new(&mob_entity.navigator);
        navigator.tick(&mob_entity.living_entity).await;
        let navigation_target = navigator.next_movement_target();
        drop(navigator);

        // Vanilla transfers the result of navigation before customServerAiStep and
        // before the movement/look controls tick. This also lets a custom AI hook
        // replace the wanted position without a stale navigation result being applied
        // after the hook returns.
        if let Some((target, speed)) = navigation_target {
            mob_entity
                .move_control
                .lock()
                .unwrap()
                .set_wanted_position(target.x, target.y, target.z, speed);
        }

        if mob.should_tick_brain()
            && let Some(brain) = &mob_entity.brain
        {
            let game_time = mob_entity
                .living_entity
                .entity
                .world
                .load_full()
                .get_world_age()
                .await;
            brain.tick(mob, game_time).await;
        }

        mob.mob_tick(caller).await;

        let mut move_control = mob_entity.move_control.lock().unwrap();
        move_control.tick(mob);

        {
            let mut look_control = mob_entity.look_control.lock().unwrap();
            look_control.tick(mob);
        };

        // Vanilla runs JumpControl after MoveControl and LookControl. Publish the request
        // only after both controls have completed so the following LivingEntity movement
        // phase sees exactly one tick's decision.
        let jump_requested = mob_entity.jump_requested.swap(false, Relaxed);
        mob.jump_control_tick(jump_requested);
    })
}

impl<T: Mob + Send + 'static> EntityBase for T {
    fn notify_leash_holder(&self, entity: &dyn EntityBase) {
        Mob::notify_leash_holder(self, entity);
    }

    fn get_vehicle_attachment_point(&self, vehicle: &Entity) -> Option<Vector3<f64>> {
        Mob::get_vehicle_attachment_point(self, vehicle)
    }

    fn get_mob(&self) -> Option<&dyn Mob> {
        Some(self)
    }

    fn can_be_collided_with(&self) -> bool {
        Mob::can_be_collided_with(self)
    }

    /// `LivingEntity.isPushable` (`LivingEntity.java:3365-3367`): alive, not spectating and
    /// not on a climbable block.
    ///
    /// Without this the blanket impl fell through to `EntityBase`'s `false`, so no mob was
    /// pushable by anything - mobs never displaced each other, and nothing could shove them.
    /// `LivingEntity` carries its own override but mobs never reach it through this impl.
    fn is_pushable(&self) -> bool {
        let living = &self.get_mob_entity().living_entity;
        living.health.load() > 0.0
            && !living.dead.load(std::sync::atomic::Ordering::Relaxed)
            && !living.climbing.load(std::sync::atomic::Ordering::Relaxed)
    }

    fn check_despawn(&self) -> EntityBaseFuture<'_, ()> {
        Mob::check_despawn(self)
    }

    fn on_lightning_strike<'a>(
        &'a self,
        caller: &'a dyn EntityBase,
        lightning: &'a crate::entity::lightning::LightningBoltEntity,
    ) -> EntityBaseFuture<'a, ()> {
        Box::pin(async move {
            self.mob_on_lightning_strike(caller, lightning).await;
        })
    }

    fn get_item_steerable(&self) -> Option<&dyn crate::entity::item_steerable::ItemSteerable> {
        Mob::get_item_steerable(self)
    }

    fn calculate_fall_damage(&self, fall_distance: f64, damage_modifier: f32) -> i32 {
        self.mob_calculate_fall_damage(fall_distance, damage_modifier)
    }

    fn is_sensitive_to_water(&self) -> bool {
        self.mob_is_sensitive_to_water()
    }

    fn can_use_portal(&self) -> bool {
        Mob::mob_can_use_portal(self)
    }

    fn init_data_tracker(&self) -> EntityBaseFuture<'_, ()> {
        Box::pin(async move {
            self.mob_init_data_tracker().await;
            let world = self.get_mob_entity().living_entity.entity.world.load();
            // `Mob.finalizeSpawn` work, which vanilla runs only on a genuine spawn. A mob read
            // back out of chunk NBT already carries the equipment it was saved with
            // (`LivingEntity`'s HandItems/ArmorItems), so re-rolling it here would hand every
            // reloaded zombie a fresh random set.
            if !self.is_restored_from_nbt() {
                crate::entity::mob::equipment::equip_mob_on_spawn(self as &dyn EntityBase, &world)
                    .await;
            }

            let entity_name = self.get_entity().entity_type.resource_name;
            if let Some(def) = crate::entity::mob::equipment::EQUIPMENT_REGISTRY.get(entity_name)
                && def.can_pick_up_loot
            {
                let difficulty = crate::entity::mob::equipment::RegionalDifficulty::at(
                    &world,
                    self.get_entity().pos.load(),
                );
                let pickup_chance = 0.55 * difficulty.special_multiplier;
                // Deliberately still rolled for a reloaded mob: vanilla persists this as
                // `CanPickUpLoot` (`Mob.java:370`, read back at `Mob.java:396`), but nothing
                // here writes that tag yet, so suppressing the roll would silently turn every
                // reloaded mob's loot pickup off instead of restoring what it had. Move this
                // under the guard above once `CanPickUpLoot` is persisted.
                self.get_mob_entity()
                    .set_can_pick_up_loot(rand::random::<f32>() < pickup_chance);
            }
        })
    }

    fn set_variant_name(&self, name: &str) {
        self.mob_set_variant_name(name);
    }

    #[allow(clippy::too_many_lines)]
    fn tick<'a>(
        &'a self,
        caller: &'a Arc<dyn EntityBase>,
        server: &'a Server,
    ) -> EntityBaseFuture<'a, ()> {
        Box::pin(async move {
            let mob_entity = self.get_mob_entity();
            mob_entity.sync_no_ai_flag();
            mob_entity.tick_count.fetch_add(1, Relaxed);
            if !mob_entity.is_no_ai()
                && !mob_entity.living_entity.dead.load(Relaxed)
                && mob_entity.living_entity.health.load() > 0.0
                && !mob_entity.living_entity.entity.is_removed()
            {
                mob_entity.no_action_time.fetch_add(1, Relaxed);
                if uses_monster_no_action_time(mob_entity.living_entity.entity.entity_type) {
                    let world = mob_entity.living_entity.entity.world.load();
                    if self.light_level_dependent_magic_value(&world) > 0.5 {
                        // `Monster.updateNoActionTime` adds two more ticks in bright light.
                        mob_entity.no_action_time.fetch_add(2, Relaxed);
                    }
                }
            }
            let entity = &mob_entity.living_entity.entity;
            if mob_entity.leash_home_active.load(Relaxed)
                && entity.leashed_to.lock().await.is_none()
            {
                self.on_leash_removed();
            }
            if let Some((holder_pos, distance)) = entity.tick_leash(self).await {
                // `Leashable.tickLeash` (`Leashable.java:155-160`) re-runs
                // `whenLeashedTo` on every leashed tick before the snap/elastic/close-range
                // dispatch; `PathfinderMob.whenLeashedTo` retargets the home to the holder.
                self.when_leashed_to(BlockPos::floored_v(holder_pos));
                if distance > entity.leash_snap_distance() {
                    self.leash_too_far_behavior().await;
                    self.on_leash_removed();
                } else {
                    self.close_range_leash_behavior(holder_pos, distance);
                    self.on_elastic_leash_pull();
                }
            }

            if mob_entity.breeding_cooldown.load(Relaxed) > 0 {
                mob_entity.breeding_cooldown.fetch_sub(1, Relaxed);
            }

            // Vanilla `Animal.customServerAiStep` (`Animal.java:59-65`) and `Animal.aiStep`
            // (`Animal.java:70-72`) both clear the in-love state whenever the age is not exactly
            // 0, so a baby fed to hearts, or an adult still inside its post-breed cooldown,
            // cannot stay in love. Only animals ever set `love_ticks`, so the check is a no-op
            // for every other mob.
            if mob_entity.living_entity.entity.age.load(Relaxed) != 0 {
                mob_entity.reset_love_ticks();
            }

            if mob_entity.love_ticks.load(Relaxed) > 0 {
                let ticks = mob_entity.love_ticks.fetch_sub(1, Relaxed);
                if ticks % 10 == 0 {
                    let entity = &mob_entity.living_entity.entity;
                    let pos = entity.pos.load();
                    let world = entity.world.load();
                    world.spawn_particle(
                        pos + Vector3::new(0.0, f64::from(entity.height()) + 0.5, 0.0),
                        Vector3::new(0.5, 0.5, 0.5),
                        1.0,
                        1,
                        pumpkin_data::particle::Particle::Heart,
                    );
                }
            }

            mob_entity.living_entity.tick(caller, server).await;
            self.tick_ambient_sound();
            self.tick_sun_burn().await;
            self.mob_try_pick_up_items().await;
            mob_entity.reset_schooling_if_isolated(self.get_random().random_range(0..200));
            self.post_tick().await;

            if mob_entity.tick_count.load(Relaxed) % 5 == 0 {
                self.update_control_flags().await;
            }

            // --- Packet logic remains the same ---
            let entity = &mob_entity.living_entity.entity;
            let yaw = (entity.yaw.load() * 256.0 / 360.0).rem_euclid(256.0) as u8;
            let pitch = (entity.pitch.load() * 256.0 / 360.0).rem_euclid(256.0) as u8;
            let head_yaw = (entity.head_yaw.load() * 256.0 / 360.0).rem_euclid(256.0) as u8;

            let last_yaw = mob_entity.last_sent_yaw.load(Relaxed);
            let last_pitch = mob_entity.last_sent_pitch.load(Relaxed);
            let last_head_yaw = mob_entity.last_sent_head_yaw.load(Relaxed);

            let chunk_pos = entity.chunk_pos.load();
            if yaw.abs_diff(last_yaw) >= 1 || pitch.abs_diff(last_pitch) >= 1 {
                let world = entity.world.load();
                world.broadcast_to_chunk(
                    chunk_pos,
                    &CUpdateEntityRot::new(
                        entity.entity_id.into(),
                        yaw,
                        pitch,
                        entity.on_ground.load(Relaxed),
                    ),
                );
                mob_entity.last_sent_yaw.store(yaw, Relaxed);
                mob_entity.last_sent_pitch.store(pitch, Relaxed);
            }

            if head_yaw.abs_diff(last_head_yaw) >= 1 {
                let world = entity.world.load();

                world.broadcast_to_chunk(
                    chunk_pos,
                    &CHeadRot::new(entity.entity_id.into(), head_yaw),
                );
                mob_entity.last_sent_head_yaw.store(head_yaw, Relaxed);
            }
        })
    }

    fn is_collidable(&self, _entity: Option<Box<dyn EntityBase>>) -> bool {
        true
    }

    fn can_hit(&self) -> bool {
        true
    }

    fn damage_with_context<'a>(
        &'a self,
        caller: &'a dyn EntityBase,
        amount: f32,
        damage_type: DamageType,
        position: Option<Vector3<f64>>,
        source: Option<&'a dyn EntityBase>,
        cause: Option<&'a dyn EntityBase>,
    ) -> EntityBaseFuture<'a, bool> {
        Box::pin(async move {
            // pre_damage hook: allows mobs to dodge/cancel damage (e.g. enderman projectile dodge)
            if !self.pre_damage(damage_type, source).await {
                return false;
            }
            // Mob-specific damage modifier (e.g. shulker armor when closed).
            let amount = self.modify_incoming_damage(amount, damage_type);
            let health = self.get_mob_entity().living_entity.health.load();
            let (amount, rescue_lethal) = self.mob_pre_apply_damage(health, amount).await;
            let damaged = self
                .get_mob_entity()
                .living_entity
                .damage_with_context(caller, amount, damage_type, position, source, cause)
                .await;
            if damaged {
                // `Animal.actuallyHurt` (`Animal.java:87-90`) clears love mode after damage is
                // accepted. Only animals currently use `love_ticks`, so keeping this in the
                // shared successful-mob-damage path applies the hook to every Animal implementor
                // without duplicating damage overrides across the species.
                self.get_mob_entity().reset_love_ticks();
                self.on_damage(damage_type, source).await;
                if rescue_lethal {
                    self.mob_on_lethal_rescue().await;
                }
            }
            damaged
        })
    }

    fn interact<'a>(
        &'a self,
        player: &'a Arc<Player>,
        item_stack: &'a mut ItemStack,
    ) -> EntityBaseFuture<'a, bool> {
        Box::pin(async move {
            // `Wolf.canShearEquipment` (`Wolf.java:447-450`) overrides the base
            // `!isVehicle()` check to also require ownership; this codebase has no
            // per-species override point for the shared shear path, so check it here.
            let wolf_shear_allowed = (self as &dyn std::any::Any)
                .downcast_ref::<WolfEntity>()
                .is_none_or(|wolf| wolf.mob_entity.owner.load() == Some(player.gameprofile.id));
            if item_stack.is_shears()
                && !player.get_entity().is_sneaking()
                && wolf_shear_allowed
                && self
                    .get_mob_entity()
                    .attempt_to_shear_equipment(player)
                    .await
            {
                return true;
            }
            if self.spawn_egg_interact(player, item_stack).await {
                return true;
            }
            self.mob_interact(player, item_stack).await
        })
    }

    fn on_player_collision<'a>(&'a self, player: &'a Arc<Player>) -> EntityBaseFuture<'a, ()> {
        Box::pin(async move { self.mob_player_collision(player).await })
    }

    fn get_entity(&self) -> &Entity {
        &self.get_mob_entity().living_entity.entity
    }

    fn get_living_entity(&self) -> Option<&LivingEntity> {
        Some(&self.get_mob_entity().living_entity)
    }

    fn is_pickable(&self) -> bool {
        Mob::is_pickable(self)
    }

    fn cast_any(&self) -> &dyn std::any::Any {
        self
    }

    fn is_in_love(&self) -> bool {
        self.get_mob_entity().is_in_love()
    }

    fn is_breeding_ready(&self) -> bool {
        self.get_mob_entity().is_breeding_ready()
    }

    fn reset_love(&self) {
        self.get_mob_entity().reset_love_ticks();
    }

    fn try_claim_love(&self) -> bool {
        self.get_mob_entity().try_claim_love()
    }

    fn set_breeding_cooldown(&self, ticks: i32) {
        self.get_mob_entity()
            .breeding_cooldown
            .store(ticks, Relaxed);
    }

    fn is_panicking(&self) -> bool {
        // `PathfinderMob.isPanicking` (`PathfinderMob.java:37-48`) first checks the Brain
        // memory and then the currently running goals. Pumpkin's concrete mobs are represented
        // through `Mob` rather than a separate Java-style PathfinderMob subtype, so query both
        // sources here instead of relying on the currently-unused optional PathAwareEntity
        // adapter.
        if self.get_mob_entity().brain.as_ref().is_some_and(|brain| {
            brain.has_value::<crate::entity::ai::brain::memory::IsPanickingMemory>()
        }) {
            return true;
        }

        self.get_mob_entity()
            .goals_selector
            .lock()
            .unwrap()
            .is_panic_running()
    }

    fn get_job_site_pos(&self) -> Option<pumpkin_util::math::position::BlockPos> {
        <T as Mob>::get_job_site(self)
    }

    fn get_home_pos(&self) -> Option<pumpkin_util::math::position::BlockPos> {
        <T as Mob>::get_home(self)
    }

    fn projectile_deflection(
        &self,
        projectile: &dyn EntityBase,
    ) -> crate::entity::projectile_deflection::ProjectileDeflectionType {
        <T as Mob>::mob_projectile_deflection(self, projectile)
    }

    fn as_nbt_storage(&self) -> &dyn NBTStorage {
        self
    }

    fn get_gravity(&self) -> f64 {
        self.get_mob_gravity()
    }

    fn get_y_velocity_drag(&self) -> Option<f64> {
        self.get_mob_y_velocity_drag()
    }

    fn is_pushed_by_fluids(&self) -> bool {
        self.mob_is_pushed_by_fluids()
    }

    fn is_invulnerable_to_piercing_weapon(&self) -> bool {
        self.mob_is_invulnerable_to_piercing_weapon()
    }

    fn get_experience_reward(&self, _killer: Option<&dyn EntityBase>) -> u32 {
        if self
            .get_entity()
            .age
            .load(std::sync::atomic::Ordering::Relaxed)
            < 0
        {
            return 0;
        }
        // TODO: apply enchantment processing like in vanilla
        Mob::get_base_experience_reward(self)
    }

    fn get_base_experience_reward(&self) -> u32 {
        Mob::get_base_experience_reward(self)
    }
}

/// Vanilla's bright-light no-action update is defined by `Monster.aiStep`, not by the
/// `MONSTER` category. Slimes and cube mobs share that category but extend `Mob`/`AbstractCubeMob`
/// instead of `Monster`; the ender dragon, ghast, phantom, hoglin, and shulker also extend other
/// base classes directly. Camel husks and zombie mounts are also in the monster category without
/// extending `Monster`.
fn uses_monster_no_action_time(entity_type: &EntityType) -> bool {
    entity_type.category == &MobCategory::MONSTER
        && entity_type.id != EntityType::ENDER_DRAGON.id
        && entity_type.id != EntityType::GHAST.id
        && entity_type.id != EntityType::HOGLIN.id
        && entity_type.id != EntityType::PHANTOM.id
        && entity_type.id != EntityType::SHULKER.id
        && entity_type.id != EntityType::SLIME.id
        && entity_type.id != EntityType::MAGMA_CUBE.id
        && entity_type.id != EntityType::SULFUR_CUBE.id
        && entity_type.id != EntityType::CAMEL_HUSK.id
        && entity_type.id != EntityType::ZOMBIE_HORSE.id
        && entity_type.id != EntityType::ZOMBIE_NAUTILUS.id
}

#[expect(dead_code)]
const DEFAULT_PATHFINDING_FAVOR: f32 = 0.0;

const fn fire_aspect_ticks(level: i32) -> u32 {
    if level > 0 { level as u32 * 80 } else { 0 }
}

const fn knockback_enchantment_strength(level: u32) -> f64 {
    level as f64 * 0.5
}

const fn attack_knockback_strength(attribute: f64, enchantment_level: u32) -> f64 {
    attribute + knockback_enchantment_strength(enchantment_level)
}

fn mob_weapon_durability_cost(stack: &ItemStack) -> i32 {
    stack
        .get_data_component::<WeaponImpl>()
        .map_or(0, |weapon| weapon.item_damage_per_attack as i32)
}

/// Vanilla `Mob.canReplaceEqualItem` (`Mob.java:654-666`): prefer the stack with
/// more enchantments, then the stack with less damage, then a newly named stack.
fn can_replace_equal_item(new_stack: &ItemStack, current_stack: &ItemStack) -> bool {
    let new_enchantment_count = new_stack
        .get_data_component::<pumpkin_data::data_component_impl::EnchantmentsImpl>()
        .map_or(0, |enchantments| enchantments.enchantment.len());
    let current_enchantment_count = current_stack
        .get_data_component::<pumpkin_data::data_component_impl::EnchantmentsImpl>()
        .map_or(0, |enchantments| enchantments.enchantment.len());
    if new_enchantment_count != current_enchantment_count {
        return new_enchantment_count > current_enchantment_count;
    }

    let new_damage = new_stack.get_damage();
    let current_damage = current_stack.get_damage();
    (new_damage != current_damage && new_damage < current_damage)
        || (new_damage == current_damage
            && new_stack
                .get_data_component::<pumpkin_data::data_component_impl::CustomNameImpl>()
                .is_some()
            && current_stack
                .get_data_component::<pumpkin_data::data_component_impl::CustomNameImpl>()
                .is_none())
}

pub trait PathAwareEntity: Mob + Send + Sync {
    fn get_pathfinding_favor(&self, _block_pos: BlockPos, _world: Arc<World>) -> f32 {
        0.0
    }

    // TODO: missing SpawnReason attribute
    fn can_spawn(&self, world: Arc<World>) -> bool {
        self.get_pathfinding_favor(
            self.get_mob_entity().living_entity.entity.block_pos.load(),
            world,
        ) >= 0.0
    }

    fn is_navigation<'a>(&'a self) -> Pin<Box<dyn Future<Output = bool> + Send + 'a>> {
        Box::pin(async {
            let navigator = self
                .get_mob_entity()
                .navigator
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            !navigator.is_idle()
        })
    }

    fn is_panicking(&self) -> bool {
        if self.get_mob_entity().brain.as_ref().is_some_and(|brain| {
            brain.has_value::<crate::entity::ai::brain::memory::IsPanickingMemory>()
        }) {
            return true;
        }

        self.get_mob_entity()
            .goals_selector
            .lock()
            .unwrap()
            .is_panic_running()
    }

    fn should_follow_leash(&self) -> bool {
        true
    }

    fn on_short_leash_tick(&self) {
        // TODO: implement
    }

    fn before_leash_tick(&self) {
        // TODO: implement
    }

    fn get_follow_leash_speed(&self) -> f32 {
        1.0
    }
}

#[cfg(test)]
mod tests {
    use super::{
        DEFAULT_ITEM_PICKUP_REACH, EntityType, attack_knockback_strength, can_replace_equal_item,
        fire_aspect_ticks, knockback_enchantment_strength, max_fall_distance_for_state,
        max_spawn_cluster_size_for, mob_weapon_durability_cost, uses_monster_no_action_time,
    };
    use pumpkin_data::item::Item;
    use pumpkin_data::item_stack::ItemStack;

    #[test]
    fn fire_aspect_uses_eighty_ticks_per_level() {
        assert_eq!(fire_aspect_ticks(1), 80);
        assert_eq!(fire_aspect_ticks(2), 160);
    }

    #[test]
    fn knockback_enchantment_adds_half_strength_per_level() {
        assert_eq!(knockback_enchantment_strength(1), 0.5);
        assert_eq!(knockback_enchantment_strength(2), 1.0);
    }

    #[test]
    fn attack_knockback_keeps_the_attribute_and_enchantment_components() {
        assert_eq!(attack_knockback_strength(0.0, 0), 0.0);
        assert_eq!(attack_knockback_strength(1.5, 0), 1.5);
        assert_eq!(attack_knockback_strength(1.5, 2), 2.5);
    }

    #[test]
    fn mob_weapon_durability_uses_the_weapon_component_cost() {
        assert_eq!(
            mob_weapon_durability_cost(&ItemStack::new(1, &Item::IRON_SWORD)),
            1
        );
        assert_eq!(
            mob_weapon_durability_cost(&ItemStack::new(1, &Item::IRON_AXE)),
            2
        );
        assert_eq!(
            mob_weapon_durability_cost(&ItemStack::new(1, &Item::COAL)),
            0
        );
    }

    #[test]
    fn equal_equipment_prefers_less_damage_and_then_a_custom_name() {
        let mut damaged = ItemStack::new(1, &Item::IRON_SWORD);
        damaged.set_damage(10);
        let undamaged = ItemStack::new(1, &Item::IRON_SWORD);
        assert!(can_replace_equal_item(&undamaged, &damaged));
        assert!(!can_replace_equal_item(&damaged, &undamaged));

        let mut named = ItemStack::new(1, &Item::IRON_SWORD);
        named.set_custom_name("named".into());
        assert!(can_replace_equal_item(&named, &undamaged));
    }

    #[test]
    fn bright_monster_no_action_update_excludes_non_monster_mob_classes() {
        for entity_type in [
            EntityType::SLIME,
            EntityType::MAGMA_CUBE,
            EntityType::SULFUR_CUBE,
            EntityType::ENDER_DRAGON,
            EntityType::GHAST,
            EntityType::HOGLIN,
            EntityType::PHANTOM,
            EntityType::SHULKER,
            EntityType::CAMEL_HUSK,
            EntityType::ZOMBIE_HORSE,
            EntityType::ZOMBIE_NAUTILUS,
        ] {
            assert!(!uses_monster_no_action_time(&entity_type));
        }
        assert!(uses_monster_no_action_time(&EntityType::ZOMBIE));
    }

    #[test]
    fn peaceful_despawn_uses_the_entity_type_flag() {
        const {
            assert!(!EntityType::ZOMBIE.allowed_in_peaceful);
            assert!(EntityType::PIGLIN.allowed_in_peaceful);
            assert!(EntityType::SHULKER.allowed_in_peaceful);
        }
    }

    #[test]
    // `Mob.java:104-105, 517-518`.
    fn default_item_pickup_reach_matches_vanilla() {
        assert_eq!(DEFAULT_ITEM_PICKUP_REACH, (1.0, 0.0, 1.0));
    }

    #[test]
    fn max_fall_distance_spends_health_only_with_a_target() {
        // `Mob.getMaxFallDistance` (`Mob.java:834-846`) uses three blocks without a target and
        // adds the difficulty/health-derived sacrifice only while targeting.
        assert_eq!(max_fall_distance_for_state(false, 20.0, 20.0, 2), 3);
        assert_eq!(max_fall_distance_for_state(true, 20.0, 20.0, 2), 12);
        assert_eq!(max_fall_distance_for_state(true, 4.0, 20.0, 3), 3);
    }

    #[test]
    fn max_spawn_cluster_size_uses_mob_overrides() {
        // `Mob.getMaxSpawnClusterSize` (`Mob.java:825-827`) and its overrides select these
        // limits before `NaturalSpawner` checks the accumulated cluster.
        assert_eq!(max_spawn_cluster_size_for(EntityType::GHAST.id), 1);
        assert_eq!(max_spawn_cluster_size_for(EntityType::WOLF.id), 8);
        assert_eq!(max_spawn_cluster_size_for(EntityType::SALMON.id), 5);
        assert_eq!(max_spawn_cluster_size_for(EntityType::HORSE.id), 6);
        assert_eq!(max_spawn_cluster_size_for(EntityType::ZOMBIE.id), 4);
    }
}
