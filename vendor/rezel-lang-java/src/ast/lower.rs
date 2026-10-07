use std::sync::Arc;

use rezel_common::{
    Input, IterMode, StringInput, SyntaxLanguage, SyntaxNode, TextRange, TextSize, Tree, TypedNode,
};

use super::model::{
    AstBuilder, AstError, AstNodeId, JavaAst, JavaAstField, JavaAstKind, JavaAstProperty,
    JavaLambdaBodyKind, JavaModifier, JavaModuleKind, JavaPrimitiveKind, JavaReferenceMode,
    JavaSourceRange,
};
use crate::identifier::canonical_name;
use crate::input::JavaInput;
use crate::syntax::{
    CallableBody, CallableShape, CompactMember, CompactMemberKind, ConstructorInvocationTarget,
    FieldAccessSelection, ModifierItem, ParameterKind, ParameterShape, TypeDeclarationKind,
    TypeDeclarationShape, TypeMember, TypeMemberKind, UnitMember, callable_shape,
    constructor_invocation_target, declarator_entries, field_access_selection, is_compact_unit,
    local_type_declaration_shape, modifier_items, parameter_shape, resource_entries,
    scoped_name_parts, try_for_each_class_member, try_for_each_type_member,
    try_for_each_unit_member, type_declaration_shape,
};
use crate::typed::{
    JavaAnnotationNode, JavaAnnotationValue, JavaAssertStatement, JavaBlock, JavaCatchClause,
    JavaCompilationUnit, JavaConstructorBody, JavaDimension, JavaDoStatement, JavaElementValuePair,
    JavaEnhancedForStatement, JavaExpressionStatement, JavaFinallyClause, JavaForStatement,
    JavaGenericType, JavaIfStatement, JavaImportDeclaration, JavaKind, JavaLabeledStatement,
    JavaLanguage, JavaLocalTypeDeclaration, JavaModuleDeclaration, JavaModuleDirective, JavaName,
    JavaPackageDeclaration, JavaPattern, JavaPatternBody, JavaPatternModifiers, JavaPatternName,
    JavaPatternVariable, JavaProgram, JavaRecordPatternBody, JavaResource,
    JavaResourceSpecification, JavaScopedTypeName, JavaStatement, JavaSwitchBlockStatementGroup,
    JavaSwitchExpression, JavaSwitchExpressionItem, JavaSwitchExpressionOutcome,
    JavaSwitchExpressionRule, JavaSwitchGroupStatements, JavaSwitchLabel, JavaSwitchStatement,
    JavaSwitchStatementItem, JavaSwitchStatementOutcome, JavaSwitchStatementRule,
    JavaSynchronizedStatement, JavaTopLevelDeclaration, JavaTopLevelTypeDeclaration,
    JavaTryStatement, JavaTryWithResourcesStatement, JavaUnnamedCompilationUnit,
    JavaUnnamedCompilationUnitBody, JavaWhileStatement, JavaWildcard,
};

impl JavaAst {
    /// Lower one strict Java CST to the public JDK 26 compiler-tree model.
    ///
    /// # Errors
    ///
    /// Returns [`AstError::RecoveryTree`] for a tree containing an error node,
    /// or an invariant error when the CST does not match the generated schema.
    pub fn lower(tree: &Tree, source: &str) -> Result<Self, AstError> {
        Lowerer::new(source, None)?.lower(tree)
    }

    /// Lower a compact compilation unit using its source file name.
    ///
    /// # Errors
    ///
    /// Returns the same failures as [`Self::lower`].
    pub fn lower_with_file_name(
        tree: &Tree,
        source: &str,
        file_name: &str,
    ) -> Result<Self, AstError> {
        Lowerer::new(source, Some(file_name))?.lower(tree)
    }
}

struct Lowerer<'source> {
    input: JavaInput,
    file_name: Option<&'source str>,
    ast: AstBuilder,
}

impl<'source> Lowerer<'source> {
    fn new(source: &'source str, file_name: Option<&'source str>) -> Result<Self, AstError> {
        let input: Arc<dyn Input> =
            Arc::new(StringInput::try_new(source).map_err(|_| AstError::SourceTooLarge)?);
        Ok(Self {
            input: JavaInput::new(input),
            file_name,
            ast: AstBuilder::new(),
        })
    }

    fn lower(mut self, tree: &Tree) -> Result<JavaAst, AstError> {
        reject_recovery_tree(tree)?;
        let program =
            JavaProgram::downcast_from(tree.top_node()).map_err(|_| AstError::InconsistentCst {
                context: "Java source",
                expected: "Program",
            })?;
        let compilation_unit = program
            .compilation_unit()
            .ok_or(AstError::InconsistentCst {
                context: "Program",
                expected: "CompilationUnit",
            })?;
        let compilation_unit_syntax = compilation_unit.syntax();
        let root = self.ast.push_node(
            JavaAstKind::CompilationUnit,
            compilation_unit_syntax.range().into(),
        )?;
        self.lower_compilation_unit(root, &compilation_unit)?;
        let end = self
            .ast
            .child_range_end(root)
            .unwrap_or(compilation_unit_syntax.from());
        self.ast.set_range(
            root,
            JavaSourceRange::new(Some(compilation_unit_syntax.from()), Some(end)),
        )?;
        self.ast.finish(root)
    }

    fn lower_compilation_unit(
        &mut self,
        parent: AstNodeId,
        compilation_unit: &JavaCompilationUnit,
    ) -> Result<(), AstError> {
        if let Some(package) = compilation_unit.package() {
            let package = self.lower_package(&package)?;
            self.ast.push_edge(parent, JavaAstField::Package, package)?;
        }
        for import in compilation_unit.imports() {
            let import = self.lower_import(&import)?;
            self.ast.push_edge(parent, JavaAstField::Imports, import)?;
        }
        for declaration in compilation_unit.declarations() {
            if let Some(declaration) = self.lower_top_level_declaration(&declaration)? {
                self.ast
                    .push_edge(parent, JavaAstField::TypeDecls, declaration)?;
            }
        }
        if let Some(unnamed) = compilation_unit.unnamed() {
            self.lower_unnamed_unit(parent, &unnamed)?;
        }
        Ok(())
    }

    fn lower_package(
        &mut self,
        declaration: &JavaPackageDeclaration,
    ) -> Result<AstNodeId, AstError> {
        let syntax = declaration.syntax();
        let package = self
            .ast
            .push_node(JavaAstKind::Package, syntax.range().into())?;
        for annotation in declaration.annotations() {
            let annotation = self.lower_annotation(annotation.syntax(), false)?;
            self.ast
                .push_edge(package, JavaAstField::Annotations, annotation)?;
        }
        let name = declaration.name().ok_or(AstError::InconsistentCst {
            context: "PackageDeclaration",
            expected: "package name",
        })?;
        let name = self.lower_qualified_name(name.syntax())?;
        self.ast
            .push_edge(package, JavaAstField::PackageName, name)?;
        Ok(package)
    }

    fn lower_import(&mut self, declaration: &JavaImportDeclaration) -> Result<AstNodeId, AstError> {
        let declaration = declaration.declaration().ok_or(AstError::InconsistentCst {
            context: "ImportDeclaration",
            expected: "module or type import",
        })?;
        let (body, name, is_module, is_static, wildcard) = match declaration {
            crate::typed::JavaImportKind::Module(declaration) => (
                declaration.syntax().clone(),
                declaration.name(),
                true,
                false,
                None,
            ),
            crate::typed::JavaImportKind::TypeOrStatic(declaration) => (
                declaration.syntax().clone(),
                declaration.name(),
                false,
                declaration.static_token().is_some(),
                declaration.asterisk_token(),
            ),
        };
        let name = name.ok_or(AstError::InconsistentCst {
            context: "import declaration",
            expected: "qualified identifier",
        })?;
        let name = name.syntax();
        let import = self
            .ast
            .push_node(JavaAstKind::Import, body.range().into())?;
        self.ast
            .push_property(import, JavaAstProperty::ImportModule(is_module))?;
        self.ast
            .push_property(import, JavaAstProperty::ImportStatic(is_static))?;
        let qualified = if let Some(asterisk) = wildcard {
            let select = self.ast.push_node(
                JavaAstKind::MemberSelect,
                TextRange::new(name.from(), asterisk.to()).into(),
            )?;
            self.ast.push_name(select, "*", Some(asterisk.range()))?;
            let qualifier = self.lower_qualified_name(name)?;
            self.ast
                .push_edge(select, JavaAstField::Expression, qualifier)?;
            select
        } else {
            self.lower_qualified_name(name)?
        };
        self.ast
            .push_edge(import, JavaAstField::QualifiedIdentifier, qualified)?;
        Ok(import)
    }

    fn lower_unnamed_unit(
        &mut self,
        parent: AstNodeId,
        unit: &JavaUnnamedCompilationUnit,
    ) -> Result<(), AstError> {
        let body = unit.body().ok_or(AstError::InconsistentCst {
            context: "UnnamedCompilationUnit",
            expected: "module declaration or unit body",
        })?;
        match body {
            JavaUnnamedCompilationUnitBody::Module(module) => {
                let module = self.lower_module(module.syntax())?;
                self.ast.push_edge(parent, JavaAstField::Module, module)?;
            }
            JavaUnnamedCompilationUnitBody::Compact(body) => {
                let compact = is_compact_unit(&body).ok_or(AstError::InconsistentCst {
                    context: "UnitBeforeFirstMethod",
                    expected: "top-level or compact declaration sequence",
                })?;
                if compact {
                    let declaration = self.lower_compact_unit(&body)?;
                    self.ast
                        .push_edge(parent, JavaAstField::TypeDecls, declaration)?;
                } else {
                    self.lower_unit_declarations(parent, &body)?;
                }
            }
        }
        Ok(())
    }

    fn lower_unit_declarations(
        &mut self,
        parent: AstNodeId,
        unit: &crate::typed::JavaUnitBeforeFirstMethod,
    ) -> Result<(), AstError> {
        let complete = try_for_each_unit_member(unit, |member| match member {
            UnitMember::TopLevel(wrapper) => {
                if let Some(declaration) = self.lower_top_level_declaration(&wrapper)? {
                    self.ast
                        .push_edge(parent, JavaAstField::TypeDecls, declaration)?;
                }
                Ok(())
            }
            UnitMember::Compact(_) => Err(AstError::InconsistentCst {
                context: "UnitBeforeFirstMethod",
                expected: "top-level declaration sequence",
            }),
        })?;
        if !complete {
            return Err(AstError::InconsistentCst {
                context: "UnitBeforeFirstMethod",
                expected: "top-level declaration sequence",
            });
        }
        Ok(())
    }

    fn lower_compact_unit(
        &mut self,
        unit: &crate::typed::JavaUnitBeforeFirstMethod,
    ) -> Result<AstNodeId, AstError> {
        let file_name = self.file_name.ok_or(AstError::InconsistentCst {
            context: "compact compilation unit",
            expected: "source file name",
        })?;
        let name = file_name.strip_suffix(".java").unwrap_or(file_name);
        let declaration = self
            .ast
            .push_node(JavaAstKind::Class, unit.syntax().range().into())?;
        self.ast.push_name(declaration, name, None)?;
        self.ast
            .push_property(declaration, JavaAstProperty::Modifier(JavaModifier::Final))?;
        let modifiers = self
            .ast
            .push_node(JavaAstKind::Modifiers, JavaSourceRange::default())?;
        self.ast
            .push_property(modifiers, JavaAstProperty::Modifier(JavaModifier::Final))?;
        self.ast
            .push_edge(declaration, JavaAstField::Modifiers, modifiers)?;
        let complete = try_for_each_unit_member(unit, |member| match member {
            UnitMember::TopLevel(wrapper) => {
                if let Some(nested) = self.lower_top_level_declaration(&wrapper)? {
                    self.ast
                        .push_edge(declaration, JavaAstField::Members, nested)?;
                }
                Ok(())
            }
            UnitMember::Compact(member) => self.lower_compact_member(declaration, &member),
        })?;
        if !complete {
            return Err(AstError::InconsistentCst {
                context: "compact compilation unit",
                expected: "field, method, or nested type sequence",
            });
        }
        Ok(declaration)
    }

    fn lower_compact_member(
        &mut self,
        parent: AstNodeId,
        member: &CompactMember,
    ) -> Result<(), AstError> {
        match member.kind {
            CompactMemberKind::Field => {
                self.lower_field_declaration(parent, &member.syntax)?;
            }
            CompactMemberKind::Method => {
                let method = self.lower_method(&member.syntax, CallableForm::Method)?;
                self.ast.push_edge(parent, JavaAstField::Members, method)?;
            }
            CompactMemberKind::NestedType => {
                let nested = self.lower_type_declaration(&member.syntax)?;
                self.ast.push_edge(parent, JavaAstField::Members, nested)?;
            }
        }
        Ok(())
    }

    fn lower_top_level_declaration(
        &mut self,
        wrapper: &JavaTopLevelDeclaration,
    ) -> Result<Option<AstNodeId>, AstError> {
        if let Some(declaration) = wrapper.declaration() {
            return self
                .lower_top_level_type_declaration(&declaration)
                .map(Some);
        }
        if let Some(semicolon) = wrapper.semicolon_token() {
            return self
                .ast
                .push_node(JavaAstKind::EmptyStatement, semicolon.range().into())
                .map(Some);
        }
        Ok(None)
    }

    fn lower_top_level_type_declaration(
        &mut self,
        declaration: &JavaTopLevelTypeDeclaration,
    ) -> Result<AstNodeId, AstError> {
        self.lower_type_declaration(declaration.syntax())
    }

    fn lower_type_declaration(&mut self, declaration: &SyntaxNode) -> Result<AstNodeId, AstError> {
        let shape = type_declaration_shape(declaration).ok_or(AstError::InconsistentCst {
            context: "type declaration",
            expected: "class, record, interface, annotation, or enum",
        })?;
        self.lower_type_declaration_parts(&shape)
    }

    fn lower_type_declaration_parts(
        &mut self,
        declaration: &TypeDeclarationShape,
    ) -> Result<AstNodeId, AstError> {
        let context = match declaration.kind {
            TypeDeclarationKind::Class => TypeContext::Class,
            TypeDeclarationKind::Record => TypeContext::Record,
            TypeDeclarationKind::Interface => TypeContext::Interface,
            TypeDeclarationKind::Annotation => TypeContext::Annotation,
            TypeDeclarationKind::Enum => TypeContext::Enum,
        };
        let ty = self
            .ast
            .push_node(context.ast_kind(), declaration.syntax.range().into())?;
        let modifiers = declaration.modifiers.as_ref();
        let modifier_values = Self::modifiers(modifiers, &[]);
        for modifier in &modifier_values {
            self.ast
                .push_property(ty, JavaAstProperty::Modifier(*modifier))?;
        }
        let cooked = self.cooked_name(&declaration.name)?;
        self.ast
            .push_name(ty, &cooked, Some(declaration.name.range()))?;

        let modifiers_range = if context == TypeContext::Annotation {
            let start = modifiers.map_or(declaration.syntax.from(), SyntaxNode::from);
            JavaSourceRange::new(
                Some(start),
                Some(declaration.core_start + TextSize::from(1)),
            )
        } else {
            modifiers.map_or_else(JavaSourceRange::default, |node| node.range().into())
        };
        let modifier_node = self
            .ast
            .push_node(JavaAstKind::Modifiers, modifiers_range)?;
        for modifier in modifier_values {
            self.ast
                .push_property(modifier_node, JavaAstProperty::Modifier(modifier))?;
        }
        if let Some(modifiers) = modifiers {
            self.lower_modifier_annotations(modifier_node, modifiers)?;
        }
        self.ast
            .push_edge(ty, JavaAstField::Modifiers, modifier_node)?;

        self.lower_type_header(ty, declaration)?;
        self.lower_type_body(ty, declaration, &cooked)?;
        Ok(ty)
    }

    fn lower_type_header(
        &mut self,
        parent: AstNodeId,
        declaration: &TypeDeclarationShape,
    ) -> Result<(), AstError> {
        if let Some(parameters) = &declaration.type_parameters {
            for parameter in parameters.parameters() {
                let parameter = self.lower_type_parameter(parameter.syntax())?;
                self.ast
                    .push_edge(parent, JavaAstField::TypeParameters, parameter)?;
            }
        }
        if let Some(superclass) = &declaration.superclass {
            let ty = self.lower_type(superclass.syntax())?;
            self.ast
                .push_edge(parent, JavaAstField::ExtendsClause, ty)?;
        }
        if let Some(interfaces) = &declaration.interfaces {
            for interface in interfaces.types() {
                let interface = self.lower_type(interface.syntax())?;
                self.ast
                    .push_edge(parent, JavaAstField::ImplementsClause, interface)?;
            }
        }
        if let Some(permits) = &declaration.permits {
            for permit in permits.types() {
                let permit = self.lower_type(permit.syntax())?;
                self.ast
                    .push_edge(parent, JavaAstField::PermitsClause, permit)?;
            }
        }
        Ok(())
    }

    fn lower_type_body(
        &mut self,
        parent: AstNodeId,
        declaration: &TypeDeclarationShape,
        type_name: &str,
    ) -> Result<(), AstError> {
        let record_components =
            declaration
                .record_header
                .as_ref()
                .map_or_else(Vec::new, |header| {
                    header
                        .components()
                        .map(TypedNode::into_syntax)
                        .collect::<Vec<_>>()
                });
        for component in &record_components {
            let component = self.lower_record_component(component)?;
            self.ast
                .push_edge(parent, JavaAstField::Members, component)?;
        }
        try_for_each_type_member(&declaration.body, |member| {
            self.lower_member(parent, &member, type_name, &record_components)
        })
    }

    fn lower_member(
        &mut self,
        parent: AstNodeId,
        member: &TypeMember,
        type_name: &str,
        record_components: &[SyntaxNode],
    ) -> Result<(), AstError> {
        match &member.kind {
            TypeMemberKind::Field | TypeMemberKind::Constant => {
                self.lower_field_declaration(parent, &member.syntax)?;
            }
            TypeMemberKind::Method => {
                let method = self.lower_method(&member.syntax, CallableForm::Method)?;
                self.ast.push_edge(parent, JavaAstField::Members, method)?;
            }
            TypeMemberKind::Constructor => {
                let method = self.lower_method(&member.syntax, CallableForm::Constructor)?;
                self.ast.push_edge(parent, JavaAstField::Members, method)?;
            }
            TypeMemberKind::CompactConstructor => {
                let method = self.lower_method(
                    &member.syntax,
                    CallableForm::CompactConstructor(record_components),
                )?;
                self.ast.push_edge(parent, JavaAstField::Members, method)?;
            }
            TypeMemberKind::AnnotationElement => {
                let method = self.lower_annotation_element(&member.syntax)?;
                self.ast.push_edge(parent, JavaAstField::Members, method)?;
            }
            TypeMemberKind::EnumConstant => {
                let constant = self.lower_enum_constant(&member.syntax, type_name)?;
                self.ast
                    .push_edge(parent, JavaAstField::Members, constant)?;
            }
            TypeMemberKind::NestedType => {
                let declaration = self.lower_type_declaration(&member.syntax)?;
                self.ast
                    .push_edge(parent, JavaAstField::Members, declaration)?;
            }
            TypeMemberKind::Initializer => {
                let block = self.lower_block(&member.syntax, false)?;
                self.ast.push_edge(parent, JavaAstField::Members, block)?;
            }
            TypeMemberKind::StaticInitializer { block } => {
                let block = block.as_ref().ok_or(AstError::InconsistentCst {
                    context: "StaticInitializer",
                    expected: "block",
                })?;
                let block = self.lower_block_with_range(
                    block.syntax(),
                    true,
                    member.syntax.range().into(),
                )?;
                self.ast.push_edge(parent, JavaAstField::Members, block)?;
            }
        }
        Ok(())
    }

