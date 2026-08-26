//! Macro support for datapack functions.
//!
//! Port of vanilla `net.minecraft.commands.functions.MacroFunction` (26.2)
//! together with its helpers `StringTemplate` and the `$`-line compilation of
//! `CommandFunction.fromLines`/`FunctionBuilder`. A `.mcfunction` file
//! containing at least one `$`-prefixed line compiles into a [`LoadedFunction`]
//! carrying macro templates plus the ordered parameter-name list; executing it
//! requires an NBT compound of arguments whose stringified values are
//! substituted into every macro line before dispatch.
//!
//! Difference to vanilla worth noting: vanilla parses each substituted command
//! through the dispatcher during instantiation and caches the compiled actions
//! (`MacroFunction.substituteAndParse`, `MacroFunction.java:101-111`). Pumpkin
//! parses commands lazily inside `CommandDispatcher::handle_command`, so
//! instantiation here yields the substituted command strings and the cache
//! stores those instead of parsed actions.

use pumpkin_nbt::compound::NbtCompound;
use pumpkin_nbt::tag::NbtTag;

/// Maximum number of cached instantiations per function, mirroring vanilla's
/// `MAX_CACHE_ENTRIES = 8` (`MacroFunction.java:34`).
const MAX_CACHE_ENTRIES: usize = 8;

/// Maximum characters allowed for a single (substituted) command line, port of
/// `CommandFunction.checkCommandLineLength` (`CommandFunction.java:89-94`).
const MAX_COMMAND_LINE_LENGTH: usize = 2_000_000;

/// One parsed function body line.
///
/// Direct analogue of the two `MacroFunction.Entry` implementations: a
/// pre-resolved command ([`FunctionLine::Plain`], vanilla
/// `PlainTextEntry`, `MacroFunction.java:149-165`) or a `$` macro template
/// ([`FunctionLine::Macro`], vanilla `MacroEntry`,
/// `MacroFunction.java:119-147`).
#[derive(Clone, Debug)]
pub enum FunctionLine {
    Plain(String),
    Macro(MacroTemplate),
}

/// A loaded datapack function: either purely plain lines (vanilla
/// `PlainTextFunction`) or mixed plain/macro entries with a parameter list
/// (vanilla `MacroFunction`, `MacroFunction.java:30-44`).
#[derive(Clone, Debug, Default)]
pub struct LoadedFunction {
    pub lines: Vec<FunctionLine>,
    /// Every macro variable used by this function, ordered by first appearance
    /// across all macro lines — port of the deduplicated insertion order built
    /// by `FunctionBuilder.getArgumentIndex` (`FunctionBuilder.java:25-33`)
    /// stored in `MacroFunction.parameters` (`MacroFunction.java:35`).
    pub parameters: Vec<String>,
}

/// Why instantiating a macro function failed. Each variant maps onto the
/// `FunctionInstantiationException` raised by vanilla's
/// `MacroFunction.instantiate` (`MacroFunction.java:52-68`).
#[derive(Debug, PartialEq, Eq)]
pub enum InstantiationError {
    /// No arguments were supplied to a macro function
    /// (`MacroFunction.java:53-55`).
    MissingArguments,
    /// The supplied argument compound lacks a required parameter
    /// (`MacroFunction.java:61-65`).
    MissingArgument(String),
}

