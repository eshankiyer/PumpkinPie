// Legacy invariant checks retained for vanilla behavior; migrate these paths before removing this allow.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicI64, Ordering};
use std::sync::{Arc, Weak};
use uuid::Uuid;

use crate::block::blocks::{bed::BedBlock, composter::ComposterBlock};
use pumpkin_data::Block;
use pumpkin_data::Enchantment;
use pumpkin_data::HorizontalFacingExt;
use pumpkin_data::attributes::Attributes;
use pumpkin_data::block_properties::{
    BedPart, BlockProperties, ComposterLikeProperties, WhiteBedLikeProperties as BedProperties,
};
use pumpkin_data::damage::DamageType;
use pumpkin_data::effect::StatusEffect;
use pumpkin_data::entity::{EntityPose, EntityType};
use pumpkin_data::item::{Item, JavaToBedrockItemMapping};
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::potion::Effect;
use pumpkin_data::tag::{Enchantment as EnchantmentTag, Taggable};
use pumpkin_data::tracked_data;
use pumpkin_inventory::merchant::merchant_screen_handler::MerchantScreenHandler;
use pumpkin_inventory::screen_handler::{
    BoxFuture, InventoryPlayer, ScreenHandlerFactory, SharedScreenHandler,
};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_protocol::bedrock::{
    client::set_actor_data::{EntityMetadata, MetadataValue, entity_data_key},
    server::actor_event::ActorEventType,
};
use pumpkin_protocol::codec::var_int::VarInt;
use pumpkin_protocol::java::client::play::{CMerchantOffers, Metadata};
use pumpkin_util::math::{boundingbox::BoundingBox, position::BlockPos, vector3::Vector3};
use pumpkin_util::text::TextComponent;
use pumpkin_util::version::JavaMinecraftVersion;
use pumpkin_world::inventory::SimpleInventory;
use tokio::sync::Mutex;

use crate::entity::player::Player;
use crate::entity::{
    Entity, EntityBase, NBTStorage,
    ageable::{AgeableData, AgeableMob},
    ai::{
        goal::{
            avoid_entity::AvoidEntityGoal,
            interact_with_door::InteractWithDoorGoal,
            look_around::RandomLookAroundGoal,
            look_at_entity::LookAtEntityGoal,
            ring_bell::RingBellGoal,
            show_trades_to_player::ShowTradesToPlayerGoal,
            socialize_at_bell::SocializeAtBellGoal,
            stroll_around_poi::{StrollAroundPoiGoal, StrollPoi},
            swim::SwimGoal,
            trade_with_player::TradeWithPlayerGoal,
            villager_panic::VillagerPanicGoal,
            villager_schedule::{self, VillagerScheduleGoal},
            wander_around::WanderAroundGoal,
            work_at_job_site::WorkAtJobSiteGoal,
        },
        pathfinder::Navigator,
    },
    experience_orb::ExperienceOrbEntity,
    mob::{Mob, MobEntity},
};
use crate::world::World;
use crate::world::villager_poi::profession_for_block;

pub mod breed;
pub mod data;
pub mod gossip;
pub use breed::VillagerBreedGoal;
pub use data::{
    BREEDING_FOOD_THRESHOLD, GossipType, VillagerData, VillagerProfession, VillagerType,
    get_food_points, villager_type_at, villager_type_by_biome,
};
pub use gossip::GossipContainer;

async fn trigger_trade_advancement(player: &Player) {
    player
        .trigger_advancement(
            crate::entity::player::advancement::trigger::AdvancementTrigger::TradedWithVillager,
        )
        .await;
}

fn enchanted_book_offer_items(
    rng: &mut impl rand::Rng,
) -> Option<(ItemStack, ItemStack, Option<ItemStack>)> {
    use pumpkin_data::data_component::DataComponent;
    use pumpkin_data::data_component_impl::StoredEnchantmentsImpl;
    use rand::{RngExt, seq::IndexedRandom};
    use std::borrow::Cow;

    let enchantment_id = *EnchantmentTag::MINECRAFT_TRADEABLE.1.choose(rng)? as u8;
    let enchantment = Enchantment::from_id(enchantment_id)?;
    let level = rng.random_range(1..=enchantment.max_level);
    let mut emeralds = 2 + rng.random_range(0..5 + level * 10) + 3 * level;
    if enchantment.has_tag(&EnchantmentTag::MINECRAFT_DOUBLE_TRADE_PRICE) {
        emeralds *= 2;
    }

    let output = ItemStack::new_with_component(
        1,
        &Item::ENCHANTED_BOOK,
        vec![(
            DataComponent::StoredEnchantments,
            Some(Box::new(StoredEnchantmentsImpl {
                enchantment: Cow::Owned(vec![(enchantment, level)]),
            })),
        )],
    );

    Some((
        ItemStack::new(emeralds.min(64) as u8, &Item::EMERALD),
        output,
        Some(ItemStack::new(1, &Item::BOOK)),
    ))
}

fn enchant_trade_item(
    rng: &mut impl rand::Rng,
    item: &'static Item,
    min_level: i32,
    max_level: i32,
) -> Option<(ItemStack, i32)> {
    use pumpkin_data::data_component_impl::EnchantableImpl;
    use rand::RngExt;

    let mut stack = ItemStack::new(1, item);
    let enchantability = stack
        .get_data_component::<EnchantableImpl>()
        .map_or(1, |value| value.value);
    let additional_cost = rng.random_range(min_level..=max_level);
    let mut level = additional_cost
        + 1
        + rng.random_range(0..=enchantability / 4)
        + rng.random_range(0..=enchantability / 4);
    let bonus = (rng.random::<f32>() + rng.random::<f32>() - 1.0) * 0.15;
    level = ((level as f32 + level as f32 * bonus).round() as i32).max(1);

    let mut available = EnchantmentTag::MINECRAFT_ON_TRADED_EQUIPMENT
        .1
        .iter()
        .filter_map(|id| Enchantment::from_id(*id as u8))
        .filter(|enchantment| enchantment.can_enchant(item))
        .filter_map(|enchantment| {
            (1..=enchantment.max_level)
                .rev()
                .find(|candidate_level| {
                    (enchantment.min_cost.calculate(*candidate_level)
                        ..=enchantment.max_cost.calculate(*candidate_level))
                        .contains(&level)
                })
                .map(|candidate_level| (enchantment, candidate_level))
        })
        .collect::<Vec<_>>();

    if available.is_empty() {
        return None;
    }

    while !available.is_empty() {
        let total_weight: i32 = available
            .iter()
            .map(|(enchantment, _)| enchantment.weight)
            .sum();
        let mut choice = rng.random_range(0..total_weight);
        let chosen_index = available
            .iter()
            .position(|(enchantment, _)| {
                choice -= enchantment.weight;
                choice < 0
            })
            .unwrap_or(0);
        let (enchantment, enchantment_level) = available[chosen_index];
        stack.enchant(enchantment, enchantment_level);

        if rng.random_range(0..50) > level {
            break;
        }
        available.retain(|(candidate, _)| candidate.are_compatible(enchantment));
        level /= 2;
    }
    Some((stack, additional_cost))
}

fn apply_random_dye(rng: &mut impl rand::Rng, stack: &mut ItemStack) {
    use pumpkin_data::data_component::DataComponent;
    use pumpkin_data::data_component_impl::{DataComponentImpl, DyedColorImpl};
    use rand::RngExt;

    const COLORS: [i32; 16] = [
        0xF9FFFE, 0xF9801D, 0xC74EBD, 0x3AB3DA, 0xFED83D, 0x80C71F, 0xF38BAA, 0x474F52, 0x9D9D97,
        0x169C9C, 0x8932B8, 0x3C44AA, 0x835432, 0x5E7C16, 0xB02E26, 0x1D1D21,
    ];
    let dye_count = 1 + i32::from(rng.random_bool(0.75)) + i32::from(rng.random_bool(0.75));
    let mut channels = [0; 3];
    let mut brightness = 0;
    for _ in 0..dye_count {
        let color = COLORS[rng.random_range(0..COLORS.len())];
        let rgb = [(color >> 16) & 255, (color >> 8) & 255, color & 255];
        brightness += rgb[0].max(rgb[1]).max(rgb[2]);
        for (total, value) in channels.iter_mut().zip(rgb) {
            *total += value;
        }
    }
    let mut rgb = channels.map(|channel| channel / dye_count);
    let average_brightness = brightness / dye_count;
    let max_channel = rgb[0].max(rgb[1]).max(rgb[2]);
    for channel in &mut rgb {
        *channel = average_brightness * *channel / max_channel;
    }
    let color = (rgb[0] << 16) | (rgb[1] << 8) | rgb[2];
    stack.patch.push((
        DataComponent::DyedColor,
        Some(DyedColorImpl { rgb: color }.to_dyn()),
    ));
}

fn apply_random_stew_effect(rng: &mut impl rand::Rng, stack: &mut ItemStack) {
    use pumpkin_data::data_component::DataComponent;
    use pumpkin_data::data_component_impl::{
        DataComponentImpl, SuspiciousStewEffect, SuspiciousStewEffectsImpl,
    };
    use rand::RngExt;
    use std::borrow::Cow;

    const EFFECTS: [(&str, i32); 6] = [
        ("minecraft:night_vision", 100),
        ("minecraft:jump_boost", 160),
        ("minecraft:weakness", 140),
        ("minecraft:blindness", 120),
        ("minecraft:poison", 280),
        ("minecraft:saturation", 7),
    ];
    let (effect, duration) = EFFECTS[rng.random_range(0..EFFECTS.len())];
    stack.patch.push((
        DataComponent::SuspiciousStewEffects,
        Some(
            SuspiciousStewEffectsImpl {
                effects: Cow::Owned(vec![SuspiciousStewEffect {
                    effect: Cow::Owned(effect.to_owned()),
                    duration,
                }]),
            }
            .to_dyn(),
        ),
    ));
}

fn apply_potion(stack: &mut ItemStack, potion_name: &str) {
    use pumpkin_data::data_component::DataComponent;
    use pumpkin_data::data_component_impl::{DataComponentImpl, PotionContentsImpl};

    let Some(potion) = pumpkin_data::potion::Potion::from_name(
        potion_name
            .strip_prefix("minecraft:")
            .unwrap_or(potion_name),
    ) else {
        return;
    };
    stack.patch.push((
        DataComponent::PotionContents,
        Some(
            PotionContentsImpl {
                potion_id: Some(i32::from(potion.id)),
                custom_color: None,
                custom_effects: Vec::new(),
                custom_name: None,
            }
            .to_dyn(),
        ),
    ));
}

pub struct VillagerEntity {
    pub mob_entity: MobEntity,
    pub villager_data: Mutex<VillagerData>,
    pub food_level: AtomicI32,
    pub xp: AtomicI32,
    pub last_restock_time: AtomicI64,
    pub last_restock_check_day: AtomicI64,
    pub last_worked_at_poi: AtomicI64,
    pub restocks_today: AtomicI32,
    pub gossips: Mutex<GossipContainer>,
    pub last_gossip_decay_time: AtomicI64,
    /// Vanilla `lastGossipTime` (`Villager.java:814-819`): world-age tick of the
    /// last successful gossip exchange, gating the 1200-tick per-side cooldown.
    pub last_gossip_time: AtomicI64,
    /// Vanilla `LAST_SLEPT` brain memory (`Villager::golemSpawnConditionsMet`). World-age tick
    /// at which this villager last entered the sleeping pose; 0 == never slept.
    pub last_slept_time: AtomicI64,
    /// Vanilla `GOLEM_DETECTED_RECENTLY` brain memory (599-tick expiry), set both by a
    /// successful golem spawn and by nearby-golem detection. World-age tick after which the
    /// memory is considered expired; 0 == not set.
    pub golem_detected_until: AtomicI64,
    pub inventory: Arc<Mutex<Vec<Arc<Mutex<ItemStack>>>>>,
    pub merchant_inventory: Arc<SimpleInventory>,
    pub offers: Mutex<Vec<pumpkin_protocol::java::client::play::MerchantOffer>>,
    pub merchant_update_timer: AtomicI32,
    pub unhappy_counter: AtomicI32,
    pub trade_sound_cooldown: AtomicI32,
    pub increase_profession_level_on_update: AtomicBool,
    pub last_traded_player: Mutex<Option<Uuid>>,
    pub trading_player: std::sync::Mutex<Option<(Uuid, u8)>>,
    pub is_trading: AtomicBool,
    pub job_site: std::sync::Mutex<Option<BlockPos>>,
    pub job_site_pending: AtomicBool,
    pub home_pos: std::sync::Mutex<Option<BlockPos>>,
    /// Vanilla `MEETING_POINT` brain memory: the bell POI claimed via `AcquirePoi`
    /// (`VillagerGoalPackages.java`, `getCorePackage` priority 10).
    pub meeting_point: std::sync::Mutex<Option<BlockPos>>,
    pub self_weak: std::sync::Mutex<Option<Weak<Self>>>,
    /// Vanilla `AgeableMob`'s age/`AgeLocked` state. Villagers are `AgeableMob`s
    /// (`Villager extends AbstractVillager extends AgeableMob`), so babies grow up on the
    /// shared `ageable_ai_step` clock.
    pub ageable_data: AgeableData,
    /// Counts `mob_tick` invocations for the 20-tick sensor cadence below. Previously that
    /// cadence keyed off `Entity::age`, which for villagers is the vanilla *breeding* age and
    /// not a monotonic tick counter, so it read 0 forever and the "every 20 ticks" block ran
    /// every tick.
    pub sensor_tick: AtomicI32,
}

