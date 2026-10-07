//! Stable grammatical views over flattened Python syntax productions.
//!
//! New shape adapters are split by grammatical family. Existing adapters are
//! being migrated out of this module as the AST lowerer stops consuming raw
//! syntax nodes.

mod declarations;
mod expressions;
mod groups;
mod patterns;
mod statements;
mod strings;
#[cfg(test)]
mod tests;
mod validate;

pub(crate) use declarations::{PythonTypeParameter, PythonTypeParameterKind};
pub(crate) use expressions::{
    PythonBooleanExpression, PythonBooleanOperator, PythonBooleanTerm, PythonCallArgument,
    PythonDictionaryEntry, PythonOperator, boolean_expression,
};
pub(crate) use patterns::{PythonMappingKey, PythonSequencePatternView};
pub(crate) use strings::{
    PythonInterpolatedLiteralKind, PythonInterpolatedPart, PythonInterpolatedString,
    PythonInterpolatedStringKind, PythonInterpolation, format_spec_parts,
};
pub(crate) use validate::validate_syntax;

use rezel_common::{SyntaxNode, TextRange, TextSize, TypedNode};

use crate::{
    PythonArrayComprehensionExpression, PythonAssignStatement, PythonBody,
    PythonComprehensionExpression, PythonDictionaryComprehensionExpression, PythonExpressionNode,
    PythonExpressionStatement, PythonForStatement, PythonIfStatement, PythonImportStatement,
    PythonMatchBody, PythonMatchClause, PythonMemberExpression, PythonParamList,
    PythonPropertyName, PythonReturnStatement, PythonScopeStatement,
    PythonSetComprehensionExpression, PythonStatementNode, PythonTryStatement, PythonTypeDef,
    PythonVariableName, PythonWhileStatement, PythonYieldExpression, PythonYieldStatement,
};

/// A direct expression item, with unpacking preserved independently of the
/// underlying flattened grammar production.
#[derive(Clone, Debug)]
pub(crate) enum PythonExpressionItem {
    Plain(PythonExpressionNode),
    Starred {
        star_token: SyntaxNode,
        value: PythonExpressionNode,
    },
}

impl PythonExpressionItem {
    #[must_use]
    pub(crate) fn range(&self) -> TextRange {
        match self {
            Self::Plain(value) => value.syntax().range(),
            Self::Starred { star_token, value } => {
                TextRange::new(star_token.from(), value.syntax().to())
            }
        }
    }
}

/// One comma-separated expression group between assignment operators.
#[derive(Clone, Debug)]
pub(crate) struct PythonExpressionGroup {
    items: Vec<PythonExpressionItem>,
    range: TextRange,
    sequence: bool,
}

/// The normalized fields of an annotation assignment.
#[derive(Clone, Debug)]
pub(crate) struct PythonAnnotatedAssignment {
    target: PythonExpressionNode,
    annotation: PythonExpressionNode,
    value: Option<PythonExpressionNode>,
    simple: bool,
}

impl PythonAnnotatedAssignment {
    #[must_use]
    pub(crate) fn target(&self) -> &PythonExpressionNode {
        &self.target
    }

    #[must_use]
    pub(crate) fn annotation(&self) -> &PythonExpressionNode {
        &self.annotation
    }

    #[must_use]
    pub(crate) fn value(&self) -> Option<&PythonExpressionNode> {
        self.value.as_ref()
    }

    #[must_use]
    pub(crate) const fn is_simple(&self) -> bool {
        self.simple
    }
}

/// The syntactic operation represented by one `MemberExpression`.
///
/// Member expressions are recursive, so `base` contains all operations to the
/// left of the final attribute or subscript operation.
#[derive(Clone, Debug)]
pub(crate) struct PythonMemberAccess {
    base: PythonExpressionNode,
    suffix: PythonMemberSuffix,
}

impl PythonMemberAccess {
    #[must_use]
    pub(crate) fn base(&self) -> &PythonExpressionNode {
        &self.base
    }

    #[must_use]
    pub(crate) fn suffix(&self) -> &PythonMemberSuffix {
        &self.suffix
    }
}

/// The final operation of a `MemberExpression`.
#[derive(Clone, Debug)]
pub(crate) enum PythonMemberSuffix {
    Attribute(PythonPropertyName),
    Subscript(PythonSubscriptList),
}

/// The comma-separated items inside one subscript pair.
#[derive(Clone, Debug)]
pub(crate) struct PythonSubscriptList {
    items: Vec<PythonSubscriptItem>,
    range: TextRange,
}

impl PythonSubscriptList {
    #[must_use]
    pub(crate) fn items(&self) -> &[PythonSubscriptItem] {
        &self.items
    }

    #[must_use]
    pub(crate) const fn range(&self) -> TextRange {
        self.range
    }
}

/// One index or slice in a subscript list.
#[derive(Clone, Debug)]
pub(crate) enum PythonSubscriptItem {
    Index(PythonExpressionItem),
    Slice(PythonSlice),
}

impl PythonSubscriptItem {
    #[must_use]
    pub(crate) fn range(&self) -> TextRange {
        match self {
            Self::Index(expression) => expression.range(),
            Self::Slice(slice) => slice.range(),
        }
    }
}

/// A slice with the distinction between omitted bounds preserved.
#[derive(Clone, Debug)]
pub(crate) struct PythonSlice {
    lower: Option<PythonExpressionNode>,
    upper: Option<PythonExpressionNode>,
    step: Option<PythonExpressionNode>,
    range: TextRange,
}

impl PythonSlice {
    #[must_use]
    pub(crate) fn lower(&self) -> Option<&PythonExpressionNode> {
        self.lower.as_ref()
    }

    #[must_use]
    pub(crate) fn upper(&self) -> Option<&PythonExpressionNode> {
        self.upper.as_ref()
    }

    #[must_use]
    pub(crate) fn step(&self) -> Option<&PythonExpressionNode> {
        self.step.as_ref()
    }

    #[must_use]
    pub(crate) const fn range(&self) -> TextRange {
        self.range
    }
}

/// `CPython`'s syntactic parameter groups, independent of the flattened
/// `ParamList` production.
#[derive(Clone, Debug)]
pub(crate) struct PythonParameters {
    positional_only: Vec<PythonParameter>,
    positional_or_keyword: Vec<PythonParameter>,
    vararg: Option<PythonParameter>,
    keyword_only: Vec<PythonParameter>,
    kwarg: Option<PythonParameter>,
}

impl PythonParameters {
    #[must_use]
    pub(crate) fn positional_only(&self) -> &[PythonParameter] {
        &self.positional_only
    }

    #[must_use]
    pub(crate) fn positional_or_keyword(&self) -> &[PythonParameter] {
        &self.positional_or_keyword
    }

