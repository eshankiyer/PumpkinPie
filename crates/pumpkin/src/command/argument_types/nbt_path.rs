use crate::command::errors::command_syntax_error::CommandSyntaxError;
use crate::command::{
    argument_types::argument_type::{ArgumentType, JavaClientArgumentType},
    context::command_context::CommandContext,
    errors::error_types::CommandErrorType,
    snbt::SnbtParser,
    string_reader::StringReader,
};
use pumpkin_data::translation;
use pumpkin_nbt::{compound::NbtCompound, tag::NbtTag};
use pumpkin_util::text::TextComponent;

// Vanilla NbtPathArgument.java:62-149 defines the path grammar and node separators.
const INVALID_NODE_ERROR: CommandErrorType<0> = CommandErrorType::new(
    translation::java::ARGUMENTS_NBTPATH_NODE_INVALID,
    translation::java::ARGUMENTS_NBTPATH_NODE_INVALID,
);

// Vanilla NbtPathArgument.java:489-529 uses this error when a queried path has no result.
const NOTHING_FOUND_ERROR: CommandErrorType<1> = CommandErrorType::new(
    translation::java::ARGUMENTS_NBTPATH_NOTHING_FOUND,
    translation::java::ARGUMENTS_NBTPATH_NOTHING_FOUND,
);

// Vanilla NbtPathArgument.java:155-529 contains the six node implementations ported below.
#[derive(Clone, Debug, PartialEq)]
pub enum PathNode {
    CompoundChild(String),
    MatchObject(String, NbtCompound),
    MatchRootObject(NbtCompound),
    IndexedElement(i32),
    MatchElement(NbtCompound),
    AllElements,
}

// Vanilla NbtUtils.java:65-111 compares compounds as subsets and list elements by existence.
#[must_use]
pub fn compare_nbt(expected: &NbtTag, actual: &NbtTag, partial_list_matches: bool) -> bool {
    if std::mem::discriminant(expected) != std::mem::discriminant(actual) {
        return false;
    }

    match (expected, actual) {
        (NbtTag::Compound(expected), NbtTag::Compound(actual)) => {
            expected.child_tags.len() <= actual.child_tags.len()
                && expected.child_tags.iter().all(|(key, value)| {
                    actual
                        .child_tags
                        .get(key)
                        .is_some_and(|actual| compare_nbt(value, actual, partial_list_matches))
                })
        }
        (NbtTag::List(expected), NbtTag::List(actual)) if partial_list_matches => {
            if expected.is_empty() {
                return actual.is_empty();
            }
            expected.len() <= actual.len()
                && expected.iter().all(|expected| {
                    actual
                        .iter()
                        .any(|actual| compare_nbt(expected, actual, partial_list_matches))
                })
        }
        _ => expected == actual,
    }
}

