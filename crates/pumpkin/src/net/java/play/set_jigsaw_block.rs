#[allow(clippy::wildcard_imports)]
use super::*;

impl JavaClient {
    pub async fn handle_set_jigsaw_block(&self, player: &Arc<Player>, jigsaw: SSetJigsawBlock<'_>) {
        // `ServerGamePacketListenerImpl.handleSetJigsawBlock` gates the packet with
        // `Player.canUseGameMasterBlocks` (`ServerGamePacketListenerImpl.java:958-962`).
        if !player.can_use_game_master_blocks() {
            return;
        }
        let pos = jigsaw.pos;
        let block_entity = player.world().get_block_entity(&pos);
        if let Some(block_entity) = block_entity {
            if block_entity.resource_location() != JigsawBlockEntity::ID {
                warn!("Client tried to change Jigsaw block but not Jigsaw block entity found");
                return;
            }

            let Some(jigsaw_block) = block_entity.as_any().downcast_ref::<JigsawBlockEntity>()
            else {
                return;
            };

            // `JigsawBlockEntity` applies these packet fields through its setters
            // (`ServerGamePacketListenerImpl.java:940-952`; `JigsawBlockEntity.java:78-104`).
            jigsaw_block.set_name(jigsaw.name.to_string()).await;
            *jigsaw_block.target.lock().await = jigsaw.target.to_string();
            jigsaw_block.set_pool(jigsaw.pool.to_string()).await;
            jigsaw_block
                .set_final_state(jigsaw.final_state.to_string())
                .await;
            jigsaw_block
                .set_joint(JigsawJointType::from_str(jigsaw.joint))
                .await;
            jigsaw_block.set_selection_priority(jigsaw.selection_priority.0);
            jigsaw_block.set_placement_priority(jigsaw.placement_priority.0);
            jigsaw_block.dirty.store(true, Ordering::Relaxed);

            player.world().update_block_entity(&block_entity);
        }
    }
}