    #[must_use]
    pub(crate) fn vararg(&self) -> Option<&PythonParameter> {
        self.vararg.as_ref()
    }

    #[must_use]
    pub(crate) fn keyword_only(&self) -> &[PythonParameter] {
        &self.keyword_only
    }

    #[must_use]
    pub(crate) fn kwarg(&self) -> Option<&PythonParameter> {
        self.kwarg.as_ref()
    }
}

/// One named parameter with its optional annotation and default.
#[derive(Clone, Debug)]
pub(crate) struct PythonParameter {
    name: PythonVariableName,
    annotation: Option<PythonExpressionNode>,
    default: Option<PythonExpressionNode>,
}

/// The value expression before the first comprehension `for` clause.
#[derive(Clone, Debug)]
pub(crate) enum PythonComprehensionHead {
    Element(PythonExpressionItem),
    KeyValue {
        key: PythonExpressionNode,
        value: PythonExpressionNode,
    },
    DictionaryUnpack,
}

/// A normalized comprehension independent of Lezer's flattened `compFor` and
/// `compIf` helper productions.
#[derive(Clone, Debug)]
pub(crate) struct PythonComprehension {
    head: PythonComprehensionHead,
    generators: Vec<PythonComprehensionGenerator>,
}

impl PythonComprehension {
    #[must_use]
    pub(crate) fn head(&self) -> &PythonComprehensionHead {
        &self.head
    }

    #[must_use]
    pub(crate) fn generators(&self) -> &[PythonComprehensionGenerator] {
        &self.generators
    }
}

/// One `for` clause and the immediately following `if` filters.
#[derive(Clone, Debug)]
pub(crate) struct PythonComprehensionGenerator {
    asynchronous: bool,
    targets: Vec<PythonExpressionItem>,
    target_sequence: bool,
    iterator: PythonExpressionNode,
    filters: Vec<PythonExpressionNode>,
}

/// A normalized `try` statement with each clause assigned its grammatical role.
#[derive(Clone, Debug)]
pub(crate) struct PythonTry {
    body: PythonBody,
    handlers: Vec<PythonExceptClause>,
    orelse: Option<PythonBody>,
    finalbody: Option<PythonBody>,
}

impl PythonTry {
    #[must_use]
    pub(crate) fn body(&self) -> &PythonBody {
        &self.body
    }

    #[must_use]
    pub(crate) fn handlers(&self) -> &[PythonExceptClause] {
        &self.handlers
    }

    #[must_use]
    pub(crate) fn orelse(&self) -> Option<&PythonBody> {
        self.orelse.as_ref()
    }

    #[must_use]
    pub(crate) fn finalbody(&self) -> Option<&PythonBody> {
        self.finalbody.as_ref()
    }
}

/// One `except` or `except*` clause.
#[derive(Clone, Debug)]
pub(crate) struct PythonExceptClause {
    exception_group: bool,
    types: Vec<PythonExpressionNode>,
    name: Option<PythonVariableName>,
    body: PythonBody,
    range: TextRange,
}

/// One `if` or `elif` condition and body.
#[derive(Clone, Debug)]
pub(crate) struct PythonConditionalClause {
    start: rezel_common::TextSize,
    test: PythonExpressionNode,
    body: PythonBody,
}

impl PythonConditionalClause {
    #[must_use]
    pub(crate) const fn start(&self) -> rezel_common::TextSize {
        self.start
    }

    #[must_use]
    pub(crate) fn test(&self) -> &PythonExpressionNode {
        &self.test
    }

    #[must_use]
    pub(crate) fn body(&self) -> &PythonBody {
        &self.body
    }
}

/// A normalized `if`/`elif`/`else` chain.
#[derive(Clone, Debug)]
pub(crate) struct PythonConditional {
    clauses: Vec<PythonConditionalClause>,
    orelse: Option<PythonBody>,
}

impl PythonConditional {
    #[must_use]
    pub(crate) fn clauses(&self) -> &[PythonConditionalClause] {
        &self.clauses
    }

    #[must_use]
    pub(crate) fn orelse(&self) -> Option<&PythonBody> {
        self.orelse.as_ref()
    }
}

/// A normalized `while` statement.
#[derive(Clone, Debug)]
pub(crate) struct PythonWhile {
    test: PythonExpressionNode,
    body: PythonBody,
    orelse: Option<PythonBody>,
}

impl PythonWhile {
    #[must_use]
    pub(crate) fn test(&self) -> &PythonExpressionNode {
        &self.test
    }

    #[must_use]
    pub(crate) fn body(&self) -> &PythonBody {
        &self.body
    }

    #[must_use]
    pub(crate) fn orelse(&self) -> Option<&PythonBody> {
        self.orelse.as_ref()
    }
}

/// A normalized synchronous or asynchronous `for` statement.
#[derive(Clone, Debug)]
pub(crate) struct PythonFor {
    asynchronous: bool,
    targets: Vec<PythonExpressionItem>,
    target_sequence: bool,
    iterators: Vec<PythonExpressionNode>,
    body: PythonBody,
    orelse: Option<PythonBody>,
}

/// A normalized import statement.
#[derive(Clone, Debug)]
pub(crate) struct PythonImport {
    from: Option<PythonImportFrom>,
    aliases: Vec<PythonImportAlias>,
}

impl PythonImport {
    #[must_use]
    pub(crate) fn from(&self) -> Option<&PythonImportFrom> {
        self.from.as_ref()
    }

    #[must_use]
    pub(crate) fn aliases(&self) -> &[PythonImportAlias] {
        &self.aliases
    }
}

/// The relative level and optional module of a `from` import.
#[derive(Clone, Debug)]
pub(crate) struct PythonImportFrom {
    level: usize,
    module: Vec<PythonVariableName>,
}

impl PythonImportFrom {
    #[must_use]
    pub(crate) const fn level(&self) -> usize {
        self.level
    }

    #[must_use]
    pub(crate) fn module(&self) -> &[PythonVariableName] {
        &self.module
    }
}

/// One dotted or wildcard import alias.
#[derive(Clone, Debug)]
pub(crate) struct PythonImportAlias {
    name: Vec<PythonVariableName>,
    wildcard: bool,
    as_name: Option<PythonVariableName>,
    range: TextRange,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PythonScopeKind {
    Global,
    Nonlocal,
}

/// The normalized shape of a yield expression or statement.
#[derive(Clone, Debug)]
pub(crate) struct PythonYield {
    from: bool,
    group: Option<PythonExpressionGroup>,
}

impl PythonYield {
    #[must_use]
    pub(crate) const fn is_from(&self) -> bool {
        self.from
    }

    #[must_use]
    pub(crate) fn group(&self) -> Option<&PythonExpressionGroup> {
        self.group.as_ref()
    }
}

impl PythonImportAlias {
    #[must_use]
    pub(crate) fn name(&self) -> &[PythonVariableName] {
        &self.name
    }

