use pumpkin_data::sound::Sound;
use pumpkin_data::{Block, BlockId, BlockState};

use pumpkin_data::BlockStateId;
use pumpkin_data::block_properties::blocks_movement;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::random::{RandomGenerator, get_seed, xoroshiro128::Xoroshiro};

use crate::entity::experience_orb::ExperienceOrbEntity;
use crate::entity::player::Player;
use crate::world::World;
use crate::world::loot::{LootContextParameters, LootTableExt};
use std::pin::Pin;
use std::sync::Arc;

pub mod blocks;
pub mod entities;
pub mod fluid;
pub mod pathfindable;
pub mod registry;
pub mod sculk_behaviour;
pub mod viewer;

use crate::block::entities::BlockEntity;
use crate::block::registry::BlockActionResult;
use crate::entity::EntityBase;
use crate::server::Server;
use pumpkin_data::BlockDirection;
use pumpkin_data::block_rotation::{Mirror, Rotation};
use pumpkin_data::data_component_impl::EquipmentSlot;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::tag::Taggable;
use pumpkin_protocol::java::server::play::SUseItemOn;
use pumpkin_util::math::boundingbox::BoundingBox;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_world::world::{BlockAccessor, BlockFlags};

pub trait BlockMetadata {
    fn ids() -> Box<[BlockId]>;
}

pub trait FluidMetadata {
    fn ids() -> Box<[u16]>;
}

pub type BlockFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

pub(crate) fn stop_vertical_movement_after_fall(entity: &dyn EntityBase) {
    let entity = entity.get_entity();
    let mut velocity = entity.velocity.load();
    velocity.y = 0.0;
    entity.velocity.store(velocity);
}

pub(crate) fn bounce_entity_after_fall(entity: &dyn EntityBase, bounce_multiplier: f64) {
    let base_entity = entity.get_entity();
    let mut velocity = base_entity.velocity.load();

    if base_entity.is_sneaking() {
        velocity.y = 0.0;
    } else if velocity.y < 0.0 {
        let entity_factor = if entity.get_living_entity().is_some() {
            1.0
        } else {
            0.8
        };
        velocity.y = -velocity.y * bounce_multiplier * entity_factor;
    }

    base_entity.velocity.store(velocity);
}

pub(crate) fn block_bounce_restitution(block: &Block) -> f64 {
    // `Block.getBounceRestitution` (`Block.java:494-499`) returns the property set by
    // `Blocks.java:675-685` for beds and `Blocks.java:2922-2926` for slime blocks.
    if block.has_tag(&pumpkin_data::tag::Block::MINECRAFT_BEDS) {
        0.75
    } else if block == &Block::SLIME_BLOCK {
        1.0
    } else {
        0.0
    }
}

