// Legacy invariant checks retained for vanilla behavior; migrate these paths before removing this allow.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use std::sync::Arc;

use crate::block::entities::bed::BedBlockEntity;
use pumpkin_data::Block;
use pumpkin_data::BlockStateId;
use pumpkin_data::block_properties::{BedPart, BlockProperties, CampfireLikeProperties};
use pumpkin_data::dimension::Dimension;
use pumpkin_data::entity::{EntityPose, EntityType};
use pumpkin_data::tag::Taggable;
use pumpkin_data::translation;
use pumpkin_macros::pumpkin_block_from_tag;
use pumpkin_util::GameMode;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_world::world::BlockFlags;
use uuid::Uuid;

use crate::block::OnLandedUponArgs;
use crate::block::UpdateEntityMovementAfterFallOnArgs;
use crate::block::bounce_entity_after_fall;
use crate::block::registry::BlockActionResult;
use crate::block::{
    BlockBehaviour, BrokenArgs, CanPlaceAtArgs, NormalUseArgs, OnPlaceArgs, OnStateReplacedArgs,
    PlacedArgs, PlayerPlacedArgs,
};
use crate::entity::{EntityBase, player::Player};
use crate::world::World;

type BedProperties = pumpkin_data::block_properties::WhiteBedLikeProperties;

const NO_SLEEP_IDS: &[u16] = &[
    EntityType::BLAZE.id, EntityType::BOGGED.id, EntityType::SKELETON.id,
    EntityType::STRAY.id, EntityType::WITHER_SKELETON.id, EntityType::BREEZE.id,
    EntityType::CREAKING.id, EntityType::CREEPER.id, EntityType::DROWNED.id,
    EntityType::ENDERMITE.id, EntityType::EVOKER.id, EntityType::GIANT.id,
    EntityType::GUARDIAN.id, EntityType::ELDER_GUARDIAN.id, EntityType::ILLUSIONER.id,
    EntityType::OCELOT.id, EntityType::PIGLIN.id, EntityType::PIGLIN_BRUTE.id,
    EntityType::PILLAGER.id, EntityType::PHANTOM.id, EntityType::RAVAGER.id,
    EntityType::SILVERFISH.id, EntityType::SPIDER.id, EntityType::CAVE_SPIDER.id,
    EntityType::VEX.id, EntityType::VINDICATOR.id, EntityType::WARDEN.id,
    EntityType::WITCH.id, EntityType::WITHER.id, EntityType::ZOGLIN.id,
    EntityType::ZOMBIE.id, EntityType::ZOMBIE_VILLAGER.id, EntityType::HUSK.id,
    EntityType::ENDERMAN.id, EntityType::ZOMBIFIED_PIGLIN.id,
];

#[pumpkin_block_from_tag("minecraft:beds")]
pub struct BedBlock;

impl BlockBehaviour for BedBlock {
    fn can_place_at(&self, args: CanPlaceAtArgs<'_>) -> bool {
        if let Some(player) = args.player {
            let facing = player.get_entity().get_horizontal_facing();
            return args.block_accessor.get_block_state(args.position).replaceable()
                && args.block_accessor
                    .get_block_state(&args.position.offset(facing.to_offset())).replaceable();
        }
        false
    }

    fn on_landed_upon(&self, args: OnLandedUponArgs<'_>) {
        if let Some(living) = args.entity.get_living_entity() {
            living.handle_fall_damage(args.entity, args.fall_distance * 0.5, 1.0);
        }
    }

    fn update_entity_movement_after_fall_on(&self, args: UpdateEntityMovementAfterFallOnArgs<'_>) {
        bounce_entity_after_fall(args.entity, 0.66);
    }

    fn on_place(&self, args: OnPlaceArgs<'_>) -> BlockStateId {
        let mut props = BedProperties::default(args.block);
        props.facing = args.player.get_entity().get_horizontal_facing();
        props.part = BedPart::Foot;
        props.to_state_id(args.block)
    }

    fn placed(&self, args: PlacedArgs<'_>) {
        args.world.add_block_entity(Arc::new(BedBlockEntity::new(*args.position)));
        let mut head = BedProperties::default(args.block);
        head.facing = BedProperties::from_state_id(args.state_id, args.block).facing;
        head.part = BedPart::Head;
        let head_pos = args.position.offset(head.facing.to_offset());
        args.world.set_block_state(
            &head_pos, head.to_state_id(args.block),
            BlockFlags::NOTIFY_ALL | BlockFlags::SKIP_BLOCK_ADDED_CALLBACK,
        );
        args.world.add_block_entity(Arc::new(BedBlockEntity::new(head_pos)));
    }

