use std::sync::{Arc, atomic::Ordering};

use super::redstone::block_receives_redstone_power;
use crate::block::entities::{BlockEntity, command_block::CommandBlockEntity};
use crate::command::CommandSender;
use crate::entity::EntityBase;
use crate::{
    block::{
        BlockBehaviour, BlockFuture, BlockMetadata, CanPlaceAtArgs, NormalUseArgs,
        OnNeighborUpdateArgs, OnPlaceArgs, OnScheduledTickArgs, PlacedArgs, PlayerPlacedArgs,
        registry::BlockActionResult,
    },
    server::Server,
    world::World,
};
use pumpkin_data::{
    Block, BlockId, BlockStateId, FacingExt,
    block_properties::{BlockProperties, CommandBlockLikeProperties, Facing},
};
use pumpkin_util::math::position::BlockPos;
use pumpkin_world::tick::TickPriority;
use tracing::warn;

pub struct CommandBlock;

impl CommandBlock {
    /// `CommandBlock.setPoweredAndUpdate` (`CommandBlock.java:72-85`).
    fn update(
        world: &World,
        block: &Block,
        command_block: &CommandBlockEntity,
        pos: &BlockPos,
        powered: bool,
    ) {
        if command_block.powered.swap(powered, Ordering::Relaxed) == powered || !powered {
            return;
        }
        if command_block.auto.load(Ordering::Relaxed) || block.id == Block::CHAIN_COMMAND_BLOCK.id
        {
            return;
        }
        command_block.mark_condition_met(world);
        world.schedule_block_tick(block, *pos, 1, TickPriority::Normal);
    }

    /// `BaseCommandBlock.performCommand` (`BaseCommandBlock.java:89-130`). Returns false only
    /// when the block already ran this game tick, which is what stops looped chains.
    async fn perform_command(
        server: &Arc<Server>,
        world: &Arc<World>,
        block_entity: Arc<dyn BlockEntity>,
    ) -> bool {
        let Ok(command_entity) = Arc::downcast::<CommandBlockEntity>(block_entity) else {
            warn!("Failed to downcast block entity to CommandBlockEntity");
            return false;
        };

        let game_time = world.level_time.lock().await.world_age;
        if game_time == command_entity.last_execution.load(Ordering::Acquire) {
            return false;
        }

        let command = command_entity.command.lock().await.clone();
        if command.eq_ignore_ascii_case("Searge") {
            *command_entity.last_output.lock().await = "#itzlipofutzli".to_string();
            command_entity.success_count.store(1, Ordering::Release);
            return true;
        }

        command_entity.success_count.store(0, Ordering::Release);
        let command_blocks_work = world.level_info.load().game_rules.command_blocks_work;
        if command_blocks_work && !command.is_empty() {
            command_entity.last_output.lock().await.clear();
            let source = CommandSender::CommandBlock(command_entity.clone(), world.clone())
                .into_source(server)
                .await;

            server
                .command_dispatcher
                .load()
                .handle_command(&source, &command)
                .await;
        }

        let last_execution = if command_entity.update_last_execution.load(Ordering::Acquire) {
            game_time
        } else {
            -1
        };
        command_entity
            .last_execution
            .store(last_execution, Ordering::Release);
        true
    }

    /// `CommandBlock.execute` (`CommandBlock.java:117-125`): an empty command only clears the
    /// success count; the chain in front runs either way.
    async fn execute(
        server: &Arc<Server>,
        world: &Arc<World>,
        block_entity: Arc<dyn BlockEntity>,
        command_set: bool,
        pos: BlockPos,
        facing: Facing,
    ) {
        if command_set {
            Self::perform_command(server, world, block_entity).await;
        } else if let Some(command_entity) =
            block_entity.as_any().downcast_ref::<CommandBlockEntity>()
        {
            command_entity.success_count.store(0, Ordering::Release);
        }

        Self::chain_execute(server, world, pos, facing).await;
    }

