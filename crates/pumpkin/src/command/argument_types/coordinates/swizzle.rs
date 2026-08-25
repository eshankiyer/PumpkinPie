use std::pin::Pin;

use pumpkin_data::translation;
use pumpkin_util::math::vector3::Axis;

use crate::command::{
    argument_types::argument_type::{ArgumentType, JavaClientArgumentType},
    context::command_context::CommandContext,
    errors::{command_syntax_error::CommandSyntaxError, error_types::CommandErrorType},
    string_reader::StringReader,
    suggestion::suggestions::{Suggestions, SuggestionsBuilder},
};

pub const INVALID_ERROR_TYPE: CommandErrorType<0> = CommandErrorType::new(
    translation::java::ARGUMENTS_SWIZZLE_INVALID,
    translation::java::ARGUMENTS_SWIZZLE_INVALID,
);

/// The set of coordinate axes selected by a parsed swizzle argument.
///
/// Mirrors vanilla's `EnumSet<Direction.Axis>` result
/// (`net/minecraft/commands/arguments/coordinates/SwizzleArgument.java:15`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Axes {
    pub x: bool,
    pub y: bool,
    pub z: bool,
}

impl Axes {
    /// Returns whether the given [`Axis`] is part of this set.
    #[must_use]
    pub const fn contains(&self, axis: Axis) -> bool {
        match axis {
            Axis::X => self.x,
            Axis::Y => self.y,
            Axis::Z => self.z,
        }
    }

    const fn with_axis(mut self, axis: Axis) -> Self {
        match axis {
            Axis::X => self.x = true,
            Axis::Y => self.y = true,
            Axis::Z => self.z = true,
        }
        self
    }
}

/// An argument type parsing a combination of axis letters (`x`, `y`, `z`), e.g. `xyz`.
///
/// Ported from vanilla `SwizzleArgument`
/// (`net/minecraft/commands/arguments/coordinates/SwizzleArgument.java:15-53`).
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub struct SwizzleArgumentType;

impl ArgumentType for SwizzleArgumentType {
    type Item = Axes;

    /// Vanilla `SwizzleArgument.parse`
    /// (`net/minecraft/commands/arguments/coordinates/SwizzleArgument.java:27-47`):
    /// reads axis characters until whitespace/end and rejects unknown or duplicate
    /// axes with `arguments.swizzle.invalid`.
    fn parse(&self, reader: &mut StringReader) -> Result<Self::Item, CommandSyntaxError> {
        let mut axes = Axes::default();

        while reader.can_read_char() && reader.peek() != Some(' ') {
            let axis = match reader.read() {
                Some('x') => Axis::X,
                Some('y') => Axis::Y,
                Some('z') => Axis::Z,
                _ => return Err(INVALID_ERROR_TYPE.create(reader)),
            };

            if axes.contains(axis) {
                return Err(INVALID_ERROR_TYPE.create(reader));
            }

            axes = axes.with_axis(axis);
        }

        Ok(axes)
    }

    fn client_side_parser(&'_ self) -> JavaClientArgumentType {
        JavaClientArgumentType::Swizzle
    }

    fn list_suggestions<'a>(
        &'a self,
        _context: &'a CommandContext,
        builder: SuggestionsBuilder,
    ) -> Pin<Box<dyn Future<Output = Suggestions> + Send + 'a>> {
        Box::pin(async move { builder.build() })
    }

    fn examples(&self) -> Vec<String> {
        examples!("xyz", "x")
    }
}

impl_copy_get!(SwizzleArgumentType, Axes);

#[cfg(test)]
mod test {
    use super::{Axes, INVALID_ERROR_TYPE, SwizzleArgumentType};
    use crate::command::{
        argument_types::argument_type::ArgumentType, string_reader::StringReader,
    };
    use pumpkin_util::math::vector3::Axis;

    #[test]
    fn parse_swizzle() {
        let mut reader = StringReader::new("xyz");
        let axes = ArgumentType::parse(&SwizzleArgumentType, &mut reader).unwrap();
        assert!(axes.contains(Axis::X));
        assert!(axes.contains(Axis::Y));
        assert!(axes.contains(Axis::Z));

        let mut reader = StringReader::new("x ");
        let axes = ArgumentType::parse(&SwizzleArgumentType, &mut reader).unwrap();
        assert!(axes.contains(Axis::X));
        assert!(!axes.contains(Axis::Y));
        assert!(!axes.contains(Axis::Z));

        // Vanilla stops at the space without consuming it
        // (`SwizzleArgument.java:30`).
        assert_eq!(reader.cursor(), 1);
    }

    #[test]
    fn parse_swizzle_errors() {
        // Duplicate axes are rejected (`SwizzleArgument.java:39-41`).
        let mut reader = StringReader::new("xx");
        let error = ArgumentType::parse(&SwizzleArgumentType, &mut reader).unwrap_err();
        let expected: &'static dyn crate::command::errors::error_types::AnyCommandErrorType =
            &INVALID_ERROR_TYPE;
        assert_eq!(error.error_type, expected);

        // Unknown letters are rejected (`SwizzleArgument.java:33-37`).
        let mut reader = StringReader::new("w");
        assert!(ArgumentType::parse(&SwizzleArgumentType, &mut reader).is_err());
    }

    #[test]
    fn default_axes_are_empty() {
        assert_eq!(
            Axes::default(),
            Axes {
                x: false,
                y: false,
                z: false
            }
        );
    }
}
