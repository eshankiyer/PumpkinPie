use std::pin::Pin;

use std::sync::Arc;

use crate::block::entities::mob_spawner::MobSpawnerBlockEntity;
use crate::block::entities::trial_spawner::TrialSpawnerBlockEntity;
use crate::entity::EntityBase;
use crate::entity::player::Player;
use crate::entity::r#type::from_type;
use crate::item::{ItemBehaviour, ItemMetadata};
use crate::server::Server;
use crate::world::World;
use pumpkin_data::data_component_impl::{
    AxolotlVariantImpl, CatVariantImpl, ChickenVariantImpl, CowVariantImpl, FoxVariantImpl,
    FrogVariantImpl, HorseVariantImpl, LlamaVariantImpl, MooshroomVariantImpl, PigVariantImpl,
    RabbitVariantImpl, SheepColorImpl, ShulkerColorImpl, VillagerVariantImpl, WolfVariantImpl,
    ZombieNautilusVariantImpl,
};
use pumpkin_data::entity::{EntityType, entity_from_egg};
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::{Block, BlockDirection};
use pumpkin_util::Hand;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_util::math::wrap_degrees;
use uuid::Uuid;

pub struct SpawnEggItem;

impl ItemMetadata for SpawnEggItem {
    fn ids() -> Box<[u16]> {
        pumpkin_data::entity::spawn_egg_ids()
    }
}

pub(crate) fn apply_entity_variant(item: &ItemStack, mob: &dyn EntityBase) {
    if let Some(comp) = item.get_data_component::<ZombieNautilusVariantImpl>() {
        // Vanilla `ZombieNautilus.applyImplicitComponent` (ZombieNautilus.java:159-165)
        // applies the spawn-egg's ZOMBIE_NAUTILUS_VARIANT component after finalizeSpawn.
        mob.set_variant_name(&comp.value);
    } else if let Some(comp) = item.get_data_component::<ChickenVariantImpl>() {
        mob.set_variant_name(&comp.value);
    } else if let Some(comp) = item.get_data_component::<FrogVariantImpl>() {
        mob.set_variant_name(&comp.value);
    } else if let Some(comp) = item.get_data_component::<WolfVariantImpl>() {
        mob.set_variant_name(&comp.value);
    } else if let Some(comp) = item.get_data_component::<CatVariantImpl>() {
        mob.set_variant_name(&comp.value);
    } else if let Some(comp) = item.get_data_component::<VillagerVariantImpl>() {
        mob.set_variant_name(&comp.value);
    } else if let Some(comp) = item.get_data_component::<FoxVariantImpl>() {
        mob.set_variant_name(&comp.value);
    } else if let Some(comp) = item.get_data_component::<MooshroomVariantImpl>() {
        mob.set_variant_name(&comp.value);
    } else if let Some(comp) = item.get_data_component::<RabbitVariantImpl>() {
        mob.set_variant_name(&comp.value);
    } else if let Some(comp) = item.get_data_component::<PigVariantImpl>() {
        mob.set_variant_name(&comp.value);
    } else if let Some(comp) = item.get_data_component::<CowVariantImpl>() {
        mob.set_variant_name(&comp.value);
    } else if let Some(comp) = item.get_data_component::<HorseVariantImpl>() {
        mob.set_variant_name(&comp.value);
    } else if let Some(comp) = item.get_data_component::<LlamaVariantImpl>() {
        mob.set_variant_name(&comp.value);
    } else if let Some(comp) = item.get_data_component::<AxolotlVariantImpl>() {
        mob.set_variant_name(&comp.value);
    } else if let Some(comp) = item.get_data_component::<SheepColorImpl>() {
        mob.set_variant_name(&comp.value);
    } else if let Some(comp) = item.get_data_component::<ShulkerColorImpl>() {
        mob.set_variant_name(&comp.value);
    }
}

async fn spawn_egg_mob(
    entity_type: &'static EntityType,
    stack: &ItemStack,
    world: &Arc<World>,
    pos: Vector3<f64>,
) {
    // Create rotation like Vanilla
    let yaw = wrap_degrees(rand::random::<f32>() * 360.0) % 360.0;

    let mob = from_type(entity_type, pos, world, Uuid::new_v4());

    // Set the rotation
    mob.get_entity().set_rotation(yaw, 0.0);

    apply_entity_variant(stack, mob.as_ref());

    // `SpawnEggItem.spawn` applies the stack's implicit entity components before adding the
    // offspring (`SpawnEggItem.java:169-171`).
    mob.get_entity()
        .apply_components_from_item_stack(stack)
        .await;

    // Broadcast the new mob to all players
    world.spawn_entity(mob).await;
}

