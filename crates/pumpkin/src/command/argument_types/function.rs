use pumpkin_util::identifier::Identifier;

use crate::command::argument_types::FromStringReader;
use crate::command::argument_types::argument_type::{ArgumentType, JavaClientArgumentType};
use crate::command::argument_types::resource_or_tag::ResourceOrTag;
use crate::command::context::command_context::CommandContext;
use crate::command::errors::command_syntax_error::CommandSyntaxError;
use crate::command::string_reader::StringReader;

/// Parses a function id or a `#`-prefixed function tag id (vanilla
/// `FunctionArgument`, `FunctionArgument.java:31-73`).
///
/// The id is not resolved here; like vanilla, a missing function or tag is
/// reported when the command runs.
pub struct FunctionArgumentType;

impl ArgumentType for FunctionArgumentType {
    type Item = ResourceOrTag;

    fn parse(&self, reader: &mut StringReader) -> Result<Self::Item, CommandSyntaxError> {
        // Unlike `ResourceOrTagKeyArgument`, vanilla does not restore the cursor
        // to before the '#' when the tag id is invalid.
        if reader.peek() == Some('#') {
            reader.skip();
            Ok(ResourceOrTag::Tag(Identifier::from_reader(reader)?))
        } else {
            Ok(ResourceOrTag::Resource(Identifier::from_reader(reader)?))
        }
    }

    fn client_side_parser(&'_ self) -> JavaClientArgumentType {
        JavaClientArgumentType::Function
    }

    fn examples(&self) -> Vec<String> {
        examples!("foo", "foo:bar", "#foo")
    }
}

impl FunctionArgumentType {
    /// Returns the parsed function reference from the name of the argument.
    pub fn get<'a>(
        context: &'a CommandContext,
        name: &'_ str,
    ) -> Result<&'a ResourceOrTag, CommandSyntaxError> {
        context.get_argument(name)
    }
}

#[cfg(test)]
mod tests {
    use super::FunctionArgumentType;
    use crate::command::argument_types::argument_type::ArgumentType;
    use crate::command::argument_types::resource_or_tag::ResourceOrTag;
    use crate::command::string_reader::StringReader;

    #[test]
    fn parses_namespaced_ids_and_tags() {
        let mut reader = StringReader::new("foo");
        assert!(matches!(
            FunctionArgumentType.parse(&mut reader),
            Ok(ResourceOrTag::Resource(id)) if id.to_string() == "minecraft:foo"
        ));

        let mut reader = StringReader::new("ns:dir/bar rest");
        assert!(matches!(
            FunctionArgumentType.parse(&mut reader),
            Ok(ResourceOrTag::Resource(id)) if id.to_string() == "ns:dir/bar"
        ));
        assert_eq!(reader.cursor(), "ns:dir/bar".len());

        let mut reader = StringReader::new("#ns:tag");
        assert!(matches!(
            FunctionArgumentType.parse(&mut reader),
            Ok(ResourceOrTag::Tag(id)) if id.to_string() == "ns:tag"
        ));
    }
}
