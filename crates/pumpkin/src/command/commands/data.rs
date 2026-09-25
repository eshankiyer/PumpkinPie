use std::sync::Arc;

// Vanilla DataCommands.java:48-105 defines the entity/block get, merge, and remove command tree.
use crate::block::entities::BlockEntity;
use crate::command::argument_builder::{argument, command, literal};
use crate::command::argument_types::coordinates::block_pos::BlockPosArgumentType;
use crate::command::argument_types::core::double::DoubleArgumentType;
use crate::command::argument_types::entity::EntityArgumentType;
use crate::command::argument_types::nbt::NbtCompoundArgumentType;
use crate::command::argument_types::nbt_path::NbtPath;
use crate::command::context::command_context::CommandContext;
use crate::command::errors::command_syntax_error::CommandSyntaxError;
use crate::command::errors::error_types::CommandErrorType;
use crate::command::node::dispatcher::CommandDispatcher;
use crate::command::node::{CommandExecutor, CommandExecutorResult};
use crate::entity::EntityBase;
use pumpkin_data::translation;
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_nbt::tag::NbtTag;
use pumpkin_util::PermissionLvl;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::permission::{Permission, PermissionDefault, PermissionRegistry};
use pumpkin_util::text::TextComponent;
use pumpkin_util::text::color::NamedColor;

const DESCRIPTION: &str = "Query and modify data of entities and blocks";
const PERMISSION: &str = "minecraft:command.data";
const ARG_ENTITY: &str = "entity";
const ARG_POS: &str = "pos";
const ARG_PATH: &str = "path";
const ARG_SCALE: &str = "scale";
const ARG_NBT: &str = "nbt";

const MERGE_UNCHANGED_ERROR: CommandErrorType<0> = CommandErrorType::new(
    translation::java::COMMANDS_DATA_MERGE_FAILED,
    translation::java::COMMANDS_DATA_MERGE_FAILED,
);
const GET_NOT_NUMBER_ERROR: CommandErrorType<1> = CommandErrorType::new(
    translation::java::COMMANDS_DATA_GET_INVALID,
    translation::java::COMMANDS_DATA_GET_INVALID,
);
const GET_NON_EXISTENT_ERROR: CommandErrorType<1> = CommandErrorType::new(
    translation::java::COMMANDS_DATA_GET_UNKNOWN,
    translation::java::COMMANDS_DATA_GET_UNKNOWN,
);
const MULTIPLE_TAGS_ERROR: CommandErrorType<0> = CommandErrorType::new(
    translation::java::COMMANDS_DATA_GET_MULTIPLE,
    translation::java::COMMANDS_DATA_GET_MULTIPLE,
);
const NO_PLAYERS_ERROR: CommandErrorType<0> = CommandErrorType::new(
    translation::java::COMMANDS_DATA_ENTITY_INVALID,
    translation::java::COMMANDS_DATA_ENTITY_INVALID,
);
const INVALID_BLOCK_ENTITY_ERROR: CommandErrorType<0> = CommandErrorType::new(
    translation::java::COMMANDS_DATA_BLOCK_INVALID,
    translation::java::COMMANDS_DATA_BLOCK_INVALID,
);

#[derive(Clone, Copy)]
enum TargetKind {
    Entity,
    Block,
}

enum DataTarget {
    Entity(Arc<dyn EntityBase>),
    Block {
        entity: Arc<dyn BlockEntity>,
        position: BlockPos,
    },
}

async fn resolve_target(
    context: &CommandContext<'_>,
    target_kind: TargetKind,
) -> Result<DataTarget, CommandSyntaxError> {
    match target_kind {
        TargetKind::Entity => Ok(DataTarget::Entity(
            EntityArgumentType::get_entity(context, ARG_ENTITY).await?,
        )),
        TargetKind::Block => {
            let position = BlockPosArgumentType::get_loaded_block_pos(context, ARG_POS)?;
            let entity = context
                .source
                .world()
                .get_block_entity(&position)
                .ok_or_else(|| INVALID_BLOCK_ENTITY_ERROR.create_without_context())?;
            Ok(DataTarget::Block { entity, position })
        }
    }
}