    // The remaining declaration, executable, type, expression, and module
    // lowering routines are defined below. Each routine consumes only direct
    // typed CST children, so every CST edge is visited a bounded number of
    // times and no source-backed structural search is needed.

    fn lower_field_declaration(
        &mut self,
        parent: AstNodeId,
        declaration: &SyntaxNode,
    ) -> Result<(), AstError> {
        let (base_type, modifiers_node) = match kind(declaration) {
            Some(JavaKind::FieldDeclaration) => {
                let shape = typed_node::<crate::typed::JavaFieldDeclaration>(
                    declaration,
                    "FieldDeclaration",
                    "typed field declaration",
                )?;
                let ty = shape.type_().ok_or(AstError::InconsistentCst {
                    context: "field declaration",
                    expected: "field type",
                })?;
                (
                    ty.syntax().clone(),
                    shape.modifiers().map(TypedNode::into_syntax),
                )
            }
            Some(JavaKind::ConstantDeclaration) => {
                let shape = typed_node::<crate::typed::JavaConstantDeclaration>(
                    declaration,
                    "ConstantDeclaration",
                    "typed constant declaration",
                )?;
                let ty = shape.type_().ok_or(AstError::InconsistentCst {
                    context: "constant declaration",
                    expected: "constant type",
                })?;
                (
                    ty.syntax().clone(),
                    shape.modifiers().map(TypedNode::into_syntax),
                )
            }
            _ => {
                return Err(AstError::InconsistentCst {
                    context: "field declaration",
                    expected: "field or constant declaration",
                });
            }
        };
        let modifiers = Self::modifiers(modifiers_node.as_ref(), &[]);
        for entry in declarator_entries(declaration, declaration.to()) {
            let variable = self.lower_variable(
                entry.declarator.syntax(),
                Some(&base_type),
                entry.range,
                modifiers_node.as_ref(),
                &modifiers,
            )?;
            self.ast
                .push_edge(parent, JavaAstField::Members, variable)?;
        }
        Ok(())
    }

    fn lower_method(
        &mut self,
        declaration: &SyntaxNode,
        form: CallableForm<'_>,
    ) -> Result<AstNodeId, AstError> {
        let shape = callable_shape(declaration).ok_or(AstError::InconsistentCst {
            context: "callable declaration",
            expected: "typed method or constructor declaration",
        })?;
        let definition = &shape.name;
        let name = if matches!(form, CallableForm::Method) {
            self.cooked_name(definition.syntax())?
        } else {
            "<init>".to_owned()
        };
        let method = self
            .ast
            .push_node(JavaAstKind::Method, shape.syntax.range().into())?;
        self.ast
            .push_name(method, &name, Some(definition.syntax().range()))?;
        self.lower_callable_modifiers(method, &shape)?;
        self.lower_callable_return_type(method, &shape, form)?;
        self.lower_callable_type_parameters(method, &shape)?;
        self.lower_callable_parameters(method, &shape, form)?;
        self.lower_callable_throws(method, &shape)?;
        self.lower_callable_body(method, &shape)?;
        Ok(method)
    }

    fn lower_callable_modifiers(
        &mut self,
        method: AstNodeId,
        declaration: &CallableShape,
    ) -> Result<(), AstError> {
        let modifiers_node = declaration.modifiers.as_ref();
        let modifiers = Self::modifiers(modifiers_node.map(TypedNode::syntax), &[]);
        for modifier in &modifiers {
            self.ast
                .push_property(method, JavaAstProperty::Modifier(*modifier))?;
        }
        let modifier_range = modifiers_node.as_ref().map_or_else(
            || {
                declaration.type_parameters.as_ref().map_or_else(
                    JavaSourceRange::default,
                    |parameters| {
                        JavaSourceRange::new(
                            Some(parameters.syntax().from()),
                            Some(parameters.syntax().from()),
                        )
                    },
                )
            },
            |node| node.syntax().range().into(),
        );
        let modifier_ast = self.ast.push_node(JavaAstKind::Modifiers, modifier_range)?;
        for modifier in modifiers {
            self.ast
                .push_property(modifier_ast, JavaAstProperty::Modifier(modifier))?;
        }
        if let Some(modifiers) = modifiers_node.as_ref() {
            self.lower_modifier_annotations(modifier_ast, modifiers.syntax())?;
        }
        self.ast
            .push_edge(method, JavaAstField::Modifiers, modifier_ast)?;
        Ok(())
    }

    fn lower_callable_return_type(
        &mut self,
        method: AstNodeId,
        declaration: &CallableShape,
        form: CallableForm<'_>,
    ) -> Result<(), AstError> {
        if !matches!(form, CallableForm::Method) {
            return Ok(());
        }
        let return_type = declaration
            .return_type
            .as_ref()
            .ok_or(AstError::InconsistentCst {
                context: "method declaration",
                expected: "return type",
            })?;
        let dimensions = declaration
            .dimensions
            .iter()
            .map(TypedNode::syntax)
            .cloned()
            .collect::<Vec<_>>();
        let return_syntax = return_type;
        let return_type = self.lower_type_with_dimensions(
            return_syntax,
            &dimensions,
            TextRange::new(
                return_syntax.from(),
                dimensions.last().map_or(return_syntax.to(), SyntaxNode::to),
            ),
        )?;
        self.ast
            .push_edge(method, JavaAstField::ReturnType, return_type)
    }

    fn lower_callable_type_parameters(
        &mut self,
        method: AstNodeId,
        declaration: &CallableShape,
    ) -> Result<(), AstError> {
        if let Some(type_parameters) = declaration.type_parameters.as_ref() {
            for parameter in type_parameters.parameters() {
                let parameter = self.lower_type_parameter(parameter.syntax())?;
                self.ast
                    .push_edge(method, JavaAstField::TypeParameters, parameter)?;
            }
        }
        Ok(())
    }

    fn lower_callable_parameters(
        &mut self,
        method: AstNodeId,
        declaration: &CallableShape,
        form: CallableForm<'_>,
    ) -> Result<(), AstError> {
        match form {
            CallableForm::Method | CallableForm::Constructor => {
                let parameters =
                    declaration
                        .parameters
                        .as_ref()
                        .ok_or(AstError::InconsistentCst {
                            context: "callable declaration",
                            expected: "formal parameters",
                        })?;
                for parameter in parameters.parameters() {
                    let field = match &parameter {
                        crate::typed::JavaParameter::Formal(_)
                        | crate::typed::JavaParameter::Spread(_) => JavaAstField::Parameters,
                        crate::typed::JavaParameter::Receiver(_) => JavaAstField::ReceiverParameter,
                    };
                    let parameter = self.lower_parameter(parameter.syntax())?;
                    self.ast.push_edge(method, field, parameter)?;
                }
            }
            CallableForm::CompactConstructor(components) => {
                for component in components {
                    let parameter = self.lower_compact_constructor_parameter(component)?;
                    self.ast
                        .push_edge(method, JavaAstField::Parameters, parameter)?;
                }
            }
        }
        Ok(())
    }

    fn lower_callable_throws(
        &mut self,
        method: AstNodeId,
        declaration: &CallableShape,
    ) -> Result<(), AstError> {
        if let Some(throws) = declaration.throws.as_ref() {
            for exception in throws.types() {
                let exception = self.lower_type(exception.syntax())?;
                self.ast
                    .push_edge(method, JavaAstField::Throws, exception)?;
            }
        }
        Ok(())
    }

    fn lower_callable_body(
        &mut self,
        method: AstNodeId,
        declaration: &CallableShape,
    ) -> Result<(), AstError> {
        if let Some(body) = declaration.body.as_ref() {
            let body = match body {
                CallableBody::Block(body) => self.lower_block(body.syntax(), false)?,
                CallableBody::Constructor(body) => self.lower_constructor_body(body.syntax())?,
            };
            self.ast.push_edge(method, JavaAstField::Body, body)?;
        }
        Ok(())
    }

    fn lower_annotation_element(
        &mut self,
        declaration: &SyntaxNode,
    ) -> Result<AstNodeId, AstError> {
        let shape = typed_node::<crate::typed::JavaAnnotationTypeElementDeclaration>(
            declaration,
            "AnnotationTypeElementDeclaration",
            "typed annotation element",
        )?;
        let identifier = shape.name().ok_or(AstError::InconsistentCst {
            context: "annotation element",
            expected: "name",
        })?;
        let method = self
            .ast
            .push_node(JavaAstKind::Method, declaration.range().into())?;
        let name = self.cooked_name(identifier.syntax())?;
        self.ast
            .push_name(method, &name, Some(identifier.syntax().range()))?;
        let modifiers_node = shape.modifiers();
        let modifiers = Self::modifiers(modifiers_node.as_ref().map(TypedNode::syntax), &[]);
        for modifier in &modifiers {
            self.ast
                .push_property(method, JavaAstProperty::Modifier(*modifier))?;
        }
        let modifier_ast = self.ast.push_node(
            JavaAstKind::Modifiers,
            modifiers_node
                .as_ref()
                .map_or_else(JavaSourceRange::default, |node| {
                    node.syntax().range().into()
                }),
        )?;
        for modifier in modifiers {
            self.ast
                .push_property(modifier_ast, JavaAstProperty::Modifier(modifier))?;
        }
        if let Some(modifiers) = modifiers_node.as_ref() {
            self.lower_modifier_annotations(modifier_ast, modifiers.syntax())?;
        }
        self.ast
            .push_edge(method, JavaAstField::Modifiers, modifier_ast)?;
        let base = shape.type_().ok_or(AstError::InconsistentCst {
            context: "annotation element",
            expected: "return type",
        })?;
        let dimensions = shape
            .dimensions()
            .map(TypedNode::into_syntax)
            .collect::<Vec<_>>();
        let base_syntax = base.syntax();
        let return_type = self.lower_type_with_dimensions(
            base_syntax,
            &dimensions,
            TextRange::new(
                base_syntax.from(),
                dimensions.last().map_or(base_syntax.to(), SyntaxNode::to),
            ),
        )?;
        self.ast
            .push_edge(method, JavaAstField::ReturnType, return_type)?;
        if let Some(default_value) = shape.default()
            && let Some(value) = default_value.value()
        {
            let value = self.lower_annotation_value(&value)?;
            self.ast
                .push_edge(method, JavaAstField::DefaultValue, value)?;
        }
        Ok(method)
    }

    fn lower_enum_constant(
        &mut self,
        node: &SyntaxNode,
        enum_name: &str,
    ) -> Result<AstNodeId, AstError> {
        let shape = typed_node::<crate::typed::JavaEnumConstant>(
            node,
            "EnumConstant",
            "typed enum constant",
        )?;
        let definition = shape.name().ok_or(AstError::InconsistentCst {
            context: "EnumConstant",
            expected: "name",
        })?;
        let definition_syntax = definition.syntax();
        let name = self.cooked_name(definition_syntax)?;
        let variable = self
            .ast
            .push_node(JavaAstKind::Variable, node.range().into())?;
        self.ast
            .push_name(variable, &name, Some(definition_syntax.range()))?;
        let modifiers_node = shape.modifiers();
        let modifiers = Self::modifiers(
            modifiers_node.as_ref().map(TypedNode::syntax),
            &[
                JavaModifier::Final,
                JavaModifier::Public,
                JavaModifier::Static,
            ],
        );
        for modifier in &modifiers {
            self.ast
                .push_property(variable, JavaAstProperty::Modifier(*modifier))?;
        }
        let modifiers_ast = self.ast.push_node(
            JavaAstKind::Modifiers,
            modifiers_node
                .as_ref()
                .map_or_else(JavaSourceRange::default, |node| {
                    node.syntax().range().into()
                }),
        )?;
        for modifier in modifiers {
            self.ast
                .push_property(modifiers_ast, JavaAstProperty::Modifier(modifier))?;
        }
        if let Some(modifiers) = modifiers_node.as_ref() {
            self.lower_modifier_annotations(modifiers_ast, modifiers.syntax())?;
        }
        self.ast
            .push_edge(variable, JavaAstField::Modifiers, modifiers_ast)?;
        let enum_type = self.push_synthetic_identifier(
            enum_name,
            Some(definition_syntax.from()),
            None,
            Some(definition_syntax.range()),
        )?;
        self.ast
            .push_edge(variable, JavaAstField::Type, enum_type)?;

        let arguments = shape.arguments();
        let class_body = shape.class_body();
        let has_arguments = arguments
            .as_ref()
            .is_some_and(|node| node.arguments().next().is_some());
        let has_initializer_extent = has_arguments || class_body.is_some();
        let initializer_start = arguments
            .as_ref()
            .filter(|_| has_initializer_extent)
            .map(|node| node.syntax().from())
            .or_else(|| class_body.as_ref().map(|node| node.syntax().from()))
            .unwrap_or_else(|| definition_syntax.from());
        let initializer = self.ast.push_node(
            JavaAstKind::NewClass,
            JavaSourceRange::new(
                Some(initializer_start),
                has_initializer_extent.then_some(node.to()),
            ),
        )?;
        let enum_type = self.push_synthetic_identifier(
            enum_name,
            Some(definition_syntax.from()),
            None,
            Some(definition_syntax.range()),
        )?;
        self.ast
            .push_edge(initializer, JavaAstField::Identifier, enum_type)?;
        if let Some(arguments) = arguments {
            for argument in arguments.arguments() {
                let argument = self.lower_expression(argument.syntax())?;
                self.ast
                    .push_edge(initializer, JavaAstField::Arguments, argument)?;
            }
        }
        if let Some(class_body) = class_body {
            self.lower_enum_constant_class_body(
                initializer,
                &class_body,
                definition_syntax,
                enum_name,
            )?;
        }
        self.ast
            .push_edge(variable, JavaAstField::Initializer, initializer)?;
        Ok(variable)
    }

    fn lower_enum_constant_class_body(
        &mut self,
        initializer: AstNodeId,
        class_body: &crate::typed::JavaClassBody,
        definition: &SyntaxNode,
        enum_name: &str,
    ) -> Result<(), AstError> {
        let body = self.ast.push_node(
            JavaAstKind::Enum,
            TextRange::new(definition.from(), class_body.syntax().to()).into(),
        )?;
        self.ast.push_name(body, "", None)?;
        let modifiers = self
            .ast
            .push_node(JavaAstKind::Modifiers, JavaSourceRange::default())?;
        self.ast
            .push_edge(body, JavaAstField::Modifiers, modifiers)?;
        try_for_each_class_member(class_body, |member| {
            self.lower_member(body, &member, enum_name, &[])
        })?;
        self.ast
            .push_edge(initializer, JavaAstField::ClassBody, body)
    }

    fn lower_variable(
        &mut self,
        node: &SyntaxNode,
        base_type: Option<&SyntaxNode>,
        source_range: TextRange,
        modifiers_node: Option<&SyntaxNode>,
        modifiers: &[JavaModifier],
    ) -> Result<AstNodeId, AstError> {
        let shape = typed_node::<crate::typed::JavaVariableDeclarator>(
            node,
            "VariableDeclarator",
            "typed variable declarator",
        )?;
        let definition = shape.name().ok_or(AstError::InconsistentCst {
            context: "VariableDeclarator",
            expected: "name",
        })?;
        let variable = self
            .ast
            .push_node(JavaAstKind::Variable, source_range.into())?;
        let name = self.cooked_name(definition.syntax())?;
        self.ast
            .push_name(variable, &name, Some(definition.syntax().range()))?;
        for modifier in modifiers {
            self.ast
                .push_property(variable, JavaAstProperty::Modifier(*modifier))?;
        }
        let modifiers_ast = self.ast.push_node(
            JavaAstKind::Modifiers,
            modifiers_node.map_or_else(JavaSourceRange::default, |node| node.range().into()),
        )?;
        for modifier in modifiers {
            self.ast
                .push_property(modifiers_ast, JavaAstProperty::Modifier(*modifier))?;
        }
        if let Some(modifiers) = modifiers_node {
            self.lower_modifier_annotations(modifiers_ast, modifiers)?;
        }
        self.ast
            .push_edge(variable, JavaAstField::Modifiers, modifiers_ast)?;
        if let Some(base_type) = base_type
            && kind(base_type) != Some(JavaKind::InferredLocalType)
        {
            let dimensions = shape
                .dimensions()
                .map(TypedNode::into_syntax)
                .collect::<Vec<_>>();
            let end = dimensions.last().map_or(base_type.to(), SyntaxNode::to);
            let ty = self.lower_type_with_dimensions(
                base_type,
                &dimensions,
                TextRange::new(base_type.from(), end),
            )?;
            self.ast.push_edge(variable, JavaAstField::Type, ty)?;
        }
        if let Some(initializer) = shape.initializer() {
            let initializer = self.lower_expression(initializer.syntax())?;
            self.ast
                .push_edge(variable, JavaAstField::Initializer, initializer)?;
        }
        Ok(variable)
    }

    fn lower_parameter(&mut self, node: &SyntaxNode) -> Result<AstNodeId, AstError> {
        let shape = parameter_shape(node).ok_or(AstError::InconsistentCst {
            context: "formal parameter",
            expected: "typed parameter shape",
        })?;
        let variable = self
            .ast
            .push_node(JavaAstKind::Variable, shape.syntax.range().into())?;
        let name = if shape.kind == ParameterKind::Receiver {
            "this".to_owned()
        } else {
            self.cooked_name(&shape.name)?
        };
        self.ast
            .push_name(variable, &name, Some(shape.name.range()))?;
        self.lower_parameter_modifiers(variable, &shape)?;
        if shape.kind == ParameterKind::Inferred {
            return Ok(variable);
        }
        self.lower_parameter_type(variable, &shape)?;
        if shape.kind == ParameterKind::Receiver {
            self.lower_receiver_name_expression(variable, &shape)?;
        }
        Ok(variable)
    }

    fn lower_parameter_modifiers(
        &mut self,
        variable: AstNodeId,
        shape: &ParameterShape,
    ) -> Result<(), AstError> {
        let modifiers_node = shape.modifiers.as_ref().map(TypedNode::syntax);
        let modifiers = Self::modifiers(modifiers_node, &[]);
        for modifier in &modifiers {
            self.ast
                .push_property(variable, JavaAstProperty::Modifier(*modifier))?;
        }
        let modifier_range = modifiers_node.map_or_else(
            || {
                shape
                    .leading_annotations
                    .first()
                    .zip(shape.leading_annotations.last())
                    .map_or_else(JavaSourceRange::default, |(first, last)| {
                        TextRange::new(first.syntax().from(), last.syntax().to()).into()
                    })
            },
            |node| node.range().into(),
        );
        let modifiers_ast = self.ast.push_node(JavaAstKind::Modifiers, modifier_range)?;
        for modifier in modifiers {
            self.ast
                .push_property(modifiers_ast, JavaAstProperty::Modifier(modifier))?;
        }
        if let Some(modifiers) = modifiers_node {
            self.lower_modifier_annotations(modifiers_ast, modifiers)?;
        } else {
            for annotation in &shape.leading_annotations {
                let annotation = self.lower_annotation(annotation.syntax(), false)?;
                self.ast
                    .push_edge(modifiers_ast, JavaAstField::Annotations, annotation)?;
            }
        }
        self.ast
            .push_edge(variable, JavaAstField::Modifiers, modifiers_ast)?;
        Ok(())
    }