    fn player_placed(&self, args: PlayerPlacedArgs<'_>) {
        args.world.play_bedrock_level_sound(
            "place", &args.position.to_centered_f64(),
            i32::from(pumpkin_data::BlockState::to_be_network_id(args.state_id)),
        );
    }

    fn broken(&self, args: BrokenArgs<'_>) {
        let props = BedProperties::from_state_id(args.state.id, args.block);
        let other = if props.part == BedPart::Head {
            args.position.offset(props.facing.opposite().to_offset())
        } else { args.position.offset(props.facing.to_offset()) };
        if args.world.get_block_state_id(&other).to_block_id() != args.block.id {
            args.world.update_neighbors(&other, None);
            return;
        }
        let flags = if props.part == BedPart::Foot && args.player.gamemode.load() != GameMode::Creative {
            BlockFlags::NOTIFY_ALL
        } else { BlockFlags::SKIP_DROPS | BlockFlags::NOTIFY_ALL };
        args.world.break_block(&other, Some(args.player), flags);
    }

    fn on_state_replaced(&self, args: OnStateReplacedArgs<'_>) {
        if args.moved || Block::from_state_id(args.world.get_block_state_id(args.position)) == &Block::AIR {
            return;
        }
        let props = BedProperties::from_state_id(args.old_state_id, args.block);
        let other = if props.part == BedPart::Head {
            args.position.offset(props.facing.opposite().to_offset())
        } else { args.position.offset(props.facing.to_offset()) };
        let (other_block, other_state) = args.world.get_block_and_state(&other);
        if other_block == args.block
            && BedProperties::from_state_id(other_state.id, other_block).part != props.part
        {
            args.world.break_block(&other, None, BlockFlags::SKIP_DROPS | BlockFlags::NOTIFY_ALL);
        }
    }

    fn normal_use(&self, args: NormalUseArgs<'_>) -> BlockActionResult {
        Self::use_bed(args.world, args.player, args.block, args.position)
    }
}