impl DataTarget {
    // Vanilla EntityDataAccessor.java:51-66 and BlockDataAccessor.java:56-65 expose full NBT.
    async fn get_data(&self) -> NbtCompound {
        let mut data = NbtCompound::new();
        match self {
            Self::Entity(entity) => entity.write_nbt(&mut data).await,
            Self::Block { entity, position } => {
                entity.write_nbt(&mut data).await;
                data.put_string("id", entity.resource_location().to_string());
                data.put_int("x", position.0.x);
                data.put_int("y", position.0.y);
                data.put_int("z", position.0.z);
            }
        }
        data
    }

    // Vanilla EntityDataAccessor.java:40-49 refuses players before loading modified NBT;
    // BlockDataAccessor.java:40-53 applies the result to the existing block entity.
    async fn set_data(
        self,
        context: &CommandContext<'_>,
        mut data: NbtCompound,
    ) -> Result<(), CommandSyntaxError> {
        match self {
            Self::Entity(entity) => {
                if entity.get_player().is_some() {
                    return Err(NO_PLAYERS_ERROR.create_without_context());
                }
                entity.read_nbt_non_mut(&data).await;
            }
            Self::Block { entity, position } => {
                if !data.has("id") {
                    data.put_string("id", entity.resource_location().to_string());
                }
                let replacement = crate::block::entities::block_entity_from_nbt_at(&data, position)
                    .ok_or_else(|| INVALID_BLOCK_ENTITY_ERROR.create_without_context())?;
                context.source.world().add_block_entity(replacement);
            }
        }
        Ok(())
    }

    async fn query_message(&self, tag: &NbtTag) -> Result<TextComponent, CommandSyntaxError> {
        let display = snbt_colorful_display(tag, 0).map_err(|string| {
            CommandSyntaxError::create_without_context(
                &INVALID_BLOCK_ENTITY_ERROR,
                TextComponent::text(string),
            )
        })?;
        Ok(match self {
            Self::Entity(entity) => TextComponent::translate_cross(
                translation::java::COMMANDS_DATA_ENTITY_QUERY,
                translation::java::COMMANDS_DATA_ENTITY_QUERY,
                [entity.get_display_name().await, display],
            ),
            Self::Block { position, .. } => TextComponent::translate_cross(
                translation::java::COMMANDS_DATA_BLOCK_QUERY,
                translation::java::COMMANDS_DATA_BLOCK_QUERY,
                [
                    TextComponent::text(position.0.x.to_string()),
                    TextComponent::text(position.0.y.to_string()),
                    TextComponent::text(position.0.z.to_string()),
                    display,
                ],
            ),
        })
    }

    async fn modified_message(&self) -> TextComponent {
        match self {
            Self::Entity(entity) => TextComponent::translate_cross(
                translation::java::COMMANDS_DATA_ENTITY_MODIFIED,
                translation::java::COMMANDS_DATA_ENTITY_MODIFIED,
                [entity.get_display_name().await],
            ),
            Self::Block { position, .. } => TextComponent::translate_cross(
                translation::java::COMMANDS_DATA_BLOCK_MODIFIED,
                translation::java::COMMANDS_DATA_BLOCK_MODIFIED,
                [
                    TextComponent::text(position.0.x.to_string()),
                    TextComponent::text(position.0.y.to_string()),
                    TextComponent::text(position.0.z.to_string()),
                ],
            ),
        }
    }