    fn lower_parameter_type(
        &mut self,
        variable: AstNodeId,
        shape: &ParameterShape,
    ) -> Result<(), AstError> {
        let base = shape.type_.as_ref().ok_or(AstError::InconsistentCst {
            context: "formal parameter",
            expected: "parameter type",
        })?;
        let base = base.syntax();
        let ty = if shape.kind == ParameterKind::Spread {
            self.lower_varargs_type(&shape.syntax, base)?
        } else {
            let dimensions = shape
                .dimensions
                .iter()
                .map(TypedNode::syntax)
                .cloned()
                .collect::<Vec<_>>();
            let end = dimensions.last().map_or(base.to(), SyntaxNode::to);
            self.lower_type_with_dimensions(base, &dimensions, TextRange::new(base.from(), end))?
        };
        self.ast.push_edge(variable, JavaAstField::Type, ty)?;
        Ok(())
    }

    fn lower_receiver_name_expression(
        &mut self,
        variable: AstNodeId,
        shape: &ParameterShape,
    ) -> Result<(), AstError> {
        let name_expression = if let Some(qualifier) = &shape.qualifier {
            let qualifier_ast = self.lower_expression(qualifier)?;
            let select = self.ast.push_node(
                JavaAstKind::MemberSelect,
                TextRange::new(qualifier.from(), shape.name.to()).into(),
            )?;
            self.ast
                .push_name(select, "this", Some(shape.name.range()))?;
            self.ast
                .push_edge(select, JavaAstField::Expression, qualifier_ast)?;
            select
        } else {
            self.push_synthetic_identifier(
                "this",
                Some(shape.name.from()),
                Some(shape.name.to()),
                Some(shape.name.range()),
            )?
        };
        self.ast
            .push_edge(variable, JavaAstField::NameExpression, name_expression)
    }

    fn lower_compact_constructor_parameter(
        &mut self,
        component: &SyntaxNode,
    ) -> Result<AstNodeId, AstError> {
        let shape = typed_node::<crate::typed::JavaRecordComponent>(
            component,
            "RecordComponent",
            "typed record component",
        )?;
        let definition = shape.name().ok_or(AstError::InconsistentCst {
            context: "record component",
            expected: "name",
        })?;
        let parameter = self.ast.push_node(
            JavaAstKind::Variable,
            JavaSourceRange::new(Some(component.from()), None),
        )?;
        let name = self.cooked_name(definition.syntax())?;
        self.ast
            .push_name(parameter, &name, Some(definition.syntax().range()))?;
        let modifiers = self
            .ast
            .push_node(JavaAstKind::Modifiers, JavaSourceRange::default())?;
        self.ast
            .push_edge(parameter, JavaAstField::Modifiers, modifiers)?;
        let base = shape.type_().ok_or(AstError::InconsistentCst {
            context: "record component",
            expected: "component type",
        })?;
        let ty = if shape.ellipsis_token().is_some() {
            self.lower_varargs_type(component, base.syntax())?
        } else {
            let dimensions = shape
                .dimensions()
                .map(TypedNode::into_syntax)
                .collect::<Vec<_>>();
            self.lower_type_with_dimensions(base.syntax(), &dimensions, component.range())?
        };
        self.ast.push_edge(parameter, JavaAstField::Type, ty)?;
        Ok(parameter)
    }

    fn lower_record_component(&mut self, node: &SyntaxNode) -> Result<AstNodeId, AstError> {
        let shape = typed_node::<crate::typed::JavaRecordComponent>(
            node,
            "RecordComponent",
            "typed record component",
        )?;
        let definition = shape.name().ok_or(AstError::InconsistentCst {
            context: "RecordComponent",
            expected: "name",
        })?;
        let name = self.cooked_name(definition.syntax())?;
        let variable = self
            .ast
            .push_node(JavaAstKind::Variable, node.range().into())?;
        self.ast
            .push_name(variable, &name, Some(definition.syntax().range()))?;
        let modifiers_node = shape.modifiers();
        let modifiers = Self::modifiers(
            modifiers_node.as_ref().map(TypedNode::syntax),
            &[JavaModifier::Final, JavaModifier::Private],
        );
        for modifier in &modifiers {
            self.ast
                .push_property(variable, JavaAstProperty::Modifier(*modifier))?;
        }
        let modifiers_ast = self.ast.push_node(
            JavaAstKind::Modifiers,
            modifiers_node
                .as_ref()
                .map_or_else(JavaSourceRange::default, |node| {
                    node.syntax().range().into()
                }),
        )?;
        for modifier in modifiers {
            self.ast
                .push_property(modifiers_ast, JavaAstProperty::Modifier(modifier))?;
        }
        if let Some(modifiers) = modifiers_node {
            self.lower_modifier_annotations(modifiers_ast, modifiers.syntax())?;
        }
        self.ast
            .push_edge(variable, JavaAstField::Modifiers, modifiers_ast)?;
        let base = shape.type_().ok_or(AstError::InconsistentCst {
            context: "RecordComponent",
            expected: "component type",
        })?;
        let ty = if shape.ellipsis_token().is_some() {
            self.lower_varargs_type(node, base.syntax())?
        } else {
            let dimensions = shape
                .dimensions()
                .map(TypedNode::into_syntax)
                .collect::<Vec<_>>();
            self.lower_type_with_dimensions(base.syntax(), &dimensions, node.range())?
        };
        self.ast.push_edge(variable, JavaAstField::Type, ty)?;
        Ok(variable)
    }

    fn lower_type_parameter(&mut self, node: &SyntaxNode) -> Result<AstNodeId, AstError> {
        let shape = typed_node::<crate::typed::JavaTypeParameter>(
            node,
            "TypeParameter",
            "typed type parameter",
        )?;
        let definition = shape.name().ok_or(AstError::InconsistentCst {
            context: "TypeParameter",
            expected: "name",
        })?;
        let parameter = self
            .ast
            .push_node(JavaAstKind::TypeParameter, node.range().into())?;
        let name = self.cooked_name(definition.syntax())?;
        self.ast
            .push_name(parameter, &name, Some(definition.syntax().range()))?;
        for annotation in shape.annotations() {
            let annotation = self.lower_annotation(annotation.syntax(), true)?;
            self.ast
                .push_edge(parameter, JavaAstField::Annotations, annotation)?;
        }
        if let Some(bounds) = shape.bound() {
            for bound in bounds.types() {
                let bound = self.lower_type(bound.syntax())?;
                self.ast.push_edge(parameter, JavaAstField::Bounds, bound)?;
            }
        }
        Ok(parameter)
    }

    fn lower_type(&mut self, node: &SyntaxNode) -> Result<AstNodeId, AstError> {
        match kind(node) {
            Some(JavaKind::AnnotatedType) => self.lower_annotated_type(node),
            Some(JavaKind::ArrayType) => self.lower_array_type(node),
            Some(JavaKind::GenericType) => self.lower_parameterized_type(node),
            Some(JavaKind::ScopedTypeName) => self.lower_scoped_type(node),
            Some(JavaKind::TypeName) => self.lower_type_name(node),
            Some(JavaKind::Wildcard) => self.lower_wildcard(node),
            Some(JavaKind::PrimitiveType | JavaKind::Void) => self.lower_primitive_type(node),
            Some(JavaKind::ExceptionType | JavaKind::CatchAlternativeType) => {
                self.lower_type_wrapper(node)
            }
            _ => Err(AstError::InconsistentCst {
                context: "Java type",
                expected: "typed Java type node",
            }),
        }
    }

    fn lower_created_type(&mut self, node: &SyntaxNode) -> Result<AstNodeId, AstError> {
        let (base, annotations) = match kind(node) {
            Some(JavaKind::CreatedClassType) => {
                let shape = typed_node::<crate::typed::JavaCreatedClassType>(
                    node,
                    "CreatedClassType",
                    "typed created class type",
                )?;
                let base = shape.type_().ok_or(AstError::InconsistentCst {
                    context: "CreatedClassType",
                    expected: "created Java type",
                })?;
                (
                    base.syntax().clone(),
                    shape.annotations().collect::<Vec<_>>(),
                )
            }
            Some(JavaKind::ArrayCreationType) => {
                let shape = typed_node::<crate::typed::JavaArrayCreationType>(
                    node,
                    "ArrayCreationType",
                    "typed array creation type",
                )?;
                let base = shape.type_().ok_or(AstError::InconsistentCst {
                    context: "ArrayCreationType",
                    expected: "created Java type",
                })?;
                (
                    base.syntax().clone(),
                    shape.annotations().collect::<Vec<_>>(),
                )
            }
            _ => {
                return Err(AstError::InconsistentCst {
                    context: "created type",
                    expected: "created class or array type",
                });
            }
        };
        if annotations.is_empty() {
            self.lower_type(&base)
        } else {
            self.lower_type_with_leading_annotations(&base, &annotations, node.from())
        }
    }

    fn lower_type_with_leading_annotations(
        &mut self,
        node: &SyntaxNode,
        annotations: &[JavaAnnotationNode],
        annotation_start: TextSize,
    ) -> Result<AstNodeId, AstError> {
        match kind(node) {
            Some(JavaKind::GenericType) => {
                let shape =
                    typed_node::<JavaGenericType>(node, "GenericType", "typed generic type")?;
                let parameterized = self.ast.push_node(
                    JavaAstKind::ParameterizedType,
                    TextRange::new(annotation_start, node.to()).into(),
                )?;
                let base = shape.type_().ok_or(AstError::InconsistentCst {
                    context: "GenericType",
                    expected: "base type",
                })?;
                let base = self.lower_type_with_leading_annotations(
                    base.syntax(),
                    annotations,
                    annotation_start,
                )?;
                self.ast
                    .push_edge(parameterized, JavaAstField::Type, base)?;
                let arguments = shape.arguments().ok_or(AstError::InconsistentCst {
                    context: "GenericType",
                    expected: "type arguments",
                })?;
                for argument in arguments.arguments() {
                    let argument = self.lower_type(argument.syntax())?;
                    self.ast
                        .push_edge(parameterized, JavaAstField::TypeArguments, argument)?;
                }
                Ok(parameterized)
            }
            Some(JavaKind::ScopedTypeName) => {
                let shape =
                    typed_node::<JavaScopedTypeName>(node, "ScopedTypeName", "typed scoped type")?;
                let qualifier = shape.qualifier().ok_or(AstError::InconsistentCst {
                    context: "ScopedTypeName",
                    expected: "qualifier type",
                })?;
                let segment = shape.name().ok_or(AstError::InconsistentCst {
                    context: "ScopedTypeName",
                    expected: "selected type",
                })?;
                let selected = self.ast.push_node(
                    JavaAstKind::MemberSelect,
                    TextRange::new(annotation_start, node.to()).into(),
                )?;
                let name = self.cooked_name(segment.syntax())?;
                self.ast
                    .push_name(selected, &name, Some(segment.syntax().range()))?;
                let qualifier = self.lower_type_with_leading_annotations(
                    qualifier.syntax(),
                    annotations,
                    annotation_start,
                )?;
                self.ast
                    .push_edge(selected, JavaAstField::Expression, qualifier)?;
                Ok(selected)
            }
            _ => {
                let annotated = self.ast.push_node(
                    JavaAstKind::AnnotatedType,
                    TextRange::new(annotation_start, node.to()).into(),
                )?;
                for annotation in annotations {
                    let annotation = self.lower_annotation(annotation.syntax(), true)?;
                    self.ast
                        .push_edge(annotated, JavaAstField::Annotations, annotation)?;
                }
                let underlying = self.lower_type(node)?;
                self.ast
                    .push_edge(annotated, JavaAstField::UnderlyingType, underlying)?;
                Ok(annotated)
            }
        }
    }

    fn lower_block(&mut self, node: &SyntaxNode, is_static: bool) -> Result<AstNodeId, AstError> {
        self.lower_block_with_range(node, is_static, node.range().into())
    }

    fn lower_constructor_body(&mut self, node: &SyntaxNode) -> Result<AstNodeId, AstError> {
        let syntax =
            typed_node::<JavaConstructorBody>(node, "ConstructorBody", "typed constructor body")?;
        let block = self
            .ast
            .push_node(JavaAstKind::Block, node.range().into())?;
        self.ast
            .push_property(block, JavaAstProperty::BlockStatic(false))?;
        self.lower_statement_list(block, syntax.statements())?;
        Ok(block)
    }

    fn lower_block_with_range(
        &mut self,
        node: &SyntaxNode,
        is_static: bool,
        range: JavaSourceRange,
    ) -> Result<AstNodeId, AstError> {
        let syntax = typed_node::<JavaBlock>(node, "Block", "typed block")?;
        let block = self.ast.push_node(JavaAstKind::Block, range)?;
        self.ast
            .push_property(block, JavaAstProperty::BlockStatic(is_static))?;
        self.lower_statement_list(block, syntax.statements())?;
        Ok(block)
    }

    fn lower_statement_list(
        &mut self,
        block: AstNodeId,
        statements: impl IntoIterator<Item = JavaStatement>,
    ) -> Result<(), AstError> {
        for statement in statements {
            if let JavaStatement::LocalVariable(declaration) = &statement {
                for declaration in self.lower_local_variables(declaration.syntax())? {
                    self.ast
                        .push_edge(block, JavaAstField::Statements, declaration)?;
                }
                continue;
            }
            let statement = self.lower_statement(&statement)?;
            self.ast
                .push_edge(block, JavaAstField::Statements, statement)?;
        }
        Ok(())
    }

    fn lower_statement(&mut self, statement: &JavaStatement) -> Result<AstNodeId, AstError> {
        match statement {
            JavaStatement::Block(node) => self.lower_block(node.syntax(), false),
            JavaStatement::LocalType(node) => self.lower_local_type(node),
            JavaStatement::LocalVariable(_) => Err(AstError::InconsistentCst {
                context: "statement",
                expected: "local variable lowered by its containing statement list",
            }),
            JavaStatement::Expression(node) => self.lower_expression_statement(node),
            JavaStatement::ConstructorInvocation(node) => {
                self.lower_constructor_invocation(node.syntax())
            }
            JavaStatement::Labeled(node) => self.lower_labeled(node),
            JavaStatement::If(node) => self.lower_if(node),
            JavaStatement::While(node) => self.lower_while(node),
            JavaStatement::Do(node) => self.lower_do_while(node),
            JavaStatement::For(node) => self.lower_for(node),
            JavaStatement::EnhancedFor(node) => self.lower_enhanced_for(node),
            JavaStatement::Assert(node) => self.lower_assert(node),
            JavaStatement::Switch(node) => self.lower_switch_statement(node),
            JavaStatement::Break(node) => {
                self.lower_jump(node.syntax(), node.label(), JavaAstKind::Break)
            }
            JavaStatement::Continue(node) => {
                self.lower_jump(node.syntax(), node.label(), JavaAstKind::Continue)
            }
            JavaStatement::Empty(node) => self
                .ast
                .push_node(JavaAstKind::EmptyStatement, node.syntax().range().into()),
            JavaStatement::Return(node) => {
                let statement = self
                    .ast
                    .push_node(JavaAstKind::Return, node.syntax().range().into())?;
                if let Some(expression) = node.expression() {
                    let expression = self.lower_expression(expression.syntax())?;
                    self.ast
                        .push_edge(statement, JavaAstField::Expression, expression)?;
                }
                Ok(statement)
            }
            JavaStatement::Throw(node) => {
                let statement = self
                    .ast
                    .push_node(JavaAstKind::Throw, node.syntax().range().into())?;
                let expression = node.expression().ok_or(AstError::InconsistentCst {
                    context: "ThrowStatement",
                    expected: "expression",
                })?;
                let expression = self.lower_expression(expression.syntax())?;
                self.ast
                    .push_edge(statement, JavaAstField::Expression, expression)?;
                Ok(statement)
            }
            JavaStatement::Yield(node) => {
                let statement = self
                    .ast
                    .push_node(JavaAstKind::Yield, node.syntax().range().into())?;
                let expression = node.expression().ok_or(AstError::InconsistentCst {
                    context: "YieldStatement",
                    expected: "expression",
                })?;
                let expression = self.lower_expression(expression.syntax())?;
                self.ast
                    .push_edge(statement, JavaAstField::Value, expression)?;
                Ok(statement)
            }
            JavaStatement::Synchronized(node) => self.lower_synchronized(node),
            JavaStatement::Try(node) => self.lower_try(node),
            JavaStatement::TryWithResources(node) => self.lower_try_with_resources(node),
        }
    }

    fn lower_local_type(&mut self, node: &JavaLocalTypeDeclaration) -> Result<AstNodeId, AstError> {
        let declaration = local_type_declaration_shape(node).ok_or(AstError::InconsistentCst {
            context: "LocalTypeDeclaration",
            expected: "typed local type declaration",
        })?;
        self.lower_type_declaration_parts(&declaration)
    }

    fn lower_expression_statement(
        &mut self,
        node: &JavaExpressionStatement,
    ) -> Result<AstNodeId, AstError> {
        let statement = self.ast.push_node(
            JavaAstKind::ExpressionStatement,
            node.syntax().range().into(),
        )?;
        let expression = node.expression().ok_or(AstError::InconsistentCst {
            context: "ExpressionStatement",
            expected: "statement expression",
        })?;
        let expression = self.lower_expression(expression.syntax())?;
        self.ast
            .push_edge(statement, JavaAstField::Expression, expression)?;
        Ok(statement)
    }

    fn lower_constructor_invocation(&mut self, node: &SyntaxNode) -> Result<AstNodeId, AstError> {
        let shape = typed_node::<crate::typed::JavaExplicitConstructorInvocation>(
            node,
            "ExplicitConstructorInvocation",
            "typed constructor invocation",
        )?;
        let statement = self
            .ast
            .push_node(JavaAstKind::ExpressionStatement, node.range().into())?;
        let arguments = shape.arguments().ok_or(AstError::InconsistentCst {
            context: "ExplicitConstructorInvocation",
            expected: "argument list",
        })?;
        let invocation = self.ast.push_node(
            JavaAstKind::MethodInvocation,
            TextRange::new(node.from(), arguments.syntax().to()).into(),
        )?;
        if let Some(arguments) = shape.type_arguments() {
            for argument in arguments.arguments() {
                let argument = self.lower_type(argument.syntax())?;
                self.ast
                    .push_edge(invocation, JavaAstField::TypeArguments, argument)?;
            }
        }
        let ConstructorInvocationTarget { qualifier, target } =
            constructor_invocation_target(&shape).ok_or(AstError::InconsistentCst {
                context: "ExplicitConstructorInvocation",
                expected: "this or super",
            })?;
        let target_name = if kind(&target) == Some(JavaKind::This) {
            "this"
        } else {
            "super"
        };
        let select = if let Some(qualifier) = qualifier {
            let select = self.ast.push_node(
                JavaAstKind::MemberSelect,
                TextRange::new(qualifier.from(), target.to()).into(),
            )?;
            self.ast
                .push_name(select, target_name, Some(target.range()))?;
            let qualifier = self.lower_expression_or_keyword(&qualifier)?;
            self.ast
                .push_edge(select, JavaAstField::Expression, qualifier)?;
            select
        } else {
            self.push_synthetic_identifier(
                target_name,
                Some(target.from()),
                Some(target.to()),
                Some(target.range()),
            )?
        };
        self.ast
            .push_edge(invocation, JavaAstField::MethodSelect, select)?;
        for argument in arguments.arguments() {
            let argument = self.lower_expression(argument.syntax())?;
            self.ast
                .push_edge(invocation, JavaAstField::Arguments, argument)?;
        }
        self.ast
            .push_edge(statement, JavaAstField::Expression, invocation)?;
        Ok(statement)
    }

