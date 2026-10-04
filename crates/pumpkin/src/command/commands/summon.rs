use std::sync::Arc;

use pumpkin_data::entity::EntityType;
use pumpkin_data::translation;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_util::permission::{Permission, PermissionDefault, PermissionRegistry};
use pumpkin_util::text::TextComponent;
use pumpkin_util::{Difficulty, PermissionLvl};
use uuid::Uuid;

use crate::command::argument_builder::{ArgumentBuilder, argument, command};
use crate::command::argument_types::coordinates::vec3::Vec3ArgumentType;
use crate::command::argument_types::nbt::NbtCompoundArgumentType;
use crate::command::argument_types::resource::{ENTITY_TYPE_ARGUMENT, ResourceArgument};
use crate::command::context::command_context::CommandContext;
use crate::command::context::command_source::CommandSource;
use crate::command::errors::command_syntax_error::CommandSyntaxError;
use crate::command::errors::error_types::CommandErrorType;
use crate::command::node::dispatcher::CommandDispatcher;
use crate::command::node::{CommandExecutor, CommandExecutorResult};
use crate::entity::{EntityBase, NBTStorage};
use crate::entity::r#type::from_type;
use crate::world::World;

const DESCRIPTION: &str = "Summons an entity.";
const PERMISSION: &str = "minecraft:command.summon";

const ARG_ENTITY: &str = "entity";
const ARG_POS: &str = "pos";
const ARG_NBT: &str = "nbt";

static INVALID_POSITION: CommandErrorType<0> = CommandErrorType::new(
    translation::java::COMMANDS_SUMMON_INVALIDPOSITION,
    translation::java::COMMANDS_SUMMON_INVALIDPOSITION,
);
static FAILED_PEACEFUL: CommandErrorType<0> = CommandErrorType::new(
    translation::java::COMMANDS_SUMMON_FAILED_PEACEFUL,
    translation::java::COMMANDS_SUMMON_FAILED_PEACEFUL,
);
static FAILED_UUID: CommandErrorType<0> = CommandErrorType::new(
    translation::java::COMMANDS_SUMMON_FAILED_UUID,
    translation::java::COMMANDS_SUMMON_FAILED_UUID,
);

/// Vanilla `SummonCommand.createEntity` (`SummonCommand.java:72-104`), shared with
/// `execute summon`.
///
/// `nbt` is [`None`] for the forms vanilla runs with `finalize = true`. Reading NBT marks the
/// entity as restored, which suppresses the `finalizeSpawn` roll exactly as vanilla's
/// `finalize = false` does for the nbt form. Passengers are not loaded.
pub async fn create_entity(
    source: &CommandSource,
    entity_type: &'static EntityType,
    pos: Vector3<f64>,
    nbt: Option<&NbtCompound>,
) -> Result<Arc<dyn EntityBase>, CommandSyntaxError> {
    if !World::is_valid(BlockPos::floored_v(pos)) {
        return Err(INVALID_POSITION.create_without_context());
    }

    let world = source.world();
    if world.level_info.load().difficulty == Difficulty::Peaceful
        && !entity_type.allowed_in_peaceful
    {
        return Err(FAILED_PEACEFUL.create_without_context());
    }

    let uuid = nbt
        .and_then(|nbt| nbt.get_uuid("UUID"))
        .unwrap_or_else(Uuid::new_v4);
    let entity = from_type(entity_type, pos, world, uuid);
    if let Some(nbt) = nbt {
        // Same load order as the chunk loader: base data first, then the type's own.
        if let Some(living) = entity.get_living_entity() {
            living.read_nbt_non_mut(nbt).await;
        } else {
            entity.get_entity().read_nbt_non_mut(nbt).await;
        }
        entity.read_nbt_non_mut(nbt).await;
        // `LivingEntity.readAdditionalSaveData`: `Health` defaults to `getMaxHealth()` (after
        // attributes are loaded), whereas the shared reader defaults it to 0 for saved data.
        if nbt.get_float("Health").is_none()
            && let Some(living) = entity.get_living_entity()
        {
            living.health.store(living.get_max_health());
        }
        // `snapTo(pos, yRot, xRot)`: the command position wins over NBT `Pos`, rotation is kept.
        entity.get_entity().set_pos(pos);
    }

    // `tryAddFreshEntityWithPassengers` refuses an entity whose UUID is already in use.
    if world.get_entity_by_uuid(uuid).is_some() || world.get_player_by_uuid(uuid).is_some() {
        return Err(FAILED_UUID.create_without_context());
    }
    world.spawn_entity(entity.clone()).await;
    Ok(entity)
}

struct SummonExecutor {
    has_pos: bool,
    has_nbt: bool,
}

impl CommandExecutor for SummonExecutor {
    fn execute<'a>(&'a self, context: &'a CommandContext) -> CommandExecutorResult<'a> {
        Box::pin(async move {
            let entity_type = ResourceArgument::get_summonable_entity_type(context, ARG_ENTITY)?;
            let pos = if self.has_pos {
                Vec3ArgumentType::get_vector3(context, ARG_POS)?
            } else {
                context.source.position
            };
            let nbt = if self.has_nbt {
                Some(NbtCompoundArgumentType::get(context, ARG_NBT)?)
            } else {
                None
            };

            let entity = create_entity(&context.source, entity_type, pos, nbt).await?;
            context
                .source
                .send_feedback(
                    TextComponent::translate_cross(
                        translation::java::COMMANDS_SUMMON_SUCCESS,
                        translation::bedrock::COMMANDS_SUMMON_SUCCESS,
                        [entity.get_display_name().await],
                    ),
                    true,
                )
                .await;

            Ok(1)
        })
    }
}

pub fn register(dispatcher: &mut CommandDispatcher, registry: &PermissionRegistry) {
    registry.register_permission_or_panic(Permission::new(
        PERMISSION,
        DESCRIPTION,
        PermissionDefault::Op(PermissionLvl::Two),
    ));

    dispatcher.register(
        command("summon", DESCRIPTION).requires(PERMISSION).then(
            argument(ARG_ENTITY, ENTITY_TYPE_ARGUMENT.clone())
                .executes(SummonExecutor {
                    has_pos: false,
                    has_nbt: false,
                })
                .then(
                    argument(ARG_POS, Vec3ArgumentType::Default)
                        .executes(SummonExecutor {
                            has_pos: true,
                            has_nbt: false,
                        })
                        .then(argument(ARG_NBT, NbtCompoundArgumentType).executes(
                            SummonExecutor {
                                has_pos: true,
                                has_nbt: true,
                            },
                        )),
                ),
        ),
    );
}