impl LoadedFunction {
    /// Compiles raw `.mcfunction` lines into [`FunctionLine`]s.
    ///
    /// Lines starting with `$` become macro templates parsed by
    /// [`MacroTemplate::from_string`]; everything else stays plain. Port of the
    /// `$` branch of `CommandFunction.fromLines` (`CommandFunction.java:74-75`)
    /// feeding `FunctionBuilder.addMacro` (`FunctionBuilder.java:45-64`); the
    /// returned error text mirrors the
    /// "Can't parse function line {line}: '{command}'"
    /// wrapper of `FunctionBuilder.java:50`.
    pub fn from_lines(lines: &[String]) -> Result<Self, String> {
        let mut function_lines = Vec::with_capacity(lines.len());
        let mut parameters: Vec<String> = Vec::new();

        for (i, line) in lines.iter().enumerate() {
            if let Some(rest) = line.strip_prefix('$') {
                let template = MacroTemplate::from_string(rest)
                    .map_err(|e| format!("Can't parse function line {}: '{line}' ({e})", i + 1))?;
                // `FunctionBuilder.getArgumentIndex` (`FunctionBuilder.java:25-33`):
                // register variables in order of first appearance, deduplicated.
                for variable in &template.variables {
                    if !parameters.contains(variable) {
                        parameters.push(variable.clone());
                    }
                }
                function_lines.push(FunctionLine::Macro(template));
            } else {
                function_lines.push(FunctionLine::Plain(line.clone()));
            }
        }

        Ok(Self {
            lines: function_lines,
            parameters,
        })
    }

    /// Returns whether this function contains any `$` macro line, i.e. whether
    /// it behaves like vanilla's `MacroFunction` rather than
    /// `PlainTextFunction` (`FunctionBuilder.build`,
    /// `FunctionBuilder.java:66-68`).
    #[must_use]
    pub const fn is_macro(&self) -> bool {
        !self.parameters.is_empty()
    }

    /// Extracts and stringifies every parameter value from the argument
    /// compound, in parameter order. Port of the argument-collection loop of
    /// `MacroFunction.instantiate` (`MacroFunction.java:57-68`).
    pub fn extract_parameter_values(
        &self,
        arguments: &NbtCompound,
    ) -> Result<Vec<String>, InstantiationError> {
        self.parameters
            .iter()
            .map(|parameter| {
                arguments
                    .get(parameter)
                    .map(stringify)
                    .ok_or_else(|| InstantiationError::MissingArgument(parameter.clone()))
            })
            .collect()
    }

    /// Substitutes the already-stringified parameter values into every macro
    /// line, producing the concrete command lines to dispatch. Port of
    /// `MacroFunction.lookupValues` + `substituteAndParse`
    /// (`MacroFunction.java:96-111`), minus the dispatcher parse (see module
    /// docs).
    ///
    /// # Errors
    /// Propagates [`MacroTemplate::substitute`] failures (excessively long
    /// command lines).
    pub fn substitute_all(&self, values: &[String]) -> Result<Vec<String>, String> {
        self.lines
            .iter()
            .map(|line| match line {
                FunctionLine::Plain(command) => Ok(command.clone()),
                FunctionLine::Macro(template) => template.substitute(values),
            })
            .collect()
    }
}

/// A parsed `$` macro line: literal segments interleaved with variable names.
///
/// Port of vanilla's record `StringTemplate(List<String> segments,
/// List<String> variables)` (`StringTemplate.java:7`); `segments.len()` is
/// either `variables.len()` or `variables.len() + 1`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MacroTemplate {
    pub segments: Vec<String>,
    pub variables: Vec<String>,
}

impl MacroTemplate {
    /// Parses the body of a `$` line (without the leading `$`).
    ///
    /// Port of `StringTemplate.fromString` (`StringTemplate.java:8-45`): a `$`
    /// followed by `(` opens a variable closed by the next `)`; any other `$`
    /// is a literal dollar sign. Fails with vanilla's messages for an
    /// unterminated variable (`:20`), an invalid variable name (`:25`) and a
    /// macro without any variable (`:37`).
    pub fn from_string(input: &str) -> Result<Self, String> {
        let bytes = input.as_bytes();
        let mut segments = Vec::new();
        let mut variables = Vec::new();
        let mut start = 0usize;
        let mut search = 0usize;

        while let Some(index) = find_ascii_byte(bytes, search, b'$') {
            // `input.charAt(index + 1) == '('` guard (`StringTemplate.java:16`);
            // `$` and `(` are ASCII, so byte indexing cannot split a code point.
            if index + 1 < bytes.len() && bytes[index + 1] == b'(' {
                segments.push(input[start..index].to_string());

                let Some(close) = find_ascii_byte(bytes, index + 2, b')') else {
                    return Err("Unterminated macro variable".to_string());
                };

                let variable = &input[index + 2..close];
                if !Self::is_valid_variable_name(variable) {
                    return Err(format!("Invalid macro variable name '{variable}'"));
                }

                variables.push(variable.to_string());
                start = close + 1;
                search = start;
            } else {
                search = index + 1;
            }
        }

        if start == 0 {
            return Err("No variables in macro".to_string());
        }

        if start != bytes.len() {
            segments.push(input[start..].to_string());
        }

        Ok(Self {
            segments,
            variables,
        })
    }