impl PathNode {
    // Vanilla NbtPathArgument.java:159-176, 239-251, 303-313, 360-365, 419-428, and 475-479.
    fn get_tag<'a>(&self, parent: &'a NbtTag, output: &mut Vec<&'a NbtTag>) {
        match self {
            Self::AllElements => {
                if let NbtTag::List(list) = parent {
                    output.extend(list);
                }
            }
            Self::CompoundChild(name) => {
                if let NbtTag::Compound(compound) = parent
                    && let Some(tag) = compound.get(name)
                {
                    output.push(tag);
                }
            }
            Self::IndexedElement(index) => {
                if let NbtTag::List(list) = parent {
                    if let Some(index) = list_index(*index, list.len()) {
                        output.push(&list[index]);
                    }
                }
            }
            Self::MatchElement(pattern) => {
                if let NbtTag::List(list) = parent {
                    output.extend(
                        list.iter().filter(|tag| {
                            compare_nbt(&NbtTag::Compound(pattern.clone()), tag, true)
                        }),
                    );
                }
            }
            Self::MatchObject(name, pattern) => {
                if let NbtTag::Compound(compound) = parent
                    && let Some(tag) = compound.get(name)
                    && compare_nbt(&NbtTag::Compound(pattern.clone()), tag, true)
                {
                    output.push(tag);
                }
            }
            Self::MatchRootObject(pattern) => {
                if matches!(parent, NbtTag::Compound(_))
                    && compare_nbt(&NbtTag::Compound(pattern.clone()), parent, true)
                {
                    output.push(parent);
                }
            }
        }
    }

    // Vanilla NbtPathArgument.java:166-176, 252-266, 314-323, 366-370, 429-445, and 480-483.
    fn get_or_create_tag<'a>(
        &self,
        parent: &'a mut NbtTag,
        preferred_parent: Option<&PathNode>,
        new_tag: &mut dyn FnMut() -> NbtTag,
        output: &mut Vec<&'a mut NbtTag>,
    ) {
        match self {
            Self::AllElements => {
                if let NbtTag::List(list) = parent {
                    if list.is_empty() {
                        list.push(
                            preferred_parent.map_or_else(new_tag, PathNode::preferred_parent),
                        );
                    }
                    output.extend(list.iter_mut());
                }
            }
            Self::CompoundChild(name) => {
                if let NbtTag::Compound(compound) = parent {
                    if !compound.has(name) {
                        compound.put(
                            name,
                            preferred_parent.map_or_else(new_tag, PathNode::preferred_parent),
                        );
                    }
                    if let Some(tag) = compound.get_mut(name) {
                        output.push(tag);
                    }
                }
            }
            Self::IndexedElement(index) => {
                if let NbtTag::List(list) = parent
                    && let Some(index) = list_index(*index, list.len())
                {
                    output.push(&mut list[index]);
                }
            }
            Self::MatchElement(pattern) => {
                if let NbtTag::List(list) = parent {
                    if !list
                        .iter()
                        .any(|tag| compare_nbt(&NbtTag::Compound(pattern.clone()), tag, true))
                    {
                        list.push(NbtTag::Compound(pattern.clone()));
                    }
                    output.extend(
                        list.iter_mut().filter(|tag| {
                            compare_nbt(&NbtTag::Compound(pattern.clone()), tag, true)
                        }),
                    );
                }
            }
            Self::MatchObject(name, pattern) => {
                if let NbtTag::Compound(compound) = parent {
                    if !compound.has(name) {
                        compound.put(name, NbtTag::Compound(pattern.clone()));
                    }
                    if let Some(tag) = compound.get_mut(name)
                        && compare_nbt(&NbtTag::Compound(pattern.clone()), tag, true)
                    {
                        output.push(tag);
                    }
                }
            }
            Self::MatchRootObject(pattern) => {
                if matches!(parent, NbtTag::Compound(_))
                    && compare_nbt(&NbtTag::Compound(pattern.clone()), parent, true)
                {
                    output.push(parent);
                }
            }
        }
    }

    // Vanilla NbtPathArgument.java:180-182, 267-280, 324-347, 371-390, 446-464, and 484-487.
    fn preferred_parent(&self) -> NbtTag {
        match self {
            Self::AllElements | Self::IndexedElement(_) | Self::MatchElement(_) => {
                NbtTag::List(Vec::new())
            }
            Self::CompoundChild(_) | Self::MatchObject(_, _) | Self::MatchRootObject(_) => {
                NbtTag::Compound(NbtCompound::new())
            }
        }
    }

    // Vanilla NbtPathArgument.java:185-225, 281-301, 325-358, and 391-417.
    fn set_tag(&self, parent: &mut NbtTag, new_tag: &mut dyn FnMut() -> NbtTag) -> i32 {
        match self {
            Self::AllElements => {
                let NbtTag::List(list) = parent else { return 0 };
                let size = list.len();
                if size == 0 {
                    list.push(new_tag());
                    return 1;
                }
                let first = new_tag();
                let changed = size - list.iter().filter(|tag| **tag == first).count();
                if changed == 0 {
                    return 0;
                }
                list.clear();
                list.push(first);
                for _ in 1..size {
                    list.push(new_tag());
                }
                changed as i32
            }
            Self::CompoundChild(name) => {
                let NbtTag::Compound(compound) = parent else {
                    return 0;
                };
                let new_tag = new_tag();
                let changed = compound.get(name) != Some(&new_tag);
                compound.put(name, new_tag);
                i32::from(changed)
            }
            Self::IndexedElement(index) => {
                let NbtTag::List(list) = parent else { return 0 };
                let Some(index) = list_index(*index, list.len()) else {
                    return 0;
                };
                let new_tag = new_tag();
                if list[index] == new_tag {
                    0
                } else {
                    list[index] = new_tag;
                    1
                }
            }
            Self::MatchElement(pattern) => {
                let NbtTag::List(list) = parent else { return 0 };
                if list.is_empty() {
                    list.push(new_tag());
                    return 1;
                }
                let mut changed = 0;
                for tag in list.iter_mut() {
                    if compare_nbt(&NbtTag::Compound(pattern.clone()), tag, true) {
                        let replacement = new_tag();
                        if *tag != replacement {
                            *tag = replacement;
                            changed += 1;
                        }
                    }
                }
                changed
            }
            Self::MatchObject(name, pattern) => {
                let NbtTag::Compound(compound) = parent else {
                    return 0;
                };
                let Some(current) = compound.get(name) else {
                    return 0;
                };
                if !compare_nbt(&NbtTag::Compound(pattern.clone()), current, true) {
                    return 0;
                }
                let replacement = new_tag();
                if current == &replacement {
                    0
                } else {
                    compound.put(name, replacement);
                    1
                }
            }
            Self::MatchRootObject(_) => 0,
        }
    }

    // Vanilla NbtPathArgument.java:215-225, 292-301, 347-358, and 405-417.
    fn remove_tag(&self, parent: &mut NbtTag) -> i32 {
        match self {
            Self::AllElements => {
                let NbtTag::List(list) = parent else { return 0 };
                let count = list.len();
                list.clear();
                count as i32
            }
            Self::CompoundChild(name) => {
                let NbtTag::Compound(compound) = parent else {
                    return 0;
                };
                i32::from(compound.remove(name).is_some())
            }
            Self::IndexedElement(index) => {
                let NbtTag::List(list) = parent else { return 0 };
                let Some(index) = list_index(*index, list.len()) else {
                    return 0;
                };
                list.remove(index);
                1
            }
            Self::MatchElement(pattern) => {
                let NbtTag::List(list) = parent else { return 0 };
                let old_len = list.len();
                list.retain(|tag| !compare_nbt(&NbtTag::Compound(pattern.clone()), tag, true));
                (old_len - list.len()) as i32
            }
            Self::MatchObject(name, pattern) => {
                let NbtTag::Compound(compound) = parent else {
                    return 0;
                };
                let Some(current) = compound.get(name) else {
                    return 0;
                };
                if compare_nbt(&NbtTag::Compound(pattern.clone()), current, true) {
                    compound.remove(name);
                    1
                } else {
                    0
                }
            }
            Self::MatchRootObject(_) => 0,
        }
    }
}

