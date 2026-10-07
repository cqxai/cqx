//! Strict-tree invariants shared by parser entry points.

use rezel_common::{SyntaxNode, TextSize, Tree, TypedNode};

use crate::{
    PythonArgList, PythonArrayExpression, PythonAssignStatement, PythonContinuedString,
    PythonDeleteStatement, PythonExpressionNode, PythonForStatement, PythonFormatString,
    PythonMappingPattern, PythonParamList, PythonParenthesizedExpression, PythonString,
    PythonStringPart, PythonTemplateString, PythonTupleExpression, PythonUpdateStatement,
    PythonWithStatement,
};

use super::{PythonComprehension, PythonExpressionItem, expression_items};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct PythonSyntaxError {
    position: TextSize,
    message: &'static str,
}

impl PythonSyntaxError {
    const fn new(position: TextSize, message: &'static str) -> Self {
        Self { position, message }
    }

    pub(crate) const fn position(self) -> TextSize {
        self.position
    }

    pub(crate) const fn message(self) -> &'static str {
        self.message
    }
}

pub(crate) fn validate_syntax(tree: &Tree, source: &str) -> Result<(), PythonSyntaxError> {
    validate_node(&tree.top_node(), source, None)
}

fn validate_node(
    node: &SyntaxNode,
    source: &str,
    inherited_escape_context: Option<EscapeContext>,
) -> Result<(), PythonSyntaxError> {
    if let Ok(statement) = PythonAssignStatement::downcast_from(node.clone()) {
        validate_assignment(&statement)?;
    }
    if let Ok(statement) = PythonUpdateStatement::downcast_from(node.clone()) {
        validate_update(&statement)?;
    }
    if let Ok(statement) = PythonDeleteStatement::downcast_from(node.clone()) {
        validate_delete(&statement)?;
    }
    if let Ok(statement) = PythonForStatement::downcast_from(node.clone()) {
        validate_for(&statement)?;
    }
    if let Ok(statement) = PythonWithStatement::downcast_from(node.clone()) {
        validate_with(&statement)?;
    }
    if let Ok(expression) = PythonExpressionNode::downcast_from(node.clone()) {
        validate_comprehension_expression(&expression)?;
    }
    if let Ok(parameters) = PythonParamList::downcast_from(node.clone()) {
        parameters
            .parameters(source)
            .map_err(|error| PythonSyntaxError::new(node.from(), error.expected()))?;
    }
    if let Ok(arguments) = PythonArgList::downcast_from(node.clone()) {
        arguments
            .call_arguments()
            .map_err(|error| PythonSyntaxError::new(node.from(), error.expected()))?;
    }
    if let Ok(pattern) = PythonMappingPattern::downcast_from(node.clone()) {
        let mapping = pattern
            .mapping()
            .map_err(|error| PythonSyntaxError::new(node.from(), error.expected()))?;
        if let Some(rest) = mapping.rest() {
            let name = rest.name().ok_or(PythonSyntaxError::new(
                rest.syntax().from(),
                "a named mapping rest capture",
            ))?;
            if node_text(name.syntax(), source)? == "_" {
                return Err(PythonSyntaxError::new(
                    name.syntax().from(),
                    "a named mapping rest capture other than wildcard",
                ));
            }
        }
    }
    if let Ok(string) = PythonString::downcast_from(node.clone()) {
        validate_plain_string(&string, source)?;
    }
    if let Ok(string) = PythonContinuedString::downcast_from(node.clone()) {
        validate_continued_string(&string, source)?;
    }
    let escape_context = literal_escape_context(node, source)?.or(inherited_escape_context);
    if node.name().as_ref() == "Escape" {
        let context = escape_context.ok_or(PythonSyntaxError::new(
            node.from(),
            "a string literal around an escape",
        ))?;
        validate_escape(node, source, context)?;
    }
    for child in node.children() {
        validate_node(&child, source, escape_context)?;
    }
    Ok(())
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum LiteralFamily {
    Text,
    Bytes,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum EscapeContext {
    Text,
    Bytes,
    Raw,
}

fn validate_plain_string(string: &PythonString, source: &str) -> Result<(), PythonSyntaxError> {
    let spelling = node_text(string.syntax(), source)?;
    let family = literal_family(spelling).ok_or(PythonSyntaxError::new(
        string.syntax().from(),
        "a quoted Python string literal",
    ))?;
    if family == LiteralFamily::Bytes && !spelling.is_ascii() {
        return Err(PythonSyntaxError::new(
            string.syntax().from(),
            "only ASCII source characters in a bytes literal",
        ));
    }
    Ok(())
}

fn validate_continued_string(
    string: &PythonContinuedString,
    source: &str,
) -> Result<(), PythonSyntaxError> {
    let mut family = None;
    for part in string.parts() {
        let part_family = match part {
            PythonStringPart::String(part) => {
                let spelling = node_text(part.syntax(), source)?;
                literal_family(spelling).ok_or(PythonSyntaxError::new(
                    part.syntax().from(),
                    "a quoted Python string literal",
                ))?
            }
            PythonStringPart::Format(_) | PythonStringPart::Template(_) => LiteralFamily::Text,
        };
        if family.is_some_and(|family| family != part_family) {
            return Err(PythonSyntaxError::new(
                string.syntax().from(),
                "no implicit concatenation of bytes and text literals",
            ));
        }
        family = Some(part_family);
    }
    Ok(())
}

fn literal_family(spelling: &str) -> Option<LiteralFamily> {
    let quote = spelling.find(['\'', '"'])?;
    let prefix = spelling.get(..quote)?;
    Some(if prefix.bytes().any(|byte| matches!(byte, b'b' | b'B')) {
        LiteralFamily::Bytes
    } else {
        LiteralFamily::Text
    })
}

fn literal_escape_context(
    node: &SyntaxNode,
    source: &str,
) -> Result<Option<EscapeContext>, PythonSyntaxError> {
    let literal = PythonString::downcast_from(node.clone()).is_ok()
        || PythonFormatString::downcast_from(node.clone()).is_ok()
        || PythonTemplateString::downcast_from(node.clone()).is_ok();
    if !literal {
        return Ok(None);
    }
    let spelling = node_text(node, source)?;
    let quote = spelling.find(['\'', '"']).ok_or(PythonSyntaxError::new(
        node.from(),
        "a quoted Python string literal",
    ))?;
    let prefix = spelling.get(..quote).ok_or(PythonSyntaxError::new(
        node.from(),
        "a Python string prefix",
    ))?;
    let context = if prefix.bytes().any(|byte| matches!(byte, b'r' | b'R')) {
        EscapeContext::Raw
    } else if prefix.bytes().any(|byte| matches!(byte, b'b' | b'B')) {
        EscapeContext::Bytes
    } else {
        EscapeContext::Text
    };
    Ok(Some(context))
}

fn validate_escape(
    node: &SyntaxNode,
    source: &str,
    context: EscapeContext,
) -> Result<(), PythonSyntaxError> {
    let spelling = node_text(node, source)?;
    if context == EscapeContext::Raw {
        return Ok(());
    }
    let mut characters = spelling.chars();
    if characters.next() != Some('\\') {
        return Err(PythonSyntaxError::new(
            node.from(),
            "a backslash-prefixed string escape",
        ));
    }
    let Some(kind) = characters.next() else {
        return Err(PythonSyntaxError::new(
            node.from(),
            "a complete string escape",
        ));
    };
    let valid = match kind {
        'x' => exact_hex_value(characters, 2).is_some(),
        'u' if context == EscapeContext::Text => exact_hex_value(characters, 4).is_some(),
        'U' if context == EscapeContext::Text => {
            exact_hex_value(characters, 8).is_some_and(|value| value <= 0x10_ffff)
        }
        'N' => {
            let name = characters.collect::<String>();
            if context == EscapeContext::Bytes {
                true
            } else {
                name.strip_prefix('{')
                    .and_then(|name| name.strip_suffix('}'))
                    .is_some_and(|name| crate::unicode_names::character(name).is_some())
            }
        }
        _ => true,
    };
    if valid {
        Ok(())
    } else {
        Err(PythonSyntaxError::new(
            node.from(),
            "a complete Python string escape",
        ))
    }
}

fn exact_hex_value(mut characters: impl Iterator<Item = char>, count: usize) -> Option<u32> {
    let mut value = 0_u32;
    for _ in 0..count {
        value = value.checked_mul(16)?;
        value = value.checked_add(characters.next()?.to_digit(16)?)?;
    }
    characters.next().is_none().then_some(value)
}

fn node_text<'source>(
    node: &SyntaxNode,
    source: &'source str,
) -> Result<&'source str, PythonSyntaxError> {
    source
        .get(usize::from(node.from())..usize::from(node.to()))
        .ok_or(PythonSyntaxError::new(
            node.from(),
            "a CST range on UTF-8 boundaries",
        ))
}

