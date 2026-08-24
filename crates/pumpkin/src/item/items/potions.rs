use std::pin::Pin;
use std::sync::Arc;

use crate::entity::Entity;
use crate::entity::EntityBase;
use crate::entity::player::Player;
use crate::entity::projectile::{
    lingering_potion::LingeringPotionEntity, splash_potion::SplashPotionEntity,
};
use crate::item::{ItemBehaviour, ItemMetadata};
use pumpkin_data::BlockDirection;
use pumpkin_data::data_component_impl::PotionContentsImpl;
use pumpkin_data::entity::EntityType;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::statistic::StatisticCategory;
use pumpkin_data::tag::Taggable;
use pumpkin_data::{Block, game_event::GameEvent, particle::Particle};
use pumpkin_util::GameMode;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_world::world::BlockFlags;

pub struct PotionItem;
pub struct SplashPotionItem;
pub struct LingeringPotionItem;

impl ItemMetadata for PotionItem {
    fn ids() -> Box<[u16]> {
        [Item::POTION.id].into()
    }
}

impl ItemMetadata for SplashPotionItem {
    fn ids() -> Box<[u16]> {
        [Item::SPLASH_POTION.id].into()
    }
}

impl ItemMetadata for LingeringPotionItem {
    fn ids() -> Box<[u16]> {
        [Item::LINGERING_POTION.id].into()
    }
}

// Vanilla ThrowablePotionItem#use:
// Projectile.spawnProjectileFromRotation(..., -20.0F, 0.5F, 1.0F). The -20.0F is the `yOffset` of
// Projectile#shootFromRotation (`yd = -sin(xRot + yOffset)`), the extra upward arc of the throw.
const Y_OFFSET: f32 = -20.0;
const POWER: f32 = 0.5;

impl ItemBehaviour for PotionItem {
    fn normal_use<'a>(
        &'a self,
        _item: &'a Item,
        _player: &'a Player,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        // Drinking is handled by the consumable flow in the server (active hand + consumption tick).
        Box::pin(async move {})
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    /// Vanilla `PotionItem.useOn` (`PotionItem.java:35-69`): a water potion clicked on a
    /// non-downward convertible block creates five splash particles, returns a glass bottle,
    /// emits `FLUID_PLACE`, and replaces the block with mud.
    fn use_on_block<'a>(
        &'a self,
        item: &'a mut ItemStack,
        player: &'a Player,
        location: BlockPos,
        face: BlockDirection,
        _cursor_pos: Vector3<f32>,
        block: &'a Block,
        _server: &'a crate::server::Server,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            let is_water =
                item.get_data_component::<PotionContentsImpl>()
                    .is_some_and(|contents| {
                        contents.potion_id == Some(pumpkin_data::potion::Potion::WATER.id as i32)
                            && contents.custom_effects.is_empty()
                    });
            if face == BlockDirection::Down
                || !block.has_tag(&pumpkin_data::tag::Block::MINECRAFT_CONVERTABLE_TO_MUD)
                || !is_water
            {
                return;
            }

            let world = player.world();
            world.play_block_sound(
                pumpkin_data::sound::Sound::EntityGenericSplash,
                pumpkin_data::sound::SoundCategory::Blocks,
                location,
            );

            for _ in 0..5 {
                world.spawn_particle(
                    Vector3::new(
                        f64::from(location.0.x) + rand::random::<f64>(),
                        f64::from(location.0.y + 1),
                        f64::from(location.0.z) + rand::random::<f64>(),
                    ),
                    Vector3::new(0.0, 0.0, 0.0),
                    1.0,
                    1,
                    Particle::Splash,
                );
            }

            let glass_bottle = ItemStack::new(1, &Item::GLASS_BOTTLE);
            if player.gamemode.load() == GameMode::Creative {
                if !player.inventory.contains_item(&Item::GLASS_BOTTLE) {
                    player
                        .inventory
                        .offer_or_drop_stack(glass_bottle, player)
                        .await;
                }
            } else if item.item_count == 1 {
                *item = glass_bottle;
            } else {
                item.decrement(1);
                player
                    .inventory
                    .offer_or_drop_stack(glass_bottle, player)
                    .await;
            }

            world.play_block_sound(
                pumpkin_data::sound::Sound::ItemBottleEmpty,
                pumpkin_data::sound::SoundCategory::Blocks,
                location,
            );
            crate::world::game_event::emit_game_event(
                &world,
                GameEvent::FluidPlace,
                location.to_centered_f64(),
                crate::world::game_event::GameEventContext::none(),
            )
            .await;
            world
                .set_block_state(
                    &location,
                    Block::MUD.default_state.id,
                    BlockFlags::NOTIFY_ALL,
                )
                .await;
        })
    }
}