fn list_index(index: i32, len: usize) -> Option<usize> {
    let index = if index < 0 { len as i32 + index } else { index };
    (index >= 0 && (index as usize) < len).then_some(index as usize)
}

// Vanilla NbtPathArgument.java:62-149 and 489-529 define parsing, path traversal, and mutation.
#[derive(Clone, Debug, PartialEq)]
pub struct NbtPath {
    pub original: String,
    pub nodes: Vec<PathNode>,
    node_positions: Vec<usize>,
}

impl NbtPath {
    pub fn parse(reader: &mut StringReader) -> Result<Self, CommandSyntaxError> {
        let start = reader.cursor();
        let mut nodes = Vec::new();
        let mut node_positions = Vec::new();
        let mut first_node = true;

        while reader.can_read_char() && reader.peek() != Some(' ') {
            let node = parse_node(reader, first_node)?;
            nodes.push(node);
            node_positions.push(reader.cursor() - start);
            first_node = false;
            if reader.can_read_char() {
                let next = reader.peek();
                if next != Some(' ') && next != Some('[') && next != Some('{') {
                    reader.expect('.')?;
                }
            }
        }

        Ok(Self {
            original: reader.string()[start..reader.cursor()].to_owned(),
            nodes,
            node_positions,
        })
    }

    fn collect<'a>(&self, tag: &'a NbtTag) -> Vec<&'a NbtTag> {
        let mut current = vec![tag];
        for node in &self.nodes {
            let mut next = Vec::new();
            for parent in current {
                node.get_tag(parent, &mut next);
            }
            current = next;
        }
        current
    }

    pub fn get<'a>(&self, tag: &'a NbtTag) -> Result<Vec<&'a NbtTag>, CommandSyntaxError> {
        let mut current = vec![tag];
        for (index, node) in self.nodes.iter().enumerate() {
            let mut next = Vec::new();
            for parent in current {
                node.get_tag(parent, &mut next);
            }
            if next.is_empty() {
                return Err(self.nothing_found(index));
            }
            current = next;
        }
        Ok(current)
    }

    #[must_use]
    pub fn count_matching(&self, tag: &NbtTag) -> usize {
        self.collect(tag).len()
    }

    fn collect_mut<'a>(
        &self,
        index: usize,
        parents: Vec<&'a mut NbtTag>,
        create: bool,
        new_tag: &mut dyn FnMut() -> NbtTag,
    ) -> Vec<&'a mut NbtTag> {
        let node = &self.nodes[index];
        let preferred = (index + 1 < self.nodes.len()).then(|| &self.nodes[index + 1]);
        let mut children = Vec::new();
        for parent in parents {
            if create {
                node.get_or_create_tag(parent, preferred, new_tag, &mut children);
            } else {
                collect_existing_node(node, parent, &mut children);
            }
        }
        children
    }

    fn parents_mut<'a>(
        &self,
        tag: &'a mut NbtTag,
        create: bool,
        new_tag: &mut dyn FnMut() -> NbtTag,
    ) -> Result<Vec<&'a mut NbtTag>, CommandSyntaxError> {
        if self.nodes.is_empty() {
            return Err(INVALID_NODE_ERROR.create_without_context());
        }
        let mut current = vec![tag];
        for index in 0..self.nodes.len() - 1 {
            current = self.collect_mut(index, current, create, new_tag);
            if current.is_empty() {
                return Err(self.nothing_found(index));
            }
        }
        Ok(current)
    }

    pub fn get_or_create<'a, F>(
        &self,
        tag: &'a mut NbtTag,
        mut new_tag: F,
    ) -> Result<Vec<&'a mut NbtTag>, CommandSyntaxError>
    where
        F: FnMut() -> NbtTag,
    {
        let last = self
            .nodes
            .len()
            .checked_sub(1)
            .ok_or_else(|| INVALID_NODE_ERROR.create_without_context())?;
        let parents = self.parents_mut(tag, true, &mut new_tag)?;
        let mut output = Vec::new();
        for parent in parents {
            self.nodes[last].get_or_create_tag(parent, None, &mut new_tag, &mut output);
        }
        Ok(output)
    }

    // Vanilla NbtPathArgument.java:517-529 applies the final node operation to every parent.
    pub fn set(&self, tag: &mut NbtTag, to_add: &NbtTag) -> Result<i32, CommandSyntaxError> {
        if is_too_deep(to_add, self.nodes.len()) {
            return Err(DATA_TOO_DEEP_ERROR.create_without_context());
        }
        let mut replacement = Some(to_add.clone());
        let parents = self.parents_mut(tag, true, &mut || {
            replacement.take().unwrap_or_else(|| to_add.clone())
        })?;
        let last = self
            .nodes
            .last()
            .ok_or_else(|| INVALID_NODE_ERROR.create_without_context())?;
        let mut first = Some(to_add.clone());
        Ok(parents
            .into_iter()
            .map(|parent| {
                last.set_tag(parent, &mut || {
                    first.take().unwrap_or_else(|| to_add.clone())
                })
            })
            .sum())
    }

    pub fn remove(&self, tag: &mut NbtTag) -> i32 {
        let Some(last) = self.nodes.last() else {
            return 0;
        };
        let mut no_create = || NbtTag::End;
        let parents = self
            .parents_mut(tag, false, &mut no_create)
            .unwrap_or_default();
        parents
            .into_iter()
            .map(|parent| last.remove_tag(parent))
            .sum()
    }

    fn nothing_found(&self, index: usize) -> CommandSyntaxError {
        let position = self
            .node_positions
            .get(index)
            .copied()
            .unwrap_or(self.original.len());
        NOTHING_FOUND_ERROR
            .create_without_context(TextComponent::text(self.original[..position].to_owned()))
    }

    #[must_use]
    pub fn as_string(&self) -> &str {
        &self.original
    }

    // Vanilla NbtPathArgument.java:504-514 rejects NBT deeper than 512 levels.
    #[must_use]
    pub fn is_too_deep(tag: &NbtTag, depth: usize) -> bool {
        if depth >= 512 {
            return true;
        }
        match tag {
            NbtTag::Compound(compound) => compound
                .child_tags
                .values()
                .any(|tag| is_too_deep(tag, depth + 1)),
            NbtTag::List(list) => list.iter().any(|tag| is_too_deep(tag, depth + 1)),
            _ => false,
        }
    }
}