    #[must_use]
    pub(crate) const fn is_wildcard(&self) -> bool {
        self.wildcard
    }

    #[must_use]
    pub(crate) fn as_name(&self) -> Option<&PythonVariableName> {
        self.as_name.as_ref()
    }

    #[must_use]
    pub(crate) const fn range(&self) -> TextRange {
        self.range
    }
}

impl PythonFor {
    #[must_use]
    pub(crate) const fn is_async(&self) -> bool {
        self.asynchronous
    }

    #[must_use]
    pub(crate) fn targets(&self) -> &[PythonExpressionItem] {
        &self.targets
    }

    #[must_use]
    pub(crate) const fn target_is_sequence(&self) -> bool {
        self.target_sequence
    }

    #[must_use]
    pub(crate) fn iterators(&self) -> &[PythonExpressionNode] {
        &self.iterators
    }

    #[must_use]
    pub(crate) fn body(&self) -> &PythonBody {
        &self.body
    }

    #[must_use]
    pub(crate) fn orelse(&self) -> Option<&PythonBody> {
        self.orelse.as_ref()
    }
}

impl PythonExceptClause {
    #[must_use]
    pub(crate) const fn is_exception_group(&self) -> bool {
        self.exception_group
    }

    #[must_use]
    pub(crate) fn types(&self) -> &[PythonExpressionNode] {
        &self.types
    }

    #[must_use]
    pub(crate) fn name(&self) -> Option<&PythonVariableName> {
        self.name.as_ref()
    }

    #[must_use]
    pub(crate) fn body(&self) -> &PythonBody {
        &self.body
    }

    #[must_use]
    pub(crate) const fn range(&self) -> TextRange {
        self.range
    }
}

impl PythonComprehensionGenerator {
    #[must_use]
    pub(crate) const fn is_async(&self) -> bool {
        self.asynchronous
    }

    #[must_use]
    pub(crate) fn targets(&self) -> &[PythonExpressionItem] {
        &self.targets
    }

    #[must_use]
    pub(crate) const fn target_is_sequence(&self) -> bool {
        self.target_sequence
    }

    #[must_use]
    pub(crate) fn iterator(&self) -> &PythonExpressionNode {
        &self.iterator
    }

    #[must_use]
    pub(crate) fn filters(&self) -> &[PythonExpressionNode] {
        &self.filters
    }
}

impl PythonParameter {
    #[must_use]
    pub(crate) fn name(&self) -> &PythonVariableName {
        &self.name
    }

    #[must_use]
    pub(crate) fn annotation(&self) -> Option<&PythonExpressionNode> {
        self.annotation.as_ref()
    }

    #[must_use]
    pub(crate) fn default(&self) -> Option<&PythonExpressionNode> {
        self.default.as_ref()
    }
}

impl PythonExpressionGroup {
    #[must_use]
    pub(crate) fn items(&self) -> &[PythonExpressionItem] {
        &self.items
    }

    #[must_use]
    pub(crate) const fn range(&self) -> TextRange {
        self.range
    }

    /// Whether commas make this group an implicit tuple shape.
    #[must_use]
    pub(crate) const fn is_sequence(&self) -> bool {
        self.sequence
    }
}

/// A violated invariant between a strict CST and its generated typed model.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct PythonCstInvariantError {
    expected: &'static str,
}

impl PythonCstInvariantError {
    const fn new(expected: &'static str) -> Self {
        Self { expected }
    }

    #[must_use]
    pub(crate) const fn expected(self) -> &'static str {
        self.expected
    }
}

impl std::fmt::Display for PythonCstInvariantError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "Python CST expected {}", self.expected)
    }
}

impl std::error::Error for PythonCstInvariantError {}

impl PythonAssignStatement {
    /// Return assignment targets followed by the value as stable expression
    /// groups. Annotation assignments intentionally use their dedicated typed
    /// fields instead.
    ///
    /// # Errors
    ///
    /// Returns an error for an annotated or structurally incomplete assignment.
    pub(crate) fn assignment_groups(
        &self,
    ) -> Result<Vec<PythonExpressionGroup>, PythonCstInvariantError> {
        if self.type_definition().is_some() {
            return Err(PythonCstInvariantError::new("an unannotated assignment"));
        }
        expression_groups(self.syntax(), "AssignOp")
    }

    /// Return the `CPython` roles of an annotation assignment.
    ///
    /// # Errors
    ///
    /// Returns an error when a strict annotation assignment lacks a required role.
    pub(crate) fn annotated_assignment(
        &self,
    ) -> Result<PythonAnnotatedAssignment, PythonCstInvariantError> {
        let definition = self
            .type_definition()
            .ok_or(PythonCstInvariantError::new("an annotation assignment"))?;
        let annotation = definition
            .annotation()
            .ok_or(PythonCstInvariantError::new("an annotation expression"))?;
        let expressions = self.expressions().collect::<Vec<_>>();
        let ([target] | [target, _]) = expressions.as_slice() else {
            return Err(PythonCstInvariantError::new(
                "one target and an optional value in an annotation assignment",
            ));
        };
        let value = expressions.get(1).cloned();
        Ok(PythonAnnotatedAssignment {
            simple: matches!(target, PythonExpressionNode::VariableName(_)),
            target: target.clone(),
            annotation,
            value,
        })
    }
}

impl PythonExpressionStatement {
    /// Preserve comma grouping for an implicit tuple expression.
    ///
    /// # Errors
    ///
    /// Returns an error when the statement has no expression.
    pub(crate) fn expression_group(
        &self,
    ) -> Result<PythonExpressionGroup, PythonCstInvariantError> {
        expression_group(self.syntax())?
            .ok_or(PythonCstInvariantError::new("an expression statement"))
    }
}

impl PythonReturnStatement {
    /// Preserve comma grouping and starred values after `return`.
    ///
    /// # Errors
    ///
    /// Returns an error when a star marker lacks a following expression.
    pub(crate) fn return_group(
        &self,
    ) -> Result<Option<PythonExpressionGroup>, PythonCstInvariantError> {
        expression_group(self.syntax())
    }
}

impl PythonMemberExpression {
    /// Interpret the flattened Lezer member production as one stable access.
    ///
    /// # Errors
    ///
    /// Returns an error when a strict member CST lacks its base or suffix.
    pub(crate) fn access(&self) -> Result<PythonMemberAccess, PythonCstInvariantError> {
        member_access(self)
    }
}

impl PythonParamList {
    /// Interpret `/`, bare `*`, `*args`, and `**kwargs` as `CPython` parameter
    /// groups without adding punctuation-only nodes to the persistent CST.
    ///
    /// # Errors
    ///
    /// Returns an error when source gaps or strict parameter roles are inconsistent.
    pub(crate) fn parameters(
        &self,
        source: &str,
    ) -> Result<PythonParameters, PythonCstInvariantError> {
        parameters(self, source)
    }
}