    async fn get_message(&self, path: &NbtPath, scale: f64, value: i32) -> TextComponent {
        let scale = TextComponent::text(format!("{scale:.2}"));
        match self {
            Self::Entity(entity) => TextComponent::translate_cross(
                translation::java::COMMANDS_DATA_ENTITY_GET,
                translation::java::COMMANDS_DATA_ENTITY_GET,
                [
                    TextComponent::text(path.as_string().to_owned()),
                    entity.get_display_name().await,
                    scale,
                    TextComponent::text(value.to_string()),
                ],
            ),
            Self::Block { position, .. } => TextComponent::translate_cross(
                translation::java::COMMANDS_DATA_BLOCK_GET,
                translation::java::COMMANDS_DATA_BLOCK_GET,
                [
                    TextComponent::text(path.as_string().to_owned()),
                    TextComponent::text(position.0.x.to_string()),
                    TextComponent::text(position.0.y.to_string()),
                    TextComponent::text(position.0.z.to_string()),
                    scale,
                    TextComponent::text(value.to_string()),
                ],
            ),
        }
    }
}

#[expect(clippy::too_many_lines)]
pub fn snbt_colorful_display(tag: &NbtTag, depth: usize) -> Result<TextComponent, String> {
    let folded = TextComponent::text("<...>").color_named(NamedColor::Gray);
    match tag {
        NbtTag::End => Err("Unexpected end tag".into()),
        NbtTag::Byte(value) => {
            let byte_format = TextComponent::text("b").color_named(NamedColor::Red);
            Ok(TextComponent::text(format!("{value}"))
                .color_named(NamedColor::Gold)
                .add_child(byte_format))
        }
        NbtTag::Short(value) => {
            let short_format = TextComponent::text("s").color_named(NamedColor::Red);
            Ok(TextComponent::text(format!("{value}"))
                .color_named(NamedColor::Gold)
                .add_child(short_format))
        }
        NbtTag::Int(value) => {
            Ok(TextComponent::text(format!("{value}")).color_named(NamedColor::Gold))
        }
        NbtTag::Long(value) => {
            let long_format = TextComponent::text("L").color_named(NamedColor::Red);
            Ok(TextComponent::text(format!("{value}"))
                .color_named(NamedColor::Gold)
                .add_child(long_format))
        }
        NbtTag::Float(value) => {
            let float_format = TextComponent::text("f").color_named(NamedColor::Red);
            Ok(TextComponent::text(format!("{value}"))
                .color_named(NamedColor::Gold)
                .add_child(float_format))
        }
        NbtTag::Double(value) => {
            let double_format = TextComponent::text("d").color_named(NamedColor::Red);
            Ok(TextComponent::text(format!("{value}"))
                .color_named(NamedColor::Gold)
                .add_child(double_format))
        }
        NbtTag::ByteArray(value) => {
            let byte_array_format = TextComponent::text("B").color_named(NamedColor::Red);
            let mut content = TextComponent::text("[")
                .add_child(byte_array_format.clone())
                .add_child(TextComponent::text("; "));

            for (index, byte) in value.iter().take(128).enumerate() {
                content = content
                    .add_child(TextComponent::text(format!("{byte}")))
                    .add_child(byte_array_format.clone());
                if index < value.len() - 1 {
                    content = content.add_child(TextComponent::text(", "));
                }
            }

            if value.len() > 128 {
                content = content.add_child(folded);
            }

            content = content.add_child(TextComponent::text("]"));
            Ok(content)
        }
        NbtTag::String(value) => {
            let escaped_value = value
                .replace('"', "\\\"")
                .replace('\\', "\\\\")
                .replace('\n', "\\n")
                .replace('\t', "\\t")
                .replace('\r', "\\r")
                .replace('\x0c', "\\f")
                .replace('\x08', "\\b");

            Ok(TextComponent::text(format!("\"{escaped_value}\"")).color_named(NamedColor::Green))
        }
        NbtTag::List(value) => {
            if value.is_empty() {
                Ok(TextComponent::text("[]"))
            } else if depth >= 64 {
                Ok(TextComponent::text("[")
                    .add_child(folded)
                    .add_child(TextComponent::text("]")))
            } else {
                let mut content = TextComponent::text("[");

                for (index, item) in value.iter().take(128).enumerate() {
                    let item_display = snbt_colorful_display(item, depth + 1)
                        .map_err(|string| format!("Error displaying item.[{index}]: {string}"))?;
                    content = content.add_child(item_display);

                    if index < value.len() - 1 {
                        content = content.add_child(TextComponent::text(", "));
                    }
                }

                if value.len() > 128 {
                    content = content.add_child(folded);
                }

                content = content.add_child(TextComponent::text("]"));
                Ok(content)
            }
        }
        NbtTag::Compound(value) => {
            if value.is_empty() {
                Ok(TextComponent::text("{}"))
            } else if depth >= 64 {
                Ok(TextComponent::text("{")
                    .add_child(folded)
                    .add_child(TextComponent::text("}")))
            } else {
                let mut content = TextComponent::text("{");

                for (index, (key, item)) in value.child_tags.iter().take(128).enumerate() {
                    let item_display = snbt_colorful_display(item, depth + 1)
                        .map_err(|string| format!("Error displaying item.{key}: {string}"))?;
                    content = content
                        .add_child(
                            TextComponent::text(key.to_string()).color_named(NamedColor::Aqua),
                        )
                        .add_child(TextComponent::text(": "))
                        .add_child(item_display);

                    if index < value.child_tags.len() - 1 {
                        content = content.add_child(TextComponent::text(", "));
                    }
                }

                if value.child_tags.len() > 128 {
                    content = content.add_child(folded);
                }

                content = content.add_child(TextComponent::text("}"));
                Ok(content)
            }
        }
        NbtTag::IntArray(value) => {
            let int_array_format = TextComponent::text("I").color_named(NamedColor::Red);
            let mut content = TextComponent::text("[")
                .add_child(int_array_format)
                .add_child(TextComponent::text("; "));

            for (index, int) in value.iter().take(128).enumerate() {
                content = content
                    .add_child(TextComponent::text(format!("{int}")).color_named(NamedColor::Gold));
                if index < value.len() - 1 {
                    content = content.add_child(TextComponent::text(", "));
                }
            }

            if value.len() > 128 {
                content = content.add_child(folded);
            }

            content = content.add_child(TextComponent::text("]"));
            Ok(content)
        }
        NbtTag::LongArray(value) => {
            let long_array_format = TextComponent::text("L").color_named(NamedColor::Red);
            let mut content = TextComponent::text("[")
                .add_child(long_array_format.clone())
                .add_child(TextComponent::text("; "));

            for (index, long) in value.iter().take(128).enumerate() {
                content = content
                    .add_child(TextComponent::text(format!("{long}")))
                    .add_child(long_array_format.clone());
                if index < value.len() - 1 {
                    content = content.add_child(TextComponent::text(", "));
                }
            }

            if value.len() > 128 {
                content = content.add_child(folded);
            }

            content = content.add_child(TextComponent::text("]"));
            Ok(content)
        }
    }
}