/// Returns the server-side step/fall sounds and the vanilla sound volume/pitch modifiers.
/// `BlockBehaviour.getSoundType` returns the registered `SoundType` (`BlockBehaviour.java:405-407`);
/// common material registrations are in `SoundType.java:10-46` and
/// `Blocks.java:83-98,315-324,547-552,557-563`.
#[must_use]
#[expect(clippy::too_many_lines)]
pub(crate) fn block_sound_type(block: &Block) -> (Sound, Sound, f32, f32) {
    let name = block.name;
    match name {
        "anvil" => (Sound::BlockAnvilStep, Sound::BlockAnvilFall, 0.3, 1.0),
        "slime_block" => (
            Sound::BlockSlimeBlockStep,
            Sound::BlockSlimeBlockFall,
            1.0,
            1.0,
        ),
        "honey_block" => (
            Sound::BlockHoneyBlockStep,
            Sound::BlockHoneyBlockFall,
            1.0,
            1.0,
        ),
        "powder_snow" => (
            Sound::BlockPowderSnowStep,
            Sound::BlockPowderSnowFall,
            1.0,
            1.0,
        ),
        "snow" | "snow_block" => (Sound::BlockSnowStep, Sound::BlockSnowFall, 1.0, 1.0),
        "sand" | "red_sand" => (Sound::BlockSandStep, Sound::BlockSandFall, 1.0, 1.0),
        "suspicious_sand" => (
            Sound::BlockSuspiciousSandStep,
            Sound::BlockSuspiciousSandFall,
            1.0,
            1.0,
        ),
        "gravel" => (Sound::BlockGravelStep, Sound::BlockGravelFall, 1.0, 1.0),
        "suspicious_gravel" => (
            Sound::BlockSuspiciousGravelStep,
            Sound::BlockSuspiciousGravelFall,
            1.0,
            1.0,
        ),
        "glass" | "glass_pane" | "tinted_glass" => {
            (Sound::BlockGlassStep, Sound::BlockGlassFall, 1.0, 1.0)
        }
        name if name.ends_with("_glass") || name.ends_with("_glass_pane") => {
            (Sound::BlockGlassStep, Sound::BlockGlassFall, 1.0, 1.0)
        }
        "grass_block" | "short_grass" | "tall_grass" => {
            (Sound::BlockGrassStep, Sound::BlockGrassFall, 1.0, 1.0)
        }
        name if name.ends_with("_leaves") => {
            (Sound::BlockGrassStep, Sound::BlockGrassFall, 1.0, 1.0)
        }
        "dirt" | "coarse_dirt" | "podzol" | "mycelium" | "rooted_dirt" | "dirt_path"
        | "farmland" | "clay" => (Sound::BlockGravelStep, Sound::BlockGravelFall, 1.0, 1.0),
        "moss_block" => (Sound::BlockMossStep, Sound::BlockMossFall, 1.0, 1.0),
        "mud" => (Sound::BlockMudStep, Sound::BlockMudFall, 1.0, 1.0),
        "netherrack" => (
            Sound::BlockNetherrackStep,
            Sound::BlockNetherrackFall,
            1.0,
            1.0,
        ),
        name if name.starts_with("deepslate") => (
            Sound::BlockDeepslateStep,
            Sound::BlockDeepslateFall,
            1.0,
            1.0,
        ),
        name if name.starts_with("copper") || name.ends_with("_copper_block") => {
            (Sound::BlockCopperStep, Sound::BlockCopperFall, 1.0, 1.0)
        }
        name if name.starts_with("tuff") => (Sound::BlockTuffStep, Sound::BlockTuffFall, 1.0, 1.0),
        name if name.starts_with("resin") => {
            (Sound::BlockResinStep, Sound::BlockResinFall, 1.0, 1.0)
        }
        name if name.starts_with("sulfur") => {
            (Sound::BlockSulfurStep, Sound::BlockSulfurFall, 1.0, 1.0)
        }
        name if name.starts_with("cinnabar") => {
            (Sound::BlockCinnabarStep, Sound::BlockCinnabarFall, 1.0, 1.0)
        }
        "bamboo" | "bamboo_sapling" => (Sound::BlockBambooStep, Sound::BlockBambooFall, 1.0, 1.0),
        name if name.starts_with("bamboo_") => (
            Sound::BlockBambooWoodStep,
            Sound::BlockBambooWoodFall,
            1.0,
            1.0,
        ),
        name if name.starts_with("cherry_")
            && (name.ends_with("_planks") || name.ends_with("_log") || name.ends_with("_wood")) =>
        {
            (
                Sound::BlockCherryWoodStep,
                Sound::BlockCherryWoodFall,
                1.0,
                1.0,
            )
        }
        name if (name.starts_with("crimson_") || name.starts_with("warped_"))
            && (name.ends_with("_planks")
                || name.ends_with("_stem")
                || name.ends_with("_hyphae")) =>
        {
            (
                Sound::BlockNetherWoodStep,
                Sound::BlockNetherWoodFall,
                1.0,
                1.0,
            )
        }
        name if name.ends_with("_planks")
            || name.ends_with("_log")
            || name.ends_with("_wood")
            || name.starts_with("stripped_") =>
        {
            (Sound::BlockWoodStep, Sound::BlockWoodFall, 1.0, 1.0)
        }
        name if name.ends_with("_wool") || name.ends_with("_carpet") => {
            (Sound::BlockWoolStep, Sound::BlockWoolFall, 1.0, 1.0)
        }
        name if name == "iron_block" || name.ends_with("_iron_bars") => {
            (Sound::BlockIronStep, Sound::BlockIronFall, 1.0, 1.0)
        }
        name if name.ends_with("_block") && (name.contains("gold") || name.contains("diamond")) => {
            (Sound::BlockMetalStep, Sound::BlockMetalFall, 1.0, 1.0)
        }
        _ => (Sound::BlockStoneStep, Sound::BlockStoneFall, 1.0, 1.0),
    }
}

/// Returns whether a block participates in comparator output notification.
/// Vanilla's `BlockBehaviour.hasAnalogOutputSignal` defaults to false and is
/// enabled by the listed block implementations (`BlockBehaviour.java:235-237`,
/// `BarrelBlock.java:72-75`, `AbstractFurnaceBlock.java:60-64`,
/// `AbstractCauldronBlock.java:78-82`, and `BlockBehaviour.java:637-643`).
#[must_use]
pub(crate) fn has_analog_output_signal(block: &Block) -> bool {
    let name = block.name;
    name == "barrel"
        || name == "furnace"
        || name == "blast_furnace"
        || name == "smoker"
        || name == "cauldron"
        || name == "water_cauldron"
        || name == "lava_cauldron"
        || name == "powder_snow_cauldron"
        || name == "beehive"
        || name == "bee_nest"
        || name == "brewing_stand"
        || name == "cake"
        || name.contains("candle_cake")
        || name == "chest"
        || name == "trapped_chest"
        || name == "copper_chest"
        || name.ends_with("_copper_chest")
        || name == "chiseled_bookshelf"
        || name == "command_block"
        || name == "chain_command_block"
        || name == "repeating_command_block"
        || name == "composter"
        || name == "copper_bulb"
        || name.ends_with("_copper_bulb")
        || name == "copper_golem_statue"
        || name.ends_with("_copper_golem_statue")
        || name == "crafter"
        || name == "creaking_heart"
        || name == "decorated_pot"
        || name == "detector_rail"
        || name == "dispenser"
        || name == "dropper"
        || name == "end_portal_frame"
        || name == "hopper"
        || name == "jukebox"
        || name == "lectern"
        || name == "respawn_anchor"
        || name == "sculk_sensor"
        || name == "calibrated_sculk_sensor"
        || name.ends_with("_shelf")
        || name == "shulker_box"
        || name.ends_with("_shulker_box")
}