impl PythonTryStatement {
    /// Assign flattened bodies and exception syntax to stable clause roles.
    ///
    /// # Errors
    ///
    /// Returns an error when a strict `try` CST has inconsistent clauses.
    pub(crate) fn clauses(&self) -> Result<PythonTry, PythonCstInvariantError> {
        try_clauses(self)
    }
}

impl PythonIfStatement {
    /// # Errors
    ///
    /// Returns an error when a strict conditional CST lacks a condition or body.
    pub(crate) fn conditional(&self) -> Result<PythonConditional, PythonCstInvariantError> {
        conditional(self)
    }
}

impl PythonWhileStatement {
    /// # Errors
    ///
    /// Returns an error when a strict `while` CST lacks a condition or body.
    pub(crate) fn while_parts(&self) -> Result<PythonWhile, PythonCstInvariantError> {
        while_parts(self)
    }
}

impl PythonForStatement {
    /// # Errors
    ///
    /// Returns an error when a strict `for` CST lacks targets, iterators, or a body.
    pub(crate) fn for_parts(&self) -> Result<PythonFor, PythonCstInvariantError> {
        for_parts(self)
    }
}

impl PythonImportStatement {
    /// Recover omitted relative-import punctuation from source gaps without
    /// increasing the persistent CST shape.
    ///
    /// # Errors
    ///
    /// Returns an error when source gaps or strict import roles are inconsistent.
    pub(crate) fn import(&self, source: &str) -> Result<PythonImport, PythonCstInvariantError> {
        import_parts(self, source)
    }
}

impl PythonScopeStatement {
    /// # Errors
    ///
    /// Returns an error when the strict CST contains neither scope keyword.
    pub(crate) fn scope_kind(&self) -> Result<PythonScopeKind, PythonCstInvariantError> {
        let kind = self
            .syntax()
            .children()
            .find_map(|child| match child.name().as_ref() {
                "global" => Some(PythonScopeKind::Global),
                "nonlocal" => Some(PythonScopeKind::Nonlocal),
                _ => None,
            });
        kind.ok_or(PythonCstInvariantError::new("a global or nonlocal keyword"))
    }
}

impl PythonYieldStatement {
    /// # Errors
    ///
    /// Returns an error when a strict yield statement has inconsistent expression items.
    pub(crate) fn yield_parts(&self) -> Result<PythonYield, PythonCstInvariantError> {
        yield_parts(self.syntax())
    }
}

impl PythonYieldExpression {
    /// # Errors
    ///
    /// Returns an error when a strict yield expression has inconsistent expression items.
    pub(crate) fn yield_parts(&self) -> Result<PythonYield, PythonCstInvariantError> {
        yield_parts(self.syntax())
    }
}

macro_rules! impl_comprehension_view {
    ($node:ty, $dictionary:literal) => {
        impl $node {
            /// # Errors
            ///
            /// Returns an error when a strict comprehension CST has inconsistent clauses.
            pub(crate) fn comprehension(
                &self,
            ) -> Result<PythonComprehension, PythonCstInvariantError> {
                comprehension(self.syntax(), $dictionary)
            }
        }
    };
}

impl_comprehension_view!(PythonComprehensionExpression, false);
impl_comprehension_view!(PythonArrayComprehensionExpression, false);
impl_comprehension_view!(PythonDictionaryComprehensionExpression, true);
impl_comprehension_view!(PythonSetComprehensionExpression, false);

/// Collect direct expression items without losing `*` markers.
///
/// # Errors
///
/// Returns an error when a star marker lacks a following expression.
pub(crate) fn expression_items(
    node: &SyntaxNode,
) -> Result<Vec<PythonExpressionItem>, PythonCstInvariantError> {
    collect_group(node.children())
}

/// Preserve the tuple-shaping comma and its source range for a direct expression group.
///
/// # Errors
///
/// Returns an error when a star marker lacks a following expression.
pub(crate) fn expression_group(
    node: &SyntaxNode,
) -> Result<Option<PythonExpressionGroup>, PythonCstInvariantError> {
    expression_group_from_children(&node.children().collect::<Vec<_>>())
}

/// End of the final statement content, excluding trailing layout tokens.
#[must_use]
pub(crate) fn statement_content_end(node: &SyntaxNode) -> TextSize {
    node.children()
        .filter_map(|child| {
            let statement = PythonStatementNode::downcast_from(child.clone()).is_ok();
            let body = PythonBody::downcast_from(child.clone()).is_ok();
            let match_body = PythonMatchBody::downcast_from(child.clone()).is_ok();
            let match_clause = PythonMatchClause::downcast_from(child.clone()).is_ok();
            (statement || body || match_body || match_clause).then(|| statement_content_end(&child))
        })
        .last()
        .unwrap_or_else(|| node.to())
}

fn expression_groups(
    node: &SyntaxNode,
    separator: &str,
) -> Result<Vec<PythonExpressionGroup>, PythonCstInvariantError> {
    let mut groups = Vec::new();
    let mut children = Vec::new();
    for child in node.children() {
        if child.name().as_ref() == separator {
            groups.push(group_from_children(&children)?);
            children.clear();
        } else {
            children.push(child);
        }
    }
    groups.push(group_from_children(&children)?);
    if groups.len() < 2 {
        return Err(PythonCstInvariantError::new(
            "an assignment target and value",
        ));
    }
    Ok(groups)
}

fn group_from_children(
    children: &[SyntaxNode],
) -> Result<PythonExpressionGroup, PythonCstInvariantError> {
    expression_group_from_children(children)?
        .ok_or(PythonCstInvariantError::new("an expression group"))
}

fn expression_group_from_children(
    children: &[SyntaxNode],
) -> Result<Option<PythonExpressionGroup>, PythonCstInvariantError> {
    let sequence = children.iter().any(|child| child.name().as_ref() == ",");
    let items = collect_group(children.iter().cloned())?;
    let Some(first) = items.first() else {
        return Ok(None);
    };
    let last = items.last().expect("first item was present");
    let start = first.range().start();
    let mut end = last.range().end();
    if sequence {
        for comma in children.iter().filter(|child| child.name().as_ref() == ",") {
            if comma.from() >= start {
                end = end.max(comma.to());
            }
        }
    }
    let range = TextRange::new(start, end);
    Ok(Some(PythonExpressionGroup {
        items,
        range,
        sequence,
    }))
}

