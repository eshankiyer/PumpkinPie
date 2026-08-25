use pumpkin_data::entity::MobCategory;
use pumpkin_util::permission::{Permission, PermissionDefault, PermissionRegistry};

use crate::command::argument_builder::{ArgumentBuilder, argument, command, literal};
use crate::command::argument_types::coordinates::block_pos::BlockPosArgumentType;
use crate::command::context::command_context::CommandContext;
use crate::command::node::dispatcher::CommandDispatcher;
use crate::command::node::{CommandExecutor, CommandExecutorResult};
use crate::world::natural_spawner::spawn_category_for_position;

const DESCRIPTION: &str = "Spawns mobs from a category at a position.";
const PERMISSION: &str = "minecraft:command.debugmobspawning";
const ARG_POSITION: &str = "at";

struct SpawnMobsExecutor {
    category: &'static MobCategory,
}

impl CommandExecutor for SpawnMobsExecutor {
    /// Implements `DebugMobSpawningCommand.spawnMobs` (`DebugMobSpawningCommand.java:29-31`).
    fn execute<'a>(&'a self, context: &'a CommandContext) -> CommandExecutorResult<'a> {
        Box::pin(async move {
            let position = BlockPosArgumentType::get_loaded_block_pos(context, ARG_POSITION)?;
            let world = context.world();
            let chunk_position = position.chunk_position();
            let is_thundering = world.is_thundering().await;
            let spawn_state = world.spawn_state.load();
            let entities = spawn_category_for_position(
                self.category,
                world,
                position,
                &chunk_position,
                &spawn_state,
                is_thundering,
            );

            for entity in entities {
                world.spawn_entity(entity).await;
            }

            Ok(1)
        })
    }
}

/// Registers `/debugmobspawning`, matching `DebugMobSpawningCommand.java:13-27`.
pub fn register(dispatcher: &mut CommandDispatcher, registry: &PermissionRegistry) {
    registry.register_permission_or_panic(Permission::new(
        PERMISSION,
        DESCRIPTION,
        PermissionDefault::Op(pumpkin_util::PermissionLvl::Two),
    ));

    let categories = [
        ("monster", &MobCategory::MONSTER),
        ("creature", &MobCategory::CREATURE),
        ("ambient", &MobCategory::AMBIENT),
        ("axolotls", &MobCategory::AXOLOTLS),
        (
            "underground_water_creature",
            &MobCategory::UNDERGROUND_WATER_CREATURE,
        ),
        ("water_creature", &MobCategory::WATER_CREATURE),
        ("water_ambient", &MobCategory::WATER_AMBIENT),
        ("misc", &MobCategory::MISC),
    ];

    let mut root = command("debugmobspawning", DESCRIPTION).requires(PERMISSION);
    for (name, category) in categories {
        root = root.then(literal(name).then(
            argument(ARG_POSITION, BlockPosArgumentType).executes(SpawnMobsExecutor { category }),
        ));
    }
    dispatcher.register(root);
}
