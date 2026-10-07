use rezel_common::NodePropSource;

#[cfg(feature = "highlight")]
use rezel_highlight::TagSet;

pub(crate) fn kotlin_highlighting() -> NodePropSource {
    #[cfg(feature = "highlight")]
    {
        let tags = rezel_highlight::tags();
        rezel_highlight::style_tags([
            (
                "class interface object fun val var typealias",
                TagSet::from(tags.definition_keyword),
            ),
            ("package import", TagSet::from(tags.module_keyword)),
            (
                "if else when try catch finally for while do throw return break continue",
                TagSet::from(tags.control_keyword),
            ),
            ("BooleanLiteral", TagSet::from(tags.bool_)),
            ("NullLiteral", TagSet::from(tags.null)),
            ("IntegerLiteral RealLiteral", TagSet::from(tags.number)),
            ("CharacterLiteral", TagSet::from(tags.character)),
            ("StringLiteral", TagSet::from(tags.string)),
            ("Identifier", TagSet::from(tags.variable_name)),
            (
                "Definition",
                TagSet::from(tags.definition.apply(tags.variable_name)),
            ),
            ("TypeName", TagSet::from(tags.type_name)),
            ("LineComment", TagSet::from(tags.line_comment)),
            ("BlockComment", TagSet::from(tags.block_comment)),
            ("ArithOp", TagSet::from(tags.arithmetic_operator)),
            ("CompareOp", TagSet::from(tags.compare_operator)),
            ("LogicOp", TagSet::from(tags.logic_operator)),
            ("UpdateOp", TagSet::from(tags.update_operator)),
            ("( )", TagSet::from(tags.paren)),
            ("[ ]", TagSet::from(tags.square_bracket)),
            ("{ }", TagSet::from(tags.brace)),
        ])
        .expect("the Kotlin highlight selectors are valid")
    }

    #[cfg(not(feature = "highlight"))]
    {
        rezel_common::group_prop().source(|_| None)
    }
}
