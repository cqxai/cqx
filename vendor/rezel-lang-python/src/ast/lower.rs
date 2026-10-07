use rezel_common::{IterMode, SyntaxNode, TextSize, Tree, TypedNode};

use super::model::{
    AstBuilder, AstError, AstNodeId, PythonAst, PythonAstOptions, PythonAstValue, PythonConstant,
    PythonSourceRange,
};
use super::{PythonAstField, PythonAstKind};
use crate::identifier::canonical_name;
use crate::syntax::{
    PythonBooleanExpression, PythonBooleanOperator, PythonBooleanTerm, PythonCallArgument,
    PythonComprehension, PythonComprehensionGenerator, PythonComprehensionHead,
    PythonDictionaryEntry, PythonExceptClause, PythonExpressionGroup, PythonExpressionItem,
    PythonImportAlias, PythonInterpolatedLiteralKind, PythonInterpolatedPart,
    PythonInterpolatedString, PythonInterpolatedStringKind, PythonInterpolation, PythonMappingKey,
    PythonMemberSuffix, PythonOperator, PythonParameter, PythonParameters, PythonScopeKind,
    PythonSequencePatternView, PythonSlice, PythonSubscriptItem, PythonTypeParameter,
    PythonTypeParameterKind, PythonYield, boolean_expression, expression_items, format_spec_parts,
    statement_content_end, validate_syntax,
};
use crate::typed::{
    PythonArgList, PythonAssertStatement, PythonAssignStatement, PythonAttributePattern,
    PythonAwaitExpression, PythonBinaryExpression, PythonBody, PythonCallExpression,
    PythonCapturePattern, PythonClassDefinition, PythonContinuedString, PythonDecoratedStatement,
    PythonDecorator, PythonDictionaryExpression, PythonExpression as PythonExpressionTop,
    PythonExpressionNode, PythonForStatement, PythonFormatSpec, PythonFunctionDefinition,
    PythonFunctionType, PythonIfStatement, PythonInteractive, PythonLambdaExpression,
    PythonLiteralPattern, PythonMatchClause, PythonMatchStatement, PythonMemberExpression,
    PythonModule, PythonNamedExpression, PythonPatternNode, PythonRaiseStatement,
    PythonScopeStatement, PythonStatementNode, PythonStringPart, PythonTop, PythonTypeDefinition,
    PythonTypeParamList, PythonUnaryExpression, PythonUpdateStatement, PythonWhileStatement,
    PythonWithStatement,
};

impl PythonAst {
    /// Lower one strict Python CST with default `CPython` AST options.
    ///
    /// # Errors
    ///
    /// Returns an error for recovery trees, forward-looking CST-only extensions,
    /// and maintained CST kinds whose `CPython` projection is not implemented.
    pub fn lower(tree: &Tree, source: &str) -> Result<Self, AstError> {
        Self::lower_with_options(tree, source, PythonAstOptions::default())
    }

    /// Lower one strict Python CST with explicit AST options.
    ///
    /// # Errors
    ///
    /// Returns an error for recovery trees, forward-looking CST-only extensions,
    /// and maintained CST kinds whose `CPython` projection is not implemented.
    pub fn lower_with_options(
        tree: &Tree,
        source: &str,
        options: PythonAstOptions,
    ) -> Result<Self, AstError> {
        Lowerer::new(source, options)?.lower(tree)
    }
}

struct Lowerer<'source> {
    source: &'source str,
    options: PythonAstOptions,
    ast: AstBuilder,
}

impl<'source> Lowerer<'source> {
    fn new(source: &'source str, options: PythonAstOptions) -> Result<Self, AstError> {
        TextSize::try_from(source.len()).map_err(|_| AstError::SourceTooLarge)?;
        Ok(Self {
            source,
            options,
            ast: AstBuilder::new(),
        })
    }

    fn lower(mut self, tree: &Tree) -> Result<PythonAst, AstError> {
        reject_recovery_tree(tree)?;
        validate_syntax(tree, self.source).map_err(|error| AstError::InvalidSyntax {
            position: error.position(),
            message: error.message(),
        })?;
        let top = tree.top_node();
        let top = PythonTop::downcast_from(top).map_err(|node| AstError::UnsupportedSyntax {
            kind: node.name().to_string(),
        })?;
        let root = match &top {
            PythonTop::Module(module) => self.lower_module(module)?,
            PythonTop::Expression(expression) => self.lower_expression_top(expression)?,
            PythonTop::Interactive(interactive) => self.lower_interactive(interactive)?,
            PythonTop::FunctionType(function_type) => self.lower_function_type(function_type)?,
        };
        Ok(self.ast.finish(root))
    }

    fn lower_module(&mut self, node: &PythonModule) -> Result<AstNodeId, AstError> {
        let root = self
            .ast
            .push_node(PythonAstKind::Module, PythonSourceRange::default())?;
        let mut body = Vec::new();
        for statement in node.statements() {
            body.extend(self.lower_statement_items(statement.syntax())?);
        }
        self.ast
            .push_field(root, PythonAstField::Body, PythonAstValue::Nodes(body))?;
        let type_ignores = if self.options.type_comments {
            self.lower_type_ignores()?
        } else {
            Vec::new()
        };
        self.ast.push_field(
            root,
            PythonAstField::TypeIgnores,
            PythonAstValue::Nodes(type_ignores),
        )?;
        Ok(root)
    }

    fn lower_expression_top(&mut self, node: &PythonExpressionTop) -> Result<AstNodeId, AstError> {
        let child = node.body().ok_or(AstError::InconsistentCst {
            context: "Expression",
            expected: "expression child",
        })?;
        let value = self.lower_expression(child.syntax(), PythonAstKind::Load)?;
        let root = self
            .ast
            .push_node(PythonAstKind::Expression, PythonSourceRange::default())?;
        self.ast
            .push_field(root, PythonAstField::Body, PythonAstValue::Node(value))?;
        Ok(root)
    }

    fn lower_interactive(&mut self, node: &PythonInteractive) -> Result<AstNodeId, AstError> {
        let root = self
            .ast
            .push_node(PythonAstKind::Interactive, PythonSourceRange::default())?;
        let body = node
            .statement()
            .map(|statement| self.lower_statement_items(statement.syntax()))
            .transpose()?
            .unwrap_or_default();
        self.ast
            .push_field(root, PythonAstField::Body, PythonAstValue::Nodes(body))?;
        Ok(root)
    }

    fn lower_function_type(&mut self, node: &PythonFunctionType) -> Result<AstNodeId, AstError> {
        let mut expressions = node.expressions().collect::<Vec<_>>();
        let returns = expressions.pop().ok_or(AstError::InconsistentCst {
            context: "FunctionType",
            expected: "return type",
        })?;
        let arguments = expressions
            .iter()
            .map(|argument| self.lower_expression(argument.syntax(), PythonAstKind::Load))
            .collect::<Result<Vec<_>, _>>()?;
        let returns = self.lower_expression(returns.syntax(), PythonAstKind::Load)?;
        let root = self
            .ast
            .push_node(PythonAstKind::FunctionType, PythonSourceRange::default())?;
        self.ast.push_field(
            root,
            PythonAstField::Argtypes,
            PythonAstValue::Nodes(arguments),
        )?;
        self.ast
            .push_field(root, PythonAstField::Returns, PythonAstValue::Node(returns))?;
        Ok(root)
    }

    #[allow(clippy::too_many_lines)] // Linear exhaustive syntax dispatch is easier to audit.
    fn lower_statement(&mut self, node: &SyntaxNode) -> Result<AstNodeId, AstError> {
        let statement = PythonStatementNode::downcast_from(node.clone()).map_err(|_| {
            AstError::InconsistentCst {
                context: "statement",
                expected: "typed statement node",
            }
        })?;
        match &statement {
            PythonStatementNode::Assign(statement) => {
                if statement.type_definition().is_some() {
                    self.lower_annotated_assign(statement)
                } else {
                    self.lower_assign(statement)
                }
            }
            PythonStatementNode::Update(statement) => self.lower_augmented_assign(statement),
            PythonStatementNode::Expression(statement) => {
                let group =
                    statement
                        .expression_group()
                        .map_err(|error| AstError::InconsistentCst {
                            context: "ExpressionStatement",
                            expected: error.expected(),
                        })?;
                let value = self.lower_expression_group(&group, PythonAstKind::Load)?;
                let result = self.ast.push_node(
                    PythonAstKind::Expr,
                    PythonSourceRange::from(statement.syntax().range()),
                )?;
                self.ast
                    .push_field(result, PythonAstField::Value, PythonAstValue::Node(value))?;
                Ok(result)
            }
            PythonStatementNode::Return(statement) => {
                let group =
                    statement
                        .return_group()
                        .map_err(|error| AstError::InconsistentCst {
                            context: "ReturnStatement",
                            expected: error.expected(),
                        })?;
                let value = group
                    .as_ref()
                    .map(|group| self.lower_expression_group(group, PythonAstKind::Load))
                    .transpose()?;
                let result = self.ast.push_node(
                    PythonAstKind::Return,
                    PythonSourceRange::from(statement.syntax().range()),
                )?;
                self.ast.push_field(
                    result,
                    PythonAstField::Value,
                    value.map_or(PythonAstValue::None, PythonAstValue::Node),
                )?;
                Ok(result)
            }
            PythonStatementNode::Delete(statement) => {
                let targets = statement
                    .targets()
                    .map(|target| self.lower_expression(target.syntax(), PythonAstKind::Del))
                    .collect::<Result<Vec<_>, _>>()?;
                let result = self
                    .ast
                    .push_node(PythonAstKind::Delete, statement.syntax().range().into())?;
                self.ast.push_field(
                    result,
                    PythonAstField::Targets,
                    PythonAstValue::Nodes(targets),
                )?;
                Ok(result)
            }
            PythonStatementNode::Pass(statement) => self
                .ast
                .push_node(PythonAstKind::Pass, statement.syntax().range().into()),
            PythonStatementNode::Break(statement) => self
                .ast
                .push_node(PythonAstKind::Break, statement.syntax().range().into()),
            PythonStatementNode::Continue(statement) => self
                .ast
                .push_node(PythonAstKind::Continue, statement.syntax().range().into()),
            PythonStatementNode::Scope(statement) => self.lower_scope(statement),
            PythonStatementNode::Assert(statement) => self.lower_assert(statement),
            PythonStatementNode::Raise(statement) => self.lower_raise(statement),
            PythonStatementNode::Import(statement) => self.lower_import(statement),
            PythonStatementNode::TypeDefinition(statement) => self.lower_type_alias(statement),
            PythonStatementNode::Yield(statement) => {
                let parts = statement
                    .yield_parts()
                    .map_err(|error| AstError::InconsistentCst {
                        context: "YieldStatement",
                        expected: error.expected(),
                    })?;
                let value = self.lower_yield(statement.syntax(), &parts)?;
                let result = self
                    .ast
                    .push_node(PythonAstKind::Expr, statement.syntax().range().into())?;
                self.ast
                    .push_field(result, PythonAstField::Value, PythonAstValue::Node(value))?;
                Ok(result)
            }
            PythonStatementNode::If(statement) => self.lower_if(statement),
            PythonStatementNode::While(statement) => self.lower_while(statement),
            PythonStatementNode::For(statement) => self.lower_for(statement),
            PythonStatementNode::Try(statement) => self.lower_try(statement),
            PythonStatementNode::With(statement) => self.lower_with(statement),
            PythonStatementNode::Function(statement) => self.lower_function(statement, Vec::new()),
            PythonStatementNode::Class(statement) => self.lower_class(statement, Vec::new()),
            PythonStatementNode::Decorated(statement) => self.lower_decorated(statement),
            PythonStatementNode::Match(statement) => self.lower_match(statement),
            PythonStatementNode::StatementGroup(_) => Err(AstError::InconsistentCst {
                context: "statement",
                expected: "a statement group flattened by the statement-list lowerer",
            }),
        }
    }

    fn lower_statement_items(&mut self, node: &SyntaxNode) -> Result<Vec<AstNodeId>, AstError> {
        let statement = PythonStatementNode::downcast_from(node.clone()).map_err(|_| {
            AstError::InconsistentCst {
                context: "statement list",
                expected: "a typed statement node",
            }
        })?;
        let PythonStatementNode::StatementGroup(group) = statement else {
            return Ok(vec![self.lower_statement(node)?]);
        };
        let mut statements = Vec::new();
        for statement in group.statements() {
            statements.extend(self.lower_statement_items(statement.syntax())?);
        }
        Ok(statements)
    }

    fn lower_decorated(&mut self, node: &PythonDecoratedStatement) -> Result<AstNodeId, AstError> {
        let decorators = node
            .decorators()
            .map(|decorator| self.lower_decorator(&decorator))
            .collect::<Result<Vec<_>, _>>()?;
        match (node.function(), node.class()) {
            (Some(function), None) => self.lower_function(&function, decorators),
            (None, Some(class)) => self.lower_class(&class, decorators),
            _ => Err(AstError::InconsistentCst {
                context: "DecoratedStatement",
                expected: "exactly one function or class definition",
            }),
        }
    }

    fn lower_type_alias(&mut self, node: &PythonTypeDefinition) -> Result<AstNodeId, AstError> {
        let name = node.name().ok_or(AstError::InconsistentCst {
            context: "TypeDefinition",
            expected: "a type alias name",
        })?;
        let name = self.lower_expression(name.syntax(), PythonAstKind::Store)?;
        let type_parameters = node
            .type_parameters()
            .map(|parameters| self.lower_type_parameters(&parameters))
            .transpose()?
            .unwrap_or_default();
        let value = node.value().ok_or(AstError::InconsistentCst {
            context: "TypeDefinition",
            expected: "a type alias value",
        })?;
        let value = self.lower_expression(value.syntax(), PythonAstKind::Load)?;
        let range = PythonSourceRange::new(
            Some(node.syntax().from()),
            Some(statement_content_end(node.syntax())),
        );
        let result = self.ast.push_node(PythonAstKind::TypeAlias, range)?;
        self.ast
            .push_field(result, PythonAstField::Name, PythonAstValue::Node(name))?;
        self.ast.push_field(
            result,
            PythonAstField::TypeParams,
            PythonAstValue::Nodes(type_parameters),
        )?;
        self.ast
            .push_field(result, PythonAstField::Value, PythonAstValue::Node(value))?;
        Ok(result)
    }

    fn lower_with(&mut self, node: &PythonWithStatement) -> Result<AstNodeId, AstError> {
        let with = node
            .with_items()
            .map_err(|error| AstError::InconsistentCst {
                context: "WithStatement",
                expected: error.expected(),
            })?;
        let items = with
            .items()
            .iter()
            .map(|item| {
                let context =
                    self.lower_expression(item.context().syntax(), PythonAstKind::Load)?;
                let target = item
                    .target()
                    .map(|target| self.lower_expression_item(target, PythonAstKind::Store))
                    .transpose()?;
                let result = self.ast.push_node(
                    PythonAstKind::AbstractWithitem,
                    PythonSourceRange::default(),
                )?;
                self.ast.push_field(
                    result,
                    PythonAstField::ContextExpr,
                    PythonAstValue::Node(context),
                )?;
                self.ast.push_field(
                    result,
                    PythonAstField::OptionalVars,
                    target.map_or(PythonAstValue::None, PythonAstValue::Node),
                )?;
                Ok(result)
            })
            .collect::<Result<Vec<_>, AstError>>()?;
        let body = self.lower_typed_body(with.body())?;
        let kind = if with.is_async() {
            PythonAstKind::AsyncWith
        } else {
            PythonAstKind::With
        };
        let range = PythonSourceRange::new(
            Some(node.syntax().from()),
            Some(statement_content_end(node.syntax())),
        );
        let result = self.ast.push_node(kind, range)?;
        self.ast
            .push_field(result, PythonAstField::Items, PythonAstValue::Nodes(items))?;
        self.ast
            .push_field(result, PythonAstField::Body, PythonAstValue::Nodes(body))?;
        self.ast
            .push_field(result, PythonAstField::TypeComment, PythonAstValue::None)?;
        Ok(result)
    }

