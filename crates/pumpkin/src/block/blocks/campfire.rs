use pumpkin_data::{
    Block, BlockDirection, BlockStateId, Enchantment,
    block_properties::{BlockProperties, CampfireLikeProperties},
    damage::DamageType,
    data_component_impl::EquipmentSlot,
    effect::StatusEffect,
    fluid::Fluid,
    game_event::GameEvent,
    item_stack::ItemStack,
    recipes::CookingRecipeKind,
    sound::{Sound, SoundCategory},
};
use pumpkin_macros::pumpkin_block_from_tag;
use pumpkin_world::tick::TickPriority;
use pumpkin_world::world::BlockFlags;

use crate::block::entities::campfire::CampfireBlockEntity;
use crate::block::registry::BlockActionResult;
use crate::{
    block::{
        BlockBehaviour, BlockIsReplacing, BrokenArgs, GetStateForNeighborUpdateArgs, NormalUseArgs,
        OnEntityCollisionArgs, OnPlaceArgs, OnProjectileHitArgs, PlacedArgs, UseWithItemArgs,
    },
    entity::EntityBase,
    world::game_event::{GameEventContext, emit_game_event},
};
use std::sync::Arc;
use std::sync::atomic::Ordering;

#[pumpkin_block_from_tag("minecraft:campfires")]
pub struct CampfireBlock;

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

    fn add_item(campfire: &CampfireBlockEntity, stack: &mut ItemStack, creative: bool) -> bool {
        let Some(recipe) = pumpkin_data::recipes::get_cooking_recipe_with_ingredient(
            stack.item,
            CookingRecipeKind::CampfireCooking,
        ) else {
            return false;
        };
        for (slot, item_mutex) in campfire.items.iter().enumerate() {
            let Ok(mut slot_stack) = item_mutex.lock() else {
                continue;
            };
            if slot_stack.is_empty() {
                *slot_stack = stack.copy_with_count(1);
                if !creative {
                    stack.decrement(1);
                }
                if let Some(cooking_time) = campfire.cooking_times.get(slot)
                    && let Ok(mut cooking_time) = cooking_time.lock()
                {
                    *cooking_time = 0;
                }
                if let Some(total_time) = campfire.cooking_total_times.get(slot)
                    && let Ok(mut total_time) = total_time.lock()
                {
                    *total_time = recipe.cookingtime;
                }
                return true;
            }
        }
        false
    }
}

impl BlockBehaviour for CampfireBlock {
    fn normal_use(&self, args: NormalUseArgs<'_>) -> BlockActionResult {
        let Some(block_entity) = args.world.get_block_entity(args.position) else {
            return BlockActionResult::Pass;
        };
        let Some(inventory) = block_entity.get_inventory() else {
            return BlockActionResult::Pass;
        };
        for slot in 0..inventory.size() {
            let item = inventory.remove_stack(slot);
            if !item.is_empty() {
                tokio::task::block_in_place(|| {
                    tokio::runtime::Handle::current().block_on(
                        args.player.inventory.offer_or_drop_stack(item, args.player.as_ref()),
                    );
                });
                args.player.increment_stat(
                    pumpkin_data::statistic::StatisticCategory::Custom,
                    pumpkin_data::statistic::CustomStatistic::InteractWithCampfire as i32,
                    1,
                );
                return BlockActionResult::Success;
            }
        }
        BlockActionResult::Pass
    }

