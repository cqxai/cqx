use std::collections::HashMap;

use rezel_common::{TextRange, TextSize};

macro_rules! javac_enum {
    (
        $(#[$meta:meta])*
        pub enum $name:ident {
            $($variant:ident => $value:literal),+ $(,)?
        }
    ) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
        #[non_exhaustive]
        pub enum $name {
            $($variant),+
        }

        impl $name {
            /// Public JDK compiler-tree spelling.
            #[must_use]
            pub const fn javac_name(self) -> &'static str {
                match self {
                    $(Self::$variant => $value),+
                }
            }
        }
    };
}

javac_enum! {
    /// Public JDK 26 `Tree.Kind` represented by a Java AST node.
    pub enum JavaAstKind {
        AnnotatedType => "ANNOTATED_TYPE",
        Annotation => "ANNOTATION",
        TypeAnnotation => "TYPE_ANNOTATION",
        ArrayAccess => "ARRAY_ACCESS",
        ArrayType => "ARRAY_TYPE",
        Assert => "ASSERT",
        Assignment => "ASSIGNMENT",
        CompilationUnit => "COMPILATION_UNIT",
        Package => "PACKAGE",
        Import => "IMPORT",
        Class => "CLASS",
        Interface => "INTERFACE",
        AnnotationType => "ANNOTATION_TYPE",
        Enum => "ENUM",
        Record => "RECORD",
        Modifiers => "MODIFIERS",
        TypeParameter => "TYPE_PARAMETER",
        ParameterizedType => "PARAMETERIZED_TYPE",
        Variable => "VARIABLE",
        Method => "METHOD",
        Block => "BLOCK",
        Break => "BREAK",
        Case => "CASE",
        Catch => "CATCH",
        ConditionalExpression => "CONDITIONAL_EXPRESSION",
        Continue => "CONTINUE",
        DoWhileLoop => "DO_WHILE_LOOP",
        EmptyStatement => "EMPTY_STATEMENT",
        EnhancedForLoop => "ENHANCED_FOR_LOOP",
        ExpressionStatement => "EXPRESSION_STATEMENT",
        ForLoop => "FOR_LOOP",
        If => "IF",
        InstanceOf => "INSTANCE_OF",
        LabeledStatement => "LABELED_STATEMENT",
        LambdaExpression => "LAMBDA_EXPRESSION",
        MemberReference => "MEMBER_REFERENCE",
        MethodInvocation => "METHOD_INVOCATION",
        NewArray => "NEW_ARRAY",
        NewClass => "NEW_CLASS",
        Parenthesized => "PARENTHESIZED",
        AnyPattern => "ANY_PATTERN",
        BindingPattern => "BINDING_PATTERN",
        DefaultCaseLabel => "DEFAULT_CASE_LABEL",
        ConstantCaseLabel => "CONSTANT_CASE_LABEL",
        PatternCaseLabel => "PATTERN_CASE_LABEL",
        DeconstructionPattern => "DECONSTRUCTION_PATTERN",
        Return => "RETURN",
        Switch => "SWITCH",
        SwitchExpression => "SWITCH_EXPRESSION",
        Synchronized => "SYNCHRONIZED",
        Throw => "THROW",
        Try => "TRY",
        UnionType => "UNION_TYPE",
        IntersectionType => "INTERSECTION_TYPE",
        TypeCast => "TYPE_CAST",
        WhileLoop => "WHILE_LOOP",
        Yield => "YIELD",
        PostfixIncrement => "POSTFIX_INCREMENT",
        PostfixDecrement => "POSTFIX_DECREMENT",
        PrefixIncrement => "PREFIX_INCREMENT",
        PrefixDecrement => "PREFIX_DECREMENT",
        UnaryPlus => "UNARY_PLUS",
        UnaryMinus => "UNARY_MINUS",
        BitwiseComplement => "BITWISE_COMPLEMENT",
        LogicalComplement => "LOGICAL_COMPLEMENT",
        Multiply => "MULTIPLY",
        Divide => "DIVIDE",
        Remainder => "REMAINDER",
        Plus => "PLUS",
        Minus => "MINUS",
        LeftShift => "LEFT_SHIFT",
        RightShift => "RIGHT_SHIFT",
        UnsignedRightShift => "UNSIGNED_RIGHT_SHIFT",
        LessThan => "LESS_THAN",
        GreaterThan => "GREATER_THAN",
        LessThanEqual => "LESS_THAN_EQUAL",
        GreaterThanEqual => "GREATER_THAN_EQUAL",
        EqualTo => "EQUAL_TO",
        NotEqualTo => "NOT_EQUAL_TO",
        And => "AND",
        Xor => "XOR",
        Or => "OR",
        ConditionalAnd => "CONDITIONAL_AND",
        ConditionalOr => "CONDITIONAL_OR",
        MultiplyAssignment => "MULTIPLY_ASSIGNMENT",
        DivideAssignment => "DIVIDE_ASSIGNMENT",
        RemainderAssignment => "REMAINDER_ASSIGNMENT",
        PlusAssignment => "PLUS_ASSIGNMENT",
        MinusAssignment => "MINUS_ASSIGNMENT",
        LeftShiftAssignment => "LEFT_SHIFT_ASSIGNMENT",
        RightShiftAssignment => "RIGHT_SHIFT_ASSIGNMENT",
        UnsignedRightShiftAssignment => "UNSIGNED_RIGHT_SHIFT_ASSIGNMENT",
        AndAssignment => "AND_ASSIGNMENT",
        XorAssignment => "XOR_ASSIGNMENT",
        OrAssignment => "OR_ASSIGNMENT",
        PrimitiveType => "PRIMITIVE_TYPE",
        Identifier => "IDENTIFIER",
        MemberSelect => "MEMBER_SELECT",
        UnboundedWildcard => "UNBOUNDED_WILDCARD",
        ExtendsWildcard => "EXTENDS_WILDCARD",
        SuperWildcard => "SUPER_WILDCARD",
        IntLiteral => "INT_LITERAL",
        LongLiteral => "LONG_LITERAL",
        FloatLiteral => "FLOAT_LITERAL",
        DoubleLiteral => "DOUBLE_LITERAL",
        BooleanLiteral => "BOOLEAN_LITERAL",
        CharLiteral => "CHAR_LITERAL",
        StringLiteral => "STRING_LITERAL",
        NullLiteral => "NULL_LITERAL",
        Module => "MODULE",
        Exports => "EXPORTS",
        Opens => "OPENS",
        Provides => "PROVIDES",
        Requires => "REQUIRES",
        Uses => "USES",
    }
}