    /// `CommandBlock.executeChain` (`CommandBlock.java:185-220`): walks the chain blocks in
    /// front of `start`, each one turning the walk to its own facing, for at most
    /// `maxCommandChainLength` steps.
    async fn chain_execute(
        server: &Arc<Server>,
        world: &Arc<World>,
        start: BlockPos,
        mut direction: Facing,
    ) {
        let mut max_iterations = world
            .level_info
            .load()
            .game_rules
            .max_command_sequence_length;
        let mut pos = start;

        loop {
            // `while (maxIterations-- > 0)`.
            let remaining = max_iterations;
            max_iterations = max_iterations.saturating_sub(1);
            if remaining <= 0 {
                break;
            }

            pos = pos.offset(direction.to_block_direction().to_offset());
            let (block, state_id) = world.get_block_and_state_id(&pos);
            if block.id != Block::CHAIN_COMMAND_BLOCK.id {
                break;
            }
            let Some(block_entity) = world.get_block_entity(&pos) else {
                break;
            };
            let Some(command_entity) = block_entity.as_any().downcast_ref::<CommandBlockEntity>()
            else {
                break;
            };
            let props = CommandBlockLikeProperties::from_state_id(state_id, block);

            if command_entity.powered.load(Ordering::Relaxed)
                || command_entity.auto.load(Ordering::Relaxed)
            {
                if command_entity.mark_condition_met(world) {
                    if !Self::perform_command(server, world, block_entity.clone()).await {
                        break;
                    }
                    world.update_comparators(&pos, block).await;
                } else if props.conditional {
                    command_entity.success_count.store(0, Ordering::Release);
                }
            }

            direction = props.facing;
        }

        if max_iterations <= 0 {
            let limit = world
                .level_info
                .load()
                .game_rules
                .max_command_sequence_length
                .max(0);
            warn!("Command Block chain tried to execute more than {limit} steps!");
        }
    }
}

impl BlockMetadata for CommandBlock {
    fn ids() -> Box<[BlockId]> {
        [
            BlockId::COMMAND_BLOCK,
            BlockId::CHAIN_COMMAND_BLOCK,
            BlockId::REPEATING_COMMAND_BLOCK,
        ]
        .into()
    }
}

impl BlockBehaviour for CommandBlock {
    fn on_place<'a>(&'a self, args: OnPlaceArgs<'a>) -> BlockFuture<'a, BlockStateId> {
        Box::pin(async move {
            let mut props = CommandBlockLikeProperties::default(args.block);
            props.facing = args.player.get_entity().get_facing().opposite();
            props.to_state_id(args.block)
        })
    }

    fn normal_use<'a>(&'a self, args: NormalUseArgs<'a>) -> BlockFuture<'a, BlockActionResult> {
        Box::pin(async move {
            // `CommandBlock.useWithoutItem` requires `Player.canUseGameMasterBlocks`
            // (`CommandBlock.java:131-133`; `Player.java:1863-1865`).
            if !args.player.can_use_game_master_blocks() {
                return BlockActionResult::Pass;
            }
            let Some(block_entity) = args.world.get_block_entity(args.position) else {
                return BlockActionResult::Pass;
            };
            let Some(command_entity) = block_entity.as_any().downcast_ref::<CommandBlockEntity>()
            else {
                return BlockActionResult::Pass;
            };
            // `ServerPlayer.openCommandBlock` sends the custom tag only to the interacting
            // player (`ServerPlayer.java:1408-1411`); a world update would leak it to trackers.
            args.player.open_command_block(command_entity).await;
            BlockActionResult::SuccessServer
        })
    }