impl VillagerEntity {
    fn bedrock_metadata(data: VillagerData, xp: i32) -> EntityMetadata {
        const PROFESSIONS: [i32; 15] = [0, 8, 11, 6, 7, 1, 2, 4, 12, 5, 13, 14, 3, 10, 9];
        const REGIONS: [i32; 7] = [1, 2, 0, 3, 4, 5, 6];

        let mut metadata = EntityMetadata::new();
        metadata.set(
            entity_data_key::VARIANT,
            MetadataValue::Int(
                usize::try_from(data.profession.0)
                    .ok()
                    .and_then(|id| PROFESSIONS.get(id))
                    .copied()
                    .unwrap_or_default(),
            ),
        );
        metadata.set(
            entity_data_key::MARK_VARIANT,
            MetadataValue::Int(
                usize::try_from(data.r#type.0)
                    .ok()
                    .and_then(|id| REGIONS.get(id))
                    .copied()
                    .unwrap_or_default(),
            ),
        );
        metadata.set(
            entity_data_key::TRADE_TIER,
            MetadataValue::Int(data.level.0.saturating_sub(1)),
        );
        metadata.set(entity_data_key::MAX_TRADE_TIER, MetadataValue::Int(4));
        metadata.set(entity_data_key::TRADE_EXPERIENCE, MetadataValue::Int(xp));
        metadata
    }

    #[allow(clippy::too_many_lines)]
    pub fn new(entity: Entity) -> Arc<Self> {
        // Vanilla `Villager#finalizeSpawn` sets the type from `VillagerType.byBiome`
        // at the spawn position.
        let villager_type = data::villager_type_at(&entity);
        let mob_entity = MobEntity::new(entity);
        let villager_data = VillagerData::new(villager_type, VillagerProfession::None, 1);
        let inventory = Arc::new(Mutex::new(
            (0..8)
                .map(|_| Arc::new(Mutex::new(ItemStack::EMPTY.clone())))
                .collect(),
        ));

        let villager = Self {
            mob_entity,
            villager_data: Mutex::new(villager_data),
            food_level: AtomicI32::new(0),
            xp: AtomicI32::new(0),
            last_restock_time: AtomicI64::new(0),
            last_restock_check_day: AtomicI64::new(0),
            last_worked_at_poi: AtomicI64::new(0),
            restocks_today: AtomicI32::new(0),
            gossips: Mutex::new(GossipContainer::new()),
            last_gossip_decay_time: AtomicI64::new(0),
            last_gossip_time: AtomicI64::new(0),
            last_slept_time: AtomicI64::new(0),
            golem_detected_until: AtomicI64::new(0),
            inventory,
            merchant_inventory: Arc::new(SimpleInventory::new(3)),
            offers: Mutex::new(Vec::new()),
            merchant_update_timer: AtomicI32::new(0),
            unhappy_counter: AtomicI32::new(0),
            trade_sound_cooldown: AtomicI32::new(0),
            increase_profession_level_on_update: AtomicBool::new(false),
            last_traded_player: Mutex::new(None),
            trading_player: std::sync::Mutex::new(None),
            is_trading: AtomicBool::new(false),
            job_site: std::sync::Mutex::new(None),
            job_site_pending: AtomicBool::new(false),
            home_pos: std::sync::Mutex::new(None),
            meeting_point: std::sync::Mutex::new(None),
            self_weak: std::sync::Mutex::new(None),
            ageable_data: AgeableData::default(),
            sensor_tick: AtomicI32::new(0),
        };
        let mob_arc = Arc::new(villager);
        *mob_arc
            .self_weak
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Arc::downgrade(&mob_arc));
        let mob_weak: Weak<dyn Mob> = {
            let mob_arc: Arc<dyn Mob> = mob_arc.clone();
            Arc::downgrade(&mob_arc)
        };

        // Vanilla `Villager` constructor: `this.getNavigation().setCanOpenDoors(true);`.
        mob_arc
            .mob_entity
            .navigator
            .lock()
            .unwrap()
            .set_can_open_doors(true);

        {
            let mut goal_selector = mob_arc
                .mob_entity
                .goals_selector
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);

            goal_selector.add_goal(0, Box::new(SwimGoal::default()));
            // Approximates vanilla's brain-based `InteractWithDoor` behavior
            // (`VillagerGoalPackages.java:37`, `Pair.of(0, InteractWithDoor.create())`) with the
            // goal-based port, same as `CopperGolemEntity` does for its own `InteractWithDoor`
            // core-activity entry.
            goal_selector.add_goal(0, Box::new(InteractWithDoorGoal::new(true)));
            // `VillagerGoalPackages.java:39`, `Pair.of(0, new VillagerPanicTrigger())`: the
            // panic-state holder (hurt/hostile gate + 100-tick golem-summon cadence). Runs
            // alongside the flee goals below; see `villager_panic.rs` for the split.
            goal_selector.add_goal(0, VillagerPanicGoal::new());
            // `RingBell.create` (`RingBell.java:15-31`) belongs to the vanilla pre-raid package.
            // The goal gates itself on the same nearby pre-raid/inter-wave raid state.
            goal_selector.add_goal(0, Box::new(RingBellGoal));
            // Villagers avoid threats
            goal_selector.add_goal(
                1,
                Box::new(AvoidEntityGoal::new(&EntityType::ZOMBIE, 8.0, 0.5, 0.5)),
            );
            goal_selector.add_goal(
                1,
                Box::new(AvoidEntityGoal::new(
                    &EntityType::ZOMBIE_VILLAGER,
                    8.0,
                    0.5,
                    0.5,
                )),
            );
            goal_selector.add_goal(
                1,
                Box::new(AvoidEntityGoal::new(&EntityType::HUSK, 8.0, 0.5, 0.5)),
            );
            goal_selector.add_goal(
                1,
                Box::new(AvoidEntityGoal::new(&EntityType::DROWNED, 8.0, 0.5, 0.5)),
            );
            goal_selector.add_goal(
                1,
                Box::new(AvoidEntityGoal::new(&EntityType::PILLAGER, 12.0, 0.5, 0.5)),
            );
            goal_selector.add_goal(
                1,
                Box::new(AvoidEntityGoal::new(
                    &EntityType::VINDICATOR,
                    12.0,
                    0.5,
                    0.5,
                )),
            );
            goal_selector.add_goal(
                1,
                Box::new(AvoidEntityGoal::new(&EntityType::EVOKER, 12.0, 0.5, 0.5)),
            );
            goal_selector.add_goal(
                1,
                Box::new(AvoidEntityGoal::new(&EntityType::RAVAGER, 12.0, 0.5, 0.5)),
            );
            goal_selector.add_goal(
                1,
                Box::new(AvoidEntityGoal::new(&EntityType::VEX, 12.0, 0.5, 0.5)),
            );

            // `VillagerGoalPackages.getWorkPackage`/`getRestPackage`, simplified: walk to
            // the claimed job site/bed on schedule. Priority 1 (below AvoidEntityGoal at
            // 1, above WanderAroundGoal at 2) so it preempts wandering during work/rest
            // hours and yields MOVE back to wandering during meet/idle hours.
            goal_selector.add_goal(1, Box::new(VillagerScheduleGoal::new(0.5)));

            goal_selector.add_goal(2, Box::new(TradeWithPlayerGoal::new(0.5)));
            // `VillagerGoalPackages.getWorkPackage`/`getMeetPackage`/`getIdlePackage`
            // register `ShowTradesToPlayer(400, 1600)` in all three
            // (`VillagerGoalPackages.java:94,155,195`); see `show_trades_to_player.rs`
            // for what the port does.
            goal_selector.add_goal(2, Box::new(ShowTradesToPlayerGoal::new()));
            // `VillagerGoalPackages.getIdlePackage` runs `VillagerMakeLove` behind the
            // `InteractWith` that sets `BREED_TARGET`; see `breed.rs` for what the port does
            // and does not carry across.
            goal_selector.add_goal(2, Box::new(VillagerBreedGoal::new(0.5)));
            // `SocializeAtBell.create` (`SocializeAtBell.java:14-41`) is one of the two
            // shuffled MEET behaviors (`VillagerGoalPackages.java:147-153`).
            goal_selector.add_goal(2, Box::new(SocializeAtBellGoal::new()));
            // Basic movement and looking (Vanilla uses 0.5 speed)
            goal_selector.add_goal(3, Box::new(WorkAtJobSiteGoal::new(0.5)));
            // `VillagerGoalPackages.getWorkPackage`/`getMeetPackage`: once already near the
            // claimed POI, mill about it instead of standing still or free-wandering.
            // `StrollAroundPoi.create(MemoryModuleType.JOB_SITE, 0.4F, 4)` /
            // `StrollAroundPoi.create(MemoryModuleType.MEETING_POINT, 0.4F, 40)`
            // (`VillagerGoalPackages.java:86,152`). Priority 3, alongside WorkAtJobSiteGoal:
            // both are only reachable once VillagerScheduleGoal (priority 1) has stopped
            // holding MOVE, i.e. the villager has already arrived.
            goal_selector.add_goal(
                3,
                Box::new(StrollAroundPoiGoal::new(StrollPoi::JobSite, 0.4, 4)),
            );
            goal_selector.add_goal(
                3,
                Box::new(StrollAroundPoiGoal::new(StrollPoi::MeetingPoint, 0.4, 40)),
            );
            goal_selector.add_goal(4, Box::new(WanderAroundGoal::new(0.5)));
            goal_selector.add_goal(
                5,
                LookAtEntityGoal::with_default(mob_weak.clone(), &EntityType::PLAYER, 8.0),
            );
            goal_selector.add_goal(
                6,
                LookAtEntityGoal::with_default(mob_weak, &EntityType::VILLAGER, 8.0),
            );
            goal_selector.add_goal(7, Box::new(RandomLookAroundGoal::default()));
        };

        // Send initial metadata
        let bedrock_metadata = Self::bedrock_metadata(villager_data, 0);
        mob_arc.get_entity().send_meta_data(
            &[Metadata::new(
                tracked_data::villager::VILLAGER_DATA,
                villager_data,
            )],
            Some(&bedrock_metadata),
        );