struct GetExecutor {
    target_kind: TargetKind,
    has_path: bool,
    has_scale: bool,
}

impl CommandExecutor for GetExecutor {
    // Vanilla DataCommands.java:330-369 defines whole-tag, path, and scaled numeric get results.
    fn execute<'a>(&'a self, context: &'a CommandContext) -> CommandExecutorResult<'a> {
        Box::pin(async move {
            let target = resolve_target(context, self.target_kind).await?;
            let data = target.get_data().await;
            if !self.has_path {
                let root = NbtTag::Compound(data);
                context
                    .source
                    .send_feedback(target.query_message(&root).await?, false)
                    .await;
                return Ok(1);
            }

            let path = context.get_argument::<NbtPath>(ARG_PATH)?;
            let root = NbtTag::Compound(data);
            let tag = get_single_tag(path, &root)?;
            let value = if self.has_scale {
                let scale = DoubleArgumentType::get(context, ARG_SCALE)?;
                let (NbtTag::Byte(_)
                | NbtTag::Short(_)
                | NbtTag::Int(_)
                | NbtTag::Long(_)
                | NbtTag::Float(_)
                | NbtTag::Double(_)) = tag
                else {
                    return Err(GET_NOT_NUMBER_ERROR
                        .create_without_context(TextComponent::text(path.as_string().to_owned())));
                };
                let numeric = match tag {
                    NbtTag::Byte(value) => f64::from(*value),
                    NbtTag::Short(value) => f64::from(*value),
                    NbtTag::Int(value) => f64::from(*value),
                    NbtTag::Long(value) => *value as f64,
                    NbtTag::Float(value) => f64::from(*value),
                    NbtTag::Double(value) => *value,
                    _ => unreachable!(),
                };
                let value = (numeric * scale).floor() as i32;
                context
                    .source
                    .send_feedback(target.get_message(path, scale, value).await, false)
                    .await;
                return Ok(value);
            } else {
                tag_result(tag, path)?
            };
            context
                .source
                .send_feedback(target.query_message(value).await?, false)
                .await;
            Ok(value_result(value))
        })
    }
}