fn validate_assignment(statement: &PythonAssignStatement) -> Result<(), PythonSyntaxError> {
    if statement.type_definition().is_some() {
        let assignment = statement
            .annotated_assignment()
            .map_err(|error| PythonSyntaxError::new(statement.syntax().from(), error.expected()))?;
        return validate_target(assignment.target(), TargetMode::Single);
    }
    let groups = statement
        .assignment_groups()
        .map_err(|error| PythonSyntaxError::new(statement.syntax().from(), error.expected()))?;
    let Some((_, targets)) = groups.split_last() else {
        return Err(PythonSyntaxError::new(
            statement.syntax().from(),
            "an assignment target and value",
        ));
    };
    for target in targets {
        validate_target_items(target.items(), TargetMode::Store, target.range().start())?;
    }
    Ok(())
}

fn validate_update(statement: &PythonUpdateStatement) -> Result<(), PythonSyntaxError> {
    let expressions = statement.expressions().collect::<Vec<_>>();
    let [target, _] = expressions.as_slice() else {
        return Err(PythonSyntaxError::new(
            statement.syntax().from(),
            "one augmented-assignment target and value",
        ));
    };
    validate_target(target, TargetMode::Single)
}

fn validate_delete(statement: &PythonDeleteStatement) -> Result<(), PythonSyntaxError> {
    for target in statement.targets() {
        validate_target(&target, TargetMode::Delete)?;
    }
    Ok(())
}