    fn lower_labeled(&mut self, node: &JavaLabeledStatement) -> Result<AstNodeId, AstError> {
        let label_node = node.label().ok_or(AstError::InconsistentCst {
            context: "LabeledStatement",
            expected: "label",
        })?;
        let identifier = label_node.identifier().ok_or(AstError::InconsistentCst {
            context: "Label",
            expected: "identifier",
        })?;
        let label = self.cooked_name(identifier.syntax())?;
        let statement = self
            .ast
            .push_node(JavaAstKind::LabeledStatement, node.syntax().range().into())?;
        self.ast
            .push_name(statement, &label, Some(identifier.syntax().range()))?;
        let body = node.statement().ok_or(AstError::InconsistentCst {
            context: "LabeledStatement",
            expected: "statement body",
        })?;
        let body = self.lower_statement(&body)?;
        self.ast
            .push_edge(statement, JavaAstField::Statement, body)?;
        Ok(statement)
    }

    fn lower_if(&mut self, node: &JavaIfStatement) -> Result<AstNodeId, AstError> {
        let statement = self
            .ast
            .push_node(JavaAstKind::If, node.syntax().range().into())?;
        let condition = node.condition().ok_or(AstError::InconsistentCst {
            context: "IfStatement",
            expected: "condition",
        })?;
        let condition = self.lower_expression(condition.syntax())?;
        self.ast
            .push_edge(statement, JavaAstField::Condition, condition)?;
        let then_body = node.then_statement().ok_or(AstError::InconsistentCst {
            context: "IfStatement",
            expected: "then statement",
        })?;
        let then_body = self.lower_statement(&then_body)?;
        self.ast
            .push_edge(statement, JavaAstField::ThenStatement, then_body)?;
        if let Some(else_body) = node.else_statement() {
            let else_body = self.lower_statement(&else_body)?;
            self.ast
                .push_edge(statement, JavaAstField::ElseStatement, else_body)?;
        }
        Ok(statement)
    }

    fn lower_while(&mut self, node: &JavaWhileStatement) -> Result<AstNodeId, AstError> {
        let statement = self
            .ast
            .push_node(JavaAstKind::WhileLoop, node.syntax().range().into())?;
        let condition = node.condition().ok_or(AstError::InconsistentCst {
            context: "WhileStatement",
            expected: "condition",
        })?;
        let condition = self.lower_expression(condition.syntax())?;
        self.ast
            .push_edge(statement, JavaAstField::Condition, condition)?;
        let body = node.statement().ok_or(AstError::InconsistentCst {
            context: "WhileStatement",
            expected: "statement body",
        })?;
        let body = self.lower_statement(&body)?;
        self.ast
            .push_edge(statement, JavaAstField::Statement, body)?;
        Ok(statement)
    }

    fn lower_do_while(&mut self, node: &JavaDoStatement) -> Result<AstNodeId, AstError> {
        let statement = self
            .ast
            .push_node(JavaAstKind::DoWhileLoop, node.syntax().range().into())?;
        let body = node.statement().ok_or(AstError::InconsistentCst {
            context: "DoStatement",
            expected: "statement body",
        })?;
        let body = self.lower_statement(&body)?;
        self.ast
            .push_edge(statement, JavaAstField::Statement, body)?;
        let condition = node.condition().ok_or(AstError::InconsistentCst {
            context: "DoStatement",
            expected: "condition",
        })?;
        let condition = self.lower_expression(condition.syntax())?;
        self.ast
            .push_edge(statement, JavaAstField::Condition, condition)?;
        Ok(statement)
    }

    fn lower_for(&mut self, node: &JavaForStatement) -> Result<AstNodeId, AstError> {
        let statement = self
            .ast
            .push_node(JavaAstKind::ForLoop, node.syntax().range().into())?;
        let specification = node.specification().ok_or(AstError::InconsistentCst {
            context: "ForStatement",
            expected: "ForSpec",
        })?;
        if let Some(initializer) = specification.initializer() {
            if let Some(declaration) = initializer.declaration() {
                for declaration in self.lower_local_variables(declaration.syntax())? {
                    self.ast
                        .push_edge(statement, JavaAstField::Initializer, declaration)?;
                }
            } else {
                for expression in initializer.expressions() {
                    let expression = self.lower_expression_statement_like(expression.syntax())?;
                    self.ast
                        .push_edge(statement, JavaAstField::Initializer, expression)?;
                }
            }
        }
        if let Some(condition) = specification.condition() {
            let expression = condition.expression().ok_or(AstError::InconsistentCst {
                context: "ForCondition",
                expected: "expression",
            })?;
            let expression = self.lower_expression(expression.syntax())?;
            self.ast
                .push_edge(statement, JavaAstField::Condition, expression)?;
        }
        if let Some(update) = specification.update() {
            for expression in update.expressions() {
                let expression = self.lower_expression_statement_like(expression.syntax())?;
                self.ast
                    .push_edge(statement, JavaAstField::Update, expression)?;
            }
        }
        let body = node.statement().ok_or(AstError::InconsistentCst {
            context: "ForStatement",
            expected: "statement body",
        })?;
        let body = self.lower_statement(&body)?;
        self.ast
            .push_edge(statement, JavaAstField::Statement, body)?;
        Ok(statement)
    }

    fn lower_expression_statement_like(
        &mut self,
        expression: &SyntaxNode,
    ) -> Result<AstNodeId, AstError> {
        let statement = self
            .ast
            .push_node(JavaAstKind::ExpressionStatement, expression.range().into())?;
        let expression = self.lower_expression(expression)?;
        self.ast
            .push_edge(statement, JavaAstField::Expression, expression)?;
        Ok(statement)
    }

    fn lower_enhanced_for(
        &mut self,
        node: &JavaEnhancedForStatement,
    ) -> Result<AstNodeId, AstError> {
        let statement = self
            .ast
            .push_node(JavaAstKind::EnhancedForLoop, node.syntax().range().into())?;
        let specification = node.specification().ok_or(AstError::InconsistentCst {
            context: "EnhancedForStatement",
            expected: "ForSpec",
        })?;
        let definition = specification
            .enhanced_name()
            .ok_or(AstError::InconsistentCst {
                context: "enhanced for",
                expected: "iteration variable",
            })?;
        let variable = self.lower_inline_variable(&specification, definition.syntax())?;
        self.ast
            .push_edge(statement, JavaAstField::Variable, variable)?;
        let expression = specification
            .enhanced_expression()
            .ok_or(AstError::InconsistentCst {
                context: "enhanced for",
                expected: "iterated expression",
            })?;
        let expression = self.lower_expression(expression.syntax())?;
        self.ast
            .push_edge(statement, JavaAstField::Expression, expression)?;
        let body = node.statement().ok_or(AstError::InconsistentCst {
            context: "EnhancedForStatement",
            expected: "statement body",
        })?;
        let body = self.lower_statement(&body)?;
        self.ast
            .push_edge(statement, JavaAstField::Statement, body)?;
        Ok(statement)
    }

    fn lower_assert(&mut self, node: &JavaAssertStatement) -> Result<AstNodeId, AstError> {
        let statement = self
            .ast
            .push_node(JavaAstKind::Assert, node.syntax().range().into())?;
        let condition = node.condition().ok_or(AstError::InconsistentCst {
            context: "AssertStatement",
            expected: "condition",
        })?;
        let condition = self.lower_expression(condition.syntax())?;
        self.ast
            .push_edge(statement, JavaAstField::Condition, condition)?;
        if let Some(detail) = node.detail() {
            let detail = self.lower_expression(detail.syntax())?;
            self.ast
                .push_edge(statement, JavaAstField::Detail, detail)?;
        }
        Ok(statement)
    }

    fn lower_jump(
        &mut self,
        node: &SyntaxNode,
        label: Option<crate::typed::JavaLabel>,
        kind: JavaAstKind,
    ) -> Result<AstNodeId, AstError> {
        let statement = self.ast.push_node(kind, node.range().into())?;
        if let Some(identifier) = label.and_then(|label| label.identifier()) {
            let name = self.cooked_name(identifier.syntax())?;
            self.ast
                .push_name(statement, &name, Some(identifier.syntax().range()))?;
        }
        Ok(statement)
    }

    fn lower_synchronized(
        &mut self,
        node: &JavaSynchronizedStatement,
    ) -> Result<AstNodeId, AstError> {
        let statement = self
            .ast
            .push_node(JavaAstKind::Synchronized, node.syntax().range().into())?;
        let expression = node.expression().ok_or(AstError::InconsistentCst {
            context: "SynchronizedStatement",
            expected: "lock expression",
        })?;
        let expression = self.lower_expression(expression.syntax())?;
        self.ast
            .push_edge(statement, JavaAstField::Expression, expression)?;
        let block = node.block().ok_or(AstError::InconsistentCst {
            context: "SynchronizedStatement",
            expected: "block",
        })?;
        let block = self.lower_block(block.syntax(), false)?;
        self.ast.push_edge(statement, JavaAstField::Block, block)?;
        Ok(statement)
    }

    fn lower_local_variables(
        &mut self,
        declaration: &SyntaxNode,
    ) -> Result<Vec<AstNodeId>, AstError> {
        let (declaration_core, final_end) = if let Ok(wrapper) =
            crate::typed::JavaLocalVariableDeclaration::downcast_from(declaration.clone())
        {
            let core = wrapper.declaration().ok_or(AstError::InconsistentCst {
                context: "LocalVariableDeclaration",
                expected: "local variable declaration core",
            })?;
            (core, declaration.to())
        } else {
            (
                typed_node::<crate::typed::JavaLocalVariableDeclarationCore>(
                    declaration,
                    "LocalVariableDeclaration",
                    "local variable declaration core",
                )?,
                declaration.to(),
            )
        };
        let base_type = declaration_core.type_().ok_or(AstError::InconsistentCst {
            context: "LocalVariableDeclaration",
            expected: "local variable type",
        })?;
        let modifiers_node = declaration_core.modifiers();
        let modifiers = Self::modifiers(modifiers_node.as_ref().map(TypedNode::syntax), &[]);
        let variables = declarator_entries(declaration_core.syntax(), final_end);
        let mut output = Vec::with_capacity(variables.len());
        for entry in variables {
            output.push(self.lower_variable(
                entry.declarator.syntax(),
                Some(base_type.syntax()),
                entry.range,
                modifiers_node.as_ref().map(TypedNode::syntax),
                &modifiers,
            )?);
        }
        Ok(output)
    }

    fn lower_inline_variable(
        &mut self,
        declaration: &crate::typed::JavaForSpec,
        definition: &SyntaxNode,
    ) -> Result<AstNodeId, AstError> {
        let modifiers_node = declaration.enhanced_modifiers();
        let ty = declaration
            .enhanced_type()
            .ok_or(AstError::InconsistentCst {
                context: "variable declaration",
                expected: "variable type",
            })?;
        let start = modifiers_node
            .as_ref()
            .map_or_else(|| ty.syntax().from(), |node| node.syntax().from());
        let variable = self.ast.push_node(
            JavaAstKind::Variable,
            TextRange::new(start, definition.to()).into(),
        )?;
        let name = self.cooked_name(definition)?;
        self.ast
            .push_name(variable, &name, Some(definition.range()))?;
        let modifiers = Self::modifiers(modifiers_node.as_ref().map(TypedNode::syntax), &[]);
        for modifier in &modifiers {
            self.ast
                .push_property(variable, JavaAstProperty::Modifier(*modifier))?;
        }
        let modifiers_ast = self.ast.push_node(
            JavaAstKind::Modifiers,
            modifiers_node
                .as_ref()
                .map_or_else(JavaSourceRange::default, |node| {
                    node.syntax().range().into()
                }),
        )?;
        for modifier in modifiers {
            self.ast
                .push_property(modifiers_ast, JavaAstProperty::Modifier(modifier))?;
        }
        if let Some(modifiers) = modifiers_node.as_ref() {
            self.lower_modifier_annotations(modifiers_ast, modifiers.syntax())?;
        }
        self.ast
            .push_edge(variable, JavaAstField::Modifiers, modifiers_ast)?;
        if !matches!(ty, crate::typed::JavaLocalVariableType::Inferred(_)) {
            let dimensions = declaration
                .enhanced_dimensions()
                .map(TypedNode::into_syntax)
                .collect::<Vec<_>>();
            let end = dimensions
                .last()
                .map_or_else(|| ty.syntax().to(), SyntaxNode::to);
            let ty = self.lower_type_with_dimensions(
                ty.syntax(),
                &dimensions,
                TextRange::new(ty.syntax().from(), end),
            )?;
            self.ast.push_edge(variable, JavaAstField::Type, ty)?;
        }
        Ok(variable)
    }

    fn lower_try(&mut self, node: &JavaTryStatement) -> Result<AstNodeId, AstError> {
        let statement = self
            .ast
            .push_node(JavaAstKind::Try, node.syntax().range().into())?;
        let body = node.block().ok_or(AstError::InconsistentCst {
            context: "TryStatement",
            expected: "try block",
        })?;
        self.lower_try_tail(statement, &body, node.catches(), node.finally())?;
        Ok(statement)
    }

    fn lower_try_with_resources(
        &mut self,
        node: &JavaTryWithResourcesStatement,
    ) -> Result<AstNodeId, AstError> {
        let statement = self
            .ast
            .push_node(JavaAstKind::Try, node.syntax().range().into())?;
        if let Some(specification) = node.resources() {
            self.lower_resources(statement, &specification)?;
        }
        let body = node.block().ok_or(AstError::InconsistentCst {
            context: "TryWithResourcesStatement",
            expected: "try block",
        })?;
        self.lower_try_tail(statement, &body, node.catches(), node.finally())?;
        Ok(statement)
    }

    fn lower_resources(
        &mut self,
        statement: AstNodeId,
        specification: &JavaResourceSpecification,
    ) -> Result<(), AstError> {
        for entry in resource_entries(specification) {
            let resource = self.lower_resource(entry.resource.syntax())?;
            self.ast.set_range(resource, entry.range.into())?;
            self.ast
                .push_edge(statement, JavaAstField::Resources, resource)?;
        }
        Ok(())
    }

    fn lower_try_tail(
        &mut self,
        statement: AstNodeId,
        body: &JavaBlock,
        catches: impl IntoIterator<Item = JavaCatchClause>,
        finally: Option<JavaFinallyClause>,
    ) -> Result<(), AstError> {
        let body = self.lower_block(body.syntax(), false)?;
        self.ast.push_edge(statement, JavaAstField::Block, body)?;
        for clause in catches {
            let clause = self.lower_catch(&clause)?;
            self.ast
                .push_edge(statement, JavaAstField::Catches, clause)?;
        }
        if let Some(finally) = finally {
            let block = finally.block().ok_or(AstError::InconsistentCst {
                context: "FinallyClause",
                expected: "block",
            })?;
            let block = self.lower_block(block.syntax(), false)?;
            self.ast
                .push_edge(statement, JavaAstField::FinallyBlock, block)?;
        }
        Ok(())
    }

    fn lower_resource(&mut self, node: &SyntaxNode) -> Result<AstNodeId, AstError> {
        let shape = typed_node::<JavaResource>(node, "Resource", "typed resource")?;
        let Some(definition) = shape.name() else {
            let expression = shape.expression().ok_or(AstError::InconsistentCst {
                context: "Resource",
                expected: "resource declaration or expression",
            })?;
            return self.lower_expression(expression.syntax());
        };
        let variable = self
            .ast
            .push_node(JavaAstKind::Variable, node.range().into())?;
        let name = self.cooked_name(definition.syntax())?;
        self.ast
            .push_name(variable, &name, Some(definition.syntax().range()))?;
        let modifiers_node = shape.modifiers();
        let modifiers = Self::modifiers(modifiers_node.as_ref().map(TypedNode::syntax), &[]);
        for modifier in &modifiers {
            self.ast
                .push_property(variable, JavaAstProperty::Modifier(*modifier))?;
        }
        let modifiers_ast = self.ast.push_node(
            JavaAstKind::Modifiers,
            modifiers_node
                .as_ref()
                .map_or_else(JavaSourceRange::default, |node| {
                    node.syntax().range().into()
                }),
        )?;
        for modifier in modifiers {
            self.ast
                .push_property(modifiers_ast, JavaAstProperty::Modifier(modifier))?;
        }
        if let Some(modifiers) = modifiers_node.as_ref() {
            self.lower_modifier_annotations(modifiers_ast, modifiers.syntax())?;
        }
        self.ast
            .push_edge(variable, JavaAstField::Modifiers, modifiers_ast)?;
        let ty = shape.type_().ok_or(AstError::InconsistentCst {
            context: "Resource",
            expected: "resource type",
        })?;
        if kind(ty.syntax()) != Some(JavaKind::InferredLocalType) {
            let ty = self.lower_type(ty.syntax())?;
            self.ast.push_edge(variable, JavaAstField::Type, ty)?;
        }
        let initializer = shape.expression().ok_or(AstError::InconsistentCst {
            context: "Resource",
            expected: "resource initializer",
        })?;
        let initializer = self.lower_expression(initializer.syntax())?;
        self.ast
            .push_edge(variable, JavaAstField::Initializer, initializer)?;
        Ok(variable)
    }

    fn lower_catch(&mut self, node: &JavaCatchClause) -> Result<AstNodeId, AstError> {
        let clause = self
            .ast
            .push_node(JavaAstKind::Catch, node.syntax().range().into())?;
        let parameter = node.parameter().ok_or(AstError::InconsistentCst {
            context: "CatchClause",
            expected: "catch parameter",
        })?;
        let parameter = self.lower_catch_parameter(parameter.syntax())?;
        self.ast
            .push_edge(clause, JavaAstField::Parameter, parameter)?;
        let block = node.block().ok_or(AstError::InconsistentCst {
            context: "CatchClause",
            expected: "block",
        })?;
        let block = self.lower_block(block.syntax(), false)?;
        self.ast.push_edge(clause, JavaAstField::Block, block)?;
        Ok(clause)
    }

    fn lower_catch_parameter(&mut self, node: &SyntaxNode) -> Result<AstNodeId, AstError> {
        let shape = typed_node::<crate::typed::JavaCatchFormalParameter>(
            node,
            "CatchFormalParameter",
            "typed catch parameter",
        )?;
        let definition = shape.name().ok_or(AstError::InconsistentCst {
            context: "CatchFormalParameter",
            expected: "name",
        })?;
        let catch_type = shape.type_().ok_or(AstError::InconsistentCst {
            context: "CatchFormalParameter",
            expected: "catch type",
        })?;
        let modifiers_node = shape.modifiers();
        let start = modifiers_node
            .as_ref()
            .map_or_else(|| catch_type.syntax().from(), |node| node.syntax().from());
        let variable = self.ast.push_node(
            JavaAstKind::Variable,
            TextRange::new(start, definition.syntax().to()).into(),
        )?;
        let name = self.cooked_name(definition.syntax())?;
        self.ast
            .push_name(variable, &name, Some(definition.syntax().range()))?;
        let modifiers = Self::modifiers(modifiers_node.as_ref().map(TypedNode::syntax), &[]);
        for modifier in &modifiers {
            self.ast
                .push_property(variable, JavaAstProperty::Modifier(*modifier))?;
        }
        let modifiers_ast = self.ast.push_node(
            JavaAstKind::Modifiers,
            modifiers_node
                .as_ref()
                .map_or_else(JavaSourceRange::default, |node| {
                    node.syntax().range().into()
                }),
        )?;
        for modifier in modifiers {
            self.ast
                .push_property(modifiers_ast, JavaAstProperty::Modifier(modifier))?;
        }
        if let Some(modifiers) = modifiers_node.as_ref() {
            self.lower_modifier_annotations(modifiers_ast, modifiers.syntax())?;
        }
        self.ast
            .push_edge(variable, JavaAstField::Modifiers, modifiers_ast)?;
        let alternatives = catch_type
            .alternatives()
            .map(TypedNode::into_syntax)
            .collect::<Vec<_>>();
        let ty = if alternatives.len() > 1 {
            let union = self
                .ast
                .push_node(JavaAstKind::UnionType, catch_type.syntax().range().into())?;
            for alternative in alternatives {
                let alternative = self.lower_type(&alternative)?;
                self.ast
                    .push_edge(union, JavaAstField::TypeAlternatives, alternative)?;
            }
            union
        } else {
            let alternative = alternatives.first().ok_or(AstError::InconsistentCst {
                context: "CatchType",
                expected: "exception type",
            })?;
            self.lower_type(alternative)?
        };
        self.ast.push_edge(variable, JavaAstField::Type, ty)?;
        Ok(variable)
    }