impl ItemBehaviour for SplashPotionItem {
    fn normal_use<'a>(
        &'a self,
        _item: &'a Item,
        player: &'a Player,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            let position = player.position();
            let world = player.world();
            // Vanilla `SplashPotionItem.use` plays this sound before delegating to
            // `ThrowablePotionItem.use` (`SplashPotionItem.java:21-32`).
            world.play_sound_fine(
                pumpkin_data::sound::Sound::EntitySplashPotionThrow,
                pumpkin_data::sound::SoundCategory::Players,
                &position,
                0.5,
                super::throw_sound_pitch(rand::random()),
            );
            let entity = Entity::new(world.clone(), position, &EntityType::SPLASH_POTION);
            let splash = SplashPotionEntity::new_shot(entity, player.get_entity());

            // Copy the held item stack data into the projectile
            let main_s = player.inventory.held_item().await;
            let mut used_main = true;
            let mut stack = (!main_s.is_empty()
                && main_s.item.id == pumpkin_data::item::Item::SPLASH_POTION.id)
                .then_some(main_s);
            if stack.is_none() {
                let off_s = player.inventory.off_hand_item().await;
                if !off_s.is_empty() && off_s.item.id == pumpkin_data::item::Item::SPLASH_POTION.id
                {
                    stack = Some(off_s);
                    used_main = false;
                }
            }
            let stack = stack.unwrap_or_else(|| ItemStack::EMPTY.clone());
            splash.set_item_stack(stack).await;

            let (yaw, pitch) = player.rotation();
            splash
                .thrown
                .set_velocity_from(player.get_entity(), pitch, yaw, Y_OFFSET, POWER, 1.0);

            world.spawn_entity(Arc::new(splash)).await;

            // Decrement the used stack (clear)
            if used_main {
                let mut s = player.inventory.held_item().await;
                s.decrement_unless_creative(player.gamemode.load(), 1);
                player.inventory.set_held_item(s).await;
            } else {
                let mut s = player.inventory.off_hand_item().await;
                s.decrement_unless_creative(player.gamemode.load(), 1);
                player
                    .inventory
                    .set_stack_in_hand(pumpkin_util::Hand::Left, s)
                    .await;
            }

            // `ThrowablePotionItem.use` awards ITEM_USED after spawning and consuming
            // (`ThrowablePotionItem.java:23-31`).
            player
                .increment_stat(StatisticCategory::Used, Item::SPLASH_POTION.id as i32, 1)
                .await;
        })
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

impl ItemBehaviour for LingeringPotionItem {
    fn normal_use<'a>(
        &'a self,
        _item: &'a Item,
        player: &'a Player,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            let position = player.position();
            let world = player.world();
            // Vanilla `LingeringPotionItem.use` plays this sound before delegating to
            // `ThrowablePotionItem.use` (`LingeringPotionItem.java:21-32`).
            world.play_sound_fine(
                pumpkin_data::sound::Sound::EntityLingeringPotionThrow,
                pumpkin_data::sound::SoundCategory::Neutral,
                &position,
                0.5,
                super::throw_sound_pitch(rand::random()),
            );
            let entity = Entity::new(world.clone(), position, &EntityType::LINGERING_POTION);
            let ling = LingeringPotionEntity::new_shot(entity, player.get_entity());

            // Copy the held item stack data into the projectile
            let main_s = player.inventory.held_item().await;
            let mut used_main = true;
            let mut stack = (!main_s.is_empty()
                && main_s.item.id == pumpkin_data::item::Item::LINGERING_POTION.id)
                .then_some(main_s);
            if stack.is_none() {
                let off_s = player.inventory.off_hand_item().await;
                if !off_s.is_empty()
                    && off_s.item.id == pumpkin_data::item::Item::LINGERING_POTION.id
                {
                    stack = Some(off_s);
                    used_main = false;
                }
            }
            let stack = stack.unwrap_or_else(|| ItemStack::EMPTY.clone());
            ling.set_item_stack(stack).await;

            let (yaw, pitch) = player.rotation();
            ling.thrown
                .set_velocity_from(player.get_entity(), pitch, yaw, Y_OFFSET, POWER, 1.0);

            world.spawn_entity(Arc::new(ling)).await;

            // Decrement the used stack (clear)
            if used_main {
                let mut s = player.inventory.held_item().await;
                s.decrement_unless_creative(player.gamemode.load(), 1);
                player.inventory.set_held_item(s).await;
            } else {
                let mut s = player.inventory.off_hand_item().await;
                s.decrement_unless_creative(player.gamemode.load(), 1);
                player
                    .inventory
                    .set_stack_in_hand(pumpkin_util::Hand::Left, s)
                    .await;
            }

            // `ThrowablePotionItem.use` awards ITEM_USED after spawning and consuming
            // (`ThrowablePotionItem.java:23-31`).
            player
                .increment_stat(StatisticCategory::Used, Item::LINGERING_POTION.id as i32, 1)
                .await;
        })
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}
