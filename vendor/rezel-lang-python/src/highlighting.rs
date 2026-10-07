use rezel_common::NodePropSource;

#[cfg(feature = "highlight")]
use rezel_highlight::TagSet;

pub(crate) fn python_highlighting() -> NodePropSource {
    #[cfg(feature = "highlight")]
    {
        let tags = rezel_highlight::tags();
        rezel_highlight::style_tags([
            (
                "async \"*\" \"**\" FormatConversion FormatSpec",
                TagSet::from(tags.modifier),
            ),
            (
                "for while if elif else try except finally return raise break continue with pass assert await yield match case",
                TagSet::from(tags.control_keyword),
            ),
            ("in not and or is del", TagSet::from(tags.operator_keyword)),
            (
                "from def class global nonlocal lambda type",
                TagSet::from(tags.definition_keyword),
            ),
            ("import", TagSet::from(tags.module_keyword)),
            ("with as print", TagSet::from(tags.keyword)),
            ("Boolean", TagSet::from(tags.bool_)),
            ("None", TagSet::from(tags.null)),
            ("VariableName", TagSet::from(tags.variable_name)),
            (
                "FunctionDefinition/VariableName",
                TagSet::from(tags.function.apply(tags.definition.apply(tags.variable_name))),
            ),
            (
                "ClassDefinition/VariableName",
                TagSet::from(tags.definition.apply(tags.class_name)),
            ),
            ("PropertyName", TagSet::from(tags.property_name)),
            ("Comment", TagSet::from(tags.line_comment)),
            ("Number", TagSet::from(tags.number)),
            ("String", TagSet::from(tags.string)),
            (
                "FormatString TemplateString",
                TagSet::from(tags.special.apply(tags.string)),
            ),
            ("Escape", TagSet::from(tags.escape)),
            ("UpdateOp", TagSet::from(tags.update_operator)),
            ("ArithOp", TagSet::from(tags.arithmetic_operator)),
            ("BitOp", TagSet::from(tags.bitwise_operator)),
            ("CompareOp", TagSet::from(tags.compare_operator)),
            ("AssignOp", TagSet::from(tags.definition_operator)),
            ("Ellipsis", TagSet::from(tags.punctuation)),
            ("At", TagSet::from(tags.meta)),
            ("( )", TagSet::from(tags.paren)),
            ("[ ]", TagSet::from(tags.square_bracket)),
            ("{ }", TagSet::from(tags.brace)),
            (".", TagSet::from(tags.deref_operator)),
            (", ;", TagSet::from(tags.separator)),
        ])
        .expect("Python highlight selectors are valid")
    }
    #[cfg(not(feature = "highlight"))]
    {
        rezel_common::group_prop().source(|_| None)
    }
}
