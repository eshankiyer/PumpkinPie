use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::{
    data_component::DataComponent,
    item::Item,
    tag::{RegistryKey, get_tag_ids},
};
use pumpkin_protocol::java::client::play::{ArgumentType, SuggestionProviders};
use pumpkin_util::text::TextComponent;

use crate::command::{
    CommandSender,
    args::{
        Arg, ArgumentConsumer, ConsumeResult, ConsumedArgs, DefaultNameArgConsumer, FindArg,
        GetClientSideArgParser,
    },
    dispatcher::CommandError,
    tree::RawArgs,
};
use crate::server::Server;

pub struct ItemArgumentConsumer;

fn malformed_item_error(error: impl Into<String>) -> CommandError {
    CommandError::CommandFailed(TextComponent::translate_cross(
        "arguments.item.malformed",
        "arguments.item.malformed",
        [TextComponent::text(error.into())],
    ))
}

impl ItemArgumentConsumer {
    /// Vanilla `ItemInput.createItemStack` (`ItemInput.java:21-28`) constructs the
    /// requested count, rejects overstacked results, and validates component data.
    pub fn create_item_stack(stack: &ItemStack, count: i32) -> Result<ItemStack, CommandError> {
        let max_stack_size = i32::from(stack.get_max_stack_size());
        if count > max_stack_size {
            return Err(CommandError::CommandFailed(TextComponent::translate_cross(
                "arguments.item.overstacked",
                "arguments.item.overstacked",
                [
                    TextComponent::text(stack.item.registry_key.to_string()),
                    TextComponent::text(max_stack_size.to_string()),
                ],
            )));
        }

        let mut result = stack.clone();
        result.item_count = u8::try_from(count)
            .map_err(|_| malformed_item_error(format!("invalid item count {count}")))?;
        Ok(result)
    }
}

impl GetClientSideArgParser for ItemArgumentConsumer {
    fn get_client_side_parser(&self) -> ArgumentType {
        ArgumentType::ItemStack
    }

    fn get_client_side_suggestion_type_override(&self) -> Option<SuggestionProviders> {
        None
    }
}

impl ArgumentConsumer for ItemArgumentConsumer {
    fn consume<'a>(
        &'a self,
        _sender: &'a CommandSender,
        _server: &'a Server,
        args: &mut RawArgs<'a>,
    ) -> ConsumeResult<'a> {
        let item = args.pop().map(|arg| arg.value);
        // TODO: When supporting data components in this argument, do it for ItemPredicateArgumentConsumer as well (both tags and items)
        match item {
            Some(s) => Box::pin(async move { Some(Arg::Item(s)) }),
            None => Box::pin(async move { None }),
        }
    }
}

impl DefaultNameArgConsumer for ItemArgumentConsumer {
    fn default_name(&self) -> &'static str {
        "item"
    }
}