/// Matches vanilla `BlockStateBase.isSuffocating` (`BlockBehaviour.java:801-803`) and the
/// block-property overrides registered in `Blocks.java:421-422, 585-586, 637-638, 1299-1300,
/// 2027-2028, 3702-3703, 5116-5117, 5257-5258, 5478-5479, 5699-5708, 5769-5794`.
pub(crate) fn is_suffocating(block: &Block, state: &BlockState, shulker_closed: bool) -> bool {
    match block.name {
        "farmland" | "dirt_path" | "mud" | "soul_sand" | "end_gateway" => true,
        "mangrove_roots" | "glass" | "moving_piston" | "repeater" | "tinted_glass"
        | "firefly_bush" => false,
        name if name.ends_with("_leaves")
            || name.ends_with("_stained_glass")
            || name.ends_with("copper_grate") =>
        {
            false
        }
        name if name == "shulker_box" || name.ends_with("_shulker_box") => shulker_closed,
        "piston" | "sticky_piston" => !block.properties(state.id).is_some_and(|properties| {
            properties
                .to_props()
                .iter()
                .any(|(key, value)| *key == "extended" && *value == "true")
        }),
        _ => blocks_movement(state, block.id) && state.is_full_cube(),
    }
}

pub trait BlockBehaviour: Send + Sync {
    fn is_valid_bonemeal_target(&self, _args: BonemealArgs<'_>) -> bool {
        false
    }

    fn is_bonemeal_success(&self, _args: BonemealArgs<'_>) -> bool {
        true
    }

