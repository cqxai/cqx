use rezel_common::{SyntaxLanguage, TextRange, TextSize, TypedNode};

use crate::typed::{
    JavaAnnotationMemberDeclaration, JavaAnnotationNode, JavaAnnotationTypeBody,
    JavaAnnotationTypeDeclarationCore, JavaBlock, JavaClassBody, JavaClassDeclarationCore,
    JavaCompactConstructorDeclaration, JavaConstructorBody, JavaConstructorDeclaration,
    JavaDefinition, JavaDimension, JavaEnumBody, JavaEnumDeclarationCore, JavaFormalParameter,
    JavaFormalParameters, JavaInterfaceBody, JavaInterfaceDeclarationCore,
    JavaInterfaceMemberDeclaration, JavaInterfaceTypeList, JavaKind, JavaLambdaVarParameter,
    JavaLanguage, JavaLocalTypeCore, JavaLocalTypeDeclaration, JavaMemberDeclaration,
    JavaMethodDeclaration, JavaModifiers, JavaReceiverParameter, JavaRecordBody,
    JavaRecordDeclarationCore, JavaRecordHeader, JavaRecordMemberDeclaration, JavaSpreadParameter,
    JavaThrows, JavaTopLevelTypeDeclaration, JavaType, JavaTypeParameters, JavaVariableDeclarator,
};

pub(crate) struct DeclaratorEntry {
    pub(crate) declarator: JavaVariableDeclarator,
    pub(crate) range: TextRange,
}

pub(crate) fn declarator_entries(
    declaration: &rezel_common::SyntaxNode,
    final_end: TextSize,
) -> Vec<DeclaratorEntry> {
    let mut entries = Vec::new();
    let mut current = None;
    for child in declaration.children() {
        if let Ok(declarator) = JavaVariableDeclarator::downcast_from(child.clone()) {
            current = Some(declarator);
            continue;
        }
        if JavaLanguage::kind(&child) == Some(JavaKind::Comma)
            && let Some(declarator) = current.take()
        {
            entries.push(DeclaratorEntry {
                range: TextRange::new(declaration.from(), child.to()),
                declarator,
            });
        }
    }
    if let Some(declarator) = current {
        entries.push(DeclaratorEntry {
            range: TextRange::new(declaration.from(), final_end),
            declarator,
        });
    }
    entries
}

pub(crate) enum CallableBody {
    Block(JavaBlock),
    Constructor(JavaConstructorBody),
}

pub(crate) struct CallableShape {
    pub(crate) syntax: rezel_common::SyntaxNode,
    pub(crate) modifiers: Option<JavaModifiers>,
    pub(crate) type_parameters: Option<JavaTypeParameters>,
    pub(crate) return_type: Option<rezel_common::SyntaxNode>,
    pub(crate) dimensions: Vec<JavaDimension>,
    pub(crate) name: JavaDefinition,
    pub(crate) parameters: Option<JavaFormalParameters>,
    pub(crate) throws: Option<JavaThrows>,
    pub(crate) body: Option<CallableBody>,
}