fn collect_group(
    children: impl IntoIterator<Item = SyntaxNode>,
) -> Result<Vec<PythonExpressionItem>, PythonCstInvariantError> {
    let mut items = Vec::new();
    let mut star_token = None;
    for child in children {
        if child.name().as_ref() == "*" {
            if star_token.replace(child).is_some() {
                return Err(PythonCstInvariantError::new(
                    "one star marker before an expression",
                ));
            }
            continue;
        }
        let Ok(expression) = PythonExpressionNode::downcast_from(child) else {
            continue;
        };
        let item = match star_token.take() {
            Some(star_token) => PythonExpressionItem::Starred {
                star_token,
                value: expression,
            },
            None => PythonExpressionItem::Plain(expression),
        };
        items.push(item);
    }
    if star_token.is_some() {
        return Err(PythonCstInvariantError::new(
            "an expression after a star marker",
        ));
    }
    Ok(items)
}

fn member_access(
    node: &PythonMemberExpression,
) -> Result<PythonMemberAccess, PythonCstInvariantError> {
    let base = node
        .base()
        .ok_or(PythonCstInvariantError::new("a member base expression"))?;
    let children = node.syntax().children().collect::<Vec<_>>();
    let base_index = children
        .iter()
        .position(|child| {
            child.from() == base.syntax().from()
                && child.to() == base.syntax().to()
                && PythonExpressionNode::downcast_from(child.clone()).is_ok()
        })
        .ok_or(PythonCstInvariantError::new(
            "the typed member base in its CST",
        ))?;
    let suffix = &children[base_index + 1..];
    let property = suffix
        .iter()
        .find_map(|child| PythonPropertyName::downcast_from(child.clone()).ok());
    let suffix = if let Some(property) = property {
        if !suffix.iter().any(|child| child.name().as_ref() == ".") {
            return Err(PythonCstInvariantError::new(
                "a dot before an attribute name",
            ));
        }
        PythonMemberSuffix::Attribute(property)
    } else {
        PythonMemberSuffix::Subscript(subscript_list(suffix)?)
    };
    Ok(PythonMemberAccess { base, suffix })
}

fn subscript_list(children: &[SyntaxNode]) -> Result<PythonSubscriptList, PythonCstInvariantError> {
    let Some(open) = children
        .iter()
        .position(|child| child.name().as_ref() == "[")
    else {
        return Err(PythonCstInvariantError::new("an opening subscript bracket"));
    };
    let Some(close) = children
        .iter()
        .rposition(|child| child.name().as_ref() == "]")
    else {
        return Err(PythonCstInvariantError::new("a closing subscript bracket"));
    };
    if close <= open + 1 {
        return Err(PythonCstInvariantError::new("a non-empty subscript"));
    }

    let mut items = Vec::new();
    let mut start = open + 1;
    for index in open + 1..=close {
        let at_end = index == close;
        let at_comma = children[index].name().as_ref() == ",";
        if !at_end && !at_comma {
            continue;
        }
        if start < index {
            items.push(subscript_item(&children[start..index])?);
        } else if !at_end {
            return Err(PythonCstInvariantError::new(
                "a subscript item before a comma",
            ));
        }
        start = index + 1;
    }
    let first = items
        .first()
        .ok_or(PythonCstInvariantError::new("a subscript item"))?;
    let last = items.last().expect("first item was present");
    let range = TextRange::new(first.range().start(), last.range().end());
    Ok(PythonSubscriptList { items, range })
}

fn subscript_item(children: &[SyntaxNode]) -> Result<PythonSubscriptItem, PythonCstInvariantError> {
    let colon_count = children
        .iter()
        .filter(|child| child.name().as_ref() == ":")
        .count();
    if colon_count == 0 {
        let expressions = collect_group(children.iter().cloned())?;
        let [expression] = expressions.as_slice() else {
            return Err(PythonCstInvariantError::new("one expression in an index"));
        };
        return Ok(PythonSubscriptItem::Index(expression.clone()));
    }
    if colon_count > 2 {
        return Err(PythonCstInvariantError::new(
            "at most two colons in a slice",
        ));
    }

    let mut slots: [Option<PythonExpressionNode>; 3] = [None, None, None];
    let mut slot = 0;
    for child in children {
        if child.name().as_ref() == ":" {
            slot += 1;
            continue;
        }
        let Ok(expression) = PythonExpressionNode::downcast_from(child.clone()) else {
            continue;
        };
        if slots[slot].replace(expression).is_some() {
            return Err(PythonCstInvariantError::new(
                "one expression per slice bound",
            ));
        }
    }
    let first = children
        .first()
        .ok_or(PythonCstInvariantError::new("a slice"))?;
    let last = children
        .last()
        .ok_or(PythonCstInvariantError::new("a slice"))?;
    let [lower, upper, step] = slots;
    Ok(PythonSubscriptItem::Slice(PythonSlice {
        lower,
        upper,
        step,
        range: TextRange::new(first.from(), last.to()),
    }))
}

fn parameters(
    node: &PythonParamList,
    source: &str,
) -> Result<PythonParameters, PythonCstInvariantError> {
    let mut positional = Vec::new();
    let mut positional_only = Vec::new();
    let mut vararg = None;
    let mut keyword_only = Vec::new();
    let mut kwarg = None;
    let mut after_star = false;
    let mut bare_star = false;
    let mut saw_positional_default = false;
    let mut saw_positional_separator = false;

    try_for_each_source_group(node.syntax(), source, |children, source_children, range| {
        let omitted = omitted_spelling(source, range, source_children)?;
        if omitted == "/" {
            if saw_positional_separator || after_star || positional.is_empty() {
                return Err(PythonCstInvariantError::new(
                    "one positional-only separator after positional parameters",
                ));
            }
            saw_positional_separator = true;
            positional_only.append(&mut positional);
            return Ok(());
        }
        if !omitted.is_empty() {
            return Err(PythonCstInvariantError::new(
                "only recognized punctuation in a parameter source gap",
            ));
        }

        let marker = children.first().map(SyntaxNode::name);
        if marker.as_deref() == Some("*") {
            if after_star || kwarg.is_some() {
                return Err(PythonCstInvariantError::new("one star separator"));
            }
            after_star = true;
            if children
                .iter()
                .any(|child| PythonVariableName::downcast_from(child.clone()).is_ok())
            {
                vararg = Some(parameter_from_group(children)?);
            } else {
                bare_star = true;
            }
            return Ok(());
        }
        if marker.as_deref() == Some("**") {
            if kwarg.is_some() {
                return Err(PythonCstInvariantError::new(
                    "one keyword variadic parameter",
                ));
            }
            if bare_star && keyword_only.is_empty() {
                return Err(PythonCstInvariantError::new(
                    "a named keyword-only parameter after a bare star",
                ));
            }
            after_star = true;
            kwarg = Some(parameter_from_group(children)?);
            return Ok(());
        }

        let parameter = parameter_from_group(children)?;
        if kwarg.is_some() {
            return Err(PythonCstInvariantError::new(
                "no parameter after a keyword variadic parameter",
            ));
        }
        if after_star {
            keyword_only.push(parameter);
        } else {
            if parameter.default().is_some() {
                saw_positional_default = true;
            } else if saw_positional_default {
                return Err(PythonCstInvariantError::new(
                    "no positional parameter without a default after one with a default",
                ));
            }
            positional.push(parameter);
        }
        Ok(())
    })?;

    if bare_star && keyword_only.is_empty() {
        return Err(PythonCstInvariantError::new(
            "a named keyword-only parameter after a bare star",
        ));
    }

    Ok(PythonParameters {
        positional_only,
        positional_or_keyword: positional,
        vararg,
        keyword_only,
        kwarg,
    })
}