javac_enum! {
    /// Named child role exposed by a public JDK compiler-tree getter.
    pub enum JavaAstField {
        AnnotationType => "annotationType",
        Annotations => "annotations",
        Arguments => "arguments",
        Block => "block",
        Bound => "bound",
        Cases => "cases",
        Catches => "catches",
        ClassBody => "classBody",
        Condition => "condition",
        ConstantExpression => "constantExpression",
        Deconstructor => "deconstructor",
        DefaultValue => "defaultValue",
        Detail => "detail",
        DimAnnotations => "dimAnnotations",
        Dimensions => "dimensions",
        Directives => "directives",
        ElseStatement => "elseStatement",
        EnclosingExpression => "enclosingExpression",
        ErrorTrees => "errorTrees",
        Expressions => "expressions",
        FalseExpression => "falseExpression",
        FinallyBlock => "finallyBlock",
        Guard => "guard",
        Identifier => "identifier",
        ImplementationNames => "implementationNames",
        Index => "index",
        Initializers => "initializers",
        Labels => "labels",
        LeftOperand => "leftOperand",
        RightOperand => "rightOperand",
        MethodSelect => "methodSelect",
        Module => "module",
        ModuleName => "moduleName",
        ModuleNames => "moduleNames",
        Name => "name",
        NameExpression => "nameExpression",
        NestedPatterns => "nestedPatterns",
        Package => "package",
        PackageName => "packageName",
        Imports => "imports",
        TypeDecls => "typeDecls",
        Modifiers => "modifiers",
        TypeParameters => "typeParameters",
        Bounds => "bounds",
        ExtendsClause => "extendsClause",
        ImplementsClause => "implementsClause",
        PermitsClause => "permitsClause",
        Members => "members",
        Type => "type",
        TypeArguments => "typeArguments",
        ReturnType => "returnType",
        Parameters => "parameters",
        Throws => "throws",
        Body => "body",
        Initializer => "initializer",
        Parameter => "parameter",
        Statements => "statements",
        Statement => "statement",
        ThenStatement => "thenStatement",
        TrueExpression => "trueExpression",
        TypeAlternatives => "typeAlternatives",
        UnderlyingType => "underlyingType",
        Update => "update",
        Value => "value",
        Variable => "variable",
        Pattern => "pattern",
        ReceiverParameter => "receiverParameter",
        Resources => "resources",
        ServiceName => "serviceName",
        QualifierExpression => "qualifierExpression",
        QualifiedIdentifier => "qualifiedIdentifier",
        Expression => "expression",
    }
}