    fn lower_switch(&mut self, node: &SyntaxNode, expression: bool) -> Result<AstNodeId, AstError> {
        if !expression {
            let shape = typed_node::<JavaSwitchStatement>(node, "SwitchStatement", "typed switch")?;
            return self.lower_switch_statement(&shape);
        }
        let shape = typed_node::<JavaSwitchExpression>(
            node,
            "SwitchExpression",
            "typed switch expression",
        )?;
        let switch = self
            .ast
            .push_node(JavaAstKind::SwitchExpression, node.range().into())?;
        let selector = shape.selector().ok_or(AstError::InconsistentCst {
            context: "switch expression",
            expected: "selector",
        })?;
        let selector = self.lower_expression(selector.syntax())?;
        self.ast
            .push_edge(switch, JavaAstField::Expression, selector)?;
        let block = shape.body().ok_or(AstError::InconsistentCst {
            context: "switch expression",
            expected: "switch block",
        })?;
        for entry in block.items() {
            match entry {
                JavaSwitchExpressionItem::Rule(rule) => {
                    let case = self.lower_switch_expression_rule(&rule)?;
                    self.ast.push_edge(switch, JavaAstField::Cases, case)?;
                }
                JavaSwitchExpressionItem::Group(group) => {
                    self.lower_switch_group(switch, &group)?;
                }
            }
        }
        Ok(switch)
    }

    fn lower_switch_statement(
        &mut self,
        shape: &JavaSwitchStatement,
    ) -> Result<AstNodeId, AstError> {
        let node = shape.syntax();
        let switch = self
            .ast
            .push_node(JavaAstKind::Switch, node.range().into())?;
        let selector = shape.selector().ok_or(AstError::InconsistentCst {
            context: "switch statement",
            expected: "selector",
        })?;
        let selector = self.lower_expression(selector.syntax())?;
        self.ast
            .push_edge(switch, JavaAstField::Expression, selector)?;
        let block = shape.body().ok_or(AstError::InconsistentCst {
            context: "switch statement",
            expected: "switch block",
        })?;
        for entry in block.items() {
            match entry {
                JavaSwitchStatementItem::Rule(rule) => {
                    let case = self.lower_switch_statement_rule(&rule)?;
                    self.ast.push_edge(switch, JavaAstField::Cases, case)?;
                }
                JavaSwitchStatementItem::Group(group) => {
                    self.lower_switch_group(switch, &group)?;
                }
            }
        }
        Ok(switch)
    }

    fn lower_switch_statement_rule(
        &mut self,
        node: &JavaSwitchStatementRule,
    ) -> Result<AstNodeId, AstError> {
        let case = self
            .ast
            .push_node(JavaAstKind::Case, node.syntax().range().into())?;
        self.ast.push_property(
            case,
            JavaAstProperty::CaseKind(super::model::JavaCaseKind::Rule),
        )?;
        let label = node.label().ok_or(AstError::InconsistentCst {
            context: "switch rule",
            expected: "switch label",
        })?;
        self.lower_switch_label(case, label.syntax())?;
        if let Some(outcome) = node.outcome() {
            let outcome = match outcome {
                JavaSwitchStatementOutcome::Expression(outcome) => {
                    let statement = self.lower_expression_statement_like(outcome.syntax())?;
                    let end = node
                        .semicolon_token()
                        .map_or_else(|| outcome.syntax().to(), |semicolon| semicolon.to());
                    self.ast.set_range(
                        statement,
                        TextRange::new(outcome.syntax().from(), end).into(),
                    )?;
                    statement
                }
                JavaSwitchStatementOutcome::Statement(outcome) => self.lower_statement(&outcome)?,
            };
            self.ast.push_edge(case, JavaAstField::Body, outcome)?;
        }
        Ok(case)
    }

    fn lower_switch_expression_rule(
        &mut self,
        node: &JavaSwitchExpressionRule,
    ) -> Result<AstNodeId, AstError> {
        let case = self
            .ast
            .push_node(JavaAstKind::Case, node.syntax().range().into())?;
        self.ast.push_property(
            case,
            JavaAstProperty::CaseKind(super::model::JavaCaseKind::Rule),
        )?;
        let label = node.label().ok_or(AstError::InconsistentCst {
            context: "switch expression rule",
            expected: "switch label",
        })?;
        self.lower_switch_label(case, label.syntax())?;
        if let Some(outcome) = node.outcome() {
            let outcome = match outcome {
                JavaSwitchExpressionOutcome::Expression(outcome) => {
                    self.lower_expression(outcome.syntax())?
                }
                JavaSwitchExpressionOutcome::Statement(outcome) => {
                    self.lower_statement(&outcome)?
                }
            };
            self.ast.push_edge(case, JavaAstField::Body, outcome)?;
        }
        Ok(case)
    }

    fn lower_switch_group(
        &mut self,
        parent: AstNodeId,
        node: &JavaSwitchBlockStatementGroup,
    ) -> Result<(), AstError> {
        let syntax = node.syntax();
        let case = self
            .ast
            .push_node(JavaAstKind::Case, syntax.range().into())?;
        self.ast.push_property(
            case,
            JavaAstProperty::CaseKind(super::model::JavaCaseKind::Statement),
        )?;
        let label = node.label().ok_or(AstError::InconsistentCst {
            context: "switch statement group",
            expected: "switch label",
        })?;
        self.lower_switch_label(case, label.syntax())?;
        let body = node.body().ok_or(AstError::InconsistentCst {
            context: "switch statement group",
            expected: "group body",
        })?;
        self.lower_switch_group_body(case, &body)?;
        let colon_end = node.colon_token().map_or(syntax.from(), |colon| colon.to());
        let end = self
            .ast
            .child_range_end(case)
            .unwrap_or(colon_end)
            .max(colon_end);
        self.ast
            .set_range(case, JavaSourceRange::new(Some(syntax.from()), Some(end)))?;
        self.ast.push_edge(parent, JavaAstField::Cases, case)?;
        Ok(())
    }

    fn lower_switch_group_body(
        &mut self,
        case: AstNodeId,
        node: &crate::typed::JavaSwitchBlockStatementGroupBody,
    ) -> Result<(), AstError> {
        let statements = node.statements().ok_or(AstError::InconsistentCst {
            context: "switch statement group",
            expected: "group statements",
        })?;
        self.lower_switch_group_statements(case, &statements)?;
        Ok(())
    }

    fn lower_switch_group_statements(
        &mut self,
        case: AstNodeId,
        node: &JavaSwitchGroupStatements,
    ) -> Result<(), AstError> {
        if let Some(statement) = node.statements() {
            if let JavaStatement::LocalVariable(declaration) = statement {
                for statement in self.lower_local_variables(declaration.syntax())? {
                    self.ast
                        .push_edge(case, JavaAstField::Statements, statement)?;
                }
            } else {
                let statement = self.lower_statement(&statement)?;
                self.ast
                    .push_edge(case, JavaAstField::Statements, statement)?;
            }
        }
        if let Some(nested) = node.nested() {
            self.lower_switch_group_statements(case, &nested)?;
        }
        Ok(())
    }

    fn lower_switch_label(&mut self, case: AstNodeId, label: &SyntaxNode) -> Result<(), AstError> {
        let shape = typed_node::<JavaSwitchLabel>(label, "SwitchLabel", "typed switch label")?;
        if let Some(default) = shape.default_token() {
            let label = self
                .ast
                .push_node(JavaAstKind::DefaultCaseLabel, default.range().into())?;
            self.ast.push_edge(case, JavaAstField::Labels, label)?;
            return Ok(());
        }
        let rest = shape.labels().ok_or(AstError::InconsistentCst {
            context: "SwitchLabel",
            expected: "case label rest",
        })?;
        for element in rest.elements() {
            let element_syntax = element.syntax();
            let label = if let Some(pattern) = element.pattern() {
                let label = self
                    .ast
                    .push_node(JavaAstKind::PatternCaseLabel, element_syntax.range().into())?;
                let pattern = self.lower_pattern(pattern.syntax())?;
                self.ast.push_edge(label, JavaAstField::Pattern, pattern)?;
                label
            } else if let Some(default) = element.default_token() {
                self.ast
                    .push_node(JavaAstKind::DefaultCaseLabel, default.range().into())?
            } else {
                let expression = element.expression().ok_or(AstError::InconsistentCst {
                    context: "SwitchCaseElement",
                    expected: "constant expression, pattern, or default",
                })?;
                let label = self.ast.push_node(
                    JavaAstKind::ConstantCaseLabel,
                    element_syntax.range().into(),
                )?;
                let expression = self.lower_expression(expression.syntax())?;
                self.ast
                    .push_edge(label, JavaAstField::ConstantExpression, expression)?;
                label
            };
            self.ast.push_edge(case, JavaAstField::Labels, label)?;
        }
        if let Some(guard) = rest.guard() {
            let expression = guard.expression().ok_or(AstError::InconsistentCst {
                context: "Guard",
                expected: "guard expression",
            })?;
            let expression = self.lower_expression(expression.syntax())?;
            self.ast.push_edge(case, JavaAstField::Guard, expression)?;
        }
        Ok(())
    }

    fn lower_annotation(
        &mut self,
        node: &SyntaxNode,
        type_use: bool,
    ) -> Result<AstNodeId, AstError> {
        let shape = typed_node::<JavaAnnotationNode>(node, "annotation", "typed annotation")?;
        let annotation = self.ast.push_node(
            if type_use {
                JavaAstKind::TypeAnnotation
            } else {
                JavaAstKind::Annotation
            },
            node.range().into(),
        )?;
        let (name, arguments) = match shape {
            JavaAnnotationNode::Marker(annotation) => (annotation.name(), None),
            JavaAnnotationNode::Normal(annotation) => (annotation.name(), annotation.arguments()),
        };
        let name = name.ok_or(AstError::InconsistentCst {
            context: "annotation",
            expected: "annotation type",
        })?;
        let name = self.lower_qualified_name(name.syntax())?;
        self.ast
            .push_edge(annotation, JavaAstField::AnnotationType, name)?;
        if let Some(arguments) = arguments {
            for argument in arguments.values() {
                let lowered = self.lower_annotation_value(&argument)?;
                self.ast
                    .push_edge(annotation, JavaAstField::Arguments, lowered)?;
            }
        }
        Ok(annotation)
    }

    fn lower_type_name(&mut self, node: &SyntaxNode) -> Result<AstNodeId, AstError> {
        let name = self.cooked_name(node)?;
        let identifier = self
            .ast
            .push_node(JavaAstKind::Identifier, node.range().into())?;
        self.ast.push_name(identifier, &name, Some(node.range()))?;
        Ok(identifier)
    }

    fn lower_scoped_type(&mut self, node: &SyntaxNode) -> Result<AstNodeId, AstError> {
        let shape = typed_node::<JavaScopedTypeName>(node, "ScopedTypeName", "typed scoped type")?;
        let qualifier = shape.qualifier().ok_or(AstError::InconsistentCst {
            context: "ScopedTypeName",
            expected: "qualifier type",
        })?;
        let segment = shape.name().ok_or(AstError::InconsistentCst {
            context: "ScopedTypeName",
            expected: "selected type",
        })?;
        let annotations = shape.annotations().collect::<Vec<_>>();
        let annotated = if annotations.is_empty() {
            None
        } else {
            let annotated = self
                .ast
                .push_node(JavaAstKind::AnnotatedType, node.range().into())?;
            for annotation in annotations {
                let annotation = self.lower_annotation(annotation.syntax(), true)?;
                self.ast
                    .push_edge(annotated, JavaAstField::Annotations, annotation)?;
            }
            Some(annotated)
        };
        let name = self.cooked_name(segment.syntax())?;
        let selected = self
            .ast
            .push_node(JavaAstKind::MemberSelect, node.range().into())?;
        self.ast
            .push_name(selected, &name, Some(segment.syntax().range()))?;
        let qualifier = self.lower_type(qualifier.syntax())?;
        self.ast
            .push_edge(selected, JavaAstField::Expression, qualifier)?;
        if let Some(annotated) = annotated {
            self.ast
                .push_edge(annotated, JavaAstField::UnderlyingType, selected)?;
            Ok(annotated)
        } else {
            Ok(selected)
        }
    }

    fn lower_parameterized_type(&mut self, node: &SyntaxNode) -> Result<AstNodeId, AstError> {
        let shape = typed_node::<JavaGenericType>(node, "GenericType", "typed generic type")?;
        let parameterized = self
            .ast
            .push_node(JavaAstKind::ParameterizedType, node.range().into())?;
        let base = shape.type_().ok_or(AstError::InconsistentCst {
            context: "GenericType",
            expected: "base type",
        })?;
        let base = self.lower_type(base.syntax())?;
        self.ast
            .push_edge(parameterized, JavaAstField::Type, base)?;
        let arguments = shape.arguments().ok_or(AstError::InconsistentCst {
            context: "GenericType",
            expected: "type arguments",
        })?;
        for argument in arguments.arguments() {
            let argument = self.lower_type(argument.syntax())?;
            self.ast
                .push_edge(parameterized, JavaAstField::TypeArguments, argument)?;
        }
        Ok(parameterized)
    }

    fn lower_annotated_type(&mut self, node: &SyntaxNode) -> Result<AstNodeId, AstError> {
        let shape =
            typed_node::<crate::typed::JavaAnnotatedType>(node, "AnnotatedType", "typed type")?;
        let annotated = self
            .ast
            .push_node(JavaAstKind::AnnotatedType, node.range().into())?;
        for annotation in shape.annotations() {
            let annotation = self.lower_annotation(annotation.syntax(), true)?;
            self.ast
                .push_edge(annotated, JavaAstField::Annotations, annotation)?;
        }
        let underlying = shape.type_().ok_or(AstError::InconsistentCst {
            context: "AnnotatedType",
            expected: "underlying type",
        })?;
        let underlying = self.lower_type(underlying.syntax())?;
        self.ast
            .push_edge(annotated, JavaAstField::UnderlyingType, underlying)?;
        Ok(annotated)
    }

    fn lower_array_type(&mut self, node: &SyntaxNode) -> Result<AstNodeId, AstError> {
        self.lower_array_type_with_range(node, node.range())
    }

    fn lower_array_type_with_range(
        &mut self,
        node: &SyntaxNode,
        full_range: TextRange,
    ) -> Result<AstNodeId, AstError> {
        let mut dimensions = Vec::new();
        let base = collect_array_parts(node, &mut dimensions)?;
        self.lower_type_with_dimensions(&base, &dimensions, full_range)
    }

    fn lower_type_with_dimensions(
        &mut self,
        base: &SyntaxNode,
        dimensions: &[SyntaxNode],
        full_range: TextRange,
    ) -> Result<AstNodeId, AstError> {
        let Some((dimension, remaining)) = dimensions.split_first() else {
            return self.lower_type(base);
        };
        let dimension = typed_node::<JavaDimension>(dimension, "Dimension", "typed dimension")?;
        let annotations = dimension.annotations().collect::<Vec<_>>();
        let annotated = if annotations.is_empty() {
            None
        } else {
            let annotated = self
                .ast
                .push_node(JavaAstKind::AnnotatedType, full_range.into())?;
            for annotation in annotations {
                let annotation = self.lower_annotation(annotation.syntax(), true)?;
                self.ast
                    .push_edge(annotated, JavaAstField::Annotations, annotation)?;
            }
            Some(annotated)
        };
        let array = self
            .ast
            .push_node(JavaAstKind::ArrayType, full_range.into())?;
        let element = self.lower_type_with_dimensions(base, remaining, full_range)?;
        self.ast.push_edge(array, JavaAstField::Type, element)?;
        if let Some(annotated) = annotated {
            self.ast
                .push_edge(annotated, JavaAstField::UnderlyingType, array)?;
            Ok(annotated)
        } else {
            Ok(array)
        }
    }

    fn lower_created_type_with_dimensions(
        &mut self,
        base: &SyntaxNode,
        dimensions: &[SyntaxNode],
        full_range: TextRange,
    ) -> Result<AstNodeId, AstError> {
        let Some((dimension, remaining)) = dimensions.split_first() else {
            return self.lower_created_type(base);
        };
        let dimension = typed_node::<JavaDimension>(dimension, "Dimension", "typed dimension")?;
        let annotations = dimension.annotations().collect::<Vec<_>>();
        let annotated = if annotations.is_empty() {
            None
        } else {
            let annotated = self
                .ast
                .push_node(JavaAstKind::AnnotatedType, full_range.into())?;
            for annotation in annotations {
                let annotation = self.lower_annotation(annotation.syntax(), true)?;
                self.ast
                    .push_edge(annotated, JavaAstField::Annotations, annotation)?;
            }
            Some(annotated)
        };
        let array = self
            .ast
            .push_node(JavaAstKind::ArrayType, full_range.into())?;
        let element = self.lower_created_type_with_dimensions(base, remaining, full_range)?;
        self.ast.push_edge(array, JavaAstField::Type, element)?;
        if let Some(annotated) = annotated {
            self.ast
                .push_edge(annotated, JavaAstField::UnderlyingType, array)?;
            Ok(annotated)
        } else {
            Ok(array)
        }
    }

    fn lower_varargs_type(
        &mut self,
        declaration: &SyntaxNode,
        base: &SyntaxNode,
    ) -> Result<AstNodeId, AstError> {
        let (ellipsis, annotations) = match kind(declaration) {
            Some(JavaKind::SpreadParameter) => {
                let shape = typed_node::<crate::typed::JavaSpreadParameter>(
                    declaration,
                    "SpreadParameter",
                    "typed spread parameter",
                )?;
                let ellipsis = shape.ellipsis_token().ok_or(AstError::InconsistentCst {
                    context: "SpreadParameter",
                    expected: "ellipsis",
                })?;
                (ellipsis, shape.varargs_annotations().collect::<Vec<_>>())
            }
            Some(JavaKind::RecordComponent) => {
                let shape = typed_node::<crate::typed::JavaRecordComponent>(
                    declaration,
                    "RecordComponent",
                    "typed record component",
                )?;
                let ellipsis = shape.ellipsis_token().ok_or(AstError::InconsistentCst {
                    context: "RecordComponent",
                    expected: "ellipsis",
                })?;
                (ellipsis, shape.varargs_annotations().collect::<Vec<_>>())
            }
            _ => {
                return Err(AstError::InconsistentCst {
                    context: "variable arity declaration",
                    expected: "spread parameter or record component",
                });
            }
        };
        let full_range = TextRange::new(base.from(), ellipsis.to());
        let annotation_end = annotations.last().map(|node| node.syntax().to());
        let annotated = if annotations.is_empty() {
            None
        } else {
            let annotated = self
                .ast
                .push_node(JavaAstKind::AnnotatedType, full_range.into())?;
            for annotation in annotations {
                let annotation = self.lower_annotation(annotation.syntax(), true)?;
                self.ast
                    .push_edge(annotated, JavaAstField::Annotations, annotation)?;
            }
            Some(annotated)
        };
        let array = self
            .ast
            .push_node(JavaAstKind::ArrayType, full_range.into())?;
        let element = if kind(base) == Some(JavaKind::ArrayType) {
            self.lower_array_type_with_range(base, full_range)?
        } else {
            self.lower_type(base)?
        };
        if let Some(annotation_end) = annotation_end {
            self.ast
                .set_range(element, TextRange::new(base.from(), annotation_end).into())?;
        }
        self.ast.push_edge(array, JavaAstField::Type, element)?;
        if let Some(annotated) = annotated {
            self.ast
                .push_edge(annotated, JavaAstField::UnderlyingType, array)?;
            Ok(annotated)
        } else {
            Ok(array)
        }
    }

