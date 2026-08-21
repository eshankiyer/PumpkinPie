use std::sync::Arc;
use std::sync::atomic::Ordering;

use pumpkin_data::{Block, BlockState};
use pumpkin_macros::pumpkin_block;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;

use crate::block::entities::end_gateway::EndGatewayBlockEntity;
use crate::block::{BlockBehaviour, BlockFuture, OnEntityCollisionArgs, PlacedArgs};
use crate::world::World;

/// `end_gateway` (`EndGatewayBlock.java`).
///
/// Two pieces of `TheEndGatewayBlockEntity` are deliberately not carried across:
///
/// * **Gateway creation.** `getPortalPosition` (`TheEndGatewayBlockEntity.java:128-145`) generates
///   a brand new exit gateway through `spawnGatewayPortal` and `findOrCreateValidTeleportPos`,
///   which place the `END_ISLAND` configured feature. Runtime feature placement needs a
///   `GenerationCache` that a live `World` cannot supply, so a gateway with no recorded
///   `exit_portal` stays inert instead of minting one.
/// * **The 40-tick gateway-side cooldown.** `triggerCooldown`
///   (`TheEndGatewayBlockEntity.java:111-116`) stores `teleportCooldown` on the block entity and
///   `portalTick` decrements it. That is block-entity state and a block-entity ticker, neither of
///   which lives in this file. The teleporting entity's own `portal_cooldown` is used as the gate
///   instead, so a gateway can be re-entered sooner than in vanilla.
#[pumpkin_block("minecraft:end_gateway")]
pub struct EndGatewayBlock;

impl BlockBehaviour for EndGatewayBlock {
    /// `EndGatewayBlock.newBlockEntity` (`EndGatewayBlock.java:40-43`).
    fn placed<'a>(&'a self, args: PlacedArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async move {
            args.world
                .add_block_entity(Arc::new(EndGatewayBlockEntity::new(*args.position)));
        })
    }

    /// `EndGatewayBlock.entityInside` (`EndGatewayBlock.java:88-99`) plus the destination lookup
    /// from `EndGatewayBlock.getPortalDestination` (`EndGatewayBlock.java:101-123`).
    fn on_entity_collision<'a>(&'a self, args: OnEntityCollisionArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async move {
            let entity = args.entity.get_entity();

            // `Entity.canUsePortal(false)`: alive, and not a passenger.
            if !entity.is_alive() || entity.has_vehicle().await {
                return;
            }
            if entity.portal_cooldown.load(Ordering::Relaxed) > 0 {
                return;
            }

            let Some(block_entity) = args.world.get_block_entity(args.position) else {
                return;
            };
            let Some(gateway) = block_entity
                .as_any()
                .downcast_ref::<EndGatewayBlockEntity>()
            else {
                return;
            };

            let Some(exit_portal) = *gateway.exit_portal.lock().await else {
                return;
            };
            let exact = *gateway.exact_teleport.lock().await;

            // `TheEndGatewayBlockEntity.getPortalPosition` (:139-142).
            let target = if exact {
                exit_portal
            } else {
                find_exit_position(args.world, &exit_portal)
            };
            // `Vec3.atBottomCenterOf`.
            let destination = Vector3::new(
                f64::from(target.0.x) + 0.5,
                f64::from(target.0.y),
                f64::from(target.0.z) + 0.5,
            );

            let Some(entity_arc) = args.world.get_entity_by_id(entity.entity_id) else {
                return;
            };
            // `Entity.getDimensionChangingDelay` (300 ticks; `ServerPlayer` overrides it to 10).
            let cooldown = if entity.entity_type == &pumpkin_data::entity::EntityType::PLAYER {
                10
            } else {
                300
            };
            entity.portal_cooldown.store(cooldown, Ordering::Relaxed);
            entity_arc
                .teleport(destination, None, None, args.world.clone())
                .await;
        })
    }
}

/// `TheEndGatewayBlockEntity.findExitPosition` (`TheEndGatewayBlockEntity.java:147-151`).
fn find_exit_position(world: &World, exit_portal: &BlockPos) -> BlockPos {
    let around = exit_portal.offset(Vector3::new(0, 2, 0));
    find_tallest_block(world, &around, 5, false).up()
}

/// `TheEndGatewayBlockEntity.findTallestBlock` (`TheEndGatewayBlockEntity.java:202-221`).
fn find_tallest_block(
    world: &World,
    around: &BlockPos,
    dist: i32,
    allow_bedrock: bool,
) -> BlockPos {
    let min_y = world.dimension.min_y;
    let max_y = min_y + world.dimension.height - 1;
    let mut tallest: Option<BlockPos> = None;

    for xd in -dist..=dist {
        for zd in -dist..=dist {
            if xd == 0 && zd == 0 && !allow_bedrock {
                continue;
            }
            let floor = tallest.map_or(min_y, |pos| pos.0.y);
            let mut y = max_y;
            while y > floor {
                let pos = BlockPos::new(around.0.x + xd, y, around.0.z + zd);
                let (block, state) = world.get_block_and_state(&pos);
                if is_collision_shape_full_block(state)
                    && (allow_bedrock || block.id != Block::BEDROCK.id)
                {
                    tallest = Some(pos);
                    break;
                }
                y -= 1;
            }
        }
    }

    tallest.unwrap_or(*around)
}

const fn is_collision_shape_full_block(state: &BlockState) -> bool {
    state.is_full_cube()
}