javac_enum! {
    /// Java declaration modifier.
    pub enum JavaModifier {
        Abstract => "ABSTRACT",
        Default => "DEFAULT",
        Final => "FINAL",
        Native => "NATIVE",
        NonSealed => "NON_SEALED",
        Private => "PRIVATE",
        Protected => "PROTECTED",
        Public => "PUBLIC",
        Sealed => "SEALED",
        Static => "STATIC",
        Strictfp => "STRICTFP",
        Synchronized => "SYNCHRONIZED",
        Transient => "TRANSIENT",
        Volatile => "VOLATILE",
    }
}

javac_enum! {
    /// Primitive type property.
    pub enum JavaPrimitiveKind {
        Boolean => "BOOLEAN",
        Byte => "BYTE",
        Short => "SHORT",
        Int => "INT",
        Long => "LONG",
        Char => "CHAR",
        Float => "FLOAT",
        Double => "DOUBLE",
        Void => "VOID",
    }
}

javac_enum! {
    /// `CaseTree.CaseKind`.
    pub enum JavaCaseKind {
        Statement => "STATEMENT",
        Rule => "RULE",
    }
}

javac_enum! {
    /// `LambdaExpressionTree.BodyKind`.
    pub enum JavaLambdaBodyKind {
        Expression => "EXPRESSION",
        Statement => "STATEMENT",
    }
}

javac_enum! {
    /// `MemberReferenceTree.ReferenceMode`.
    pub enum JavaReferenceMode {
        Invoke => "INVOKE",
        New => "NEW",
    }
}

javac_enum! {
    /// `ModuleTree.ModuleKind`.
    pub enum JavaModuleKind {
        Open => "OPEN",
        Strong => "STRONG",
    }
}

/// Stable interned string identity.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct StringId(u32);

impl StringId {
    /// Numeric arena index.
    #[must_use]
    pub const fn index(self) -> usize {
        self.0 as usize
    }
}

/// Deterministic AST-owned string arena.
#[derive(Debug, Default)]
pub struct StringInterner {
    values: Vec<Box<str>>,
    ids: HashMap<Box<str>, StringId>,
}

impl StringInterner {
    pub(crate) fn intern(&mut self, value: &str) -> Result<StringId, AstError> {
        if let Some(id) = self.ids.get(value) {
            return Ok(*id);
        }
        let id = StringId(u32::try_from(self.values.len()).map_err(|_| AstError::IndexOverflow)?);
        let value: Box<str> = value.into();
        self.values
            .try_reserve(1)
            .map_err(|_| AstError::AllocationFailed)?;
        self.ids
            .try_reserve(1)
            .map_err(|_| AstError::AllocationFailed)?;
        self.values.push(value.clone());
        self.ids.insert(value, id);
        Ok(id)
    }

    /// Resolve one interned string.
    #[must_use]
    pub fn resolve(&self, id: StringId) -> Option<&str> {
        self.values.get(id.index()).map(AsRef::as_ref)
    }
}

/// Stable node identity in one [`JavaAst`].
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct AstNodeId(u32);

impl AstNodeId {
    fn try_from_index(index: usize) -> Result<Self, AstError> {
        Ok(Self(
            u32::try_from(index).map_err(|_| AstError::IndexOverflow)?,
        ))
    }

    /// Construct an identity for an existing arena index.
    #[must_use]
    pub fn from_index(index: usize) -> Option<Self> {
        u32::try_from(index).ok().map(Self)
    }

    /// Numeric arena index.
    #[must_use]
    pub const fn index(self) -> usize {
        self.0 as usize
    }
}

/// Independent optional javac source endpoints (`NOPOS` maps to `None`).
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct JavaSourceRange {
    start: Option<TextSize>,
    end: Option<TextSize>,
}

impl JavaSourceRange {
    /// Construct one source range.
    #[must_use]
    pub const fn new(start: Option<TextSize>, end: Option<TextSize>) -> Self {
        Self { start, end }
    }