    fn lower_wildcard(&mut self, node: &SyntaxNode) -> Result<AstNodeId, AstError> {
        let shape = typed_node::<JavaWildcard>(node, "Wildcard", "typed wildcard")?;
        let kind = if shape.extends_token().is_some() {
            JavaAstKind::ExtendsWildcard
        } else if shape.super_token().is_some() {
            JavaAstKind::SuperWildcard
        } else {
            JavaAstKind::UnboundedWildcard
        };
        let question = shape.question_token().ok_or(AstError::InconsistentCst {
            context: "Wildcard",
            expected: "question mark",
        })?;
        let underlying_range = TextRange::new(question.from(), node.to());
        let annotations = shape.annotations().collect::<Vec<_>>();
        let annotated = if annotations.is_empty() {
            None
        } else {
            let annotated = self
                .ast
                .push_node(JavaAstKind::AnnotatedType, node.range().into())?;
            for annotation in annotations {
                let annotation = self.lower_annotation(annotation.syntax(), true)?;
                self.ast
                    .push_edge(annotated, JavaAstField::Annotations, annotation)?;
            }
            Some(annotated)
        };
        let wildcard = self.ast.push_node(kind, underlying_range.into())?;
        if let Some(bound) = shape.bound() {
            let bound = self.lower_type(bound.syntax())?;
            self.ast.push_edge(wildcard, JavaAstField::Bound, bound)?;
        }
        if let Some(annotated) = annotated {
            self.ast
                .push_edge(annotated, JavaAstField::UnderlyingType, wildcard)?;
            Ok(annotated)
        } else {
            Ok(wildcard)
        }
    }

    fn lower_primitive_type(&mut self, node: &SyntaxNode) -> Result<AstNodeId, AstError> {
        let spelling = self.input.read_logical(node.range());
        let primitive = match spelling.as_ref() {
            "boolean" => JavaPrimitiveKind::Boolean,
            "byte" => JavaPrimitiveKind::Byte,
            "short" => JavaPrimitiveKind::Short,
            "int" => JavaPrimitiveKind::Int,
            "long" => JavaPrimitiveKind::Long,
            "char" => JavaPrimitiveKind::Char,
            "float" => JavaPrimitiveKind::Float,
            "double" => JavaPrimitiveKind::Double,
            "void" => JavaPrimitiveKind::Void,
            _ => {
                return Err(AstError::InconsistentCst {
                    context: "primitive type",
                    expected: "Java primitive spelling",
                });
            }
        };
        let node_id = self
            .ast
            .push_node(JavaAstKind::PrimitiveType, node.range().into())?;
        self.ast
            .push_property(node_id, JavaAstProperty::PrimitiveKind(primitive))?;
        Ok(node_id)
    }

    fn lower_type_wrapper(&mut self, node: &SyntaxNode) -> Result<AstNodeId, AstError> {
        let (underlying, annotations) = match kind(node) {
            Some(JavaKind::ExceptionType) => {
                let shape = typed_node::<crate::typed::JavaExceptionType>(
                    node,
                    "ExceptionType",
                    "typed exception type",
                )?;
                let underlying = shape.type_().ok_or(AstError::InconsistentCst {
                    context: "ExceptionType",
                    expected: "reference type",
                })?;
                (
                    underlying.syntax().clone(),
                    shape.annotations().collect::<Vec<_>>(),
                )
            }
            Some(JavaKind::CatchAlternativeType) => {
                let shape = typed_node::<crate::typed::JavaCatchAlternativeType>(
                    node,
                    "CatchAlternativeType",
                    "typed catch alternative",
                )?;
                let underlying = shape.type_().ok_or(AstError::InconsistentCst {
                    context: "CatchAlternativeType",
                    expected: "reference type",
                })?;
                (
                    underlying.syntax().clone(),
                    shape.annotations().collect::<Vec<_>>(),
                )
            }
            _ => {
                return Err(AstError::InconsistentCst {
                    context: "type wrapper",
                    expected: "exception type wrapper",
                });
            }
        };
        if annotations.is_empty() {
            return self.lower_type(&underlying);
        }
        let annotated = self
            .ast
            .push_node(JavaAstKind::AnnotatedType, node.range().into())?;
        for annotation in annotations {
            let annotation = self.lower_annotation(annotation.syntax(), true)?;
            self.ast
                .push_edge(annotated, JavaAstField::Annotations, annotation)?;
        }
        let underlying = self.lower_type(&underlying)?;
        self.ast
            .push_edge(annotated, JavaAstField::UnderlyingType, underlying)?;
        Ok(annotated)
    }

    fn lower_annotation_value(
        &mut self,
        value: &JavaAnnotationValue,
    ) -> Result<AstNodeId, AstError> {
        match value {
            JavaAnnotationValue::Expression(value) => self.lower_expression(value.syntax()),
            JavaAnnotationValue::Array(value) => self.lower_element_value_array(value.syntax()),
            JavaAnnotationValue::Annotation(value) => self.lower_annotation(value.syntax(), false),
            JavaAnnotationValue::Pair(value) => self.lower_element_value_pair(value),
        }
    }

    fn lower_element_value_pair(
        &mut self,
        node: &JavaElementValuePair,
    ) -> Result<AstNodeId, AstError> {
        let assignment = self
            .ast
            .push_node(JavaAstKind::Assignment, node.syntax().range().into())?;
        let identifier = node.name().ok_or(AstError::InconsistentCst {
            context: "ElementValuePair",
            expected: "name",
        })?;
        let variable = self.lower_expression(identifier.syntax())?;
        self.ast
            .push_edge(assignment, JavaAstField::Variable, variable)?;
        let value = node.value().ok_or(AstError::InconsistentCst {
            context: "ElementValuePair",
            expected: "element value",
        })?;
        let value = self.lower_annotation_value(&value)?;
        self.ast
            .push_edge(assignment, JavaAstField::Expression, value)?;
        Ok(assignment)
    }

    fn lower_element_value_array(&mut self, node: &SyntaxNode) -> Result<AstNodeId, AstError> {
        let shape = typed_node::<crate::typed::JavaElementValueArrayInitializer>(
            node,
            "ElementValueArrayInitializer",
            "annotation value array",
        )?;
        let array = self
            .ast
            .push_node(JavaAstKind::NewArray, node.range().into())?;
        for value in shape.values() {
            let value = match value {
                crate::typed::JavaAnnotationValue::Array(value) => {
                    Some(self.lower_element_value_array(value.syntax())?)
                }
                crate::typed::JavaAnnotationValue::Annotation(value) => {
                    Some(self.lower_annotation(value.syntax(), false)?)
                }
                crate::typed::JavaAnnotationValue::Expression(value) => {
                    Some(self.lower_expression(value.syntax())?)
                }
                crate::typed::JavaAnnotationValue::Pair(_) => None,
            };
            if let Some(value) = value {
                self.ast
                    .push_edge(array, JavaAstField::Initializers, value)?;
            }
        }
        Ok(array)
    }

    fn lower_expression(&mut self, node: &SyntaxNode) -> Result<AstNodeId, AstError> {
        match kind(node) {
            Some(JavaKind::StatementExpression) => {
                let statement = typed_node::<crate::typed::JavaStatementExpression>(
                    node,
                    "StatementExpression",
                    "statement expression",
                )?;
                let expression = statement.expression().ok_or(AstError::InconsistentCst {
                    context: "StatementExpression",
                    expected: "expression",
                })?;
                self.lower_expression(expression.syntax())
            }
            Some(JavaKind::AssignmentExpression) => self.lower_assignment(node),
            Some(JavaKind::BinaryExpression) => self.lower_binary(node),
            Some(JavaKind::TernaryExpression) => self.lower_ternary(node),
            Some(JavaKind::UnaryExpression) => self.lower_unary(node, false),
            Some(JavaKind::UpdateExpression) => self.lower_unary(node, true),
            Some(JavaKind::ParenthesizedExpression) => self.lower_parenthesized(node),
            Some(JavaKind::ArrayAccess) => self.lower_array_access(node),
            Some(JavaKind::FieldAccess) => self.lower_field_access(node),
            Some(JavaKind::QualifiedSuperExpression) => self.lower_qualified_super(node),
            Some(JavaKind::MethodInvocation) => self.lower_method_invocation(node),
            Some(JavaKind::MethodReference) => self.lower_method_reference(node),
            Some(JavaKind::ClassLiteral) => self.lower_class_literal(node),
            Some(JavaKind::ObjectCreationExpression) => self.lower_new_class(node),
            Some(JavaKind::ArrayCreationExpression) => self.lower_new_array(node),
            Some(JavaKind::ArrayInitializer) => self.lower_array_initializer(node),
            Some(JavaKind::CastExpression) => self.lower_cast(node),
            Some(JavaKind::InstanceofExpression) => self.lower_instanceof(node),
            Some(JavaKind::LambdaExpression) => self.lower_lambda(node),
            Some(JavaKind::SwitchExpression) => self.lower_switch(node, true),
            Some(JavaKind::ThisExpression) => self.push_synthetic_identifier(
                "this",
                Some(node.from()),
                Some(node.to()),
                Some(node.range()),
            ),
            Some(JavaKind::SuperExpression) => self.push_synthetic_identifier(
                "super",
                Some(node.from()),
                Some(node.to()),
                Some(node.range()),
            ),
            Some(JavaKind::Identifier) => {
                let name = self.cooked_name(node)?;
                let identifier = self
                    .ast
                    .push_node(JavaAstKind::Identifier, node.range().into())?;
                self.ast.push_name(identifier, &name, Some(node.range()))?;
                Ok(identifier)
            }
            Some(JavaKind::IntegerLiteral) => self.lower_integer_literal(node, node.range()),
            Some(JavaKind::FloatingPointLiteral) => {
                let spelling = self.input.read_logical(node.range()).to_ascii_lowercase();
                self.ast.push_node(
                    if spelling.ends_with('f') {
                        JavaAstKind::FloatLiteral
                    } else {
                        JavaAstKind::DoubleLiteral
                    },
                    node.range().into(),
                )
            }
            Some(JavaKind::BooleanLiteral) => self
                .ast
                .push_node(JavaAstKind::BooleanLiteral, node.range().into()),
            Some(JavaKind::CharacterLiteral) => self
                .ast
                .push_node(JavaAstKind::CharLiteral, node.range().into()),
            Some(JavaKind::StringLiteral | JavaKind::TextBlock) => self
                .ast
                .push_node(JavaAstKind::StringLiteral, node.range().into()),
            Some(JavaKind::Null) => self
                .ast
                .push_node(JavaAstKind::NullLiteral, node.range().into()),
            _ => Err(AstError::InconsistentCst {
                context: "expression",
                expected: "implemented Java expression",
            }),
        }
    }

    fn lower_assignment(&mut self, node: &SyntaxNode) -> Result<AstNodeId, AstError> {
        let shape = typed_node::<crate::typed::JavaAssignmentExpression>(
            node,
            "AssignmentExpression",
            "assignment expression",
        )?;
        let operator = shape.operator().ok_or(AstError::InconsistentCst {
            context: "AssignmentExpression",
            expected: "assignment operator",
        })?;
        let spelling = self.input.read_logical(operator.syntax().range());
        let kind = assignment_kind(&spelling).ok_or(AstError::InconsistentCst {
            context: "AssignmentExpression",
            expected: "known assignment operator",
        })?;
        let assignment = self.ast.push_node(kind, node.range().into())?;
        let variable = shape.left().ok_or(AstError::InconsistentCst {
            context: "AssignmentExpression",
            expected: "left operand",
        })?;
        let expression = shape.right().ok_or(AstError::InconsistentCst {
            context: "AssignmentExpression",
            expected: "right operand",
        })?;
        let variable = self.lower_expression(variable.syntax())?;
        self.ast
            .push_edge(assignment, JavaAstField::Variable, variable)?;
        let expression = self.lower_expression(expression.syntax())?;
        self.ast
            .push_edge(assignment, JavaAstField::Expression, expression)?;
        Ok(assignment)
    }

    fn lower_binary(&mut self, node: &SyntaxNode) -> Result<AstNodeId, AstError> {
        let shape = typed_node::<crate::typed::JavaBinaryExpression>(
            node,
            "BinaryExpression",
            "binary expression",
        )?;
        let operator = shape.operator().ok_or(AstError::InconsistentCst {
            context: "BinaryExpression",
            expected: "binary operator",
        })?;
        let spelling = self.input.read_logical(operator.syntax().range());
        let kind = binary_kind(&spelling).ok_or(AstError::InconsistentCst {
            context: "BinaryExpression",
            expected: "known binary operator",
        })?;
        if kind == JavaAstKind::Plus {
            return self.lower_plus_expression(node);
        }
        let left = shape.left().ok_or(AstError::InconsistentCst {
            context: "BinaryExpression",
            expected: "left operand",
        })?;
        let right = shape.right().ok_or(AstError::InconsistentCst {
            context: "BinaryExpression",
            expected: "right operand",
        })?;
        let expression = self.ast.push_node(kind, node.range().into())?;
        let left = self.lower_expression(left.syntax())?;
        self.ast
            .push_edge(expression, JavaAstField::LeftOperand, left)?;
        let right = self.lower_expression(right.syntax())?;
        self.ast
            .push_edge(expression, JavaAstField::RightOperand, right)?;
        Ok(expression)
    }

    fn lower_plus_expression(&mut self, node: &SyntaxNode) -> Result<AstNodeId, AstError> {
        let mut syntax_operands = Vec::new();
        self.collect_plus_operands(node, &mut syntax_operands)?;
        let mut operands = Vec::new();
        for operand in syntax_operands {
            if matches!(
                kind(&operand),
                Some(JavaKind::StringLiteral | JavaKind::TextBlock)
            ) {
                if let Some(PlusOperand::StringLiteral(range)) = operands.last_mut() {
                    *range = TextRange::new(range.start(), operand.to());
                } else {
                    operands.push(PlusOperand::StringLiteral(operand.range()));
                }
            } else {
                operands.push(PlusOperand::Expression(operand));
            }
        }

        let mut operands = operands.into_iter();
        let first = operands.next().ok_or(AstError::InconsistentCst {
            context: "BinaryExpression",
            expected: "left operand",
        })?;
        let start = first.range().start();
        let mut expression = self.lower_plus_operand(&first)?;
        for operand in operands {
            let end = operand.range().end();
            let right = self.lower_plus_operand(&operand)?;
            let combined = self
                .ast
                .push_node(JavaAstKind::Plus, TextRange::new(start, end).into())?;
            self.ast
                .push_edge(combined, JavaAstField::LeftOperand, expression)?;
            self.ast
                .push_edge(combined, JavaAstField::RightOperand, right)?;
            expression = combined;
        }
        Ok(expression)
    }

    fn collect_plus_operands(
        &self,
        node: &SyntaxNode,
        operands: &mut Vec<SyntaxNode>,
    ) -> Result<(), AstError> {
        if kind(node) != Some(JavaKind::BinaryExpression) {
            operands.push(node.clone());
            return Ok(());
        }
        let shape = typed_node::<crate::typed::JavaBinaryExpression>(
            node,
            "BinaryExpression",
            "binary expression",
        )?;
        let operator = shape.operator().ok_or(AstError::InconsistentCst {
            context: "BinaryExpression",
            expected: "binary operator",
        })?;
        if self.input.read_logical(operator.syntax().range()) != "+" {
            operands.push(node.clone());
            return Ok(());
        }
        let left = shape.left().ok_or(AstError::InconsistentCst {
            context: "BinaryExpression",
            expected: "left operand",
        })?;
        let right = shape.right().ok_or(AstError::InconsistentCst {
            context: "BinaryExpression",
            expected: "right operand",
        })?;
        self.collect_plus_operands(left.syntax(), operands)?;
        self.collect_plus_operands(right.syntax(), operands)
    }

    fn lower_plus_operand(&mut self, operand: &PlusOperand) -> Result<AstNodeId, AstError> {
        match operand {
            PlusOperand::Expression(node) => self.lower_expression(node),
            PlusOperand::StringLiteral(range) => self
                .ast
                .push_node(JavaAstKind::StringLiteral, (*range).into()),
        }
    }

    fn lower_ternary(&mut self, node: &SyntaxNode) -> Result<AstNodeId, AstError> {
        let shape = typed_node::<crate::typed::JavaTernaryExpression>(
            node,
            "TernaryExpression",
            "conditional expression",
        )?;
        let expression = self
            .ast
            .push_node(JavaAstKind::ConditionalExpression, node.range().into())?;
        let condition = shape.condition().ok_or(AstError::InconsistentCst {
            context: "TernaryExpression",
            expected: "condition",
        })?;
        let true_expression = shape.true_expression().ok_or(AstError::InconsistentCst {
            context: "TernaryExpression",
            expected: "true expression",
        })?;
        let false_expression = shape.false_expression().ok_or(AstError::InconsistentCst {
            context: "TernaryExpression",
            expected: "false expression",
        })?;
        let condition = self.lower_expression(condition.syntax())?;
        self.ast
            .push_edge(expression, JavaAstField::Condition, condition)?;
        let true_expression = self.lower_expression(true_expression.syntax())?;
        self.ast
            .push_edge(expression, JavaAstField::TrueExpression, true_expression)?;
        let false_expression = self.lower_expression(false_expression.syntax())?;
        self.ast
            .push_edge(expression, JavaAstField::FalseExpression, false_expression)?;
        Ok(expression)
    }

    fn lower_unary(&mut self, node: &SyntaxNode, update: bool) -> Result<AstNodeId, AstError> {
        let (operator, operand) = if update {
            let shape = typed_node::<crate::typed::JavaUpdateExpression>(
                node,
                "UpdateExpression",
                "update expression",
            )?;
            let operator = shape.operator().ok_or(AstError::InconsistentCst {
                context: "update expression",
                expected: "update operator",
            })?;
            let operand = shape.expression().ok_or(AstError::InconsistentCst {
                context: "update expression",
                expected: "operand",
            })?;
            (operator.syntax().clone(), operand.into_syntax())
        } else {
            let shape = typed_node::<crate::typed::JavaUnaryExpression>(
                node,
                "UnaryExpression",
                "unary expression",
            )?;
            let operator = shape.operator().ok_or(AstError::InconsistentCst {
                context: "unary expression",
                expected: "unary operator",
            })?;
            let operand = shape.expression().ok_or(AstError::InconsistentCst {
                context: "unary expression",
                expected: "operand",
            })?;
            (operator.syntax().clone(), operand.into_syntax())
        };
        let spelling = self.input.read_logical(operator.range());
        let prefix = operator.from() < operand.from();
        if !update
            && prefix
            && spelling == "-"
            && kind(&operand) == Some(JavaKind::IntegerLiteral)
            && self.is_foldable_negative_integer(&operand)
        {
            return self.lower_integer_literal(&operand, node.range());
        }
        let kind = unary_kind(&spelling, prefix, update).ok_or(AstError::InconsistentCst {
            context: "unary expression",
            expected: "known unary operator",
        })?;
        let expression = self.ast.push_node(kind, node.range().into())?;
        let operand = self.lower_expression(&operand)?;
        self.ast
            .push_edge(expression, JavaAstField::Expression, operand)?;
        Ok(expression)
    }