    /// Port of `StringTemplate.isValidVariableName`
    /// (`StringTemplate.java:47-56`): letters, digits and underscores.
    fn is_valid_variable_name(variable: &str) -> bool {
        variable.chars().all(|c| c.is_alphanumeric() || c == '_')
    }

    /// Substitutes concrete values for this template's variables.
    ///
    /// Port of `StringTemplate.substitute` (`StringTemplate.java:58-72`),
    /// including the per-concatenation line-length guard of
    /// `CommandFunction.checkCommandLineLength`
    /// (`CommandFunction.java:89-94`).
    ///
    /// # Errors
    /// Fails if `arguments` is shorter than the template's variable count
    /// (cannot happen through [`LoadedFunction`], which always passes one value
    /// per registered parameter) or if the resulting line exceeds
    /// [`MAX_COMMAND_LINE_LENGTH`].
    pub fn substitute(&self, arguments: &[String]) -> Result<String, String> {
        let mut builder = String::new();

        for (i, variable) in self.variables.iter().enumerate() {
            let value = arguments
                .get(i)
                .ok_or_else(|| format!("Missing macro argument '{variable}' for template"))?;
            builder.push_str(&self.segments[i]);
            builder.push_str(value);
            check_command_line_length(&builder)?;
        }

        // `segments.len() > variables.len()` guarantees a final segment exists.
        if self.segments.len() > self.variables.len()
            && let Some(last) = self.segments.last()
        {
            builder.push_str(last);
        }

        check_command_line_length(&builder)?;
        Ok(builder)
    }
}

/// Finds `needle` in `bytes` at or after `from`; unlike `str::find` this never
/// panics on non-char-boundary offsets because both inputs are ASCII-boundary
/// aware (`needle` is an ASCII byte and matches only at char boundaries).
fn find_ascii_byte(bytes: &[u8], from: usize, needle: u8) -> Option<usize> {
    assert!(needle.is_ascii(), "only ASCII needles are supported");
    bytes[from.min(bytes.len())..]
        .iter()
        .position(|&b| b == needle)
        .map(|pos| from.min(bytes.len()) + pos)
}

/// Port of `CommandFunction.checkCommandLineLength`
/// (`CommandFunction.java:89-94`).
fn check_command_line_length(line: &str) -> Result<(), String> {
    if line.len() > MAX_COMMAND_LINE_LENGTH {
        let truncated: String = line.chars().take(512).collect();
        return Err(format!(
            "Command too long: {} characters, contents: {truncated}...",
            line.len()
        ));
    }
    Ok(())
}

/// Converts an NBT tag to its macro substitution string.
///
/// Port of `MacroFunction.stringify` (`MacroFunction.java:84-94`): floats and
/// doubles go through [`format_decimal`], byte/short/long render as bare
/// integers, strings render unquoted, and everything else (ints, lists,
/// compounds) falls back to `tag.toString()`.
#[must_use]
pub fn stringify(tag: &NbtTag) -> String {
    match tag {
        NbtTag::Float(value) => format_decimal(f64::from(*value)),
        NbtTag::Double(value) => format_decimal(*value),
        NbtTag::Byte(value) => value.to_string(),
        NbtTag::Short(value) => value.to_string(),
        NbtTag::Long(value) => value.to_string(),
        NbtTag::String(value) => value.to_string(),
        other => other.to_string(),
    }
}

