// Legacy invariant checks retained for vanilla behavior; migrate these paths before removing this allow.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::{Arc, Weak};

use pumpkin_data::entity::EntityType;
use pumpkin_data::sound::Sound;
use pumpkin_inventory::merchant::merchant_screen_handler::MerchantScreenHandler;
use pumpkin_inventory::screen_handler::{
    BoxFuture, InventoryPlayer, ScreenHandlerFactory, SharedScreenHandler,
};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_protocol::codec::var_int::VarInt;
use pumpkin_protocol::java::client::play::CMerchantOffers;
use pumpkin_util::text::TextComponent;
use pumpkin_world::inventory::SimpleInventory;
use tokio::sync::Mutex;

use crate::entity::player::Player;
use crate::entity::{
    Entity, EntityBase, NBTStorage,
    ai::goal::{
        avoid_entity::AvoidEntityGoal, escape_danger::EscapeDangerGoal, interact::InteractGoal,
        look_at_entity::LookAtEntityGoal, look_at_trading_player,
        move_towards_restriction::MoveTowardsRestrictionGoal, swim::SwimGoal,
        trade_with_player::TradeWithPlayerGoal, wander_around::WanderAroundGoal,
    },
    mob::{Mob, MobEntity},
};

/// Vanilla `WanderingTrader::despawnDelay` default (`WanderingTrader.java:52-53`): `0`,
/// meaning "never auto-despawns" until something sets it positive (`maybeDespawn` only acts
/// when `despawnDelay > 0`). Only `WanderingTraderSpawner.spawn` sets it to 48000 on a
/// natural spawn; since that spawner is deferred (see module doc), every trader created here
/// via spawn egg/command correctly never despawns on its own, matching vanilla for a
/// non-naturally-spawned trader.
const DEFAULT_DESPAWN_DELAY: i32 = 0;

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
/// **Not implemented, deferred with reasons:**
/// - `WanderingTraderSpawner`/`WanderingTraderData` (per-world periodic natural spawner) --
///   a separate, larger piece of world-tick + saved-data infrastructure, out of scope here.
///   Without it, a trader can currently only appear via spawn egg/command.
/// - `WanderToPositionGoal` (`WanderingTrader.java:89`) -- steers back to the `wanderTarget`
///   that only `WanderingTraderSpawner` ever sets. With that spawner unimplemented the target
///   is always absent, so the goal could never fire; left out rather than registered dead.
/// - The two `UseItemGoal`s (`WanderingTrader.java:62-78`, drink invisibility after dark and
///   milk at dawn) -- vanilla's generic `UseItemGoal` has no Rust counterpart yet.
/// - `LookAtTradingPlayerGoal` (`WanderingTrader.java:88`) is ported as
///   [`crate::entity::ai::goal::look_at_trading_player::LookAtTradingPlayerGoal`].
pub struct WanderingTraderEntity {
    pub mob_entity: MobEntity,
    pub offers: Mutex<Vec<pumpkin_protocol::java::client::play::MerchantOffer>>,
    pub merchant_inventory: Arc<SimpleInventory>,
    /// Vanilla `despawnDelay` (`WanderingTrader.java:52-53, 195-201`). Ticks remaining before
    /// natural despawn; decremented in `mob_tick` while not trading.
    pub despawn_delay: AtomicI32,
    /// Vanilla `AbstractVillager.tradingPlayer`. Set while the merchant screen is open, which
    /// is what makes `isTrading()` (`WanderingTrader.java:212`) and `TradeWithPlayerGoal`
    /// meaningful here. Mirrors `VillagerEntity::trading_player`.
    pub trading_player: std::sync::Mutex<Option<uuid::Uuid>>,
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
            trading_player: std::sync::Mutex::new(None),
            self_weak: std::sync::Mutex::new(None),
        };
        let mob_arc = Arc::new(trader);
        *mob_arc.self_weak.lock().unwrap() = Some(Arc::downgrade(&mob_arc));
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

            // `WanderingTrader.registerGoals` (`WanderingTrader.java:60-94`).
            goal_selector.add_goal(0, Box::new(SwimGoal::default()));
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

    /// Vanilla `WanderingTrader::updateTrades` (`WanderingTrader.java:129-135`), via the
    /// shared `AbstractVillager.addOffersFromTradeSet` helper: pulls buying, then uncommon,
    /// then common trade sets, in that fixed order.
    pub async fn update_trades(&self) {
        use pumpkin_data::villager::{
            WANDERING_TRADER_TRADE_SET_BUYING, WANDERING_TRADER_TRADE_SET_COMMON,
            WANDERING_TRADER_TRADE_SET_UNCOMMON,
        };
        use pumpkin_protocol::codec::item_stack_seralizer::ItemStackSerializer;
        use rand::seq::IndexedRandom;
        use std::borrow::Cow;

        let mut offers = self.offers.lock().await;
        let mut rng = rand::rng();

        for trade_set in [
            WANDERING_TRADER_TRADE_SET_BUYING,
            WANDERING_TRADER_TRADE_SET_UNCOMMON,
            WANDERING_TRADER_TRADE_SET_COMMON,
        ] {
            let chosen_trades = trade_set.trades.sample(&mut rng, trade_set.amount as usize);
            for trade in chosen_trades {
                offers.push(pumpkin_protocol::java::client::play::MerchantOffer {
                    base_cost_a: ItemStackSerializer(Cow::Owned(
                        pumpkin_data::item_stack::ItemStack::new(
                            trade.wants.count as u8,
                            trade.wants.item,
                        ),
                    )),
                    output: ItemStackSerializer(Cow::Owned(
                        pumpkin_data::item_stack::ItemStack::new(
                            trade.gives.count as u8,
                            trade.gives.item,
                        ),
                    )),
                    cost_b: trade.wants_b.as_ref().map(|b| {
                        ItemStackSerializer(Cow::Owned(pumpkin_data::item_stack::ItemStack::new(
                            b.count as u8,
                            b.item,
                        )))
                    }),
                    reward_exp: true,
                    uses: 0,
                    max_uses: trade.max_uses,
                    xp: trade.xp,
                    special_price: 0,
                    price_multiplier: trade.price_multiplier,
                    demand: 0,
                });
            }
        }
    }

    pub async fn open_trading_screen(&self, player: &Arc<Player>) {
        if let Some(sync_id) = player.open_handled_screen(self, None).await {
            let offers = self.offers.lock().await.clone();
            player
                .send_client_packet(&CMerchantOffers::new(
                    VarInt(sync_id as i32),
                    offers,
                    // Vanilla `WanderingTrader.mobInteract` opens `openTradingScreen(player,
                    // displayName, 1)` -- wandering traders have no level progression, so the
                    // level is always fixed at 1.
                    VarInt(1),
                    VarInt(0),
                    false,
                    false,
                ))
                .await;
        }
    }
}