        mob_arc
    }

    pub async fn count_food_points_in_inventory(&self) -> i32 {
        let inventory = self.inventory.lock().await;
        let mut total = 0;
        for stack_mutex in inventory.iter() {
            let stack = stack_mutex.lock().await;
            if !stack.is_empty() {
                total += get_food_points(stack.get_item()) * stack.item_count as i32;
            }
        }
        total
    }

    fn poi_owner(&self) -> Option<Weak<dyn EntityBase>> {
        let owner = self
            .self_weak
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()?;
        let owner: Weak<dyn EntityBase> = owner;
        Some(owner)
    }

    pub async fn eat_until_full(&self) {
        if self.food_level.load(Ordering::Relaxed) >= BREEDING_FOOD_THRESHOLD {
            return;
        }
        let inventory = self.inventory.lock().await;
        for stack_mutex in inventory.iter() {
            let mut stack = stack_mutex.lock().await;
            if !stack.is_empty() {
                let points = get_food_points(stack.get_item());
                if points > 0 {
                    while stack.item_count > 0
                        && self.food_level.load(Ordering::Relaxed) < BREEDING_FOOD_THRESHOLD
                    {
                        self.food_level.fetch_add(points, Ordering::Relaxed);
                        stack.item_count -= 1;
                    }
                    if stack.item_count == 0 {
                        *stack = ItemStack::EMPTY.clone();
                    }
                    if self.food_level.load(Ordering::Relaxed) >= BREEDING_FOOD_THRESHOLD {
                        break;
                    }
                }
            }
        }
    }

    pub async fn set_villager_data(&self, data: VillagerData) {
        let old_profession = {
            let mut villager_data = self.villager_data.lock().await;
            let old_profession = villager_data.profession;
            *villager_data = data;
            old_profession
        };
        let bedrock_metadata = Self::bedrock_metadata(data, self.xp.load(Ordering::Relaxed));
        self.get_entity().send_meta_data(
            &[Metadata::new(tracked_data::villager::VILLAGER_DATA, data)],
            Some(&bedrock_metadata),
        );

        if old_profession != data.profession {
            self.offers.lock().await.clear();
        }
    }

    #[expect(clippy::too_many_lines)]
    async fn create_explorer_map(&self, destination: &str) -> Option<ItemStack> {
        use pumpkin_data::data_component::DataComponent;
        use pumpkin_data::data_component_impl::{DataComponentImpl, ItemNameImpl, MapIdImpl};
        use pumpkin_data::structures::{StructureKeys, StructureSet};
        use pumpkin_world::generation::generator::structure_finder::find_nearest_structure_start;

        let (structure_set, structure, name, icon_type) = match destination {
            "minecraft:on_jungle_explorer_maps" => (
                "jungle_temples",
                StructureKeys::JunglePyramid,
                "filled_map.explorer_jungle",
                32,
            ),
            "minecraft:on_swamp_explorer_maps" => (
                "swamp_huts",
                StructureKeys::SwampHut,
                "filled_map.explorer_swamp",
                33,
            ),
            "minecraft:on_desert_village_maps" => (
                "villages",
                StructureKeys::VillageDesert,
                "filled_map.village_desert",
                27,
            ),
            "minecraft:on_plains_village_maps" => (
                "villages",
                StructureKeys::VillagePlains,
                "filled_map.village_plains",
                28,
            ),
            "minecraft:on_savanna_village_maps" => (
                "villages",
                StructureKeys::VillageSavanna,
                "filled_map.village_savanna",
                29,
            ),
            "minecraft:on_snowy_village_maps" => (
                "villages",
                StructureKeys::VillageSnowy,
                "filled_map.village_snowy",
                30,
            ),
            "minecraft:on_taiga_village_maps" => (
                "villages",
                StructureKeys::VillageTaiga,
                "filled_map.village_taiga",
                31,
            ),
            "minecraft:on_ocean_explorer_maps" => (
                "ocean_monuments",
                StructureKeys::Monument,
                "filled_map.monument",
                9,
            ),
            "minecraft:on_trial_chambers_maps" => (
                "trial_chambers",
                StructureKeys::TrialChambers,
                "filled_map.trial_chambers",
                34,
            ),
            "minecraft:on_woodland_explorer_maps" => (
                "woodland_mansions",
                StructureKeys::Mansion,
                "filled_map.mansion",
                8,
            ),
            _ => return None,
        };

        let world = self.get_entity().world.load().clone();
        let generator = world.level.world_gen();
        let target = find_nearest_structure_start(
            self.get_entity().block_pos.load(),
            StructureSet::get(structure_set)?,
            &[structure],
            100,
            &generator,
        )?;
        let server = world.server.upgrade()?;
        let map_id = server.next_map_id();
        let map = server.map_manager.create_map(
            map_id,
            world.dimension.clone(),
            target.0.x,
            target.0.z,
            2,
        );
        map.lock()
            .await
            .decorations
            .push(crate::world::map::MapDecoration {
                icon_type,
                x: 0,
                z: 0,
                direction: 8,
                display_name: None,
            });

        let mut stack = ItemStack::new(1, &Item::FILLED_MAP);
        stack.patch.push((
            DataComponent::MapId,
            Some(MapIdImpl { id: map_id }.to_dyn()),
        ));
        stack.patch.push((
            DataComponent::ItemName,
            Some(ItemNameImpl { name: name.into() }.to_dyn()),
        ));
        Some(stack)
    }

    pub async fn add_trades(&self, profession: VillagerProfession, level: i32) {
        use pumpkin_data::villager::VillagerTradeModifier;
        use pumpkin_protocol::codec::item_stack_seralizer::ItemStackSerializer;
        use rand::seq::IndexedRandom;
        use rand::{RngExt, SeedableRng, rngs::StdRng};
        use std::borrow::Cow;

        let villager_type = self.villager_data.lock().await.type_enum();
        let mut offers = self.offers.lock().await;

        if let Some(trade_set) = profession.trade_set(level) {
            let mut rng = StdRng::from_rng(&mut rand::rng());
            let mut remaining_trades = trade_set.trades.iter().collect::<Vec<_>>();
            let mut added = 0;
            while added < trade_set.amount && !remaining_trades.is_empty() {
                let index = rng.random_range(0..remaining_trades.len());
                let trade = remaining_trades.remove(index);
                if !trade.allowed_types.is_empty() && !trade.allowed_types.contains(&villager_type)
                {
                    continue;
                }
                let mut base_cost_a = ItemStack::new(trade.wants.count as u8, trade.wants.item);
                let mut output = ItemStack::new(trade.gives.count as u8, trade.gives.item);
                let mut cost_b = trade
                    .wants_b
                    .as_ref()
                    .map(|b| ItemStack::new(b.count as u8, b.item));

                match trade.modifier {
                    VillagerTradeModifier::None => {}
                    VillagerTradeModifier::EnchantRandomly => {
                        let Some(items) = enchanted_book_offer_items(&mut rng) else {
                            continue;
                        };
                        (base_cost_a, output, cost_b) = items;
                    }
                    VillagerTradeModifier::EnchantWithLevels { min, max } => {
                        let Some((enchanted, additional_cost)) =
                            enchant_trade_item(&mut rng, trade.gives.item, min, max)
                        else {
                            continue;
                        };
                        output = enchanted;
                        let count = i32::from(base_cost_a.item_count)
                            .saturating_add(additional_cost)
                            .clamp(0, i32::from(base_cost_a.get_max_stack_size()));
                        if count == 0 {
                            continue;
                        }
                        base_cost_a.set_count(count as u8);
                    }
                    VillagerTradeModifier::ExplorationMap { destination } => {
                        let Some(map) = self.create_explorer_map(destination).await else {
                            continue;
                        };
                        output = map;
                    }
                    VillagerTradeModifier::RandomDyes => apply_random_dye(&mut rng, &mut output),
                    VillagerTradeModifier::RandomPotion => {
                        let Some(potion_name) = pumpkin_data::tag::Potion::MINECRAFT_TRADEABLE
                            .0
                            .choose(&mut rng)
                        else {
                            continue;
                        };
                        apply_potion(&mut output, potion_name);
                    }
                    VillagerTradeModifier::SuspiciousStew => {
                        apply_random_stew_effect(&mut rng, &mut output);
                    }
                    VillagerTradeModifier::Potion(potion) => apply_potion(&mut output, potion),
                }
                offers.push(pumpkin_protocol::java::client::play::MerchantOffer {
                    base_cost_a: ItemStackSerializer(Cow::Owned(base_cost_a)),
                    output: ItemStackSerializer(Cow::Owned(output)),
                    cost_b: cost_b.map(|stack| ItemStackSerializer(Cow::Owned(stack))),
                    reward_exp: true,
                    uses: 0,
                    max_uses: trade.max_uses,
                    xp: trade.xp,
                    special_price: 0,
                    price_multiplier: trade.price_multiplier,
                    demand: 0,
                });
                added += 1;
            }
        }
    }

    pub async fn generate_trades(&self, profession: VillagerProfession, level: i32) {
        self.offers.lock().await.clear();
        self.add_trades(profession, level).await;
    }

    async fn update_special_prices(&self, player: &Player) {
        let player_uuid = player.get_entity().entity_uuid;
        let reputation = self
            .gossips
            .lock()
            .await
            .get_reputation(player_uuid, |_| true);
        let hero_amplifier = player
            .living_entity
            .get_effect(&StatusEffect::HERO_OF_THE_VILLAGE)
            .await
            .map(|effect| i32::from(effect.amplifier));

        let mut offers = self.offers.lock().await;
        for offer in offers.iter_mut() {
            offer.special_price = -((reputation as f32 * offer.price_multiplier).floor() as i32);
            if let Some(amplifier) = hero_amplifier {
                let discount = ((0.3 + 0.0625 * f64::from(amplifier))
                    * f64::from(offer.base_cost_a.0.item_count))
                .floor() as i32;
                offer.special_price -= discount.max(1);
            }
        }
    }

    async fn reset_special_prices(&self) {
        for offer in self.offers.lock().await.iter_mut() {
            offer.special_price = 0;
        }
    }

    fn can_continue_trading(
        &self,
        inventory_player: &dyn InventoryPlayer,
        player_uuid: Uuid,
        sync_id: u8,
    ) -> bool {
        let Some(player) = inventory_player.as_any().downcast_ref::<Player>() else {
            return false;
        };
        let entity = self.get_entity();
        let range = player
            .living_entity
            .get_attribute_value(&Attributes::ENTITY_INTERACTION_RANGE)
            + 4.0;
        entity.is_alive()
            && self.mob_entity.living_entity.health.load() > 0.0
            && self
                .trading_player
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .as_ref()
                .is_some_and(|(uuid, id)| *uuid == player_uuid && *id == sync_id)
            && entity
                .bounding_box
                .load()
                .squared_magnitude(player.eye_position())
                < range * range
    }

    async fn complete_trade(&self, offer_index: usize, world: &Arc<World>, player_uuid: Uuid) {
        let (xp_gain, reward_exp) = {
            let mut offers = self.offers.lock().await;
            let Some(offer) = offers.get_mut(offer_index) else {
                return;
            };
            offer.uses += 1;
            (offer.xp, offer.reward_exp)
        };

        let current_xp = self.xp.fetch_add(xp_gain, Ordering::Relaxed) + xp_gain;
        let villager_data = *self.villager_data.lock().await;
        let bedrock_metadata = Self::bedrock_metadata(villager_data, current_xp);
        self.get_entity().send_meta_data(
            &[Metadata::new(
                tracked_data::villager::VILLAGER_DATA,
                villager_data,
            )],
            Some(&bedrock_metadata),
        );
        let mut reward_xp = {
            use rand::RngExt;
            rand::rng().random_range(3..=6)
        };

        let current_level = villager_data.level.0;
        if current_level < 5 {
            let max_xp = match current_level {
                1 => 10,
                2 => 70,
                3 => 150,
                4 => 250,
                _ => 0,
            };
            if current_xp >= max_xp {
                self.merchant_update_timer.store(40, Ordering::Relaxed);
                self.increase_profession_level_on_update
                    .store(true, Ordering::Relaxed);
                reward_xp += 5;
            }
        }
        self.get_entity()
            .play_sound(pumpkin_data::sound::Sound::EntityVillagerYes);
        self.trade_sound_cooldown.store(20, Ordering::Relaxed);
        *self.last_traded_player.lock().await = Some(player_uuid);
        if reward_exp {
            let position = self.get_entity().pos.load().add_raw(0.0, 0.5, 0.0);
            ExperienceOrbEntity::spawn(world, position, reward_xp).await;
        }

        if let Some(player) = world.get_player_by_uuid(player_uuid) {
            trigger_trade_advancement(&player).await;
        }
    }

    async fn resend_offers_to_trading_player(&self) {
        let trading_player = *self
            .trading_player
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some((player_uuid, sync_id)) = trading_player else {
            return;
        };
        let world = self.get_entity().world.load();
        let Some(player) = world.get_player_by_uuid(player_uuid) else {
            return;
        };
        let offers = self.offers.lock().await.clone();
        let villager_data = *self.villager_data.lock().await;

        let screen = player.current_screen_handler.lock().await.clone();
        let mut screen = screen.lock().await;
        if screen.sync_id() != sync_id {
            return;
        }
        if let Some(handler) = screen.as_any_mut().downcast_mut::<MerchantScreenHandler>() {
            handler.offers.clone_from(&offers);
            handler.update_result_slot().await;
        } else {
            return;
        }
        drop(screen);
        self.send_trade_offers(&player, sync_id, offers, villager_data)
            .await;
    }

    async fn decay_gossips(&self, game_time: i64) {
        let last_decay = self.last_gossip_decay_time.load(Ordering::Relaxed);
        if last_decay == 0 {
            self.last_gossip_decay_time
                .store(game_time, Ordering::Relaxed);
            return;
        }
        if game_time < last_decay + 24_000 {
            return;
        }

        self.gossips.lock().await.decay();
        self.last_gossip_decay_time
            .store(game_time, Ordering::Relaxed);
    }

    async fn work_at_job_site(&self, world: &Arc<World>, game_time: i64, day_time: i64, day: i64) {
        use rand::RngExt;

        if !(2_000..9_000).contains(&day_time)
            || game_time - self.last_worked_at_poi.load(Ordering::Relaxed) < 300
            || !rand::rng().random_bool(0.5)
        {
            return;
        }
        self.last_worked_at_poi.store(game_time, Ordering::Relaxed);

        let Some(job_site) = self.get_job_site() else {
            return;
        };
        if job_site
            .to_centered_f64()
            .squared_distance_to_vec(&self.get_entity().pos.load())
            >= 1.73f64.powi(2)
        {
            return;
        }

        let profession = self.villager_data.lock().await.profession_enum();
        if let Some(sound) = profession.work_sound() {
            self.get_entity().play_sound(sound);
        }

        if profession == VillagerProfession::Farmer {
            self.work_at_composter(world, job_site).await;
        }

        let last_restock = self.last_restock_time.load(Ordering::Relaxed);
        let last_check_day = self.last_restock_check_day.swap(day, Ordering::Relaxed);
        if game_time > last_restock + 12_000 || (last_check_day > 0 && day > last_check_day) {
            let missed_restock_count = (2 - self.restocks_today.load(Ordering::Relaxed)).max(0);
            let mut offers = self.offers.lock().await;
            if missed_restock_count > 0 {
                for offer in offers.iter_mut() {
                    offer.reset_uses();
                }
            }
            for _ in 0..missed_restock_count {
                for offer in offers.iter_mut() {
                    offer.update_demand();
                }
            }
            drop(offers);
            self.last_restock_time.store(game_time, Ordering::Relaxed);
            self.restocks_today.store(0, Ordering::Relaxed);
            self.resend_offers_to_trading_player().await;
        }

        let restocks_today = self.restocks_today.load(Ordering::Relaxed);
        let allowed = restocks_today == 0
            || (restocks_today < 2
                && game_time > self.last_restock_time.load(Ordering::Relaxed) + 2_400);
        if !allowed {
            return;
        }

        let mut offers = self.offers.lock().await;
        if !offers
            .iter()
            .any(pumpkin_protocol::java::client::play::MerchantOffer::needs_restock)
        {
            return;
        }
        for offer in offers.iter_mut() {
            offer.update_demand();
            offer.reset_uses();
        }
        self.last_restock_time.store(game_time, Ordering::Relaxed);
        self.restocks_today.fetch_add(1, Ordering::Relaxed);
        drop(offers);
        self.resend_offers_to_trading_player().await;
    }

    /// `WorkAtComposter.useWorkstation` (`WorkAtComposter.java:23-31`) is the farmer-specific
    /// workstation effect that the generic work/re-stock path does not provide. It is called
    /// only after the existing `WorkAtPoi`-equivalent distance, cooldown, and random checks have
    /// succeeded, so the behavior remains reachable through the villager's normal work path.
    async fn work_at_composter(&self, world: &Arc<World>, job_site: BlockPos) {
        let (block, state_id) = world.get_block_and_state_id(&job_site);
        if block != &Block::COMPOSTER {
            return;
        }

        let mut initial_level = ComposterLikeProperties::from_state_id(state_id, block).level;
        if initial_level == 8 {
            // `WorkAtComposter.compostItems` (`WorkAtComposter.java:35-39`) extracts bone meal
            // before using seeds, exactly as a player empty-handedly uses a full composter.
            ComposterBlock
                .clear_composter(world, &job_site, state_id, block)
                .await;
            let (_, cleared_state_id) = world.get_block_and_state_id(&job_site);
            initial_level = ComposterLikeProperties::from_state_id(cleared_state_id, block).level;
        }

        self.make_bread(world).await;

        // `WorkAtComposter.compostItems` (`WorkAtComposter.java:41-70`) keeps ten items of each
        // supported seed type in the inventory and uses at most twenty excess items, scanning
        // the inventory from its last slot to its first.
        let compostable = [Item::WHEAT_SEEDS, Item::BEETROOT_SEEDS];
        let mut total_items_to_use = 20u8;
        let mut items_seen = [0u8; 2];
        let inventory = self.inventory.lock().await;
        for slot in inventory.iter().rev() {
            if total_items_to_use == 0 {
                break;
            }
            let mut stack = slot.lock().await;
            let Some(index) = compostable.iter().position(|item| item.id == stack.item.id) else {
                continue;
            };
            let stack_size = stack.item_count;
            items_seen[index] = items_seen[index].saturating_add(stack_size);
            let available = items_seen[index].saturating_sub(10);
            let items_to_use = available.min(total_items_to_use).min(stack_size);
            for _ in 0..items_to_use {
                if !ComposterBlock::insert_item_from_villager(world, &job_site, &mut stack).await {
                    let (_, current_state_id) = world.get_block_and_state_id(&job_site);
                    let current_level =
                        ComposterLikeProperties::from_state_id(current_state_id, block).level;
                    if current_level == 7 {
                        world.sync_world_event(
                            pumpkin_data::world::WorldEvent::ComposterFill,
                            job_site,
                            i32::from(current_level != initial_level),
                        );
                        return;
                    }
                    break;
                }
                total_items_to_use -= 1;
                if stack.item_count == 0 {
                    stack.clear();
                }
                let (_, current_state_id) = world.get_block_and_state_id(&job_site);
                let current_level =
                    ComposterLikeProperties::from_state_id(current_state_id, block).level;
                if current_level == 7 {
                    world.sync_world_event(
                        pumpkin_data::world::WorldEvent::ComposterFill,
                        job_site,
                        i32::from(current_level != initial_level),
                    );
                    return;
                }
            }
        }
        drop(inventory);

        let (_, final_state_id) = world.get_block_and_state_id(&job_site);
        let final_level = ComposterLikeProperties::from_state_id(final_state_id, block).level;
        world.sync_world_event(
            pumpkin_data::world::WorldEvent::ComposterFill,
            job_site,
            i32::from(final_level != initial_level),
        );
    }

    /// `WorkAtComposter.makeBread` (`WorkAtComposter.java:77-90`): farmers craft at most three
    /// loaves when carrying no more than thirty-six bread, consume three wheat per loaf, and
    /// drop any loaf that does not fit in their inventory.
    async fn make_bread(&self, world: &Arc<World>) {
        let (bread_count, wheat_count) = {
            let inventory = self.inventory.lock().await;
            let mut bread = 0u16;
            let mut wheat = 0u16;
            for slot in inventory.iter() {
                let stack = slot.lock().await;
                if stack.item.id == Item::BREAD.id {
                    bread += u16::from(stack.item_count);
                } else if stack.item.id == Item::WHEAT.id {
                    wheat += u16::from(stack.item_count);
                }
            }
            (bread, wheat)
        };
        if bread_count > 36 || wheat_count < 3 {
            return;
        }

        let loaves = (wheat_count / 3).min(3) as u8;
        let mut wheat_to_remove = loaves * 3;
        {
            let inventory = self.inventory.lock().await;
            for slot in inventory.iter() {
                if wheat_to_remove == 0 {
                    break;
                }
                let mut stack = slot.lock().await;
                if stack.item.id != Item::WHEAT.id {
                    continue;
                }
                let removed = wheat_to_remove.min(stack.item_count);
                stack.decrement(removed);
                wheat_to_remove -= removed;
                if stack.item_count == 0 {
                    stack.clear();
                }
            }
        }

        let bread = ItemStack::new(loaves, &Item::BREAD);
        let inserted = self.add_to_inventory(&bread);
        if inserted == loaves {
            return;
        }

        let leftover = bread.copy_with_count(loaves - inserted);
        let position = self.get_entity().pos.load().add_raw(0.0, 0.5, 0.0);
        let entity = crate::entity::item::ItemEntity::new(
            Entity::new(world.clone(), position, &EntityType::ITEM),
            leftover,
        );
        world.spawn_entity(Arc::new(entity)).await;
    }

    #[expect(clippy::too_many_lines)]
    async fn update_job_site(&self, world: &crate::world::World) {
        let data = *self.villager_data.lock().await;
        let profession = data.profession_enum();
        let is_adult = self.get_entity().age.load(Ordering::Relaxed) >= 0;

        if profession == VillagerProfession::Nitwit || !is_adult {
            if let Some(site) = self.get_job_site() {
                world
                    .villager_poi
                    .lock()
                    .await
                    .release(site, self.get_entity().entity_uuid);
                *self
                    .job_site
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
                self.job_site_pending.store(false, Ordering::Relaxed);
            }
            return;
        }

        // `ResetProfession.create` (`ResetProfession.java:11-17`) fires a novice villager
        // whenever its JOB_SITE memory is absent, including the interval in which no new POI
        // can be acquired. The nearby-POI validation below handles the invalid-existing-site
        // case; this branch covers the genuinely absent-site case before acquisition runs.
        if self.get_job_site().is_none()
            && profession != VillagerProfession::None
            && profession != VillagerProfession::Nitwit
            && self.xp.load(Ordering::Relaxed) == 0
            && data.level.0 <= 1
        {
            let r#type = data.type_enum();
            self.set_villager_data(VillagerData::new(r#type, VillagerProfession::None, 1))
                .await;
            return;
        }

        let Some(owner) = self.poi_owner() else {
            return;
        };

        if let Some(current_site) = self.get_job_site()
            && current_site
                .to_centered_f64()
                .squared_distance_to_vec(&self.get_entity().pos.load())
                < 16.0f64.powi(2)
        {
            let (block, _state) = world.get_block_and_state(&current_site);
            let expected = (profession != VillagerProfession::None).then_some(profession);
            let valid = world
                .villager_poi
                .lock()
                .await
                .claim(current_site, block, owner.clone(), expected)
                .is_some();

            if !valid {
                *self
                    .job_site
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
                self.job_site_pending.store(false, Ordering::Relaxed);
                if self.xp.load(Ordering::Relaxed) == 0
                    && data.level.0 <= 1
                    && profession != VillagerProfession::None
                {
                    let r#type = self.villager_data.lock().await.type_enum();
                    self.set_villager_data(VillagerData::new(r#type, VillagerProfession::None, 1))
                        .await;
                }
            }
        }

        if self.get_job_site().is_none() {
            let profession = self.villager_data.lock().await.profession_enum();
            let expected = (profession != VillagerProfession::None).then_some(profession);
            let pos = self.get_entity().block_pos.load();
            let start = BlockPos::new(pos.0.x - 10, pos.0.y - 4, pos.0.z - 10);
            let end = BlockPos::new(pos.0.x + 10, pos.0.y + 4, pos.0.z + 10);
            let mut candidates = Vec::new();

            let indexed_sites = world
                .villager_poi
                .lock()
                .await
                .available_job_sites(pos, 48, expected);
            let saved_sites = world.portal_poi.lock().await.get_in_square(pos, 48, None);
            for position in indexed_sites.into_iter().chain(saved_sites) {
                let delta = position.0 - pos.0;
                if i64::from(delta.x).pow(2) + i64::from(delta.y).pow(2) + i64::from(delta.z).pow(2)
                    > 48i64.pow(2)
                    || candidates
                        .iter()
                        .any(|(_, candidate, _, _)| *candidate == position)
                {
                    continue;
                }
                let (block, _state) = world.get_block_and_state(&position);
                if let Some(site_profession) = profession_for_block(block)
                    && expected.is_none_or(|profession| profession == site_profession)
                {
                    let distance = position
                        .to_centered_f64()
                        .squared_distance_to_vec(&self.get_entity().pos.load());
                    candidates.push((distance, position, block, site_profession));
                }
            }

            for position in BlockPos::iterate(start, end) {
                let (block, _state) = world.get_block_and_state(&position);
                let Some(site_profession) = profession_for_block(block) else {
                    continue;
                };
                if expected.is_some_and(|profession| profession != site_profession)
                    || candidates
                        .iter()
                        .any(|(_, candidate, _, _)| *candidate == position)
                {
                    continue;
                }
                let distance = position
                    .to_centered_f64()
                    .squared_distance_to_vec(&self.get_entity().pos.load());
                candidates.push((distance, position, block, site_profession));
            }
            candidates.sort_by(|left, right| left.0.total_cmp(&right.0));

            let mut navigator = Navigator::default();
            let mut claimed = None;
            for (_, position, block, _) in candidates.into_iter().take(5) {
                if !navigator
                    .can_reach_within(
                        &self.mob_entity.living_entity,
                        position.to_centered_f64(),
                        1.73,
                    )
                    .await
                {
                    continue;
                }
                if world
                    .villager_poi
                    .lock()
                    .await
                    .claim(position, block, owner.clone(), expected)
                    .is_some()
                {
                    claimed = Some(position);
                    break;
                }
            }

            if let Some(site) = claimed {
                *self
                    .job_site
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(site);
                self.job_site_pending.store(true, Ordering::Relaxed);
            }
        }

        if self.job_site_pending.load(Ordering::Relaxed)
            && let Some(site) = self.get_job_site()
            && site
                .to_centered_f64()
                .squared_distance_to_vec(&self.get_entity().pos.load())
                < 2.0f64.powi(2)
        {
            let (block, _state) = world.get_block_and_state(&site);
            if let Some(claimed_profession) = profession_for_block(block) {
                let profession = self.villager_data.lock().await.profession_enum();
                if profession != VillagerProfession::None && profession != claimed_profession {
                    return;
                }
                world.send_entity_status(
                    self.get_entity(),
                    pumpkin_data::entity::EntityStatus::VillagerHappy,
                    Some(ActorEventType::VillagerHappy),
                );
                self.job_site_pending.store(false, Ordering::Relaxed);
                if profession == VillagerProfession::None {
                    let r#type = self.villager_data.lock().await.type_enum();
                    self.set_villager_data(VillagerData::new(r#type, claimed_profession, 1))
                        .await;
                }
            }
        }
    }

    pub fn set_unhappy(&self) {
        let entity = self.get_entity();
        self.unhappy_counter.store(40, Ordering::Relaxed);
        entity.send_meta_data(
            &[Metadata::new(
                tracked_data::villager::UNHAPPY_COUNTER,
                VarInt(40),
            )],
            None,
        );
        entity.world.load().send_entity_status(
            entity,
            pumpkin_data::entity::EntityStatus::VillagerAngry,
            Some(ActorEventType::VillagerAngry),
        );
        entity.play_sound(pumpkin_data::sound::Sound::EntityVillagerNo);
    }

    pub async fn open_trading_screen(&self, player: &Arc<Player>) {
        // Open the merchant screen and then send the current offers packet
        if let Some(sync_id) = player.open_handled_screen(self, None).await {
            let offers = self.offers.lock().await.clone();
            let villager_data = *self.villager_data.lock().await;
            self.send_trade_offers(player, sync_id, offers, villager_data)
                .await;
        }
    }

    /// Vanilla `LivingEntity::stopSleeping`, invoked by `BedBlock::kickVillagerOutOfBed`
    /// when a player uses an occupied bed: un-occupies the bed and stands the villager up.
    pub async fn stop_sleeping(&self, world: &Arc<World>) {
        let home_pos = *self.home_pos.lock().unwrap();
        let mut stand_up_position = None;
        if let Some(home_pos) = home_pos {
            let (block, state) = world.get_block_and_state(&home_pos);
            if block.has_tag(&pumpkin_data::tag::Block::MINECRAFT_BEDS) {
                let bed_props = BedProperties::from_state_id(state.id, block);
                if bed_props.occupied {
                    stand_up_position = BedBlock::find_stand_up_position(
                        world,
                        &home_pos,
                        bed_props.facing.to_block_direction(),
                        self.get_entity().yaw.load(),
                        &EntityType::VILLAGER,
                    );
                    BedBlock::set_occupied(false, world, block, &home_pos, state.id).await;
                }
            }
        }

        if let Some(position) = stand_up_position {
            self.get_entity().set_pos(position);
        }
        self.get_entity().set_pose(EntityPose::Standing);
        self.get_entity().send_meta_data(
            &[Metadata::new(
                pumpkin_data::tracked_data::villager::SLEEPING_POS_ID,
                None::<BlockPos>,
            )],
            None,
        );
    }

    /// Vanilla `Villager::wantsToSpawnGolem` + `golemSpawnConditionsMet`
    /// (`Villager.java:850-852, 896-899`). Despite the "panicking neighbors" framing in
    /// vanilla's constant names, the actual gate checked in code is "slept within the last
    /// in-game day" (`LAST_SLEPT` recency), not a live panic flag -- Pumpkin has no brain/
    /// panic-activity system, so this is ported exactly as read from the cited lines rather
    /// than inventing a stricter panic-based gate.
    #[must_use]
    pub fn wants_to_spawn_golem(&self, world_age: i64) -> bool {
        golem_spawn_conditions_met(
            self.last_slept_time.load(Ordering::Relaxed),
            self.golem_detected_until.load(Ordering::Relaxed),
            world_age,
        )
    }

    /// Vanilla `Villager::spawnGolemIfNeeded` (`Villager.java:834-848`), called from
    /// `Villager::gossip` after a successful exchange. `on_damage` also retains the existing
    /// crisis-trigger approximation; removing that separate trigger is a follow-up.
    pub async fn spawn_golem_if_needed(
        &self,
        world: &Arc<World>,
        world_age: i64,
        villagers_needed: usize,
    ) {
        if !self.wants_to_spawn_golem(world_age) {
            return;
        }

        let pos = self.get_entity().pos.load();
        let aabb = BoundingBox::new(
            Vector3::new(pos.x - 10.0, pos.y - 10.0, pos.z - 10.0),
            Vector3::new(pos.x + 10.0, pos.y + 10.0, pos.z + 10.0),
        );

        // Vanilla's `.limit(5L)` -- a hard cap regardless of `villagers_needed` (`Villager.java:838`).
        let agreeing = world
            .get_all_at_box(&aabb)
            .into_iter()
            .filter(|e| {
                e.get_entity().entity_type == &EntityType::VILLAGER
                    && e.cast_any()
                        .downcast_ref::<Self>()
                        .is_some_and(|v| v.wants_to_spawn_golem(world_age))
            })
            .count()
            .min(5);
        if agreeing < villagers_needed {
            return;
        }

        // `SpawnUtil.trySpawnMob(IRON_GOLEM, ..., 10, 8, 6, ...)` (`Villager.java:840-842`):
        // vanilla searches a 10-horizontal/8-up/6-down radius for a valid position. The exact
        // placement-validity rules inside `SpawnUtil.trySpawnMob` were not read for this pass
        // (flagged in the design doc as needing follow-up); this uses the simplest possible
        // approximation available from existing Rust infrastructure -- `World::is_space_empty`
        // for a golem-sized bounding box plus a solid-ground check directly below -- searched
        // over a small horizontal ring at the villager's own height, rather than porting
        // `SpawnUtil`'s full search shape.
        let block_pos = self.get_entity().block_pos.load();
        let mut spawn_pos = None;
        'search: for dx in -3..=3i32 {
            for dz in -3..=3i32 {
                let candidate =
                    BlockPos::new(block_pos.0.x + dx, block_pos.0.y, block_pos.0.z + dz);
                let below = candidate.down();
                let (_, below_state) = world.get_block_and_state(&below);
                if !below_state.is_solid() {
                    continue;
                }
                let feet = candidate.to_f64();
                let check_box = BoundingBox::new(
                    Vector3::new(feet.x - 0.7, feet.y, feet.z - 0.7),
                    Vector3::new(feet.x + 0.7, feet.y + 2.7, feet.z + 0.7),
                );
                if world.is_space_empty(check_box) {
                    spawn_pos = Some(candidate);
                    break 'search;
                }
            }
        }

        let Some(spawn_pos) = spawn_pos else {
            return;
        };

        let entity = Entity::new(
            world.clone(),
            spawn_pos.to_centered_f64(),
            &EntityType::IRON_GOLEM,
        );
        let golem = crate::entity::passive::iron_golem::IronGolemEntity::new(entity);
        world.spawn_entity(golem).await;

        // `nearbyVillagers.forEach(GolemSensor::golemDetected)` (`Villager.java:844`) --
        // every villager in the *unfiltered* nearby list is suppressed, not just the ones
        // that agreed. Re-scan since `nearby_villagers` above was already filtered.
        for entity in world.get_all_at_box(&aabb) {
            if entity.get_entity().entity_type == &EntityType::VILLAGER
                && let Some(villager) = entity.cast_any().downcast_ref::<Self>()
            {
                villager
                    .golem_detected_until
                    .store(world_age + 599, Ordering::Relaxed);
            }
        }
    }

    /// Vanilla `Villager::gossip` (`Villager.java:814-822`): transfer a weighted-random sample
    /// of `target`'s gossip into `self`, gated by a 1200-tick cooldown on both villagers.
    ///
    /// Vanilla invokes this from `TradeWithVillager.tick` during the MEET activity after its
    /// interaction target is within `distanceToSqr <= 5.0` and visible. Pumpkin does not yet
    /// have the Brain interaction-target/sensor graph, so `mob_tick` supplies those conditions
    /// with the existing MEET schedule, the same distance threshold, and a block raycast.
    /// Gossip mutexes are acquired in entity-id order because both villagers can run this
    /// symmetric check concurrently; neither mutex is held across an await.
    pub async fn gossip_with(&self, world: &Arc<World>, target: &Self, timestamp: i64) {
        let self_id = self.get_entity().entity_id;
        let target_id = target.get_entity().entity_id;
        if self_id == target_id {
            return;
        }

        let self_last = self.last_gossip_time.load(Ordering::Relaxed);
        let target_last = target.last_gossip_time.load(Ordering::Relaxed);
        if !gossip_cooldown_ready(self_last, timestamp)
            || !gossip_cooldown_ready(target_last, timestamp)
        {
            return;
        }

        // ThreadRng is !Send on this rand version. Keep it and both mutex guards inside a
        // synchronous scope so the mob-tick future never carries them across the next await.
        if self_id < target_id {
            {
                let mut self_gossips = self.gossips.lock().await;
                let target_gossips = target.gossips.lock().await;
                let mut rng = rand::rng();
                self_gossips.transfer_from(&target_gossips, &mut rng, 10);
            }
        } else {
            {
                let target_gossips = target.gossips.lock().await;
                let mut self_gossips = self.gossips.lock().await;
                let mut rng = rand::rng();
                self_gossips.transfer_from(&target_gossips, &mut rng, 10);
            }
        }

        self.last_gossip_time.store(timestamp, Ordering::Relaxed);
        target.last_gossip_time.store(timestamp, Ordering::Relaxed);

        // `Villager::gossip` unconditionally follows a successful transfer with this call.
        self.spawn_golem_if_needed(world, timestamp, 5).await;
    }

    /// Vanilla `Villager::restock` (`Villager.java:365-375`): recompute demand for every
    /// offer and reset its use counter. Does not resend the updated offers to a currently
    /// trading player -- vanilla's `resendOffersToTradingPlayer` (`Villager.java:377-385`)
    /// has no Rust equivalent since `VillagerEntity` doesn't track a persistent
    /// "currently trading player" handle; a player with the trade screen already open will
    /// see the new prices next time they reopen it. Documented deviation, not silently
    /// dropped.
    pub async fn restock(&self, world_age: i64) {
        let mut offers = self.offers.lock().await;
        for offer in offers.iter_mut() {
            offer.update_demand();
            offer.uses = 0;
        }
        drop(offers);
        self.last_restock_time.store(world_age, Ordering::Relaxed);
        self.restocks_today.fetch_add(1, Ordering::Relaxed);
    }

    /// Vanilla `Villager::shouldRestock`/`needsToRestock` (`Villager.java:387-419`),
    /// approximated: a villager may restock up to twice per in-game day, at least
    /// 12000 ticks (half a day) apart, whenever any offer has been used at least once.
    /// Vanilla's exact `isNewDay` check against a `Timelines.OVERWORLD_DAY` clock was not
    /// read for this pass (flagged in the design doc); this derives the day boundary from
    /// `world_age / 24000`, which is the same 24000-tick day length already used elsewhere
    /// in this file (gossip decay), and resets the daily restock counter on a day rollover.
    pub async fn maybe_restock(&self, world_age: i64) {
        let last_restock = self.last_restock_time.load(Ordering::Relaxed);
        if is_new_restock_day(last_restock, world_age) {
            self.restocks_today.store(0, Ordering::Relaxed);
        }

        if !restock_is_due(
            last_restock,
            self.restocks_today.load(Ordering::Relaxed),
            world_age,
        ) {
            return;
        }

        let needs_restock = {
            let offers = self.offers.lock().await;
            offers.iter().any(|o| o.uses > 0)
        };
        if needs_restock {
            self.restock(world_age).await;
        }
    }
}

/// Vanilla `Villager::golemSpawnConditionsMet` (`Villager.java:896-899`), extracted as a
/// pure function for unit testing.
#[must_use]
const fn golem_spawn_conditions_met(
    last_slept: i64,
    golem_detected_until: i64,
    world_age: i64,
) -> bool {
    if last_slept == 0 || world_age - last_slept >= 24000 {
        return false;
    }
    golem_detected_until == 0 || world_age >= golem_detected_until
}

/// Vanilla `Villager::shouldRestock`'s half-day check: the restock counter resets once more than
/// 12000 ticks have passed since the last restock, not on a 24000-tick calendar boundary. Vanilla
/// additionally resets on a day rollover reported by `Timelines.OVERWORLD_DAY`, which has no
/// counterpart here; the half-day window fires first in every ordinary case.
#[must_use]
const fn is_new_restock_day(last_restock: i64, world_age: i64) -> bool {
    last_restock != 0 && world_age > last_restock + 12000
}

/// Vanilla `Villager::shouldRestock` (`Villager.java:387-419`) gate (minus the `needsRestock`
/// per-offer check, which needs the offers list and stays in `maybe_restock`), extracted as a
/// pure function for unit testing.
#[must_use]
const fn restock_is_due(last_restock: i64, restocks_today: i32, world_age: i64) -> bool {
    // `allowedToRestock`: the first restock after a counter reset has no cooldown at all, and the
    // second needs only 2400 ticks. Requiring half a day for the first one meant a villager whose
    // trades were used up stayed sold out until the next in-game morning.
    restocks_today == 0 || (restocks_today < 2 && world_age > last_restock + 2400)
}

impl VillagerEntity {
    fn bedrock_trade_item(stack: &ItemStack, count: u8) -> NbtCompound {
        let mut item = NbtCompound::new();
        if stack.is_empty() {
            return item;
        }
        let Some(mapping) = JavaToBedrockItemMapping::from_java_item_id(stack.item.id) else {
            return item;
        };
        item.put_byte("Count", count as i8);
        item.put_short("Damage", mapping.bedrock_data as i16);
        item.put_string("Name", mapping.bedrock_item.registry_key.to_owned());
        item
    }

    fn bedrock_trade_data(
        offers: &[pumpkin_protocol::java::client::play::MerchantOffer],
        level: i32,
    ) -> NbtCompound {
        use pumpkin_nbt::tag::NbtTag;

        let tier = level.saturating_sub(1).max(0);
        let mut recipes = Vec::with_capacity(offers.len() + usize::from(level < 5));
        for (index, offer) in offers.iter().enumerate() {
            let base_cost = &offer.base_cost_a.0;
            let demand_bonus = (i32::from(base_cost.item_count).saturating_mul(offer.demand) as f32
                * offer.price_multiplier)
                .floor()
                .max(0.0) as i32;
            let adjusted_count = i32::from(base_cost.item_count)
                .saturating_add(demand_bonus)
                .saturating_add(offer.special_price)
                .clamp(1, i32::from(base_cost.get_max_stack_size()))
                as u8;

            let mut recipe = NbtCompound::new();
            recipe.put_int("netId", index as i32 + 1);
            recipe.put_int(
                "maxUses",
                if offer.is_out_of_stock() {
                    0
                } else {
                    offer.max_uses
                },
            );
            recipe.put_int("traderExp", offer.xp);
            recipe.put_float("priceMultiplierA", offer.price_multiplier);
            recipe.put_float("priceMultiplierB", 0.0);
            recipe.put_compound(
                "sell",
                Self::bedrock_trade_item(&offer.output.0, offer.output.0.item_count),
            );
            recipe.put_int("buyCountA", i32::from(base_cost.item_count));
            recipe.put_int(
                "buyCountB",
                offer
                    .cost_b
                    .as_ref()
                    .map_or(0, |cost| i32::from(cost.0.item_count)),
            );
            recipe.put_int("demand", offer.demand);
            recipe.put_int("tier", (index as i32 / 2).min(tier));
            recipe.put_compound("buyA", Self::bedrock_trade_item(base_cost, adjusted_count));
            recipe.put_compound(
                "buyB",
                offer.cost_b.as_ref().map_or_else(NbtCompound::new, |cost| {
                    Self::bedrock_trade_item(&cost.0, cost.0.item_count)
                }),
            );
            recipe.put_int("uses", offer.uses);
            recipe.put_byte("rewardExp", i8::from(offer.reward_exp));
            recipes.push(NbtTag::Compound(recipe));
        }

        // Bedrock uses this hidden next-tier entry to render the villager XP bar.
        if level < 5 {
            let mut recipe = NbtCompound::new();
            recipe.put_int("maxUses", 0);
            recipe.put_int("traderExp", 0);
            recipe.put_float("priceMultiplierA", 0.0);
            recipe.put_float("priceMultiplierB", 0.0);
            recipe.put_int("buyCountA", 0);
            recipe.put_int("buyCountB", 0);
            recipe.put_int("demand", 0);
            recipe.put_int("tier", 5);
            recipe.put_int("uses", 0);
            recipe.put_byte("rewardExp", 0);
            recipes.push(NbtTag::Compound(recipe));
        }

        let mut data = NbtCompound::new();
        data.put_list("Recipes", recipes);
        data.put_list(
            "TierExpRequirements",
            [0, 10, 70, 150, 250]
                .into_iter()
                .enumerate()
                .map(|(tier, xp)| {
                    let mut requirement = NbtCompound::new();
                    requirement.put_int(&tier.to_string(), xp);
                    NbtTag::Compound(requirement)
                })
                .collect(),
        );
        data
    }

    async fn send_trade_offers(
        &self,
        player: &Player,
        sync_id: u8,
        offers: Vec<pumpkin_protocol::java::client::play::MerchantOffer>,
        villager_data: VillagerData,
    ) {
        use pumpkin_protocol::{bedrock::client::CUpdateTrade, codec::var_long::VarLong};

        let java = CMerchantOffers::new(
            VarInt(i32::from(sync_id)),
            offers.clone(),
            villager_data.level,
            VarInt(self.xp.load(Ordering::Relaxed)),
            true,
            true,
        );
        let bedrock = CUpdateTrade {
            container_id: sync_id,
            r#type: 15,
            size: VarInt(0),
            trader_tier: VarInt(villager_data.level.0.saturating_sub(1)),
            entity_unique_id: VarLong(i64::from(self.get_entity().entity_id)),
            last_trading_player: VarLong(i64::from(player.entity_id())),
            display_name: ScreenHandlerFactory::get_display_name(self).to_pretty_console(),
            use_new_trade_screen: true,
            using_economy_trade: true,
            data: Self::bedrock_trade_data(&offers, villager_data.level.0),
        };
        player
            .client
            .enqueue_packet_editioned(&java, &bedrock)
            .await;
    }
}

impl ScreenHandlerFactory for VillagerEntity {
    #[allow(clippy::too_many_lines)]
    fn create_screen_handler<'a>(
        &'a self,
        sync_id: u8,
        player_inventory: &'a Arc<pumpkin_inventory::player::player_inventory::PlayerInventory>,
        player: &'a dyn InventoryPlayer,
    ) -> BoxFuture<'a, Option<SharedScreenHandler>> {
        Box::pin(async move {
            let self_weak = self
                .self_weak
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()?;
            let server_player = player.as_any().downcast_ref::<Player>();
            let player_uuid =
                server_player.map_or_else(uuid::Uuid::nil, |p| p.get_entity().entity_uuid);
            if let Some(player) = server_player {
                self.update_special_prices(player).await;
            }
            let offers = self.offers.lock().await;
            let world = self.get_entity().world.load().clone();

            let mut handler = MerchantScreenHandler::new(
                sync_id,
                player_inventory,
                self.merchant_inventory.clone(),
                offers.clone(),
            )
            .await;

            self.is_trading.store(true, Ordering::Relaxed);
            *self
                .trading_player
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some((player_uuid, sync_id));
            let validity_weak = self_weak.clone();
            handler.validity_check = Some(Box::new(move |inventory_player| {
                validity_weak.upgrade().is_some_and(|villager| {
                    villager.can_continue_trading(inventory_player, player_uuid, sync_id)
                })
            }));
            let update_weak = self_weak.clone();
            handler.on_trade_updated = Some(Box::new(move |has_result| {
                let Some(villager) = update_weak.upgrade() else {
                    return;
                };
                if villager
                    .trade_sound_cooldown
                    .compare_exchange(0, 20, Ordering::Relaxed, Ordering::Relaxed)
                    .is_ok()
                {
                    villager.get_entity().play_sound(if has_result {
                        pumpkin_data::sound::Sound::EntityVillagerYes
                    } else {
                        pumpkin_data::sound::Sound::EntityVillagerNo
                    });
                }
            }));
            let close_weak = self_weak.clone();
            handler.on_close = Some(Box::new(move || {
                let close_weak = close_weak.clone();
                Box::pin(async move {
                    if let Some(villager) = close_weak.upgrade() {
                        villager.is_trading.store(false, Ordering::Relaxed);
                        *villager
                            .trading_player
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
                        villager.reset_special_prices().await;
                    }
                })
            }));

            handler.on_trade = Some(Box::new(move |offer_index| {
                let self_weak = self_weak.clone();
                let world = world.clone();
                Box::pin(async move {
                    if let Some(villager) = self_weak.upgrade() {
                        villager
                            .complete_trade(offer_index, &world, player_uuid)
                            .await;
                    }
                })
            }));

            Some(Arc::new(Mutex::new(handler)) as SharedScreenHandler)
        })
    }

    fn get_display_name(&self) -> TextComponent {
        let profession = self
            .villager_data
            .try_lock()
            .map_or(VillagerProfession::None, |data| data.profession_enum());
        TextComponent::translate(profession.translation_key(), [])
    }
}

/// Vanilla `Villager` is an `AgeableMob`; babies produced by `VillagerMakeLove` start at
/// -24000 and grow up on the shared clock.
impl AgeableMob for VillagerEntity {
    fn get_ageable_data(&self) -> &AgeableData {
        &self.ageable_data
    }

    /// `Villager.BABY_DIMENSIONS` (`Villager.java:116`):
    /// `EntityDimensions.scalable(0.49F, 0.98F).withEyeHeight(0.63F)`.
    fn baby_dimensions(&self) -> Option<pumpkin_util::math::boundingbox::EntityDimensions> {
        Some(pumpkin_util::math::boundingbox::EntityDimensions::new(
            0.49, 0.98, 0.63,
        ))
    }
}

/// `Villager.canBreed` (`Villager.java:645-647`), as a pure predicate.
///
/// Note the age test is `== 0`, not `>= 0`: a villager inside its post-breed cooldown (age
/// 6000, counting down) must not immediately breed again, which is what the generic
/// `Mob::is_breeding_ready` (`age >= 0`) would allow.
#[must_use]
pub const fn can_breed_from(
    food_level: i32,
    inventory_food_points: i32,
    sleeping: bool,
    age: i32,
) -> bool {
    age == 0 && !sleeping && food_level + inventory_food_points >= BREEDING_FOOD_THRESHOLD
}

/// `Villager.getBreedOffspring`'s type roll (`Villager.java:739-747`).
#[must_use]
pub const fn breed_offspring_type(
    roll: f64,
    biome_type: VillagerType,
    own_type: VillagerType,
    partner_type: VillagerType,
) -> VillagerType {
    if roll < 0.5 {
        biome_type
    } else if roll < 0.75 {
        own_type
    } else {
        partner_type
    }
}

impl VillagerEntity {
    /// `Villager.canBreed` (`Villager.java:645-647`).
    pub async fn can_breed_villager(&self) -> bool {
        can_breed_from(
            self.food_level.load(Ordering::Relaxed),
            self.count_food_points_in_inventory().await,
            self.get_entity().pose.load() == EntityPose::Sleeping,
            self.get_entity().age.load(Ordering::Relaxed),
        )
    }

    /// `Villager.eatAndDigestFood` (`Villager.java:683-686`): top the food bar up from the
    /// inventory, then spend 12 points on the breed.
    pub async fn eat_and_digest_food(&self) {
        self.eat_until_full().await;
        self.food_level
            .fetch_sub(BREEDING_FOOD_THRESHOLD, Ordering::Relaxed);
    }

    /// `Villager.hasExcessFood` (`Villager.java:784-786`).
    pub async fn has_excess_food(&self) -> bool {
        self.count_food_points_in_inventory().await >= 2 * BREEDING_FOOD_THRESHOLD
    }

    /// `Villager.wantsMoreFood` (`Villager.java:788-790`).
    pub async fn wants_more_food(&self) -> bool {
        self.count_food_points_in_inventory().await < BREEDING_FOOD_THRESHOLD
    }

    pub(super) fn send_breeding_event(&self, status: pumpkin_data::entity::EntityStatus) {
        let world = self.get_entity().world.load();
        let bedrock = match status {
            pumpkin_data::entity::EntityStatus::InLoveHearts => Some(ActorEventType::InLoveHearts),
            pumpkin_data::entity::EntityStatus::LoveHearts => Some(ActorEventType::LoveHearts),
            pumpkin_data::entity::EntityStatus::VillagerAngry => {
                Some(ActorEventType::VillagerAngry)
            }
            _ => None,
        };
        world.send_entity_status(self.get_entity(), status, bedrock);
    }

    /// `Villager.getBreedOffspring` (`Villager.java:738-753`): the child's type is the local
    /// biome's half the time, otherwise one parent's, and it is always unemployed at level 1.
    async fn breed_offspring_data(&self, partner: &Self) -> VillagerData {
        use rand::RngExt;
        let roll = rand::rng().random::<f64>();
        let r#type = breed_offspring_type(
            roll,
            villager_type_at(self.get_entity()),
            self.villager_data.lock().await.type_enum(),
            partner.villager_data.lock().await.type_enum(),
        );
        VillagerData::new(r#type, VillagerProfession::None, 1)
    }

    /// `VillagerMakeLove.tryToGiveBirth` (`VillagerMakeLove.java:64-78`) plus its `breed` and
    /// `giveBedToChild` helpers. Returns `true` when a baby was actually spawned; a village
    /// with no vacant bed within 48 blocks produces entity event 13 and no child, which is
    /// the mechanic every villager breeder depends on.
    pub async fn try_to_give_birth(&self, partner: &Self) -> bool {
        // Both parents run their own copy of the breeding goal, so claim the birth by moving
        // both ages 0 -> 6000 atomically (`VillagerMakeLove.java:107-108`) before doing
        // anything else. Vanilla gets the same exclusion for free because the loser's
        // `canStillUse` re-check sees the age it just lost; here the two villagers can tick
        // concurrently, so the claim has to be a compare-exchange rather than a store.
        if self
            .get_entity()
            .age
            .compare_exchange(0, 6000, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return false;
        }
        if partner
            .get_entity()
            .age
            .compare_exchange(0, 6000, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            self.get_entity().age.store(0, Ordering::Release);
            return false;
        }

        let world = self.get_entity().world.load();
        let Some(bed) = world
            .acquire_poi(
                crate::world::village_poi::POI_TYPE_HOME,
                self.get_entity().block_pos.load(),
                48,
            )
            .await
        else {
            // `VillagerMakeLove.tryToGiveBirth` only broadcasts event 13 here and never
            // reaches `breed`, so neither parent takes the post-breed cooldown: release the
            // claim so the pair can try again once a bed frees up.
            self.release_birth_claim(partner);
            self.send_breeding_event(pumpkin_data::entity::EntityStatus::VillagerAngry);
            partner.send_breeding_event(pumpkin_data::entity::EntityStatus::VillagerAngry);
            return false;
        };

        let data = self.breed_offspring_data(partner).await;
        let pos = self.get_entity().pos.load();
        let baby =
            crate::entity::r#type::from_type(&EntityType::VILLAGER, pos, &world, Uuid::new_v4());
        let Some(baby_villager) = baby.cast_any().downcast_ref::<Self>() else {
            // `VillagerMakeLove.java:73-76`: a null child releases the bed ticket again.
            world.release_poi(bed).await;
            self.release_birth_claim(partner);
            return false;
        };

        baby_villager.set_villager_data(data).await;
        baby_villager.set_age(crate::entity::ageable::BABY_START_AGE);
        *baby_villager
            .home_pos
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(bed);

        world.spawn_entity(baby.clone()).await;
        baby_villager.send_breeding_event(pumpkin_data::entity::EntityStatus::LoveHearts);
        true
    }

    fn release_birth_claim(&self, partner: &Self) {
        self.get_entity().age.store(0, Ordering::Release);
        partner.get_entity().age.store(0, Ordering::Release);
    }

    /// `InventoryCarrier.pickUpItem` -> `SimpleContainer.addItem`, over the villager's own
    /// eight-slot inventory. A contended inventory lock simply declines the pickup this tick.
    fn add_to_inventory(&self, stack: &ItemStack) -> u8 {
        let Ok(inventory) = self.inventory.try_lock() else {
            return 0;
        };
        let mut taken = 0u8;
        let mut remaining = stack.item_count;
        for slot in inventory.iter() {
            if remaining == 0 {
                break;
            }
            let Ok(mut existing) = slot.try_lock() else {
                continue;
            };
            if existing.is_empty() {
                let mut new_stack = stack.clone();
                new_stack.item_count = remaining;
                *existing = new_stack;
                taken += remaining;
                remaining = 0;
            } else if existing.are_items_and_components_equal(stack) {
                let space = existing
                    .get_max_stack_size()
                    .saturating_sub(existing.item_count);
                let moved = space.min(remaining);
                existing.item_count += moved;
                taken += moved;
                remaining -= moved;
            }
        }
        taken
    }
}

impl NBTStorage for VillagerEntity {
    #[expect(clippy::too_many_lines)]
    fn write_nbt<'a>(&'a self, nbt: &'a mut NbtCompound) -> crate::entity::NbtFuture<'a, ()> {
        Box::pin(async move {
            self.mob_entity.living_entity.write_nbt(nbt).await;
            let data = self.villager_data.lock().await;
            let mut villager_data_nbt = NbtCompound::new();
            villager_data_nbt.put_int("Type", data.r#type.0);
            villager_data_nbt.put_int("Profession", data.profession.0);
            villager_data_nbt.put_int("Level", data.level.0);
            nbt.put_compound("VillagerData", villager_data_nbt);

            self.write_ageable_nbt(nbt);
            nbt.put_int("FoodLevel", self.food_level.load(Ordering::Relaxed));
            nbt.put_int("Xp", self.xp.load(Ordering::Relaxed));
            nbt.put_long(
                "LastRestock",
                self.last_restock_time.load(Ordering::Relaxed),
            );
            nbt.put_int("RestocksToday", self.restocks_today.load(Ordering::Relaxed));
            nbt.put_long(
                "LastGossipDecay",
                self.last_gossip_decay_time.load(Ordering::Relaxed),
            );

            let job_site_pos = *self
                .job_site
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(pos) = job_site_pos {
                nbt.put_int("JobSiteX", pos.0.x);
                nbt.put_int("JobSiteY", pos.0.y);
                nbt.put_int("JobSiteZ", pos.0.z);
                nbt.put_bool(
                    "JobSitePending",
                    self.job_site_pending.load(Ordering::Relaxed),
                );
            }

            let home_pos = *self
                .home_pos
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(pos) = home_pos {
                nbt.put_int("HomeX", pos.0.x);
                nbt.put_int("HomeY", pos.0.y);
                nbt.put_int("HomeZ", pos.0.z);
            }

            let meeting_pos = *self.meeting_point.lock().unwrap();
            if let Some(pos) = meeting_pos {
                nbt.put_int("MeetingX", pos.0.x);
                nbt.put_int("MeetingY", pos.0.y);
                nbt.put_int("MeetingZ", pos.0.z);
            }

            // Save Offers
            {
                let offers = self.offers.lock().await;
                let mut recipes = Vec::new();
                for offer in offers.iter() {
                    let mut recipe = NbtCompound::new();

                    let mut buy = NbtCompound::new();
                    offer.base_cost_a.0.write_item_stack(&mut buy);
                    recipe.put_compound("buy", buy);

                    if let Some(cost_b) = &offer.cost_b
                        && !cost_b.0.is_empty()
                    {
                        let mut buy_b = NbtCompound::new();
                        cost_b.0.write_item_stack(&mut buy_b);
                        recipe.put_compound("buyB", buy_b);
                    }

                    let mut sell_item = NbtCompound::new();
                    offer.output.0.write_item_stack(&mut sell_item);
                    recipe.put_compound("sell", sell_item);

                    recipe.put_int("uses", offer.uses);
                    recipe.put_int("maxUses", offer.max_uses);
                    recipe.put_bool("rewardExp", offer.reward_exp);
                    recipe.put_int("xp", offer.xp);
                    recipe.put_float("priceMultiplier", offer.price_multiplier);
                    recipe.put_int("specialPrice", offer.special_price);
                    recipe.put_int("demand", offer.demand);

                    recipes.push(pumpkin_nbt::tag::NbtTag::Compound(recipe));
                }
                let mut offers_compound = NbtCompound::new();
                offers_compound.put("Recipes", pumpkin_nbt::tag::NbtTag::List(recipes));
                nbt.put_compound("Offers", offers_compound);
            };

            // Inventory
            let inventory = self.inventory.lock().await;
            let mut inventory_list = Vec::new();
            for stack_mutex in inventory.iter() {
                let stack = stack_mutex.lock().await;
                if !stack.is_empty() {
                    let mut item_nbt = NbtCompound::new();
                    stack.write_item_stack(&mut item_nbt);
                    inventory_list.push(pumpkin_nbt::tag::NbtTag::Compound(item_nbt));
                }
            }
            nbt.put("Inventory", pumpkin_nbt::tag::NbtTag::List(inventory_list));

            // Gossips
            let gossips = self.gossips.lock().await;
            let mut gossip_list = Vec::new();
            for (uuid, types) in gossips.raw() {
                for (gtype, value) in types {
                    let mut gossip_nbt = NbtCompound::new();
                    let uuid_val = uuid.as_u128();
                    gossip_nbt.put(
                        "Target",
                        pumpkin_nbt::tag::NbtTag::IntArray(vec![
                            (uuid_val >> 96) as i32,
                            ((uuid_val >> 64) & 0xFFFF_FFFF) as i32,
                            ((uuid_val >> 32) & 0xFFFF_FFFF) as i32,
                            (uuid_val & 0xFFFF_FFFF) as i32,
                        ]),
                    );
                    gossip_nbt.put_string("Type", gtype.name().to_owned());
                    gossip_nbt.put_int("Value", *value);
                    gossip_list.push(pumpkin_nbt::tag::NbtTag::Compound(gossip_nbt));
                }
            }
            nbt.put("Gossips", pumpkin_nbt::tag::NbtTag::List(gossip_list));
        })
    }

    #[allow(clippy::too_many_lines)]
    fn read_nbt_non_mut<'a>(&'a self, nbt: &'a NbtCompound) -> crate::entity::NbtFuture<'a, ()> {
        Box::pin(async move {
            self.mob_entity.living_entity.read_nbt_non_mut(nbt).await;
            if let Some(villager_data_nbt) = nbt.get_compound("VillagerData") {
                let mut data = self.villager_data.lock().await;
                if let Some(t) = villager_data_nbt.get_int("Type") {
                    data.r#type = VarInt(t);
                }
                if let Some(p) = villager_data_nbt.get_int("Profession") {
                    data.profession = VarInt(p);
                }
                if let Some(l) = villager_data_nbt.get_int("Level") {
                    data.level = VarInt(l);
                }
            }

            self.read_ageable_nbt(nbt);
            if let Some(food) = nbt.get_int("FoodLevel") {
                self.food_level.store(food, Ordering::Relaxed);
            }
            if let Some(xp) = nbt.get_int("Xp") {
                self.xp.store(xp, Ordering::Relaxed);
            }
            if let Some(restock) = nbt.get_long("LastRestock") {
                self.last_restock_time.store(restock, Ordering::Relaxed);
            }
            if let Some(today) = nbt.get_int("RestocksToday") {
                self.restocks_today.store(today, Ordering::Relaxed);
            }
            if let Some(last_decay) = nbt.get_long("LastGossipDecay") {
                self.last_gossip_decay_time
                    .store(last_decay, Ordering::Relaxed);
            }

            if let (Some(x), Some(y), Some(z)) = (
                nbt.get_int("JobSiteX"),
                nbt.get_int("JobSiteY"),
                nbt.get_int("JobSiteZ"),
            ) {
                *self
                    .job_site
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) =
                    Some(BlockPos::new(x, y, z));
                self.job_site_pending.store(
                    nbt.get_bool("JobSitePending").unwrap_or(false),
                    Ordering::Relaxed,
                );
            } else {
                *self
                    .job_site
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
                self.job_site_pending.store(false, Ordering::Relaxed);
            }

            if let (Some(x), Some(y), Some(z)) = (
                nbt.get_int("HomeX").or_else(|| nbt.get_int("BedX")),
                nbt.get_int("HomeY").or_else(|| nbt.get_int("BedY")),
                nbt.get_int("HomeZ").or_else(|| nbt.get_int("BedZ")),
            ) {
                *self
                    .home_pos
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) =
                    Some(BlockPos::new(x, y, z));
            } else {
                *self
                    .home_pos
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
            }

            if let (Some(x), Some(y), Some(z)) = (
                nbt.get_int("MeetingX"),
                nbt.get_int("MeetingY"),
                nbt.get_int("MeetingZ"),
            ) {
                *self.meeting_point.lock().unwrap() = Some(BlockPos::new(x, y, z));
            } else {
                *self.meeting_point.lock().unwrap() = None;
            }

            if let Some(offers_compound) = nbt.get_compound("Offers")
                && let Some(recipes) = offers_compound.get_list("Recipes")
            {
                let mut offers = self.offers.lock().await;
                offers.clear();
                for tag in recipes {
                    if let Some(recipe) = tag.extract_compound() {
                        let buy = recipe
                            .get_compound("buy")
                            .and_then(ItemStack::read_item_stack);
                        let buy_b = recipe
                            .get_compound("buyB")
                            .and_then(ItemStack::read_item_stack);
                        let sell_item = recipe
                            .get_compound("sell")
                            .and_then(ItemStack::read_item_stack);

                        if let (Some(buy), Some(sell_item)) = (buy, sell_item)
                            && !buy.is_empty()
                            && !sell_item.is_empty()
                            && buy_b.as_ref().is_none_or(|stack| !stack.is_empty())
                        {
                            let uses = recipe.get_int("uses").unwrap_or(0);
                            let max_uses = recipe.get_int("maxUses").unwrap_or(12);
                            let reward_exp = recipe.get_bool("rewardExp").unwrap_or(true);
                            let xp = recipe.get_int("xp").unwrap_or(2);
                            let price_multiplier =
                                recipe.get_float("priceMultiplier").unwrap_or(0.05);
                            let special_price = recipe.get_int("specialPrice").unwrap_or(0);
                            let demand = recipe.get_int("demand").unwrap_or(0);

                            offers.push(pumpkin_protocol::java::client::play::MerchantOffer {
                                base_cost_a: buy.into(),
                                output: sell_item.into(),
                                cost_b: buy_b.map(Into::into),
                                reward_exp,
                                uses,
                                max_uses,
                                xp,
                                special_price,
                                price_multiplier,
                                demand,
                            });
                        }
                    }
                }
            }

            // Inventory
            if let Some(inventory_list) = nbt.get_list("Inventory") {
                let mut inventory = self.inventory.lock().await;
                inventory.clear();
                for tag in inventory_list {
                    if let Some(item_compound) = tag.extract_compound()
                        && let Some(stack) = ItemStack::read_item_stack(item_compound)
                    {
                        inventory.push(Arc::new(Mutex::new(stack)));
                    }
                }
            }

            // Gossips
            if let Some(gossip_list) = nbt.get_list("Gossips") {
                let mut raw: HashMap<Uuid, HashMap<GossipType, i32>> = HashMap::new();
                for tag in gossip_list {
                    if let Some(gossip_nbt) = tag.extract_compound() {
                        let uuid = gossip_nbt.get_int_array("Target").map(|uuid_array| {
                            Uuid::from_u128(
                                (uuid_array[0] as u128) << 96
                                    | (uuid_array[1] as u128) << 64
                                    | (uuid_array[2] as u128) << 32
                                    | (uuid_array[3] as u128),
                            )
                        });
                        let gossip_type = gossip_nbt
                            .get_string("Type")
                            .and_then(GossipType::from_name)
                            .or_else(|| {
                                gossip_nbt
                                    .get_int("Type")
                                    .and_then(GossipType::from_legacy_id)
                            });
                        if let (Some(uuid), Some(gossip_type), Some(val)) =
                            (uuid, gossip_type, gossip_nbt.get_int("Value"))
                        {
                            raw.entry(uuid).or_default().insert(gossip_type, val);
                        }
                    }
                }
                *self.gossips.lock().await = GossipContainer::from_raw(raw);
            }
        })
    }
}

/// Vanilla `ValidateNearbyPoi.MAX_DISTANCE` / `BlockPos::closerToCenterThan`
/// (`ValidateNearbyPoi.java:19,25`): a claimed POI is only (in)validated against the live
/// block state while the villager is within 16 blocks of it. Outside that range vanilla's
/// behavior returns `false` (no-op) rather than erasing the memory, which matters here
/// because a chunk outside simulation/load range can read back as air - without this gate a
/// villager that merely wandered away from an unloaded job site would incorrectly lose its
/// ticket and profession.
#[must_use]
fn close_to_poi(pos: Vector3<f64>, poi: BlockPos) -> bool {
    poi.to_centered_f64().squared_distance_to_vec(&pos) < 16.0 * 16.0
}

/// Vanilla `Villager::gossip`'s two cooldown predicates (`Villager.java:815-817`).
#[must_use]
const fn gossip_cooldown_ready(last_gossip: i64, timestamp: i64) -> bool {
    timestamp < last_gossip || timestamp >= last_gossip + 1200
}

fn block_to_profession(block: &Block) -> Option<VillagerProfession> {
    if block == &Block::COMPOSTER {
        Some(VillagerProfession::Farmer)
    } else if block == &Block::LECTERN {
        Some(VillagerProfession::Librarian)
    } else if block == &Block::BLAST_FURNACE {
        Some(VillagerProfession::Armorer)
    } else if block == &Block::SMOKER {
        Some(VillagerProfession::Butcher)
    } else if block == &Block::CARTOGRAPHY_TABLE {
        Some(VillagerProfession::Cartographer)
    } else if block == &Block::BREWING_STAND {
        Some(VillagerProfession::Cleric)
    } else if block == &Block::BARREL {
        Some(VillagerProfession::Fisherman)
    } else if block == &Block::FLETCHING_TABLE {
        Some(VillagerProfession::Fletcher)
    } else if block == &Block::CAULDRON
        || block == &Block::WATER_CAULDRON
        || block == &Block::LAVA_CAULDRON
        || block == &Block::POWDER_SNOW_CAULDRON
    {
        Some(VillagerProfession::Leatherworker)
    } else if block == &Block::STONECUTTER {
        Some(VillagerProfession::Mason)
    } else if block == &Block::LOOM {
        Some(VillagerProfession::Shepherd)
    } else if block == &Block::SMITHING_TABLE {
        Some(VillagerProfession::Toolsmith)
    } else if block == &Block::GRINDSTONE {
        Some(VillagerProfession::Weaponsmith)
    } else {
        None
    }
}

fn profession_matches_block(profession: VillagerProfession, block: &Block) -> bool {
    match profession {
        VillagerProfession::Farmer => block == &Block::COMPOSTER,
        VillagerProfession::Librarian => block == &Block::LECTERN,
        VillagerProfession::Armorer => block == &Block::BLAST_FURNACE,
        VillagerProfession::Butcher => block == &Block::SMOKER,
        VillagerProfession::Cartographer => block == &Block::CARTOGRAPHY_TABLE,
        VillagerProfession::Cleric => block == &Block::BREWING_STAND,
        VillagerProfession::Fisherman => block == &Block::BARREL,
        VillagerProfession::Fletcher => block == &Block::FLETCHING_TABLE,
        VillagerProfession::Leatherworker => {
            block == &Block::CAULDRON
                || block == &Block::WATER_CAULDRON
                || block == &Block::LAVA_CAULDRON
                || block == &Block::POWDER_SNOW_CAULDRON
        }
        VillagerProfession::Mason => block == &Block::STONECUTTER,
        VillagerProfession::Shepherd => block == &Block::LOOM,
        VillagerProfession::Toolsmith => block == &Block::SMITHING_TABLE,
        VillagerProfession::Weaponsmith => block == &Block::GRINDSTONE,
        _ => false,
    }
}

impl Mob for VillagerEntity {
    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    fn mob_bedrock_identifier(&self) -> Option<&'static str> {
        Some("minecraft:villager_v2")
    }

    fn mob_java_spawn_metadata(
        &self,
        version: JavaMinecraftVersion,
    ) -> crate::entity::EntityBaseFuture<'_, Option<Box<[u8]>>> {
        Box::pin(async move {
            let mut metadata = Vec::new();
            Metadata::new(
                tracked_data::villager::VILLAGER_DATA,
                *self.villager_data.lock().await,
            )
            .write(&mut metadata, &version)
            .ok()?;
            metadata.push(255);
            Some(metadata.into_boxed_slice())
        })
    }

    fn mob_bedrock_spawn_metadata(
        &self,
    ) -> crate::entity::EntityBaseFuture<'_, Option<EntityMetadata>> {
        Box::pin(async move {
            Some(Self::bedrock_metadata(
                *self.villager_data.lock().await,
                self.xp.load(Ordering::Relaxed),
            ))
        })
    }

    /// Vanilla persists `CanPickUpLoot: 1b` on every villager
    /// (`VillagerSetCanPickUpLootFix`), and `Villager.pickUpItem` delegates straight to
    /// `InventoryCarrier.pickUpItem` - villagers are always allowed to pick items up.
    fn can_pick_up_loot(&self) -> bool {
        true
    }

    /// `Villager.wantsToPickUp` (`Villager.java:778-782`).
    fn wants_to_pick_up_item(&self, _world: &World, stack: &ItemStack) -> bool {
        if stack
            .item
            .has_tag(&pumpkin_data::tag::Item::MINECRAFT_VILLAGER_PICKS_UP)
        {
            return true;
        }
        // `getVillagerData().profession().value().requestedItems()`. A contended lock only
        // costs one tick's pickup of a profession-specific item, never a tagged one.
        self.villager_data.try_lock().is_ok_and(|data| {
            data.profession_enum()
                .requested_items()
                .iter()
                .any(|item| item.id == stack.item.id)
        })
    }

    fn on_item_pickup<'a>(
        &'a self,
        stack: &'a ItemStack,
    ) -> crate::entity::EntityBaseFuture<'a, u8> {
        Box::pin(async move { self.add_to_inventory(stack) })
    }

    fn get_job_site(&self) -> Option<BlockPos> {
        *self
            .job_site
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn is_job_site_pending(&self) -> crate::entity::EntityBaseFuture<'_, bool> {
        Box::pin(async move { self.job_site_pending.load(Ordering::Relaxed) })
    }

    fn release_pending_job_site(
        &self,
        position: BlockPos,
    ) -> crate::entity::EntityBaseFuture<'_, ()> {
        Box::pin(async move {
            if self.get_job_site() != Some(position)
                || !self.job_site_pending.load(Ordering::Relaxed)
            {
                return;
            }
            self.get_entity()
                .world
                .load()
                .villager_poi
                .lock()
                .await
                .release(position, self.get_entity().entity_uuid);
            if self.get_job_site() == Some(position) {
                *self
                    .job_site
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
                self.job_site_pending.store(false, Ordering::Relaxed);
            }
        })
    }

    fn get_trading_player(&self) -> Option<Arc<Player>> {
        let trading_player = *self
            .trading_player
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let (player_uuid, _) = trading_player?;
        self.get_entity()
            .world
            .load()
            .get_player_by_uuid(player_uuid)
    }

    fn get_home(&self) -> Option<BlockPos> {
        *self
            .home_pos
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn get_meeting_point(&self) -> Option<BlockPos> {
        *self.meeting_point.lock().unwrap()
    }

    /// `Villager::setLastHurtByMob` -> `onReputationEventFrom(VILLAGER_HURT, ...)`
    /// (Villager.java:585-593, 861-862): the hurt villager itself records
    /// `MINOR_NEGATIVE` gossip against its attacker.
    fn on_damage<'a>(
        &'a self,
        _damage_type: DamageType,
        source: Option<&'a dyn EntityBase>,
    ) -> crate::entity::EntityBaseFuture<'a, ()> {
        Box::pin(async move {
            let world = self.get_entity().world.load();
            let mut attacker_uuid = source.map(|source| source.get_entity().entity_uuid);
            if attacker_uuid.is_none() {
                let attacker_id = self
                    .mob_entity
                    .living_entity
                    .last_attacker_id
                    .load(Ordering::Relaxed);
                if attacker_id != 0 {
                    attacker_uuid = world
                        .get_entity_by_id(attacker_id)
                        .map(|attacker| attacker.get_entity().entity_uuid);
                }
            }
            let Some(attacker_uuid) = attacker_uuid else {
                return;
            };
            self.gossips
                .lock()
                .await
                .add(attacker_uuid, GossipType::MinorNegative, 25);
            world.send_entity_status(
                self.get_entity(),
                pumpkin_data::entity::EntityStatus::VillagerAngry,
                Some(ActorEventType::VillagerAngry),
            );

            // Golem summoning trigger deviation: see `spawn_golem_if_needed`'s doc comment.
            // Vanilla only reaches `spawnGolemIfNeeded` via panicking-villager gossip
            // exchange, which Pumpkin has no infrastructure for; being attacked is used here
            // as the closest existing "villager in a crisis" event.
            let world_age = world.get_world_age().await;
            self.spawn_golem_if_needed(&world, world_age, 5).await;
        })
    }

    /// `Villager::tellWitnessesThatIWasMurdered` -> `onReputationEventFrom(VILLAGER_KILLED, ...)`
    /// (Villager.java:615-624, 863-864): every witnessing villager (vanilla uses the brain's
    /// `NEAREST_VISIBLE_LIVING_ENTITIES` memory; approximated here with a 16-block box, the
    /// default `FOLLOW_RANGE` vanilla's sensor inflates by -- `Mob.java:167` -- since Pumpkin
    /// has no brain/sensor system) records `MAJOR_NEGATIVE` gossip against the murderer.
    fn on_mob_death<'a>(
        &'a self,
        cause: Option<&'a dyn EntityBase>,
    ) -> crate::entity::EntityBaseFuture<'a, ()> {
        Box::pin(async move {
            let world = self.get_entity().world.load();

            // `Villager::die` -> `releaseAllPois` (Villager.java:596-605): release every
            // claimed POI ticket unconditionally, murderer or not (fall damage, starvation,
            // etc. all reach this too), so a dead villager never permanently locks a
            // job-site/bed/bell that no other villager can ever claim. Positions are taken
            // into an owned `Vec` before the first `.await` so no `std::sync::MutexGuard`
            // (non-`Send`) is held across it.
            let claimed_pois: Vec<BlockPos> = [
                self.job_site.lock().unwrap().take(),
                self.home_pos.lock().unwrap().take(),
                self.meeting_point.lock().unwrap().take(),
            ]
            .into_iter()
            .flatten()
            .collect();
            for pos in claimed_pois {
                world.release_poi(pos).await;
            }

            let Some(murderer) = cause else {
                return;
            };
            let murderer_uuid = murderer.get_entity().entity_uuid;
            let pos = self.get_entity().pos.load();
            let aabb = BoundingBox::new(
                Vector3::new(pos.x - 16.0, pos.y - 16.0, pos.z - 16.0),
                Vector3::new(pos.x + 16.0, pos.y + 16.0, pos.z + 16.0),
            );
            for entity in world.get_all_at_box(&aabb) {
                if entity.get_entity().entity_id == self.get_entity().entity_id
                    || entity.get_entity().entity_type != &EntityType::VILLAGER
                {
                    continue;
                }
                if let Some(villager) = entity.cast_any().downcast_ref::<Self>() {
                    villager
                        .gossips
                        .lock()
                        .await
                        .add(murderer_uuid, GossipType::MajorNegative, 25);
                }
            }
        })
    }

    fn mob_init_data_tracker(&self) -> crate::entity::EntityBaseFuture<'_, ()> {
        Box::pin(async move {
            let entity = self.get_entity();
            let data = *self.villager_data.lock().await;
            let bedrock_metadata = Self::bedrock_metadata(data, self.xp.load(Ordering::Relaxed));
            entity.send_meta_data(
                &[Metadata::new(tracked_data::villager::VILLAGER_DATA, data)],
                Some(&bedrock_metadata),
            );
            if entity.age.load(Ordering::Relaxed) < 0 {
                entity.send_meta_data(
                    &[Metadata::new(tracked_data::villager::BABY_ID, true)],
                    None,
                );
            }
        })
    }

    #[expect(clippy::too_many_lines)]
    fn mob_tick<'a>(
        &'a self,
        _caller: &'a Arc<dyn EntityBase>,
    ) -> crate::entity::EntityBaseFuture<'a, ()> {
        Box::pin(async move {
            let world = self.get_entity().world.load();

            let unhappy_counter = self.unhappy_counter.load(Ordering::Relaxed);
            if unhappy_counter > 0 {
                let unhappy_counter = unhappy_counter - 1;
                self.unhappy_counter
                    .store(unhappy_counter, Ordering::Relaxed);
                self.get_entity().send_meta_data(
                    &[Metadata::new(
                        tracked_data::villager::UNHAPPY_COUNTER,
                        VarInt(unhappy_counter),
                    )],
                    None,
                );
            }
            self.trade_sound_cooldown
                .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |cooldown| {
                    (cooldown > 0).then_some(cooldown - 1)
                })
                .ok();

            let last_traded_player = self.last_traded_player.lock().await.take();
            if let Some(player_uuid) = last_traded_player {
                self.gossips
                    .lock()
                    .await
                    .add(player_uuid, GossipType::Trading, 2);
                world.send_entity_status(
                    self.get_entity(),
                    pumpkin_data::entity::EntityStatus::VillagerHappy,
                    Some(ActorEventType::VillagerHappy),
                );
            }

            if !self.is_trading.load(Ordering::Relaxed)
                && self.merchant_update_timer.load(Ordering::Relaxed) > 0
                && self.merchant_update_timer.fetch_sub(1, Ordering::Relaxed) == 1
            {
                if self
                    .increase_profession_level_on_update
                    .swap(false, Ordering::Relaxed)
                {
                    let mut data = *self.villager_data.lock().await;
                    data.level.0 += 1;
                    self.set_villager_data(data).await;
                    self.add_trades(data.profession_enum(), data.level.0).await;
                }
                self.mob_entity
                    .living_entity
                    .add_effect(Effect {
                        effect_type: &StatusEffect::REGENERATION,
                        duration: 200,
                        amplifier: 0,
                        ambient: false,
                        show_particles: true,
                        show_icon: true,
                        blend: false,
                    })
                    .await;
            }

            let (game_time, day_time, day) = {
                let time = world.level_time.lock().await;
                (time.world_age, time.query_daytime(), time.query_day())
            };
            self.decay_gossips(game_time).await;
            self.work_at_job_site(&world, game_time, day_time, day)
                .await;

            // `AgeableMob.aiStep`: babies grow up, and a post-breed cooldown counts back
            // down to 0. Must run every tick, ahead of the sensor-cadence gate below.
            self.ageable_ai_step();

            if self.sensor_tick.fetch_add(1, Ordering::Relaxed) % 20 != 0 {
                return;
            }
            self.update_job_site(&world).await;

            // `Villager::maybeDecayGossip` (Villager.java:824-832) runs above in
            // `decay_gossips`.
            let world_age = world.get_world_age().await;

            // `GolemSensor` equivalent (`GolemSensor.java`): approximated with a 16-block box
            // (vanilla scans the brain's `NEAREST_LIVING_ENTITIES` memory, itself populated
            // from a follow-range-sized box -- Pumpkin has no such memory, so this piggybacks
            // on the existing 20-tick cadence instead of a distinct sensor abstraction).
            {
                let pos = self.get_entity().pos.load();
                let aabb = BoundingBox::new(
                    Vector3::new(pos.x - 16.0, pos.y - 16.0, pos.z - 16.0),
                    Vector3::new(pos.x + 16.0, pos.y + 16.0, pos.z + 16.0),
                );
                if world
                    .get_all_at_box(&aabb)
                    .iter()
                    .any(|e| e.get_entity().entity_type == &EntityType::IRON_GOLEM)
                {
                    self.golem_detected_until
                        .store(world_age + 599, Ordering::Relaxed);
                }
            }

            // `TradeWithVillager.tick` (`TradeWithVillager.java:44-62`) is brain-driven in
            // vanilla. The existing schedule goal already models the MEET activity, so use it
            // as the activity gate while the Brain interaction-target/sensor graph is absent.
            if villager_schedule::villager_activity_for_time(world.get_time_of_day().await)
                == villager_schedule::VillagerActivity::Meet
            {
                let pos = self.get_entity().pos.load();
                let aabb = BoundingBox::new(
                    Vector3::new(pos.x - 3.0, pos.y - 3.0, pos.z - 3.0),
                    Vector3::new(pos.x + 3.0, pos.y + 3.0, pos.z + 3.0),
                );
                for entity in world.get_all_at_box(&aabb) {
                    if entity.get_entity().entity_id == self.get_entity().entity_id
                        || !entity.get_entity().is_alive()
                        || entity.get_entity().entity_type != &EntityType::VILLAGER
                    {
                        continue;
                    }
                    let Some(other) = entity.cast_any().downcast_ref::<Self>() else {
                        continue;
                    };
                    if pos.squared_distance_to_vec(&entity.get_entity().pos.load()) > 5.0 {
                        continue;
                    }

                    // `BehaviorUtils.targetIsValid` requires the target to be in
                    // `NEAREST_VISIBLE_LIVING_ENTITIES`; use the same raycast primitive as
                    // the existing target-visibility goal for this sensor approximation.
                    let visible = world
                        .raycast(
                            self.get_eye_pos(),
                            entity.get_entity().get_eye_pos(),
                            async |block_pos, world| world.get_block_state(block_pos).is_solid(),
                        )
                        .await
                        .is_none();
                    if visible {
                        self.gossip_with(&world, other, world_age).await;
                    }
                }
            }

            self.maybe_restock(world_age).await;

            // 1. Bed / Sleeping logic (for all villagers: babies, nitwits, adults)
            let is_sleeping = self.get_entity().pose.load() == EntityPose::Sleeping;
            let self_pos = self.get_entity().pos.load();

            // Check if current bed is still valid
            if let Some(current_home) = self.get_home_pos()
                && close_to_poi(self_pos, current_home)
            {
                let (block, state) = world.get_block_and_state(&current_home);
                let valid = if block.has_tag(&pumpkin_data::tag::Block::MINECRAFT_BEDS) {
                    let bed_props = BedProperties::from_state_id(state.id, block);
                    bed_props.part == BedPart::Head
                } else {
                    false
                };

                if !valid {
                    // Vanilla `ValidateNearbyPoi`/`Villager.releasePoi`:
                    // release the claimed bed's ticket once it's no longer a
                    // valid (head-part) bed, e.g. it was broken.
                    world.release_poi(current_home).await;
                    *self
                        .home_pos
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
                    if is_sleeping {
                        // Wake up if bed was broken
                        self.get_entity().set_pose(EntityPose::Standing);
                        self.get_entity().send_meta_data(
                            &[Metadata::new(
                                pumpkin_data::tracked_data::villager::SLEEPING_POS_ID,
                                None::<BlockPos>,
                            )],
                            None,
                        );
                    }
                }
            }

            // If no bed, atomically claim the closest unclaimed one -
            // vanilla `AcquirePoi` (`SCAN_RANGE = 48`), via
            // `World::acquire_poi` (`PoiManager.take`,
            // `Occupancy.HAS_SPACE`). Because acquisition decrements the
            // POI's `free_tickets`, no other villager can claim the same
            // bed - unlike the old ad-hoc scan, this doesn't need to ask
            // every nearby villager what it has already claimed.
            if self.get_home_pos().is_none() {
                let pos = self.get_entity().block_pos.load();
                if let Some(home) = world
                    .acquire_poi(crate::world::village_poi::POI_TYPE_HOME, pos, 48)
                    .await
                {
                    *self
                        .home_pos
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(home);
                } else {
                    let start = BlockPos::new(pos.0.x - 16, pos.0.y - 4, pos.0.z - 16);
                    let end = BlockPos::new(pos.0.x + 16, pos.0.y + 4, pos.0.z + 16);

                    let aabb = BoundingBox::new(
                        Vector3::new(
                            pos.0.x as f64 - 32.0,
                            pos.0.y as f64 - 16.0,
                            pos.0.z as f64 - 32.0,
                        ),
                        Vector3::new(
                            pos.0.x as f64 + 32.0,
                            pos.0.y as f64 + 16.0,
                            pos.0.z as f64 + 32.0,
                        ),
                    );
                    let nearby_entities = world.get_all_at_box(&aabb);

                    let mut claimed_homes = Vec::new();
                    for entity in nearby_entities {
                        if entity.get_entity().entity_id != self.get_entity().entity_id
                            && entity.get_entity().entity_type
                                == &pumpkin_data::entity::EntityType::VILLAGER
                            && let Some(home) = entity.get_home_pos()
                        {
                            claimed_homes.push(home);
                        }
                    }

                    let mut best_home = None;
                    let mut best_dist = f64::MAX;

                    for p in BlockPos::iterate(start, end) {
                        let (block, state) = world.get_block_and_state(&p);
                        if block.has_tag(&pumpkin_data::tag::Block::MINECRAFT_BEDS) {
                            let bed_props = BedProperties::from_state_id(state.id, block);
                            let bed_head_pos = if bed_props.part == BedPart::Head {
                                p
                            } else {
                                p.offset(bed_props.facing.to_offset())
                            };

                            if claimed_homes.contains(&bed_head_pos) {
                                continue;
                            }

                            let dist = bed_head_pos
                                .to_f64()
                                .squared_distance_to_vec(&self.get_entity().pos.load());
                            if dist < best_dist {
                                best_dist = dist;
                                best_home = Some(bed_head_pos);
                            }
                        }
                    }

                    if let Some(home) = best_home {
                        *self
                            .home_pos
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(home);
                    }
                }
            }

            // Handle Sleeping/Waking up based on time
            let is_sleeping = self.get_entity().pose.load() == EntityPose::Sleeping;
            if let Some(home_pos) = self.get_home_pos() {
                let time = world.level_time.lock().await.time_of_day;
                let is_night = (12000..=23000).contains(&time);

                if is_night {
                    if !is_sleeping {
                        // Check distance to bed. If close enough, go to sleep
                        let dist = home_pos
                            .to_f64()
                            .squared_distance_to_vec(&self.get_entity().pos.load());
                        if dist <= 4.0 {
                            // Within 2 blocks (squared distance 4.0)
                            let (block, state) = world.get_block_and_state(&home_pos);
                            if block.has_tag(&pumpkin_data::tag::Block::MINECRAFT_BEDS) {
                                let bed_props = BedProperties::from_state_id(state.id, block);
                                if !bed_props.occupied {
                                    // Make bed occupied
                                    BedBlock::set_occupied(
                                        true, &world, block, &home_pos, state.id,
                                    )
                                    .await;

                                    self.get_entity().set_pose(EntityPose::Sleeping);
                                    // Vanilla `LAST_SLEPT` brain memory, set whenever the
                                    // sleep-behavior brain task puts the villager to sleep
                                    // (referenced by `golemSpawnConditionsMet`,
                                    // `Villager.java:896-899`; the task that sets it wasn't
                                    // itself read for this pass).
                                    self.last_slept_time.store(world_age, Ordering::Relaxed);
                                    self.get_entity().send_meta_data(
                                        &[Metadata::new(
                                            pumpkin_data::tracked_data::villager::SLEEPING_POS_ID,
                                            Some(home_pos),
                                        )],
                                        None,
                                    );
                                }
                            }
                        }
                    }
                } else if is_sleeping {
                    // It is day, wake up!
                    let (block, state) = world.get_block_and_state(&home_pos);
                    if block.has_tag(&pumpkin_data::tag::Block::MINECRAFT_BEDS) {
                        let bed_props = BedProperties::from_state_id(state.id, block);
                        if bed_props.occupied {
                            BedBlock::set_occupied(false, &world, block, &home_pos, state.id).await;
                        }
                    }

                    self.get_entity().set_pose(EntityPose::Standing);
                    self.get_entity().send_meta_data(
                        &[Metadata::new(
                            pumpkin_data::tracked_data::villager::SLEEPING_POS_ID,
                            None::<BlockPos>,
                        )],
                        None,
                    );
                }
            }

            let is_adult = self.get_entity().age.load(Ordering::Relaxed) >= 0;

            // 1b. Meeting-point (bell) POI - `AcquirePoi.create(p -> p.is(PoiTypes.MEETING),
            // MEETING_POINT, true, ...)` (`VillagerGoalPackages.getCorePackage`, priority 10).
            // `onlyIfAdult = true`, unconditional on profession (even Nitwits gather at the
            // bell), so this runs ahead of the profession early-return below.
            if is_adult {
                let self_pos = self.get_entity().pos.load();
                if let Some(current_meeting) = self.get_meeting_point()
                    && close_to_poi(self_pos, current_meeting)
                {
                    let (block, _state) = world.get_block_and_state(&current_meeting);
                    if block != &Block::BELL {
                        world.release_poi(current_meeting).await;
                        *self.meeting_point.lock().unwrap() = None;
                    }
                }
                if self.get_meeting_point().is_none() {
                    let pos = self.get_entity().block_pos.load();
                    if let Some(meeting) = world
                        .acquire_poi(crate::world::village_poi::POI_TYPE_MEETING, pos, 48)
                        .await
                    {
                        *self.meeting_point.lock().unwrap() = Some(meeting);
                    }
                }
            }

            // 2. Job / Profession logic (skip for Nitwits and babies)
            let data = self.villager_data.lock().await;
            let xp = self.xp.load(Ordering::Relaxed);
            let profession = data.profession_enum();
            drop(data);

            if profession == VillagerProfession::Nitwit || !is_adult {
                return;
            }

            if let Some(current_site) = self.get_job_site()
                && close_to_poi(self.get_entity().pos.load(), current_site)
            {
                let (block, _state) = world.get_block_and_state(&current_site);
                let valid = if profession == VillagerProfession::None {
                    block_to_profession(block).is_some()
                } else {
                    profession_matches_block(profession, block)
                };

                if !valid {
                    // Vanilla `ValidateNearbyPoi`: release the job-site ticket once the block
                    // stops matching (e.g. broken), same as the bed-release path above.
                    world.release_poi(current_site).await;
                    *self
                        .job_site
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
                    if xp == 0 && profession != VillagerProfession::None {
                        let r#type = self.villager_data.lock().await.type_enum();
                        self.set_villager_data(VillagerData::new(
                            r#type,
                            VillagerProfession::None,
                            1,
                        ))
                        .await;
                        self.offers.lock().await.clear();
                    }
                } else if profession == VillagerProfession::None
                    && let Some(prof) = block_to_profession(block)
                {
                    // `AssignProfessionFromJobSite` (`VillagerGoalPackages.java`, priority 10):
                    // a valid job-site claim with no profession yet assigns one. Covers a
                    // villager loaded from a save with a claimed `JobSiteX` but no profession
                    // (e.g. an interrupted acquisition), not just the fresh-acquisition path
                    // below.
                    let r#type = self.villager_data.lock().await.type_enum();
                    self.set_villager_data(VillagerData::new(r#type, prof, 1))
                        .await;
                }
            }

            // Atomically claim the closest unclaimed job-site POI - vanilla `AcquirePoi`
            // (`AcquirePoi.SCAN_RANGE = 48`). Ticket-based via `World::acquire_poi[_where]`
            // (`PoiManager.take`), matching the bed acquisition above: no other villager can
            // claim the same block, so there is no need to separately scan nearby villagers
            // for what they've already claimed. An employed villager is restricted to POIs
            // whose block still matches its own profession (`AssignProfessionFromJobSite`
            // never reassigns an already-employed villager away from its trade).
            if self.get_job_site().is_none() {
                let pos = self.get_entity().block_pos.load();
                let claimed = if profession == VillagerProfession::None {
                    world
                        .acquire_poi(crate::world::village_poi::POI_TYPE_JOB_SITE, pos, 48)
                        .await
                } else {
                    world
                        .acquire_poi_where(
                            crate::world::village_poi::POI_TYPE_JOB_SITE,
                            pos,
                            48,
                            |block| profession_matches_block(profession, block),
                        )
                        .await
                };

                if let Some(site) = claimed {
                    *self
                        .job_site
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(site);
                } else {
                    let start = BlockPos::new(pos.0.x - 16, pos.0.y - 4, pos.0.z - 16);
                    let end = BlockPos::new(pos.0.x + 16, pos.0.y + 4, pos.0.z + 16);
                    let aabb = BoundingBox::new(
                        Vector3::new(
                            pos.0.x as f64 - 32.0,
                            pos.0.y as f64 - 16.0,
                            pos.0.z as f64 - 32.0,
                        ),
                        Vector3::new(
                            pos.0.x as f64 + 32.0,
                            pos.0.y as f64 + 16.0,
                            pos.0.z as f64 + 32.0,
                        ),
                    );
                    let nearby_entities = world.get_all_at_box(&aabb);

                    let mut claimed_sites = Vec::new();
                    for entity in nearby_entities {
                        if entity.get_entity().entity_id != self.get_entity().entity_id
                            && entity.get_entity().entity_type
                                == &pumpkin_data::entity::EntityType::VILLAGER
                            && let Some(site) = entity.get_job_site_pos()
                        {
                            claimed_sites.push(site);
                        }
                    }

                    let mut best_site = None;
                    let mut best_dist = f64::MAX;
                    for p in BlockPos::iterate(start, end) {
                        if claimed_sites.contains(&p) {
                            continue;
                        }

                        let (block, _state) = world.get_block_and_state(&p);
                        if let Some(prof) = block_to_profession(block) {
                            if profession != VillagerProfession::None && prof != profession {
                                continue;
                            }

                            let dist = p
                                .to_f64()
                                .squared_distance_to_vec(&self.get_entity().pos.load());
                            if dist < best_dist {
                                best_dist = dist;
                                best_site = Some(p);
                            }
                        }
                    }

                    if let Some(site) = best_site {
                        *self
                            .job_site
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(site);
                        if profession == VillagerProfession::None {
                            let (block, _state) = world.get_block_and_state(&site);
                            if let Some(prof) = block_to_profession(block) {
                                let r#type = self.villager_data.lock().await.type_enum();
                                self.set_villager_data(VillagerData::new(r#type, prof, 1))
                                    .await;
                            }
                        }
                    }
                }
            }
        })
    }

    fn mob_interact<'a>(
        &'a self,
        player: &'a Arc<Player>,
        item_stack: &'a mut pumpkin_data::item_stack::ItemStack,
    ) -> crate::entity::EntityBaseFuture<'a, bool> {
        let player = player.clone();
        Box::pin(async move {
            if item_stack.item == &Item::VILLAGER_SPAWN_EGG
                || self.mob_entity.living_entity.health.load() <= 0.0
                || self.is_trading.load(Ordering::Relaxed)
                || self.get_entity().pose.load() == EntityPose::Sleeping
            {
                return false;
            }
            if self.get_entity().age.load(Ordering::Relaxed) < 0 {
                self.set_unhappy();
                return true;
            }

            let mut offers = self.offers.lock().await;
            if offers.is_empty() {
                let data = self.villager_data.lock().await;
                if data.profession_enum() != VillagerProfession::None
                    && data.profession_enum() != VillagerProfession::Nitwit
                {
                    let prof = data.profession_enum();
                    let level = data.level.0;
                    drop(data);
                    drop(offers);
                    self.generate_trades(prof, level).await;
                    offers = self.offers.lock().await;
                } else {
                    drop(data);
                }
            }

            if offers.is_empty() {
                self.set_unhappy();
                return true;
            }
            drop(offers);

            player
                .increment_stat(
                    pumpkin_data::statistic::StatisticCategory::Custom,
                    pumpkin_data::statistic::CustomStatistic::TalkedToVillager as i32,
                    1,
                )
                .await;

            self.open_trading_screen(&player).await;

            true
        })
    }
}