    fn lower_match(&mut self, node: &PythonMatchStatement) -> Result<AstNodeId, AstError> {
        let subject = node.subject().ok_or(AstError::InconsistentCst {
            context: "MatchStatement",
            expected: "a match subject",
        })?;
        let subject = self.lower_expression(subject.syntax(), PythonAstKind::Load)?;
        let body = node.body().ok_or(AstError::InconsistentCst {
            context: "MatchStatement",
            expected: "a match body",
        })?;
        let cases = body
            .clauses()
            .map(|clause| self.lower_match_clause(&clause))
            .collect::<Result<Vec<_>, _>>()?;
        let range = PythonSourceRange::new(
            Some(node.syntax().from()),
            Some(statement_content_end(node.syntax())),
        );
        let result = self.ast.push_node(PythonAstKind::Match, range)?;
        self.ast.push_field(
            result,
            PythonAstField::Subject,
            PythonAstValue::Node(subject),
        )?;
        self.ast
            .push_field(result, PythonAstField::Cases, PythonAstValue::Nodes(cases))?;
        Ok(result)
    }

    fn lower_match_clause(&mut self, node: &PythonMatchClause) -> Result<AstNodeId, AstError> {
        let patterns = node.patterns().collect::<Vec<_>>();
        let pattern = match patterns.as_slice() {
            [pattern] => self.lower_pattern(pattern)?,
            [] => {
                return Err(AstError::InconsistentCst {
                    context: "MatchClause",
                    expected: "at least one pattern",
                });
            }
            patterns => {
                let lowered = patterns
                    .iter()
                    .map(|pattern| self.lower_pattern(pattern))
                    .collect::<Result<Vec<_>, _>>()?;
                let range = rezel_common::TextRange::new(
                    patterns[0].syntax().from(),
                    patterns.last().expect("non-empty patterns").syntax().to(),
                );
                let sequence = self
                    .ast
                    .push_node(PythonAstKind::MatchSequence, range.into())?;
                self.ast.push_field(
                    sequence,
                    PythonAstField::Patterns,
                    PythonAstValue::Nodes(lowered),
                )?;
                sequence
            }
        };
        let guard = node
            .guard()
            .and_then(|guard| guard.test())
            .map(|guard| self.lower_expression(guard.syntax(), PythonAstKind::Load))
            .transpose()?;
        let body = node.body().ok_or(AstError::InconsistentCst {
            context: "MatchClause",
            expected: "a case body",
        })?;
        let body = self.lower_typed_body(&body)?;
        let result = self.ast.push_node(
            PythonAstKind::AbstractMatchCase,
            PythonSourceRange::default(),
        )?;
        self.ast.push_field(
            result,
            PythonAstField::Pattern,
            PythonAstValue::Node(pattern),
        )?;
        self.ast.push_field(
            result,
            PythonAstField::Guard,
            guard.map_or(PythonAstValue::None, PythonAstValue::Node),
        )?;
        self.ast
            .push_field(result, PythonAstField::Body, PythonAstValue::Nodes(body))?;
        Ok(result)
    }