impl ScreenHandlerFactory for WanderingTraderEntity {
    fn create_screen_handler<'a>(
        &'a self,
        sync_id: u8,
        player_inventory: &'a Arc<pumpkin_inventory::player::player_inventory::PlayerInventory>,
        player: &'a dyn InventoryPlayer,
    ) -> BoxFuture<'a, Option<SharedScreenHandler>> {
        Box::pin(async move {
            let offers = self.offers.lock().await;
            let self_weak = self.self_weak.lock().unwrap().clone().unwrap();

            let mut handler = MerchantScreenHandler::new(
                sync_id,
                player_inventory,
                self.merchant_inventory.clone(),
                offers.clone(),
            )
            .await;

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
                            .compare_exchange(
                                current,
                                -interval,
                                Ordering::Relaxed,
                                Ordering::Relaxed,
                            )
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

            handler.on_trade = Some(Box::new(move |offer_index| {
                let self_weak = self_weak.clone();
                Box::pin(async move {
                    if let Some(trader) = self_weak.upgrade() {
                        let reward_exp = {
                            let mut offers = trader.offers.lock().await;
                            if offer_index >= offers.len() {
                                return;
                            }
                            let offer = &mut offers[offer_index];
                            offer.uses += 1;
                            offer.reward_exp
                        };

                        // `WanderingTrader::rewardTradeXp` (WanderingTrader.java:158-163): unlike
                        // `Villager`, no persisted XP counter or profession leveling, just a flat
                        // orb, and only when `MerchantOffer::shouldRewardExp`.
                        if reward_exp {
                            let entity = trader.get_entity();
                            crate::entity::experience_orb::ExperienceOrbEntity::spawn(
                                &entity.world.load(),
                                entity.pos.load(),
                                3 + rand::random_range(0..4u32),
                            )
                            .await;
                        }
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
        })
    }

    fn get_display_name(&self) -> TextComponent {
        TextComponent::text("Wandering Trader")
    }
}

impl NBTStorage for WanderingTraderEntity {
    fn write_nbt<'a>(&'a self, nbt: &'a mut NbtCompound) -> crate::entity::NbtFuture<'a, ()> {
        Box::pin(async move {
            self.mob_entity.living_entity.write_nbt(nbt).await;
            nbt.put_int("DespawnDelay", self.despawn_delay.load(Ordering::Relaxed));

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
        })
    }

    fn read_nbt_non_mut<'a>(&'a self, nbt: &'a NbtCompound) -> crate::entity::NbtFuture<'a, ()> {
        Box::pin(async move {
            self.mob_entity.living_entity.read_nbt_non_mut(nbt).await;
            if let Some(delay) = nbt.get_int("DespawnDelay") {
                self.despawn_delay.store(delay, Ordering::Relaxed);
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

impl Mob for WanderingTraderEntity {
    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
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
                    world.remove_entity(self).await;
                }
            }
        })
    }

    fn mob_interact<'a>(
        &'a self,
        player: &'a Arc<Player>,
        _item_stack: &'a mut pumpkin_data::item_stack::ItemStack,
    ) -> crate::entity::EntityBaseFuture<'a, bool> {
        let player = player.clone();
        Box::pin(async move {
            if self.get_entity().age.load(Ordering::Relaxed) < 0 {
                return true;
            }

            let mut offers = self.offers.lock().await;
            if offers.is_empty() {
                drop(offers);
                self.update_trades().await;
                offers = self.offers.lock().await;
            }

            // Vanilla: `getOffers().isEmpty()` returns `CONSUME` (acknowledge, no menu).
            if offers.is_empty() {
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
mod tests {
    use super::AVOIDED;
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
}