    fn perform_bonemeal<'a>(&'a self, _args: BonemealArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async {})
    }

    /// Called when a player starts punching this block (`BlockBehaviour.attack` in vanilla).
    fn attack<'a>(&'a self, _args: AttackArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async {})
    }

    fn normal_use<'a>(&'a self, _args: NormalUseArgs<'a>) -> BlockFuture<'a, BlockActionResult> {
        Box::pin(async move { BlockActionResult::Pass })
    }

    fn use_with_item<'a>(
        &'a self,
        _args: UseWithItemArgs<'a>,
    ) -> BlockFuture<'a, BlockActionResult> {
        Box::pin(async move { BlockActionResult::PassToDefaultBlockAction })
    }

    fn on_entity_collision<'a>(&'a self, _args: OnEntityCollisionArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async {})
    }

    fn on_projectile_hit<'a>(&'a self, _args: OnProjectileHitArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async {})
    }

    /// Called when an entity is standing on / walking over the top face of this block.
    fn on_entity_step<'a>(&'a self, _args: OnEntityStepArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async {})
    }

    fn should_drop_items_on_explosion(&self) -> bool {
        true
    }

    fn explode<'a>(&'a self, _args: ExplodeArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async {})
    }

    /// Handles the block event, which is an event specific to a block with an integer ID and data.
    ///
    /// returns whether the event was handled successfully
    fn on_synced_block_event<'a>(
        &'a self,
        _args: OnSyncedBlockEventArgs<'a>,
    ) -> BlockFuture<'a, bool> {
        Box::pin(async move { false })
    }

    /// getPlacementState in source code
    fn on_place<'a>(&'a self, args: OnPlaceArgs<'a>) -> BlockFuture<'a, BlockStateId> {
        Box::pin(async move { args.block.default_state.id })
    }

    fn random_tick<'a>(&'a self, _args: RandomTickArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async {})
    }

    fn can_place_at(&self, _args: CanPlaceAtArgs<'_>) -> bool {
        true
    }

    fn can_update_at(&self, _args: CanUpdateAtArgs<'_>) -> bool {
        false
    }

    /// onBlockAdded in source code
    fn placed<'a>(&'a self, _args: PlacedArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async {})
    }

    fn player_placed<'a>(&'a self, _args: PlayerPlacedArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async {})
    }

    fn on_landed_upon<'a>(&'a self, args: OnLandedUponArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async move {
            if let Some(living) = args.entity.get_living_entity() {
                living
                    .handle_fall_damage(args.entity, args.fall_distance, 1.0)
                    .await;
            }
        })
    }

    fn update_entity_movement_after_fall_on<'a>(
        &'a self,
        args: UpdateEntityMovementAfterFallOnArgs<'a>,
    ) -> BlockFuture<'a, ()> {
        Box::pin(async move {
            stop_vertical_movement_after_fall(args.entity);
        })
    }

    fn broken<'a>(&'a self, _args: BrokenArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async {})
    }

    /// Called before a player destroys a block, matching vanilla's
    /// `BlockBehaviour.playerWillDestroy` hook.
    fn player_will_destroy<'a>(&'a self, _args: PlayerWillDestroyArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async {})
    }

    fn on_neighbor_update<'a>(&'a self, _args: OnNeighborUpdateArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async {})
    }

    /// Called if a block state is replaced or it replaces another state
    fn prepare<'a>(&'a self, _args: PrepareArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async {})
    }

    fn get_state_for_neighbor_update<'a>(
        &'a self,
        args: GetStateForNeighborUpdateArgs<'a>,
    ) -> BlockFuture<'a, BlockStateId> {
        Box::pin(async move { args.state_id })
    }

    fn on_scheduled_tick<'a>(&'a self, _args: OnScheduledTickArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async {})
    }

    /// `Block#handlePrecipitation`: called from `ServerLevel#tickPrecipitation` on the block
    /// directly below the exposed rain/snow column while it is raining. Only cauldrons override
    /// this; every other block is a no-op like vanilla's base implementation.
    fn handle_precipitation<'a>(
        &'a self,
        _args: HandlePrecipitationArgs<'a>,
    ) -> BlockFuture<'a, ()> {
        Box::pin(async {})
    }

    fn on_state_replaced<'a>(&'a self, _args: OnStateReplacedArgs<'a>) -> BlockFuture<'a, ()> {
        Box::pin(async {})
    }

    // --- Redstone/Comparator Methods ---

    /// Sides where redstone connects to
    fn emits_redstone_power<'a>(
        &'a self,
        _args: EmitsRedstonePowerArgs<'a>,
    ) -> BlockFuture<'a, bool> {
        Box::pin(async move { false })
    }

    /// Weak redstone power, aka. block that should be powered needs to be directly next to the source block
    fn get_weak_redstone_power<'a>(
        &'a self,
        _args: GetRedstonePowerArgs<'a>,
    ) -> BlockFuture<'a, u8> {
        Box::pin(async move { 0 })
    }

    /// Strong redstone power. this can power a block that then gives power
    fn get_strong_redstone_power<'a>(
        &'a self,
        _args: GetRedstonePowerArgs<'a>,
    ) -> BlockFuture<'a, u8> {
        Box::pin(async move { 0 })
    }

    fn get_comparator_output<'a>(
        &'a self,
        _args: GetComparatorOutputArgs<'a>,
    ) -> BlockFuture<'a, Option<u8>> {
        Box::pin(async move { None })
    }

    fn get_inside_collision_shape<'a>(
        &'a self,
        _args: GetInsideCollisionShapeArgs<'a>,
    ) -> BlockFuture<'a, BoundingBox> {
        Box::pin(async move { BoundingBox::full_block() })
    }

    fn mirror(&self, block: &Block, state_id: BlockStateId, mirror: Mirror) -> &'static BlockState {
        block.mirror(state_id, mirror)
    }

    fn rotate(
        &self,
        block: &Block,
        state_id: BlockStateId,
        rotation: Rotation,
    ) -> &'static BlockState {
        block.rotate(state_id, rotation)
    }

    /// Vanilla `Block#getCloneItemStack` (creative pick-block). Returns an optional item stack
    /// to give the player when middle-clicking this block, overriding the block's registered
    /// item. The default (`None`) falls back to the block's `item_id`, matching vanilla's base
    /// implementation.
    fn get_clone_item_stack(&self, _args: GetCloneItemStackArgs<'_>) -> Option<ItemStack> {
        None
    }
}

pub struct GetCloneItemStackArgs<'a> {
    pub world: &'a Arc<World>,
    pub block: &'a Block,
    pub position: &'a BlockPos,
}

#[derive(Clone, Copy)]
pub struct BonemealArgs<'a> {
    pub world: &'a Arc<World>,
    pub block: &'a Block,
    pub position: &'a BlockPos,
    pub state_id: BlockStateId,
}

pub struct NormalUseArgs<'a> {
    pub server: &'a Server,
    pub world: &'a Arc<World>,
    pub block: &'a Block,
    pub position: &'a BlockPos,
    pub player: &'a Arc<Player>,
    pub hit: &'a BlockHitResult<'a>,
}

pub struct UseWithItemArgs<'a> {
    pub server: &'a Server,
    pub world: &'a Arc<World>,
    pub block: &'a Block,
    pub position: &'a BlockPos,
    pub player: &'a Arc<Player>,
    pub hit: &'a BlockHitResult<'a>,
    pub item_stack: &'a mut ItemStack,
    pub equipment_slot: &'a EquipmentSlot,
}

