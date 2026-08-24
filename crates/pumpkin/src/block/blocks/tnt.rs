use pumpkin_data::item::Item;
use std::sync::Arc;

use crate::block::registry::BlockActionResult;
use crate::block::{
    BlockBehaviour, BlockFuture, BrokenArgs, ExplodeArgs, OnNeighborUpdateArgs, PlacedArgs,
    UseWithItemArgs,
};
use crate::entity::Entity;
use crate::entity::tnt::TNTEntity;
use crate::world::World;
use crate::world::game_event::{GameEventContext, emit_game_event};
use pumpkin_data::BlockStateId;
use pumpkin_data::block_properties::{BlockProperties, TntLikeProperties};
use pumpkin_data::entity::EntityType;
use pumpkin_data::game_event::GameEvent;
use pumpkin_data::sound::SoundCategory;
use pumpkin_macros::pumpkin_block;
use pumpkin_util::GameMode;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_world::world::BlockFlags;
use rand::RngExt;

use super::redstone::block_receives_redstone_power;

#[pumpkin_block("minecraft:tnt")]
pub struct TNTBlock;

impl TNTBlock {
    pub async fn prime(world: &Arc<World>, location: &BlockPos) {
        let mut event = crate::plugin::api::events::block::tnt_prime::TNTPrimeEvent::new(
            *location,
            "REDSTONE".to_string(),
        );
        if let Some(server) = world.server.upgrade() {
            server.plugin_manager.fire(&server, &mut event).await;
        }
        if event.cancelled {
            return;
        }

        let entity = Entity::new(world.clone(), location.to_f64(), &EntityType::TNT);
        let mut prime_event =
            crate::plugin::api::events::entity::explosion_prime::ExplosionPrimeEvent::new(
                entity.entity_id,
                DEFAULT_POWER,
                false,
            );
        if let Some(server) = world.server.upgrade() {
            server.plugin_manager.fire(&server, &mut prime_event).await;
        }
        if prime_event.cancelled {
            return;
        }

        let pos = entity.pos.load();
        let tnt = Arc::new(TNTEntity::new(entity, DEFAULT_POWER, DEFAULT_FUSE));
        world.spawn_entity(tnt).await;
        world.play_sound(
            pumpkin_data::sound::Sound::EntityTntPrimed,
            SoundCategory::Blocks,
            &pos,
        );
        // TntBlock.java:92 (`prime`): fires PRIME_FUSE with no source entity for every
        // priming path pumpkin routes through this function (flint & steel/fire charge,
        // initial redstone power, post-place redstone power, and fire spreading onto the
        // block in fire.rs).
        emit_game_event(world, GameEvent::PrimeFuse, pos, GameEventContext::none()).await;
        world
            .set_block_state(location, BlockStateId::AIR, BlockFlags::NOTIFY_ALL)
            .await;
    }
}

const DEFAULT_FUSE: u32 = 80;
const DEFAULT_POWER: f32 = 4.0;

impl BlockBehaviour for TNTBlock {
    fn use_with_item<'a>(
        &'a self,
        args: UseWithItemArgs<'a>,
    ) -> BlockFuture<'a, BlockActionResult> {
        Box::pin(async move {
            let item = args.item_stack.item;
            if item != &Item::FLINT_AND_STEEL && item != &Item::FIRE_CHARGE {
                return BlockActionResult::Pass;
            }
            let world = args.player.world();
            Self::prime(&world, args.position).await;

            BlockActionResult::Consume
        })
    }

    fn placed<'a>(&'a self, args: PlacedArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async move {
            if block_receives_redstone_power(args.world, args.position).await {
                Self::prime(args.world, args.position).await;
            }
        })
    }

    fn on_neighbor_update<'a>(&'a self, args: OnNeighborUpdateArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async move {
            if block_receives_redstone_power(args.world, args.position).await {
                Self::prime(args.world, args.position).await;
            }
        })
    }

    fn broken<'a>(&'a self, args: BrokenArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async move {
            // TntBlock.java:66-72 (`playerWillDestroy`): breaking an `unstable=true` TNT
            // block by hand (not in creative/instabuild) primes it.
            let props = TntLikeProperties::from_state_id(args.state.id, args.block);
            if props.r#unstable && args.player.gamemode.load() != GameMode::Creative {
                Self::prime(args.world, args.position).await;
            }
        })
    }

    fn explode<'a>(&'a self, args: ExplodeArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async move {
            // TntBlock.java:75-83 (`wasExploded`): only spawns the TNT entity when the
            // `tntExplodes` game rule is enabled.
            if !args.world.level_info.load().game_rules.tnt_explodes {
                return;
            }
            let entity = Entity::new(args.world.clone(), args.position.to_f64(), &EntityType::TNT);
            let angle = rand::random::<f64>() * std::f64::consts::TAU;
            entity.set_velocity(Vector3::new(-angle.sin() * 0.02, 0.2, -angle.cos() * 0.02));
            let fuse = rand::rng().random_range(0..DEFAULT_FUSE / 4) + DEFAULT_FUSE / 8;
            let tnt = Arc::new(TNTEntity::new(entity, DEFAULT_POWER, fuse));
            args.world.spawn_entity(tnt).await;
        })
    }

    fn should_drop_items_on_explosion(&self) -> bool {
        false
    }
}
