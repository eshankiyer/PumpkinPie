// Legacy invariant checks retained for vanilla behavior; migrate these paths before removing this allow.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use std::borrow::Cow;
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::{Arc, Weak};

use crossbeam::atomic::AtomicCell;
use pumpkin_data::data_component_impl::EquipmentSlot;
use pumpkin_data::effect::StatusEffect;
use pumpkin_data::entity::EntityType;
use pumpkin_data::item::{Item, JavaToBedrockItemMapping};
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_data::villager::{
    VillagerTrade, VillagerTradeModifier, VillagerTradeSet, WANDERING_TRADER_TRADE_SET_BUYING,
    WANDERING_TRADER_TRADE_SET_COMMON, WANDERING_TRADER_TRADE_SET_UNCOMMON,
};
use pumpkin_inventory::merchant::merchant_screen_handler::MerchantScreenHandler;
use pumpkin_inventory::screen_handler::{
    InventoryPlayer, ScreenHandlerFactory, SharedScreenHandler,
};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_nbt::tag::NbtTag;
use pumpkin_protocol::codec::item_stack_seralizer::ItemStackSerializer;
use pumpkin_protocol::codec::var_int::VarInt;
use pumpkin_protocol::java::client::play::CMerchantOffers;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_util::text::TextComponent;
use pumpkin_world::inventory::SimpleInventory;
use rand::RngExt;
use rand::seq::IndexedRandom;
use std::sync::Mutex;

use super::villager::{
    apply_potion, apply_random_dye, apply_random_stew_effect, enchant_trade_item,
    enchanted_book_offer_items, trigger_trade_advancement,
};
use crate::entity::ageable::{AgeableData, AgeableMob};
use crate::entity::player::Player;
use crate::entity::{
    Entity, EntityBase, NBTStorage,
    ai::goal::{
        Controls, Goal, avoid_entity::AvoidEntityGoal, escape_danger::EscapeDangerGoal,
        interact::InteractGoal, look_at_entity::LookAtEntityGoal, look_at_trading_player,
        move_towards_restriction::MoveTowardsRestrictionGoal, swim::SwimGoal,
        trade_with_player::TradeWithPlayerGoal, wander_around::WanderAroundGoal,
    },
    ai::pathfinder::NavigatorGoal,
    mob::{Mob, MobEntity},
};
use crate::world::World;

/// Vanilla `WanderingTrader::despawnDelay` default (`WanderingTrader.java:52-53`): `0`,
/// meaning "never auto-despawns" until something sets it positive (`maybeDespawn` only acts
/// when `despawnDelay > 0`). `WanderingTraderSpawner.spawn` sets it to 48000 for natural
/// spawns; traders created via spawn egg/command retain the zero default.
const DEFAULT_DESPAWN_DELAY: i32 = 0;

// Vanilla `MerchantContainer.stillValid` requires the current trading player
// (`MerchantContainer.java:79-81`).
fn trading_player_matches(trading_player: Option<uuid::Uuid>, player_uuid: uuid::Uuid) -> bool {
    trading_player.is_some_and(|trading_player| trading_player == player_uuid)
}

/// `PotionContents.createItemStack(Items.POTION, Potions.INVISIBILITY)`
/// (`WanderingTrader.java:66`), the stack the dusk `UseItemGoal` drinks.
fn create_invisibility_potion() -> ItemStack {
    let mut stack = ItemStack::new(1, &Item::POTION);
    apply_potion(&mut stack, "invisibility");
    stack
}

