//! `BeehiveBlock` (`BeehiveBlock.java`).
//!
//! Ported: the comparator output, `useItemOn`'s shears/glass-bottle harvest at honey level 5,
//! `resetHoneyLevel`, `releaseBeesAndResetHoneyLevel`, `hiveContainsBees` and `angerNearbyBees`.
//!
//! Documented reductions:
//!
//! - `dropHoneycomb` pulls `BuiltInLootTables.HARVEST_BEEHIVE`. Pumpkin has no block-interact
//!   loot-table path, so the three honeycomb that table always yields are popped directly.
//! - `getDrops` also releases bees for a small set of explosive entity sources in vanilla. The
//!   live explosion context carries no source entity, so that source-specific branch remains
//!   blocked; the common `onExplosionHit` anger path is wired below.
//! - `CriteriaTriggers.BEE_NEST_DESTROYED` and `Stats.ITEM_USED` have no equivalent here.

use crate::block::entities::beehive::{
    BeeReleaseStatus, BeehiveBlockEntity, MAX_HONEY_LEVELS, is_smokey_pos,
};
use crate::block::entities::collect_components_from_block_entity;
use crate::block::registry::BlockActionResult;
use crate::block::{
    BlockBehaviour, BlockFuture, BlockMetadata, BrokenArgs, ExplodeArgs, GetComparatorOutputArgs,
    OnPlaceArgs, PlayerWillDestroyArgs, UseWithItemArgs,
};
use crate::entity::passive::bee::as_bee;
use crate::entity::{Entity, EntityBase, item::ItemEntity};
use crate::world::World;
use pumpkin_data::block_properties::{BeeNestLikeProperties, BlockProperties};
use pumpkin_data::data_component::DataComponent;
use pumpkin_data::data_component_impl::BlockStateImpl;
use pumpkin_data::entity::EntityType;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_data::{Block, BlockId, BlockStateId};
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_util::{GameMode, Hand};
use pumpkin_world::world::BlockFlags;
use rand::RngExt;
use std::borrow::Cow;
use std::sync::Arc;

/// `BuiltInLootTables.HARVEST_BEEHIVE` yields a flat three honeycomb.
const HONEYCOMB_PER_HARVEST: u8 = 3;
/// `BeehiveBlock.angerNearbyBees` inflates the hive's box by 8/6/8; the widest half-extent is
/// what the sphere query here has to cover.
const ANGER_RADIUS: f64 = 8.0;

pub struct BeehiveBlock;

impl BlockMetadata for BeehiveBlock {
    fn ids() -> Box<[BlockId]> {
        [BlockId::BEEHIVE, BlockId::BEE_NEST].into()
    }
}

