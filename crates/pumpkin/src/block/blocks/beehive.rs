//! `BeehiveBlock` (`BeehiveBlock.java`).
//!
//! Ported: the comparator output, `useItemOn`'s shears/glass-bottle harvest at honey level 5,
//! `resetHoneyLevel`, `releaseBeesAndResetHoneyLevel`, `hiveContainsBees` and `angerNearbyBees`.
//!
//! Documented reductions:
//!
//! - `dropHoneycomb` pulls `BuiltInLootTables.HARVEST_BEEHIVE`. Pumpkin has no block-interact
//!   loot-table path, so the three honeycomb that table always yields are popped directly.
//! - `playerDestroy`, `onExplosionHit`, `getDrops` and `updateShape` also empty and anger a hive
//!   in vanilla. Those hooks need block-break/explosion plumbing this block does not own yet;
//!   the fire case is covered from the block entity's own tick (`isFireNearby`) instead.
//! - `CriteriaTriggers.BEE_NEST_DESTROYED` and `Stats.ITEM_USED` have no equivalent here.

use crate::block::entities::beehive::{
    BeeReleaseStatus, BeehiveBlockEntity, MAX_HONEY_LEVELS, is_smokey_pos,
};
use crate::block::registry::BlockActionResult;
use crate::block::{
    BlockBehaviour, BlockFuture, BlockMetadata, GetComparatorOutputArgs, UseWithItemArgs,
};
use crate::entity::mob::Mob;
use crate::entity::passive::bee::as_bee;
use crate::entity::{Entity, EntityBase, item::ItemEntity};
use crate::world::World;
use pumpkin_data::block_properties::{BeeNestLikeProperties, BlockProperties};
use pumpkin_data::entity::EntityType;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_data::{Block, BlockId, BlockStateId};
use pumpkin_util::GameMode;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_world::world::BlockFlags;
use rand::RngExt;
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
        bee.set_mob_target(Some(target)).await;
    }
}