impl BedBlock {
    fn use_bed(world: &Arc<World>, player: &Arc<Player>, block: &Block, position: &BlockPos) -> BlockActionResult {
        let state_id = world.get_block_state_id(position);
        let props = BedProperties::from_state_id(state_id, block);
        let (head_pos, foot_pos) = if props.part == BedPart::Head {
            (*position, position.offset(props.facing.opposite().to_offset()))
        } else { (position.offset(props.facing.to_offset()), *position) };

        if world.dimension != Dimension::OVERWORLD {
            world.break_block(&head_pos, None, BlockFlags::SKIP_DROPS);
            world.break_block(&foot_pos, None, BlockFlags::SKIP_DROPS);
            world.explode(head_pos.to_centered_f64(), 5.0, crate::world::ExplosionInteraction::Block);
            return BlockActionResult::SuccessServer;
        }
        if world.get_block_state(&head_pos.up()).is_solid() || world.get_block_state(&foot_pos.up()).is_solid() {
            player.send_system_message_raw(&pumpkin_macros::translate_cross!(
                translation::java::BLOCK_MINECRAFT_BED_OBSTRUCTED,
                translation::bedrock::TILE_BED_OBSTRUCTED), true);
            return BlockActionResult::SuccessServer;
        }
        if props.occupied {
            // `BedBlock.kickVillagerOutOfBed` (`BedBlock.java:...`): wake a sleeping villager first.
            let aabb = pumpkin_util::math::boundingbox::BoundingBox::new(
                head_pos.to_f64(), Vector3::new(
                    f64::from(head_pos.0.x) + 1.0, f64::from(head_pos.0.y) + 1.0,
                    f64::from(head_pos.0.z) + 1.0));
            let sleeping = world.get_entities_at_box(&aabb).into_iter().find_map(|entity| {
                let villager = entity.cast_any().downcast_ref::<crate::entity::passive::villager::VillagerEntity>()?;
                (villager.get_entity().pose.load() == EntityPose::Sleeping).then(|| entity.clone())
            });
            if let Some(entity) = sleeping {
                let villager = entity.cast_any()
                    .downcast_ref::<crate::entity::passive::villager::VillagerEntity>()
                    .expect("entity was identified as a villager");
                tokio::task::block_in_place(|| {
                    tokio::runtime::Handle::current().block_on(villager.stop_sleeping(world));
                });
                return BlockActionResult::SuccessServer;
            }
            player.send_system_message_raw(&pumpkin_macros::translate_cross!(
                translation::java::BLOCK_MINECRAFT_BED_OCCUPIED,
                translation::bedrock::TILE_BED_OCCUPIED), true);
            return BlockActionResult::SuccessServer;
        }
        if !player.position().is_within_bounds(head_pos.to_f64(), 3.0, 2.0, 3.0)
            && !player.position().is_within_bounds(foot_pos.to_f64(), 3.0, 2.0, 3.0)
        {
            player.send_system_message_raw(&pumpkin_macros::translate_cross!(
                translation::java::BLOCK_MINECRAFT_BED_TOO_FAR_AWAY,
                translation::bedrock::TILE_BED_TOOFAR), true);
            return BlockActionResult::SuccessServer;
        }
        if player.set_respawn_point(world.dimension.clone(), head_pos, player.get_entity().yaw.load(), player.get_entity().pitch.load(), false) {
            player.send_system_message(&pumpkin_macros::translate_cross!(
                translation::java::BLOCK_MINECRAFT_SET_SPAWN,
                translation::bedrock::TILE_BED_RESPAWNSET));
        }
        if !can_sleep(world) {
            player.send_system_message_raw(&pumpkin_macros::translate_cross!(
                translation::java::BLOCK_MINECRAFT_BED_NO_SLEEP,
                translation::bedrock::TILE_BED_NOSLEEP), true);
            return BlockActionResult::SuccessServer;
        }

        let universal_anger = world.level_info.load().game_rules.universal_anger;
        for entity in world.entities.load().iter() {
            let prevents = tokio::task::block_in_place(|| tokio::runtime::Handle::current().block_on(
                entity_prevents_sleep(entity.as_ref(), player.gameprofile.id, universal_anger)));
            if !prevents { continue; }
            let pos = entity.get_entity().pos.load();
            if pos.is_within_bounds(head_pos.to_f64(), 8.0, 5.0, 8.0)
                || pos.is_within_bounds(foot_pos.to_f64(), 8.0, 5.0, 8.0)
            {
                player.send_system_message_raw(&pumpkin_macros::translate_cross!(
                    translation::java::BLOCK_MINECRAFT_BED_NOT_SAFE,
                    translation::bedrock::TILE_BED_NOTSAFE), true);
                return BlockActionResult::SuccessServer;
            }
        }
        if let Some(server) = world.server.upgrade() {
            let mut event = crate::plugin::api::events::player::player_bed::PlayerBedEnterEvent::new(player.clone(), head_pos);
            server.plugin_manager.fire_blocking(&server, &mut event);
            if event.cancelled { return BlockActionResult::SuccessServer; }
        }
        player.sleep(head_pos);
        player.trigger_advancement(crate::entity::player::advancement::trigger::AdvancementTrigger::SleptInBed);
        player.increment_stat(pumpkin_data::statistic::StatisticCategory::Custom, pumpkin_data::statistic::CustomStatistic::SleepInBed as i32, 1);
        Self::set_occupied(true, world, block, position, state_id);
        BlockActionResult::SuccessServer
    }

    /// `BedBlock.findStandUpPosition` / `DismountHelper`.
    fn is_dangerous_stand_position(world: &World, position: &BlockPos, entity_type: &EntityType) -> bool {
        let state = world.get_block_state(position);
        let block = Block::from_state_id(state.id);
        let burning = block.has_tag(&pumpkin_data::tag::Block::MINECRAFT_FIRE)
            || block == &Block::LAVA || block == &Block::MAGMA_BLOCK || block == &Block::LAVA_CAULDRON
            || ((block == &Block::CAMPFIRE || block == &Block::SOUL_CAMPFIRE)
                && CampfireLikeProperties::from_state_id(state.id, block).lit);
        (!entity_type.fire_immune && burning) || block == &Block::WITHER_ROSE
            || block == &Block::SWEET_BERRY_BUSH || block == &Block::CACTUS || block == &Block::POWDER_SNOW
    }

    fn find_stand_up_candidate(world: &World, base: &BlockPos, offsets: &[(i32, i32)], check_dangerous: bool, entity_type: &EntityType) -> Option<Vector3<f64>> {
        offsets.iter().find_map(|(dx, dz)| {
            let candidate = BlockPos(Vector3::new(base.0.x + dx, base.0.y, base.0.z + dz));
            if check_dangerous && Self::is_dangerous_stand_position(world, &candidate, entity_type) { return None; }
            let floor_height = world.get_dismount_height(&candidate);
            if !floor_height.is_finite() || floor_height >= 1.0 { return None; }
            if check_dangerous && floor_height <= 0.0 && Self::is_dangerous_stand_position(world, &candidate.down(), entity_type) { return None; }
            let position = Vector3::new(f64::from(candidate.0.x) + 0.5, f64::from(candidate.0.y) + floor_height, f64::from(candidate.0.z) + 0.5);
            let half_width = f64::from(entity_type.dimension[0]) / 2.0;
            let box_ = pumpkin_util::math::boundingbox::BoundingBox::new(
                Vector3::new(position.x - half_width, position.y, position.z - half_width),
                Vector3::new(position.x + half_width, position.y + f64::from(entity_type.dimension[1]), position.z + half_width));
            let invalid = entity_type == &EntityType::PLAYER && (world.get_block(&candidate).has_tag(&pumpkin_data::tag::Block::MINECRAFT_INVALID_SPAWN_INSIDE) || world.get_block(&candidate.up()).has_tag(&pumpkin_data::tag::Block::MINECRAFT_INVALID_SPAWN_INSIDE));
            (!invalid && world.is_space_empty(box_)).then_some(position)
        })
    }