pub(crate) fn callable_shape(declaration: &rezel_common::SyntaxNode) -> Option<CallableShape> {
    if let Ok(method) = JavaMethodDeclaration::downcast_from(declaration.clone()) {
        return Some(CallableShape {
            syntax: method.syntax().clone(),
            modifiers: method.modifiers(),
            type_parameters: method.type_parameters(),
            return_type: method
                .return_type()
                .map(TypedNode::into_syntax)
                .or_else(|| method.void_token()),
            dimensions: method.dimensions().collect(),
            name: method.name()?,
            parameters: method.parameters(),
            throws: method.throws(),
            body: method.body().map(CallableBody::Block),
        });
    }
    if let Ok(compact) = JavaCompactConstructorDeclaration::downcast_from(declaration.clone()) {
        return Some(CallableShape {
            syntax: compact.syntax().clone(),
            modifiers: compact.modifiers(),
            type_parameters: None,
            return_type: None,
            dimensions: Vec::new(),
            name: compact.name()?,
            parameters: None,
            throws: None,
            body: compact.body().map(CallableBody::Block),
        });
    }
    let constructor = JavaConstructorDeclaration::downcast_from(declaration.clone()).ok()?;
    Some(CallableShape {
        syntax: constructor.syntax().clone(),
        modifiers: constructor.modifiers(),
        type_parameters: constructor.type_parameters(),
        return_type: None,
        dimensions: Vec::new(),
        name: constructor.name()?,
        parameters: constructor.parameters(),
        throws: constructor.throws(),
        body: constructor.body().map(CallableBody::Constructor),
    })
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub(crate) enum ParameterKind {
    Formal,
    Receiver,
    Spread,
    Inferred,
}

pub(crate) struct ParameterShape {
    pub(crate) syntax: rezel_common::SyntaxNode,
    pub(crate) kind: ParameterKind,
    pub(crate) modifiers: Option<JavaModifiers>,
    pub(crate) leading_annotations: Vec<JavaAnnotationNode>,
    pub(crate) type_: Option<JavaType>,
    pub(crate) dimensions: Vec<JavaDimension>,
    pub(crate) name: rezel_common::SyntaxNode,
    pub(crate) qualifier: Option<rezel_common::SyntaxNode>,
}

pub(crate) fn parameter_shape(node: &rezel_common::SyntaxNode) -> Option<ParameterShape> {
    if let Ok(parameter) = JavaFormalParameter::downcast_from(node.clone()) {
        return Some(ParameterShape {
            syntax: parameter.syntax().clone(),
            kind: ParameterKind::Formal,
            modifiers: parameter.modifiers(),
            leading_annotations: Vec::new(),
            type_: parameter.type_(),
            dimensions: parameter.dimensions().collect(),
            name: parameter.name()?.into_syntax(),
            qualifier: None,
        });
    }
    if let Ok(parameter) = JavaReceiverParameter::downcast_from(node.clone()) {
        return Some(ParameterShape {
            syntax: parameter.syntax().clone(),
            kind: ParameterKind::Receiver,
            modifiers: None,
            leading_annotations: parameter.annotations().collect(),
            type_: parameter.type_(),
            dimensions: Vec::new(),
            name: parameter.this_token()?,
            qualifier: parameter.qualifier().map(TypedNode::into_syntax),
        });
    }
    if let Ok(parameter) = JavaSpreadParameter::downcast_from(node.clone()) {
        return Some(ParameterShape {
            syntax: parameter.syntax().clone(),
            kind: ParameterKind::Spread,
            modifiers: parameter.modifiers(),
            leading_annotations: Vec::new(),
            type_: parameter.type_(),
            dimensions: Vec::new(),
            name: parameter.name()?.into_syntax(),
            qualifier: None,
        });
    }
    let parameter = JavaLambdaVarParameter::downcast_from(node.clone()).ok()?;
    Some(ParameterShape {
        syntax: parameter.syntax().clone(),
        kind: ParameterKind::Inferred,
        modifiers: parameter.modifiers(),
        leading_annotations: Vec::new(),
        type_: None,
        dimensions: Vec::new(),
        name: parameter.name()?.into_syntax(),
        qualifier: None,
    })
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub(crate) enum TypeDeclarationKind {
    Class,
    Record,
    Interface,
    Annotation,
    Enum,
}

pub(crate) enum TypeMemberKind {
    Field,
    Constant,
    Method,
    Constructor,
    CompactConstructor,
    AnnotationElement,
    EnumConstant,
    NestedType,
    Initializer,
    StaticInitializer { block: Option<JavaBlock> },
}

pub(crate) struct TypeMember {
    pub(crate) syntax: rezel_common::SyntaxNode,
    pub(crate) kind: TypeMemberKind,
}

pub(crate) struct TypeDeclarationShape {
    pub(crate) syntax: rezel_common::SyntaxNode,
    pub(crate) core_start: TextSize,
    pub(crate) kind: TypeDeclarationKind,
    pub(crate) modifiers: Option<rezel_common::SyntaxNode>,
    pub(crate) name: rezel_common::SyntaxNode,
    pub(crate) type_parameters: Option<JavaTypeParameters>,
    pub(crate) superclass: Option<JavaType>,
    pub(crate) interfaces: Option<JavaInterfaceTypeList>,
    pub(crate) permits: Option<JavaInterfaceTypeList>,
    pub(crate) record_header: Option<JavaRecordHeader>,
    pub(crate) body: TypeBody,
}

pub(crate) enum TypeBody {
    Class(JavaClassBody),
    Record(JavaRecordBody),
    Interface(JavaInterfaceBody),
    Annotation(JavaAnnotationTypeBody),
    Enum(JavaEnumBody),
}

pub(crate) fn type_declaration_shape(
    declaration: &rezel_common::SyntaxNode,
) -> Option<TypeDeclarationShape> {
    let declaration = JavaTopLevelTypeDeclaration::downcast_from(declaration.clone()).ok()?;
    let (syntax, modifiers, core) = match declaration {
        JavaTopLevelTypeDeclaration::Class(declaration) => (
            declaration.syntax().clone(),
            declaration.modifiers().map(TypedNode::into_syntax),
            TypeCore::Class(declaration.declaration()?),
        ),
        JavaTopLevelTypeDeclaration::Record(declaration) => (
            declaration.syntax().clone(),
            declaration.modifiers().map(TypedNode::into_syntax),
            TypeCore::Record(declaration.declaration()?),
        ),
        JavaTopLevelTypeDeclaration::Interface(declaration) => (
            declaration.syntax().clone(),
            declaration.modifiers().map(TypedNode::into_syntax),
            TypeCore::Interface(declaration.declaration()?),
        ),
        JavaTopLevelTypeDeclaration::AnnotationType(declaration) => (
            declaration.syntax().clone(),
            declaration.modifiers().map(TypedNode::into_syntax),
            TypeCore::Annotation(declaration.declaration()?),
        ),
        JavaTopLevelTypeDeclaration::Enum(declaration) => (
            declaration.syntax().clone(),
            declaration.modifiers().map(TypedNode::into_syntax),
            TypeCore::Enum(declaration.declaration()?),
        ),
    };
    shape_from_core(syntax, modifiers, &core)
}

pub(crate) fn local_type_declaration_shape(
    declaration: &JavaLocalTypeDeclaration,
) -> Option<TypeDeclarationShape> {
    let syntax = declaration.syntax().clone();
    let modifiers = declaration.modifiers().map(TypedNode::into_syntax);
    let core = match declaration.declaration()? {
        JavaLocalTypeCore::Class(core) => TypeCore::Class(core),
        JavaLocalTypeCore::Record(core) => TypeCore::Record(core),
        JavaLocalTypeCore::Interface(core) => TypeCore::Interface(core),
        JavaLocalTypeCore::AnnotationType(core) => TypeCore::Annotation(core),
        JavaLocalTypeCore::Enum(core) => TypeCore::Enum(core),
    };
    shape_from_core(syntax, modifiers, &core)
}

enum TypeCore {
    Class(JavaClassDeclarationCore),
    Record(JavaRecordDeclarationCore),
    Interface(JavaInterfaceDeclarationCore),
    Annotation(JavaAnnotationTypeDeclarationCore),
    Enum(JavaEnumDeclarationCore),
}

fn shape_from_core(
    syntax: rezel_common::SyntaxNode,
    modifiers: Option<rezel_common::SyntaxNode>,
    core: &TypeCore,
) -> Option<TypeDeclarationShape> {
    Some(TypeDeclarationShape {
        syntax,
        core_start: core.syntax().from(),
        kind: core.kind(),
        modifiers,
        name: core.name()?,
        type_parameters: core.type_parameters(),
        superclass: core.superclass(),
        interfaces: core.interfaces(),
        permits: core.permits(),
        record_header: core.record_header(),
        body: core.body()?,
    })
}

impl TypeCore {
    fn syntax(&self) -> &rezel_common::SyntaxNode {
        match self {
            Self::Class(core) => core.syntax(),
            Self::Record(core) => core.syntax(),
            Self::Interface(core) => core.syntax(),
            Self::Annotation(core) => core.syntax(),
            Self::Enum(core) => core.syntax(),
        }
    }

    const fn kind(&self) -> TypeDeclarationKind {
        match self {
            Self::Class(_) => TypeDeclarationKind::Class,
            Self::Record(_) => TypeDeclarationKind::Record,
            Self::Interface(_) => TypeDeclarationKind::Interface,
            Self::Annotation(_) => TypeDeclarationKind::Annotation,
            Self::Enum(_) => TypeDeclarationKind::Enum,
        }
    }

    fn name(&self) -> Option<rezel_common::SyntaxNode> {
        match self {
            Self::Class(core) => core.name().map(TypedNode::into_syntax),
            Self::Record(core) => core.name().map(TypedNode::into_syntax),
            Self::Interface(core) => core.name().map(TypedNode::into_syntax),
            Self::Annotation(core) => core.name().map(TypedNode::into_syntax),
            Self::Enum(core) => core.name().map(TypedNode::into_syntax),
        }
    }

    fn type_parameters(&self) -> Option<JavaTypeParameters> {
        match self {
            Self::Class(core) => core.type_parameters(),
            Self::Record(core) => core.type_parameters(),
            Self::Interface(core) => core.type_parameters(),
            Self::Annotation(_) | Self::Enum(_) => None,
        }
    }

    fn superclass(&self) -> Option<JavaType> {
        match self {
            Self::Class(core) => core.superclass().and_then(|clause| clause.type_()),
            _ => None,
        }
    }

    fn interfaces(&self) -> Option<JavaInterfaceTypeList> {
        match self {
            Self::Class(core) => core.interfaces().and_then(|clause| clause.types()),
            Self::Record(core) => core.interfaces().and_then(|clause| clause.types()),
            Self::Interface(core) => core.extends().and_then(|clause| clause.types()),
            Self::Enum(core) => core.interfaces().and_then(|clause| clause.types()),
            Self::Annotation(_) => None,
        }
    }

    fn permits(&self) -> Option<JavaInterfaceTypeList> {
        match self {
            Self::Class(core) => core.permits().and_then(|clause| clause.types()),
            Self::Interface(core) => core.permits().and_then(|clause| clause.types()),
            _ => None,
        }
    }

    fn record_header(&self) -> Option<JavaRecordHeader> {
        match self {
            Self::Record(core) => core.header(),
            _ => None,
        }
    }

    fn body(&self) -> Option<TypeBody> {
        match self {
            Self::Class(core) => core.body().map(TypeBody::Class),
            Self::Record(core) => core.body().map(TypeBody::Record),
            Self::Interface(core) => core.body().map(TypeBody::Interface),
            Self::Annotation(core) => core.body().map(TypeBody::Annotation),
            Self::Enum(core) => core.body().map(TypeBody::Enum),
        }
    }
}

pub(crate) fn try_for_each_type_member<E>(
    body: &TypeBody,
    mut visit: impl FnMut(TypeMember) -> Result<(), E>,
) -> Result<(), E> {
    match body {
        TypeBody::Class(body) => {
            for member in body.members() {
                visit(class_member(member))?;
            }
        }
        TypeBody::Record(body) => {
            for member in body.members() {
                visit(record_member(member))?;
            }
        }
        TypeBody::Interface(body) => {
            for member in body.members() {
                visit(interface_member(member))?;
            }
        }
        TypeBody::Annotation(body) => {
            for member in body.members() {
                visit(annotation_member(member))?;
            }
        }
        TypeBody::Enum(body) => {
            for constant in body.constants() {
                visit(TypeMember {
                    syntax: constant.into_syntax(),
                    kind: TypeMemberKind::EnumConstant,
                })?;
            }
            if let Some(declarations) = body.declarations() {
                for member in declarations.members() {
                    visit(class_member(member))?;
                }
            }
        }
    }
    Ok(())
}

pub(crate) fn try_for_each_class_member<E>(
    body: &JavaClassBody,
    mut visit: impl FnMut(TypeMember) -> Result<(), E>,
) -> Result<(), E> {
    for member in body.members() {
        visit(class_member(member))?;
    }
    Ok(())
}

fn class_member(member: JavaMemberDeclaration) -> TypeMember {
    let (syntax, kind) = match member {
        JavaMemberDeclaration::Field(node) => (node.into_syntax(), TypeMemberKind::Field),
        JavaMemberDeclaration::Method(node) => (node.into_syntax(), TypeMemberKind::Method),
        JavaMemberDeclaration::Constructor(node) => {
            (node.into_syntax(), TypeMemberKind::Constructor)
        }
        JavaMemberDeclaration::StaticInitializer(node) => {
            let block = node.block();
            (
                node.into_syntax(),
                TypeMemberKind::StaticInitializer { block },
            )
        }
        JavaMemberDeclaration::Initializer(node) => {
            (node.into_syntax(), TypeMemberKind::Initializer)
        }
        JavaMemberDeclaration::Class(node) => (node.into_syntax(), TypeMemberKind::NestedType),
        JavaMemberDeclaration::Record(node) => (node.into_syntax(), TypeMemberKind::NestedType),
        JavaMemberDeclaration::Interface(node) => (node.into_syntax(), TypeMemberKind::NestedType),
        JavaMemberDeclaration::AnnotationType(node) => {
            (node.into_syntax(), TypeMemberKind::NestedType)
        }
        JavaMemberDeclaration::Enum(node) => (node.into_syntax(), TypeMemberKind::NestedType),
    };
    TypeMember { syntax, kind }
}

fn record_member(member: JavaRecordMemberDeclaration) -> TypeMember {
    if let JavaRecordMemberDeclaration::CompactConstructor(node) = member {
        return TypeMember {
            syntax: node.into_syntax(),
            kind: TypeMemberKind::CompactConstructor,
        };
    }
    let member = match member {
        JavaRecordMemberDeclaration::Field(node) => JavaMemberDeclaration::Field(node),
        JavaRecordMemberDeclaration::Method(node) => JavaMemberDeclaration::Method(node),
        JavaRecordMemberDeclaration::Constructor(node) => JavaMemberDeclaration::Constructor(node),
        JavaRecordMemberDeclaration::StaticInitializer(node) => {
            JavaMemberDeclaration::StaticInitializer(node)
        }
        JavaRecordMemberDeclaration::Initializer(node) => JavaMemberDeclaration::Initializer(node),
        JavaRecordMemberDeclaration::Class(node) => JavaMemberDeclaration::Class(node),
        JavaRecordMemberDeclaration::Record(node) => JavaMemberDeclaration::Record(node),
        JavaRecordMemberDeclaration::Interface(node) => JavaMemberDeclaration::Interface(node),
        JavaRecordMemberDeclaration::AnnotationType(node) => {
            JavaMemberDeclaration::AnnotationType(node)
        }
        JavaRecordMemberDeclaration::Enum(node) => JavaMemberDeclaration::Enum(node),
        JavaRecordMemberDeclaration::CompactConstructor(_) => unreachable!(),
    };
    class_member(member)
}

fn interface_member(member: JavaInterfaceMemberDeclaration) -> TypeMember {
    let (syntax, kind) = match member {
        JavaInterfaceMemberDeclaration::Constant(node) => {
            (node.into_syntax(), TypeMemberKind::Constant)
        }
        JavaInterfaceMemberDeclaration::Method(node) => {
            (node.into_syntax(), TypeMemberKind::Method)
        }
        JavaInterfaceMemberDeclaration::Class(node) => {
            (node.into_syntax(), TypeMemberKind::NestedType)
        }
        JavaInterfaceMemberDeclaration::Record(node) => {
            (node.into_syntax(), TypeMemberKind::NestedType)
        }
        JavaInterfaceMemberDeclaration::Interface(node) => {
            (node.into_syntax(), TypeMemberKind::NestedType)
        }
        JavaInterfaceMemberDeclaration::AnnotationType(node) => {
            (node.into_syntax(), TypeMemberKind::NestedType)
        }
        JavaInterfaceMemberDeclaration::Enum(node) => {
            (node.into_syntax(), TypeMemberKind::NestedType)
        }
    };
    TypeMember { syntax, kind }
}

fn annotation_member(member: JavaAnnotationMemberDeclaration) -> TypeMember {
    let (syntax, kind) = match member {
        JavaAnnotationMemberDeclaration::Element(node) => {
            (node.into_syntax(), TypeMemberKind::AnnotationElement)
        }
        JavaAnnotationMemberDeclaration::Constant(node) => {
            (node.into_syntax(), TypeMemberKind::Constant)
        }
        JavaAnnotationMemberDeclaration::Class(node) => {
            (node.into_syntax(), TypeMemberKind::NestedType)
        }
        JavaAnnotationMemberDeclaration::Record(node) => {
            (node.into_syntax(), TypeMemberKind::NestedType)
        }
        JavaAnnotationMemberDeclaration::Interface(node) => {
            (node.into_syntax(), TypeMemberKind::NestedType)
        }
        JavaAnnotationMemberDeclaration::AnnotationType(node) => {
            (node.into_syntax(), TypeMemberKind::NestedType)
        }
        JavaAnnotationMemberDeclaration::Enum(node) => {
            (node.into_syntax(), TypeMemberKind::NestedType)
        }
    };
    TypeMember { syntax, kind }
}
