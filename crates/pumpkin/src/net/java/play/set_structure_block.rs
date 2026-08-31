#[allow(clippy::wildcard_imports)]
use super::*;
use crate::block::entities::structure_block::StructureBlockBlockEntity;
use pumpkin_data::block_properties::{
    BlockProperties, StructureBlockLikeProperties, StructureblockMode,
};
use pumpkin_protocol::java::server::play::SSetStructureBlock;

impl JavaClient {
    #[expect(clippy::too_many_lines)]
    pub async fn handle_set_structure_block(
        &self,
        player: &Arc<Player>,
        packet: &SSetStructureBlock<'_>,
    ) {
        // `ServerGamePacketListenerImpl.handleSetStructureBlock` gates the packet with
        // `Player.canUseGameMasterBlocks` and updates the entity fields
        // (`ServerGamePacketListenerImpl.java:825-845`).
        if !player.can_use_game_master_blocks() {
            return;
        }
        player.update_last_action_time();

        let Some(block_entity) = player.world().get_block_entity(&packet.location) else {
            return;
        };
        let Some(structure_block) = block_entity
            .as_any()
            .downcast_ref::<StructureBlockBlockEntity>()
        else {
            return;
        };

        // The wire values are the enum ordinals and clamped packet fields from
        // `ServerboundSetStructureBlockPacket` (`ServerboundSetStructureBlockPacket.java:73-91`).
        let mode = match packet.mode.0 {
            SSetStructureBlock::MODE_SAVE => "SAVE",
            SSetStructureBlock::MODE_LOAD => "LOAD",
            SSetStructureBlock::MODE_CORNER => "CORNER",
            SSetStructureBlock::MODE_DATA => "DATA",
            _ => return,
        };
        let state_mode = match packet.mode.0 {
            SSetStructureBlock::MODE_SAVE => StructureblockMode::Save,
            SSetStructureBlock::MODE_LOAD => StructureblockMode::Load,
            SSetStructureBlock::MODE_CORNER => StructureblockMode::Corner,
            SSetStructureBlock::MODE_DATA => StructureblockMode::Data,
            _ => return,
        };
        let mirror = match packet.mirror.0 {
            0 => "NONE",
            1 => "LEFT_RIGHT",
            2 => "FRONT_BACK",
            _ => return,
        };
        let rotation = match packet.rotation.0 {
            0 => "NONE",
            1 => "CLOCKWISE_90",
            2 => "CLOCKWISE_180",
            3 => "COUNTERCLOCKWISE_90",
            _ => return,
        };

        *structure_block.mode.lock().await = mode.to_string();
        structure_block.set_structure_name(packet.name).await;
        structure_block
            .set_structure_pos(BlockPos::new(
                i32::from(packet.offset_x).clamp(-48, 48),
                i32::from(packet.offset_y).clamp(-48, 48),
                i32::from(packet.offset_z).clamp(-48, 48),
            ))
            .await;
        structure_block
            .set_structure_size(Vector3::new(
                i32::from(packet.size_x).min(48),
                i32::from(packet.size_y).min(48),
                i32::from(packet.size_z).min(48),
            ))
            .await;
        *structure_block.mirror.lock().await = mirror.to_string();
        *structure_block.rotation.lock().await = rotation.to_string();
        structure_block.set_meta_data(packet.metadata).await;
        structure_block
            .set_integrity(packet.integrity.clamp(0.0, 1.0))
            .await;
        structure_block.set_strict(packet.strict()).await;
        structure_block.set_show_air(packet.show_air()).await;
        structure_block
            .set_show_bounding_box(packet.show_bounding_box())
            .await;

        // `StructureBlockEntity.setMode` updates the block-state MODE property
        // (`StructureBlockEntity.java:224-229`), while the packet applies all entity fields first
        // (`ServerGamePacketListenerImpl.java:830-844`).
        let world = player.world();
        let block = world.get_block(&packet.location);
        if block.id == Block::STRUCTURE_BLOCK.id {
            let state_id = world.get_block_state_id(&packet.location);
            let mut properties = StructureBlockLikeProperties::from_state_id(state_id, block);
            properties.r#mode = state_mode;
            let new_state_id = properties.to_state_id(block);
            if new_state_id != state_id {
                world
                    .set_block_state(
                        &packet.location,
                        new_state_id,
                        BlockFlags::SKIP_BLOCK_ADDED_CALLBACK,
                    )
                    .await;
            }
        }

        if structure_block.has_structure_name().await {
            let structure_name = structure_block.get_structure_name().await;
            match packet.action.0 {
                SSetStructureBlock::ACTION_SAVE_AREA => {
                    structure_block.save_structure(&world, true).await;
                }
                SSetStructureBlock::ACTION_LOAD_AREA => {
                    if structure_block.is_structure_loadable().await {
                        structure_block.place_structure_if_same_size(&world).await;
                    }
                }
                SSetStructureBlock::ACTION_SCAN_AREA => {
                    structure_block.detect_size(&world).await;
                }
                _ => {}
            }
            debug!(
                "Player {} set structure block at {:?}, name: {}, mode: {}",
                player.gameprofile.name, packet.location, structure_name, packet.mode.0
            );
        }

        // Vanilla marks the block entity dirty and sends its update after all packet fields and
        // the selected action are applied (`ServerGamePacketListenerImpl.java:872-873`).
        player.world().update_block_entity(&block_entity);
    }
}
