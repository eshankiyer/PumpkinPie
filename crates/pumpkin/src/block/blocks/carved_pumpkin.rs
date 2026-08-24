use std::sync::Arc;

use pumpkin_data::{
    Block, BlockDirection, BlockId, BlockStateId, HorizontalFacingExt,
    block_properties::{
        BlockProperties, ChestLikeProperties, ChestType, HorizontalFacing, WallTorchLikeProperties,
    },
    entity::EntityType,
    tag::{self, Taggable},
    world::WorldEvent,
};
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_world::world::BlockFlags;

use crate::{
    block::{
        BlockBehaviour, BlockFuture, BlockMetadata, OnPlaceArgs, PlacedArgs,
        blocks::copper_weathering,
    },
    entity::{
        Entity,
        passive::{
            copper_golem::{CopperGolemEntity, CopperWeatherState},
            iron_golem::IronGolemEntity,
            snow_golem::SnowGolemEntity,
        },
    },
    world::World,
};

pub struct CarvedPumpkinBlock;

impl BlockMetadata for CarvedPumpkinBlock {
    fn ids() -> Box<[BlockId]> {
        [BlockId::JACK_O_LANTERN, BlockId::CARVED_PUMPKIN].into()
    }
}

impl CarvedPumpkinBlock {
    /// Vanilla `CarvedPumpkinBlock.canSpawnGolem` (CarvedPumpkinBlock.java:59-63): reports
    /// whether placing a golem head at `top_pos` completes a build. Checks each
    /// `getOrCreate*Base` pattern (snow :148-157, copper :196-202, iron :171-181) anchored
    /// at `top_pos`, along either horizontal axis for the iron base.
    pub fn can_spawn_golem(world: &Arc<World>, top_pos: &BlockPos) -> bool {
        let head = *top_pos;
        let below = head.down();
        // Snow golem base `" ", "#", "#"`: two snow blocks under the open head spot.
        if world.get_block(&below) == &Block::SNOW_BLOCK
            && world.get_block(&below.down()) == &Block::SNOW_BLOCK
        {
            return true;
        }

        // Copper golem base `" ", "#"`: any `minecraft:copper` block directly below.
        if world
            .get_block(&below)
            .has_tag(&tag::Block::MINECRAFT_COPPER)
        {
            return true;
        }

        // Iron golem base `"~ ~", "###", "~#~"`: three iron across `below`, one centred
        // beneath it, and air beside the head and beside the waist (`'~'` is the air
        // predicate per the builder at CarvedPumpkinBlock.java:176).
        if world.get_block(&below) != &Block::IRON_BLOCK
            || world.get_block(&below.down()) != &Block::IRON_BLOCK
        {
            return false;
        }
        let is_iron = |pos: &BlockPos| world.get_block(pos) == &Block::IRON_BLOCK;
        let is_air = |pos: &BlockPos| world.get_block_state(pos).is_air();
        for (left, right) in [
            (BlockDirection::North, BlockDirection::South),
            (BlockDirection::East, BlockDirection::West),
        ] {
            let arm_l = below.offset(left.to_offset());
            let arm_r = below.offset(right.to_offset());
            if is_iron(&arm_l)
                && is_iron(&arm_r)
                && is_air(&head.offset(left.to_offset()))
                && is_air(&head.offset(right.to_offset()))
                && is_air(&arm_l.down())
                && is_air(&arm_r.down())
            {
                return true;
            }
        }
        false
    }

    /// Vanilla `CarvedPumpkinBlock.getWeatherStateFromPattern`
    /// (CarvedPumpkinBlock.java:96-105): the weathering age of the copper block the golem
    /// was built from. Waxed variants resolve through the wax-off mapping
    /// (`HoneycombItem.WAX_OFF_BY_BLOCK`) to their unwaxed counterpart's age.
    fn weather_state_from_copper(block: &Block) -> CopperWeatherState {
        match block.id {
            BlockId::WAXED_EXPOSED_COPPER => CopperWeatherState::Exposed,
            BlockId::WAXED_WEATHERED_COPPER => CopperWeatherState::Weathered,
            BlockId::WAXED_OXIDIZED_COPPER => CopperWeatherState::Oxidized,
            _ => CopperWeatherState::from_id(i32::from(
                copper_weathering::oxidation_level_of(block).unwrap_or(0),
            )),
        }
    }