impl ItemBehaviour for SpawnEggItem {
    /// Vanilla `SpawnEggItem.use` (SpawnEggItem.java:100-131): right-clicking without hitting
    /// a block raycasts with `ClipContext.Fluid.SOURCE_ONLY` and, if the hit block is a
    /// `LiquidBlock`, spawns the mob inside the fluid itself (`tryMoveDown = false`).
    /// Without this, water/lava mob eggs (cod, squid, dolphin, strider, ...) do nothing when
    /// used on the surface of a fluid.
    fn normal_use<'a>(
        &'a self,
        item: &'a Item,
        player: &'a Player,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            let Some(entity_type) = entity_from_egg(item.id) else {
                return;
            };
            let world = player.world();
            let (start_pos, end_pos) = self.get_start_and_end_pos(player);

            // ClipContext.Fluid.SOURCE_ONLY: stop on any non-air block outline and on
            // full fluid source blocks, but pass through flowing fluid.
            let checker = async |pos: &BlockPos, world_inner: &Arc<World>| {
                let state_id = world_inner.get_block_state_id(pos);
                let block = Block::from_state_id(state_id);
                if state_id == Block::AIR.default_state.id {
                    return false;
                }
                if block.id == Block::WATER.id || block.id == Block::LAVA.id {
                    return state_id == block.default_state.id;
                }
                true
            };

            let Some((pos, face)) = world.raycast(start_pos, end_pos, checker).await else {
                return;
            };

            // SpawnEggItem.java:114: only a LiquidBlock is spawned into by `use`; anything
            // else is left to `useOn`.
            let hit_block = world.get_block(&pos);
            if hit_block.id != Block::WATER.id && hit_block.id != Block::LAVA.id {
                return;
            }

            let inventory = player.inventory();
            let held = inventory.held_item().await;
            let (mut stack, hand) = if !held.is_empty() && held.item.id == item.id {
                (held, Hand::Right)
            } else {
                let off_hand = inventory.off_hand_item().await;
                if !off_hand.is_empty() && off_hand.item.id == item.id {
                    (off_hand, Hand::Left)
                } else {
                    return;
                }
            };

            // `SpawnEggItem.use` gates liquid spawning with `Player.mayUseItemAt`
            // (`SpawnEggItem.java:112-119`).
            if !player.may_use_item_at(&pos, face, &stack).await {
                return;
            }

            let spawn_pos = Vector3::new(
                f64::from(pos.0.x) + 0.5,
                f64::from(pos.0.y),
                f64::from(pos.0.z) + 0.5,
            );
            spawn_egg_mob(entity_type, &stack, &world, spawn_pos).await;
            stack.decrement_unless_creative(player.gamemode.load(), 1);
            inventory.set_stack_in_hand(hand, stack).await;
        })
    }

    fn use_on_block<'a>(
        &'a self,
        item: &'a mut ItemStack,
        player: &'a Player,
        location: BlockPos,
        face: BlockDirection,
        _cursor_pos: Vector3<f32>,
        _block: &'a Block,
        _server: &'a Server,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            if let Some(entity_type) = entity_from_egg(item.item.id) {
                let world = player.world();

                if let Some(block_entity) = player.world().get_block_entity(&location)
                    && let Some(spawner) = block_entity
                        .as_any()
                        .downcast_ref::<MobSpawnerBlockEntity>()
                {
                    spawner.set_entity_type(entity_type);
                    world.update_block_entity(&block_entity);
                    item.decrement_unless_creative(player.gamemode.load(), 1);
                    return;
                }
                // Vanilla `SpawnEggItem#useOn` (SpawnEggItem.java:54): the `Spawner`
                // check also matches trial spawner block entities, whose
                // `setEntityId` overrides the entity they spawn.
                if let Some(block_entity) = player.world().get_block_entity(&location)
                    && let Some(trial_spawner) = block_entity
                        .as_any()
                        .downcast_ref::<TrialSpawnerBlockEntity>()
                {
                    trial_spawner.set_entity_id(&world, entity_type).await;
                    world.update_block_entity(&block_entity);
                    item.decrement_unless_creative(player.gamemode.load(), 1);
                    return;
                }
                // Vanilla `SpawnEggItem#useOn`: the mob is placed inside the clicked block when
                // that block has no collision shape (grass, torches, ...), and only otherwise on
                // the block adjacent to the clicked face.
                let pos = if world
                    .get_block_state(&location)
                    .get_block_collision_shapes()
                    .next()
                    .is_none()
                {
                    location
                } else {
                    BlockPos(location.0 + face.to_offset())
                };
                let pos = Vector3::new(
                    f64::from(pos.0.x) + 0.5,
                    f64::from(pos.0.y),
                    f64::from(pos.0.z) + 0.5,
                );
                spawn_egg_mob(entity_type, item, &world, pos).await;
                item.decrement_unless_creative(player.gamemode.load(), 1);
            }
        })
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}