    /// Finds a collision-free, non-dangerous stand-up position using vanilla's offset ordering.
    pub fn find_stand_up_position(world: &World, bed_pos: &BlockPos, forward: pumpkin_data::BlockDirection, yaw: f32, entity_type: &EntityType) -> Option<Vector3<f64>> {
        let right = forward.rotate_clockwise();
        let radians = yaw.to_radians();
        let facing_right = match right {
            pumpkin_data::BlockDirection::North => radians.cos() < 0.0,
            pumpkin_data::BlockDirection::South => radians.cos() > 0.0,
            pumpkin_data::BlockDirection::West => radians.sin() > 0.0,
            pumpkin_data::BlockDirection::East => radians.sin() < 0.0,
            _ => false,
        };
        let side = if facing_right { right.opposite() } else { right };
        let step = |d: pumpkin_data::BlockDirection| { let o = d.to_offset(); (o.x, o.z) };
        let (fx, fz) = step(forward); let (sx, sz) = step(side);
        let surround = [(sx,sz),(sx-fx,sz-fz),(sx-fx*2,sz-fz*2),(-fx*2,-fz*2),(-sx-fx*2,-sz-fz*2),(-sx-fx,-sz-fz),(-sx,-sz),(-sx+fx,-sz+fz),(fx,fz),(sx+fx,sz+fz)];
        let above = [(0,0),(-fx,-fz)];
        let bunk = world.get_block(&bed_pos.down()).has_tag(&pumpkin_data::tag::Block::MINECRAFT_BEDS);
        let mut offsets = surround.to_vec(); if !bunk { offsets.extend(above); }
        if bunk {
            for dangerous in [true, false] {
                if let Some(p) = Self::find_stand_up_candidate(world, bed_pos, &surround, dangerous, entity_type) { return Some(p); }
                if let Some(p) = Self::find_stand_up_candidate(world, &bed_pos.down(), &surround, dangerous, entity_type) { return Some(p); }
                if let Some(p) = Self::find_stand_up_candidate(world, bed_pos, &above, dangerous, entity_type) { return Some(p); }
            }
        } else {
            for dangerous in [true, false] {
                if let Some(p) = Self::find_stand_up_candidate(world, bed_pos, &offsets, dangerous, entity_type) { return Some(p); }
            }
        }
        None
    }

    pub fn set_occupied(occupied: bool, world: &Arc<World>, block: &Block, pos: &BlockPos, state_id: BlockStateId) {
        let mut props = BedProperties::from_state_id(state_id, block);
        props.occupied = occupied;
        world.set_block_state(pos, props.to_state_id(block), BlockFlags::NOTIFY_LISTENERS);
        let other = if props.part == BedPart::Head { pos.offset(props.facing.opposite().to_offset()) } else { pos.offset(props.facing.to_offset()) };
        props.part = if props.part == BedPart::Head { BedPart::Foot } else { BedPart::Head };
        world.set_block_state(&other, props.to_state_id(block), BlockFlags::NOTIFY_LISTENERS);
    }
}

fn can_sleep(world: &Arc<World>) -> bool {
    let time = world.level_time.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let weather = world.weather.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let day_time = time.time_of_day.rem_euclid(24_000);
    if weather.thundering { true } else if weather.raining { day_time > 12010 && day_time < 23991 } else { day_time > 12542 && day_time < 23459 }
}

async fn entity_prevents_sleep(entity: &dyn EntityBase, player_uuid: Uuid, universal_anger: bool) -> bool {
    if entity.get_entity().entity_type.id == EntityType::ZOMBIFIED_PIGLIN.id {
        if let Some(mob) = entity.get_mob() { return mob.is_preventing_player_rest(player_uuid, universal_anger).await; }
        return false;
    }
    NO_SLEEP_IDS.contains(&entity.get_entity().entity_type.id)
}