fn try_for_each_source_group(
    node: &SyntaxNode,
    source: &str,
    mut visit: impl FnMut(
        &[SyntaxNode],
        &[SyntaxNode],
        TextRange,
    ) -> Result<(), PythonCstInvariantError>,
) -> Result<(), PythonCstInvariantError> {
    let mut group = Vec::new();
    let mut source_group = Vec::new();
    let mut start = node.from();
    for child in node.children() {
        match child.name().as_ref() {
            "(" => start = child.to(),
            ")" => {
                let range = TextRange::new(start, child.from());
                let omitted = omitted_spelling(source, range, &source_group)?;
                if !group.is_empty() || !omitted.is_empty() {
                    visit(&group, &source_group, range)?;
                    group.clear();
                    source_group.clear();
                }
            }
            "," => {
                visit(&group, &source_group, TextRange::new(start, child.from()))?;
                group.clear();
                source_group.clear();
                start = child.to();
            }
            "Comment" => source_group.push(child),
            _ => {
                source_group.push(child.clone());
                group.push(child);
            }
        }
    }
    if !group.is_empty() {
        visit(&group, &source_group, TextRange::new(start, node.to()))?;
    }
    Ok(())
}

/// Return non-layout source bytes not represented by direct CST children.
///
/// Lezer intentionally omits some anonymous punctuation. Computing this view
/// only during lowering preserves that information without storing extra tree
/// nodes. Child ranges (including comments) are excluded before spelling is
/// interpreted by a grammatical adapter.
fn omitted_spelling(
    source: &str,
    range: TextRange,
    children: &[SyntaxNode],
) -> Result<String, PythonCstInvariantError> {
    let start = usize::from(range.start());
    let end = usize::from(range.end());
    let Some(_) = source.get(start..end) else {
        return Err(PythonCstInvariantError::new(
            "a source range on UTF-8 boundaries",
        ));
    };
    let mut spelling = String::new();
    let mut cursor = start;
    for child in children {
        let child_start = usize::from(child.from()).clamp(start, end);
        let child_end = usize::from(child.to()).clamp(start, end);
        if cursor < child_start {
            let Some(fragment) = source.get(cursor..child_start) else {
                return Err(PythonCstInvariantError::new(
                    "a source gap on UTF-8 boundaries",
                ));
            };
            append_non_layout(fragment, &mut spelling);
        }
        cursor = cursor.max(child_end);
    }
    if cursor < end {
        let Some(fragment) = source.get(cursor..end) else {
            return Err(PythonCstInvariantError::new(
                "a source gap on UTF-8 boundaries",
            ));
        };
        append_non_layout(fragment, &mut spelling);
    }
    Ok(spelling)
}

fn append_non_layout(fragment: &str, spelling: &mut String) {
    let mut characters = fragment.chars().peekable();
    while let Some(character) = characters.next() {
        if character.is_whitespace() {
            continue;
        }
        if character == '\\'
            && characters
                .peek()
                .is_some_and(|next| matches!(next, '\n' | '\r'))
        {
            let newline = characters.next();
            if newline == Some('\r') && characters.peek() == Some(&'\n') {
                characters.next();
            }
            continue;
        }
        spelling.push(character);
    }
}

fn parameter_from_group(group: &[SyntaxNode]) -> Result<PythonParameter, PythonCstInvariantError> {
    let name = group
        .iter()
        .find_map(|child| PythonVariableName::downcast_from(child.clone()).ok())
        .ok_or(PythonCstInvariantError::new("a parameter name"))?;
    let annotation = group
        .iter()
        .find_map(|child| PythonTypeDef::downcast_from(child.clone()).ok())
        .and_then(|definition| definition.annotation());
    let mut defaults = group
        .iter()
        .filter(|child| child.from() != name.syntax().from() || child.to() != name.syntax().to())
        .filter_map(|child| PythonExpressionNode::downcast_from(child.clone()).ok());
    let default = defaults.next();
    if defaults.next().is_some() {
        return Err(PythonCstInvariantError::new(
            "at most one parameter default",
        ));
    }
    Ok(PythonParameter {
        name,
        annotation,
        default,
    })
}

fn comprehension(
    node: &SyntaxNode,
    dictionary: bool,
) -> Result<PythonComprehension, PythonCstInvariantError> {
    let children = node.children().collect::<Vec<_>>();
    let first_for = children
        .iter()
        .position(|child| child.name().as_ref() == "for")
        .ok_or(PythonCstInvariantError::new(
            "a for clause in a comprehension",
        ))?;
    let head_end = first_for
        .checked_sub(1)
        .filter(|index| children[*index].name().as_ref() == "async")
        .unwrap_or(first_for);
    let head = comprehension_head(&children[..head_end], dictionary)?;
    let mut generators = Vec::new();
    let mut index = head_end;

    while index < children.len() {
        let asynchronous = children[index].name().as_ref() == "async";
        if asynchronous {
            index += 1;
        }
        if children
            .get(index)
            .is_none_or(|child| child.name().as_ref() != "for")
        {
            return Err(PythonCstInvariantError::new(
                "for after an optional async in a comprehension",
            ));
        }
        index += 1;

        let target_start = index;
        while index < children.len() && children[index].name().as_ref() != "in" {
            index += 1;
        }
        let target_children = &children[target_start..index];
        let target_sequence = target_children
            .iter()
            .any(|child| child.name().as_ref() == ",");
        let targets = collect_group(target_children.iter().cloned())?;
        if targets.is_empty() {
            return Err(PythonCstInvariantError::new(
                "a target before comprehension in",
            ));
        }
        if index == children.len() {
            return Err(PythonCstInvariantError::new(
                "in after a comprehension target",
            ));
        }
        index += 1;
        let iterator = next_comprehension_expression(&children, &mut index).ok_or(
            PythonCstInvariantError::new("an iterator after comprehension in"),
        )?;

        let mut filters = Vec::new();
        while index < children.len() && children[index].name().as_ref() == "if" {
            index += 1;
            let filter = next_comprehension_expression(&children, &mut index).ok_or(
                PythonCstInvariantError::new("an expression after comprehension if"),
            )?;
            filters.push(filter);
        }
        while index < children.len() && !matches!(children[index].name().as_ref(), "async" | "for")
        {
            index += 1;
        }
        generators.push(PythonComprehensionGenerator {
            asynchronous,
            targets,
            target_sequence,
            iterator,
            filters,
        });
    }
    if generators.is_empty() {
        return Err(PythonCstInvariantError::new(
            "at least one comprehension generator",
        ));
    }
    Ok(PythonComprehension { head, generators })
}