    /// Start byte, or `None` for javac `NOPOS`.
    #[must_use]
    pub const fn start(self) -> Option<TextSize> {
        self.start
    }

    /// End byte, or `None` for javac `NOPOS`.
    #[must_use]
    pub const fn end(self) -> Option<TextSize> {
        self.end
    }

    /// Complete half-open range when both endpoints exist.
    #[must_use]
    pub const fn byte_range(self) -> Option<TextRange> {
        match (self.start, self.end) {
            (Some(from), Some(to)) => Some(TextRange::new(from, to)),
            _ => None,
        }
    }
}

impl From<TextRange> for JavaSourceRange {
    fn from(range: TextRange) -> Self {
        Self::new(Some(range.start()), Some(range.end()))
    }
}

/// Scalar metadata attached to one AST node.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[non_exhaustive]
pub enum JavaAstProperty {
    Name(StringId),
    NameRange(TextRange),
    Modifier(JavaModifier),
    ImportModule(bool),
    ImportStatic(bool),
    BlockStatic(bool),
    CaseKind(JavaCaseKind),
    LambdaBodyKind(JavaLambdaBodyKind),
    ReferenceMode(JavaReferenceMode),
    ModuleKind(JavaModuleKind),
    RequiresStatic(bool),
    RequiresTransitive(bool),
    PrimitiveKind(JavaPrimitiveKind),
}

/// One public javac-aligned AST node.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct JavaAstNode {
    kind: JavaAstKind,
    source_range: JavaSourceRange,
    edge_start: u32,
    edge_count: u32,
    property_start: u32,
    property_count: u32,
}

impl JavaAstNode {
    /// Node kind.
    #[must_use]
    pub const fn kind(&self) -> JavaAstKind {
        self.kind
    }

    /// Source range.
    #[must_use]
    pub const fn source_range(&self) -> JavaSourceRange {
        self.source_range
    }
}

/// One ordered public getter edge.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct JavaAstEdge {
    field: JavaAstField,
    node: AstNodeId,
}

impl JavaAstEdge {
    /// Getter field.
    #[must_use]
    pub const fn field(self) -> JavaAstField {
        self.field
    }

    /// Child node.
    #[must_use]
    pub const fn node(self) -> AstNodeId {
        self.node
    }
}

/// Complete strict-only Java AST.
#[derive(Debug)]
pub struct JavaAst {
    pub(crate) nodes: Vec<JavaAstNode>,
    pub(crate) edges: Vec<JavaAstEdge>,
    pub(crate) properties: Vec<JavaAstProperty>,
    pub(crate) strings: StringInterner,
    pub(crate) root: AstNodeId,
}

impl JavaAst {
    /// Root node.
    #[must_use]
    pub fn root(&self) -> &JavaAstNode {
        &self.nodes[self.root.index()]
    }

    /// Root identity.
    #[must_use]
    pub const fn root_id(&self) -> AstNodeId {
        self.root
    }

    /// All nodes in public `TreeScanner` preorder.
    #[must_use]
    pub fn nodes(&self) -> &[JavaAstNode] {
        &self.nodes
    }

    /// Look up a node.
    #[must_use]
    pub fn node(&self, id: AstNodeId) -> Option<&JavaAstNode> {
        self.nodes.get(id.index())
    }

    /// Ordered child edges for one node.
    #[must_use]
    pub fn edges(&self, id: AstNodeId) -> Option<&[JavaAstEdge]> {
        let node = self.node(id)?;
        let start = usize::try_from(node.edge_start).ok()?;
        let count = usize::try_from(node.edge_count).ok()?;
        self.edges.get(start..start.checked_add(count)?)
    }

    /// Scalar properties for one node.
    #[must_use]
    pub fn properties(&self, id: AstNodeId) -> Option<&[JavaAstProperty]> {
        let node = self.node(id)?;
        let start = usize::try_from(node.property_start).ok()?;
        let count = usize::try_from(node.property_count).ok()?;
        self.properties.get(start..start.checked_add(count)?)
    }

    /// Resolve an interned AST string.
    #[must_use]
    pub fn string(&self, id: StringId) -> Option<&str> {
        self.strings.resolve(id)
    }
}

/// Java AST lowering failure.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum AstError {
    RecoveryTree,
    SourceTooLarge,
    InconsistentCst {
        context: &'static str,
        expected: &'static str,
    },
    InvalidRange(TextRange),
    IndexOverflow,
    AllocationFailed,
}