    fn use_with_item(&self, args: UseWithItemArgs<'_>) -> BlockActionResult {
        let Some(block_entity) = args.world.get_block_entity(args.position) else {
            return BlockActionResult::Pass;
        };
        let Some(campfire) = block_entity.as_any().downcast_ref::<CampfireBlockEntity>() else {
            return BlockActionResult::Pass;
        };
        if !Self::add_item(campfire, args.item_stack, args.player.is_creative()) {
            return BlockActionResult::Pass;
        }
        // Vanilla `CampfireBlockEntity.placeFood` (`CampfireBlockEntity.java:179-198`)
        // emits BLOCK_CHANGE after accepting a recipe item; this is observable by
        // game-event listeners (including sculk sensors), not just by the client.
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(emit_game_event(
                args.world,
                GameEvent::BlockChange,
                args.position.to_centered_f64(),
                GameEventContext::of_entity(args.player.clone()),
            ));
        });
        args.player.increment_stat(
            pumpkin_data::statistic::StatisticCategory::Custom,
            pumpkin_data::statistic::CustomStatistic::InteractWithCampfire as i32,
            1,
        );
        BlockActionResult::Success
    }

    fn placed(&self, args: PlacedArgs<'_>) {
        args.world
            .add_block_entity(Arc::new(CampfireBlockEntity::new(*args.position)));
    }

    fn broken(&self, args: BrokenArgs<'_>) {
        // `CampfireBlockEntity.preRemoveSideEffects` (`CampfireBlockEntity.java:199-204`)
        // drops every item still sitting on the campfire via `Containers.dropContents`
        // before the block entity goes away.
        if let Some(block_entity) = args.world.get_block_entity(args.position)
            && let Some(campfire) = block_entity.as_any().downcast_ref::<CampfireBlockEntity>()
        {
            for item_mutex in &campfire.items {
                let item = item_mutex
                    .lock()
                    .map(|stack| stack.clone())
                    .unwrap_or_else(|_| ItemStack::EMPTY.clone());
                if !item.is_empty() {
                    args.world.drop_stack(args.position, item);
                }
            }
        }
    }

    fn on_entity_collision(&self, args: OnEntityCollisionArgs<'_>) {
        if CampfireLikeProperties::from_state_id(args.state.id, args.block).lit
            && let Some(living_entity) = args.entity.get_living_entity()
        {
            let has_frost_walker_enchantment = {
                let equipment = living_entity
                    .entity_equipment
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                equipment
                    .equipment
                    .get(&EquipmentSlot::FEET)
                    .is_some_and(|boots| {
                        boots.get_enchantment_level(&Enchantment::FROST_WALKER) != 0
                    })
            };
            let has_fire_res = living_entity
                .get_effect(&StatusEffect::FIRE_RESISTANCE)
                .is_some();
            if has_frost_walker_enchantment || has_fire_res {
                //campfire burning doesn't work if entity's boots has frost walker enchantment or entity has fire resistance. source: https://minecraft.wiki/w/Campfire#Damage
                return;
            }
            let damage_amount = if args.block == &Block::SOUL_CAMPFIRE { 2.0 } else { 1.0 };
            args.entity
                .damage(args.entity, damage_amount, DamageType::CAMPFIRE);
        }
    }

    fn on_place(&self, args: OnPlaceArgs<'_>) -> BlockStateId {
        let is_replacing_water = matches!(args.replacing, BlockIsReplacing::Water(_));
        let mut props = CampfireLikeProperties::from_state_id(args.block.default_state.id, args.block);
        props.waterlogged = is_replacing_water;
        props.signal_fire = is_signal_fire_base_block(args.world.get_block(&args.position.down()));
        props.lit = !is_replacing_water;
        props.facing = args.player.get_entity().get_horizontal_facing();
        props.to_state_id(args.block)
    }

    fn get_state_for_neighbor_update(
        &self,
        args: GetStateForNeighborUpdateArgs<'_>,
    ) -> BlockStateId {
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
            props.signal_fire = is_signal_fire_base_block(args.world.get_block(args.neighbor_position));
        }
        props.to_state_id(args.block)
    }

    fn on_projectile_hit(&self, args: OnProjectileHitArgs<'_>) {
        let props = CampfireLikeProperties::from_state_id(args.state.id, args.block);
        let may_interact = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(crate::entity::projectile::projectile_may_interact(
                args.projectile,
                args.server,
                args.world,
                args.position,
            ))
        });
        if may_interact
            && args.projectile.get_entity().fire_ticks.load(Ordering::Relaxed) > 0
            && !props.lit
            && !props.waterlogged
        {
            let mut lit_props = props;
            lit_props.lit = true;
            args.world.set_block_state(
                args.position,
                lit_props.to_state_id(args.block),
                BlockFlags::NOTIFY_LISTENERS,
            );
        }
    }
}

fn is_signal_fire_base_block(block: &Block) -> bool {
    block == &Block::HAY_BLOCK
}