fn collect_existing_node<'a>(
    node: &PathNode,
    parent: &'a mut NbtTag,
    output: &mut Vec<&'a mut NbtTag>,
) {
    match node {
        PathNode::AllElements => {
            if let NbtTag::List(list) = parent {
                output.extend(list.iter_mut());
            }
        }
        PathNode::CompoundChild(name) | PathNode::MatchObject(name, _) => {
            if let NbtTag::Compound(compound) = parent
                && let Some(tag) = compound.get_mut(name)
            {
                if let PathNode::MatchObject(_, pattern) = node
                    && !compare_nbt(&NbtTag::Compound(pattern.clone()), tag, true)
                {
                    return;
                }
                output.push(tag);
            }
        }
        PathNode::IndexedElement(index) => {
            if let NbtTag::List(list) = parent
                && let Some(index) = list_index(*index, list.len())
            {
                output.push(&mut list[index]);
            }
        }
        PathNode::MatchElement(pattern) => {
            if let NbtTag::List(list) = parent {
                output.extend(
                    list.iter_mut()
                        .filter(|tag| compare_nbt(&NbtTag::Compound(pattern.clone()), tag, true)),
                );
            }
        }
        PathNode::MatchRootObject(pattern) => {
            if let NbtTag::Compound(_) = parent
                && compare_nbt(&NbtTag::Compound(pattern.clone()), parent, true)
            {
                output.push(parent);
            }
        }
    }
}