    /// `CommandBlock.neighborChanged` (`CommandBlock.java:61-70`).
    fn on_neighbor_update<'a>(&'a self, args: OnNeighborUpdateArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async move {
            if let Some(block_entity) = args.world.get_block_entity(args.position) {
                if block_entity.resource_location() != CommandBlockEntity::ID {
                    return;
                }
                let Some(command_entity) =
                    block_entity.as_any().downcast_ref::<CommandBlockEntity>()
                else {
                    warn!("Block entity at {} is not a command block", args.position);
                    return;
                };

                Self::update(
                    args.world,
                    args.block,
                    command_entity,
                    args.position,
                    block_receives_redstone_power(args.world, args.position).await,
                );
            }
        })
    }

    /// `CommandBlock.tick` (`CommandBlock.java:87-115`): the mode comes from the block
    /// (command block REDSTONE, repeating AUTO, chain SEQUENCE, which does nothing here).
    fn on_scheduled_tick<'a>(&'a self, args: OnScheduledTickArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async move {
            let Some(block_entity) = args.world.get_block_entity(args.position) else {
                return;
            };
            if block_entity.resource_location() != CommandBlockEntity::ID {
                return;
            }

            let Some(command_entity) = block_entity.as_any().downcast_ref::<CommandBlockEntity>()
            else {
                warn!("Block entity at {} is not a command block", args.position);
                return;
            };
            let Some(server) = args.world.server.upgrade() else {
                return;
            };
            let props = CommandBlockLikeProperties::from_state_id(
                args.world.get_block_state_id(args.position),
                args.block,
            );

            let command_set = !command_entity.command.lock().await.is_empty();
            let was_condition_met = command_entity.condition_met.load(Ordering::Acquire);
            let is_auto_mode = args.block.id == Block::REPEATING_COMMAND_BLOCK.id;
            if is_auto_mode || args.block.id == Block::COMMAND_BLOCK.id {
                if is_auto_mode {
                    command_entity.mark_condition_met(args.world);
                }
                if was_condition_met {
                    Self::execute(
                        &server,
                        args.world,
                        block_entity.clone(),
                        command_set,
                        *args.position,
                        props.facing,
                    )
                    .await;
                } else if props.conditional {
                    command_entity.success_count.store(0, Ordering::Release);
                }

                if is_auto_mode
                    && (command_entity.powered.load(Ordering::Relaxed)
                        || command_entity.auto.load(Ordering::Relaxed))
                {
                    args.world.schedule_block_tick(
                        args.block,
                        *args.position,
                        1,
                        TickPriority::Normal,
                    );
                }
            }

            args.world
                .update_comparators(args.position, args.block)
                .await;
        })
    }

    /// Vanilla `GameMasterBlockItem.getPlacementState` (GameMasterBlockItem.java:15-18): only
    /// blocks placement when a player IS present and cannot use game-master blocks -
    /// `player != null && !canUseGameMasterBlocks() ? null : super.getPlacementState(...)`, so a
    /// `None` player (a dispenser or other non-player placement) proceeds normally, matching
    /// `JigsawBlock::can_place_at`. `Player.canUseGameMasterBlocks` (Player.java:1863-1865)
    /// requires instabuild plus permission level 2.
    fn can_place_at(&self, args: CanPlaceAtArgs<'_>) -> bool {
        let Some(player) = args.player else {
            return true;
        };

        player.can_use_game_master_blocks()
    }

    fn placed<'a>(&'a self, args: PlacedArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async move {
            let send_command_feedback = {
                let game_rules = &args.world.level_info.load().game_rules;
                game_rules.send_command_feedback
            };

            let entity = CommandBlockEntity::new(
                *args.position,
                send_command_feedback,
                args.block.id == Block::CHAIN_COMMAND_BLOCK.id,
            );
            args.world.add_block_entity(Arc::new(entity));
        })
    }

    /// `CommandBlock.setPlacedBy` (`CommandBlock.java:149-163`) always samples the neighbour
    /// signal, so a block placed against power starts at once. The track-output and
    /// automatic defaults come from [`Self::placed`].
    fn player_placed<'a>(&'a self, args: PlayerPlacedArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async move {
            let Some(block_entity) = args.world.get_block_entity(args.position) else {
                return;
            };
            let Some(command_entity) = block_entity.as_any().downcast_ref::<CommandBlockEntity>()
            else {
                return;
            };
            Self::update(
                args.world,
                args.block,
                command_entity,
                args.position,
                block_receives_redstone_power(args.world, args.position).await,
            );
        })
    }

    fn get_comparator_output<'a>(
        &'a self,
        args: crate::block::GetComparatorOutputArgs<'a>,
    ) -> BlockFuture<'a, Option<u8>> {
        Box::pin(async {
            let entity = args.world.get_block_entity(args.position);

            entity.map_or_else(
                || {
                    warn!("Command block is missing its corresponding block entity");
                    None
                },
                |entity| {
                    let command_block_entity: Option<&CommandBlockEntity> =
                        entity.as_any().downcast_ref();
                    command_block_entity.map(|e| e.success_count.load(Ordering::Acquire) as u8)
                },
            )
        })
    }
}
