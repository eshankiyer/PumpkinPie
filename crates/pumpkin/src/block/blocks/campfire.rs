use pumpkin_data::{
    Block, BlockDirection, BlockStateId, Enchantment,
    block_properties::{BlockProperties, CampfireLikeProperties},
    damage::DamageType,
    data_component_impl::EquipmentSlot,
    effect::StatusEffect,
    fluid::Fluid,
    game_event::GameEvent,
    sound::{Sound, SoundCategory},
};
use pumpkin_macros::pumpkin_block_from_tag;
use pumpkin_world::tick::TickPriority;
use pumpkin_world::world::BlockFlags;

use crate::block::entities::campfire::CampfireBlockEntity;
use crate::block::registry::BlockActionResult;
use crate::{
    block::{
        BlockBehaviour, BlockFuture, BlockIsReplacing, GetStateForNeighborUpdateArgs,
        NormalUseArgs, OnEntityCollisionArgs, OnPlaceArgs, OnProjectileHitArgs, PlacedArgs,
        UseWithItemArgs,
    },
    entity::EntityBase,
    world::game_event::{GameEventContext, emit_game_event},
};
use std::sync::Arc;
use std::sync::atomic::Ordering;

#[pumpkin_block_from_tag("minecraft:campfires")]
pub struct CampfireBlock;

// Campfire contents use the shared pre-removal block-entity drop path, matching
// `CampfireBlockEntity.preRemoveSideEffects` (`CampfireBlockEntity.java:199-204`); the post-break
// `broken` hook must not drop the same inventory again.
impl CampfireBlock {
    /// `CampfireBlock.placeLiquid` (`CampfireBlock.java:203-220`): water extinguishes a dry
    /// campfire, emits the block-change event, and schedules the water fluid tick.
    pub(crate) async fn place_liquid(
        world: &Arc<crate::world::World>,
        position: &pumpkin_util::math::position::BlockPos,
        block: &Block,
        state_id: BlockStateId,
        fluid: &Fluid,
    ) -> bool {
        if !fluid.matches_type(&Fluid::WATER) {
            return false;
        }

        let mut properties = CampfireLikeProperties::from_state_id(state_id, block);
        if properties.waterlogged {
            return false;
        }

        if properties.lit {
            world.play_block_sound(
                Sound::EntityGenericExtinguishFire,
                SoundCategory::Blocks,
                *position,
            );
            emit_game_event(
                world,
                GameEvent::BlockChange,
                position.to_centered_f64(),
                GameEventContext::none(),
            )
            .await;
        }

        properties.waterlogged = true;
        properties.lit = false;
        world
            .set_block_state(
                position,
                properties.to_state_id(block),
                BlockFlags::NOTIFY_ALL,
            )
            .await;
        world.schedule_fluid_tick(
            &Fluid::WATER,
            *position,
            Fluid::WATER.flow_speed as u8,
            TickPriority::Normal,
        );
        true
    }
}

impl BlockBehaviour for CampfireBlock {
    fn normal_use<'a>(&'a self, args: NormalUseArgs<'a>) -> BlockFuture<'a, BlockActionResult> {
        Box::pin(async move {
            let Some(block_entity) = args.world.get_block_entity(args.position) else {
                return BlockActionResult::Pass;
            };
            let Some(inventory) = block_entity.clone().get_inventory() else {
                return BlockActionResult::Pass;
            };
            for slot in 0..inventory.size() {
                let item = inventory.remove_stack(slot).await;
                if !item.is_empty() {
                    // `CampfireBlockEntity.getUpdateTag` (`CampfireBlockEntity.java:159-177`)
                    // is sent after the slot is removed so viewers see the empty slot now.
                    args.world.update_block_entity(&block_entity);
                    args.player
                        .inventory
                        .offer_or_drop_stack(item, args.player.as_ref())
                        .await;
                    args.player
                        .increment_stat(
                            pumpkin_data::statistic::StatisticCategory::Custom,
                            pumpkin_data::statistic::CustomStatistic::InteractWithCampfire as i32,
                            1,
                        )
                        .await;
                    return BlockActionResult::Success;
                }
            }
            BlockActionResult::Pass
        })
    }