impl<'a> FindArg<'a> for ItemArgumentConsumer {
    type Data = (&'a str, ItemStack);

    fn find_arg(args: &'a ConsumedArgs, name: &str) -> Result<Self::Data, CommandError> {
        match args.get(name) {
            Some(Arg::Item(raw_name)) => {
                let item_id = raw_name.split('[').next().unwrap_or(raw_name);
                let item = Item::from_registry_key(item_id).ok_or_else(|| {
                    if item_id.starts_with("minecraft:") {
                        CommandError::CommandFailed(TextComponent::translate_cross(
                            "argument.item.id.invalid",
                            "argument.item.id.invalid",
                            [TextComponent::text((*item_id).to_string())],
                        ))
                    } else {
                        CommandError::CommandFailed(TextComponent::translate_cross(
                            "argument.item.id.invalid",
                            "argument.item.id.invalid",
                            [TextComponent::text("minecraft:".to_string() + item_id)],
                        ))
                    }
                })?;

                let mut patch = Vec::new();
                // Parse optional components inside brackets `[...]`
                if let Some(start_idx) = raw_name.find('[') {
                    if !raw_name.ends_with(']') {
                        return Err(malformed_item_error(*raw_name));
                    }
                    let inner = &raw_name[start_idx + 1..raw_name.len() - 1];
                    // Split components by comma, but we must ignore commas inside curly braces/brackets!
                    let mut components = Vec::new();
                    let mut filter_start = 0usize;
                    let mut curly_depth = 0;
                    let mut bracket_depth = 0;
                    for (i, c) in inner.char_indices() {
                        if c == '{' {
                            curly_depth += 1;
                        } else if c == '}' {
                            curly_depth -= 1;
                        } else if c == '[' {
                            bracket_depth += 1;
                        } else if c == ']' {
                            bracket_depth -= 1;
                        } else if c == ',' && curly_depth == 0 && bracket_depth == 0 {
                            components.push(&inner[filter_start..i]);
                            filter_start = i + 1;
                        }
                    }
                    components.push(&inner[filter_start..]);

                    for comp in components {
                        let comp = comp.trim();
                        if comp.is_empty() {
                            continue;
                        }
                        let mut parts = comp.splitn(2, '=');
                        let comp_key = parts
                            .next()
                            .map(str::trim)
                            .filter(|key| !key.is_empty())
                            .ok_or_else(|| malformed_item_error(comp))?;
                        let comp_val_str = parts
                            .next()
                            .map(str::trim)
                            .filter(|value| !value.is_empty())
                            .ok_or_else(|| malformed_item_error(comp))?;
                        // Parse value string as SNBT.
                        let mut reader =
                            crate::command::string_reader::StringReader::new(comp_val_str);
                        let data_comp = DataComponent::try_from_name(comp_key)
                            .ok_or_else(|| malformed_item_error(comp_key))?;
                        let nbt_tag =
                            crate::command::snbt::SnbtParser::parse_for_commands(&mut reader)
                                .map_err(|_| malformed_item_error(comp_val_str))?;
                        if reader.remaining_length() != 0 {
                            return Err(malformed_item_error(comp_val_str));
                        }
                        let comp_impl =
                            pumpkin_data::data_component_impl::read_data(data_comp, &nbt_tag)
                                .ok_or_else(|| malformed_item_error(comp_val_str))?;
                        patch.push((data_comp, Some(comp_impl)));
                    }
                }

                let mut stack = ItemStack::new(1, item);
                if !patch.is_empty() {
                    stack.patch = patch;
                }
                Ok((*raw_name, stack))
            }
            _ => Err(CommandError::InvalidConsumption(Some(name.to_string()))),
        }
    }
}

pub struct ItemPredicateArgumentConsumer;

pub enum ItemPredicate {
    Item(&'static Item),
    Tag(Vec<u16>),
    Any,
}

impl ItemPredicate {
    #[must_use]
    pub fn test_item_stack(&self, stack: &ItemStack) -> bool {
        match self {
            Self::Any => true,
            Self::Item(item) => stack.get_item() == *item,
            Self::Tag(tag) => tag.contains(&stack.get_item().id),
        }
    }
}

impl GetClientSideArgParser for ItemPredicateArgumentConsumer {
    fn get_client_side_parser(&self) -> ArgumentType {
        ArgumentType::ItemPredicate
    }

    fn get_client_side_suggestion_type_override(&self) -> Option<SuggestionProviders> {
        None
    }
}

impl ArgumentConsumer for ItemPredicateArgumentConsumer {
    fn consume<'a>(
        &'a self,
        _sender: &'a CommandSender,
        _server: &'a Server,
        args: &mut RawArgs<'a>,
    ) -> ConsumeResult<'a> {
        let item = args.pop().map(|arg| arg.value);
        match item {
            Some(s) => Box::pin(async move { Some(Arg::Item(s)) }),
            None => Box::pin(async move { None }),
        }
    }
}

impl DefaultNameArgConsumer for ItemPredicateArgumentConsumer {
    fn default_name(&self) -> &'static str {
        "item"
    }
}

impl<'a> FindArg<'a> for ItemPredicateArgumentConsumer {
    type Data = ItemPredicate;