fn parse_node(reader: &mut StringReader, first_node: bool) -> Result<PathNode, CommandSyntaxError> {
    match reader.peek() {
        Some('"' | '\'') => Ok(read_object_node(reader, reader.read_string()?)?),
        Some('[') => {
            reader.skip();
            match reader.peek() {
                Some('{') => {
                    let pattern = parse_compound(reader)?;
                    reader.expect(']')?;
                    Ok(PathNode::MatchElement(pattern))
                }
                Some(']') => {
                    reader.skip();
                    Ok(PathNode::AllElements)
                }
                _ => {
                    let index = reader.read_int()?;
                    reader.expect(']')?;
                    Ok(PathNode::IndexedElement(index))
                }
            }
        }
        Some('{') => {
            if !first_node {
                return Err(INVALID_NODE_ERROR.create(reader));
            }
            Ok(PathNode::MatchRootObject(parse_compound(reader)?))
        }
        Some(_) => Ok(read_object_node(reader, read_unquoted_name(reader)?)?),
        None => Err(INVALID_NODE_ERROR.create(reader)),
    }
}

fn read_object_node(
    reader: &mut StringReader,
    name: String,
) -> Result<PathNode, CommandSyntaxError> {
    if name.is_empty() {
        return Err(INVALID_NODE_ERROR.create(reader));
    }
    if reader.peek() == Some('{') {
        Ok(PathNode::MatchObject(name, parse_compound(reader)?))
    } else {
        Ok(PathNode::CompoundChild(name))
    }
}

