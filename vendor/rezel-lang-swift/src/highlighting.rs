use rezel_common::NodePropSource;

#[cfg(feature = "highlight")]
use rezel_highlight::{StandardTags, TagSet};

pub(crate) fn swift_highlighting() -> NodePropSource {
    #[cfg(feature = "highlight")]
    {
        let rules = swift_highlight_rules(rezel_highlight::tags());
        rezel_highlight::style_tags(rules).expect("the Swift highlight selectors are valid")
    }

    #[cfg(not(feature = "highlight"))]
    {
        rezel_common::group_prop().source(|_| None)
    }
}

#[cfg(feature = "highlight")]
fn swift_highlight_rules(tags: &'static StandardTags) -> Vec<(&'static str, TagSet)> {
    let mut rules = keyword_rules(tags);
    rules.extend(syntax_rules(tags));
    rules.extend(punctuation_rules(tags));
    rules
}

#[cfg(feature = "highlight")]
fn keyword_rules(tags: &StandardTags) -> Vec<(&'static str, TagSet)> {
    vec![
        (
            "actor associatedtype class deinit enum extension func init let macro operator precedencegroup protocol struct subscript typealias using var",
            TagSet::from(tags.definition_keyword),
        ),
        ("import", TagSet::from(tags.module_keyword)),
        (
            "break case catch continue default defer discard do else fallthrough for guard if in repeat return switch then throw while yield",
            TagSet::from(tags.control_keyword),
        ),
        ("as is", TagSet::from(tags.operator_keyword)),
        ("any each some where", TagSet::from(tags.keyword)),
        (
            "async autoclosure borrowing convenience consuming distributed dynamic escaping fileprivate final indirect infix internal isolated lazy mutating nonisolated nonmutating open optional override package postfix prefix private public reasync required rethrows sending static throws unowned weak __consuming __owned __shared _const _local",
            TagSet::from(tags.modifier),
        ),
        ("await try", TagSet::from(tags.control_keyword)),
        ("self super", TagSet::from(tags.self_)),
        ("Self Any", TagSet::from(tags.type_name)),
        ("nil", TagSet::from(tags.null)),
        ("BooleanLiteral", TagSet::from(tags.bool_)),
        ("IntegerLiteral", TagSet::from(tags.integer)),
        ("FloatLiteral", TagSet::from(tags.float)),
        ("StringLiteral", TagSet::from(tags.string)),
        ("RegexLiteral", TagSet::from(tags.regexp)),
        ("LineComment", TagSet::from(tags.line_comment)),
        ("BlockComment", TagSet::from(tags.block_comment)),
        ("Shebang", TagSet::from(tags.meta)),
        (
            "AttributeName AttributeNameWithArguments",
            TagSet::from(tags.annotation),
        ),
        (
            "PoundIf PoundElseif PoundElse PoundEndif #available #unavailable #sourceLocation",
            TagSet::from(tags.meta),
        ),
    ]
}

#[cfg(feature = "highlight")]
fn syntax_rules(tags: &StandardTags) -> Vec<(&'static str, TagSet)> {
    vec![
        ("Identifier", TagSet::from(tags.variable_name)),
        (
            "DollarIdentifier _",
            TagSet::from(tags.special.apply(tags.variable_name)),
        ),
        (
            "IdentifierPattern! ParameterNames! ClosureShorthandParameter!",
            TagSet::from(tags.definition.apply(tags.variable_name)),
        ),
        (
            "MemberBlock/VariableDeclaration/PatternBinding/IdentifierPattern!",
            TagSet::from(tags.definition.apply(tags.property_name)),
        ),
        (
            "FunctionName!",
            TagSet::from(
                tags.function
                    .apply(tags.definition.apply(tags.variable_name)),
            ),
        ),
        (
            "FunctionCallExpression/DeclReferenceExpression/Identifier FunctionCallExpression/GenericSpecializationExpression/DeclReferenceExpression/Identifier",
            TagSet::from(tags.function.apply(tags.variable_name)),
        ),
        (
            "TypeName!",
            TagSet::from(tags.definition.apply(tags.type_name)),
        ),
        (
            "MacroDeclaration/TypeName!",
            TagSet::from(tags.definition.apply(tags.macro_name)),
        ),
        ("TypeIdentifierName!", TagSet::from(tags.type_name)),
        ("ImportPathComponent!", TagSet::from(tags.namespace)),
        ("ModuleSelector/Identifier", TagSet::from(tags.namespace)),
        (
            "EnumCaseElement/Identifier",
            TagSet::from(tags.definition.apply(tags.property_name)),
        ),
        (
            "ArgumentLabel! DeclNameArgument/Identifier",
            TagSet::from(tags.label_name),
        ),
        (
            "LabeledStatement/Identifier BreakStatement/Identifier ContinueStatement/Identifier",
            TagSet::from(tags.label_name),
        ),
        (
            "ImplicitMemberExpression/DeclReferenceExpression/Identifier",
            TagSet::from(tags.property_name),
        ),
        (
            "MacroExpansionExpression/Identifier MacroExpansionDeclaration/Identifier",
            TagSet::from(tags.macro_name),
        ),
        ("DeclarationModifier!", TagSet::from(tags.modifier)),
        ("FunctionEffect!", TagSet::from(tags.modifier)),
        (
            "AccessorSpecifier! ClosureCaptureSpecifier! SimpleTypeSpecifier! NonisolatedTypeSpecifier! LifetimeTypeSpecifier!",
            TagSet::from(tags.modifier),
        ),
    ]
}

#[cfg(feature = "highlight")]
fn punctuation_rules(tags: &StandardTags) -> Vec<(&'static str, TagSet)> {
    vec![
        ("+ - \"*\" \"/\" %", TagSet::from(tags.arithmetic_operator)),
        ("== \"!=\" < <= > >=", TagSet::from(tags.compare_operator)),
        ("&& || ?? \"!\"", TagSet::from(tags.logic_operator)),
        (
            "& ~ prefixTilde prefixAmpersand binaryAmpersand",
            TagSet::from(tags.bitwise_operator),
        ),
        (
            "prefixRangeOperator binaryRangeOperator postfixRangeOperator prefixCustomOperator binaryCustomOperator postfixCustomOperator customOperator functionCustomOperator",
            TagSet::from(tags.operator),
        ),
        ("=", TagSet::from(tags.definition_operator)),
        ("->", TagSet::from(tags.type_operator)),
        (".", TagSet::from(tags.deref_operator)),
        (
            "GenericParameterClause/< GenericParameterClause/> GenericArgumentClause/< GenericArgumentClause/> PrimaryAssociatedTypeClause/< PrimaryAssociatedTypeClause/>",
            TagSet::from(tags.angle_bracket),
        ),
        ("( )", TagSet::from(tags.paren)),
        ("[ ]", TagSet::from(tags.square_bracket)),
        ("{ }", TagSet::from(tags.brace)),
        (", : ; ::", TagSet::from(tags.separator)),
        ("... # @", TagSet::from(tags.punctuation)),
    ]
}