#[cfg(test)]
mod villager_tick_logic_tests {
    use super::{
        golem_spawn_conditions_met, gossip_cooldown_ready, is_new_restock_day, restock_is_due,
    };

    #[test]
    fn golem_conditions_require_recent_sleep() {
        assert!(!golem_spawn_conditions_met(0, 0, 100));
        assert!(golem_spawn_conditions_met(100, 0, 200));
        assert!(!golem_spawn_conditions_met(100, 0, 100 + 24000));
    }

    #[test]
    fn golem_conditions_respect_suppression_window() {
        assert!(!golem_spawn_conditions_met(100, 1000, 500));
        assert!(golem_spawn_conditions_met(100, 1000, 1000));
        assert!(golem_spawn_conditions_met(100, 1000, 1500));
    }

    #[test]
    fn the_restock_counter_resets_after_half_a_day() {
        assert!(!is_new_restock_day(0, 100));
        assert!(!is_new_restock_day(100, 100 + 12000));
        assert!(is_new_restock_day(100, 100 + 12001));
    }

    #[test]
    fn restock_due_gates_on_count_and_cooldown() {
        // The first restock of a cycle is free, the second waits 2400 ticks, and there is no third.
        assert!(restock_is_due(100, 0, 100));
        assert!(!restock_is_due(100, 1, 100 + 2400));
        assert!(restock_is_due(100, 1, 100 + 2401));
        assert!(!restock_is_due(100, 2, 100 + 24000));
    }

