use std::sync::Arc;

use pumpkin_data::entity::EntityType;
use pumpkin_data::tag::{self, Taggable};
use pumpkin_data::{BlockId, BlockStateId};
use pumpkin_util::GameMode;

use crate::block::{BlockBehaviour, BlockFuture, BlockMetadata, BrokenArgs, OnPlaceArgs};
use crate::entity::Entity;
use pumpkin_data::block_properties::{BlockProperties, PaleOakWoodLikeProperties};

pub struct InfestedBlock;

impl BlockMetadata for InfestedBlock {
    /// Every infested block, per their registrations in `Blocks.java`: `InfestedBlock` for the
    /// stone and stone-brick family (:2275-2290) and `InfestedRotatedPillarBlock` for deepslate
    /// (:5539-5543). No `minecraft:` tag spans them, and `c:cobblestones/infested` - what this
    /// used to register - names only the cobblestone, so the other six released no silverfish.
    fn ids() -> Box<[BlockId]> {
        [
            BlockId::INFESTED_STONE,
            BlockId::INFESTED_COBBLESTONE,
            BlockId::INFESTED_STONE_BRICKS,
            BlockId::INFESTED_MOSSY_STONE_BRICKS,
            BlockId::INFESTED_CRACKED_STONE_BRICKS,
            BlockId::INFESTED_CHISELED_STONE_BRICKS,
            BlockId::INFESTED_DEEPSLATE,
        ]
        .into()
    }
}

impl BlockBehaviour for InfestedBlock {
    /// `InfestedRotatedPillarBlock.getStateForPlacement`
    /// (`InfestedRotatedPillarBlock.java:39-40`) stores the clicked face axis on
    /// infested deepslate. The normal infested blocks have no axis property and
    /// retain the generated default state.
    fn on_place<'a>(&'a self, args: OnPlaceArgs<'a>) -> BlockFuture<'a, BlockStateId> {
        Box::pin(async move {
            if args.block.id == BlockId::INFESTED_DEEPSLATE {
                let mut props = PaleOakWoodLikeProperties::default(args.block);
                props.axis = args.direction.to_axis();
                props.to_state_id(args.block)
            } else {
                args.block.default_state.id
            }
        })
    }

    fn broken<'a>(&'a self, args: BrokenArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async move {
            if args.player.gamemode.load() == GameMode::Creative {
                return;
            }
            if !args.world.level_info.load().game_rules.block_drops {
                return;
            }

            let tool = args.player.inventory.held_item().await;
            let prevents_spawn = {
                tool.get_data_component::<pumpkin_data::data_component_impl::EnchantmentsImpl>()
                    .is_some_and(|enchantments| {
                        enchantments.enchantment.iter().any(|(enchantment, _)| {
                            enchantment
                                .has_tag(&tag::Enchantment::MINECRAFT_PREVENTS_INFESTED_SPAWNS)
                        })
                    })
            };
            if prevents_spawn {
                return;
            }

            // `InfestedBlock.spawnInfestation`: x + 0.5, y, z + 0.5 - centred on the block,
            // not its corner.
            let mut spawn_pos = args.position.0.to_f64();
            spawn_pos.x += 0.5;
            spawn_pos.z += 0.5;
            let entity = Entity::new(args.world.clone(), spawn_pos, &EntityType::SILVERFISH);

            args.world.spawn_entity(Arc::new(entity)).await;
        })
    }
}