    fn lower_integer_literal(
        &mut self,
        node: &SyntaxNode,
        range: TextRange,
    ) -> Result<AstNodeId, AstError> {
        let spelling = self.input.read_logical(node.range()).to_ascii_lowercase();
        self.ast.push_node(
            if spelling.ends_with('l') {
                JavaAstKind::LongLiteral
            } else {
                JavaAstKind::IntLiteral
            },
            range.into(),
        )
    }

    fn is_foldable_negative_integer(&self, node: &SyntaxNode) -> bool {
        let spelling = self.input.read_logical(node.range());
        spelling
            .bytes()
            .find(|byte| *byte != b'_')
            .is_some_and(|byte| byte.is_ascii_digit() && byte != b'0')
    }

    fn lower_parenthesized(&mut self, node: &SyntaxNode) -> Result<AstNodeId, AstError> {
        let shape = typed_node::<crate::typed::JavaParenthesizedExpression>(
            node,
            "ParenthesizedExpression",
            "parenthesized expression",
        )?;
        let parenthesized = self
            .ast
            .push_node(JavaAstKind::Parenthesized, node.range().into())?;
        let expression = shape.expression().ok_or(AstError::InconsistentCst {
            context: "ParenthesizedExpression",
            expected: "expression",
        })?;
        let expression = self.lower_expression_or_keyword(expression.syntax())?;
        self.ast
            .push_edge(parenthesized, JavaAstField::Expression, expression)?;
        Ok(parenthesized)
    }

    fn lower_array_access(&mut self, node: &SyntaxNode) -> Result<AstNodeId, AstError> {
        let shape =
            typed_node::<crate::typed::JavaArrayAccess>(node, "ArrayAccess", "array access")?;
        let access = self
            .ast
            .push_node(JavaAstKind::ArrayAccess, node.range().into())?;
        let expression = shape.array().ok_or(AstError::InconsistentCst {
            context: "ArrayAccess",
            expected: "array expression",
        })?;
        let index = shape.index().ok_or(AstError::InconsistentCst {
            context: "ArrayAccess",
            expected: "index expression",
        })?;
        let expression = self.lower_expression(expression.syntax())?;
        self.ast
            .push_edge(access, JavaAstField::Expression, expression)?;
        let index = self.lower_expression(index.syntax())?;
        self.ast.push_edge(access, JavaAstField::Index, index)?;
        Ok(access)
    }

    fn lower_field_access(&mut self, node: &SyntaxNode) -> Result<AstNodeId, AstError> {
        let shape =
            typed_node::<crate::typed::JavaFieldAccess>(node, "FieldAccess", "field access")?;
        let (name, selected_range) = match field_access_selection(&shape) {
            Some(FieldAccessSelection::Identifier(selected)) => (
                self.cooked_name(selected.syntax())?,
                selected.syntax().range(),
            ),
            Some(FieldAccessSelection::This(selected)) => {
                ("this".to_owned(), selected.syntax().range())
            }
            None => {
                return Err(AstError::InconsistentCst {
                    context: "FieldAccess",
                    expected: "selected identifier",
                });
            }
        };
        let access = self
            .ast
            .push_node(JavaAstKind::MemberSelect, node.range().into())?;
        self.ast.push_name(access, &name, Some(selected_range))?;
        let qualifier = shape.qualifier().ok_or(AstError::InconsistentCst {
            context: "FieldAccess",
            expected: "qualifier expression",
        })?;
        let qualifier = self.lower_expression_or_keyword(qualifier.syntax())?;
        self.ast
            .push_edge(access, JavaAstField::Expression, qualifier)?;
        Ok(access)
    }

    fn lower_qualified_super(&mut self, node: &SyntaxNode) -> Result<AstNodeId, AstError> {
        let shape = typed_node::<crate::typed::JavaQualifiedSuperExpression>(
            node,
            "QualifiedSuperExpression",
            "qualified super expression",
        )?;
        let selected = shape.super_expression().ok_or(AstError::InconsistentCst {
            context: "QualifiedSuperExpression",
            expected: "super",
        })?;
        let access = self
            .ast
            .push_node(JavaAstKind::MemberSelect, node.range().into())?;
        self.ast
            .push_name(access, "super", Some(selected.syntax().range()))?;
        let qualifier = shape.qualifier().ok_or(AstError::InconsistentCst {
            context: "QualifiedSuperExpression",
            expected: "qualifier expression",
        })?;
        let qualifier = self.lower_qualified_name(qualifier.syntax())?;
        self.ast
            .push_edge(access, JavaAstField::Expression, qualifier)?;
        Ok(access)
    }

    fn lower_method_invocation(&mut self, node: &SyntaxNode) -> Result<AstNodeId, AstError> {
        let shape = typed_node::<crate::typed::JavaMethodInvocation>(
            node,
            "MethodInvocation",
            "method invocation",
        )?;
        let invocation = self
            .ast
            .push_node(JavaAstKind::MethodInvocation, node.range().into())?;
        if let Some(arguments) = shape.type_arguments() {
            for argument in arguments.arguments() {
                let argument = self.lower_type(argument.syntax())?;
                self.ast
                    .push_edge(invocation, JavaAstField::TypeArguments, argument)?;
            }
        }
        let method_name = shape.name().ok_or(AstError::InconsistentCst {
            context: "MethodInvocation",
            expected: "MethodName",
        })?;
        let identifier = method_name.identifier().ok_or(AstError::InconsistentCst {
            context: "MethodName",
            expected: "Identifier",
        })?;
        let name = self.cooked_name(identifier.syntax())?;
        let qualifier = shape.qualifier();
        let select = if let Some(qualifier) = qualifier {
            let select = self.ast.push_node(
                JavaAstKind::MemberSelect,
                TextRange::new(qualifier.syntax().from(), identifier.syntax().to()).into(),
            )?;
            self.ast
                .push_name(select, &name, Some(identifier.syntax().range()))?;
            let qualifier = self.lower_expression_or_keyword(qualifier.syntax())?;
            self.ast
                .push_edge(select, JavaAstField::Expression, qualifier)?;
            select
        } else {
            let select = self
                .ast
                .push_node(JavaAstKind::Identifier, identifier.syntax().range().into())?;
            self.ast
                .push_name(select, &name, Some(identifier.syntax().range()))?;
            select
        };
        self.ast
            .push_edge(invocation, JavaAstField::MethodSelect, select)?;
        if let Some(arguments) = shape.arguments() {
            for argument in arguments.arguments() {
                let argument = self.lower_expression(argument.syntax())?;
                self.ast
                    .push_edge(invocation, JavaAstField::Arguments, argument)?;
            }
        }
        Ok(invocation)
    }

    fn lower_method_reference(&mut self, node: &SyntaxNode) -> Result<AstNodeId, AstError> {
        let shape = typed_node::<crate::typed::JavaMethodReference>(
            node,
            "MethodReference",
            "method reference",
        )?;
        let reference = self
            .ast
            .push_node(JavaAstKind::MemberReference, node.range().into())?;
        let (name, name_range, is_new) = if let Some(name) = shape.member_name() {
            let identifier = name.identifier().ok_or(AstError::InconsistentCst {
                context: "MethodReference",
                expected: "Identifier",
            })?;
            (
                self.cooked_name(identifier.syntax())?,
                identifier.syntax().range(),
                false,
            )
        } else if let Some(new_token) = shape.new_token() {
            ("<init>".to_owned(), new_token.range(), true)
        } else {
            return Err(AstError::InconsistentCst {
                context: "MethodReference",
                expected: "member name",
            });
        };
        self.ast.push_name(reference, &name, Some(name_range))?;
        self.ast.push_property(
            reference,
            JavaAstProperty::ReferenceMode(if is_new {
                JavaReferenceMode::New
            } else {
                JavaReferenceMode::Invoke
            }),
        )?;
        let qualifier = if let Some(qualifier) = shape.qualifier_type() {
            self.lower_type(qualifier.syntax())?
        } else if let Some(qualifier) = shape.qualifier_expression() {
            self.lower_expression_or_keyword(qualifier.syntax())?
        } else {
            return Err(AstError::InconsistentCst {
                context: "MethodReference",
                expected: "qualifier expression",
            });
        };
        self.ast
            .push_edge(reference, JavaAstField::QualifierExpression, qualifier)?;
        if let Some(arguments) = shape.type_arguments() {
            for argument in arguments.arguments() {
                let argument = self.lower_type(argument.syntax())?;
                self.ast
                    .push_edge(reference, JavaAstField::TypeArguments, argument)?;
            }
        }
        Ok(reference)
    }

    fn lower_class_literal(&mut self, node: &SyntaxNode) -> Result<AstNodeId, AstError> {
        let shape =
            typed_node::<crate::typed::JavaClassLiteral>(node, "ClassLiteral", "class literal")?;
        let literal = self
            .ast
            .push_node(JavaAstKind::MemberSelect, node.range().into())?;
        let class = shape.class_token().ok_or(AstError::InconsistentCst {
            context: "ClassLiteral",
            expected: "class token",
        })?;
        self.ast.push_name(literal, "class", Some(class.range()))?;
        let ty = if let Some(ty) = shape.type_() {
            ty.syntax().clone()
        } else if let Some(void) = shape.void_token() {
            void
        } else {
            return Err(AstError::InconsistentCst {
                context: "ClassLiteral",
                expected: "class-literal type",
            });
        };
        let ty = self.lower_type(&ty)?;
        self.ast.push_edge(literal, JavaAstField::Expression, ty)?;
        Ok(literal)
    }

    fn lower_new_class(&mut self, node: &SyntaxNode) -> Result<AstNodeId, AstError> {
        let shape = typed_node::<crate::typed::JavaObjectCreationExpression>(
            node,
            "ObjectCreationExpression",
            "object creation expression",
        )?;
        let expression = self
            .ast
            .push_node(JavaAstKind::NewClass, node.range().into())?;
        if let Some(enclosing) = shape.enclosing_expression() {
            let enclosing = self.lower_expression_or_keyword(enclosing.syntax())?;
            self.ast
                .push_edge(expression, JavaAstField::EnclosingExpression, enclosing)?;
        }
        let identifier = shape.type_().ok_or(AstError::InconsistentCst {
            context: "ObjectCreationExpression",
            expected: "constructed type",
        })?;
        let identifier = self.lower_created_type(identifier.syntax())?;
        self.ast
            .push_edge(expression, JavaAstField::Identifier, identifier)?;
        if let Some(arguments) = shape.type_arguments() {
            for argument in arguments.arguments() {
                let argument = self.lower_type(argument.syntax())?;
                self.ast
                    .push_edge(expression, JavaAstField::TypeArguments, argument)?;
            }
        }
        if let Some(arguments) = shape.arguments() {
            for argument in arguments.arguments() {
                let argument = self.lower_expression(argument.syntax())?;
                self.ast
                    .push_edge(expression, JavaAstField::Arguments, argument)?;
            }
        }
        if let Some(body) = shape.class_body() {
            let body_syntax = body.syntax();
            let declaration = self.ast.push_node(
                JavaAstKind::Class,
                JavaSourceRange::new(Some(body_syntax.from()), Some(body_syntax.to())),
            )?;
            self.ast.push_name(declaration, "", None)?;
            let modifiers = self
                .ast
                .push_node(JavaAstKind::Modifiers, JavaSourceRange::default())?;
            self.ast
                .push_edge(declaration, JavaAstField::Modifiers, modifiers)?;
            try_for_each_class_member(&body, |member| {
                self.lower_member(declaration, &member, "", &[])
            })?;
            self.ast
                .push_edge(expression, JavaAstField::ClassBody, declaration)?;
        }
        Ok(expression)
    }

    fn lower_new_array(&mut self, node: &SyntaxNode) -> Result<AstNodeId, AstError> {
        let shape = typed_node::<crate::typed::JavaArrayCreationExpression>(
            node,
            "ArrayCreationExpression",
            "array creation expression",
        )?;
        let expression = self
            .ast
            .push_node(JavaAstKind::NewArray, node.range().into())?;
        let base = shape.type_().ok_or(AstError::InconsistentCst {
            context: "ArrayCreationExpression",
            expected: "array element type",
        })?;
        let mut empty_dimensions = shape
            .dimensions()
            .map(TypedNode::into_syntax)
            .collect::<Vec<_>>();
        let dimension_expressions = shape.dimension_expressions().collect::<Vec<_>>();
        let initializer = shape.initializer();
        let has_dimension_expression = !dimension_expressions.is_empty();
        let has_initializer = initializer.is_some();
        if !has_dimension_expression && has_initializer && !empty_dimensions.is_empty() {
            empty_dimensions.remove(0);
        }
        let ty = if empty_dimensions.is_empty() {
            self.lower_created_type(base.syntax())?
        } else {
            let full_range = TextRange::new(
                base.syntax().from(),
                empty_dimensions
                    .last()
                    .map_or(base.syntax().to(), SyntaxNode::to),
            );
            self.lower_created_type_with_dimensions(base.syntax(), &empty_dimensions, full_range)?
        };
        self.ast.push_edge(expression, JavaAstField::Type, ty)?;
        for dimension in dimension_expressions {
            if let Some(value) = dimension.expression() {
                let value = self.lower_expression(value.syntax())?;
                self.ast
                    .push_edge(expression, JavaAstField::Dimensions, value)?;
            }
            for annotation in dimension.annotations() {
                let annotation = self.lower_annotation(annotation.syntax(), true)?;
                self.ast
                    .push_edge(expression, JavaAstField::DimAnnotations, annotation)?;
            }
        }
        if let Some(initializer) = initializer {
            self.lower_array_initializers(expression, initializer.syntax())?;
        }
        Ok(expression)
    }

    fn lower_array_initializer(&mut self, node: &SyntaxNode) -> Result<AstNodeId, AstError> {
        let expression = self
            .ast
            .push_node(JavaAstKind::NewArray, node.range().into())?;
        self.lower_array_initializers(expression, node)?;
        Ok(expression)
    }

    fn lower_array_initializers(
        &mut self,
        parent: AstNodeId,
        node: &SyntaxNode,
    ) -> Result<(), AstError> {
        let shape = typed_node::<crate::typed::JavaArrayInitializer>(
            node,
            "ArrayInitializer",
            "array initializer",
        )?;
        for initializer in shape.values() {
            let initializer = self.lower_expression(initializer.syntax())?;
            self.ast
                .push_edge(parent, JavaAstField::Initializers, initializer)?;
        }
        Ok(())
    }

    fn lower_cast(&mut self, node: &SyntaxNode) -> Result<AstNodeId, AstError> {
        let shape = typed_node::<crate::typed::JavaCastExpression>(
            node,
            "CastExpression",
            "cast expression",
        )?;
        let cast = self
            .ast
            .push_node(JavaAstKind::TypeCast, node.range().into())?;
        let mut types = shape.types();
        let first = types.next().ok_or(AstError::InconsistentCst {
            context: "CastExpression",
            expected: "cast type",
        })?;
        let ty = if types.clone().next().is_some() {
            let intersection = self.ast.push_node(
                JavaAstKind::IntersectionType,
                TextRange::new(
                    node.from(),
                    types
                        .clone()
                        .last()
                        .map_or(first.syntax().to(), |ty| ty.syntax().to()),
                )
                .into(),
            )?;
            let first = self.lower_type(first.syntax())?;
            self.ast
                .push_edge(intersection, JavaAstField::Bounds, first)?;
            for ty in types {
                let ty = self.lower_type(ty.syntax())?;
                self.ast.push_edge(intersection, JavaAstField::Bounds, ty)?;
            }
            intersection
        } else {
            self.lower_type(first.syntax())?
        };
        self.ast.push_edge(cast, JavaAstField::Type, ty)?;
        let operand = shape.expression().ok_or(AstError::InconsistentCst {
            context: "CastExpression",
            expected: "operand",
        })?;
        let operand = self.lower_expression(operand.syntax())?;
        self.ast
            .push_edge(cast, JavaAstField::Expression, operand)?;
        Ok(cast)
    }

    fn lower_instanceof(&mut self, node: &SyntaxNode) -> Result<AstNodeId, AstError> {
        let shape = typed_node::<crate::typed::JavaInstanceofExpression>(
            node,
            "InstanceofExpression",
            "instanceof expression",
        )?;
        let expression = self
            .ast
            .push_node(JavaAstKind::InstanceOf, node.range().into())?;
        let operand = shape.expression().ok_or(AstError::InconsistentCst {
            context: "InstanceofExpression",
            expected: "operand",
        })?;
        let operand = self.lower_expression(operand.syntax())?;
        self.ast
            .push_edge(expression, JavaAstField::Expression, operand)?;
        let modifiers = shape.modifiers();
        let ty = shape.type_().ok_or(AstError::InconsistentCst {
            context: "InstanceofExpression",
            expected: "tested type",
        })?;
        if let Some(pattern) = shape.pattern() {
            let pattern =
                self.lower_instanceof_pattern(&pattern, ty.syntax(), modifiers.as_ref())?;
            self.ast
                .push_edge(expression, JavaAstField::Pattern, pattern)?;
        } else {
            let ty = self.lower_type(ty.syntax())?;
            self.ast.push_edge(expression, JavaAstField::Type, ty)?;
        }
        Ok(expression)
    }

    fn lower_instanceof_pattern(
        &mut self,
        node: &crate::typed::JavaInstanceofPattern,
        ty: &SyntaxNode,
        modifiers: Option<&JavaPatternModifiers>,
    ) -> Result<AstNodeId, AstError> {
        let body = node.body().ok_or(AstError::InconsistentCst {
            context: "InstanceofPattern",
            expected: "pattern variable or record pattern",
        })?;
        match body {
            JavaPatternBody::Variable(variable) => {
                self.lower_binding_pattern(ty, &variable, modifiers)
            }
            JavaPatternBody::Record(body) => self.lower_record_pattern(ty, &body),
        }
    }

    fn lower_binding_pattern(
        &mut self,
        ty: &SyntaxNode,
        variable: &JavaPatternVariable,
        modifiers: Option<&JavaPatternModifiers>,
    ) -> Result<AstNodeId, AstError> {
        let start = modifiers.map_or(ty.from(), |node| node.syntax().from());
        let pattern = self.ast.push_node(
            JavaAstKind::BindingPattern,
            TextRange::new(start, variable.syntax().to()).into(),
        )?;
        let variable_ast = self.lower_pattern_variable(ty, variable, modifiers)?;
        self.ast
            .push_edge(pattern, JavaAstField::Variable, variable_ast)?;
        Ok(pattern)
    }

    fn lower_record_pattern(
        &mut self,
        ty: &SyntaxNode,
        body: &JavaRecordPatternBody,
    ) -> Result<AstNodeId, AstError> {
        let pattern = self.ast.push_node(
            JavaAstKind::DeconstructionPattern,
            TextRange::new(ty.from(), body.syntax().to()).into(),
        )?;
        let deconstructor = self.lower_type(ty)?;
        self.ast
            .push_edge(pattern, JavaAstField::Deconstructor, deconstructor)?;
        for component in body.components() {
            let component = self.lower_component_pattern(&component)?;
            self.ast
                .push_edge(pattern, JavaAstField::NestedPatterns, component)?;
        }
        Ok(pattern)
    }

    fn lower_component_pattern(
        &mut self,
        node: &crate::typed::JavaComponentPattern,
    ) -> Result<AstNodeId, AstError> {
        let pattern = node.pattern().ok_or(AstError::InconsistentCst {
            context: "ComponentPattern",
            expected: "Pattern or underscore",
        })?;
        match pattern {
            crate::typed::JavaComponentPatternBody::Pattern(pattern) => {
                self.lower_pattern(pattern.syntax())
            }
            crate::typed::JavaComponentPatternBody::Unnamed(unnamed) => {
                let position = unnamed.syntax().to();
                self.ast.push_node(
                    JavaAstKind::AnyPattern,
                    TextRange::new(position, position).into(),
                )
            }
        }
    }

