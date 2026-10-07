use rezel_common::NodePropSource;

#[cfg(feature = "highlight")]
use rezel_highlight::TagSet;

pub(crate) fn java_highlighting() -> NodePropSource {
    #[cfg(feature = "highlight")]
    {
        let tags = rezel_highlight::tags();
        rezel_highlight::style_tags([
            ("null", TagSet::from(tags.null)),
            ("instanceof", TagSet::from(tags.operator_keyword)),
            ("this", TagSet::from(tags.self_)),
            ("new super assert open to with void", TagSet::from(tags.keyword)),
            (
                "class interface extends implements enum var record",
                TagSet::from(tags.definition_keyword),
            ),
            ("module package import", TagSet::from(tags.module_keyword)),
            (
                "switch while for if else case default do break continue return try catch finally throw yield when",
                TagSet::from(tags.control_keyword),
            ),
            (
                "requires exports opens uses provides public private protected static transitive abstract final strictfp synchronized native transient volatile throws sealed non permits",
                TagSet::from(tags.modifier),
            ),
            ("IntegerLiteral", TagSet::from(tags.integer)),
            ("FloatingPointLiteral", TagSet::from(tags.float)),
            ("StringLiteral TextBlock", TagSet::from(tags.string)),
            ("CharacterLiteral", TagSet::from(tags.character)),
            ("LineComment", TagSet::from(tags.line_comment)),
            ("BlockComment", TagSet::from(tags.block_comment)),
            ("BooleanLiteral", TagSet::from(tags.bool_)),
            (
                "PrimitiveType",
                TagSet::from(tags.standard.apply(tags.type_name)),
            ),
            ("TypeName", TagSet::from(tags.type_name)),
            ("Identifier", TagSet::from(tags.variable_name)),
            (
                "MethodName/Identifier",
                TagSet::from(tags.function.apply(tags.variable_name)),
            ),
            (
                "Definition",
                TagSet::from(tags.definition.apply(tags.variable_name)),
            ),
            ("ArithOp", TagSet::from(tags.arithmetic_operator)),
            ("LogicOp", TagSet::from(tags.logic_operator)),
            ("BitOp", TagSet::from(tags.bitwise_operator)),
            ("CompareOp", TagSet::from(tags.compare_operator)),
            ("AssignOp", TagSet::from(tags.definition_operator)),
            ("UpdateOp", TagSet::from(tags.update_operator)),
            ("Asterisk", TagSet::from(tags.punctuation)),
            ("Label", TagSet::from(tags.label_name)),
            ("( )", TagSet::from(tags.paren)),
            ("[ ]", TagSet::from(tags.square_bracket)),
            ("{ }", TagSet::from(tags.brace)),
            (".", TagSet::from(tags.deref_operator)),
            (", ;", TagSet::from(tags.separator)),
            (
                "_",
                TagSet::from(tags.special.apply(tags.variable_name)),
            ),
        ])
        .expect("Java highlight selectors are valid")
    }
    #[cfg(not(feature = "highlight"))]
    {
        rezel_common::group_prop().source(|_| None)
    }
}
