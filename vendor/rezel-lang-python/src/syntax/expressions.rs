//! Hand-written expression syntax views.

use rezel_common::{SyntaxNode, TextRange, TextSize, TypedNode};

use crate::{
    PythonArgList, PythonBinaryExpression, PythonDictionaryExpression, PythonExpressionNode,
    PythonUnaryExpression, PythonVariableName,
};

use super::{
    PythonComprehension, PythonCstInvariantError, comprehension, groups::try_for_each_comma_group,
};

#[derive(Clone, Debug)]
pub(crate) enum PythonOperator {
    Symbol(SyntaxNode),
    And,
    Or,
    In,
    NotIn,
    Is,
    IsNot,
    Not,
}

#[derive(Clone, Debug)]
pub(crate) struct PythonUnaryOperation {
    operator: PythonOperator,
    operand: PythonExpressionNode,
}

impl PythonUnaryOperation {
    #[must_use]
    pub(crate) fn operator(&self) -> &PythonOperator {
        &self.operator
    }

    #[must_use]
    pub(crate) fn operand(&self) -> &PythonExpressionNode {
        &self.operand
    }
}

#[derive(Clone, Debug)]
pub(crate) struct PythonBinaryOperation {
    left: PythonExpressionNode,
    operator: PythonOperator,
    right: PythonExpressionNode,
}

impl PythonBinaryOperation {
    #[must_use]
    pub(crate) fn left(&self) -> &PythonExpressionNode {
        &self.left
    }

    #[must_use]
    pub(crate) fn operator(&self) -> &PythonOperator {
        &self.operator
    }

    #[must_use]
    pub(crate) fn right(&self) -> &PythonExpressionNode {
        &self.right
    }
}

/// One operand in a flattened `not`/`and`/`or` expression.
#[derive(Clone, Debug)]
pub(crate) struct PythonBooleanTerm {
    not_starts: Vec<TextSize>,
    expression: PythonExpressionNode,
}

impl PythonBooleanTerm {
    #[must_use]
    pub(crate) fn not_starts(&self) -> &[TextSize] {
        &self.not_starts
    }

    #[must_use]
    pub(crate) fn expression(&self) -> &PythonExpressionNode {
        &self.expression
    }

    #[must_use]
    pub(crate) fn start(&self) -> TextSize {
        self.not_starts
            .first()
            .copied()
            .unwrap_or_else(|| self.expression.syntax().from())
    }