fn read_unquoted_name(reader: &mut StringReader) -> Result<String, CommandSyntaxError> {
    let start = reader.cursor();
    while let Some(c) = reader.peek()
        && c != ' '
        && !matches!(c, '"' | '\'' | '[' | ']' | '.' | '{' | '}')
    {
        reader.skip();
    }
    if reader.cursor() == start {
        Err(INVALID_NODE_ERROR.create(reader))
    } else {
        Ok(reader.string()[start..reader.cursor()].to_owned())
    }
}

fn parse_compound(reader: &mut StringReader) -> Result<NbtCompound, CommandSyntaxError> {
    match SnbtParser::parse_for_commands(reader)? {
        NbtTag::Compound(compound) => Ok(compound),
        _ => Err(crate::command::argument_types::nbt::EXPECTED_COMPOUND_ERROR_TYPE.create(reader)),
    }
}

pub const DATA_TOO_DEEP_ERROR: CommandErrorType<0> = CommandErrorType::new(
    translation::java::ARGUMENTS_NBTPATH_TOO_DEEP,
    translation::java::ARGUMENTS_NBTPATH_TOO_DEEP,
);

impl ArgumentType for NbtPathArgumentType {
    type Item = NbtPath;

    fn parse(&self, reader: &mut StringReader) -> Result<Self::Item, CommandSyntaxError> {
        NbtPath::parse(reader)
    }

    fn client_side_parser(&self) -> JavaClientArgumentType {
        JavaClientArgumentType::NbtPath
    }

    fn examples(&self) -> Vec<String> {
        examples!("foo", "foo.bar", "foo[0]", "[0]", "[]", "{foo=bar}")
    }
}

pub struct NbtPathArgumentType;

#[cfg(test)]
mod tests {
    use super::*;

    fn path(input: &str) -> NbtPath {
        NbtPath::parse(&mut StringReader::new(input)).expect("valid NBT path")
    }

    #[test]
    fn parses_vanilla_examples() {
        // Vanilla NbtPathArgument.java:35 lists these examples; '=' is the decompile's example spelling,
        // while Pumpkin's SNBT parser uses the command-standard ':' separator for the same compound.
        for input in ["foo", "foo.bar", "foo[0]", "[0]", "[]", "{foo:bar}"] {
            assert!(!path(input).nodes.is_empty());
        }
        assert!(NbtPath::parse(&mut StringReader::new("{foo=bar}")).is_err());
    }

    #[test]
    fn supports_negative_list_index() {
        let tag = NbtTag::List(vec![NbtTag::Int(1), NbtTag::Int(2)]);
        assert_eq!(path("[-1]").get(&tag).unwrap(), vec![&NbtTag::Int(2)]);
    }

    #[test]
    fn creates_missing_parent_containers() {
        let mut tag = NbtTag::Compound(NbtCompound::new());
        path("foo.bar").set(&mut tag, &NbtTag::Int(7)).unwrap();
        assert_eq!(
            tag,
            pumpkin_nbt::nbt_compound_tag! { "foo": pumpkin_nbt::nbt_compound_tag! { "bar": NbtTag::Int(7) } }
        );
    }

    #[test]
    fn root_match_must_be_first_node() {
        assert!(NbtPath::parse(&mut StringReader::new("foo.{bar:1}")).is_err());
    }

    #[test]
    fn compares_partial_compounds_and_lists() {
        let mut expected = NbtCompound::new();
        expected.put_int("wanted", 1);
        let mut actual = NbtCompound::new();
        actual.put_int("wanted", 1);
        actual.put_int("extra", 2);
        assert!(compare_nbt(
            &NbtTag::Compound(expected),
            &NbtTag::Compound(actual),
            true
        ));

        let mut pattern = NbtCompound::new();
        pattern.put_string("id", "target".into());
        let mut other = NbtCompound::new();
        other.put_string("id", "other".into());
        let mut matching = NbtCompound::new();
        matching.put_string("id", "target".into());
        assert!(compare_nbt(
            &NbtTag::List(vec![NbtTag::Compound(pattern)]),
            &NbtTag::List(vec![NbtTag::Compound(other), NbtTag::Compound(matching)]),
            true
        ));
    }
}