fn comprehension_head(
    children: &[SyntaxNode],
    dictionary: bool,
) -> Result<PythonComprehensionHead, PythonCstInvariantError> {
    let expressions = children
        .iter()
        .filter_map(|child| PythonExpressionNode::downcast_from(child.clone()).ok())
        .collect::<Vec<_>>();
    if dictionary {
        if children.iter().any(|child| child.name().as_ref() == "**") {
            let value = expressions
                .into_iter()
                .next()
                .ok_or(PythonCstInvariantError::new(
                    "an expression after dictionary unpacking",
                ))?;
            let _ = value;
            return Ok(PythonComprehensionHead::DictionaryUnpack);
        }
        let [key, value] = expressions.as_slice() else {
            return Err(PythonCstInvariantError::new(
                "a key and value before a dictionary comprehension",
            ));
        };
        return Ok(PythonComprehensionHead::KeyValue {
            key: key.clone(),
            value: value.clone(),
        });
    }
    let mut items = collect_group(children.iter().cloned())?;
    if items.len() != 1 {
        return Err(PythonCstInvariantError::new(
            "one element before a comprehension",
        ));
    }
    Ok(PythonComprehensionHead::Element(items.remove(0)))
}

fn next_comprehension_expression(
    children: &[SyntaxNode],
    index: &mut usize,
) -> Option<PythonExpressionNode> {
    while *index < children.len() {
        if matches!(children[*index].name().as_ref(), "async" | "for" | "if") {
            return None;
        }
        let expression = PythonExpressionNode::downcast_from(children[*index].clone()).ok();
        *index += 1;
        if expression.is_some() {
            return expression;
        }
    }
    None
}

fn try_clauses(node: &PythonTryStatement) -> Result<PythonTry, PythonCstInvariantError> {
    let children = node.syntax().children().collect::<Vec<_>>();
    let body = children
        .iter()
        .find_map(|child| PythonBody::downcast_from(child.clone()).ok())
        .ok_or(PythonCstInvariantError::new("a try body"))?;
    let mut handlers = Vec::new();
    let mut orelse = None;
    let mut finalbody = None;
    let mut index = 0;

    while index < children.len() {
        match children[index].name().as_ref() {
            "except" => {
                let start = children[index].from();
                index += 1;
                let exception_group = children
                    .get(index)
                    .is_some_and(|child| child.name().as_ref() == "*");
                if exception_group {
                    index += 1;
                }
                let mut types = Vec::new();
                let mut name = None;
                let mut binding = false;
                let clause_body = loop {
                    let child = children.get(index).ok_or(PythonCstInvariantError::new(
                        "a body after an except clause",
                    ))?;
                    if let Ok(body) = PythonBody::downcast_from(child.clone()) {
                        index += 1;
                        break body;
                    }
                    if child.name().as_ref() == "as" {
                        binding = true;
                        index += 1;
                        continue;
                    }
                    if binding {
                        if name.is_some() {
                            return Err(PythonCstInvariantError::new("one name after except as"));
                        }
                        name = PythonVariableName::downcast_from(child.clone()).ok();
                    } else if let Ok(exception_type) =
                        PythonExpressionNode::downcast_from(child.clone())
                    {
                        types.push(exception_type);
                    }
                    index += 1;
                };
                if binding && name.is_none() {
                    return Err(PythonCstInvariantError::new("a name after except as"));
                }
                if exception_group && types.is_empty() {
                    return Err(PythonCstInvariantError::new(
                        "an exception type after except star",
                    ));
                }
                let range = TextRange::new(start, clause_body.syntax().to());
                handlers.push(PythonExceptClause {
                    exception_group,
                    types,
                    name,
                    body: clause_body,
                    range,
                });
            }
            "else" => {
                index += 1;
                orelse = Some(next_body(&children, &mut index, "an else body")?);
            }
            "finally" => {
                index += 1;
                finalbody = Some(next_body(&children, &mut index, "a finally body")?);
            }
            _ => index += 1,
        }
    }
    if handlers.is_empty() && finalbody.is_none() {
        return Err(PythonCstInvariantError::new("an except or finally clause"));
    }
    Ok(PythonTry {
        body,
        handlers,
        orelse,
        finalbody,
    })
}

fn next_body(
    children: &[SyntaxNode],
    index: &mut usize,
    expected: &'static str,
) -> Result<PythonBody, PythonCstInvariantError> {
    while *index < children.len() {
        let body = PythonBody::downcast_from(children[*index].clone()).ok();
        *index += 1;
        if let Some(body) = body {
            return Ok(body);
        }
    }
    Err(PythonCstInvariantError::new(expected))
}

fn conditional(node: &PythonIfStatement) -> Result<PythonConditional, PythonCstInvariantError> {
    let children = node.syntax().children().collect::<Vec<_>>();
    let mut clauses = Vec::new();
    let mut orelse = None;
    let mut index = 0;
    while index < children.len() {
        match children[index].name().as_ref() {
            "if" | "elif" => {
                let start = children[index].from();
                index += 1;
                let test = next_expression(&children, &mut index, "an if condition")?;
                let body = next_body(&children, &mut index, "an if body")?;
                clauses.push(PythonConditionalClause { start, test, body });
            }
            "else" => {
                index += 1;
                orelse = Some(next_body(&children, &mut index, "an else body")?);
            }
            _ => index += 1,
        }
    }
    if clauses.is_empty() {
        return Err(PythonCstInvariantError::new("an if clause"));
    }
    Ok(PythonConditional { clauses, orelse })
}

fn while_parts(node: &PythonWhileStatement) -> Result<PythonWhile, PythonCstInvariantError> {
    let children = node.syntax().children().collect::<Vec<_>>();
    let mut index = children
        .iter()
        .position(|child| child.name().as_ref() == "while")
        .ok_or(PythonCstInvariantError::new("a while keyword"))?
        + 1;
    let test = next_expression(&children, &mut index, "a while condition")?;
    let body = next_body(&children, &mut index, "a while body")?;
    let orelse = children
        .iter()
        .position(|child| child.name().as_ref() == "else")
        .map(|position| {
            let mut position = position + 1;
            next_body(&children, &mut position, "a while else body")
        })
        .transpose()?;
    Ok(PythonWhile { test, body, orelse })
}