    #[must_use]
    pub(crate) fn end(&self) -> TextSize {
        self.expression.syntax().to()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PythonBooleanOperator {
    And,
    Or,
}

/// A source-ordered boolean expression independent of Lezer's associations.
#[derive(Clone, Debug)]
pub(crate) struct PythonBooleanExpression {
    terms: Vec<PythonBooleanTerm>,
    operators: Vec<PythonBooleanOperator>,
}

impl PythonBooleanExpression {
    #[must_use]
    pub(crate) fn terms(&self) -> &[PythonBooleanTerm] {
        &self.terms
    }

    #[must_use]
    pub(crate) fn operators(&self) -> &[PythonBooleanOperator] {
        &self.operators
    }
}

/// Flatten the maintained CST association before applying Python precedence.
///
/// # Errors
///
/// Returns an error when a strict unary or binary node lacks an operation.
pub(crate) fn boolean_expression(
    expression: &PythonExpressionNode,
) -> Result<Option<PythonBooleanExpression>, PythonCstInvariantError> {
    let mut terms = Vec::new();
    let mut operators = Vec::new();
    flatten_boolean(expression, Vec::new(), &mut terms, &mut operators)?;
    if operators.is_empty() {
        return Ok(None);
    }
    if terms.len() != operators.len() + 1 {
        return Err(PythonCstInvariantError::new(
            "one more boolean term than operator",
        ));
    }
    Ok(Some(PythonBooleanExpression { terms, operators }))
}

fn flatten_boolean(
    expression: &PythonExpressionNode,
    mut not_starts: Vec<TextSize>,
    terms: &mut Vec<PythonBooleanTerm>,
    operators: &mut Vec<PythonBooleanOperator>,
) -> Result<(), PythonCstInvariantError> {
    if let PythonExpressionNode::Unary(unary) = expression {
        let operation = unary.operation()?;
        if matches!(operation.operator(), PythonOperator::Not) {
            not_starts.push(unary.syntax().from());
            return flatten_boolean(operation.operand(), not_starts, terms, operators);
        }
    }
    if let PythonExpressionNode::Binary(binary) = expression {
        let operation = binary.operation()?;
        let operator = match operation.operator() {
            PythonOperator::And => Some(PythonBooleanOperator::And),
            PythonOperator::Or => Some(PythonBooleanOperator::Or),
            _ => None,
        };
        if let Some(operator) = operator {
            flatten_boolean(operation.left(), not_starts, terms, operators)?;
            operators.push(operator);
            return flatten_boolean(operation.right(), Vec::new(), terms, operators);
        }
    }
    terms.push(PythonBooleanTerm {
        not_starts,
        expression: expression.clone(),
    });
    Ok(())
}

impl PythonUnaryExpression {
    /// # Errors
    ///
    /// Returns an error when a strict unary CST lacks its operand or operator.
    pub(crate) fn operation(&self) -> Result<PythonUnaryOperation, PythonCstInvariantError> {
        let mut operand = None;
        let mut operator = OperatorClassifier::default();
        for child in self.syntax().children() {
            if let Ok(expression) = PythonExpressionNode::downcast_from(child.clone()) {
                operand = Some(expression);
                break;
            }
            operator.push(child);
        }
        let operand = operand.ok_or(PythonCstInvariantError::new("a unary operand"))?;
        let operator = operator.finish()?;
        Ok(PythonUnaryOperation { operator, operand })
    }
}

impl PythonBinaryExpression {
    /// # Errors
    ///
    /// Returns an error when a strict binary CST lacks an operand or recognized operator.
    pub(crate) fn operation(&self) -> Result<PythonBinaryOperation, PythonCstInvariantError> {
        let mut left = None;
        let mut right = None;
        let mut operator = OperatorClassifier::default();
        for child in self.syntax().children() {
            if let Ok(expression) = PythonExpressionNode::downcast_from(child.clone()) {
                if left.is_none() {
                    left = Some(expression);
                } else if right.is_none() {
                    right = Some(expression);
                }
                continue;
            }
            if left.is_some() && right.is_none() {
                operator.push(child);
            }
        }
        let left = left.ok_or(PythonCstInvariantError::new("a binary left operand"))?;
        let right = right.ok_or(PythonCstInvariantError::new("a binary right operand"))?;
        let operator = operator.finish()?;
        Ok(PythonBinaryOperation {
            left,
            operator,
            right,
        })
    }
}

#[derive(Clone, Copy)]
enum KeywordOperatorToken {
    And,
    Or,
    In,
    Not,
    Is,
}

#[derive(Default)]
struct OperatorClassifier {
    symbol: Option<SyntaxNode>,
    first: Option<KeywordOperatorToken>,
    second: Option<KeywordOperatorToken>,
    overflow: bool,
}

impl OperatorClassifier {
    fn push(&mut self, child: SyntaxNode) {
        let token = match child.name().as_ref() {
            "ArithOp" | "BitOp" | "CompareOp" => {
                if self.symbol.replace(child).is_some() {
                    self.overflow = true;
                }
                return;
            }
            "and" => KeywordOperatorToken::And,
            "or" => KeywordOperatorToken::Or,
            "in" => KeywordOperatorToken::In,
            "not" => KeywordOperatorToken::Not,
            "is" => KeywordOperatorToken::Is,
            _ => return,
        };
        if self.first.is_none() {
            self.first = Some(token);
        } else if self.second.is_none() {
            self.second = Some(token);
        } else {
            self.overflow = true;
        }
    }

    fn finish(self) -> Result<PythonOperator, PythonCstInvariantError> {
        if self.overflow || (self.symbol.is_some() && self.first.is_some()) {
            return Err(PythonCstInvariantError::new(
                "exactly one recognized Python operator",
            ));
        }
        if let Some(symbol) = self.symbol {
            return Ok(PythonOperator::Symbol(symbol));
        }
        match (self.first, self.second) {
            (Some(KeywordOperatorToken::And), None) => Ok(PythonOperator::And),
            (Some(KeywordOperatorToken::Or), None) => Ok(PythonOperator::Or),
            (Some(KeywordOperatorToken::In), None) => Ok(PythonOperator::In),
            (Some(KeywordOperatorToken::Not), None) => Ok(PythonOperator::Not),
            (Some(KeywordOperatorToken::Not), Some(KeywordOperatorToken::In)) => {
                Ok(PythonOperator::NotIn)
            }
            (Some(KeywordOperatorToken::Is), None) => Ok(PythonOperator::Is),
            (Some(KeywordOperatorToken::Is), Some(KeywordOperatorToken::Not)) => {
                Ok(PythonOperator::IsNot)
            }
            _ => Err(PythonCstInvariantError::new("a recognized Python operator")),
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) enum PythonDictionaryEntry {
    Pair {
        key: PythonExpressionNode,
        value: PythonExpressionNode,
    },
    Unpack {
        value: PythonExpressionNode,
    },
}

impl PythonDictionaryExpression {
    /// # Errors
    ///
    /// Returns an error when a strict dictionary CST contains an incomplete item shape.
    pub(crate) fn dictionary_entries(
        &self,
    ) -> Result<Vec<PythonDictionaryEntry>, PythonCstInvariantError> {
        let mut entries = Vec::new();
        try_for_each_comma_group(self.syntax(), &["(", ")", "{", "}"], |group| {
            entries.push(dictionary_entry(group)?);
            Ok(())
        })?;
        Ok(entries)
    }
}

fn dictionary_entry(
    group: &[SyntaxNode],
) -> Result<PythonDictionaryEntry, PythonCstInvariantError> {
    let mut first = None;
    let mut second = None;
    let mut overflow = false;
    for child in group {
        let Ok(expression) = PythonExpressionNode::downcast_from(child.clone()) else {
            continue;
        };
        if first.is_none() {
            first = Some(expression);
        } else if second.is_none() {
            second = Some(expression);
        } else {
            overflow = true;
        }
    }
    let unpack = group
        .first()
        .is_some_and(|child| child.name().as_ref() == "**");
    match (unpack, first, second, overflow) {
        (false, Some(key), Some(value), false) => Ok(PythonDictionaryEntry::Pair { key, value }),
        (true, Some(value), None, false) => Ok(PythonDictionaryEntry::Unpack { value }),
        _ => Err(PythonCstInvariantError::new(
            "a dictionary pair or unpacking item",
        )),
    }
}

#[derive(Clone, Debug)]
pub(crate) enum PythonCallArgument {
    Positional(PythonExpressionNode),
    Starred {
        value: PythonExpressionNode,
        range: TextRange,
    },
    Assigned {
        name: PythonVariableName,
        operator: SyntaxNode,
        value: PythonExpressionNode,
        range: TextRange,
    },
    KeywordUnpack {
        value: PythonExpressionNode,
        range: TextRange,
    },
    Generator {
        comprehension: PythonComprehension,
        range: TextRange,
    },
}

impl PythonArgList {
    /// # Errors
    ///
    /// Returns an error when a strict argument-list CST contains an incomplete item shape.
    pub(crate) fn call_arguments(
        &self,
    ) -> Result<Vec<PythonCallArgument>, PythonCstInvariantError> {
        let mut arguments = Vec::new();
        try_for_each_call_group(self.syntax(), |group| {
            arguments.push(call_argument(self, group)?);
            Ok(())
        })?;
        if arguments
            .iter()
            .any(|argument| matches!(argument, PythonCallArgument::Generator { .. }))
            && arguments.len() != 1
        {
            return Err(PythonCstInvariantError::new(
                "a generator expression as the sole call argument",
            ));
        }
        let mut saw_keyword = false;
        let mut saw_keyword_unpack = false;
        for argument in &arguments {
            match argument {
                PythonCallArgument::Positional(_) => {
                    if saw_keyword || saw_keyword_unpack {
                        return Err(PythonCstInvariantError::new(
                            "no positional argument after a keyword argument",
                        ));
                    }
                }
                PythonCallArgument::Starred { .. } => {
                    if saw_keyword_unpack {
                        return Err(PythonCstInvariantError::new(
                            "no iterable unpacking after keyword unpacking",
                        ));
                    }
                }
                PythonCallArgument::Assigned { operator, .. } => {
                    if operator.range().len() == TextSize::from(1) {
                        saw_keyword = true;
                    }
                }
                PythonCallArgument::KeywordUnpack { .. } => saw_keyword_unpack = true,
                PythonCallArgument::Generator { .. } => {}
            }
        }
        Ok(arguments)
    }
}

/// Visit logical call arguments in the flattened `ArgList` CST.
///
/// A generator expression has no persistent wrapper in the maintained Lezer
/// tree, so commas after its first `for` belong to comprehension targets and
/// iterables rather than to the surrounding call. This scanner keeps that
/// grammar state while retaining the one-pass, transient shape-adapter model.
fn try_for_each_call_group(
    node: &SyntaxNode,
    mut visit: impl FnMut(&[SyntaxNode]) -> Result<(), PythonCstInvariantError>,
) -> Result<(), PythonCstInvariantError> {
    let mut group = Vec::new();
    let mut in_comprehension = false;
    let mut after_comprehension_in = false;
    for child in node.children() {
        let name = child.name();
        if matches!(name.as_ref(), "Comment" | "(" | ")" | "{" | "}") {
            continue;
        }
        if name.as_ref() == "for" {
            in_comprehension = true;
        }
        if in_comprehension && name.as_ref() == "in" {
            after_comprehension_in = true;
        }
        if name.as_ref() == "," && (!in_comprehension || after_comprehension_in) {
            if !group.is_empty() {
                visit(&group)?;
                group.clear();
            }
            continue;
        }
        group.push(child);
    }
    if !group.is_empty() {
        visit(&group)?;
    }
    Ok(())
}

fn call_argument(
    arguments: &PythonArgList,
    group: &[SyntaxNode],
) -> Result<PythonCallArgument, PythonCstInvariantError> {
    let mut first = None;
    let mut second = None;
    let mut overflow = false;
    let mut operator = None;
    let mut generator = false;
    for child in group {
        let name = child.name();
        if name.as_ref() == "for" {
            generator = true;
        } else if name.as_ref() == "AssignOp" && operator.replace(child.clone()).is_some() {
            overflow = true;
        }
        let Ok(expression) = PythonExpressionNode::downcast_from(child.clone()) else {
            continue;
        };
        if first.is_none() {
            first = Some(expression);
        } else if second.is_none() {
            second = Some(expression);
        } else {
            overflow = true;
        }
    }
    if generator {
        return Ok(PythonCallArgument::Generator {
            comprehension: comprehension(arguments.syntax(), false)?,
            range: arguments.syntax().range(),
        });
    }
    let range = group_range(group)?;
    let marker = group.first().map(SyntaxNode::name);
    if marker.as_deref() == Some("*") {
        let (Some(value), None, false) = (first, second, overflow) else {
            return Err(PythonCstInvariantError::new(
                "one expression after a call star",
            ));
        };
        return Ok(PythonCallArgument::Starred {
            value: value.clone(),
            range,
        });
    }
    if marker.as_deref() == Some("**") {
        let (Some(value), None, false) = (first, second, overflow) else {
            return Err(PythonCstInvariantError::new(
                "one expression after a call double star",
            ));
        };
        return Ok(PythonCallArgument::KeywordUnpack {
            value: value.clone(),
            range,
        });
    }
    if let Some(operator) = operator {
        let (Some(name), Some(value), false) = (first, second, overflow) else {
            return Err(PythonCstInvariantError::new(
                "a name and value around a call assignment",
            ));
        };
        let PythonExpressionNode::VariableName(name) = name else {
            return Err(PythonCstInvariantError::new(
                "a variable before a call assignment",
            ));
        };
        return Ok(PythonCallArgument::Assigned {
            name: name.clone(),
            operator: operator.clone(),
            value: value.clone(),
            range,
        });
    }
    let (Some(value), None, false) = (first, second, overflow) else {
        return Err(PythonCstInvariantError::new(
            "one positional call expression",
        ));
    };
    Ok(PythonCallArgument::Positional(value))
}

fn group_range(group: &[SyntaxNode]) -> Result<TextRange, PythonCstInvariantError> {
    let first = group
        .first()
        .ok_or(PythonCstInvariantError::new("a non-empty comma group"))?;
    let last = group
        .last()
        .ok_or(PythonCstInvariantError::new("a non-empty comma group"))?;
    Ok(TextRange::new(first.from(), last.to()))
}