    fn find_arg(args: &'a ConsumedArgs, name: &str) -> Result<Self::Data, CommandError> {
        match args.get(name) {
            Some(Arg::Item(raw_name)) => {
                if *raw_name == "*" {
                    return Ok(ItemPredicate::Any);
                }
                let clean_name = raw_name.split('[').next().unwrap_or(raw_name);
                clean_name.strip_prefix("#").map_or_else(
                    || {
                        Item::from_registry_key(clean_name).map_or_else(
                            || {
                                if clean_name.starts_with("minecraft:") {
                                    Err(CommandError::CommandFailed(
                                        TextComponent::translate_cross(
                                            "argument.item.id.invalid",
                                            "argument.item.id.invalid",
                                            [TextComponent::text((*clean_name).to_string())],
                                        ),
                                    ))
                                } else {
                                    Err(CommandError::CommandFailed(
                                        TextComponent::translate_cross(
                                            "argument.item.id.invalid",
                                            "argument.item.id.invalid",
                                            [TextComponent::text(
                                                "minecraft:".to_string() + clean_name,
                                            )],
                                        ),
                                    ))
                                }
                            },
                            |item| Ok(ItemPredicate::Item(item)),
                        )
                    },
                    |tag| {
                        get_tag_ids(RegistryKey::Item, tag).map_or_else(
                            || {
                                Err(CommandError::CommandFailed(TextComponent::translate_cross(
                                    "arguments.item.tag.unknown",
                                    "arguments.item.tag.unknown",
                                    [TextComponent::text((*tag).to_string())],
                                )))
                            },
                            |items| Ok(ItemPredicate::Tag(items.to_vec())),
                        )
                    },
                )
            }
            _ => Err(CommandError::InvalidConsumption(Some(name.to_string()))),
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn parse_plain_item() {
        let mut args = HashMap::new();
        args.insert("item", Arg::Item("minecraft:stick"));

        let res = ItemArgumentConsumer::find_arg(&args, "item");
        assert!(res.is_ok());
        let (name, stack) = res.unwrap();
        assert_eq!(name, "minecraft:stick");
        assert_eq!(stack.item.registry_key, "stick");
        assert!(stack.patch.is_empty());
    }

    #[test]
    fn parse_item_with_profile_component() {
        let mut args = HashMap::new();
        args.insert(
            "item",
            Arg::Item("player_head[minecraft:profile={name:\"Username\"}]"),
        );

        let res = ItemArgumentConsumer::find_arg(&args, "item");
        assert!(res.is_ok());
        let (name, stack) = res.unwrap();
        assert_eq!(name, "player_head[minecraft:profile={name:\"Username\"}]");
        assert_eq!(stack.item.registry_key, "player_head");
        assert_eq!(stack.patch.len(), 1);
        assert_eq!(stack.patch[0].0.to_id(), DataComponent::Profile.to_id());
    }

    #[test]
    fn parse_item_with_custom_name() {
        let mut args = HashMap::new();
        args.insert(
            "item",
            Arg::Item("stick[minecraft:custom_name=\"Magic wand\"]"),
        );

        let res = ItemArgumentConsumer::find_arg(&args, "item");
        assert!(res.is_ok());
        let (name, stack) = res.unwrap();
        assert_eq!(name, "stick[minecraft:custom_name=\"Magic wand\"]");
        assert_eq!(stack.item.registry_key, "stick");
        assert_eq!(stack.patch.len(), 1);
        assert_eq!(stack.patch[0].0.to_id(), DataComponent::CustomName.to_id());
    }

    #[test]
    fn reject_overstacked_item_input() {
        let stack = ItemStack::new(1, &Item::ENDER_PEARL);
        assert!(ItemArgumentConsumer::create_item_stack(&stack, 17).is_err());
        assert!(ItemArgumentConsumer::create_item_stack(&stack, 16).is_ok());
    }

    #[test]
    fn reject_malformed_item_component() {
        let mut args = HashMap::new();
        args.insert("item", Arg::Item("stick[minecraft:custom_name]"));
        assert!(ItemArgumentConsumer::find_arg(&args, "item").is_err());
    }
}