impl BlockBehaviour for BeehiveBlock {
    /// `BeehiveBlock.playerWillDestroy` (`BeehiveBlock.java:290-307`): creative destruction with
    /// block drops enabled preserves occupants and honey level in the dropped hive item.
    fn player_will_destroy<'a>(&'a self, args: PlayerWillDestroyArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async move {
            if args.player.gamemode.load() != GameMode::Creative
                || !args.world.level_info.load().game_rules.block_drops
            {
                return;
            }
            let Some(block_entity) = args.world.get_block_entity(args.position) else {
                return;
            };
            let Some(hive) = block_entity.as_any().downcast_ref::<BeehiveBlockEntity>() else {
                return;
            };
            let props = BeeNestLikeProperties::from_state_id(args.state.id, args.block);
            if hive.is_empty().await && props.honey_level == 0 {
                return;
            }
            let item = if args.block.id == Block::BEE_NEST.id {
                &Item::BEE_NEST
            } else {
                &Item::BEEHIVE
            };
            let mut components = collect_components_from_block_entity(block_entity.as_ref()).await;
            components.push((
                DataComponent::BlockState,
                Some(Box::new(BlockStateImpl {
                    properties: Cow::Owned(vec![(
                        Cow::Borrowed("honey_level"),
                        Cow::Owned(props.honey_level.to_string()),
                    )]),
                })),
            ));
            args.world
                .drop_stack(
                    args.position,
                    ItemStack::new_with_component(1, item, components),
                )
                .await;
        })
    }

    /// `BeehiveBlock.playerDestroy` (`BeehiveBlock.java:91-108`): after removal, release stored
    /// bees unless Silk Touch prevents bee spawning, then anger nearby bees.
    fn broken<'a>(&'a self, args: BrokenArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async move {
            let Some(block_entity) = args.block_entity else {
                return;
            };
            let Some(hive) = block_entity.as_any().downcast_ref::<BeehiveBlockEntity>() else {
                return;
            };
            let tool = args.player.inventory().held_item().await;
            if tool.get_enchantment_level(&pumpkin_data::Enchantment::SILK_TOUCH) > 0 {
                return;
            }
            hive.empty_all_living_from_hive_with_state(
                args.world,
                Some(args.player),
                BeeReleaseStatus::Emergency,
                Some((args.block.id, args.state.id)),
            )
            .await;
            anger_nearby_bees(args.world, args.position).await;
        })
    }

    /// `BeehiveBlock.onExplosionHit` (`BeehiveBlock.java:111-117`), dispatched by the live
    /// `BlockBehaviour::explode` callback for the block's explosion interaction.
    fn explode<'a>(&'a self, args: ExplodeArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async move {
            anger_nearby_bees(args.world, args.position).await;
        })
    }

    /// `BeehiveBlock.getStateForPlacement` (`BeehiveBlock.java:269-272`) and
    /// `BlockItem.updateBlockEntityComponents` (`BlockItem.java:101-106`): hives face away from
    /// the player, while the item's `BlockState` component restores stored honey level.
    fn on_place<'a>(&'a self, args: OnPlaceArgs<'a>) -> BlockFuture<'a, BlockStateId> {
        Box::pin(async move {
            let mut props = BeeNestLikeProperties::default(args.block);
            props.facing = args.player.get_entity().get_horizontal_facing().opposite();
            let hand = Hand::from_packet_id(args.use_item_on.hand.0).unwrap_or(Hand::Right);
            let item_stack = args.player.inventory().get_stack_in_hand(hand).await;
            if let Some(block_state) = item_stack.get_data_component::<BlockStateImpl>()
                && let Some((_, value)) = block_state
                    .properties
                    .iter()
                    .find(|(key, _)| key.as_ref() == "honey_level")
                && let Ok(honey_level) = value.parse::<u8>()
            {
                props.honey_level = honey_level.min(MAX_HONEY_LEVELS);
            }
            props.to_state_id(args.block)
        })
    }

    fn get_comparator_output<'a>(
        &'a self,
        args: GetComparatorOutputArgs<'a>,
    ) -> BlockFuture<'a, Option<u8>> {
        Box::pin(async move {
            let state_id = args.world.get_block_state_id(args.position);
            let props = BeeNestLikeProperties::from_state_id(state_id, args.block);
            Some(props.honey_level)
        })
    }

    /// `BeehiveBlock.useItemOn`.
    fn use_with_item<'a>(
        &'a self,
        args: UseWithItemArgs<'a>,
    ) -> BlockFuture<'a, BlockActionResult> {
        Box::pin(async move {
            let state_id = args.world.get_block_state_id(args.position);
            let props = BeeNestLikeProperties::from_state_id(state_id, args.block);
            if props.honey_level < MAX_HONEY_LEVELS {
                return BlockActionResult::PassToDefaultBlockAction;
            }

            let player_pos = args.player.get_entity().pos.load();
            let item = args.item_stack.item;
            if item.id == Item::SHEARS.id {
                drop_honeycomb(args.world, args.position).await;
                args.world
                    .play_sound(Sound::BlockBeehiveShear, SoundCategory::Blocks, &player_pos);
                args.player.damage_held_item(1).await;
            } else if item.id == Item::GLASS_BOTTLE.id {
                if args.player.gamemode.load() != GameMode::Creative {
                    args.item_stack.decrement(1);
                }
                args.world
                    .play_sound(Sound::ItemBottleFill, SoundCategory::Blocks, &player_pos);
                let mut honey = ItemStack::new(1, &Item::HONEY_BOTTLE);
                if !args
                    .player
                    .inventory()
                    .insert_stack_anywhere(&mut honey)
                    .await
                {
                    args.player.drop_item(honey).await;
                }
            } else {
                return BlockActionResult::PassToDefaultBlockAction;
            }

            // `hiveEmptied`: the harvest happened, so the honey level resets either way, and the
            // bees stay calm only if a campfire is smoking underneath the hive.
            let sedated = is_smokey_pos(args.world, args.position);
            reset_honey_level(args.world, args.block, args.position, state_id).await;

            if !sedated
                && let Some(handle) = args.world.get_block_entity(args.position)
                && let Some(hive) = handle.as_any().downcast_ref::<BeehiveBlockEntity>()
            {
                if !hive.is_empty().await {
                    anger_nearby_bees(args.world, args.position).await;
                }
                hive.empty_all_living_from_hive(
                    args.world,
                    Some(args.player),
                    BeeReleaseStatus::Emergency,
                )
                .await;
            }

            BlockActionResult::Success
        })
    }
}

/// `BeehiveBlock.resetHoneyLevel`.
async fn reset_honey_level(
    world: &Arc<World>,
    block: &Block,
    position: &BlockPos,
    state_id: BlockStateId,
) {
    let mut props = BeeNestLikeProperties::from_state_id(state_id, block);
    props.honey_level = 0;
    world
        .set_block_state(position, props.to_state_id(block), BlockFlags::NOTIFY_ALL)
        .await;
}

/// `BeehiveBlock.dropHoneycomb`.
async fn drop_honeycomb(world: &Arc<World>, position: &BlockPos) {
    let drop_pos = Vector3::new(
        f64::from(position.0.x) + 0.5,
        f64::from(position.0.y) + 0.5,
        f64::from(position.0.z) + 0.5,
    );
    let item_entity = Arc::new(ItemEntity::new(
        Entity::new(world.clone(), drop_pos, &EntityType::ITEM),
        ItemStack::new(HONEYCOMB_PER_HARVEST, &Item::HONEYCOMB),
    ));
    world.spawn_entity(item_entity).await;
}

/// `BeehiveBlock.angerNearbyBees`: every bee within the box that has no target picks a random
/// nearby player to be angry at.
///
/// Reduction: Pumpkin's entity queries are spherical, so the 8/6/8 box becomes a radius-8
/// sphere. That is a superset on the Y axis by at most two blocks.
async fn anger_nearby_bees(world: &Arc<World>, position: &BlockPos) {
    let centre = position.to_f64();
    let players = world.get_nearby_players(centre, ANGER_RADIUS);
    if players.is_empty() {
        return;
    }

    for entity in world
        .get_nearby_entities(centre, ANGER_RADIUS)
        .into_values()
    {
        let Some(bee) = as_bee(&entity) else {
            continue;
        };
        if bee.mob_entity.get_target().await.is_some() {
            continue;
        }
        let index = rand::rng().random_range(0..players.len());
        let target: Arc<dyn EntityBase> = players[index].clone();
        bee.mob_entity.set_target(Some(target)).await;
    }
}