fn validate_for(statement: &PythonForStatement) -> Result<(), PythonSyntaxError> {
    let parts = statement
        .for_parts()
        .map_err(|error| PythonSyntaxError::new(statement.syntax().from(), error.expected()))?;
    validate_target_items(
        parts.targets(),
        TargetMode::Store,
        statement.syntax().from(),
    )
}

fn validate_with(statement: &PythonWithStatement) -> Result<(), PythonSyntaxError> {
    let parts = statement
        .with_items()
        .map_err(|error| PythonSyntaxError::new(statement.syntax().from(), error.expected()))?;
    for item in parts.items() {
        if let Some(target) = item.target() {
            validate_target_item(target, TargetMode::Store, statement.syntax().from())?;
        }
    }
    Ok(())
}

fn validate_comprehension_expression(
    expression: &PythonExpressionNode,
) -> Result<(), PythonSyntaxError> {
    let comprehension = match expression {
        PythonExpressionNode::Comprehension(node) => Some(node.comprehension()),
        PythonExpressionNode::ArrayComprehension(node) => Some(node.comprehension()),
        PythonExpressionNode::DictionaryComprehension(node) => Some(node.comprehension()),
        PythonExpressionNode::SetComprehension(node) => Some(node.comprehension()),
        _ => None,
    };
    let Some(comprehension) = comprehension else {
        return Ok(());
    };
    let comprehension = comprehension
        .map_err(|error| PythonSyntaxError::new(expression.syntax().from(), error.expected()))?;
    validate_comprehension(&comprehension, expression.syntax().from())
}

fn validate_comprehension(
    comprehension: &PythonComprehension,
    position: TextSize,
) -> Result<(), PythonSyntaxError> {
    for generator in comprehension.generators() {
        validate_target_items(generator.targets(), TargetMode::Store, position)?;
    }
    Ok(())
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum TargetMode {
    Store,
    Single,
    Delete,
}

fn validate_target(
    expression: &PythonExpressionNode,
    mode: TargetMode,
) -> Result<(), PythonSyntaxError> {
    match expression {
        PythonExpressionNode::VariableName(_) | PythonExpressionNode::Member(_) => Ok(()),
        PythonExpressionNode::Parenthesized(parenthesized) => {
            validate_parenthesized_target(parenthesized, mode)
        }
        PythonExpressionNode::Tuple(tuple) => validate_sequence_target(tuple, mode),
        PythonExpressionNode::Array(array) => validate_array_target(array, mode),
        _ => Err(PythonSyntaxError::new(
            expression.syntax().from(),
            "an assignable Python target",
        )),
    }
}

fn validate_parenthesized_target(
    target: &PythonParenthesizedExpression,
    mode: TargetMode,
) -> Result<(), PythonSyntaxError> {
    let items = expression_items(target.syntax())
        .map_err(|error| PythonSyntaxError::new(target.syntax().from(), error.expected()))?;
    let [PythonExpressionItem::Plain(expression)] = items.as_slice() else {
        return Err(PythonSyntaxError::new(
            target.syntax().from(),
            "one unstarred parenthesized target",
        ));
    };
    validate_target(expression, mode)
}

fn validate_sequence_target(
    target: &PythonTupleExpression,
    mode: TargetMode,
) -> Result<(), PythonSyntaxError> {
    if mode == TargetMode::Single {
        return Err(PythonSyntaxError::new(
            target.syntax().from(),
            "one augmented or annotated target",
        ));
    }
    let items = expression_items(target.syntax())
        .map_err(|error| PythonSyntaxError::new(target.syntax().from(), error.expected()))?;
    validate_target_items(&items, mode, target.syntax().from())
}

fn validate_array_target(
    target: &PythonArrayExpression,
    mode: TargetMode,
) -> Result<(), PythonSyntaxError> {
    if mode == TargetMode::Single {
        return Err(PythonSyntaxError::new(
            target.syntax().from(),
            "one augmented or annotated target",
        ));
    }
    let items = expression_items(target.syntax())
        .map_err(|error| PythonSyntaxError::new(target.syntax().from(), error.expected()))?;
    validate_target_items(&items, mode, target.syntax().from())
}

fn validate_target_items(
    items: &[PythonExpressionItem],
    mode: TargetMode,
    position: TextSize,
) -> Result<(), PythonSyntaxError> {
    for item in items {
        validate_target_item(item, mode, position)?;
    }
    Ok(())
}

fn validate_target_item(
    item: &PythonExpressionItem,
    mode: TargetMode,
    position: TextSize,
) -> Result<(), PythonSyntaxError> {
    match item {
        PythonExpressionItem::Plain(expression) => validate_target(expression, mode),
        PythonExpressionItem::Starred { value, .. } if mode == TargetMode::Store => {
            validate_target(value, TargetMode::Store)
        }
        PythonExpressionItem::Starred { .. } => {
            Err(PythonSyntaxError::new(position, "a starred store target"))
        }
    }
}