    #[allow(clippy::too_many_lines)] // Linear exhaustive syntax dispatch is easier to audit.
    fn lower_pattern(&mut self, pattern: &PythonPatternNode) -> Result<AstNodeId, AstError> {
        match pattern {
            PythonPatternNode::Capture(pattern) => self.lower_capture_pattern(pattern),
            PythonPatternNode::Literal(pattern) => self.lower_literal_pattern(pattern),
            PythonPatternNode::As(pattern) => {
                let inner = pattern.pattern().ok_or(AstError::InconsistentCst {
                    context: "AsPattern",
                    expected: "an inner pattern",
                })?;
                let inner = self.lower_pattern(&inner)?;
                let name = pattern.name().ok_or(AstError::InconsistentCst {
                    context: "AsPattern",
                    expected: "a capture name",
                })?;
                let name = canonical_name(self.node_text(name.syntax())?);
                let name = self.ast.intern(&name)?;
                let result = self
                    .ast
                    .push_node(PythonAstKind::MatchAs, pattern.syntax().range().into())?;
                self.ast.push_field(
                    result,
                    PythonAstField::Pattern,
                    PythonAstValue::Node(inner),
                )?;
                self.ast
                    .push_field(result, PythonAstField::Name, PythonAstValue::String(name))?;
                Ok(result)
            }
            PythonPatternNode::Or(pattern) => {
                let patterns = pattern
                    .patterns()
                    .map(|pattern| self.lower_pattern(&pattern))
                    .collect::<Result<Vec<_>, _>>()?;
                let result = self
                    .ast
                    .push_node(PythonAstKind::MatchOr, pattern.syntax().range().into())?;
                self.ast.push_field(
                    result,
                    PythonAstField::Patterns,
                    PythonAstValue::Nodes(patterns),
                )?;
                Ok(result)
            }
            PythonPatternNode::Attribute(pattern) => {
                let value = self.lower_attribute_pattern(pattern)?;
                let result = self
                    .ast
                    .push_node(PythonAstKind::MatchValue, pattern.syntax().range().into())?;
                self.ast
                    .push_field(result, PythonAstField::Value, PythonAstValue::Node(value))?;
                Ok(result)
            }
            PythonPatternNode::Sequence(pattern) => {
                let sequence = pattern
                    .sequence()
                    .map_err(|error| AstError::InconsistentCst {
                        context: "SequencePattern",
                        expected: error.expected(),
                    })?;
                let patterns = match sequence {
                    PythonSequencePatternView::Grouped(pattern) => {
                        return self.lower_pattern(&pattern);
                    }
                    PythonSequencePatternView::Sequence(patterns) => patterns,
                };
                let patterns = patterns
                    .iter()
                    .map(|pattern| self.lower_pattern(pattern))
                    .collect::<Result<Vec<_>, _>>()?;
                let result = self.ast.push_node(
                    PythonAstKind::MatchSequence,
                    pattern.syntax().range().into(),
                )?;
                self.ast.push_field(
                    result,
                    PythonAstField::Patterns,
                    PythonAstValue::Nodes(patterns),
                )?;
                Ok(result)
            }
            PythonPatternNode::Mapping(pattern) => self.lower_mapping_pattern(pattern),
            PythonPatternNode::Star(pattern) => {
                let capture = pattern.pattern().ok_or(AstError::InconsistentCst {
                    context: "StarPattern",
                    expected: "a capture pattern",
                })?;
                let PythonPatternNode::Capture(capture) = capture else {
                    return Err(AstError::InconsistentCst {
                        context: "StarPattern",
                        expected: "a capture pattern",
                    });
                };
                let name = self.capture_name(&capture)?;
                let result = self
                    .ast
                    .push_node(PythonAstKind::MatchStar, pattern.syntax().range().into())?;
                self.ast.push_field(result, PythonAstField::Name, name)?;
                Ok(result)
            }
            PythonPatternNode::Class(pattern) => {
                let class = if let Some(name) = pattern.name() {
                    self.lower_expression(name.syntax(), PythonAstKind::Load)?
                } else if let Some(attribute) = pattern.attribute() {
                    self.lower_attribute_pattern(&attribute)?
                } else {
                    return Err(AstError::InconsistentCst {
                        context: "ClassPattern",
                        expected: "a class name or attribute",
                    });
                };
                let arguments = pattern.arguments().ok_or(AstError::InconsistentCst {
                    context: "ClassPattern",
                    expected: "pattern arguments",
                })?;
                let patterns = arguments
                    .patterns()
                    .map(|pattern| self.lower_pattern(&pattern))
                    .collect::<Result<Vec<_>, _>>()?;
                let keywords = arguments.keywords().collect::<Vec<_>>();
                let keyword_attributes = keywords
                    .iter()
                    .map(|keyword| {
                        let name = keyword.name().ok_or(AstError::InconsistentCst {
                            context: "KeywordPattern",
                            expected: "a keyword name",
                        })?;
                        let name = canonical_name(self.node_text(name.syntax())?);
                        self.ast.intern(&name)
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                let keyword_patterns = keywords
                    .iter()
                    .map(|keyword| {
                        let pattern = keyword.pattern().ok_or(AstError::InconsistentCst {
                            context: "KeywordPattern",
                            expected: "a keyword pattern",
                        })?;
                        self.lower_pattern(&pattern)
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                let result = self
                    .ast
                    .push_node(PythonAstKind::MatchClass, pattern.syntax().range().into())?;
                self.ast
                    .push_field(result, PythonAstField::Cls, PythonAstValue::Node(class))?;
                self.ast.push_field(
                    result,
                    PythonAstField::Patterns,
                    PythonAstValue::Nodes(patterns),
                )?;
                self.ast.push_field(
                    result,
                    PythonAstField::KwdAttrs,
                    PythonAstValue::Strings(keyword_attributes),
                )?;
                self.ast.push_field(
                    result,
                    PythonAstField::KwdPatterns,
                    PythonAstValue::Nodes(keyword_patterns),
                )?;
                Ok(result)
            }
        }
    }

    fn capture_name(&mut self, pattern: &PythonCapturePattern) -> Result<PythonAstValue, AstError> {
        let name = pattern.name().ok_or(AstError::InconsistentCst {
            context: "CapturePattern",
            expected: "a capture name",
        })?;
        let name = canonical_name(self.node_text(name.syntax())?);
        if name == "_" {
            Ok(PythonAstValue::None)
        } else {
            Ok(PythonAstValue::String(self.ast.intern(&name)?))
        }
    }

    fn lower_capture_pattern(
        &mut self,
        pattern: &PythonCapturePattern,
    ) -> Result<AstNodeId, AstError> {
        let name = self.capture_name(pattern)?;
        let result = self
            .ast
            .push_node(PythonAstKind::MatchAs, pattern.syntax().range().into())?;
        self.ast
            .push_field(result, PythonAstField::Pattern, PythonAstValue::None)?;
        self.ast.push_field(result, PythonAstField::Name, name)?;
        Ok(result)
    }

    fn lower_literal_pattern(
        &mut self,
        pattern: &PythonLiteralPattern,
    ) -> Result<AstNodeId, AstError> {
        let literal = pattern
            .literal()
            .map_err(|error| AstError::InconsistentCst {
                context: "LiteralPattern",
                expected: error.expected(),
            })?;
        if let [value] = literal.values() {
            let singleton = match value {
                PythonExpressionNode::None(_) => Some(PythonAstValue::None),
                PythonExpressionNode::Boolean(value) => {
                    let value = match self.node_text(value.syntax())? {
                        "True" => true,
                        "False" => false,
                        spelling => {
                            return Err(AstError::InvalidLiteral {
                                kind: "pattern singleton",
                                spelling: spelling.to_owned(),
                            });
                        }
                    };
                    Some(PythonAstValue::Bool(value))
                }
                _ => None,
            };
            if let Some(singleton) = singleton {
                let result = self.ast.push_node(
                    PythonAstKind::MatchSingleton,
                    pattern.syntax().range().into(),
                )?;
                self.ast
                    .push_field(result, PythonAstField::Value, singleton)?;
                return Ok(result);
            }
        }
        let value = self.lower_pattern_literal_value(pattern)?;
        let result = self
            .ast
            .push_node(PythonAstKind::MatchValue, pattern.syntax().range().into())?;
        self.ast
            .push_field(result, PythonAstField::Value, PythonAstValue::Node(value))?;
        Ok(result)
    }

    fn lower_pattern_literal_value(
        &mut self,
        pattern: &PythonLiteralPattern,
    ) -> Result<AstNodeId, AstError> {
        let literal = pattern
            .literal()
            .map_err(|error| AstError::InconsistentCst {
                context: "LiteralPattern",
                expected: error.expected(),
            })?;
        let values = literal.values();
        let operators = literal.operators();
        match (values, operators) {
            ([value], []) => self.lower_expression(value.syntax(), PythonAstKind::Load),
            ([value], [operator]) => {
                let spelling = self.node_text(operator)?;
                let kind = match spelling {
                    "-" => PythonAstKind::USub,
                    "+" => PythonAstKind::UAdd,
                    _ => {
                        return Err(AstError::UnsupportedSyntax {
                            kind: spelling.to_owned(),
                        });
                    }
                };
                let operand = self.lower_expression(value.syntax(), PythonAstKind::Load)?;
                let op = self.context_node(kind)?;
                let result = self
                    .ast
                    .push_node(PythonAstKind::UnaryOp, pattern.syntax().range().into())?;
                self.ast
                    .push_field(result, PythonAstField::Op, PythonAstValue::Node(op))?;
                self.ast.push_field(
                    result,
                    PythonAstField::Operand,
                    PythonAstValue::Node(operand),
                )?;
                Ok(result)
            }
            ([left, right], operators @ [.., binary]) if operators.len() <= 2 => {
                let left = if let [unary, _] = operators {
                    let spelling = self.node_text(unary)?;
                    let kind = match spelling {
                        "-" => PythonAstKind::USub,
                        "+" => PythonAstKind::UAdd,
                        _ => {
                            return Err(AstError::UnsupportedSyntax {
                                kind: spelling.to_owned(),
                            });
                        }
                    };
                    let operand = self.lower_expression(left.syntax(), PythonAstKind::Load)?;
                    let op = self.context_node(kind)?;
                    let range =
                        rezel_common::TextRange::new(pattern.syntax().from(), left.syntax().to());
                    let unary = self.ast.push_node(PythonAstKind::UnaryOp, range.into())?;
                    self.ast
                        .push_field(unary, PythonAstField::Op, PythonAstValue::Node(op))?;
                    self.ast.push_field(
                        unary,
                        PythonAstField::Operand,
                        PythonAstValue::Node(operand),
                    )?;
                    unary
                } else {
                    self.lower_expression(left.syntax(), PythonAstKind::Load)?
                };
                let right = self.lower_expression(right.syntax(), PythonAstKind::Load)?;
                let spelling = self.node_text(binary)?;
                let kind = match spelling {
                    "+" => PythonAstKind::Add,
                    "-" => PythonAstKind::Sub,
                    _ => {
                        return Err(AstError::UnsupportedSyntax {
                            kind: spelling.to_owned(),
                        });
                    }
                };
                let op = self.context_node(kind)?;
                let result = self
                    .ast
                    .push_node(PythonAstKind::BinOp, pattern.syntax().range().into())?;
                self.ast
                    .push_field(result, PythonAstField::Left, PythonAstValue::Node(left))?;
                self.ast
                    .push_field(result, PythonAstField::Op, PythonAstValue::Node(op))?;
                self.ast
                    .push_field(result, PythonAstField::Right, PythonAstValue::Node(right))?;
                Ok(result)
            }
            _ => Err(AstError::InconsistentCst {
                context: "LiteralPattern",
                expected: "one literal atom or one complex-number expression",
            }),
        }
    }

    fn lower_attribute_pattern(
        &mut self,
        pattern: &PythonAttributePattern,
    ) -> Result<AstNodeId, AstError> {
        let name = pattern.name().ok_or(AstError::InconsistentCst {
            context: "AttributePattern",
            expected: "a base name",
        })?;
        let mut value = self.lower_expression(name.syntax(), PythonAstKind::Load)?;
        for property in pattern.properties() {
            let attribute = canonical_name(self.node_text(property.syntax())?);
            let attribute = self.ast.intern(&attribute)?;
            let range =
                rezel_common::TextRange::new(pattern.syntax().from(), property.syntax().to());
            let member = self.ast.push_node(PythonAstKind::Attribute, range.into())?;
            self.ast
                .push_field(member, PythonAstField::Value, PythonAstValue::Node(value))?;
            self.ast.push_field(
                member,
                PythonAstField::Attr,
                PythonAstValue::String(attribute),
            )?;
            let context = self.context_node(PythonAstKind::Load)?;
            self.ast
                .push_field(member, PythonAstField::Ctx, PythonAstValue::Node(context))?;
            value = member;
        }
        Ok(value)
    }

    fn lower_mapping_pattern(
        &mut self,
        pattern: &crate::typed::PythonMappingPattern,
    ) -> Result<AstNodeId, AstError> {
        let mapping = pattern
            .mapping()
            .map_err(|error| AstError::InconsistentCst {
                context: "MappingPattern",
                expected: error.expected(),
            })?;
        let keys = mapping
            .entries()
            .iter()
            .map(|entry| match entry.key() {
                PythonMappingKey::Literal(key) => self.lower_pattern_literal_value(key),
                PythonMappingKey::Value(key) => {
                    self.lower_expression(key.syntax(), PythonAstKind::Load)
                }
            })
            .collect::<Result<Vec<_>, _>>()?;
        let patterns = mapping
            .entries()
            .iter()
            .map(|entry| self.lower_pattern(entry.pattern()))
            .collect::<Result<Vec<_>, _>>()?;
        let rest = mapping
            .rest()
            .map(|rest| self.capture_name(rest))
            .transpose()?
            .unwrap_or(PythonAstValue::None);
        let result = self
            .ast
            .push_node(PythonAstKind::MatchMapping, pattern.syntax().range().into())?;
        self.ast
            .push_field(result, PythonAstField::Keys, PythonAstValue::Nodes(keys))?;
        self.ast.push_field(
            result,
            PythonAstField::Patterns,
            PythonAstValue::Nodes(patterns),
        )?;
        self.ast.push_field(result, PythonAstField::Rest, rest)?;
        Ok(result)
    }

    fn lower_decorator(&mut self, node: &PythonDecorator) -> Result<AstNodeId, AstError> {
        let names = node.names().collect::<Vec<_>>();
        let first = names.first().ok_or(AstError::InconsistentCst {
            context: "Decorator",
            expected: "a dotted name",
        })?;
        let mut value = self.lower_expression(first.syntax(), PythonAstKind::Load)?;
        for name in &names[1..] {
            let attribute = canonical_name(self.node_text(name.syntax())?);
            let attribute = self.ast.intern(&attribute)?;
            let range = rezel_common::TextRange::new(first.syntax().from(), name.syntax().to());
            let member = self.ast.push_node(PythonAstKind::Attribute, range.into())?;
            self.ast
                .push_field(member, PythonAstField::Value, PythonAstValue::Node(value))?;
            self.ast.push_field(
                member,
                PythonAstField::Attr,
                PythonAstValue::String(attribute),
            )?;
            let context = self.context_node(PythonAstKind::Load)?;
            self.ast
                .push_field(member, PythonAstField::Ctx, PythonAstValue::Node(context))?;
            value = member;
        }
        let Some(arguments) = node.arguments() else {
            return Ok(value);
        };
        let (args, keywords) = self.lower_arguments(&arguments)?;
        let range = rezel_common::TextRange::new(first.syntax().from(), arguments.syntax().to());
        let call = self.ast.push_node(PythonAstKind::Call, range.into())?;
        self.ast
            .push_field(call, PythonAstField::Func, PythonAstValue::Node(value))?;
        self.ast
            .push_field(call, PythonAstField::Args, PythonAstValue::Nodes(args))?;
        self.ast.push_field(
            call,
            PythonAstField::Keywords,
            PythonAstValue::Nodes(keywords),
        )?;
        Ok(call)
    }

    fn lower_function(
        &mut self,
        node: &PythonFunctionDefinition,
        decorators: Vec<AstNodeId>,
    ) -> Result<AstNodeId, AstError> {
        let name = node.name().ok_or(AstError::InconsistentCst {
            context: "FunctionDefinition",
            expected: "a function name",
        })?;
        let name = canonical_name(self.node_text(name.syntax())?);
        let name = self.ast.intern(&name)?;
        let parameters = node.parameters().ok_or(AstError::InconsistentCst {
            context: "FunctionDefinition",
            expected: "a parameter list",
        })?;
        let parameters =
            parameters
                .parameters(self.source)
                .map_err(|error| AstError::InconsistentCst {
                    context: "ParamList",
                    expected: error.expected(),
                })?;
        let arguments = self.lower_parameters(&parameters)?;
        let body = node.body().ok_or(AstError::InconsistentCst {
            context: "FunctionDefinition",
            expected: "a function body",
        })?;
        let body = self.lower_typed_body(&body)?;
        let returns = node
            .return_annotation()
            .and_then(|annotation| annotation.annotation())
            .map(|annotation| self.lower_expression(annotation.syntax(), PythonAstKind::Load))
            .transpose()?;
        let type_parameters = node
            .type_parameters()
            .map(|parameters| self.lower_type_parameters(&parameters))
            .transpose()?
            .unwrap_or_default();
        let kind = if node.is_async() {
            PythonAstKind::AsyncFunctionDef
        } else {
            PythonAstKind::FunctionDef
        };
        let range = PythonSourceRange::new(
            Some(node.syntax().from()),
            Some(statement_content_end(node.syntax())),
        );
        let result = self.ast.push_node(kind, range)?;
        self.ast
            .push_field(result, PythonAstField::Name, PythonAstValue::String(name))?;
        self.ast.push_field(
            result,
            PythonAstField::Args,
            PythonAstValue::Node(arguments),
        )?;
        self.ast
            .push_field(result, PythonAstField::Body, PythonAstValue::Nodes(body))?;
        self.ast.push_field(
            result,
            PythonAstField::DecoratorList,
            PythonAstValue::Nodes(decorators),
        )?;
        self.ast.push_field(
            result,
            PythonAstField::Returns,
            returns.map_or(PythonAstValue::None, PythonAstValue::Node),
        )?;
        self.ast
            .push_field(result, PythonAstField::TypeComment, PythonAstValue::None)?;
        self.ast.push_field(
            result,
            PythonAstField::TypeParams,
            PythonAstValue::Nodes(type_parameters),
        )?;
        Ok(result)
    }

    fn lower_class(
        &mut self,
        node: &PythonClassDefinition,
        decorators: Vec<AstNodeId>,
    ) -> Result<AstNodeId, AstError> {
        let name = node.name().ok_or(AstError::InconsistentCst {
            context: "ClassDefinition",
            expected: "a class name",
        })?;
        let name = canonical_name(self.node_text(name.syntax())?);
        let name = self.ast.intern(&name)?;
        let (bases, keywords) = node
            .arguments()
            .map(|arguments| self.lower_arguments(&arguments))
            .transpose()?
            .unwrap_or_default();
        let body = node.body().ok_or(AstError::InconsistentCst {
            context: "ClassDefinition",
            expected: "a class body",
        })?;
        let body = self.lower_typed_body(&body)?;
        let type_parameters = node
            .type_parameters()
            .map(|parameters| self.lower_type_parameters(&parameters))
            .transpose()?
            .unwrap_or_default();
        let range = PythonSourceRange::new(
            Some(node.syntax().from()),
            Some(statement_content_end(node.syntax())),
        );
        let result = self.ast.push_node(PythonAstKind::ClassDef, range)?;
        self.ast
            .push_field(result, PythonAstField::Name, PythonAstValue::String(name))?;
        self.ast
            .push_field(result, PythonAstField::Bases, PythonAstValue::Nodes(bases))?;
        self.ast.push_field(
            result,
            PythonAstField::Keywords,
            PythonAstValue::Nodes(keywords),
        )?;
        self.ast
            .push_field(result, PythonAstField::Body, PythonAstValue::Nodes(body))?;
        self.ast.push_field(
            result,
            PythonAstField::DecoratorList,
            PythonAstValue::Nodes(decorators),
        )?;
        self.ast.push_field(
            result,
            PythonAstField::TypeParams,
            PythonAstValue::Nodes(type_parameters),
        )?;
        Ok(result)
    }

    fn lower_parameters(&mut self, parameters: &PythonParameters) -> Result<AstNodeId, AstError> {
        let positional_only = parameters
            .positional_only()
            .iter()
            .map(|parameter| self.lower_parameter(parameter))
            .collect::<Result<Vec<_>, _>>()?;
        let positional = parameters
            .positional_or_keyword()
            .iter()
            .map(|parameter| self.lower_parameter(parameter))
            .collect::<Result<Vec<_>, _>>()?;
        let vararg = parameters
            .vararg()
            .map(|parameter| self.lower_parameter(parameter))
            .transpose()?;
        let keyword_only = parameters
            .keyword_only()
            .iter()
            .map(|parameter| self.lower_parameter(parameter))
            .collect::<Result<Vec<_>, _>>()?;
        let keyword_defaults = parameters
            .keyword_only()
            .iter()
            .map(|parameter| {
                parameter
                    .default()
                    .map(|default| self.lower_expression(default.syntax(), PythonAstKind::Load))
                    .transpose()
            })
            .collect::<Result<Vec<_>, _>>()?;
        let kwarg = parameters
            .kwarg()
            .map(|parameter| self.lower_parameter(parameter))
            .transpose()?;
        let defaults = parameters
            .positional_only()
            .iter()
            .chain(parameters.positional_or_keyword())
            .filter_map(PythonParameter::default)
            .map(|default| self.lower_expression(default.syntax(), PythonAstKind::Load))
            .collect::<Result<Vec<_>, _>>()?;
        let result = self.ast.push_node(
            PythonAstKind::AbstractArguments,
            PythonSourceRange::default(),
        )?;
        self.ast.push_field(
            result,
            PythonAstField::Posonlyargs,
            PythonAstValue::Nodes(positional_only),
        )?;
        self.ast.push_field(
            result,
            PythonAstField::Args,
            PythonAstValue::Nodes(positional),
        )?;
        self.ast.push_field(
            result,
            PythonAstField::Vararg,
            vararg.map_or(PythonAstValue::None, PythonAstValue::Node),
        )?;
        self.ast.push_field(
            result,
            PythonAstField::Kwonlyargs,
            PythonAstValue::Nodes(keyword_only),
        )?;
        self.ast.push_field(
            result,
            PythonAstField::KwDefaults,
            PythonAstValue::OptionalNodes(keyword_defaults),
        )?;
        self.ast.push_field(
            result,
            PythonAstField::Kwarg,
            kwarg.map_or(PythonAstValue::None, PythonAstValue::Node),
        )?;
        self.ast.push_field(
            result,
            PythonAstField::Defaults,
            PythonAstValue::Nodes(defaults),
        )?;
        Ok(result)
    }

    fn lower_parameter(&mut self, parameter: &PythonParameter) -> Result<AstNodeId, AstError> {
        let name = canonical_name(self.node_text(parameter.name().syntax())?);
        let name = self.ast.intern(&name)?;
        let annotation = parameter
            .annotation()
            .map(|annotation| self.lower_expression(annotation.syntax(), PythonAstKind::Load))
            .transpose()?;
        let end = parameter
            .annotation()
            .map_or(parameter.name().syntax().to(), |annotation| {
                annotation.syntax().to()
            });
        let range = rezel_common::TextRange::new(parameter.name().syntax().from(), end);
        let result = self
            .ast
            .push_node(PythonAstKind::AbstractArg, range.into())?;
        self.ast
            .push_field(result, PythonAstField::Arg, PythonAstValue::String(name))?;
        self.ast.push_field(
            result,
            PythonAstField::Annotation,
            annotation.map_or(PythonAstValue::None, PythonAstValue::Node),
        )?;
        self.ast
            .push_field(result, PythonAstField::TypeComment, PythonAstValue::None)?;
        Ok(result)
    }

    fn lower_type_parameters(
        &mut self,
        parameters: &PythonTypeParamList,
    ) -> Result<Vec<AstNodeId>, AstError> {
        parameters
            .type_parameters()
            .map_err(|error| AstError::InconsistentCst {
                context: "TypeParamList",
                expected: error.expected(),
            })?
            .iter()
            .map(|parameter| self.lower_type_parameter(parameter))
            .collect()
    }

    fn lower_type_parameter(
        &mut self,
        parameter: &PythonTypeParameter,
    ) -> Result<AstNodeId, AstError> {
        let name = canonical_name(self.node_text(parameter.name().syntax())?);
        let name = self.ast.intern(&name)?;
        let bound = parameter
            .bound()
            .map(|bound| self.lower_expression(bound.syntax(), PythonAstKind::Load))
            .transpose()?;
        let default = parameter
            .default()
            .map(|default| self.lower_expression_item(default, PythonAstKind::Load))
            .transpose()?;
        let kind = match parameter.kind() {
            PythonTypeParameterKind::TypeVar => PythonAstKind::TypeVar,
            PythonTypeParameterKind::TypeVarTuple => PythonAstKind::TypeVarTuple,
            PythonTypeParameterKind::ParamSpec => PythonAstKind::ParamSpec,
        };
        let result = self.ast.push_node(kind, parameter.range().into())?;
        self.ast
            .push_field(result, PythonAstField::Name, PythonAstValue::String(name))?;
        if parameter.kind() == PythonTypeParameterKind::TypeVar {
            self.ast.push_field(
                result,
                PythonAstField::Bound,
                bound.map_or(PythonAstValue::None, PythonAstValue::Node),
            )?;
        }
        self.ast.push_field(
            result,
            PythonAstField::DefaultValue,
            default.map_or(PythonAstValue::None, PythonAstValue::Node),
        )?;
        Ok(result)
    }

    fn lower_assign(&mut self, node: &PythonAssignStatement) -> Result<AstNodeId, AstError> {
        let groups = node
            .assignment_groups()
            .map_err(|error| AstError::InconsistentCst {
                context: "AssignStatement",
                expected: error.expected(),
            })?;
        let (value, targets) = groups.split_last().ok_or(AstError::InconsistentCst {
            context: "AssignStatement",
            expected: "target and value",
        })?;
        if targets.is_empty() {
            return Err(AstError::InconsistentCst {
                context: "AssignStatement",
                expected: "assignment target",
            });
        }
        let targets = targets
            .iter()
            .map(|target| self.lower_expression_group(target, PythonAstKind::Store))
            .collect::<Result<Vec<_>, _>>()?;
        let value = self.lower_expression_group(value, PythonAstKind::Load)?;
        let syntax = node.syntax();
        let type_comment = self
            .options
            .type_comments
            .then(|| self.type_comment_after(syntax))
            .flatten()
            .map(|(comment, end)| (comment.to_owned(), end));
        let range = PythonSourceRange::new(
            Some(syntax.from()),
            type_comment
                .as_ref()
                .map_or(Some(syntax.to()), |(_, end)| Some(*end)),
        );
        let result = self.ast.push_node(PythonAstKind::Assign, range)?;
        self.ast.push_field(
            result,
            PythonAstField::Targets,
            PythonAstValue::Nodes(targets),
        )?;
        self.ast
            .push_field(result, PythonAstField::Value, PythonAstValue::Node(value))?;
        let type_comment = type_comment
            .as_ref()
            .map(|(comment, _)| comment.as_str())
            .map(|value| self.ast.intern(value).map(PythonAstValue::String))
            .transpose()?
            .unwrap_or(PythonAstValue::None);
        self.ast
            .push_field(result, PythonAstField::TypeComment, type_comment)?;
        Ok(result)
    }

    fn lower_annotated_assign(
        &mut self,
        node: &PythonAssignStatement,
    ) -> Result<AstNodeId, AstError> {
        let assignment =
            node.annotated_assignment()
                .map_err(|error| AstError::InconsistentCst {
                    context: "AssignStatement",
                    expected: error.expected(),
                })?;
        let value = assignment
            .value()
            .map(|value| self.lower_expression(value.syntax(), PythonAstKind::Load))
            .transpose()?;
        let simple = u8::from(assignment.is_simple());
        let target = self.lower_expression(assignment.target().syntax(), PythonAstKind::Store)?;
        let annotation =
            self.lower_expression(assignment.annotation().syntax(), PythonAstKind::Load)?;
        let simple = self.ast.intern(&simple.to_string())?;
        let result = self
            .ast
            .push_node(PythonAstKind::AnnAssign, node.syntax().range().into())?;
        self.ast
            .push_field(result, PythonAstField::Target, PythonAstValue::Node(target))?;
        self.ast.push_field(
            result,
            PythonAstField::Annotation,
            PythonAstValue::Node(annotation),
        )?;
        self.ast.push_field(
            result,
            PythonAstField::Value,
            value.map_or(PythonAstValue::None, PythonAstValue::Node),
        )?;
        self.ast.push_field(
            result,
            PythonAstField::Simple,
            PythonAstValue::Integer(simple),
        )?;
        Ok(result)
    }

    fn lower_augmented_assign(
        &mut self,
        node: &PythonUpdateStatement,
    ) -> Result<AstNodeId, AstError> {
        let expressions = node.expressions().collect::<Vec<_>>();
        let [target, value] = expressions.as_slice() else {
            return Err(AstError::InconsistentCst {
                context: "UpdateStatement",
                expected: "target and value",
            });
        };
        let operator = node.operator().ok_or(AstError::InconsistentCst {
            context: "UpdateStatement",
            expected: "update operator",
        })?;
        let spelling = self.node_text(operator.syntax())?.strip_suffix('=').ok_or(
            AstError::InconsistentCst {
                context: "UpdateOp",
                expected: "operator ending in =",
            },
        )?;
        let operator =
            binary_operator_kind(spelling).ok_or_else(|| AstError::UnsupportedSyntax {
                kind: spelling.to_owned(),
            })?;
        let target = self.lower_expression(target.syntax(), PythonAstKind::Store)?;
        let value = self.lower_expression(value.syntax(), PythonAstKind::Load)?;
        let operator = self.ast.push_node(operator, PythonSourceRange::default())?;
        let result = self
            .ast
            .push_node(PythonAstKind::AugAssign, node.syntax().range().into())?;
        self.ast
            .push_field(result, PythonAstField::Target, PythonAstValue::Node(target))?;
        self.ast
            .push_field(result, PythonAstField::Op, PythonAstValue::Node(operator))?;
        self.ast
            .push_field(result, PythonAstField::Value, PythonAstValue::Node(value))?;
        Ok(result)
    }

    fn lower_scope(&mut self, node: &PythonScopeStatement) -> Result<AstNodeId, AstError> {
        let kind = match node
            .scope_kind()
            .map_err(|error| AstError::InconsistentCst {
                context: "ScopeStatement",
                expected: error.expected(),
            })? {
            PythonScopeKind::Global => PythonAstKind::Global,
            PythonScopeKind::Nonlocal => PythonAstKind::Nonlocal,
        };
        let names = node
            .names()
            .map(|name| {
                let name = canonical_name(self.node_text(name.syntax())?);
                self.ast.intern(&name)
            })
            .collect::<Result<Vec<_>, _>>()?;
        let result = self.ast.push_node(kind, node.syntax().range().into())?;
        self.ast.push_field(
            result,
            PythonAstField::Names,
            PythonAstValue::Strings(names),
        )?;
        Ok(result)
    }

    fn lower_assert(&mut self, node: &PythonAssertStatement) -> Result<AstNodeId, AstError> {
        let expressions = node.values().collect::<Vec<_>>();
        let test = expressions.first().ok_or(AstError::InconsistentCst {
            context: "AssertStatement",
            expected: "test",
        })?;
        let test = self.lower_expression(test.syntax(), PythonAstKind::Load)?;
        let message = expressions
            .get(1)
            .map(|message| self.lower_expression(message.syntax(), PythonAstKind::Load))
            .transpose()?;
        let result = self
            .ast
            .push_node(PythonAstKind::Assert, node.syntax().range().into())?;
        self.ast
            .push_field(result, PythonAstField::Test, PythonAstValue::Node(test))?;
        self.ast.push_field(
            result,
            PythonAstField::Msg,
            message.map_or(PythonAstValue::None, PythonAstValue::Node),
        )?;
        Ok(result)
    }

    fn lower_raise(&mut self, node: &PythonRaiseStatement) -> Result<AstNodeId, AstError> {
        let expressions = node.values().collect::<Vec<_>>();
        let exception = expressions
            .first()
            .map(|exception| self.lower_expression(exception.syntax(), PythonAstKind::Load))
            .transpose()?;
        let cause = expressions
            .get(1)
            .map(|cause| self.lower_expression(cause.syntax(), PythonAstKind::Load))
            .transpose()?;
        let result = self
            .ast
            .push_node(PythonAstKind::Raise, node.syntax().range().into())?;
        self.ast.push_field(
            result,
            PythonAstField::Exc,
            exception.map_or(PythonAstValue::None, PythonAstValue::Node),
        )?;
        self.ast.push_field(
            result,
            PythonAstField::Cause,
            cause.map_or(PythonAstValue::None, PythonAstValue::Node),
        )?;
        Ok(result)
    }

    fn lower_import(
        &mut self,
        node: &crate::typed::PythonImportStatement,
    ) -> Result<AstNodeId, AstError> {
        let import = node
            .import(self.source)
            .map_err(|error| AstError::InconsistentCst {
                context: "ImportStatement",
                expected: error.expected(),
            })?;
        let aliases = import
            .aliases()
            .iter()
            .map(|alias| self.lower_import_alias(alias))
            .collect::<Result<Vec<_>, _>>()?;
        if let Some(from) = import.from() {
            let module = if from.module().is_empty() {
                PythonAstValue::None
            } else {
                let module = from
                    .module()
                    .iter()
                    .map(|name| self.node_text(name.syntax()).map(canonical_name))
                    .collect::<Result<Vec<_>, _>>()?
                    .join(".");
                PythonAstValue::String(self.ast.intern(&module)?)
            };
            let level = self.ast.intern(&from.level().to_string())?;
            let result = self
                .ast
                .push_node(PythonAstKind::ImportFrom, node.syntax().range().into())?;
            self.ast
                .push_field(result, PythonAstField::Module, module)?;
            self.ast.push_field(
                result,
                PythonAstField::Names,
                PythonAstValue::Nodes(aliases),
            )?;
            self.ast.push_field(
                result,
                PythonAstField::Level,
                PythonAstValue::Integer(level),
            )?;
            return Ok(result);
        }
        let result = self
            .ast
            .push_node(PythonAstKind::Import, node.syntax().range().into())?;
        self.ast.push_field(
            result,
            PythonAstField::Names,
            PythonAstValue::Nodes(aliases),
        )?;
        Ok(result)
    }

    fn lower_import_alias(&mut self, alias: &PythonImportAlias) -> Result<AstNodeId, AstError> {
        let name = if alias.is_wildcard() {
            "*".to_owned()
        } else {
            alias
                .name()
                .iter()
                .map(|name| self.node_text(name.syntax()).map(canonical_name))
                .collect::<Result<Vec<_>, _>>()?
                .join(".")
        };
        let name = self.ast.intern(&name)?;
        let as_name = alias
            .as_name()
            .map(|name| self.node_text(name.syntax()))
            .transpose()?
            .map(canonical_name)
            .map(|name| self.ast.intern(&name).map(PythonAstValue::String))
            .transpose()?
            .unwrap_or(PythonAstValue::None);
        let result = self
            .ast
            .push_node(PythonAstKind::AbstractAlias, alias.range().into())?;
        self.ast
            .push_field(result, PythonAstField::Name, PythonAstValue::String(name))?;
        self.ast
            .push_field(result, PythonAstField::Asname, as_name)?;
        Ok(result)
    }

    fn lower_yield(
        &mut self,
        node: &SyntaxNode,
        parts: &PythonYield,
    ) -> Result<AstNodeId, AstError> {
        let value = parts
            .group()
            .map(|group| self.lower_expression_group(group, PythonAstKind::Load))
            .transpose()?;
        let kind = if parts.is_from() {
            PythonAstKind::YieldFrom
        } else {
            PythonAstKind::Yield
        };
        let result = self.ast.push_node(kind, node.range().into())?;
        self.ast.push_field(
            result,
            PythonAstField::Value,
            value.map_or(PythonAstValue::None, PythonAstValue::Node),
        )?;
        Ok(result)
    }

    fn lower_if(&mut self, node: &PythonIfStatement) -> Result<AstNodeId, AstError> {
        let conditional = node
            .conditional()
            .map_err(|error| AstError::InconsistentCst {
                context: "IfStatement",
                expected: error.expected(),
            })?;
        let end = statement_content_end(node.syntax());
        let mut orelse = conditional
            .orelse()
            .map(|body| self.lower_typed_body(body))
            .transpose()?
            .unwrap_or_default();
        for clause in conditional.clauses().iter().rev() {
            let test = self.lower_expression(clause.test().syntax(), PythonAstKind::Load)?;
            let body = self.lower_typed_body(clause.body())?;
            let result = self.ast.push_node(
                PythonAstKind::If,
                PythonSourceRange::new(Some(clause.start()), Some(end)),
            )?;
            self.ast
                .push_field(result, PythonAstField::Test, PythonAstValue::Node(test))?;
            self.ast
                .push_field(result, PythonAstField::Body, PythonAstValue::Nodes(body))?;
            self.ast.push_field(
                result,
                PythonAstField::Orelse,
                PythonAstValue::Nodes(orelse),
            )?;
            orelse = vec![result];
        }
        orelse.pop().ok_or(AstError::InconsistentCst {
            context: "IfStatement",
            expected: "at least one conditional clause",
        })
    }

    fn lower_while(&mut self, node: &PythonWhileStatement) -> Result<AstNodeId, AstError> {
        let parts = node
            .while_parts()
            .map_err(|error| AstError::InconsistentCst {
                context: "WhileStatement",
                expected: error.expected(),
            })?;
        let test = self.lower_expression(parts.test().syntax(), PythonAstKind::Load)?;
        let body = self.lower_typed_body(parts.body())?;
        let orelse = parts
            .orelse()
            .map(|body| self.lower_typed_body(body))
            .transpose()?
            .unwrap_or_default();
        let result = self.ast.push_node(
            PythonAstKind::While,
            PythonSourceRange::new(
                Some(node.syntax().from()),
                Some(statement_content_end(node.syntax())),
            ),
        )?;
        self.ast
            .push_field(result, PythonAstField::Test, PythonAstValue::Node(test))?;
        self.ast
            .push_field(result, PythonAstField::Body, PythonAstValue::Nodes(body))?;
        self.ast.push_field(
            result,
            PythonAstField::Orelse,
            PythonAstValue::Nodes(orelse),
        )?;
        Ok(result)
    }

    fn lower_for(&mut self, node: &PythonForStatement) -> Result<AstNodeId, AstError> {
        let parts = node
            .for_parts()
            .map_err(|error| AstError::InconsistentCst {
                context: "ForStatement",
                expected: error.expected(),
            })?;
        let target_range = rezel_common::TextRange::new(
            parts
                .targets()
                .first()
                .expect("view requires targets")
                .range()
                .start(),
            parts
                .targets()
                .last()
                .expect("view requires targets")
                .range()
                .end(),
        );
        let target = self.lower_expression_items_with_sequence(
            parts.targets(),
            parts.target_is_sequence(),
            target_range,
            PythonAstKind::Store,
        )?;
        let iterator = self.lower_typed_expression_list(parts.iterators(), PythonAstKind::Load)?;
        let body = self.lower_typed_body(parts.body())?;
        let orelse = parts
            .orelse()
            .map(|body| self.lower_typed_body(body))
            .transpose()?
            .unwrap_or_default();
        let kind = if parts.is_async() {
            PythonAstKind::AsyncFor
        } else {
            PythonAstKind::For
        };
        let type_comment = self
            .options
            .type_comments
            .then(|| self.type_comment_after(node.syntax()))
            .flatten()
            .map(|(comment, _)| comment.to_owned());
        let type_comment = type_comment
            .as_deref()
            .map(|comment| self.ast.intern(comment).map(PythonAstValue::String))
            .transpose()?
            .unwrap_or(PythonAstValue::None);
        let result = self.ast.push_node(
            kind,
            PythonSourceRange::new(
                Some(node.syntax().from()),
                Some(statement_content_end(node.syntax())),
            ),
        )?;
        self.ast
            .push_field(result, PythonAstField::Target, PythonAstValue::Node(target))?;
        self.ast
            .push_field(result, PythonAstField::Iter, PythonAstValue::Node(iterator))?;
        self.ast
            .push_field(result, PythonAstField::Body, PythonAstValue::Nodes(body))?;
        self.ast.push_field(
            result,
            PythonAstField::Orelse,
            PythonAstValue::Nodes(orelse),
        )?;
        self.ast
            .push_field(result, PythonAstField::TypeComment, type_comment)?;
        Ok(result)
    }

    fn lower_try(
        &mut self,
        node: &crate::typed::PythonTryStatement,
    ) -> Result<AstNodeId, AstError> {
        let clauses = node.clauses().map_err(|error| AstError::InconsistentCst {
            context: "TryStatement",
            expected: error.expected(),
        })?;
        let has_groups = clauses
            .handlers()
            .iter()
            .any(PythonExceptClause::is_exception_group);
        let has_regular = clauses
            .handlers()
            .iter()
            .any(|handler| !handler.is_exception_group());
        if has_groups && has_regular {
            return Err(AstError::UnsupportedSyntax {
                kind: "mixed except and except* clauses".to_owned(),
            });
        }
        let kind = if has_groups {
            PythonAstKind::TryStar
        } else {
            PythonAstKind::Try
        };
        let body = self.lower_typed_body(clauses.body())?;
        let handlers = clauses
            .handlers()
            .iter()
            .map(|handler| self.lower_except_clause(handler))
            .collect::<Result<Vec<_>, _>>()?;
        let orelse = clauses
            .orelse()
            .map(|body| self.lower_typed_body(body))
            .transpose()?
            .unwrap_or_default();
        let finalbody = clauses
            .finalbody()
            .map(|body| self.lower_typed_body(body))
            .transpose()?
            .unwrap_or_default();
        let range = PythonSourceRange::new(
            Some(node.syntax().from()),
            Some(statement_content_end(node.syntax())),
        );
        let result = self.ast.push_node(kind, range)?;
        self.ast
            .push_field(result, PythonAstField::Body, PythonAstValue::Nodes(body))?;
        self.ast.push_field(
            result,
            PythonAstField::Handlers,
            PythonAstValue::Nodes(handlers),
        )?;
        self.ast.push_field(
            result,
            PythonAstField::Orelse,
            PythonAstValue::Nodes(orelse),
        )?;
        self.ast.push_field(
            result,
            PythonAstField::Finalbody,
            PythonAstValue::Nodes(finalbody),
        )?;
        Ok(result)
    }

    fn lower_except_clause(&mut self, clause: &PythonExceptClause) -> Result<AstNodeId, AstError> {
        let exception_type = match clause.types() {
            [] => None,
            [exception_type] => {
                Some(self.lower_expression(exception_type.syntax(), PythonAstKind::Load)?)
            }
            types => {
                let elements = types
                    .iter()
                    .map(|exception_type| {
                        self.lower_expression(exception_type.syntax(), PythonAstKind::Load)
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                let first = types.first().expect("types are non-empty");
                let last = types.last().expect("types are non-empty");
                let range =
                    PythonSourceRange::new(Some(first.syntax().from()), Some(last.syntax().to()));
                let tuple = self.ast.push_node(PythonAstKind::Tuple, range)?;
                self.ast.push_field(
                    tuple,
                    PythonAstField::Elts,
                    PythonAstValue::Nodes(elements),
                )?;
                let load = self.context_node(PythonAstKind::Load)?;
                self.ast
                    .push_field(tuple, PythonAstField::Ctx, PythonAstValue::Node(load))?;
                Some(tuple)
            }
        };
        let name = clause
            .name()
            .map(|name| {
                let name = canonical_name(self.node_text(name.syntax())?);
                self.ast.intern(&name)
            })
            .transpose()?;
        let body = self.lower_typed_body(clause.body())?;
        let range = PythonSourceRange::new(
            Some(clause.range().start()),
            Some(statement_content_end(clause.body().syntax())),
        );
        let result = self.ast.push_node(PythonAstKind::ExceptHandler, range)?;
        self.ast.push_field(
            result,
            PythonAstField::Type,
            exception_type.map_or(PythonAstValue::None, PythonAstValue::Node),
        )?;
        self.ast.push_field(
            result,
            PythonAstField::Name,
            name.map_or(PythonAstValue::None, PythonAstValue::String),
        )?;
        self.ast
            .push_field(result, PythonAstField::Body, PythonAstValue::Nodes(body))?;
        Ok(result)
    }

    fn lower_typed_expression_list(
        &mut self,
        expressions: &[PythonExpressionNode],
        context: PythonAstKind,
    ) -> Result<AstNodeId, AstError> {
        let [expression] = expressions else {
            if expressions.is_empty() {
                return Err(AstError::InconsistentCst {
                    context: "expression list",
                    expected: "at least one expression",
                });
            }
            let elements = expressions
                .iter()
                .map(|expression| self.lower_expression(expression.syntax(), context))
                .collect::<Result<Vec<_>, _>>()?;
            let tuple = self.ast.push_node(
                PythonAstKind::Tuple,
                PythonSourceRange::new(
                    Some(expressions[0].syntax().from()),
                    Some(expressions.last().unwrap().syntax().to()),
                ),
            )?;
            self.ast
                .push_field(tuple, PythonAstField::Elts, PythonAstValue::Nodes(elements))?;
            let context = self.context_node(context)?;
            self.ast
                .push_field(tuple, PythonAstField::Ctx, PythonAstValue::Node(context))?;
            return Ok(tuple);
        };
        self.lower_expression(expression.syntax(), context)
    }

    fn lower_expression_group(
        &mut self,
        group: &PythonExpressionGroup,
        context: PythonAstKind,
    ) -> Result<AstNodeId, AstError> {
        self.lower_expression_items_with_sequence(
            group.items(),
            group.is_sequence(),
            group.range(),
            context,
        )
    }

    fn lower_expression_items_with_sequence(
        &mut self,
        items: &[PythonExpressionItem],
        sequence: bool,
        range: rezel_common::TextRange,
        context: PythonAstKind,
    ) -> Result<AstNodeId, AstError> {
        if !sequence {
            let [item] = items else {
                return Err(AstError::InconsistentCst {
                    context: "expression group",
                    expected: "one expression or a comma-separated sequence",
                });
            };
            return self.lower_expression_item(item, context);
        }
        self.lower_expression_tuple_items(items, range, context)
    }

    fn lower_expression_tuple_items(
        &mut self,
        items: &[PythonExpressionItem],
        range: rezel_common::TextRange,
        context: PythonAstKind,
    ) -> Result<AstNodeId, AstError> {
        if items.is_empty() {
            return Err(AstError::InconsistentCst {
                context: "expression list",
                expected: "at least one expression",
            });
        }
        let elements = items
            .iter()
            .map(|item| self.lower_expression_item(item, context))
            .collect::<Result<Vec<_>, _>>()?;
        let tuple = self.ast.push_node(PythonAstKind::Tuple, range.into())?;
        self.ast
            .push_field(tuple, PythonAstField::Elts, PythonAstValue::Nodes(elements))?;
        let context = self.context_node(context)?;
        self.ast
            .push_field(tuple, PythonAstField::Ctx, PythonAstValue::Node(context))?;
        Ok(tuple)
    }

    fn lower_expression_item(
        &mut self,
        item: &PythonExpressionItem,
        context: PythonAstKind,
    ) -> Result<AstNodeId, AstError> {
        match item {
            PythonExpressionItem::Plain(expression) => {
                self.lower_expression(expression.syntax(), context)
            }
            PythonExpressionItem::Starred { value, .. } => {
                let value = self.lower_expression(value.syntax(), context)?;
                let starred = self
                    .ast
                    .push_node(PythonAstKind::Starred, item.range().into())?;
                self.ast
                    .push_field(starred, PythonAstField::Value, PythonAstValue::Node(value))?;
                let context = self.context_node(context)?;
                self.ast
                    .push_field(starred, PythonAstField::Ctx, PythonAstValue::Node(context))?;
                Ok(starred)
            }
        }
    }

    fn lower_typed_body(&mut self, body: &PythonBody) -> Result<Vec<AstNodeId>, AstError> {
        let mut statements = Vec::new();
        for statement in body.statements() {
            statements.extend(self.lower_statement_items(statement.syntax())?);
        }
        Ok(statements)
    }

    #[allow(clippy::too_many_lines)] // Linear exhaustive syntax dispatch is easier to audit.
    fn lower_expression(
        &mut self,
        node: &SyntaxNode,
        context: PythonAstKind,
    ) -> Result<AstNodeId, AstError> {
        let expression = PythonExpressionNode::downcast_from(node.clone()).map_err(|_| {
            AstError::InconsistentCst {
                context: "expression",
                expected: "typed expression node",
            }
        })?;
        match &expression {
            PythonExpressionNode::VariableName(node) => {
                let node = node.syntax();
                let spelling = canonical_name(self.node_text(node)?);
                let id = self.ast.intern(&spelling)?;
                let context_node = self.ast.push_node(context, PythonSourceRange::default())?;
                let result = self
                    .ast
                    .push_node(PythonAstKind::Name, node.range().into())?;
                self.ast
                    .push_field(result, PythonAstField::Id, PythonAstValue::String(id))?;
                self.ast.push_field(
                    result,
                    PythonAstField::Ctx,
                    PythonAstValue::Node(context_node),
                )?;
                Ok(result)
            }
            PythonExpressionNode::Number(node) => {
                let node = node.syntax();
                let spelling = self.node_text(node)?;
                let value = parse_number(spelling).ok_or_else(|| AstError::InvalidLiteral {
                    kind: "number",
                    spelling: spelling.to_owned(),
                })?;
                let result = self
                    .ast
                    .push_node(PythonAstKind::Constant, node.range().into())?;
                let value = match value {
                    ParsedNumber::Integer(value) => {
                        PythonConstant::Integer(self.ast.intern(&value)?)
                    }
                    ParsedNumber::Float(bits) => PythonConstant::Float(bits),
                    ParsedNumber::Imaginary(bits) => PythonConstant::Complex {
                        real: 0.0_f64.to_bits(),
                        imaginary: bits,
                    },
                };
                self.ast.push_field(
                    result,
                    PythonAstField::Value,
                    PythonAstValue::Constant(value),
                )?;
                self.ast
                    .push_field(result, PythonAstField::Kind, PythonAstValue::None)?;
                Ok(result)
            }
            PythonExpressionNode::String(node) => self.lower_string(node.syntax()),
            PythonExpressionNode::ContinuedString(node) => self.lower_continued_string(node),
            PythonExpressionNode::FormatString(node) => {
                let view =
                    node.interpolated(self.source)
                        .map_err(|error| AstError::InconsistentCst {
                            context: "FormatString",
                            expected: error.expected(),
                        })?;
                self.lower_interpolated_string(&view)
            }
            PythonExpressionNode::TemplateString(node) => {
                let view =
                    node.interpolated(self.source)
                        .map_err(|error| AstError::InconsistentCst {
                            context: "TemplateString",
                            expected: error.expected(),
                        })?;
                self.lower_interpolated_string(&view)
            }
            PythonExpressionNode::Boolean(node) => {
                let node = node.syntax();
                let value = match self.node_text(node)? {
                    "True" => true,
                    "False" => false,
                    spelling => {
                        return Err(AstError::InvalidLiteral {
                            kind: "boolean",
                            spelling: spelling.to_owned(),
                        });
                    }
                };
                self.lower_constant(node, PythonConstant::Bool(value))
            }
            PythonExpressionNode::None(node) => {
                self.lower_constant(node.syntax(), PythonConstant::None)
            }
            PythonExpressionNode::Ellipsis(node) => {
                self.lower_constant(node.syntax(), PythonConstant::Ellipsis)
            }
            PythonExpressionNode::Parenthesized(node) => {
                let value = node.value().ok_or(AstError::InconsistentCst {
                    context: "ParenthesizedExpression",
                    expected: "expression",
                })?;
                self.lower_expression(value.syntax(), context)
            }
            PythonExpressionNode::Tuple(node) => {
                self.lower_sequence(node.syntax(), PythonAstKind::Tuple, context)
            }
            PythonExpressionNode::Array(node) => {
                self.lower_sequence(node.syntax(), PythonAstKind::List, context)
            }
            PythonExpressionNode::Set(node) => {
                self.lower_sequence(node.syntax(), PythonAstKind::Set, PythonAstKind::Load)
            }
            PythonExpressionNode::Dictionary(node) => self.lower_dictionary(node),
            PythonExpressionNode::Member(node) => self.lower_member(node, context),
            PythonExpressionNode::Call(node) => self.lower_call(node),
            PythonExpressionNode::Yield(node) => {
                let parts = node
                    .yield_parts()
                    .map_err(|error| AstError::InconsistentCst {
                        context: "YieldExpression",
                        expected: error.expected(),
                    })?;
                self.lower_yield(node.syntax(), &parts)
            }
            PythonExpressionNode::Named(node) => self.lower_named_expression(node),
            PythonExpressionNode::Lambda(node) => self.lower_lambda(node),
            PythonExpressionNode::Await(node) => self.lower_await(node),
            PythonExpressionNode::Unary(node) => self.lower_unary(node),
            PythonExpressionNode::Binary(node) => self.lower_binary_expression(node),
            PythonExpressionNode::Conditional(node) => {
                let body = node.body().ok_or(AstError::InconsistentCst {
                    context: "ConditionalExpression",
                    expected: "body",
                })?;
                let test = node.test().ok_or(AstError::InconsistentCst {
                    context: "ConditionalExpression",
                    expected: "test",
                })?;
                let orelse = node.orelse().ok_or(AstError::InconsistentCst {
                    context: "ConditionalExpression",
                    expected: "else expression",
                })?;
                self.lower_conditional(node.syntax(), &body, &test, &orelse)
            }
            PythonExpressionNode::Comprehension(node) => self.lower_comprehension(
                node.syntax().range(),
                &node
                    .comprehension()
                    .map_err(|error| AstError::InconsistentCst {
                        context: "ComprehensionExpression",
                        expected: error.expected(),
                    })?,
                PythonAstKind::GeneratorExp,
            ),
            PythonExpressionNode::ArrayComprehension(node) => self.lower_comprehension(
                node.syntax().range(),
                &node
                    .comprehension()
                    .map_err(|error| AstError::InconsistentCst {
                        context: "ArrayComprehensionExpression",
                        expected: error.expected(),
                    })?,
                PythonAstKind::ListComp,
            ),
            PythonExpressionNode::DictionaryComprehension(node) => self.lower_comprehension(
                node.syntax().range(),
                &node
                    .comprehension()
                    .map_err(|error| AstError::InconsistentCst {
                        context: "DictionaryComprehensionExpression",
                        expected: error.expected(),
                    })?,
                PythonAstKind::DictComp,
            ),
            PythonExpressionNode::SetComprehension(node) => self.lower_comprehension(
                node.syntax().range(),
                &node
                    .comprehension()
                    .map_err(|error| AstError::InconsistentCst {
                        context: "SetComprehensionExpression",
                        expected: error.expected(),
                    })?,
                PythonAstKind::SetComp,
            ),
        }
    }

    fn lower_constant(
        &mut self,
        node: &SyntaxNode,
        value: PythonConstant,
    ) -> Result<AstNodeId, AstError> {
        let result = self
            .ast
            .push_node(PythonAstKind::Constant, node.range().into())?;
        self.ast.push_field(
            result,
            PythonAstField::Value,
            PythonAstValue::Constant(value),
        )?;
        self.ast
            .push_field(result, PythonAstField::Kind, PythonAstValue::None)?;
        Ok(result)
    }

    fn lower_string(&mut self, node: &SyntaxNode) -> Result<AstNodeId, AstError> {
        let literal = parse_string_literal(self.node_text(node)?)?;
        self.lower_parsed_string(node, literal)
    }

    fn lower_interpolated_string(
        &mut self,
        string: &PythonInterpolatedString,
    ) -> Result<AstNodeId, AstError> {
        let values = self.lower_interpolated_parts(string.parts(), string.is_raw())?;
        let kind = match string.kind() {
            PythonInterpolatedStringKind::Format => PythonAstKind::JoinedStr,
            PythonInterpolatedStringKind::Template => PythonAstKind::TemplateStr,
        };
        let result = self.ast.push_node(kind, string.range().into())?;
        self.ast.push_field(
            result,
            PythonAstField::Values,
            PythonAstValue::Nodes(values),
        )?;
        Ok(result)
    }

    fn lower_interpolated_parts(
        &mut self,
        parts: &[PythonInterpolatedPart],
        raw: bool,
    ) -> Result<Vec<AstNodeId>, AstError> {
        let mut values = Vec::new();
        self.collect_interpolated_parts(parts, raw, &mut values)?;
        self.finish_interpolated_values(values)
    }

    fn collect_interpolated_parts(
        &mut self,
        parts: &[PythonInterpolatedPart],
        raw: bool,
        values: &mut Vec<LoweredInterpolatedValue>,
    ) -> Result<(), AstError> {
        for part in parts {
            match part {
                PythonInterpolatedPart::Literal { range, kind } => {
                    let spelling = self.source_text(*range)?;
                    let value = match kind {
                        PythonInterpolatedLiteralKind::Escaped => {
                            let spelling = collapse_interpolated_braces(spelling);
                            decode_code_points(&spelling, raw)?
                        }
                        PythonInterpolatedLiteralKind::Debug => {
                            spelling.chars().map(u32::from).collect()
                        }
                    };
                    append_interpolated_literal(values, value, *range);
                }
                PythonInterpolatedPart::Interpolation(interpolation) => {
                    let value = self.lower_interpolation(interpolation, raw)?;
                    values.push(LoweredInterpolatedValue::Node(value));
                }
            }
        }
        Ok(())
    }

    fn finish_interpolated_values(
        &mut self,
        values: Vec<LoweredInterpolatedValue>,
    ) -> Result<Vec<AstNodeId>, AstError> {
        let mut result = Vec::new();
        for value in values {
            match value {
                LoweredInterpolatedValue::Literal { value, range } if !value.is_empty() => {
                    result.push(self.lower_string_constant(&value, range)?);
                }
                LoweredInterpolatedValue::Literal { .. } => {}
                LoweredInterpolatedValue::Node(node) => result.push(node),
            }
        }
        Ok(result)
    }

    fn lower_interpolation(
        &mut self,
        interpolation: &PythonInterpolation,
        raw: bool,
    ) -> Result<AstNodeId, AstError> {
        let value = self.lower_expression_group(interpolation.expression(), PythonAstKind::Load)?;
        let conversion = match interpolation.conversion() {
            Some(node) => parse_conversion(self.node_text(node)?)?,
            None if interpolation.self_documenting().is_some() => i64::from(b'r'),
            None => -1,
        };
        let format_spec = interpolation
            .format_spec()
            .map(|spec| self.lower_format_spec(spec, raw))
            .transpose()?;
        let kind = match interpolation.kind() {
            PythonInterpolatedStringKind::Format => PythonAstKind::FormattedValue,
            PythonInterpolatedStringKind::Template => PythonAstKind::Interpolation,
        };
        let result = self.ast.push_node(kind, interpolation.range().into())?;
        self.ast
            .push_field(result, PythonAstField::Value, PythonAstValue::Node(value))?;
        if interpolation.kind() == PythonInterpolatedStringKind::Template {
            let spelling = self
                .source_text(interpolation.expression_source())?
                .to_owned();
            let spelling = self.ast.intern(&spelling)?;
            self.ast.push_field(
                result,
                PythonAstField::Str,
                PythonAstValue::String(spelling),
            )?;
        }
        let conversion = self.ast.intern(&conversion.to_string())?;
        self.ast.push_field(
            result,
            PythonAstField::Conversion,
            PythonAstValue::Integer(conversion),
        )?;
        self.ast.push_field(
            result,
            PythonAstField::FormatSpec,
            format_spec.map_or(PythonAstValue::None, PythonAstValue::Node),
        )?;
        Ok(result)
    }

    fn lower_format_spec(
        &mut self,
        spec: &PythonFormatSpec,
        raw: bool,
    ) -> Result<AstNodeId, AstError> {
        let parts =
            format_spec_parts(spec, self.source).map_err(|error| AstError::InconsistentCst {
                context: "FormatSpec",
                expected: error.expected(),
            })?;
        let values = self.lower_interpolated_parts(&parts, raw)?;
        let result = self
            .ast
            .push_node(PythonAstKind::JoinedStr, spec.syntax().range().into())?;
        self.ast.push_field(
            result,
            PythonAstField::Values,
            PythonAstValue::Nodes(values),
        )?;
        Ok(result)
    }

    fn lower_string_constant(
        &mut self,
        value: &[u32],
        range: rezel_common::TextRange,
    ) -> Result<AstNodeId, AstError> {
        let value = self.ast.intern_python_string(value)?;
        let result = self.ast.push_node(PythonAstKind::Constant, range.into())?;
        self.ast.push_field(
            result,
            PythonAstField::Value,
            PythonAstValue::Constant(PythonConstant::String(value)),
        )?;
        self.ast
            .push_field(result, PythonAstField::Kind, PythonAstValue::None)?;
        Ok(result)
    }

    fn lower_continued_string(
        &mut self,
        node: &PythonContinuedString,
    ) -> Result<AstNodeId, AstError> {
        let parts = node.parts().collect::<Vec<_>>();
        let has_format = parts
            .iter()
            .any(|part| matches!(part, PythonStringPart::Format(_)));
        let has_template = parts
            .iter()
            .any(|part| matches!(part, PythonStringPart::Template(_)));
        if has_template {
            if parts
                .iter()
                .any(|part| !matches!(part, PythonStringPart::Template(_)))
            {
                return Err(AstError::InvalidLiteral {
                    kind: "concatenated template string",
                    spelling: self.node_text(node.syntax())?.to_owned(),
                });
            }
            return self.lower_continued_interpolated(
                node,
                &parts,
                PythonInterpolatedStringKind::Template,
            );
        }
        if has_format {
            return self.lower_continued_interpolated(
                node,
                &parts,
                PythonInterpolatedStringKind::Format,
            );
        }
        let mut combined = None;
        let mut kind = None;
        for part in parts {
            let child = match part {
                PythonStringPart::String(child) => child,
                PythonStringPart::Format(_) | PythonStringPart::Template(_) => {
                    unreachable!("interpolated parts were handled above")
                }
            };
            let literal = parse_string_literal(self.node_text(child.syntax())?)?;
            match (&mut combined, literal) {
                (
                    None,
                    ParsedString::Text {
                        value,
                        kind: part_kind,
                    },
                ) => {
                    combined = Some(ParsedStringValue::Text(value));
                    kind = part_kind;
                }
                (None, ParsedString::Bytes(value)) => {
                    combined = Some(ParsedStringValue::Bytes(value));
                }
                (
                    Some(ParsedStringValue::Text(current)),
                    ParsedString::Text {
                        value,
                        kind: part_kind,
                    },
                ) => {
                    current.extend(value);
                    kind = kind.or(part_kind);
                }
                (Some(ParsedStringValue::Bytes(current)), ParsedString::Bytes(value)) => {
                    current.extend(value);
                }
                _ => {
                    return Err(AstError::InvalidLiteral {
                        kind: "concatenated string",
                        spelling: self.node_text(node.syntax())?.to_owned(),
                    });
                }
            }
        }
        let literal = match combined {
            Some(ParsedStringValue::Text(value)) => ParsedString::Text { value, kind },
            Some(ParsedStringValue::Bytes(value)) => ParsedString::Bytes(value),
            None => {
                return Err(AstError::InconsistentCst {
                    context: "ContinuedString",
                    expected: "string parts",
                });
            }
        };
        self.lower_parsed_string(node.syntax(), literal)
    }

    fn lower_continued_interpolated(
        &mut self,
        node: &PythonContinuedString,
        parts: &[PythonStringPart],
        kind: PythonInterpolatedStringKind,
    ) -> Result<AstNodeId, AstError> {
        let mut values = Vec::new();
        for part in parts {
            match part {
                PythonStringPart::String(string)
                    if kind == PythonInterpolatedStringKind::Format =>
                {
                    let literal = parse_string_literal(self.node_text(string.syntax())?)?;
                    let ParsedString::Text { value, .. } = literal else {
                        return Err(AstError::InvalidLiteral {
                            kind: "concatenated formatted string",
                            spelling: self.node_text(node.syntax())?.to_owned(),
                        });
                    };
                    append_interpolated_literal(&mut values, value, string.syntax().range());
                }
                PythonStringPart::Format(string)
                    if kind == PythonInterpolatedStringKind::Format =>
                {
                    let string = string.interpolated(self.source).map_err(|error| {
                        AstError::InconsistentCst {
                            context: "continued FormatString",
                            expected: error.expected(),
                        }
                    })?;
                    self.collect_interpolated_parts(string.parts(), string.is_raw(), &mut values)?;
                }
                PythonStringPart::Template(string)
                    if kind == PythonInterpolatedStringKind::Template =>
                {
                    let string = string.interpolated(self.source).map_err(|error| {
                        AstError::InconsistentCst {
                            context: "continued TemplateString",
                            expected: error.expected(),
                        }
                    })?;
                    self.collect_interpolated_parts(string.parts(), string.is_raw(), &mut values)?;
                }
                _ => {
                    return Err(AstError::InconsistentCst {
                        context: "ContinuedString",
                        expected: "compatible string parts",
                    });
                }
            }
        }
        let values = self.finish_interpolated_values(values)?;
        let ast_kind = match kind {
            PythonInterpolatedStringKind::Format => PythonAstKind::JoinedStr,
            PythonInterpolatedStringKind::Template => PythonAstKind::TemplateStr,
        };
        let result = self.ast.push_node(ast_kind, node.syntax().range().into())?;
        self.ast.push_field(
            result,
            PythonAstField::Values,
            PythonAstValue::Nodes(values),
        )?;
        Ok(result)
    }

    fn lower_parsed_string(
        &mut self,
        node: &SyntaxNode,
        literal: ParsedString,
    ) -> Result<AstNodeId, AstError> {
        let (value, kind) = match literal {
            ParsedString::Text { value, kind } => {
                let value = self.ast.intern_python_string(&value)?;
                let kind = kind
                    .map(|kind| self.ast.intern(kind).map(PythonAstValue::String))
                    .transpose()?
                    .unwrap_or(PythonAstValue::None);
                (PythonConstant::String(value), kind)
            }
            ParsedString::Bytes(value) => {
                let value = self.ast.intern_bytes(&value)?;
                (PythonConstant::Bytes(value), PythonAstValue::None)
            }
        };
        let result = self
            .ast
            .push_node(PythonAstKind::Constant, node.range().into())?;
        self.ast.push_field(
            result,
            PythonAstField::Value,
            PythonAstValue::Constant(value),
        )?;
        self.ast.push_field(result, PythonAstField::Kind, kind)?;
        Ok(result)
    }

    fn lower_sequence(
        &mut self,
        node: &SyntaxNode,
        kind: PythonAstKind,
        context: PythonAstKind,
    ) -> Result<AstNodeId, AstError> {
        let items = expression_items(node).map_err(|error| AstError::InconsistentCst {
            context: "sequence expression",
            expected: error.expected(),
        })?;
        let elements = items
            .iter()
            .map(|item| self.lower_expression_item(item, context))
            .collect::<Result<Vec<_>, _>>()?;
        let result = self.ast.push_node(kind, node.range().into())?;
        self.ast.push_field(
            result,
            PythonAstField::Elts,
            PythonAstValue::Nodes(elements),
        )?;
        if matches!(kind, PythonAstKind::Tuple | PythonAstKind::List) {
            let context = self.context_node(context)?;
            self.ast
                .push_field(result, PythonAstField::Ctx, PythonAstValue::Node(context))?;
        }
        Ok(result)
    }

    fn lower_dictionary(
        &mut self,
        node: &PythonDictionaryExpression,
    ) -> Result<AstNodeId, AstError> {
        let entries = node
            .dictionary_entries()
            .map_err(|error| AstError::InconsistentCst {
                context: "DictionaryExpression",
                expected: error.expected(),
            })?;
        let mut keys = Vec::new();
        let mut values = Vec::new();
        for entry in entries {
            match entry {
                PythonDictionaryEntry::Unpack { value, .. } => {
                    keys.push(None);
                    values.push(self.lower_expression(value.syntax(), PythonAstKind::Load)?);
                }
                PythonDictionaryEntry::Pair { key, value } => {
                    keys.push(Some(
                        self.lower_expression(key.syntax(), PythonAstKind::Load)?,
                    ));
                    values.push(self.lower_expression(value.syntax(), PythonAstKind::Load)?);
                }
            }
        }
        let result = self
            .ast
            .push_node(PythonAstKind::Dict, node.syntax().range().into())?;
        self.ast.push_field(
            result,
            PythonAstField::Keys,
            PythonAstValue::OptionalNodes(keys),
        )?;
        self.ast.push_field(
            result,
            PythonAstField::Values,
            PythonAstValue::Nodes(values),
        )?;
        Ok(result)
    }

    fn lower_member(
        &mut self,
        node: &PythonMemberExpression,
        context: PythonAstKind,
    ) -> Result<AstNodeId, AstError> {
        let access = node.access().map_err(|error| AstError::InconsistentCst {
            context: "MemberExpression",
            expected: error.expected(),
        })?;
        let value = self.lower_expression(access.base().syntax(), PythonAstKind::Load)?;
        if let PythonMemberSuffix::Attribute(property) = access.suffix() {
            let attribute = canonical_name(self.node_text(property.syntax())?);
            let attribute = self.ast.intern(&attribute)?;
            let context = self.context_node(context)?;
            let result = self
                .ast
                .push_node(PythonAstKind::Attribute, node.syntax().range().into())?;
            self.ast
                .push_field(result, PythonAstField::Value, PythonAstValue::Node(value))?;
            self.ast.push_field(
                result,
                PythonAstField::Attr,
                PythonAstValue::String(attribute),
            )?;
            self.ast
                .push_field(result, PythonAstField::Ctx, PythonAstValue::Node(context))?;
            return Ok(result);
        }
        let PythonMemberSuffix::Subscript(subscript) = access.suffix() else {
            unreachable!("attribute suffix returned above");
        };
        let slice = match subscript.items() {
            [item] => self.lower_subscript_item(item)?,
            items => {
                let elements = items
                    .iter()
                    .map(|item| self.lower_subscript_item(item))
                    .collect::<Result<Vec<_>, _>>()?;
                let tuple = self
                    .ast
                    .push_node(PythonAstKind::Tuple, subscript.range().into())?;
                self.ast.push_field(
                    tuple,
                    PythonAstField::Elts,
                    PythonAstValue::Nodes(elements),
                )?;
                let load = self.context_node(PythonAstKind::Load)?;
                self.ast
                    .push_field(tuple, PythonAstField::Ctx, PythonAstValue::Node(load))?;
                tuple
            }
        };
        let context = self.context_node(context)?;
        let result = self
            .ast
            .push_node(PythonAstKind::Subscript, node.syntax().range().into())?;
        self.ast
            .push_field(result, PythonAstField::Value, PythonAstValue::Node(value))?;
        self.ast
            .push_field(result, PythonAstField::Slice, PythonAstValue::Node(slice))?;
        self.ast
            .push_field(result, PythonAstField::Ctx, PythonAstValue::Node(context))?;
        Ok(result)
    }

    fn lower_subscript_item(&mut self, item: &PythonSubscriptItem) -> Result<AstNodeId, AstError> {
        match item {
            PythonSubscriptItem::Index(expression) => {
                self.lower_expression_item(expression, PythonAstKind::Load)
            }
            PythonSubscriptItem::Slice(slice) => self.lower_slice(slice),
        }
    }

    fn lower_slice(&mut self, slice: &PythonSlice) -> Result<AstNodeId, AstError> {
        let lower = slice
            .lower()
            .map(|expression| self.lower_expression(expression.syntax(), PythonAstKind::Load))
            .transpose()?;
        let upper = slice
            .upper()
            .map(|expression| self.lower_expression(expression.syntax(), PythonAstKind::Load))
            .transpose()?;
        let step = slice
            .step()
            .map(|expression| self.lower_expression(expression.syntax(), PythonAstKind::Load))
            .transpose()?;
        let result = self
            .ast
            .push_node(PythonAstKind::Slice, slice.range().into())?;
        self.ast.push_field(
            result,
            PythonAstField::Lower,
            lower.map_or(PythonAstValue::None, PythonAstValue::Node),
        )?;
        self.ast.push_field(
            result,
            PythonAstField::Upper,
            upper.map_or(PythonAstValue::None, PythonAstValue::Node),
        )?;
        self.ast.push_field(
            result,
            PythonAstField::Step,
            step.map_or(PythonAstValue::None, PythonAstValue::Node),
        )?;
        Ok(result)
    }

    fn lower_comprehension(
        &mut self,
        range: rezel_common::TextRange,
        comprehension: &PythonComprehension,
        kind: PythonAstKind,
    ) -> Result<AstNodeId, AstError> {
        let generators = comprehension
            .generators()
            .iter()
            .map(|generator| self.lower_comprehension_generator(generator))
            .collect::<Result<Vec<_>, _>>()?;
        let result = self.ast.push_node(kind, range.into())?;
        match (kind, comprehension.head()) {
            (
                PythonAstKind::GeneratorExp | PythonAstKind::ListComp | PythonAstKind::SetComp,
                PythonComprehensionHead::Element(PythonExpressionItem::Plain(element)),
            ) => {
                let element = self.lower_expression(element.syntax(), PythonAstKind::Load)?;
                self.ast
                    .push_field(result, PythonAstField::Elt, PythonAstValue::Node(element))?;
            }
            (PythonAstKind::DictComp, PythonComprehensionHead::KeyValue { key, value }) => {
                let key = self.lower_expression(key.syntax(), PythonAstKind::Load)?;
                let value = self.lower_expression(value.syntax(), PythonAstKind::Load)?;
                self.ast
                    .push_field(result, PythonAstField::Key, PythonAstValue::Node(key))?;
                self.ast
                    .push_field(result, PythonAstField::Value, PythonAstValue::Node(value))?;
            }
            (_, head) => {
                return Err(AstError::UnsupportedSyntax {
                    kind: match head {
                        PythonComprehensionHead::Element(PythonExpressionItem::Starred {
                            ..
                        }) => "starred comprehension element",
                        PythonComprehensionHead::DictionaryUnpack => {
                            "dictionary unpacking comprehension"
                        }
                        _ => "mismatched comprehension head",
                    }
                    .to_owned(),
                });
            }
        }
        self.ast.push_field(
            result,
            PythonAstField::Generators,
            PythonAstValue::Nodes(generators),
        )?;
        Ok(result)
    }

    fn lower_comprehension_generator(
        &mut self,
        generator: &PythonComprehensionGenerator,
    ) -> Result<AstNodeId, AstError> {
        let targets = generator.targets();
        let first = targets.first().ok_or(AstError::InconsistentCst {
            context: "comprehension target",
            expected: "at least one expression",
        })?;
        let last = targets.last().expect("first target was present");
        let range = rezel_common::TextRange::new(first.range().start(), last.range().end());
        let target = self.lower_expression_items_with_sequence(
            targets,
            generator.target_is_sequence(),
            range,
            PythonAstKind::Store,
        )?;
        let iterator = self.lower_expression(generator.iterator().syntax(), PythonAstKind::Load)?;
        let filters = generator
            .filters()
            .iter()
            .map(|filter| self.lower_expression(filter.syntax(), PythonAstKind::Load))
            .collect::<Result<Vec<_>, _>>()?;
        let asynchronous = u8::from(generator.is_async());
        let asynchronous = self.ast.intern(&asynchronous.to_string())?;
        let result = self.ast.push_node(
            PythonAstKind::AbstractComprehension,
            PythonSourceRange::default(),
        )?;
        self.ast
            .push_field(result, PythonAstField::Target, PythonAstValue::Node(target))?;
        self.ast
            .push_field(result, PythonAstField::Iter, PythonAstValue::Node(iterator))?;
        self.ast
            .push_field(result, PythonAstField::Ifs, PythonAstValue::Nodes(filters))?;
        self.ast.push_field(
            result,
            PythonAstField::IsAsync,
            PythonAstValue::Integer(asynchronous),
        )?;
        Ok(result)
    }

    fn lower_call(&mut self, node: &PythonCallExpression) -> Result<AstNodeId, AstError> {
        let function = node.function().ok_or(AstError::InconsistentCst {
            context: "CallExpression",
            expected: "callable expression",
        })?;
        let arguments = node.arguments().ok_or(AstError::InconsistentCst {
            context: "CallExpression",
            expected: "argument list",
        })?;
        let function = self.lower_expression(function.syntax(), PythonAstKind::Load)?;
        let (args, keywords) = self.lower_arguments(&arguments)?;
        let result = self
            .ast
            .push_node(PythonAstKind::Call, node.syntax().range().into())?;
        self.ast
            .push_field(result, PythonAstField::Func, PythonAstValue::Node(function))?;
        self.ast
            .push_field(result, PythonAstField::Args, PythonAstValue::Nodes(args))?;
        self.ast.push_field(
            result,
            PythonAstField::Keywords,
            PythonAstValue::Nodes(keywords),
        )?;
        Ok(result)
    }

    #[allow(clippy::too_many_lines)] // Each argument shape is handled in one linear pass.
    fn lower_arguments(
        &mut self,
        node: &PythonArgList,
    ) -> Result<(Vec<AstNodeId>, Vec<AstNodeId>), AstError> {
        let arguments = node
            .call_arguments()
            .map_err(|error| AstError::InconsistentCst {
                context: "ArgList",
                expected: error.expected(),
            })?;
        let mut args = Vec::new();
        let mut keywords = Vec::new();
        for argument in arguments {
            match argument {
                PythonCallArgument::Positional(value) => {
                    args.push(self.lower_expression(value.syntax(), PythonAstKind::Load)?);
                }
                PythonCallArgument::Starred { value, range } => {
                    let value = self.lower_expression(value.syntax(), PythonAstKind::Load)?;
                    let starred = self.ast.push_node(PythonAstKind::Starred, range.into())?;
                    self.ast.push_field(
                        starred,
                        PythonAstField::Value,
                        PythonAstValue::Node(value),
                    )?;
                    let load = self.context_node(PythonAstKind::Load)?;
                    self.ast.push_field(
                        starred,
                        PythonAstField::Ctx,
                        PythonAstValue::Node(load),
                    )?;
                    args.push(starred);
                }
                PythonCallArgument::KeywordUnpack { value, range } => {
                    let value = self.lower_expression(value.syntax(), PythonAstKind::Load)?;
                    let keyword = self
                        .ast
                        .push_node(PythonAstKind::AbstractKeyword, range.into())?;
                    self.ast
                        .push_field(keyword, PythonAstField::Arg, PythonAstValue::None)?;
                    self.ast.push_field(
                        keyword,
                        PythonAstField::Value,
                        PythonAstValue::Node(value),
                    )?;
                    keywords.push(keyword);
                }
                PythonCallArgument::Assigned {
                    name,
                    operator,
                    value,
                    range,
                } => {
                    let spelling = self.node_text(&operator)?;
                    if spelling == "=" {
                        let name = canonical_name(self.node_text(name.syntax())?);
                        let name = self.ast.intern(&name)?;
                        let value = self.lower_expression(value.syntax(), PythonAstKind::Load)?;
                        let keyword = self
                            .ast
                            .push_node(PythonAstKind::AbstractKeyword, range.into())?;
                        self.ast.push_field(
                            keyword,
                            PythonAstField::Arg,
                            PythonAstValue::String(name),
                        )?;
                        self.ast.push_field(
                            keyword,
                            PythonAstField::Value,
                            PythonAstValue::Node(value),
                        )?;
                        keywords.push(keyword);
                    } else if spelling == ":=" {
                        let target = self.lower_expression(name.syntax(), PythonAstKind::Store)?;
                        let value = self.lower_expression(value.syntax(), PythonAstKind::Load)?;
                        let named = self.ast.push_node(PythonAstKind::NamedExpr, range.into())?;
                        self.ast.push_field(
                            named,
                            PythonAstField::Target,
                            PythonAstValue::Node(target),
                        )?;
                        self.ast.push_field(
                            named,
                            PythonAstField::Value,
                            PythonAstValue::Node(value),
                        )?;
                        args.push(named);
                    } else {
                        return Err(AstError::UnsupportedSyntax {
                            kind: spelling.to_owned(),
                        });
                    }
                }
                PythonCallArgument::Generator {
                    comprehension,
                    range,
                } => {
                    args.push(self.lower_comprehension(
                        range,
                        &comprehension,
                        PythonAstKind::GeneratorExp,
                    )?);
                }
            }
        }
        Ok((args, keywords))
    }

    fn lower_named_expression(
        &mut self,
        node: &PythonNamedExpression,
    ) -> Result<AstNodeId, AstError> {
        let target = node.target().ok_or(AstError::InconsistentCst {
            context: "NamedExpression",
            expected: "name target",
        })?;
        let value = node.value().ok_or(AstError::InconsistentCst {
            context: "NamedExpression",
            expected: "value",
        })?;
        let target = self.lower_expression(target.syntax(), PythonAstKind::Store)?;
        let value = self.lower_expression(value.syntax(), PythonAstKind::Load)?;
        let result = self
            .ast
            .push_node(PythonAstKind::NamedExpr, node.syntax().range().into())?;
        self.ast
            .push_field(result, PythonAstField::Target, PythonAstValue::Node(target))?;
        self.ast
            .push_field(result, PythonAstField::Value, PythonAstValue::Node(value))?;
        Ok(result)
    }

    fn lower_lambda(&mut self, node: &PythonLambdaExpression) -> Result<AstNodeId, AstError> {
        let parameters = node.parameters().ok_or(AstError::InconsistentCst {
            context: "LambdaExpression",
            expected: "lambda parameters",
        })?;
        let parameters =
            parameters
                .parameters(self.source)
                .map_err(|error| AstError::InconsistentCst {
                    context: "lambda ParamList",
                    expected: error.expected(),
                })?;
        let arguments = self.lower_parameters(&parameters)?;
        let body = node.body().ok_or(AstError::InconsistentCst {
            context: "LambdaExpression",
            expected: "a lambda body",
        })?;
        let body = self.lower_expression(body.syntax(), PythonAstKind::Load)?;
        let result = self
            .ast
            .push_node(PythonAstKind::Lambda, node.syntax().range().into())?;
        self.ast.push_field(
            result,
            PythonAstField::Args,
            PythonAstValue::Node(arguments),
        )?;
        self.ast
            .push_field(result, PythonAstField::Body, PythonAstValue::Node(body))?;
        Ok(result)
    }

    fn lower_await(&mut self, node: &PythonAwaitExpression) -> Result<AstNodeId, AstError> {
        let value = node.value().ok_or(AstError::InconsistentCst {
            context: "AwaitExpression",
            expected: "awaited expression",
        })?;
        let value = self.lower_expression(value.syntax(), PythonAstKind::Load)?;
        let result = self
            .ast
            .push_node(PythonAstKind::Await, node.syntax().range().into())?;
        self.ast
            .push_field(result, PythonAstField::Value, PythonAstValue::Node(value))?;
        Ok(result)
    }

    fn lower_unary(&mut self, node: &PythonUnaryExpression) -> Result<AstNodeId, AstError> {
        let expression = PythonExpressionNode::Unary(node.clone());
        if let Some(boolean) =
            boolean_expression(&expression).map_err(|error| AstError::InconsistentCst {
                context: "boolean expression",
                expected: error.expected(),
            })?
        {
            return self.lower_boolean(&boolean);
        }
        let operation = node
            .operation()
            .map_err(|error| AstError::InconsistentCst {
                context: "UnaryExpression",
                expected: error.expected(),
            })?;
        let operator_text = self.operator_spelling(operation.operator())?;
        let operator = match operator_text.as_str() {
            "+" => PythonAstKind::UAdd,
            "-" => PythonAstKind::USub,
            "~" => PythonAstKind::Invert,
            "not" => PythonAstKind::Not,
            kind => {
                return Err(AstError::UnsupportedSyntax {
                    kind: kind.to_owned(),
                });
            }
        };
        let operand = self.lower_expression(operation.operand().syntax(), PythonAstKind::Load)?;
        let operator = self.ast.push_node(operator, PythonSourceRange::default())?;
        let result = self
            .ast
            .push_node(PythonAstKind::UnaryOp, node.syntax().range().into())?;
        self.ast
            .push_field(result, PythonAstField::Op, PythonAstValue::Node(operator))?;
        self.ast.push_field(
            result,
            PythonAstField::Operand,
            PythonAstValue::Node(operand),
        )?;
        Ok(result)
    }

    fn lower_binary_expression(
        &mut self,
        node: &PythonBinaryExpression,
    ) -> Result<AstNodeId, AstError> {
        let operation = node
            .operation()
            .map_err(|error| AstError::InconsistentCst {
                context: "BinaryExpression",
                expected: error.expected(),
            })?;
        let operator = self.operator_spelling(operation.operator())?;
        match operator.as_str() {
            "and" | "or" => {
                let expression = PythonExpressionNode::Binary(node.clone());
                let boolean = boolean_expression(&expression)
                    .map_err(|error| AstError::InconsistentCst {
                        context: "boolean expression",
                        expected: error.expected(),
                    })?
                    .ok_or(AstError::InconsistentCst {
                        context: "boolean expression",
                        expected: "at least one and/or operator",
                    })?;
                self.lower_boolean(&boolean)
            }
            "<" | "<=" | "==" | "!=" | ">" | ">=" | "in" | "not in" | "is" | "is not" => {
                self.lower_compare(node)
            }
            _ => self.lower_binary(node),
        }
    }

    fn lower_binary(&mut self, node: &PythonBinaryExpression) -> Result<AstNodeId, AstError> {
        let operation = node
            .operation()
            .map_err(|error| AstError::InconsistentCst {
                context: "BinaryExpression",
                expected: error.expected(),
            })?;
        let left = self.lower_expression(operation.left().syntax(), PythonAstKind::Load)?;
        let right = self.lower_expression(operation.right().syntax(), PythonAstKind::Load)?;
        let spelling = self.operator_spelling(operation.operator())?;
        let Some(operator) = binary_operator_kind(&spelling) else {
            return Err(AstError::UnsupportedSyntax { kind: spelling });
        };
        let operator = self.ast.push_node(operator, PythonSourceRange::default())?;
        let result = self
            .ast
            .push_node(PythonAstKind::BinOp, node.syntax().range().into())?;
        self.ast
            .push_field(result, PythonAstField::Left, PythonAstValue::Node(left))?;
        self.ast
            .push_field(result, PythonAstField::Op, PythonAstValue::Node(operator))?;
        self.ast
            .push_field(result, PythonAstField::Right, PythonAstValue::Node(right))?;
        Ok(result)
    }

    fn lower_boolean(
        &mut self,
        expression: &PythonBooleanExpression,
    ) -> Result<AstNodeId, AstError> {
        let terms = expression.terms();
        let operators = expression.operators();
        let first = terms.first().ok_or(AstError::InconsistentCst {
            context: "boolean expression",
            expected: "at least one term",
        })?;
        let last = terms.last().expect("first term was present");
        let mut and_start = first.start();
        let mut and_values = Vec::new();
        let mut or_values = Vec::new();

        for (index, term) in terms.iter().enumerate() {
            and_values.push(self.lower_boolean_term(term)?);
            match operators.get(index) {
                Some(PythonBooleanOperator::And) => {}
                Some(PythonBooleanOperator::Or) | None => {
                    let value = if and_values.len() == 1 {
                        and_values.pop().expect("one and value was present")
                    } else {
                        let range = rezel_common::TextRange::new(and_start, term.end());
                        self.finish_bool("and", range, std::mem::take(&mut and_values))?
                    };
                    or_values.push(value);
                    if let Some(next) = terms.get(index + 1) {
                        and_start = next.start();
                    }
                }
            }
        }

        if or_values.len() == 1 {
            return Ok(or_values.pop().expect("one or value was present"));
        }
        let range = rezel_common::TextRange::new(first.start(), last.end());
        self.finish_bool("or", range, or_values)
    }

    fn lower_boolean_term(&mut self, term: &PythonBooleanTerm) -> Result<AstNodeId, AstError> {
        let value = self.lower_expression(term.expression().syntax(), PythonAstKind::Load)?;
        term.not_starts()
            .iter()
            .rev()
            .try_fold(value, |value, start| {
                let operator = self
                    .ast
                    .push_node(PythonAstKind::Not, PythonSourceRange::default())?;
                let range = rezel_common::TextRange::new(*start, term.end());
                let result = self.ast.push_node(PythonAstKind::UnaryOp, range.into())?;
                self.ast
                    .push_field(result, PythonAstField::Op, PythonAstValue::Node(operator))?;
                self.ast.push_field(
                    result,
                    PythonAstField::Operand,
                    PythonAstValue::Node(value),
                )?;
                Ok(result)
            })
    }

    fn finish_bool(
        &mut self,
        operator: &str,
        range: rezel_common::TextRange,
        values: Vec<AstNodeId>,
    ) -> Result<AstNodeId, AstError> {
        let operator = match operator {
            "and" => PythonAstKind::And,
            "or" => PythonAstKind::Or,
            _ => unreachable!("boolean operator was classified"),
        };
        let operator = self.ast.push_node(operator, PythonSourceRange::default())?;
        let result = self.ast.push_node(PythonAstKind::BoolOp, range.into())?;
        self.ast
            .push_field(result, PythonAstField::Op, PythonAstValue::Node(operator))?;
        self.ast.push_field(
            result,
            PythonAstField::Values,
            PythonAstValue::Nodes(values),
        )?;
        Ok(result)
    }

    fn lower_compare(&mut self, node: &PythonBinaryExpression) -> Result<AstNodeId, AstError> {
        let mut operands = Vec::new();
        let mut operators = Vec::new();
        self.collect_comparison(node, &mut operands, &mut operators)?;
        let mut operands = operands.into_iter();
        let left = operands.next().ok_or(AstError::InconsistentCst {
            context: "BinaryExpression",
            expected: "comparison left operand",
        })?;
        let left = self.lower_expression(left.syntax(), PythonAstKind::Load)?;
        let comparators = operands
            .map(|operand| self.lower_expression(operand.syntax(), PythonAstKind::Load))
            .collect::<Result<Vec<_>, _>>()?;
        let operators = operators
            .into_iter()
            .map(|operator| {
                let kind =
                    comparison_kind(&operator).ok_or_else(|| AstError::UnsupportedSyntax {
                        kind: operator.clone(),
                    })?;
                self.ast.push_node(kind, PythonSourceRange::default())
            })
            .collect::<Result<Vec<_>, _>>()?;
        let result = self
            .ast
            .push_node(PythonAstKind::Compare, node.syntax().range().into())?;
        self.ast
            .push_field(result, PythonAstField::Left, PythonAstValue::Node(left))?;
        self.ast.push_field(
            result,
            PythonAstField::Ops,
            PythonAstValue::Nodes(operators),
        )?;
        self.ast.push_field(
            result,
            PythonAstField::Comparators,
            PythonAstValue::Nodes(comparators),
        )?;
        Ok(result)
    }

    fn collect_comparison(
        &self,
        node: &PythonBinaryExpression,
        operands: &mut Vec<PythonExpressionNode>,
        operators: &mut Vec<String>,
    ) -> Result<(), AstError> {
        let operation = node
            .operation()
            .map_err(|error| AstError::InconsistentCst {
                context: "BinaryExpression",
                expected: error.expected(),
            })?;
        let operator = self.operator_spelling(operation.operator())?;
        if let PythonExpressionNode::Binary(left) = operation.left() {
            let nested = left
                .operation()
                .map_err(|error| AstError::InconsistentCst {
                    context: "BinaryExpression",
                    expected: error.expected(),
                })?;
            let nested_operator = self.operator_spelling(nested.operator())?;
            if comparison_kind(&nested_operator).is_some() {
                self.collect_comparison(left, operands, operators)?;
            } else {
                operands.push(operation.left().clone());
            }
        } else {
            operands.push(operation.left().clone());
        }
        operators.push(operator);
        operands.push(operation.right().clone());
        Ok(())
    }

    fn lower_conditional(
        &mut self,
        node: &SyntaxNode,
        body: &PythonExpressionNode,
        test: &PythonExpressionNode,
        orelse: &PythonExpressionNode,
    ) -> Result<AstNodeId, AstError> {
        let body = self.lower_expression(body.syntax(), PythonAstKind::Load)?;
        let test = self.lower_expression(test.syntax(), PythonAstKind::Load)?;
        let orelse = self.lower_expression(orelse.syntax(), PythonAstKind::Load)?;
        let result = self
            .ast
            .push_node(PythonAstKind::IfExp, node.range().into())?;
        self.ast
            .push_field(result, PythonAstField::Test, PythonAstValue::Node(test))?;
        self.ast
            .push_field(result, PythonAstField::Body, PythonAstValue::Node(body))?;
        self.ast
            .push_field(result, PythonAstField::Orelse, PythonAstValue::Node(orelse))?;
        Ok(result)
    }

    fn operator_spelling(&self, operator: &PythonOperator) -> Result<String, AstError> {
        let spelling = match operator {
            PythonOperator::Symbol(token) => self.node_text(token)?.to_owned(),
            PythonOperator::And => "and".to_owned(),
            PythonOperator::Or => "or".to_owned(),
            PythonOperator::In => "in".to_owned(),
            PythonOperator::NotIn => "not in".to_owned(),
            PythonOperator::Is => "is".to_owned(),
            PythonOperator::IsNot => "is not".to_owned(),
            PythonOperator::Not => "not".to_owned(),
        };
        Ok(spelling)
    }

    fn context_node(&mut self, context: PythonAstKind) -> Result<AstNodeId, AstError> {
        self.ast.push_node(context, PythonSourceRange::default())
    }

    fn lower_type_ignores(&mut self) -> Result<Vec<AstNodeId>, AstError> {
        let mut result = Vec::new();
        for (index, line) in self.source.split_inclusive('\n').enumerate() {
            let Some(comment) = line.split_once("# type: ignore").map(|(_, value)| value) else {
                continue;
            };
            let node = self
                .ast
                .push_node(PythonAstKind::TypeIgnore, PythonSourceRange::default())?;
            let line = self.ast.intern(&(index + 1).to_string())?;
            let tag = self.ast.intern(comment)?;
            self.ast
                .push_field(node, PythonAstField::Lineno, PythonAstValue::Integer(line))?;
            self.ast
                .push_field(node, PythonAstField::Tag, PythonAstValue::String(tag))?;
            result.push(node);
        }
        Ok(result)
    }

    fn type_comment_after(&self, node: &SyntaxNode) -> Option<(&str, TextSize)> {
        let end = usize::from(node.to());
        let line_end = self.source[end..]
            .find('\n')
            .map_or(self.source.len(), |offset| end + offset);
        let suffix = &self.source[end..line_end];
        let comment = suffix
            .split_once("# type:")
            .map(|(_, comment)| comment.trim())
            .filter(|comment| !comment.starts_with("ignore"))?;
        let line_end = TextSize::try_from(line_end).ok()?;
        Some((comment, line_end))
    }

    fn node_text(&self, node: &SyntaxNode) -> Result<&str, AstError> {
        self.source_text(node.range())
    }

    fn source_text(&self, range: rezel_common::TextRange) -> Result<&str, AstError> {
        self.source
            .get(usize::from(range.start())..usize::from(range.end()))
            .ok_or(AstError::InconsistentCst {
                context: "source range",
                expected: "UTF-8 boundaries inside source",
            })
    }
}

fn comparison_kind(operator: &str) -> Option<PythonAstKind> {
    Some(match operator {
        "==" => PythonAstKind::Eq,
        "!=" => PythonAstKind::NotEq,
        "<" => PythonAstKind::Lt,
        "<=" => PythonAstKind::LtE,
        ">" => PythonAstKind::Gt,
        ">=" => PythonAstKind::GtE,
        "is" => PythonAstKind::Is,
        "is not" => PythonAstKind::IsNot,
        "in" => PythonAstKind::In,
        "not in" => PythonAstKind::NotIn,
        _ => return None,
    })
}

fn binary_operator_kind(operator: &str) -> Option<PythonAstKind> {
    Some(match operator {
        "+" => PythonAstKind::Add,
        "-" => PythonAstKind::Sub,
        "*" => PythonAstKind::Mult,
        "@" => PythonAstKind::MatMult,
        "/" => PythonAstKind::Div,
        "//" => PythonAstKind::FloorDiv,
        "%" => PythonAstKind::Mod,
        "**" => PythonAstKind::Pow,
        "|" => PythonAstKind::BitOr,
        "^" => PythonAstKind::BitXor,
        "&" => PythonAstKind::BitAnd,
        "<<" => PythonAstKind::LShift,
        ">>" => PythonAstKind::RShift,
        _ => return None,
    })
}

enum ParsedNumber {
    Integer(String),
    Float(u64),
    Imaginary(u64),
}

enum ParsedString {
    Text {
        value: Vec<u32>,
        kind: Option<&'static str>,
    },
    Bytes(Vec<u8>),
}

enum ParsedStringValue {
    Text(Vec<u32>),
    Bytes(Vec<u8>),
}

enum LoweredInterpolatedValue {
    Literal {
        value: Vec<u32>,
        range: rezel_common::TextRange,
    },
    Node(AstNodeId),
}

fn append_interpolated_literal(
    values: &mut Vec<LoweredInterpolatedValue>,
    value: Vec<u32>,
    range: rezel_common::TextRange,
) {
    if let Some(LoweredInterpolatedValue::Literal {
        value: previous,
        range: previous_range,
    }) = values.last_mut()
    {
        previous.extend(value);
        *previous_range = rezel_common::TextRange::new(previous_range.start(), range.end());
        return;
    }
    values.push(LoweredInterpolatedValue::Literal { value, range });
}

fn parse_conversion(spelling: &str) -> Result<i64, AstError> {
    let conversion = spelling
        .strip_prefix('!')
        .and_then(|value| {
            let mut characters = value.chars();
            let conversion = characters.next()?;
            characters.next().is_none().then_some(conversion)
        })
        .filter(|conversion| matches!(conversion, 'a' | 'r' | 's'))
        .ok_or_else(|| AstError::InvalidLiteral {
            kind: "format conversion",
            spelling: spelling.to_owned(),
        })?;
    Ok(i64::from(u32::from(conversion)))
}

fn collapse_interpolated_braces(spelling: &str) -> String {
    let mut characters = spelling.chars().peekable();
    let mut result = String::with_capacity(spelling.len());
    while let Some(character) = characters.next() {
        if matches!(character, '{' | '}') && characters.peek() == Some(&character) {
            characters.next();
        }
        result.push(character);
    }
    result
}

fn parse_number(spelling: &str) -> Option<ParsedNumber> {
    let compact = spelling.replace('_', "");
    let (number, imaginary) = compact
        .strip_suffix(['j', 'J'])
        .map_or((compact.as_str(), false), |number| (number, true));
    if imaginary {
        return number
            .parse::<f64>()
            .ok()
            .map(f64::to_bits)
            .map(ParsedNumber::Imaginary);
    }
    if number
        .get(..2)
        .is_some_and(|prefix| matches!(prefix, "0b" | "0B" | "0o" | "0O" | "0x" | "0X"))
    {
        return canonical_integer(number).map(ParsedNumber::Integer);
    }
    if number.contains(['.', 'e', 'E']) {
        return number
            .parse::<f64>()
            .ok()
            .map(f64::to_bits)
            .map(ParsedNumber::Float);
    }
    canonical_integer(number).map(ParsedNumber::Integer)
}

fn canonical_integer(spelling: &str) -> Option<String> {
    let (base, digits) = if let Some(digits) = spelling
        .strip_prefix("0b")
        .or_else(|| spelling.strip_prefix("0B"))
    {
        (2_u8, digits)
    } else if let Some(digits) = spelling
        .strip_prefix("0o")
        .or_else(|| spelling.strip_prefix("0O"))
    {
        (8_u8, digits)
    } else if let Some(digits) = spelling
        .strip_prefix("0x")
        .or_else(|| spelling.strip_prefix("0X"))
    {
        (16_u8, digits)
    } else {
        (10_u8, spelling)
    };
    if digits.is_empty() {
        return None;
    }
    let mut decimal = vec![0_u8];
    for character in digits.chars() {
        let digit = character.to_digit(u32::from(base))?;
        let mut carry = digit;
        for value in &mut decimal {
            let next = u32::from(*value) * u32::from(base) + carry;
            *value = u8::try_from(next % 10).ok()?;
            carry = next / 10;
        }
        while carry != 0 {
            decimal.push(u8::try_from(carry % 10).ok()?);
            carry /= 10;
        }
    }
    while decimal.len() > 1 && decimal.last() == Some(&0) {
        decimal.pop();
    }
    Some(
        decimal
            .iter()
            .rev()
            .map(|digit| char::from(b'0' + *digit))
            .collect(),
    )
}

fn parse_string_literal(spelling: &str) -> Result<ParsedString, AstError> {
    let quote = spelling
        .char_indices()
        .find(|(_, character)| matches!(character, '\'' | '"'))
        .ok_or_else(|| invalid_string(spelling))?;
    let prefix = spelling[..quote.0].to_ascii_lowercase();
    let raw = prefix.contains('r');
    let bytes = prefix.contains('b');
    let delimiter = if spelling[quote.0..].starts_with(&quote.1.to_string().repeat(3)) {
        quote.1.to_string().repeat(3)
    } else {
        quote.1.to_string()
    };
    let content_start = quote.0 + delimiter.len();
    let content_end = spelling
        .len()
        .checked_sub(delimiter.len())
        .filter(|end| *end >= content_start)
        .ok_or_else(|| invalid_string(spelling))?;
    if spelling.get(content_end..) != Some(delimiter.as_str()) {
        return Err(invalid_string(spelling));
    }
    let content = &spelling[content_start..content_end];
    if bytes {
        let value = decode_bytes(content, raw).ok_or_else(|| invalid_string(spelling))?;
        return Ok(ParsedString::Bytes(value));
    }
    let value = decode_code_points(content, raw)?;
    let kind = prefix.contains('u').then_some("u");
    Ok(ParsedString::Text { value, kind })
}

fn decode_code_points(content: &str, raw: bool) -> Result<Vec<u32>, AstError> {
    if raw {
        return Ok(content.chars().map(u32::from).collect());
    }
    let mut characters = content.chars().peekable();
    let mut result = Vec::new();
    while let Some(character) = characters.next() {
        if character != '\\' {
            result.push(u32::from(character));
            continue;
        }
        let Some(escaped) = characters.next() else {
            result.push(u32::from('\\'));
            break;
        };
        match escaped {
            '\n' => {}
            '\r' => {
                if characters.peek() == Some(&'\n') {
                    characters.next();
                }
            }
            '\\' | '\'' | '"' => result.push(u32::from(escaped)),
            'a' => result.push(0x07),
            'b' => result.push(0x08),
            'f' => result.push(0x0c),
            'n' => result.push(0x0a),
            'r' => result.push(0x0d),
            't' => result.push(0x09),
            'v' => result.push(0x0b),
            '0'..='7' => {
                let value = read_octal(escaped, &mut characters);
                result.push(value);
            }
            'x' => result.push(read_hex(&mut characters, 2).ok_or_else(|| {
                AstError::InvalidLiteral {
                    kind: "string escape",
                    spelling: content.to_owned(),
                }
            })?),
            'u' => result.push(read_hex(&mut characters, 4).ok_or_else(|| {
                AstError::InvalidLiteral {
                    kind: "string escape",
                    spelling: content.to_owned(),
                }
            })?),
            'U' => {
                let value =
                    read_hex(&mut characters, 8).ok_or_else(|| AstError::InvalidLiteral {
                        kind: "string escape",
                        spelling: content.to_owned(),
                    })?;
                if value > 0x10_ffff {
                    return Err(AstError::InvalidLiteral {
                        kind: "string escape",
                        spelling: content.to_owned(),
                    });
                }
                result.push(value);
            }
            'N' => {
                if characters.next() != Some('{') {
                    return Err(AstError::InvalidLiteral {
                        kind: "named Unicode escape",
                        spelling: content.to_owned(),
                    });
                }
                let mut name = String::new();
                loop {
                    let Some(character) = characters.next() else {
                        return Err(AstError::InvalidLiteral {
                            kind: "named Unicode escape",
                            spelling: content.to_owned(),
                        });
                    };
                    if character == '}' {
                        break;
                    }
                    name.push(character);
                }
                let value = crate::unicode_names::character(&name).ok_or_else(|| {
                    AstError::InvalidLiteral {
                        kind: "named Unicode escape",
                        spelling: content.to_owned(),
                    }
                })?;
                result.push(value);
            }
            other => {
                result.push(u32::from('\\'));
                result.push(u32::from(other));
            }
        }
    }
    Ok(result)
}

fn decode_bytes(content: &str, raw: bool) -> Option<Vec<u8>> {
    let mut characters = content.chars().peekable();
    let mut result = Vec::new();
    while let Some(character) = characters.next() {
        if character != '\\' || raw {
            result.push(u8::try_from(u32::from(character)).ok()?);
            continue;
        }
        let Some(escaped) = characters.next() else {
            result.push(b'\\');
            break;
        };
        match escaped {
            '\n' => {}
            '\r' => {
                if characters.peek() == Some(&'\n') {
                    characters.next();
                }
            }
            '\\' | '\'' | '"' => result.push(u8::try_from(u32::from(escaped)).ok()?),
            'a' => result.push(0x07),
            'b' => result.push(0x08),
            'f' => result.push(0x0c),
            'n' => result.push(b'\n'),
            'r' => result.push(b'\r'),
            't' => result.push(b'\t'),
            'v' => result.push(0x0b),
            '0'..='7' => {
                let value = read_octal(escaped, &mut characters) & 0xff;
                result.push(u8::try_from(value).expect("masked to one byte"));
            }
            'x' => result.push(u8::try_from(read_hex(&mut characters, 2)?).ok()?),
            other => {
                result.push(b'\\');
                result.push(u8::try_from(u32::from(other)).ok()?);
            }
        }
    }
    Some(result)
}

fn read_octal(first: char, characters: &mut std::iter::Peekable<std::str::Chars<'_>>) -> u32 {
    let mut value = first.to_digit(8).expect("octal digit");
    for _ in 0..2 {
        let Some(next) = characters
            .peek()
            .and_then(|character| character.to_digit(8))
        else {
            break;
        };
        characters.next();
        value = value * 8 + next;
    }
    value
}

fn read_hex(
    characters: &mut std::iter::Peekable<std::str::Chars<'_>>,
    count: usize,
) -> Option<u32> {
    let mut value = 0_u32;
    for _ in 0..count {
        value = value
            .checked_mul(16)?
            .checked_add(characters.next()?.to_digit(16)?)?;
    }
    Some(value)
}

fn invalid_string(spelling: &str) -> AstError {
    AstError::InvalidLiteral {
        kind: "string",
        spelling: spelling.to_owned(),
    }
}

fn reject_recovery_tree(tree: &Tree) -> Result<(), AstError> {
    let mut cursor = tree.cursor(IterMode::INCLUDE_ANONYMOUS);
    loop {
        if cursor.node_type().is_error() {
            return Err(AstError::RecoveryTree);
        }
        if !cursor.next(true) {
            return Ok(());
        }
    }
}