impl std::fmt::Display for AstError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{self:?}")
    }
}

impl std::error::Error for AstError {}

pub(crate) struct AstBuilder {
    pub(crate) nodes: Vec<JavaAstNode>,
    pub(crate) edges: Vec<Vec<JavaAstEdge>>,
    pub(crate) properties: Vec<Vec<JavaAstProperty>>,
    pub(crate) strings: StringInterner,
}

impl AstBuilder {
    pub(crate) fn new() -> Self {
        Self {
            nodes: Vec::new(),
            edges: Vec::new(),
            properties: Vec::new(),
            strings: StringInterner::default(),
        }
    }

    pub(crate) fn push_node(
        &mut self,
        kind: JavaAstKind,
        source_range: JavaSourceRange,
    ) -> Result<AstNodeId, AstError> {
        let id = AstNodeId::try_from_index(self.nodes.len())?;
        self.nodes
            .try_reserve(1)
            .map_err(|_| AstError::AllocationFailed)?;
        self.edges
            .try_reserve(1)
            .map_err(|_| AstError::AllocationFailed)?;
        self.properties
            .try_reserve(1)
            .map_err(|_| AstError::AllocationFailed)?;
        self.nodes.push(JavaAstNode {
            kind,
            source_range,
            edge_start: 0,
            edge_count: 0,
            property_start: 0,
            property_count: 0,
        });
        self.edges.push(Vec::new());
        self.properties.push(Vec::new());
        Ok(id)
    }

    pub(crate) fn set_range(
        &mut self,
        node: AstNodeId,
        source_range: JavaSourceRange,
    ) -> Result<(), AstError> {
        self.nodes
            .get_mut(node.index())
            .ok_or(AstError::IndexOverflow)?
            .source_range = source_range;
        Ok(())
    }

    pub(crate) fn child_range_end(&self, node: AstNodeId) -> Option<TextSize> {
        self.edges
            .get(node.index())?
            .iter()
            .filter_map(|edge| {
                self.nodes
                    .get(edge.node.index())
                    .and_then(|node| node.source_range.end())
            })
            .max()
    }

    pub(crate) fn push_edge(
        &mut self,
        parent: AstNodeId,
        field: JavaAstField,
        node: AstNodeId,
    ) -> Result<(), AstError> {
        self.edges
            .get_mut(parent.index())
            .ok_or(AstError::IndexOverflow)?
            .push(JavaAstEdge { field, node });
        Ok(())
    }

    pub(crate) fn push_property(
        &mut self,
        node: AstNodeId,
        property: JavaAstProperty,
    ) -> Result<(), AstError> {
        self.properties
            .get_mut(node.index())
            .ok_or(AstError::IndexOverflow)?
            .push(property);
        Ok(())
    }

    pub(crate) fn push_name(
        &mut self,
        node: AstNodeId,
        name: &str,
        range: Option<TextRange>,
    ) -> Result<(), AstError> {
        let name = self.strings.intern(name)?;
        self.push_property(node, JavaAstProperty::Name(name))?;
        if let Some(range) = range {
            self.push_property(node, JavaAstProperty::NameRange(range))?;
        }
        Ok(())
    }

    pub(crate) fn finish(mut self, root: AstNodeId) -> Result<JavaAst, AstError> {
        let mut flat_edges = Vec::new();
        let mut flat_properties = Vec::new();
        for index in 0..self.nodes.len() {
            let edge_start =
                u32::try_from(flat_edges.len()).map_err(|_| AstError::IndexOverflow)?;
            let property_start =
                u32::try_from(flat_properties.len()).map_err(|_| AstError::IndexOverflow)?;
            let edges = &self.edges[index];
            let properties = &self.properties[index];
            let edge_count = u32::try_from(edges.len()).map_err(|_| AstError::IndexOverflow)?;
            let property_count =
                u32::try_from(properties.len()).map_err(|_| AstError::IndexOverflow)?;
            flat_edges.extend_from_slice(edges);
            flat_properties.extend_from_slice(properties);
            let node = &mut self.nodes[index];
            node.edge_start = edge_start;
            node.edge_count = edge_count;
            node.property_start = property_start;
            node.property_count = property_count;
        }
        Ok(JavaAst {
            nodes: self.nodes,
            edges: flat_edges,
            properties: flat_properties,
            strings: self.strings,
            root,
        })
    }
}