    fn use_with_item<'a>(
        &'a self,
        args: UseWithItemArgs<'a>,
    ) -> BlockFuture<'a, BlockActionResult> {
        Box::pin(async move {
            let Some(block_entity) = args.world.get_block_entity(args.position) else {
                return BlockActionResult::Pass;
            };
            let Some(campfire) = block_entity.as_any().downcast_ref::<CampfireBlockEntity>() else {
                return BlockActionResult::Pass;
            };
            let item = &mut *args.item_stack;
            let is_campfire_input = pumpkin_data::recipes::get_cooking_recipe_with_ingredient(
                item.item,
                pumpkin_data::recipes::CookingRecipeKind::CampfireCooking,
            )
            .is_some();
            if !is_campfire_input {
                return BlockActionResult::Pass;
            }
            if campfire.add_item(item, args.player.is_creative()).await {
                // Vanilla `CampfireBlockEntity.placeFood` (`CampfireBlockEntity.java:179-198`)
                // emits BLOCK_CHANGE after accepting a recipe item; this is observable by
                // game-event listeners (including sculk sensors), not just by the client.
                // `placeFood` also calls `markUpdated`, whose payload is `getUpdateTag`
                // (`CampfireBlockEntity.java:159-177`).
                args.world.update_block_entity(&block_entity);
                emit_game_event(
                    args.world,
                    GameEvent::BlockChange,
                    args.position.to_centered_f64(),
                    GameEventContext::of_entity(args.player.clone()),
                )
                .await;
                args.player
                    .increment_stat(
                        pumpkin_data::statistic::StatisticCategory::Custom,
                        pumpkin_data::statistic::CustomStatistic::InteractWithCampfire as i32,
                        1,
                    )
                    .await;
                BlockActionResult::Success
            } else {
                // `CampfireBlock.useItemOn` consumes a valid campfire input even when
                // `placeFood` cannot find an empty slot (`CampfireBlock.java:100-112`).
                BlockActionResult::Consume
            }
        })
    }

    fn placed<'a>(&'a self, args: PlacedArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async move {
            let entity = CampfireBlockEntity::new(*args.position);
            args.world.add_block_entity(Arc::new(entity));
        })
    }

    fn on_entity_collision<'a>(&'a self, args: OnEntityCollisionArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async move {
            if CampfireLikeProperties::from_state_id(args.state.id, args.block).lit
                && let Some(living_entity) = args.entity.get_living_entity()
            {
                let has_frost_walker_enchantment = {
                    let equipment = living_entity.entity_equipment.lock().await;
                    equipment
                        .equipment
                        .get(&EquipmentSlot::FEET)
                        .is_some_and(|boots| {
                            boots.get_enchantment_level(&Enchantment::FROST_WALKER) != 0
                        })
                };
                let has_fire_res = living_entity
                    .get_effect(&StatusEffect::FIRE_RESISTANCE)
                    .await
                    .is_some();
                if has_frost_walker_enchantment || has_fire_res {
                    //campfire burning doesn't work if entity's boots has frost walker enchantment or entity has fire resistance. source: https://minecraft.wiki/w/Campfire#Damage
                    return;
                }
                let damage_amount = if args.block == &Block::SOUL_CAMPFIRE {
                    2.0
                } else {
                    1.0
                };
                args.entity
                    .damage(args.entity, damage_amount, DamageType::CAMPFIRE)
                    .await;
            }
        })
    }

    fn on_place<'a>(&'a self, args: OnPlaceArgs<'a>) -> BlockFuture<'a, BlockStateId> {
        Box::pin(async move {
            let is_replacing_water = matches!(args.replacing, BlockIsReplacing::Water(_));
            let mut props =
                CampfireLikeProperties::from_state_id(args.block.default_state.id, args.block);
            props.waterlogged = is_replacing_water;
            props.signal_fire =
                is_signal_fire_base_block(args.world.get_block(&args.position.down()));
            props.lit = !is_replacing_water;
            props.facing = args.player.get_entity().get_horizontal_facing();
            props.to_state_id(args.block)
        })
    }

    fn get_state_for_neighbor_update<'a>(
        &'a self,
        args: GetStateForNeighborUpdateArgs<'a>,
    ) -> BlockFuture<'a, BlockStateId> {
        Box::pin(async move {
            let mut props = CampfireLikeProperties::from_state_id(args.state_id, args.block);
            if props.waterlogged {
                props.lit = false;
                args.world.schedule_fluid_tick(
                    &Fluid::WATER,
                    *args.position,
                    Fluid::WATER.flow_speed as u8,
                    TickPriority::Normal,
                );
            }

            if args.direction == BlockDirection::Down {
                props.signal_fire =
                    is_signal_fire_base_block(args.world.get_block(args.neighbor_position));
            }

            props.to_state_id(args.block)
        })
    }

    fn on_projectile_hit<'a>(&'a self, args: OnProjectileHitArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async move {
            let props = CampfireLikeProperties::from_state_id(args.state.id, args.block);
            if crate::entity::projectile::projectile_may_interact(
                args.projectile,
                args.server,
                args.world,
                args.position,
            )
            .await
                && args
                    .projectile
                    .get_entity()
                    .fire_ticks
                    .load(Ordering::Relaxed)
                    > 0
                && !props.lit
                && !props.waterlogged
            {
                let mut lit_props = props;
                lit_props.lit = true;
                args.world
                    .set_block_state(
                        args.position,
                        lit_props.to_state_id(args.block),
                        BlockFlags::NOTIFY_LISTENERS,
                    )
                    .await;
            }
        })
    }
}

fn is_signal_fire_base_block(block: &Block) -> bool {
    block == &Block::HAY_BLOCK
}