/// Mirrors vanilla's shared `DECIMAL_FORMAT` (`MacroFunction.java:31-33`): a
/// root-locale `#` pattern with at most 15 fractional digits, which strips
/// trailing zeros (so `1.0f` substitutes as `"1"` and `0.5f` as `"0.5"`).
/// Rounding matches Java's half-to-even default; NaN/infinity spellings follow
/// `DecimalFormat`.
fn format_decimal(value: f64) -> String {
    if value.is_nan() {
        return "NaN".to_string();
    }
    if value.is_infinite() {
        return if value.is_sign_negative() {
            "-∞"
        } else {
            "∞"
        }
        .to_string();
    }

    let mut formatted = format!("{value:.15}");
    if formatted.contains('.') {
        while formatted.ends_with('0') {
            formatted.pop();
        }
        if formatted.ends_with('.') {
            formatted.pop();
        }
    }
    formatted
}

/// Per-function cache of instantiated (substituted) command-line lists, keyed
/// by the stringified parameter values.
///
/// Port of the `Object2ObjectLinkedOpenHashMap` used as an LRU by
/// `MacroFunction` (`MacroFunction.java:36`): a hit moves the entry to the
/// most-recently-used position (`getAndMoveToLast`, `:70-73`) and inserting
/// beyond capacity evicts the least-recently-used entry first (`:75-77`).
#[derive(Default)]
pub struct InstantiationCache {
    /// `(parameter values, instantiated command lines)` pairs ordered from
    /// least- to most-recently-used.
    entries: Vec<(Vec<String>, Vec<String>)>,
}

