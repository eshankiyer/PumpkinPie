#[allow(clippy::wildcard_imports)]
use super::*;
use pumpkin_nbt::{Nbt, compound::NbtCompound};
use pumpkin_protocol::java::{
    client::play::CTagQueryResponse,
    server::play::{SBlockEntityTagQuery, SEntityTagQuery},
};

impl JavaClient {
    pub async fn handle_block_entity_tag_query(
        &self,
        player: &Player,
        packet: SBlockEntityTagQuery,
    ) {
        if player.permission_lvl.load() < PermissionLvl::Two {
            return;
        }

        // Vanilla always answers; a missing block entity is sent as a null tag,
        // which the network NBT writer encodes as a lone TAG_End byte.
        let Some(block_entity) = player.world().get_block_entity(&packet.location) else {
            self.send_packet(&CTagQueryResponse::new(packet.transaction_id, &[0u8]))
                .await;
            return;
        };

        let mut compound = NbtCompound::new();
        block_entity.write_nbt(&mut compound).await;

        let nbt_bytes = Nbt::new(String::new(), compound).write_unnamed();
        self.send_packet(&CTagQueryResponse::new(packet.transaction_id, &nbt_bytes))
            .await;
    }

    pub async fn handle_entity_tag_query(&self, player: &Player, packet: SEntityTagQuery) {
        if player.permission_lvl.load() < PermissionLvl::Two {
            return;
        }

        // Vanilla sends nothing for an unknown entity.
        let Some(entity) = player.world().get_entity_by_id(packet.entity_id.0) else {
            return;
        };

        let mut compound = NbtCompound::new();
        entity.write_nbt(&mut compound).await;
        // Vanilla uses `saveWithoutId`, so the top-level "id" written by `write_nbt` is dropped.
        compound.child_tags.remove("id");

        let nbt_bytes = Nbt::new(String::new(), compound).write_unnamed();
        self.send_packet(&CTagQueryResponse::new(packet.transaction_id, &nbt_bytes))
            .await;
    }
}