    #[test]
    fn gossip_cooldown_matches_vanilla_boundary() {
        assert!(!gossip_cooldown_ready(100, 100));
        assert!(!gossip_cooldown_ready(100, 100 + 1199));
        assert!(gossip_cooldown_ready(100, 100 + 1200));
        assert!(gossip_cooldown_ready(1000, 500));
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use pumpkin_data::data_component_impl::{EnchantmentsImpl, StoredEnchantmentsImpl};
    use pumpkin_data::villager::VillagerTradeModifier;
    use pumpkin_util::version::JavaMinecraftVersion;

    use super::*;

    #[test]
    fn breeding_requires_twelve_food_points_and_an_exact_zero_age() {
        // Three bread is exactly 12 points (`Villager.FOOD_POINTS`, `Villager.java:101`).
        assert_eq!(get_food_points(&Item::BREAD) * 3, BREEDING_FOOD_THRESHOLD);
        assert!(can_breed_from(0, 12, false, 0));
        assert!(can_breed_from(12, 0, false, 0));
        assert!(!can_breed_from(6, 5, false, 0));
        // Sleeping villagers never breed.
        assert!(!can_breed_from(12, 12, true, 0));
        // The post-breed cooldown (age 6000) and babies (negative age) are both excluded,
        // where the generic `age >= 0` readiness test would let the cooldown through.
        assert!(!can_breed_from(12, 12, false, 6000));
        assert!(!can_breed_from(12, 12, false, -24000));
    }

    #[test]
    fn offspring_type_is_the_biome_half_the_time_then_each_parent() {
        let biome = VillagerType::Snow;
        let own = VillagerType::Desert;
        let partner = VillagerType::Jungle;
        assert_eq!(breed_offspring_type(0.0, biome, own, partner), biome);
        assert_eq!(breed_offspring_type(0.499, biome, own, partner), biome);
        assert_eq!(breed_offspring_type(0.5, biome, own, partner), own);
        assert_eq!(breed_offspring_type(0.749, biome, own, partner), own);
        assert_eq!(breed_offspring_type(0.75, biome, own, partner), partner);
        assert_eq!(breed_offspring_type(0.999, biome, own, partner), partner);
    }

    #[test]
    fn villager_data_metadata_uses_the_villager_tracker_slot() {
        let data = VillagerData::new(VillagerType::Plains, VillagerProfession::Librarian, 1);
        let metadata = Metadata::new(tracked_data::villager::VILLAGER_DATA, data);
        let mut bytes = Vec::new();

        metadata
            .write(&mut bytes, &JavaMinecraftVersion::V_26_2)
            .unwrap();

        assert_eq!(bytes, [19, 18, 2, 9, 1]);
    }

    #[test]
    fn villager_data_maps_to_bedrock_appearance_metadata() {
        let metadata = VillagerEntity::bedrock_metadata(
            VillagerData::new(VillagerType::Plains, VillagerProfession::Librarian, 3),
            75,
        );

        assert!(matches!(
            metadata.0.get(&entity_data_key::VARIANT),
            Some(MetadataValue::Int(5))
        ));
        assert!(matches!(
            metadata.0.get(&entity_data_key::MARK_VARIANT),
            Some(MetadataValue::Int(0))
        ));
        assert!(matches!(
            metadata.0.get(&entity_data_key::TRADE_TIER),
            Some(MetadataValue::Int(2))
        ));
        assert!(matches!(
            metadata.0.get(&entity_data_key::MAX_TRADE_TIER),
            Some(MetadataValue::Int(4))
        ));
        assert!(matches!(
            metadata.0.get(&entity_data_key::TRADE_EXPERIENCE),
            Some(MetadataValue::Int(75))
        ));
    }

    #[test]
    fn unhappy_counter_metadata_uses_the_abstract_villager_tracker_slot() {
        let metadata = Metadata::new(tracked_data::villager::UNHAPPY_COUNTER, VarInt(40));
        let mut bytes = Vec::new();

        metadata
            .write(&mut bytes, &JavaMinecraftVersion::V_26_2)
            .unwrap();

        assert_eq!(bytes, [18, 1, 40]);
    }

    #[test]
    fn enchanted_book_offer_has_vanilla_items_and_a_nonzero_price() {
        let (emeralds, enchanted_book, book) = enchanted_book_offer_items(&mut rand::rng())
            .expect("the generated tradeable-enchantment tag is populated");
        let stored = enchanted_book
            .get_data_component::<StoredEnchantmentsImpl>()
            .unwrap();

        assert_eq!(emeralds.item.id, Item::EMERALD.id);
        assert!((5..=64).contains(&emeralds.item_count));
        assert_eq!(book.unwrap().item.id, Item::BOOK.id);
        assert_eq!(enchanted_book.item.id, Item::ENCHANTED_BOOK.id);
        assert_eq!(stored.enchantment.len(), 1);
        assert!(
            stored.enchantment[0]
                .0
                .has_tag(&EnchantmentTag::MINECRAFT_TRADEABLE)
        );
        assert!((1..=stored.enchantment[0].0.max_level).contains(&stored.enchantment[0].1));
    }

    #[test]
    fn generated_trades_keep_dynamic_modifiers_and_secondary_costs() {
        let librarian = VillagerProfession::Librarian.trade_set(1).unwrap();
        let enchanted_book = librarian
            .trades
            .iter()
            .find(|trade| trade.modifier == VillagerTradeModifier::EnchantRandomly)
            .unwrap();
        assert!(enchanted_book.wants.item == &Item::EMERALD);
        assert!(enchanted_book.wants_b.unwrap().item == &Item::BOOK);
        assert_eq!(enchanted_book.price_multiplier, 0.2);

        let cartographer = VillagerProfession::Cartographer.trade_set(2).unwrap();
        assert!(cartographer.trades.iter().any(|trade| {
            matches!(trade.modifier, VillagerTradeModifier::ExplorationMap { .. })
                && !trade.allowed_types.is_empty()
                && trade
                    .wants_b
                    .is_some_and(|cost| cost.item == &Item::COMPASS)
        }));

        let fletcher = VillagerProfession::Fletcher.trade_set(5).unwrap();
        assert!(fletcher.trades.iter().any(|trade| {
            trade.modifier == VillagerTradeModifier::RandomPotion
                && trade.wants_b.is_some_and(|cost| cost.item == &Item::ARROW)
        }));
    }

    #[test]
    fn smith_trade_sets_include_the_shared_vanilla_trades() {
        let armorer_novice = VillagerProfession::Armorer.trade_set(1).unwrap();
        assert!(armorer_novice.trades.iter().any(|trade| {
            trade.wants.item == &Item::COAL && trade.gives.item == &Item::EMERALD
        }));

        let armorer_apprentice = VillagerProfession::Armorer.trade_set(2).unwrap();
        assert!(armorer_apprentice.trades.iter().any(|trade| {
            trade.wants.item == &Item::EMERALD && trade.gives.item == &Item::BELL
        }));
        assert!(armorer_apprentice.trades.iter().any(|trade| {
            trade.wants.item == &Item::IRON_INGOT && trade.gives.item == &Item::EMERALD
        }));

        for profession in [
            VillagerProfession::Toolsmith,
            VillagerProfession::Weaponsmith,
        ] {
            assert!(profession.trade_set(1).unwrap().trades.iter().any(|trade| {
                trade.wants.item == &Item::COAL && trade.gives.item == &Item::EMERALD
            }));
        }
    }

    #[test]
    fn traded_equipment_is_enchanted_and_reports_its_additional_price() {
        for _ in 0..32 {
            let (stack, additional_price) =
                enchant_trade_item(&mut rand::rng(), &Item::DIAMOND_SWORD, 5, 19).unwrap();
            let enchantments = stack.get_data_component::<EnchantmentsImpl>().unwrap();

            assert!((5..=19).contains(&additional_price));
            assert!(!enchantments.enchantment.is_empty());
        }
    }
}