/// Vanilla `AbstractVillager.addOffersFromTradeSet` /
/// `addOffersFromItemListingsWithoutDuplicates` (`AbstractVillager.java:232-253`): draws
/// `amount` distinct listings at random, skipping any whose item modifier fails to produce an
/// offer, and applies each listing's item modifier to the result.
fn add_offers_from_trade_set(
    offers: &mut Vec<pumpkin_protocol::java::client::play::MerchantOffer>,
    trade_set: VillagerTradeSet,
    rng: &mut impl rand::Rng,
) {
    let mut remaining: Vec<&'static VillagerTrade> = trade_set.trades.iter().collect();
    let mut added = 0;
    while added < trade_set.amount && !remaining.is_empty() {
        let index = rng.random_range(0..remaining.len());
        let trade = remaining.remove(index);

        let mut base_cost_a = ItemStack::new(trade.wants.count as u8, trade.wants.item);
        let mut output = ItemStack::new(trade.gives.count as u8, trade.gives.item);
        let mut cost_b = trade
            .wants_b
            .as_ref()
            .map(|b| ItemStack::new(b.count as u8, b.item));

        match trade.modifier {
            VillagerTradeModifier::None => {}
            VillagerTradeModifier::EnchantRandomly => {
                let Some(items) = enchanted_book_offer_items(rng) else {
                    continue;
                };
                (base_cost_a, output, cost_b) = items;
            }
            VillagerTradeModifier::EnchantWithLevels { min, max } => {
                let Some((enchanted, additional_cost)) =
                    enchant_trade_item(rng, trade.gives.item, min, max)
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
            // Explorer maps need a villager's structure search; no wandering-trader listing
            // uses one.
            VillagerTradeModifier::ExplorationMap { .. } => continue,
            VillagerTradeModifier::RandomDyes => apply_random_dye(rng, &mut output),
            VillagerTradeModifier::RandomPotion => {
                let Some(potion_name) =
                    pumpkin_data::tag::Potion::MINECRAFT_TRADEABLE.0.choose(rng)
                else {
                    continue;
                };
                apply_potion(&mut output, potion_name);
            }
            VillagerTradeModifier::SuspiciousStew => apply_random_stew_effect(rng, &mut output),
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

/// The seven `AvoidEntityGoal`s of `WanderingTrader.registerGoals`
/// (`WanderingTrader.java:80-86`), each at priority 1 with speeds `0.5, 0.5`.
///
/// Vanilla matches `Zombie.class` by class hierarchy, so it also covers every subclass:
/// `Drowned.java:67`, `Husk.java:31`, `ZombieVillager.java:61` and `ZombifiedPiglin.java:48`
/// all `extends Zombie`. `AvoidEntityGoal`'s `FleeSelector::Type` matches one exact entity
/// type, so those four are listed explicitly at the same 8.0 radius. The other six vanilla
/// classes here have no subclasses (verified by grepping `extends <Class>` over the decompile).
const AVOIDED: &[(&EntityType, f64)] = &[
    (&EntityType::ZOMBIE, 8.0),
    (&EntityType::DROWNED, 8.0),
    (&EntityType::HUSK, 8.0),
    (&EntityType::ZOMBIE_VILLAGER, 8.0),
    (&EntityType::ZOMBIFIED_PIGLIN, 8.0),
    (&EntityType::EVOKER, 12.0),
    (&EntityType::VINDICATOR, 8.0),
    (&EntityType::VEX, 8.0),
    (&EntityType::PILLAGER, 15.0),
    (&EntityType::ILLUSIONER, 12.0),
    (&EntityType::ZOGLIN, 10.0),
];

/// `WanderingTrader.java`.
///
/// Vanilla's `AbstractVillager` base (trading-player tracking,
/// offers, trade-XP reward) has no Rust equivalent shared with `VillagerEntity` -- see the
/// design doc's cross-cutting note; this duplicates the small amount of trading glue
/// `VillagerEntity` already has rather than introducing a shared base for a two-user case.
///
/// The two `UseItemGoal`s (`WanderingTrader.java:62-78`, drink invisibility after dark and milk
/// at dawn) are ported as the trader-specific `WanderingTraderUseItemGoal`.
/// `LookAtTradingPlayerGoal` (`WanderingTrader.java:88`) is ported as
///   [`crate::entity::ai::goal::look_at_trading_player::LookAtTradingPlayerGoal`].
pub struct WanderingTraderEntity {
    pub mob_entity: MobEntity,
    pub offers: Mutex<Vec<pumpkin_protocol::java::client::play::MerchantOffer>>,
    pub merchant_inventory: Arc<SimpleInventory>,
    /// Vanilla `despawnDelay` (`WanderingTrader.java:52-53, 195-201`). Ticks remaining before
    /// natural despawn; decremented in `mob_tick` while not trading.
    pub despawn_delay: AtomicI32,
    /// Vanilla `wanderTarget` (`WanderingTrader.java:52,217-223`).
    wander_target: AtomicCell<Option<BlockPos>>,
    /// Vanilla `AbstractVillager.tradingPlayer`. Set while the merchant screen is open, which
    /// is what makes `isTrading()` (`WanderingTrader.java:212`) and `TradeWithPlayerGoal`
    /// meaningful here. Mirrors `VillagerEntity::trading_player`.
    pub trading_player: std::sync::Mutex<Option<uuid::Uuid>>,
    pub ageable_data: AgeableData,
    pub self_weak: std::sync::Mutex<Option<Weak<Self>>>,
}

impl WanderingTraderEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let mob_entity = MobEntity::new(entity);
        let trader = Self {
            mob_entity,
            offers: Mutex::new(Vec::new()),
            merchant_inventory: Arc::new(SimpleInventory::new(3)),
            despawn_delay: AtomicI32::new(DEFAULT_DESPAWN_DELAY),
            wander_target: AtomicCell::new(None),
            trading_player: std::sync::Mutex::new(None),
            ageable_data: AgeableData::default(),
            self_weak: std::sync::Mutex::new(None),
        };
        let mob_arc = Arc::new(trader);
        *mob_arc.self_weak.lock().unwrap() = Some(Arc::downgrade(&mob_arc));
        let mob_weak: Weak<dyn Mob> = {
            let mob_arc: Arc<dyn Mob> = mob_arc.clone();
            Arc::downgrade(&mob_arc)
        };
        let trader_weak = Arc::downgrade(&mob_arc);

        {
            let mut goal_selector = mob_arc
                .mob_entity
                .goals_selector
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);

            // `WanderingTrader.registerGoals` (`WanderingTrader.java:60-94`).
            goal_selector.add_goal(0, Box::new(SwimGoal::default()));
            // `WanderingTrader.java:62-78`: the two priority-0 `UseItemGoal`s.
            goal_selector.add_goal(
                0,
                Box::new(WanderingTraderUseItemGoal::new(
                    trader_weak.clone(),
                    UseItemKind::Invisibility,
                )),
            );
            goal_selector.add_goal(
                0,
                Box::new(WanderingTraderUseItemGoal::new(
                    trader_weak,
                    UseItemKind::Milk,
                )),
            );
            goal_selector.add_goal(1, Box::new(TradeWithPlayerGoal::new(0.5)));
            // `WanderingTrader.java:89`: `addGoal(1, new LookAtTradingPlayerGoal(this))`.
            goal_selector.add_goal(
                1,
                Box::new(look_at_trading_player::LookAtTradingPlayerGoal::new(
                    mob_weak.clone(),
                )),
            );
            for (flee_type, flee_distance) in AVOIDED {
                goal_selector.add_goal(
                    1,
                    Box::new(AvoidEntityGoal::new(flee_type, *flee_distance, 0.5, 0.5)),
                );
            }
            goal_selector.add_goal(1, EscapeDangerGoal::new(0.5));
            goal_selector.add_goal(2, Box::new(WanderToPositionGoal::new(2.0, 0.35)));
            goal_selector.add_goal(4, MoveTowardsRestrictionGoal::new(0.35));
            goal_selector.add_goal(8, Box::new(WanderAroundGoal::new_water_avoiding(0.35)));
            // `new InteractGoal(this, Player.class, 3.0F, 1.0F)`
            // (`WanderingTrader.java:92`, `InteractGoal.java:13-16`).
            goal_selector.add_goal(
                9,
                InteractGoal::new(mob_weak.clone(), &EntityType::PLAYER, 3.0, 1.0, false),
            );
            // `LookAtPlayerGoal(this, Mob.class, 8.0F)` -- any mob, not only players.
            goal_selector.add_goal(10, LookAtEntityGoal::with_default_any_mob(mob_weak, 8.0));
        };

        mob_arc
    }

    /// Vanilla `WanderingTrader.setDespawnDelay` (`WanderingTrader.java:195-197`).
    pub fn set_despawn_delay(&self, delay: i32) {
        self.despawn_delay.store(delay, Ordering::Relaxed);
    }

    /// Vanilla `WanderingTrader.getDespawnDelay` (`WanderingTrader.java:199-201`).
    #[must_use]
    pub fn get_despawn_delay(&self) -> i32 {
        self.despawn_delay.load(Ordering::Relaxed)
    }

    /// Vanilla `AbstractVillager.isTrading`: a trading player is set.
    #[must_use]
    pub fn is_trading(&self) -> bool {
        self.trading_player
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_some()
    }

    /// Vanilla `WanderingTrader.getWanderTarget` (`WanderingTrader.java:221-223`).
    #[must_use]
    pub fn get_wander_target(&self) -> Option<BlockPos> {
        self.wander_target.load()
    }

    /// Vanilla `WanderingTrader.setWanderTarget` (`WanderingTrader.java:217-219`).
    pub fn set_wander_target(&self, target: Option<BlockPos>) {
        self.wander_target.store(target);
    }

    /// The spawner's `setHomeTo(referencePos, 16)` (`WanderingTraderSpawner.java:102-104`).
    pub fn set_home_to(&self, position: BlockPos, radius: i32) {
        self.mob_entity.position_target.store(position);
        self.mob_entity
            .position_target_range
            .store(radius, Ordering::Relaxed);
    }

    /// Vanilla `WanderingTrader::updateTrades` (`WanderingTrader.java:129-135`), via the
    /// shared `AbstractVillager.addOffersFromTradeSet` helper: pulls buying, then uncommon,
    /// then common trade sets, in that fixed order.
    pub fn update_trades(&self) {
        let mut offers = self
            .offers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut rng = rand::rng();
        for trade_set in [
            WANDERING_TRADER_TRADE_SET_BUYING,
            WANDERING_TRADER_TRADE_SET_UNCOMMON,
            WANDERING_TRADER_TRADE_SET_COMMON,
        ] {
            add_offers_from_trade_set(&mut offers, trade_set, &mut rng);
        }
    }

    /// Regenerates the offer list from scratch.
    pub fn generate_trades(&self) {
        self.offers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
        self.update_trades();
    }

    pub async fn open_trading_screen(&self, player: &Arc<Player>) {
        if let Some(sync_id) = player.open_handled_screen(self, None) {
            let offers = self
                .offers
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone();
            self.send_trade_offers(player, sync_id, offers).await;
        }
    }

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
    ) -> NbtCompound {
        let mut recipes = Vec::with_capacity(offers.len());
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
            recipe.put_int("tier", 0);
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

        let mut data = NbtCompound::new();
        data.put_list("Recipes", recipes);
        data.put_list(
            "TierExpRequirements",
            std::iter::once(0)
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
    ) {
        use pumpkin_protocol::{bedrock::client::CUpdateTrade, codec::var_long::VarLong};

        let java = CMerchantOffers::new(
            VarInt(i32::from(sync_id)),
            offers.clone(),
            // Vanilla `WanderingTrader.mobInteract` opens `openTradingScreen(player,
            // displayName, 1)` -- wandering traders have no level progression, so the
            // level is always fixed at 1.
            VarInt(1),
            VarInt(0),
            false,
            false,
        );
        let bedrock = CUpdateTrade {
            container_id: sync_id,
            r#type: 15,
            size: VarInt(0),
            trader_tier: VarInt(0),
            entity_unique_id: VarLong(i64::from(self.get_entity().entity_id)),
            last_trading_player: VarLong(i64::from(player.entity_id())),
            display_name: ScreenHandlerFactory::get_display_name(self).to_pretty_console(),
            use_new_trade_screen: true,
            using_economy_trade: true,
            data: Self::bedrock_trade_data(&offers),
        };
        player
            .client
            .enqueue_packet_editioned(&java, &bedrock)
            .await;
    }

    /// Vanilla `AbstractVillager.stillValid` (`AbstractVillager.java:304-306`): the menu
    /// stays open only for the current trading player, while the trader is alive and the
    /// player is within entity interaction range + 4.
    fn can_continue_trading(&self, inventory_player: &dyn InventoryPlayer) -> bool {
        let Some(player) = inventory_player.as_any().downcast_ref::<Player>() else {
            return false;
        };
        let trading_player = *self
            .trading_player
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !trading_player_matches(trading_player, player.get_entity().entity_uuid) {
            return false;
        }
        let entity = self.get_entity();
        let range = player
            .living_entity
            .get_attribute_value(&pumpkin_data::attributes::Attributes::ENTITY_INTERACTION_RANGE)
            + 4.0;
        entity.is_alive()
            && entity
                .bounding_box
                .load()
                .squared_magnitude(player.eye_position())
                < range * range
    }

    /// Vanilla `AbstractVillager.notifyTrade` (`AbstractVillager.java:135-142`) plus
    /// `WanderingTrader.rewardTradeXp` (`WanderingTrader.java:158-163`).
    fn complete_trade(&self, offer_index: usize, world: &Arc<World>) {
        let reward_exp = {
            let mut offers = self
                .offers
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let Some(offer) = offers.get_mut(offer_index) else {
                return;
            };
            // Vanilla `MerchantOffer::increaseUses`/`shouldRewardExp`
            // (`MerchantOffer.java:165-167,209-211`) update the completed trade.
            offer.increase_uses();
            offer.should_reward_exp()
        };

        // `notifyTrade`: `ambientSoundTime = -getAmbientSoundInterval()`.
        self.mob_entity
            .ambient_sound_time
            .store(-self.get_ambient_sound_interval(), Ordering::Relaxed);

        // `WanderingTrader::rewardTradeXp`: unlike `Villager`, no persisted XP counter or
        // profession leveling, just a flat `3 + nextInt(4)` orb at `getY() + 0.5`, and only
        // when `MerchantOffer::shouldRewardExp`.
        if reward_exp {
            let position = self.get_entity().pos.load().add_raw(0.0, 0.5, 0.0);
            crate::entity::experience_orb::ExperienceOrbEntity::spawn(
                world,
                position,
                3 + rand::random_range(0..4u32),
            );
        }

        // `notifyTrade`: `CriteriaTriggers.TRADE.trigger(tradingPlayer, ...)`.
        if let Some(player) = self.get_trading_player() {
            trigger_trade_advancement(&player);
        }
    }
}

impl ScreenHandlerFactory for WanderingTraderEntity {
    #[allow(clippy::too_many_lines)]
    fn create_screen_handler(
        &self,
        sync_id: u8,
        player_inventory: &Arc<pumpkin_inventory::player::player_inventory::PlayerInventory>,
        player: &dyn InventoryPlayer,
    ) -> Option<SharedScreenHandler> {
        let offers = self
            .offers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let self_weak = self.self_weak.lock().unwrap().clone().unwrap();

        let mut handler = MerchantScreenHandler::new(
            sync_id,
            player_inventory,
            self.merchant_inventory.clone(),
            offers.clone(),
        );

        // `AbstractVillager.notifyTradeUpdated` (`AbstractVillager.java:152-157)`)
        // calls the concrete `getTradeUpdatedSound` after the payment slots change,
        // while suppressing repeats during the ambient-sound cooldown window.
        let update_sound_weak = self_weak.clone();
        handler.on_trade_updated = Some(Box::new(move |has_result| {
            if let Some(trader) = update_sound_weak.upgrade() {
                let interval = trader.get_ambient_sound_interval();
                let current = trader.mob_entity.ambient_sound_time.load(Ordering::Relaxed);
                if current > -interval + 20
                    && trader
                        .mob_entity
                        .ambient_sound_time
                        .compare_exchange(current, -interval, Ordering::Relaxed, Ordering::Relaxed)
                        .is_ok()
                {
                    trader.get_entity().play_sound(if has_result {
                        Sound::EntityWanderingTraderYes
                    } else {
                        Sound::EntityWanderingTraderNo
                    });
                }
            }
        }));

        // `AbstractVillager.startTrading` sets `tradingPlayer`; `stopTrading` clears it.
        // With it set, `isTrading()` suppresses despawn and `TradeWithPlayerGoal` holds
        // the trader still (`WanderingTrader.java:79, 212`).
        if let Some(server_player) = player.as_any().downcast_ref::<Player>() {
            *self
                .trading_player
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) =
                Some(server_player.get_entity().entity_uuid);
        }

        // Vanilla `MerchantMenu.stillValid` delegates to the merchant
        // (`MerchantMenu.java:68-70`, `AbstractVillager.java:304-306`).
        let validity_weak = self_weak.clone();
        handler.validity_check = Some(Box::new(move |inventory_player| {
            validity_weak
                .upgrade()
                .is_some_and(|trader| trader.can_continue_trading(inventory_player))
        }));

        let close_weak = self_weak.clone();
        handler.on_close = Some(Box::new(move || {
            let close_weak = close_weak.clone();
            Box::pin(async move {
                if let Some(trader) = close_weak.upgrade() {
                    *trader
                        .trading_player
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
                }
            })
        }));

        let world = self.get_entity().world.load_full();
        handler.on_trade = Some(Box::new(move |offer_index| {
            let self_weak = self_weak.clone();
            let world = world.clone();
            Box::pin(async move {
                if let Some(trader) = self_weak.upgrade() {
                    trader.complete_trade(offer_index, &world);
                }
            })
        }));

        let sound_weak = self.self_weak.lock().unwrap().clone().unwrap();
        handler.on_quick_move_trade = Some(Box::new(move || {
            if let Some(trader) = sound_weak.upgrade() {
                // `MerchantMenu.playTradeSound`/`WanderingTrader.getNotifyTradeSound`
                // (`MerchantMenu.java:142-147`, `WanderingTrader.java:191-193`).
                trader
                    .get_entity()
                    .play_sound(pumpkin_data::sound::Sound::EntityWanderingTraderYes);
            }
        }));

        Some(Arc::new(Mutex::new(handler)) as SharedScreenHandler)
    }

    fn get_display_name(&self) -> TextComponent {
        TextComponent::translate("entity.minecraft.wandering_trader", [])
    }
}