fn for_parts(node: &PythonForStatement) -> Result<PythonFor, PythonCstInvariantError> {
    let children = node.syntax().children().collect::<Vec<_>>();
    let in_index = children
        .iter()
        .position(|child| child.name().as_ref() == "in")
        .ok_or(PythonCstInvariantError::new("in in a for statement"))?;
    let body_index = children
        .iter()
        .position(|child| PythonBody::downcast_from(child.clone()).is_ok())
        .ok_or(PythonCstInvariantError::new("a for body"))?;
    if body_index <= in_index {
        return Err(PythonCstInvariantError::new(
            "a for iterator before its body",
        ));
    }
    let targets = collect_group(children[..in_index].iter().cloned())?;
    if targets.is_empty() {
        return Err(PythonCstInvariantError::new("a for target"));
    }
    let target_sequence = children[..in_index]
        .iter()
        .any(|child| child.name().as_ref() == ",");
    let iterators = children[in_index + 1..body_index]
        .iter()
        .filter_map(|child| PythonExpressionNode::downcast_from(child.clone()).ok())
        .collect::<Vec<_>>();
    if iterators.is_empty() {
        return Err(PythonCstInvariantError::new("a for iterator"));
    }
    let body = PythonBody::downcast_from(children[body_index].clone())
        .map_err(|_| PythonCstInvariantError::new("a for body"))?;
    let orelse = children[body_index + 1..]
        .iter()
        .position(|child| child.name().as_ref() == "else")
        .map(|position| {
            let mut position = body_index + position + 2;
            next_body(&children, &mut position, "a for else body")
        })
        .transpose()?;
    let asynchronous = children
        .iter()
        .take(in_index)
        .any(|child| child.name().as_ref() == "async");
    Ok(PythonFor {
        asynchronous,
        targets,
        target_sequence,
        iterators,
        body,
        orelse,
    })
}

fn next_expression(
    children: &[SyntaxNode],
    index: &mut usize,
    expected: &'static str,
) -> Result<PythonExpressionNode, PythonCstInvariantError> {
    while *index < children.len() {
        let expression = PythonExpressionNode::downcast_from(children[*index].clone()).ok();
        *index += 1;
        if let Some(expression) = expression {
            return Ok(expression);
        }
    }
    Err(PythonCstInvariantError::new(expected))
}

fn import_parts(
    node: &PythonImportStatement,
    source: &str,
) -> Result<PythonImport, PythonCstInvariantError> {
    let children = node.syntax().children().collect::<Vec<_>>();
    let import_index = children
        .iter()
        .position(|child| child.name().as_ref() == "import")
        .ok_or(PythonCstInvariantError::new("an import keyword"))?;
    let is_from = children[..import_index]
        .iter()
        .any(|child| child.name().as_ref() == "from");
    let from = if is_from {
        let mut module = Vec::new();
        for child in &children[..import_index] {
            if child.name().as_ref() == "from" {
                continue;
            }
            if let Ok(name) = PythonVariableName::downcast_from(child.clone()) {
                module.push(name);
            }
        }
        let from_keyword = children
            .iter()
            .find(|child| child.name().as_ref() == "from")
            .ok_or(PythonCstInvariantError::new("a from keyword"))?;
        let prefix_end = module
            .first()
            .map_or(children[import_index].from(), |name| name.syntax().from());
        let prefix_range = TextRange::new(from_keyword.to(), prefix_end);
        let prefix_children = children[..import_index]
            .iter()
            .filter(|child| {
                child.name().as_ref() == "Comment"
                    && child.from() >= prefix_range.start()
                    && child.to() <= prefix_range.end()
            })
            .cloned()
            .collect::<Vec<_>>();
        let prefix = omitted_spelling(source, prefix_range, &prefix_children)?;
        if !prefix.chars().all(|character| character == '.') {
            return Err(PythonCstInvariantError::new(
                "only relative-import dots before a module",
            ));
        }
        let level = prefix.len();
        Some(PythonImportFrom { level, module })
    } else {
        None
    };

    let aliases = import_alias_groups(&children[import_index + 1..])?
        .iter()
        .map(|group| import_alias(group))
        .collect::<Result<Vec<_>, _>>()?;
    if aliases.is_empty() {
        return Err(PythonCstInvariantError::new("at least one import alias"));
    }
    Ok(PythonImport { from, aliases })
}

fn import_alias_groups(
    children: &[SyntaxNode],
) -> Result<Vec<Vec<SyntaxNode>>, PythonCstInvariantError> {
    let mut groups = Vec::new();
    let mut group = Vec::new();
    for child in children {
        match child.name().as_ref() {
            "(" | ")" | "Comment" => {}
            "," => {
                if group.is_empty() {
                    return Err(PythonCstInvariantError::new(
                        "an import alias before a comma",
                    ));
                }
                groups.push(std::mem::take(&mut group));
            }
            _ => group.push(child.clone()),
        }
    }
    if !group.is_empty() {
        groups.push(group);
    }
    Ok(groups)
}

fn import_alias(children: &[SyntaxNode]) -> Result<PythonImportAlias, PythonCstInvariantError> {
    let as_index = children
        .iter()
        .position(|child| child.name().as_ref() == "as");
    let name_end = as_index.unwrap_or(children.len());
    let name = children[..name_end]
        .iter()
        .filter_map(|child| PythonVariableName::downcast_from(child.clone()).ok())
        .collect::<Vec<_>>();
    let wildcard = children[..name_end]
        .iter()
        .any(|child| child.name().as_ref() == "*");
    if name.is_empty() != wildcard {
        return Err(PythonCstInvariantError::new(
            "one dotted name or wildcard import",
        ));
    }
    let as_name = as_index
        .and_then(|index| children.get(index + 1..))
        .and_then(|children| {
            children
                .iter()
                .find_map(|child| PythonVariableName::downcast_from(child.clone()).ok())
        });
    if as_index.is_some() && as_name.is_none() {
        return Err(PythonCstInvariantError::new("a name after import as"));
    }
    let first = children
        .first()
        .ok_or(PythonCstInvariantError::new("an import alias"))?;
    let last = children
        .last()
        .ok_or(PythonCstInvariantError::new("an import alias"))?;
    Ok(PythonImportAlias {
        name,
        wildcard,
        as_name,
        range: TextRange::new(first.from(), last.to()),
    })
}

fn yield_parts(node: &SyntaxNode) -> Result<PythonYield, PythonCstInvariantError> {
    let from = node.children().any(|child| child.name().as_ref() == "from");
    let group = expression_group(node)?;
    if from {
        let Some(group) = &group else {
            return Err(PythonCstInvariantError::new(
                "one expression after yield from",
            ));
        };
        if group.is_sequence() || group.items().len() != 1 {
            return Err(PythonCstInvariantError::new(
                "one expression after yield from",
            ));
        }
    }
    Ok(PythonYield { from, group })
}