    fn lower_pattern(&mut self, node: &SyntaxNode) -> Result<AstNodeId, AstError> {
        let shape = typed_node::<JavaPattern>(node, "Pattern", "typed pattern")?;
        let modifiers = shape.modifiers();
        let ty = shape.type_().ok_or(AstError::InconsistentCst {
            context: "Pattern",
            expected: "pattern type",
        })?;
        let body = shape.body().ok_or(AstError::InconsistentCst {
            context: "Pattern",
            expected: "pattern variable or record body",
        })?;
        match body {
            JavaPatternBody::Variable(variable) => {
                self.lower_binding_pattern(ty.syntax(), &variable, modifiers.as_ref())
            }
            JavaPatternBody::Record(body) => self.lower_record_pattern(ty.syntax(), &body),
        }
    }

    fn lower_pattern_variable(
        &mut self,
        ty: &SyntaxNode,
        variable: &JavaPatternVariable,
        modifier_source: Option<&JavaPatternModifiers>,
    ) -> Result<AstNodeId, AstError> {
        let name = variable.name().ok_or(AstError::InconsistentCst {
            context: "PatternVariable",
            expected: "Definition or UnnamedPattern",
        })?;
        let start = modifier_source.map_or(ty.from(), |node| node.syntax().from());
        let declaration = self.ast.push_node(
            JavaAstKind::Variable,
            TextRange::new(start, variable.syntax().to()).into(),
        )?;
        match name {
            JavaPatternName::Named(definition) => {
                let name = self.cooked_name(definition.syntax())?;
                self.ast
                    .push_name(declaration, &name, Some(definition.syntax().range()))?;
            }
            JavaPatternName::Unnamed(_) => self.ast.push_name(declaration, "", None)?,
        }
        let modifier_syntax = modifier_source.map(TypedNode::syntax);
        let modifier_values = Self::modifiers(modifier_syntax, &[]);
        for modifier in &modifier_values {
            self.ast
                .push_property(declaration, JavaAstProperty::Modifier(*modifier))?;
        }
        let annotated = crate::typed::JavaAnnotatedType::downcast_from(ty.clone()).ok();
        let first_annotation = annotated
            .as_ref()
            .and_then(|node| node.annotations().next());
        let last_annotation = annotated
            .as_ref()
            .and_then(|node| node.annotations().last());
        let modifier_range = modifier_source.map_or_else(
            || {
                first_annotation
                    .as_ref()
                    .zip(last_annotation.as_ref())
                    .map_or_else(JavaSourceRange::default, |(first, last)| {
                        TextRange::new(first.syntax().from(), last.syntax().to()).into()
                    })
            },
            |modifiers| modifiers.syntax().range().into(),
        );
        let modifiers = self.ast.push_node(JavaAstKind::Modifiers, modifier_range)?;
        for modifier in modifier_values {
            self.ast
                .push_property(modifiers, JavaAstProperty::Modifier(modifier))?;
        }
        if let Some(source) = modifier_source {
            self.lower_modifier_annotations(modifiers, source.syntax())?;
        }
        if let Some(annotated) = annotated.as_ref() {
            for annotation in annotated.annotations() {
                let annotation = self.lower_annotation(annotation.syntax(), false)?;
                self.ast
                    .push_edge(modifiers, JavaAstField::Annotations, annotation)?;
            }
        }
        self.ast
            .push_edge(declaration, JavaAstField::Modifiers, modifiers)?;
        if self.input.read_logical(ty.range()) != "var" {
            let type_node = if let Some(annotated) = annotated {
                annotated
                    .type_()
                    .ok_or(AstError::InconsistentCst {
                        context: "AnnotatedType pattern",
                        expected: "underlying type",
                    })?
                    .syntax()
                    .clone()
            } else {
                ty.clone()
            };
            let type_node = self.lower_type(&type_node)?;
            self.ast
                .push_edge(declaration, JavaAstField::Type, type_node)?;
        }
        Ok(declaration)
    }

    fn lower_lambda(&mut self, node: &SyntaxNode) -> Result<AstNodeId, AstError> {
        let shape = typed_node::<crate::typed::JavaLambdaExpression>(
            node,
            "LambdaExpression",
            "lambda expression",
        )?;
        let lambda = self
            .ast
            .push_node(JavaAstKind::LambdaExpression, node.range().into())?;
        let body_expression = shape.body_expression();
        let body_block = shape.body_block();
        let body = body_expression
            .as_ref()
            .map(TypedNode::syntax)
            .or_else(|| body_block.as_ref().map(TypedNode::syntax))
            .ok_or(AstError::InconsistentCst {
                context: "LambdaExpression",
                expected: "lambda body",
            })?;
        self.ast.push_property(
            lambda,
            JavaAstProperty::LambdaBodyKind(if kind(body) == Some(JavaKind::Block) {
                JavaLambdaBodyKind::Statement
            } else {
                JavaLambdaBodyKind::Expression
            }),
        )?;
        if let Some(parameters) = shape.parameters() {
            match parameters {
                crate::typed::JavaLambdaParameters::Formal(parameters) => {
                    for parameter in parameters.parameters() {
                        let parameter = self.lower_parameter(parameter.syntax())?;
                        self.ast
                            .push_edge(lambda, JavaAstField::Parameters, parameter)?;
                    }
                }
                crate::typed::JavaLambdaParameters::Inferred(parameters) => {
                    for definition in parameters.parameters() {
                        let parameter = self.lower_inferred_parameter(definition.syntax())?;
                        self.ast
                            .push_edge(lambda, JavaAstField::Parameters, parameter)?;
                    }
                }
                crate::typed::JavaLambdaParameters::Single(definition) => {
                    let parameter = self.lower_inferred_parameter(definition.syntax())?;
                    self.ast
                        .push_edge(lambda, JavaAstField::Parameters, parameter)?;
                }
            }
        }
        let body = if kind(body) == Some(JavaKind::Block) {
            self.lower_block(body, false)?
        } else {
            self.lower_expression(body)?
        };
        self.ast.push_edge(lambda, JavaAstField::Body, body)?;
        Ok(lambda)
    }

    fn lower_inferred_parameter(&mut self, definition: &SyntaxNode) -> Result<AstNodeId, AstError> {
        let parameter = self
            .ast
            .push_node(JavaAstKind::Variable, definition.range().into())?;
        let name = self.cooked_name(definition)?;
        self.ast
            .push_name(parameter, &name, Some(definition.range()))?;
        let modifiers = self
            .ast
            .push_node(JavaAstKind::Modifiers, JavaSourceRange::default())?;
        self.ast
            .push_edge(parameter, JavaAstField::Modifiers, modifiers)?;
        Ok(parameter)
    }

    fn lower_expression_or_keyword(&mut self, node: &SyntaxNode) -> Result<AstNodeId, AstError> {
        match kind(node) {
            Some(JavaKind::This | JavaKind::ThisExpression) => self.push_synthetic_identifier(
                "this",
                Some(node.from()),
                Some(node.to()),
                Some(node.range()),
            ),
            Some(JavaKind::Super | JavaKind::SuperExpression) => self.push_synthetic_identifier(
                "super",
                Some(node.from()),
                Some(node.to()),
                Some(node.range()),
            ),
            _ => self.lower_expression(node),
        }
    }

    fn lower_module(&mut self, node: &SyntaxNode) -> Result<AstNodeId, AstError> {
        let shape = typed_node::<JavaModuleDeclaration>(node, "ModuleDeclaration", "typed module")?;
        let module = self
            .ast
            .push_node(JavaAstKind::Module, node.range().into())?;
        self.ast.push_property(
            module,
            JavaAstProperty::ModuleKind(if shape.open_token().is_some() {
                JavaModuleKind::Open
            } else {
                JavaModuleKind::Strong
            }),
        )?;
        for annotation in shape.annotations() {
            let annotation = self.lower_annotation(annotation.syntax(), false)?;
            self.ast
                .push_edge(module, JavaAstField::Annotations, annotation)?;
        }
        let name = shape.name().ok_or(AstError::InconsistentCst {
            context: "ModuleDeclaration",
            expected: "module name",
        })?;
        let name = self.lower_qualified_name(name.syntax())?;
        self.ast.push_edge(module, JavaAstField::Name, name)?;
        let body = shape.body().ok_or(AstError::InconsistentCst {
            context: "ModuleDeclaration",
            expected: "module body",
        })?;
        for directive in body.directives() {
            let directive = self.lower_module_directive(&directive)?;
            self.ast
                .push_edge(module, JavaAstField::Directives, directive)?;
        }
        Ok(module)
    }

    fn lower_module_directive(
        &mut self,
        node: &JavaModuleDirective,
    ) -> Result<AstNodeId, AstError> {
        let (ast_kind, first_field, additional_field) = if node.requires_token().is_some() {
            (JavaAstKind::Requires, JavaAstField::ModuleName, None)
        } else if node.exports_token().is_some() {
            (
                JavaAstKind::Exports,
                JavaAstField::PackageName,
                Some(JavaAstField::ModuleNames),
            )
        } else if node.opens_token().is_some() {
            (
                JavaAstKind::Opens,
                JavaAstField::PackageName,
                Some(JavaAstField::ModuleNames),
            )
        } else if node.uses_token().is_some() {
            (JavaAstKind::Uses, JavaAstField::ServiceName, None)
        } else if node.provides_token().is_some() {
            (
                JavaAstKind::Provides,
                JavaAstField::ServiceName,
                Some(JavaAstField::ImplementationNames),
            )
        } else {
            return Err(AstError::InconsistentCst {
                context: "ModuleDirective",
                expected: "module directive keyword",
            });
        };
        let directive = self.ast.push_node(ast_kind, node.syntax().range().into())?;
        if ast_kind == JavaAstKind::Requires {
            self.ast.push_property(
                directive,
                JavaAstProperty::RequiresStatic(node.static_token().is_some()),
            )?;
            self.ast.push_property(
                directive,
                JavaAstProperty::RequiresTransitive(node.transitive_token().is_some()),
            )?;
        }
        let mut names = node.names();
        let first = names.next().ok_or(AstError::InconsistentCst {
            context: "ModuleDirective",
            expected: "directive name",
        })?;
        let first = self.lower_qualified_name(first.syntax())?;
        self.ast.push_edge(directive, first_field, first)?;
        if let Some(field) = additional_field {
            for name in names {
                let name = self.lower_qualified_name(name.syntax())?;
                self.ast.push_edge(directive, field, name)?;
            }
        } else if names.next().is_some() {
            return Err(AstError::InconsistentCst {
                context: "ModuleDirective",
                expected: "one directive name",
            });
        }
        Ok(directive)
    }

    fn lower_qualified_name(&mut self, node: &SyntaxNode) -> Result<AstNodeId, AstError> {
        let shape = typed_node::<JavaName>(node, "qualified name", "typed Java name")?;
        match shape {
            JavaName::Identifier(identifier) => {
                let syntax = identifier.syntax();
                let name = self.cooked_name(syntax)?;
                let identifier = self
                    .ast
                    .push_node(JavaAstKind::Identifier, syntax.range().into())?;
                self.ast
                    .push_name(identifier, &name, Some(syntax.range()))?;
                Ok(identifier)
            }
            JavaName::Scoped(scoped) => {
                let (left, right) =
                    scoped_name_parts(&scoped).ok_or(AstError::InconsistentCst {
                        context: "ScopedIdentifier",
                        expected: "qualifier and selected identifier",
                    })?;
                let name = self.cooked_name(right.syntax())?;
                let select = self
                    .ast
                    .push_node(JavaAstKind::MemberSelect, scoped.syntax().range().into())?;
                self.ast
                    .push_name(select, &name, Some(right.syntax().range()))?;
                let qualifier = self.lower_qualified_name(left.syntax())?;
                self.ast
                    .push_edge(select, JavaAstField::Expression, qualifier)?;
                Ok(select)
            }
        }
    }

    fn push_synthetic_identifier(
        &mut self,
        name: &str,
        start: Option<TextSize>,
        end: Option<TextSize>,
        name_range: Option<TextRange>,
    ) -> Result<AstNodeId, AstError> {
        let identifier = self
            .ast
            .push_node(JavaAstKind::Identifier, JavaSourceRange::new(start, end))?;
        self.ast.push_name(identifier, name, name_range)?;
        Ok(identifier)
    }

    fn modifiers(node: Option<&SyntaxNode>, implicit: &[JavaModifier]) -> Vec<JavaModifier> {
        let mut values = implicit.to_vec();
        if let Some(node) = node {
            for item in modifier_items(node) {
                let modifier = match item {
                    ModifierItem::Keyword(JavaKind::Abstract) => Some(JavaModifier::Abstract),
                    ModifierItem::Keyword(JavaKind::Default) => Some(JavaModifier::Default),
                    ModifierItem::Keyword(JavaKind::Final) => Some(JavaModifier::Final),
                    ModifierItem::Keyword(JavaKind::Native) => Some(JavaModifier::Native),
                    ModifierItem::Keyword(JavaKind::NonSealedModifier) => {
                        Some(JavaModifier::NonSealed)
                    }
                    ModifierItem::Keyword(JavaKind::Private) => Some(JavaModifier::Private),
                    ModifierItem::Keyword(JavaKind::Protected) => Some(JavaModifier::Protected),
                    ModifierItem::Keyword(JavaKind::Public) => Some(JavaModifier::Public),
                    ModifierItem::Keyword(JavaKind::Sealed) => Some(JavaModifier::Sealed),
                    ModifierItem::Keyword(JavaKind::Static) => Some(JavaModifier::Static),
                    ModifierItem::Keyword(JavaKind::Strictfp) => Some(JavaModifier::Strictfp),
                    ModifierItem::Keyword(JavaKind::Synchronized) => {
                        Some(JavaModifier::Synchronized)
                    }
                    ModifierItem::Keyword(JavaKind::Transient) => Some(JavaModifier::Transient),
                    ModifierItem::Keyword(JavaKind::Volatile) => Some(JavaModifier::Volatile),
                    _ => None,
                };
                if let Some(modifier) = modifier
                    && !values.contains(&modifier)
                {
                    values.push(modifier);
                }
            }
        }
        values.sort();
        values
    }

    fn lower_modifier_annotations(
        &mut self,
        parent: AstNodeId,
        modifiers: &SyntaxNode,
    ) -> Result<(), AstError> {
        for item in modifier_items(modifiers) {
            if let ModifierItem::Annotation(annotation) = item {
                let annotation = self.lower_annotation(annotation.syntax(), false)?;
                self.ast
                    .push_edge(parent, JavaAstField::Annotations, annotation)?;
            }
        }
        Ok(())
    }

    fn cooked_name(&self, node: &SyntaxNode) -> Result<String, AstError> {
        let spelling = self.input.read_logical(node.range());
        let name = canonical_name(&spelling);
        if name.is_empty() {
            return Err(AstError::InconsistentCst {
                context: "Java identifier",
                expected: "non-empty canonical name",
            });
        }
        if kind(node) == Some(JavaKind::Definition) && name == "_" {
            return Ok(String::new());
        }
        Ok(name)
    }
}

#[derive(Clone, Copy)]
enum CallableForm<'a> {
    Method,
    Constructor,
    CompactConstructor(&'a [SyntaxNode]),
}

enum PlusOperand {
    Expression(SyntaxNode),
    StringLiteral(TextRange),
}

impl PlusOperand {
    fn range(&self) -> TextRange {
        match self {
            Self::Expression(node) => node.range(),
            Self::StringLiteral(range) => *range,
        }
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum TypeContext {
    Class,
    Record,
    Interface,
    Annotation,
    Enum,
}

impl TypeContext {
    const fn ast_kind(self) -> JavaAstKind {
        match self {
            Self::Class => JavaAstKind::Class,
            Self::Record => JavaAstKind::Record,
            Self::Interface => JavaAstKind::Interface,
            Self::Annotation => JavaAstKind::AnnotationType,
            Self::Enum => JavaAstKind::Enum,
        }
    }
}

fn kind(node: &SyntaxNode) -> Option<JavaKind> {
    <JavaLanguage as SyntaxLanguage>::kind(node)
}

fn typed_node<T>(
    node: &SyntaxNode,
    context: &'static str,
    expected: &'static str,
) -> Result<T, AstError>
where
    T: TypedNode<Language = JavaLanguage>,
{
    T::downcast_from(node.clone()).map_err(|_| AstError::InconsistentCst { context, expected })
}

fn assignment_kind(operator: &str) -> Option<JavaAstKind> {
    Some(match operator {
        "=" => JavaAstKind::Assignment,
        "*=" => JavaAstKind::MultiplyAssignment,
        "/=" => JavaAstKind::DivideAssignment,
        "%=" => JavaAstKind::RemainderAssignment,
        "+=" => JavaAstKind::PlusAssignment,
        "-=" => JavaAstKind::MinusAssignment,
        "<<=" => JavaAstKind::LeftShiftAssignment,
        ">>=" => JavaAstKind::RightShiftAssignment,
        ">>>=" => JavaAstKind::UnsignedRightShiftAssignment,
        "&=" => JavaAstKind::AndAssignment,
        "^=" => JavaAstKind::XorAssignment,
        "|=" => JavaAstKind::OrAssignment,
        _ => return None,
    })
}

fn binary_kind(operator: &str) -> Option<JavaAstKind> {
    Some(match operator {
        "*" => JavaAstKind::Multiply,
        "/" => JavaAstKind::Divide,
        "%" => JavaAstKind::Remainder,
        "+" => JavaAstKind::Plus,
        "-" => JavaAstKind::Minus,
        "<<" => JavaAstKind::LeftShift,
        ">>" => JavaAstKind::RightShift,
        ">>>" => JavaAstKind::UnsignedRightShift,
        "<" => JavaAstKind::LessThan,
        ">" => JavaAstKind::GreaterThan,
        "<=" => JavaAstKind::LessThanEqual,
        ">=" => JavaAstKind::GreaterThanEqual,
        "==" => JavaAstKind::EqualTo,
        "!=" => JavaAstKind::NotEqualTo,
        "&" => JavaAstKind::And,
        "^" => JavaAstKind::Xor,
        "|" => JavaAstKind::Or,
        "&&" => JavaAstKind::ConditionalAnd,
        "||" => JavaAstKind::ConditionalOr,
        _ => return None,
    })
}

fn unary_kind(operator: &str, prefix: bool, update: bool) -> Option<JavaAstKind> {
    if update {
        return Some(match (operator, prefix) {
            ("++", true) => JavaAstKind::PrefixIncrement,
            ("--", true) => JavaAstKind::PrefixDecrement,
            ("++", false) => JavaAstKind::PostfixIncrement,
            ("--", false) => JavaAstKind::PostfixDecrement,
            _ => return None,
        });
    }
    Some(match operator {
        "+" => JavaAstKind::UnaryPlus,
        "-" => JavaAstKind::UnaryMinus,
        "~" => JavaAstKind::BitwiseComplement,
        "!" => JavaAstKind::LogicalComplement,
        _ => return None,
    })
}

fn collect_array_parts(
    array: &SyntaxNode,
    dimensions: &mut Vec<SyntaxNode>,
) -> Result<SyntaxNode, AstError> {
    let array = crate::typed::JavaArrayType::downcast_from(array.clone()).map_err(|_| {
        AstError::InconsistentCst {
            context: "ArrayType",
            expected: "typed array type",
        }
    })?;
    let base = array.type_().ok_or(AstError::InconsistentCst {
        context: "ArrayType",
        expected: "element type",
    })?;
    let base = match base {
        crate::typed::JavaType::Array(array) => collect_array_parts(array.syntax(), dimensions)?,
        base => base.into_syntax(),
    };
    dimensions.extend(array.dimensions().map(TypedNode::into_syntax));
    Ok(base)
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