impl NBTStorage for WanderingTraderEntity {
    fn write_nbt<'a>(&'a self, nbt: &'a mut NbtCompound) -> crate::entity::NbtFuture<'a, ()> {
        Box::pin(async move {
            self.mob_entity.living_entity.write_nbt(nbt);
            nbt.put_int("DespawnDelay", self.despawn_delay.load(Ordering::Relaxed));
            if let Some(target) = self.wander_target.load() {
                nbt.put(
                    "wander_target",
                    NbtTag::IntArray(vec![target.0.x, target.0.y, target.0.z]),
                );
            }

            let offers = self
                .offers
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
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
        })
    }

    fn read_nbt_non_mut<'a>(&'a self, nbt: &'a NbtCompound) -> crate::entity::NbtFuture<'a, ()> {
        Box::pin(async move {
            self.mob_entity.living_entity.read_nbt_non_mut(nbt);
            if let Some(delay) = nbt.get_int("DespawnDelay") {
                self.despawn_delay.store(delay, Ordering::Relaxed);
            }
            self.wander_target
                .store(nbt.get_int_array("wander_target").and_then(|values| {
                    let &[x, y, z] = values else {
                        return None;
                    };
                    Some(BlockPos::new(x, y, z))
                }));
            // `WanderingTrader.readAdditionalSaveData` (`WanderingTrader.java:149`):
            // `setAge(Math.max(0, getAge()))`.
            if self.get_age() < 0 {
                self.set_age(0);
            }

            if let Some(offers_compound) = nbt.get_compound("Offers")
                && let Some(recipes) = offers_compound.get_list("Recipes")
            {
                let mut offers = self
                    .offers
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                offers.clear();
                for tag in recipes {
                    if let Some(recipe) = tag.extract_compound() {
                        let buy = recipe
                            .get_compound("buy")
                            .and_then(pumpkin_data::item_stack::ItemStack::read_item_stack);
                        let buy_b = recipe
                            .get_compound("buyB")
                            .and_then(pumpkin_data::item_stack::ItemStack::read_item_stack);
                        let sell_item = recipe
                            .get_compound("sell")
                            .and_then(pumpkin_data::item_stack::ItemStack::read_item_stack);

                        if let (Some(buy), Some(sell_item)) = (buy, sell_item) {
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
        })
    }
}

/// Vanilla `WanderingTrader.WanderToPositionGoal` (`WanderingTrader.java:225-268`).
struct WanderToPositionGoal {
    goal_control: Controls,
    stop_distance: f64,
    speed: f64,
    target: Option<BlockPos>,
}

impl WanderToPositionGoal {
    const fn new(stop_distance: f64, speed: f64) -> Self {
        Self {
            goal_control: Controls::MOVE,
            stop_distance,
            speed,
            target: None,
        }
    }

    fn is_too_far(mob: &dyn Mob, target: BlockPos, distance: f64) -> bool {
        let target = target.to_f64() + Vector3::new(0.5, 0.5, 0.5);
        let delta = target - mob.get_entity().pos.load();
        delta.length_squared() > distance * distance
    }
}

impl Goal for WanderToPositionGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        let Some(trader) = mob.cast_any().downcast_ref::<WanderingTraderEntity>() else {
            return false;
        };
        let Some(target) = trader.wander_target.load() else {
            return false;
        };
        if !Self::is_too_far(mob, target, self.stop_distance) {
            return false;
        }
        self.target = Some(target);
        true
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        self.target
            .is_some_and(|target| Self::is_too_far(mob, target, self.stop_distance))
    }

    fn start(&mut self, _mob: &dyn Mob) {}

    fn stop(&mut self, mob: &dyn Mob) {
        if let Some(trader) = mob.cast_any().downcast_ref::<WanderingTraderEntity>() {
            trader.set_wander_target(None);
        }
        mob.get_mob_entity()
            .navigator
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .stop();
        self.target = None;
    }

    fn tick(&mut self, mob: &dyn Mob) {
        let Some(target) = self.target else {
            return;
        };
        let mut navigator = mob
            .get_mob_entity()
            .navigator
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !navigator.is_idle() {
            return;
        }

        let current = mob.get_entity().pos.load();
        let target_center = target.to_f64() + Vector3::new(0.5, 0.5, 0.5);
        let destination = if Self::is_too_far(mob, target, 10.0) {
            current + (target_center - current).normalize() * 10.0
        } else {
            target_center
        };
        navigator.set_progress(NavigatorGoal::new(current, destination, self.speed));
    }

    fn controls(&self) -> Controls {
        self.goal_control
    }
}

impl AgeableMob for WanderingTraderEntity {
    fn get_ageable_data(&self) -> &AgeableData {
        &self.ageable_data
    }

    fn can_be_a_baby(&self) -> bool {
        false
    }
}

/// Which of the two `UseItemGoal`s (`WanderingTrader.java:62-78`) this is.
#[derive(Clone, Copy, PartialEq, Eq)]
enum UseItemKind {
    /// Drink an invisibility potion once it is dark outside and the trader is visible.
    Invisibility,
    /// Drink milk once it is bright outside and the trader is invisible.
    Milk,
}

/// `UseItemGoal.java`, specialized for the wandering trader. Vanilla's generic goal runs the
/// item's own use/finish logic through `startUsingItem`; Pumpkin mobs have no item-use pipeline,
/// so this goal holds the stack in the main hand for the item's 32-tick use duration, plays the
/// trader's drinking sound (`WanderingTrader.getDrinkingSound`) on the living-entity use-effect
/// cadence, then applies the item's effect and the finish sound (`UseItemGoal.stop`).
struct WanderingTraderUseItemGoal {
    trader: Weak<WanderingTraderEntity>,
    kind: UseItemKind,
    remaining_ticks: i32,
}

impl WanderingTraderUseItemGoal {
    /// Potion and milk `Consumable` use duration (1.6s).
    const USE_DURATION: i32 = 32;

    const fn new(trader: Weak<WanderingTraderEntity>, kind: UseItemKind) -> Self {
        Self {
            trader,
            kind,
            remaining_ticks: 0,
        }
    }

    /// `Level.isBrightOutside` (`Level.java:372-374`).
    fn is_bright_outside(world: &World) -> bool {
        world.dimension.fixed_time.is_none() && world.sky_darken.load(Ordering::Relaxed) < 4
    }

    /// `Level.isDarkOutside` (`Level.java:376-378`).
    fn is_dark_outside(world: &World) -> bool {
        world.dimension.fixed_time.is_none() && !Self::is_bright_outside(world)
    }

    fn item(&self) -> ItemStack {
        match self.kind {
            UseItemKind::Invisibility => create_invisibility_potion(),
            UseItemKind::Milk => ItemStack::new(1, &Item::MILK_BUCKET),
        }
    }

    fn set_main_hand(trader: &WanderingTraderEntity, stack: ItemStack) {
        let living = &trader.mob_entity.living_entity;
        living
            .entity_equipment
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .put(&EquipmentSlot::MAIN_HAND, stack.clone());
        living.send_equipment_changes(&[(EquipmentSlot::MAIN_HAND, stack)]);
    }
}

impl Goal for WanderingTraderUseItemGoal {
    fn can_start(&mut self, _mob: &dyn Mob) -> bool {
        let Some(trader) = self.trader.upgrade() else {
            return false;
        };
        let living = &trader.mob_entity.living_entity;
        let world = living.entity.world.load();
        let is_invisible = living.has_effect(&StatusEffect::INVISIBILITY);
        match self.kind {
            UseItemKind::Invisibility => Self::is_dark_outside(&world) && !is_invisible,
            UseItemKind::Milk => Self::is_bright_outside(&world) && is_invisible,
        }
    }

    /// `UseItemGoal.canContinueToUse`: `mob.isUsingItem()`.
    fn should_continue(&mut self, _mob: &dyn Mob) -> bool {
        self.remaining_ticks > 0
            && self
                .trader
                .upgrade()
                .is_some_and(|trader| trader.get_entity().is_alive())
    }

    fn start(&mut self, _mob: &dyn Mob) {
        let Some(trader) = self.trader.upgrade() else {
            return;
        };
        self.remaining_ticks = Self::USE_DURATION;
        Self::set_main_hand(&trader, self.item());
    }

    fn tick(&mut self, _mob: &dyn Mob) {
        let Some(trader) = self.trader.upgrade() else {
            return;
        };
        self.remaining_ticks -= 1;
        let entity = trader.get_entity();
        // `LivingEntity.shouldTriggerItemUseEffects`: past the first 21.875% of the use
        // duration, every 4 ticks.
        let ticks_used = Self::USE_DURATION - self.remaining_ticks;
        if self.remaining_ticks > 0
            && ticks_used as f32 > Self::USE_DURATION as f32 * 0.218_75
            && self.remaining_ticks % 4 == 0
        {
            entity.play_sound(match self.kind {
                UseItemKind::Invisibility => Sound::EntityWanderingTraderDrinkPotion,
                UseItemKind::Milk => Sound::EntityWanderingTraderDrinkMilk,
            });
        }
        if self.remaining_ticks == 0 {
            let living = &trader.mob_entity.living_entity;
            match self.kind {
                // `Potions.INVISIBILITY`: invisibility for 3600 ticks.
                UseItemKind::Invisibility => {
                    for effect in pumpkin_data::potion::Potion::INVISIBILITY.effects {
                        living.add_effect(effect.clone());
                    }
                }
                // Milk clears every status effect.
                UseItemKind::Milk => {
                    living.remove_all_effects();
                }
            }
        }
    }

    /// `UseItemGoal.stop`: empty the main hand and play the finish sound.
    fn stop(&mut self, _mob: &dyn Mob) {
        if let Some(trader) = self.trader.upgrade() {
            Self::set_main_hand(&trader, ItemStack::EMPTY.clone());
            let entity = trader.get_entity();
            entity.world.load().play_sound(
                match self.kind {
                    UseItemKind::Invisibility => Sound::EntityWanderingTraderDisappeared,
                    UseItemKind::Milk => Sound::EntityWanderingTraderReappeared,
                },
                SoundCategory::Neutral,
                &entity.pos.load(),
            );
        }
        self.remaining_ticks = 0;
    }

    fn controls(&self) -> Controls {
        Controls::empty()
    }
}

impl Mob for WanderingTraderEntity {
    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    fn as_ageable(&self) -> Option<&dyn AgeableMob> {
        Some(self)
    }

    fn get_trading_player(&self) -> Option<Arc<Player>> {
        let uuid = (*self
            .trading_player
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner))?;
        self.get_entity().world.load().get_player_by_uuid(uuid)
    }

    /// Vanilla `WanderingTrader.getAmbientSound` (`WanderingTrader.java:165-168`).
    fn get_ambient_sound(&self) -> Option<Sound> {
        Some(if self.get_trading_player().is_some() {
            Sound::EntityWanderingTraderTrade
        } else {
            Sound::EntityWanderingTraderAmbient
        })
    }

    /// Vanilla `WanderingTrader::maybeDespawn` (`WanderingTrader.java:211-215`): decrements
    /// `despawnDelay` each tick while `!isTrading()`; discards the entity at 0.
    fn mob_tick<'a>(
        &'a self,
        _caller: &'a Arc<dyn EntityBase>,
    ) -> crate::entity::EntityBaseFuture<'a, ()> {
        Box::pin(async move {
            if self
                .trading_player
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .is_some()
            {
                return;
            }
            let delay = self.despawn_delay.load(Ordering::Relaxed);
            if delay > 0 {
                let new_delay = delay - 1;
                self.despawn_delay.store(new_delay, Ordering::Relaxed);
                if new_delay <= 0 {
                    let world = self.get_entity().world.load();
                    world.remove_entity(self);
                }
            }
        })
    }

    /// Vanilla `WanderingTrader.mobInteract` (`WanderingTrader.java:107-127`).
    fn mob_interact<'a>(
        &'a self,
        player: &'a Arc<Player>,
        item_stack: &'a mut ItemStack,
    ) -> crate::entity::EntityBaseFuture<'a, bool> {
        let player = player.clone();
        Box::pin(async move {
            if item_stack.item == &Item::VILLAGER_SPAWN_EGG
                || !self.get_entity().is_alive()
                || self.is_trading()
                || self.is_baby()
            {
                return false;
            }

            player.increment_stat(
                pumpkin_data::statistic::StatisticCategory::Custom,
                pumpkin_data::statistic::CustomStatistic::TalkedToVillager as i32,
                1,
            );

            let mut offers = self
                .offers
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if offers.is_empty() {
                drop(offers);
                self.update_trades();
                offers = self
                    .offers
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
            }

            // Vanilla: `getOffers().isEmpty()` returns `CONSUME` (acknowledge, no menu).
            if offers.is_empty() {
                return true;
            }
            drop(offers);

            self.open_trading_screen(&player).await;

            true
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{
        AVOIDED, WANDERING_TRADER_TRADE_SET_BUYING, WANDERING_TRADER_TRADE_SET_COMMON,
        WANDERING_TRADER_TRADE_SET_UNCOMMON, add_offers_from_trade_set, trading_player_matches,
    };
    use pumpkin_data::entity::EntityType;

    /// `WanderingTrader.registerGoals` (`WanderingTrader.java:80-86`) registers exactly seven
    /// `AvoidEntityGoal`s, and the flee radii are not uniform -- pillagers are feared from
    /// 15 blocks, evokers and illusioners from 12, zoglins from 10, the rest from 8.
    #[test]
    fn avoided_table_matches_vanilla() {
        // Six single classes plus `Zombie` and its four subclasses.
        assert_eq!(AVOIDED.len(), 11);
        let radius = |ty: &EntityType| {
            AVOIDED
                .iter()
                .find(|(t, _)| t.id == ty.id)
                .map(|(_, d)| *d)
                .expect("entity type missing from AVOIDED")
        };
        for zombie in [
            &EntityType::ZOMBIE,
            &EntityType::DROWNED,
            &EntityType::HUSK,
            &EntityType::ZOMBIE_VILLAGER,
            &EntityType::ZOMBIFIED_PIGLIN,
        ] {
            assert!((radius(zombie) - 8.0).abs() < f64::EPSILON);
        }
        assert!((radius(&EntityType::EVOKER) - 12.0).abs() < f64::EPSILON);
        assert!((radius(&EntityType::VINDICATOR) - 8.0).abs() < f64::EPSILON);
        assert!((radius(&EntityType::VEX) - 8.0).abs() < f64::EPSILON);
        assert!((radius(&EntityType::PILLAGER) - 15.0).abs() < f64::EPSILON);
        assert!((radius(&EntityType::ILLUSIONER) - 12.0).abs() < f64::EPSILON);
        assert!((radius(&EntityType::ZOGLIN) - 10.0).abs() < f64::EPSILON);
    }

    // Vanilla `MerchantContainer.stillValid` compares the current trading player by identity
    // (`MerchantContainer.java:79-81`).
    #[test]
    fn trading_player_validation_requires_the_current_player() {
        let current = uuid::Uuid::new_v4();
        let other = uuid::Uuid::new_v4();

        assert!(trading_player_matches(Some(current), current));
        assert!(!trading_player_matches(Some(current), other));
        assert!(!trading_player_matches(None, current));
    }

    /// `WanderingTrader.updateTrades` pulls 2 buying, 2 uncommon and 5 common offers.
    #[test]
    fn update_trades_creates_expected_trade_count() {
        let mut offers = Vec::new();
        let mut rng = rand::rng();
        for trade_set in [
            WANDERING_TRADER_TRADE_SET_BUYING,
            WANDERING_TRADER_TRADE_SET_UNCOMMON,
            WANDERING_TRADER_TRADE_SET_COMMON,
        ] {
            add_offers_from_trade_set(&mut offers, trade_set, &mut rng);
        }
        assert_eq!(offers.len(), 9);
    }
}