pub struct BlockHitResult<'a> {
    pub face: &'a BlockDirection,
    pub cursor_pos: &'a Vector3<f32>,
}

pub struct AttackArgs<'a> {
    pub world: &'a Arc<World>,
    pub block: &'a Block,
    pub state: &'a BlockState,
    pub position: &'a BlockPos,
    pub player: &'a Arc<Player>,
}

pub struct OnEntityCollisionArgs<'a> {
    pub server: &'a Server,
    pub world: &'a Arc<World>,
    pub block: &'a Block,
    pub state: &'a BlockState,
    pub position: &'a BlockPos,
    pub entity: &'a dyn EntityBase,
}

pub struct OnProjectileHitArgs<'a> {
    pub server: &'a Server,
    pub world: &'a Arc<World>,
    pub block: &'a Block,
    pub state: &'a BlockState,
    pub position: &'a BlockPos,
    pub projectile: &'a dyn EntityBase,
    pub hit: &'a BlockHitResult<'a>,
}

pub struct OnEntityStepArgs<'a> {
    pub world: &'a Arc<World>,
    pub block: &'a Block,
    pub state: &'a BlockState,
    pub position: &'a BlockPos,
    pub entity: &'a dyn EntityBase,
    pub below_supporting_block: bool,
}

pub struct ExplodeArgs<'a> {
    pub world: &'a Arc<World>,
    pub block: &'a Block,
    pub position: &'a BlockPos,
    /// Vanilla `ServerExplosion.canTriggerBlocks()` (`ServerExplosion.java:297-302`): only true
    /// for `TRIGGER_BLOCK` blasts (wind charges), which may flip open doors/buttons/etc.
    /// without destroying them. The breeze-owned mob-griefing special case is not modeled
    /// because `Explosion` carries no source entity here (see `explosion.rs`).
    pub can_trigger_blocks: bool,
}

pub struct OnSyncedBlockEventArgs<'a> {
    pub world: &'a Arc<World>,
    pub block: &'a Block,
    pub position: &'a BlockPos,
    pub r#type: u8,
    pub data: u8,
}

pub struct OnPlaceArgs<'a> {
    pub server: &'a Server,
    pub world: &'a World,
    pub block: &'a Block,
    pub position: &'a BlockPos,
    pub direction: BlockDirection,
    pub player: &'a Player,
    pub replacing: BlockIsReplacing,
    pub use_item_on: &'a SUseItemOn,
}

pub struct RandomTickArgs<'a> {
    pub world: &'a Arc<World>,
    pub block: &'a Block,
    pub position: &'a BlockPos,
}

pub struct CanPlaceAtArgs<'a> {
    pub server: Option<&'a Server>,
    pub world: Option<&'a World>,
    pub block_accessor: &'a dyn BlockAccessor,
    pub block: &'a Block,
    pub state: &'a BlockState,
    pub position: &'a BlockPos,
    pub direction: Option<BlockDirection>,
    pub player: Option<&'a Player>,
    pub use_item_on: Option<&'a SUseItemOn>,
}

pub struct CanUpdateAtArgs<'a> {
    pub world: &'a World,
    pub block: &'a Block,
    pub state_id: BlockStateId,
    pub position: &'a BlockPos,
    pub direction: BlockDirection,
    pub player: &'a Player,
    pub use_item_on: &'a SUseItemOn,
}

pub struct PlacedArgs<'a> {
    pub world: &'a Arc<World>,
    pub block: &'a Block,
    pub state_id: BlockStateId,
    pub old_state_id: BlockStateId,
    pub position: &'a BlockPos,
    pub notify: bool,
}

pub struct PlayerPlacedArgs<'a> {
    pub world: &'a Arc<World>,
    pub block: &'a Block,
    pub state_id: BlockStateId,
    pub position: &'a BlockPos,
    pub direction: BlockDirection,
    pub player: &'a Player,
}

pub struct OnLandedUponArgs<'a> {
    pub world: &'a Arc<World>,
    pub position: &'a BlockPos,
    pub fall_distance: f32,
    pub entity: &'a dyn EntityBase,
}

pub struct UpdateEntityMovementAfterFallOnArgs<'a> {
    pub entity: &'a dyn EntityBase,
}

pub struct BrokenArgs<'a> {
    pub block: &'a Block,
    pub player: &'a Arc<Player>,
    pub position: &'a BlockPos,
    pub server: &'a Server,
    pub world: &'a Arc<World>,
    pub state: &'a BlockState,
    /// Vanilla passes this from `ServerPlayerGameMode.destroyBlock`'s
    /// `hasCorrectToolForDrops` result to `Block.spawnAfterBreak`
    /// (`ServerPlayerGameMode.java:293-299`, `SculkShriekerBlock.java:128-132`).
    pub drop_experience: bool,
    /// Captured before block removal for post-removal callbacks such as
    /// `BeehiveBlock.playerDestroy` (`BeehiveBlock.java:91-108`).
    pub block_entity: Option<&'a dyn BlockEntity>,
}