    /// Vanilla `CopperChestBlock.COPPER_TO_COPPER_CHEST_MAPPING`
    /// (CopperChestBlock.java:40-48) consulted by `getFromCopperBlock`
    /// (CopperChestBlock.java:129-134): only the plain copper family has a chest
    /// counterpart; every other `minecraft:copper` block defaults to the unaffected chest.
    const fn copper_chest_for(block: &Block) -> &'static Block {
        match block.id {
            BlockId::EXPOSED_COPPER => &Block::EXPOSED_COPPER_CHEST,
            BlockId::WEATHERED_COPPER => &Block::WEATHERED_COPPER_CHEST,
            BlockId::OXIDIZED_COPPER => &Block::OXIDIZED_COPPER_CHEST,
            BlockId::WAXED_COPPER_BLOCK => &Block::WAXED_COPPER_CHEST,
            BlockId::WAXED_EXPOSED_COPPER => &Block::WAXED_EXPOSED_COPPER_CHEST,
            BlockId::WAXED_WEATHERED_COPPER => &Block::WAXED_WEATHERED_COPPER_CHEST,
            BlockId::WAXED_OXIDIZED_COPPER => &Block::WAXED_OXIDIZED_COPPER_CHEST,
            _ => &Block::COPPER_CHEST,
        }
    }

    /// Vanilla `HoneycombItem.WAX_OFF_BY_BLOCK` lookup used by
    /// `CopperChestBlock.unwaxBlock` (CopperChestBlock.java:126-128).
    #[must_use]
    const fn unwax_chest(block: &Block) -> BlockId {
        match block.id {
            BlockId::WAXED_COPPER_CHEST => BlockId::COPPER_CHEST,
            BlockId::WAXED_EXPOSED_COPPER_CHEST => BlockId::EXPOSED_COPPER_CHEST,
            BlockId::WAXED_WEATHERED_COPPER_CHEST => BlockId::WEATHERED_COPPER_CHEST,
            BlockId::WAXED_OXIDIZED_COPPER_CHEST => BlockId::OXIDIZED_COPPER_CHEST,
            _ => block.id,
        }
    }

    /// Vanilla `CarvedPumpkinBlock.replaceCopperBlockWithChest`
    /// (CarvedPumpkinBlock.java:216-222) plus `CopperChestBlock.getFromCopperBlock`
    /// (CopperChestBlock.java:129-134): turns the built copper block into its matching
    /// copper chest, facing away from the head spot, with the double-chest type resolved
    /// against a single aligned partner (`ChestBlock.getChestType`,
    /// ChestBlock.java:232-236, via `CopperChestBlock.chestCanConnectTo`,
    /// CopperChestBlock.java:118-120) and both halves converging on the least oxidized
    /// variant (`getLeastOxidizedChestOfConnectedBlocks`, CopperChestBlock.java:78-95).
    async fn replace_copper_block_with_chest(
        world: &Arc<World>,
        pos: &BlockPos,
        copper_block: &Block,
        facing: HorizontalFacing,
    ) {
        let chest_block = Self::copper_chest_for(copper_block);

        let partner_props = |dir: HorizontalFacing| -> Option<ChestLikeProperties> {
            let neighbor_pos = pos.offset(dir.to_block_direction().to_offset());
            let neighbor_block = world.get_block(&neighbor_pos);
            if !neighbor_block.has_tag(&tag::Block::MINECRAFT_COPPER_CHESTS) {
                return None;
            }
            let props = ChestLikeProperties::from_state_id(
                world.get_block_state_id(&neighbor_pos),
                neighbor_block,
            );
            (props.r#type == ChestType::Single && props.facing == facing).then_some(props)
        };

        let connected_dir = |r#type: ChestType| -> HorizontalFacing {
            match r#type {
                ChestType::Left => facing.rotate_clockwise(),
                _ => facing.rotate_counter_clockwise(),
            }
        };

        let r#type = if partner_props(facing.rotate_clockwise()).is_some() {
            ChestType::Left
        } else if partner_props(facing.rotate_counter_clockwise()).is_some() {
            ChestType::Right
        } else {
            ChestType::Single
        };

        // Least-oxidized merge with the single partner when forming a double chest.
        // `getLeastOxidizedChestOfConnectedBlocks` (CopperChestBlock.java:78-95) only
        // unwaxes either side when `isWaxed()` differs between them; this always compares
        // (and, if the neighbor wins, always returns) the unwaxed block, so a merge where
        // both sides are waxed incorrectly loses its wax. Not fixed here - narrow edge
        // case (a golem-built waxed copper chest connecting to an existing waxed one).
        let mut final_block = chest_block;
        let mut partner = None;
        if r#type != ChestType::Single
            && let Some(props) = partner_props(connected_dir(r#type))
        {
            let neighbor_pos = pos.offset(connected_dir(r#type).to_block_direction().to_offset());
            let neighbor_block = world.get_block(&neighbor_pos);
            partner = Some((neighbor_pos, props));
            let own =
                copper_weathering::oxidation_level_of(Self::unwax_chest(chest_block).to_block());
            let other =
                copper_weathering::oxidation_level_of(Self::unwax_chest(neighbor_block).to_block());
            if other.is_some_and(|other| own.is_none_or(|own| other < own)) {
                final_block = Self::unwax_chest(neighbor_block).to_block();
            }
        }

        let mut props = ChestLikeProperties::default(final_block);
        props.facing = facing;
        props.r#type = r#type;
        world
            .set_block_state(
                pos,
                props.to_state_id(final_block),
                BlockFlags::NOTIFY_LISTENERS,
            )
            .await;

        // `CopperChestBlock.updateShape` (CopperChestBlock.java:99-114) makes the single
        // partner adopt this block (keeping its own facing/type); mirror that settled state.
        if r#type != ChestType::Single
            && let Some((neighbor_pos, mut neighbor_props)) = partner
        {
            neighbor_props.r#type = match r#type {
                ChestType::Left => ChestType::Right,
                _ => ChestType::Left,
            };
            world
                .set_block_state(
                    &neighbor_pos,
                    neighbor_props.to_state_id(final_block),
                    BlockFlags::NOTIFY_LISTENERS,
                )
                .await;
        }
    }

    /// Vanilla `spawnGolemInWorld`'s closing `updatePatternBlocks` call
    /// (CarvedPumpkinBlock.java:116, 129-136): after the golem spawns, neighbours of every
    /// cleared cell get a shape/neighbor update.
    async fn update_pattern_blocks(world: &Arc<World>, cleared: &[BlockPos]) {
        for pos in cleared {
            world.update_neighbors(pos, None).await;
        }
    }
}

impl BlockBehaviour for CarvedPumpkinBlock {
    fn on_place<'a>(&'a self, args: OnPlaceArgs<'a>) -> BlockFuture<'a, BlockStateId> {
        Box::pin(async move {
            let mut props = WallTorchLikeProperties::default(args.block);
            props.facing = args
                .player
                .living_entity
                .entity
                .get_horizontal_facing()
                .opposite();
            props.to_state_id(args.block)
        })
    }

    fn placed<'a>(&'a self, args: PlacedArgs<'a>) -> BlockFuture<'a, ()> {
        // Mojang uses some BlockPattern magic, way too complex tbh
        Box::pin(async move {
            let down_pos = args.position.down();
            let upper = args.world.get_block(&down_pos);
            let lower = args.world.get_block(&down_pos.down());
            if upper == &Block::SNOW_BLOCK && lower == &Block::SNOW_BLOCK {
                let cleared = [*args.position, down_pos, down_pos.down()];
                for pos in cleared {
                    args.world
                        .set_block_state(
                            &pos,
                            Block::AIR.default_state.id,
                            BlockFlags::NOTIFY_LISTENERS,
                        )
                        .await;
                    args.world.sync_world_event(
                        WorldEvent::ParticlesDestroyBlock,
                        pos,
                        Block::SNOW_BLOCK.default_state.id.as_u16().into(),
                    );
                }
                let entity = Entity::new(
                    args.world.clone(),
                    down_pos.down().to_centered_f64(),
                    &EntityType::SNOW_GOLEM,
                );
                let golem = SnowGolemEntity::new(entity);
                args.world.spawn_entity(golem).await;
                Self::update_pattern_blocks(args.world, &cleared).await;
                return;
            }

            if upper == &Block::IRON_BLOCK && lower == &Block::IRON_BLOCK {
                for dir in [BlockDirection::North, BlockDirection::West] {
                    let opposite = dir.opposite();
                    let arm1 = down_pos.offset(dir.to_offset());
                    let arm2 = down_pos.offset(opposite.to_offset());

                    if args.world.get_block(&arm1) == &Block::IRON_BLOCK
                        && args.world.get_block(&arm2) == &Block::IRON_BLOCK
                    {
                        let pattern = [*args.position, down_pos, down_pos.down(), arm1, arm2];

                        for p in pattern {
                            args.world
                                .set_block_state(
                                    &p,
                                    Block::AIR.default_state.id,
                                    BlockFlags::NOTIFY_LISTENERS,
                                )
                                .await;
                            args.world.sync_world_event(
                                WorldEvent::ParticlesDestroyBlock,
                                p,
                                Block::IRON_BLOCK.default_state.id.as_u16().into(),
                            );
                        }

                        let entity = Entity::new(
                            args.world.clone(),
                            down_pos.down().to_centered_f64(),
                            &EntityType::IRON_GOLEM,
                        );
                        let golem = IronGolemEntity::new(entity);
                        // `CarvedPumpkinBlock.java:79`: `ironGolem.setPlayerCreated(true)`.
                        golem
                            .player_created
                            .store(true, std::sync::atomic::Ordering::Relaxed);
                        args.world.spawn_entity(golem).await;
                        Self::update_pattern_blocks(args.world, &pattern).await;
                        return;
                    }
                }
            }

            // Copper golem full pattern `"^", "#"` (CarvedPumpkinBlock.java:204-214):
            // the head sits on any `minecraft:copper` block.
            let copper_block = args.world.get_block(&down_pos);
            if copper_block.has_tag(&tag::Block::MINECRAFT_COPPER) {
                Self::spawn_copper_golem(&args, down_pos, copper_block).await;
            }
        })
    }
}

impl CarvedPumpkinBlock {
    async fn spawn_copper_golem(args: &PlacedArgs<'_>, down_pos: BlockPos, copper_block: &Block) {
        let weather_state = Self::weather_state_from_copper(copper_block);
        let cleared = [*args.position, down_pos];
        for pos in cleared {
            let destroyed = if pos == down_pos {
                copper_block.default_state.id
            } else {
                args.block.default_state.id
            };
            args.world
                .set_block_state(
                    &pos,
                    Block::AIR.default_state.id,
                    BlockFlags::NOTIFY_LISTENERS,
                )
                .await;
            args.world.sync_world_event(
                WorldEvent::ParticlesDestroyBlock,
                pos,
                destroyed.as_u16().into(),
            );
        }

        // `trySpawnGolem` (CarvedPumpkinBlock.java:85-92) + `spawnGolemInWorld`
        // (:107-117): spawnPos is `copperGolemMatch.getBlock(0, 0, 0)`, and
        // `getBlock`'s "down" index counts from the pattern's top row - row 0 of
        // `aisle("^", "#")` is the pumpkin cell, not the copper cell below it -
        // so the golem snaps to the pumpkin's own position via
        // `snapTo(x+0.5, y+0.05, z+0.5)` (:109), not the copper block's.
        let spawn_pos = Vector3::new(
            f64::from(args.position.0.x) + 0.5,
            f64::from(args.position.0.y) + 0.05,
            f64::from(args.position.0.z) + 0.5,
        );
        let entity = Entity::new(args.world.clone(), spawn_pos, &EntityType::COPPER_GOLEM);
        let golem = CopperGolemEntity::new(entity);
        // `CopperGolem.spawn(weatherState)` (CopperGolem.java:365-368).
        golem.set_weather_state(weather_state);
        args.world.play_sound(
            pumpkin_data::sound::Sound::EntityCopperGolemSpawn,
            pumpkin_data::sound::SoundCategory::Neutral,
            &spawn_pos,
        );
        args.world.spawn_entity(golem).await;

        let facing = WallTorchLikeProperties::from_state_id(args.state_id, args.block).facing;
        Self::replace_copper_block_with_chest(args.world, &down_pos, copper_block, facing).await;
        Self::update_pattern_blocks(args.world, &cleared).await;
    }
}