struct MergeExecutor {
    target_kind: TargetKind,
}

impl CommandExecutor for MergeExecutor {
    // Vanilla DataCommands.java:374-386 copies, merges, depth-checks, and rejects unchanged data.
    fn execute<'a>(&'a self, context: &'a CommandContext) -> CommandExecutorResult<'a> {
        Box::pin(async move {
            let target = resolve_target(context, self.target_kind).await?;
            let nbt = context.get_argument::<NbtCompound>(ARG_NBT)?.clone();
            if NbtPath::is_too_deep(&NbtTag::Compound(nbt.clone()), 0) {
                return Err(
                    crate::command::argument_types::nbt_path::DATA_TOO_DEEP_ERROR
                        .create_without_context(),
                );
            }
            let old = target.get_data().await;
            let mut merged = old.clone();
            merged.merge(&nbt);
            if old == merged {
                return Err(MERGE_UNCHANGED_ERROR.create_without_context());
            }
            target.set_data(context, merged).await?;
            context
                .source
                .send_feedback(target.modified_message().await, true)
                .await;
            Ok(1)
        })
    }
}

struct RemoveExecutor {
    target_kind: TargetKind,
}

impl CommandExecutor for RemoveExecutor {
    // Vanilla DataCommands.java:318-328 removes matching path nodes and rejects a zero count.
    fn execute<'a>(&'a self, context: &'a CommandContext) -> CommandExecutorResult<'a> {
        Box::pin(async move {
            let target = resolve_target(context, self.target_kind).await?;
            let path = context.get_argument::<NbtPath>(ARG_PATH)?;
            let data = target.get_data().await;
            let mut root = NbtTag::Compound(data);
            let count = path.remove(&mut root);
            if count == 0 {
                return Err(MERGE_UNCHANGED_ERROR.create_without_context());
            }
            let NbtTag::Compound(data) = root else {
                unreachable!()
            };
            target.set_data(context, data).await?;
            context
                .source
                .send_feedback(target.modified_message().await, true)
                .await;
            Ok(count)
        })
    }
}

fn get_single_tag<'a>(path: &NbtPath, root: &'a NbtTag) -> Result<&'a NbtTag, CommandSyntaxError> {
    let tags = path.get(root)?;
    let Some(tag) = tags.first() else {
        return Err(GET_NON_EXISTENT_ERROR
            .create_without_context(TextComponent::text(path.as_string().to_owned())));
    };
    if tags.len() > 1 {
        return Err(MULTIPLE_TAGS_ERROR.create_without_context());
    }
    Ok(tag)
}

fn tag_result<'a>(tag: &'a NbtTag, path: &NbtPath) -> Result<&'a NbtTag, CommandSyntaxError> {
    if matches!(tag, NbtTag::End) {
        Err(GET_NON_EXISTENT_ERROR
            .create_without_context(TextComponent::text(path.as_string().to_owned())))
    } else {
        Ok(tag)
    }
}