pub struct PlayerWillDestroyArgs<'a> {
    pub block: &'a Block,
    pub player: &'a Arc<Player>,
    pub position: &'a BlockPos,
    pub world: &'a Arc<World>,
    pub state: &'a BlockState,
}

pub struct OnNeighborUpdateArgs<'a> {
    pub world: &'a Arc<World>,
    pub block: &'a Block,
    pub position: &'a BlockPos,
    pub source_block: &'a Block,
    pub notify: bool,
}

pub struct PrepareArgs<'a> {
    pub world: &'a Arc<World>,
    pub block: &'a Block,
    pub state_id: BlockStateId,
    pub position: &'a BlockPos,
    pub flags: BlockFlags,
}

pub struct GetStateForNeighborUpdateArgs<'a> {
    pub world: &'a World,
    pub block: &'a Block,
    pub state_id: BlockStateId,
    pub position: &'a BlockPos,
    pub direction: BlockDirection,
    pub neighbor_position: &'a BlockPos,
    pub neighbor_state_id: BlockStateId,
}

pub struct OnScheduledTickArgs<'a> {
    pub world: &'a Arc<World>,
    pub block: &'a Block,
    pub position: &'a BlockPos,
}

/// `Biome.Precipitation`, as narrowed by `ServerLevel#tickPrecipitation`: it only ever calls
/// `handlePrecipitation` with `RAIN` or `SNOW`, never `NONE`.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Precipitation {
    Rain,
    Snow,
}

pub struct HandlePrecipitationArgs<'a> {
    pub world: &'a Arc<World>,
    pub block: &'a Block,
    pub state_id: BlockStateId,
    pub position: &'a BlockPos,
    pub precipitation: Precipitation,
}

pub struct OnStateReplacedArgs<'a> {
    pub world: &'a Arc<World>,
    pub block: &'a Block,
    pub old_state_id: BlockStateId,
    pub position: &'a BlockPos,
    pub moved: bool,
}

pub struct EmitsRedstonePowerArgs<'a> {
    pub block: &'a Block,
    pub state: &'a BlockState,
    pub direction: BlockDirection,
}

pub struct GetRedstonePowerArgs<'a> {
    pub world: &'a World,
    pub block: &'a Block,
    pub state: &'a BlockState,
    pub position: &'a BlockPos,
    pub direction: BlockDirection,
}

pub struct GetComparatorOutputArgs<'a> {
    pub world: &'a World,
    pub block: &'a Block,
    pub state: &'a BlockState,
    pub position: &'a BlockPos,
}

pub struct GetInsideCollisionShapeArgs<'a> {
    pub world: &'a World,
    pub block: &'a Block,
    pub state: &'a BlockState,
    pub position: &'a BlockPos,
}

#[derive(Clone)]
pub struct BlockEvent {
    pub pos: BlockPos,
    pub r#type: u8,
    pub data: u8,
}

pub async fn drop_loot(
    world: &Arc<World>,
    block: &Block,
    pos: &BlockPos,
    experience: bool,
    params: LootContextParameters,
) {
    // In 1.21 a tool's `block_experience` enchantment effects are folded over
    // the sampled amount, and Silk Touch is the only enchantment that defines
    // one: it sets the result to zero. Special-casing it is equivalent for
    // now, but a general effect pass would be the faithful implementation.
    // Read before `params` is moved into the loot table below.
    let silk_touched = experience
        && block.experience.is_some()
        && params.tool.as_ref().is_some_and(|tool| {
            tool.get_enchantment_level(&pumpkin_data::Enchantment::SILK_TOUCH) > 0
        });

    // Vanilla gates both item drops and experience on this rule, in
    // `Block#popResource` and `Block#popExperience` respectively, so with the
    // rule off an ore drops neither its item nor its experience orbs.
    let block_drops = world.level_info.load().game_rules.block_drops;

    if block_drops && let Some(loot_table) = &block.loot_table {
        let items = loot_table.get_loot(params);
        if !items.is_empty() {
            let mut event = crate::plugin::block::block_drop_item::BlockDropItemEvent {
                block_pos: *pos,
                world: world.clone(),
                player: None,
                items,
                cancelled: false,
            };
            if let Some(server) = world.server.upgrade() {
                server.plugin_manager.fire(&server, &mut event).await;
            }
            if !event.cancelled {
                for stack in event.items {
                    world.drop_stack(pos, stack).await;
                }
            }
        }
    }

    if block_drops
        && experience
        && !silk_touched
        && let Some(experience) = &block.experience
    {
        let mut random = RandomGenerator::Xoroshiro(Xoroshiro::from_seed(get_seed()));
        let amount = experience.experience.get(&mut random);
        if amount > 0 {
            let mut event = crate::plugin::block::block_exp::BlockExpEvent {
                block_pos: *pos,
                world: world.clone(),
                exp: amount,
            };
            if let Some(server) = world.server.upgrade() {
                server.plugin_manager.fire(&server, &mut event).await;
            }
            if event.exp > 0 {
                ExperienceOrbEntity::spawn(world, pos.to_f64(), event.exp as u32).await;
            }
        }
    }
}