impl InstantiationCache {
    /// Returns the cached instantiation for `key`, refreshing its recency, or
    /// computes and caches it on a miss. Port of the cache block of
    /// `MacroFunction.instantiate` (`MacroFunction.java:70-81`).
    ///
    /// # Errors
    /// Propagates the `compute` failure; nothing is cached in that case
    /// (matching vanilla, which only puts successfully parsed functions,
    /// `MacroFunction.java:79-81`).
    pub fn get_or_insert_with(
        &mut self,
        key: &[String],
        compute: impl FnOnce() -> Result<Vec<String>, String>,
    ) -> Result<Vec<String>, String> {
        if let Some(position) = self.entries.iter().position(|(k, _)| k == key) {
            let entry = self.entries.remove(position);
            self.entries.push(entry);
            return Ok(self
                .entries
                .last()
                .map_or_else(Vec::new, |(_, lines)| lines.clone()));
        }

        let instantiated = compute()?;
        if self.entries.len() >= MAX_CACHE_ENTRIES {
            self.entries.remove(0);
        }
        self.entries.push((key.to_vec(), instantiated.clone()));
        Ok(instantiated)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn template(input: &str) -> MacroTemplate {
        MacroTemplate::from_string(input).expect("valid template")
    }

    #[test]
    fn parse_simple_template() {
        let t = template("say hello $(name)");
        assert_eq!(t.segments, vec!["say hello ".to_string()]);
        assert_eq!(t.variables, vec!["name".to_string()]);
    }

    #[test]
    fn parse_multi_variable_template_keeps_trailing_segment() {
        let t = template("tp $(x) $(y) $(z)");
        assert_eq!(t.variables, vec!["x", "y", "z"]);
        assert_eq!(t.segments, vec!["tp ", " ", " "]);
    }

    #[test]
    fn literal_dollar_is_not_a_variable() {
        // `$$(` is a literal `$` followed by a variable open paren.
        let t = template("cost $$(price)");
        assert_eq!(t.variables, vec!["price".to_string()]);
        assert_eq!(t.segments, vec!["cost $".to_string()]);
    }

    #[test]
    fn rejects_unterminated_variable() {
        assert_eq!(
            MacroTemplate::from_string("say $(name").unwrap_err(),
            "Unterminated macro variable"
        );
    }

    #[test]
    fn rejects_invalid_variable_name() {
        assert_eq!(
            MacroTemplate::from_string("say $(bad name)").unwrap_err(),
            "Invalid macro variable name 'bad name'"
        );
    }

    #[test]
    fn rejects_variableless_macro() {
        assert_eq!(
            MacroTemplate::from_string("say hi").unwrap_err(),
            "No variables in macro"
        );
    }

    #[test]
    fn substitute_interleaves_segments_and_values() {
        let t = template("tp $(x) $(y)");
        assert_eq!(
            t.substitute(&["10".to_string(), "64".to_string()])
                .expect("fits"),
            "tp 10 64"
        );
    }

    #[test]
    fn stringify_follows_vanilla_switch() {
        // Exact doubles print without trailing zeros, matching
        // `DecimalFormat("#", maxFrac=15)` (`MacroFunction.java:31-33`).
        assert_eq!(stringify(&NbtTag::Float(1.5)), "1.5");
        assert_eq!(stringify(&NbtTag::Float(2.0)), "2");
        assert_eq!(stringify(&NbtTag::Float(-0.5)), "-0.5");
        assert_eq!(stringify(&NbtTag::Double(12.375)), "12.375");
        assert_eq!(stringify(&NbtTag::Float(1.0)), "1"); // trailing zero stripped
        assert_eq!(stringify(&NbtTag::Byte(-1)), "-1");
        assert_eq!(stringify(&NbtTag::Short(300)), "300");
        assert_eq!(stringify(&NbtTag::Long(i64::MAX)), "9223372036854775807");
        assert_eq!(stringify(&NbtTag::Int(42)), "42"); // default -> toString
        assert_eq!(stringify(&NbtTag::String("hi".into())), "hi");
    }

    #[test]
    fn cache_moves_hits_to_back_and_evicts_lru() {
        let mut cache = InstantiationCache::default();
        let key = |v: &str| vec![v.to_string()];
        let make = |v: &'static str| move || Ok(vec![v.to_string()]);

        for name in ["a", "b", "c", "d", "e", "f", "g", "h"] {
            cache
                .get_or_insert_with(&key(name), make(name))
                .expect("compute ok");
        }
        // Cache is full; "a" is LRU. Touching "b" makes "a" the eviction victim.
        assert_eq!(
            cache
                .get_or_insert_with(&key("b"), || Ok(vec!["should-not-run".to_string()]))
                .expect("hit"),
            vec!["b".to_string()]
        );
        cache
            .get_or_insert_with(&key("i"), make("i"))
            .expect("insert ok");
        // "a" was evicted; "b" survived thanks to the recency bump.
        assert_eq!(
            cache
                .get_or_insert_with(&key("b"), || Ok(vec!["recomputed".to_string()]))
                .expect("still cached"),
            vec!["b".to_string()]
        );
        assert_eq!(
            cache
                .get_or_insert_with(&key("a"), make("recomputed-a"))
                .expect("miss recomputes"),
            vec!["recomputed-a".to_string()]
        );
    }

    #[test]
    fn loaded_function_collects_parameters_in_first_appearance_order() {
        let lines = vec![
            "say plain".to_string(),
            "$tp $(x) $(y)".to_string(),
            "$say $(x) done".to_string(),
        ];
        let function = LoadedFunction::from_lines(&lines).expect("parses");
        assert!(function.is_macro());
        assert_eq!(function.parameters, vec!["x".to_string(), "y".to_string()]);
    }

    #[test]
    fn plain_function_has_no_parameters() {
        let function = LoadedFunction::from_lines(&["say hi".to_string()]).expect("parses");
        assert!(!function.is_macro());
    }
}