fn value_result(tag: &NbtTag) -> i32 {
    match tag {
        NbtTag::Byte(value) => i32::from(*value),
        NbtTag::Short(value) => i32::from(*value),
        NbtTag::Int(value) => *value,
        NbtTag::Long(value) => (*value).clamp(i32::MIN as i64, i32::MAX as i64) as i32,
        NbtTag::Float(value) => value.floor() as i32,
        NbtTag::Double(value) => value.floor() as i32,
        NbtTag::ByteArray(value) => value.len() as i32,
        NbtTag::IntArray(value) => value.len() as i32,
        NbtTag::LongArray(value) => value.len() as i32,
        NbtTag::String(value) => value.len() as i32,
        NbtTag::List(value) => value.len() as i32,
        NbtTag::Compound(value) => value.child_tags.len() as i32,
        NbtTag::End => 0,
    }
}

pub fn register(dispatcher: &mut CommandDispatcher, registry: &PermissionRegistry) {
    registry.register_permission_or_panic(Permission::new(
        PERMISSION,
        DESCRIPTION,
        PermissionDefault::Op(PermissionLvl::Two),
    ));

    let entity_path = || crate::command::argument_types::nbt_path::NbtPathArgumentType;
    let entity_get = literal("entity").then(
        argument(ARG_ENTITY, EntityArgumentType::Entity)
            .executes(GetExecutor {
                target_kind: TargetKind::Entity,
                has_path: false,
                has_scale: false,
            })
            .then(
                argument(ARG_PATH, entity_path())
                    .executes(GetExecutor {
                        target_kind: TargetKind::Entity,
                        has_path: true,
                        has_scale: false,
                    })
                    .then(
                        argument(ARG_SCALE, DoubleArgumentType::any()).executes(GetExecutor {
                            target_kind: TargetKind::Entity,
                            has_path: true,
                            has_scale: true,
                        }),
                    ),
            ),
    );
    let block_get = literal("block").then(
        argument(ARG_POS, BlockPosArgumentType)
            .executes(GetExecutor {
                target_kind: TargetKind::Block,
                has_path: false,
                has_scale: false,
            })
            .then(
                argument(ARG_PATH, entity_path())
                    .executes(GetExecutor {
                        target_kind: TargetKind::Block,
                        has_path: true,
                        has_scale: false,
                    })
                    .then(
                        argument(ARG_SCALE, DoubleArgumentType::any()).executes(GetExecutor {
                            target_kind: TargetKind::Block,
                            has_path: true,
                            has_scale: true,
                        }),
                    ),
            ),
    );
    let merge = literal("merge")
        .then(
            literal("entity").then(argument(ARG_ENTITY, EntityArgumentType::Entity).then(
                argument(ARG_NBT, NbtCompoundArgumentType).executes(MergeExecutor {
                    target_kind: TargetKind::Entity,
                }),
            )),
        )
        .then(
            literal("block").then(argument(ARG_POS, BlockPosArgumentType).then(
                argument(ARG_NBT, NbtCompoundArgumentType).executes(MergeExecutor {
                    target_kind: TargetKind::Block,
                }),
            )),
        );
    let remove = literal("remove")
        .then(
            literal("entity").then(argument(ARG_ENTITY, EntityArgumentType::Entity).then(
                argument(ARG_PATH, entity_path()).executes(RemoveExecutor {
                    target_kind: TargetKind::Entity,
                }),
            )),
        )
        .then(
            literal("block").then(argument(ARG_POS, BlockPosArgumentType).then(
                argument(ARG_PATH, entity_path()).executes(RemoveExecutor {
                    target_kind: TargetKind::Block,
                }),
            )),
        );

    dispatcher.register(
        command("data", DESCRIPTION)
            .requires(PERMISSION)
            .then(literal("get").then(entity_get).then(block_get))
            .then(merge)
            .then(remove),
    );
}