/// Keeps the pre-removal state used by `BlockBehaviour.getDrops` when the world has already
/// replaced the block (`BlockBehaviour.java:272-280`; `ServerPlayerGameMode.java:279-298`).
#[must_use]
pub(crate) const fn block_drop_state(state_id: BlockStateId) -> &'static BlockState {
    BlockState::from_id(state_id)
}

pub async fn calc_block_breaking(
    player: &Player,
    state: &BlockState,
    block: &'static Block,
) -> f32 {
    let hardness = state.hardness;
    #[expect(clippy::float_cmp)]
    if hardness == -1.0 {
        // unbreakable
        return 0.0;
    }
    let i = if player.can_harvest(state, block).await {
        30.0
    } else {
        100.0
    };

    player.get_mining_speed(block).await / hardness / i
}

#[derive(PartialEq, Eq, Debug)]
pub enum BlockIsReplacing {
    Itself(BlockStateId),
    Water(u8),
    Other,
    None,
}

impl BlockIsReplacing {
    #[must_use]
    /// Returns true if the block was a water source block.
    pub const fn water_source(&self) -> bool {
        match self {
            // Level 0 means the water is a source block
            Self::Water(level) => *level == 0,
            _ => false,
        }
    }
}

pub async fn calculate_comparator_output(
    inventory: &dyn pumpkin_world::inventory::Inventory,
) -> u8 {
    let size = inventory.size();
    if size == 0 {
        return 0;
    }
    let mut fill_sum = 0.0;
    let mut non_empty_count = 0;
    for i in 0..size {
        let stack = inventory.get_stack(i).await;
        if !stack.is_empty() {
            let max_stack = stack.get_max_stack_size() as f32;
            let count = stack.item_count as f32;
            fill_sum += count / max_stack;
            non_empty_count += 1;
        }
    }
    if non_empty_count == 0 {
        return 0;
    }
    let percentage = fill_sum / (size as f32);
    let output = 1.0 + percentage * 14.0;
    output.floor() as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn block_bounce_restitution_matches_vanilla_properties() {
        assert_eq!(block_bounce_restitution(&Block::RED_BED), 0.75);
        assert_eq!(block_bounce_restitution(&Block::SLIME_BLOCK), 1.0);
        assert_eq!(block_bounce_restitution(&Block::STONE), 0.0);
    }

    /// Vanilla walking and landing use the block `SoundType` step/fall sounds
    /// (`Entity.java:1457-1460`; `LivingEntity.java:1858-1867`).
    #[test]
    fn block_sound_type_matches_common_vanilla_materials() {
        assert_eq!(
            block_sound_type(&Block::STONE),
            (Sound::BlockStoneStep, Sound::BlockStoneFall, 1.0, 1.0)
        );
        assert_eq!(
            block_sound_type(&Block::GRASS_BLOCK),
            (Sound::BlockGrassStep, Sound::BlockGrassFall, 1.0, 1.0)
        );
        assert_eq!(
            block_sound_type(&Block::SAND),
            (Sound::BlockSandStep, Sound::BlockSandFall, 1.0, 1.0)
        );
        assert_eq!(
            block_sound_type(&Block::ANVIL),
            (Sound::BlockAnvilStep, Sound::BlockAnvilFall, 0.3, 1.0)
        );
    }

    /// `BlockBehaviour` stores the shared material values in its properties
    /// (`BlockBehaviour.java:86-97`) and exposes the state collision shape and
    /// side-solid checks (`BlockBehaviour.java:327-329,681-709`). Pumpkin's
    /// generated block and block-state tables are the runtime representation.
    #[test]
    fn block_behaviour_material_properties_are_data_modeled() {
        assert_eq!(Block::STONE.hardness, 1.5);
        assert_eq!(Block::STONE.blast_resistance, 6.0);
        assert_eq!(Block::STONE.map_color, 11);
        assert_eq!(Block::ICE.slipperiness, 0.98);
        assert_eq!(Block::STONE.default_state.hardness, Block::STONE.hardness);
        assert!(Block::STONE.default_state.is_side_solid(BlockDirection::Up));
        assert!(
            Block::STONE
                .default_state
                .get_block_collision_shapes()
                .next()
                .is_some()
        );
        assert!(
            Block::AIR
                .default_state
                .get_block_collision_shapes()
                .next()
                .is_none()
        );
    }

    #[test]
    fn block_behaviour_blocks_motion_matches_legacy_exceptions() {
        // Vanilla `BlockStateBase.blocksMotion` uses legacy solidity except for cobweb and
        // bamboo sapling (`BlockBehaviour.java:541-545`). The generated block-property table is
        // the live Rust implementation used by fluid and collision callers.
        assert!(blocks_movement(Block::STONE.default_state, Block::STONE.id));
        assert!(!blocks_movement(
            Block::COBWEB.default_state,
            Block::COBWEB.id
        ));
        assert!(!blocks_movement(
            Block::BAMBOO_SAPLING.default_state,
            Block::BAMBOO_SAPLING.id
        ));
    }

    /// Vanilla caches `useShapeForLightOcclusion` while constructing each block state
    /// (`BlockBehaviour.java:227-229,460-465,576-577`); generated state data is the live
    /// representation used by the lighting and occlusion queries.
    #[test]
    fn light_occlusion_shape_selection_is_data_modeled() {
        assert!(!Block::STONE.default_state.use_shape_for_light_occlusion);
        assert!(
            Block::STONECUTTER
                .default_state
                .use_shape_for_light_occlusion
        );
        assert!(
            Block::OAK_STAIRS
                .default_state
                .use_shape_for_light_occlusion
        );
    }

    /// Vanilla gates `Level.updateNeighbourForOutputSignal` on
    /// `BlockState.hasAnalogOutputSignal` (`Level.java:250-254`; specialized
    /// registrations are listed at `BarrelBlock.java:72-75` and
    /// `ChestBlock.java:353-357`).
    #[test]
    fn analog_output_signal_registration_matches_vanilla_families() {
        assert!(has_analog_output_signal(&Block::BARREL));
        assert!(has_analog_output_signal(&Block::CANDLE_CAKE));
        assert!(has_analog_output_signal(&Block::WAXED_COPPER_BULB));
        assert!(!has_analog_output_signal(&Block::ENDER_CHEST));
        assert!(!has_analog_output_signal(&Block::STONE));
    }

    /// Vanilla `BlockStateBase.isSuffocating` defaults at `BlockBehaviour.java:801-803`;
    /// these cases cover the registered overrides in `Blocks.java:637-638, 1299-1300,
    /// 3702-3703, 5257-5258, 5783-5785`.
    #[test]
    fn suffocation_predicate_matches_vanilla_overrides() {
        assert!(is_suffocating(
            &Block::FARMLAND,
            Block::FARMLAND.default_state,
            true
        ));
        assert!(is_suffocating(
            &Block::DIRT_PATH,
            Block::DIRT_PATH.default_state,
            true
        ));
        assert!(!is_suffocating(
            &Block::GLASS,
            Block::GLASS.default_state,
            true
        ));
        assert!(!is_suffocating(
            &Block::COPPER_GRATE,
            Block::COPPER_GRATE.default_state,
            true
        ));
        assert!(!is_suffocating(
            &Block::SHULKER_BOX,
            Block::SHULKER_BOX.default_state,
            false
        ));
        assert!(is_suffocating(
            &Block::SHULKER_BOX,
            Block::SHULKER_BOX.default_state,
            true
        ));
    }

    /// Ore experience is data-driven through `Block.experience` and `block::drop_loot`, not a
    /// per-block behaviour: `DropExperienceBlock.spawnAfterBreak` (DropExperienceBlock.java:30-35)
    /// samples the registered range, which `Blocks.java:367-369` gives as `UniformInt.of(0, 2)`
    /// for coal ore and `Blocks.java:1261-1263` as `UniformInt.of(3, 7)` for diamond ore.
    #[test]
    fn ores_carry_their_vanilla_experience_range() {
        use pumpkin_util::random::{RandomGenerator, xoroshiro128::Xoroshiro};

        let mut random = RandomGenerator::Xoroshiro(Xoroshiro::from_seed(1));
        for (block, min, max) in [
            (&Block::COAL_ORE, 0, 2),
            (&Block::DEEPSLATE_COAL_ORE, 0, 2),
            (&Block::DIAMOND_ORE, 3, 7),
            (&Block::EMERALD_ORE, 3, 7),
            (&Block::LAPIS_ORE, 2, 5),
            (&Block::NETHER_QUARTZ_ORE, 2, 5),
            (&Block::NETHER_GOLD_ORE, 0, 1),
            (&Block::IRON_ORE, 0, 0),
            (&Block::GOLD_ORE, 0, 0),
            (&Block::COPPER_ORE, 0, 0),
        ] {
            let experience = block
                .experience
                .as_ref()
                .unwrap_or_else(|| panic!("{} has no experience range", block.name));
            for _ in 0..64 {
                let amount = experience.experience.get(&mut random);
                assert!(
                    (min..=max).contains(&amount),
                    "{} rolled {amount}, outside {min}..={max}",
                    block.name
                );
            }
        }
    }

    #[test]
    fn block_drops_keep_the_pre_removal_state() {
        let original = Block::CANDLE.default_state;

        // Vanilla passes the adjusted pre-removal state to `getDrops`, not the air state written
        // by block removal (`BlockBehaviour.java:272-280`; `ServerPlayerGameMode.java:279-298`).
        assert_eq!(block_drop_state(original.id), original);
        assert_ne!(block_drop_state(original.id), Block::AIR.default_state);
    }
}
