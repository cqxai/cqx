use super::{
    PythonComprehensionHead, PythonExpressionItem, PythonInterpolatedLiteralKind,
    PythonInterpolatedPart, PythonInterpolatedStringKind, PythonMemberSuffix,
    PythonSequencePatternView, PythonSubscriptItem, PythonTypeParameterKind, format_spec_parts,
};
use crate::{
    PythonExpression, PythonExpressionNode, PythonModule, PythonPatternNode, PythonStatementNode,
};
use rezel_common::TypedNode;

#[test]
fn typed_syntax_exposes_statement_and_expression_roles() {
    let source = "answer = (item := await source)\nif answer:\n    pass\n";
    let tree = crate::parser().with_strict(true).parse(source).unwrap();
    let module = PythonModule::downcast_from(tree.top_node()).unwrap();
    let statements = module.statements().collect::<Vec<_>>();
    assert_eq!(statements.len(), 2);

    let PythonStatementNode::Assign(assign) = &statements[0] else {
        panic!("first statement should be an assignment");
    };
    let expressions = assign.expressions().collect::<Vec<_>>();
    assert_eq!(expressions.len(), 2);
    let PythonExpressionNode::Parenthesized(parenthesized) = &expressions[1] else {
        panic!("assignment value should be parenthesized");
    };
    let Some(PythonExpressionNode::Named(named)) = parenthesized.value() else {
        panic!("parenthesized value should be a named expression");
    };
    assert!(named.target().is_some());
    assert!(matches!(
        named.value(),
        Some(PythonExpressionNode::Await(_))
    ));
    assert!(matches!(statements[1], PythonStatementNode::If(_)));
}

#[test]
fn binary_expression_fields_are_grammar_checked() {
    let tree = crate::parser()
        .with_top("Expression")
        .unwrap()
        .with_strict(true)
        .parse("left + right")
        .unwrap();
    let expression = PythonExpression::downcast_from(tree.top_node()).unwrap();
    let Some(PythonExpressionNode::Binary(binary)) = expression.body() else {
        panic!("expected a binary expression");
    };
    assert!(matches!(
        binary.left(),
        Some(PythonExpressionNode::VariableName(_))
    ));
    assert!(matches!(
        binary.right(),
        Some(PythonExpressionNode::VariableName(_))
    ));
}

#[test]
fn interpolated_string_view_preserves_source_segments_and_nested_replacements() {
    let source = "rf\"{{x}} {value=} {value:{width}.2f}\"";
    let tree = crate::parser()
        .with_top("Expression")
        .unwrap()
        .with_strict(true)
        .parse(source)
        .unwrap();
    let expression = PythonExpression::downcast_from(tree.top_node()).unwrap();
    let Some(PythonExpressionNode::FormatString(string)) = expression.body() else {
        panic!("expected a format string");
    };
    let string = string.interpolated(source).unwrap();
    assert_eq!(string.kind(), PythonInterpolatedStringKind::Format);
    assert!(string.is_raw());
    assert!(matches!(
        string.parts()[0],
        PythonInterpolatedPart::Literal {
            kind: PythonInterpolatedLiteralKind::Escaped,
            ..
        }
    ));
    assert!(string.parts().iter().any(|part| matches!(
        part,
        PythonInterpolatedPart::Literal {
            kind: PythonInterpolatedLiteralKind::Debug,
            ..
        }
    )));
    let interpolation = string
        .parts()
        .iter()
        .filter_map(|part| match part {
            PythonInterpolatedPart::Interpolation(interpolation) => Some(interpolation),
            PythonInterpolatedPart::Literal { .. } => None,
        })
        .next_back()
        .unwrap();
    assert_eq!(interpolation.expression().items().len(), 1);
    let spec = interpolation.format_spec().unwrap();
    let spec_parts = format_spec_parts(spec, source).unwrap();
    assert_eq!(spec_parts.len(), 2);
    assert!(matches!(
        spec_parts[0],
        PythonInterpolatedPart::Interpolation(_)
    ));
    assert!(matches!(
        spec_parts[1],
        PythonInterpolatedPart::Literal {
            kind: PythonInterpolatedLiteralKind::Escaped,
            ..
        }
    ));
}

#[test]
fn assignment_groups_preserve_commas_chaining_and_unpacking() {
    let tree = crate::parser()
        .with_strict(true)
        .parse("first, *rest = intermediate = values\n")
        .unwrap();
    let module = PythonModule::downcast_from(tree.top_node()).unwrap();
    let Some(PythonStatementNode::Assign(assign)) = module.statements().next() else {
        panic!("expected assignment");
    };
    let groups = assign.assignment_groups().unwrap();
    assert_eq!(groups.len(), 3);
    assert_eq!(groups[0].items().len(), 2);
    assert!(matches!(
        groups[0].items()[1],
        PythonExpressionItem::Starred { .. }
    ));
    assert_eq!(groups[1].items().len(), 1);
    assert_eq!(groups[2].items().len(), 1);
}

#[test]
fn subscript_view_preserves_indices_and_omitted_slice_bounds() {
    let tree = crate::parser()
        .with_top("Expression")
        .unwrap()
        .with_strict(true)
        .parse("value[first:second:step, index, :upper, lower:]")
        .unwrap();
    let expression = PythonExpression::downcast_from(tree.top_node()).unwrap();
    let Some(PythonExpressionNode::Member(member)) = expression.body() else {
        panic!("expected a member expression");
    };
    let access = member.access().unwrap();
    let PythonMemberSuffix::Subscript(subscript) = access.suffix() else {
        panic!("expected a subscript");
    };
    assert_eq!(subscript.items().len(), 4);
    let PythonSubscriptItem::Slice(first) = &subscript.items()[0] else {
        panic!("first item should be a slice");
    };
    assert!(first.lower().is_some());
    assert!(first.upper().is_some());
    assert!(first.step().is_some());
    assert!(matches!(
        subscript.items()[1],
        PythonSubscriptItem::Index(_)
    ));
    let PythonSubscriptItem::Slice(third) = &subscript.items()[2] else {
        panic!("third item should be a slice");
    };
    assert!(third.lower().is_none());
    assert!(third.upper().is_some());
    assert!(third.step().is_none());
    let PythonSubscriptItem::Slice(fourth) = &subscript.items()[3] else {
        panic!("fourth item should be a slice");
    };
    assert!(fourth.lower().is_some());
    assert!(fourth.upper().is_none());
    assert!(fourth.step().is_none());
}

#[test]
fn parameter_view_preserves_cpython_argument_groups() {
    let source =
        "def f(a: A = first, /, b=second, *args: T, c, d: D = fourth, **kwargs: P):\n    pass\n";
    let tree = crate::parser().with_strict(true).parse(source).unwrap();
    let module = PythonModule::downcast_from(tree.top_node()).unwrap();
    let Some(PythonStatementNode::Function(function)) = module.statements().next() else {
        panic!("expected a function");
    };
    let parameters = function.parameters().unwrap().parameters(source).unwrap();
    assert_eq!(parameters.positional_only().len(), 1);
    assert!(parameters.positional_only()[0].annotation().is_some());
    assert!(parameters.positional_only()[0].default().is_some());
    assert_eq!(parameters.positional_or_keyword().len(), 1);
    assert!(parameters.vararg().is_some());
    assert_eq!(parameters.keyword_only().len(), 2);
    assert!(parameters.keyword_only()[0].default().is_none());
    assert!(parameters.keyword_only()[1].annotation().is_some());
    assert!(parameters.keyword_only()[1].default().is_some());
    assert!(parameters.kwarg().is_some());
}

#[test]
fn type_parameter_view_preserves_kind_bound_and_default_roles() {
    let source =
        "def f[T: int = str, *Ts = *tuple[int], **P = [int]](value: T) -> T:\n    return value\n";
    let tree = crate::parser().with_strict(true).parse(source).unwrap();
    let module = PythonModule::downcast_from(tree.top_node()).unwrap();
    let Some(PythonStatementNode::Function(function)) = module.statements().next() else {
        panic!("expected a function");
    };
    let parameters = function
        .type_parameters()
        .unwrap()
        .type_parameters()
        .unwrap();
    assert_eq!(parameters.len(), 3);
    assert_eq!(parameters[0].kind(), PythonTypeParameterKind::TypeVar);
    assert!(parameters[0].bound().is_some());
    assert!(matches!(
        parameters[0].default(),
        Some(PythonExpressionItem::Plain(_))
    ));
    assert_eq!(parameters[1].kind(), PythonTypeParameterKind::TypeVarTuple);
    assert!(parameters[1].bound().is_none());
    assert!(matches!(
        parameters[1].default(),
        Some(PythonExpressionItem::Starred { .. })
    ));
    assert_eq!(parameters[2].kind(), PythonTypeParameterKind::ParamSpec);
    assert!(parameters[2].bound().is_none());
    assert!(matches!(
        parameters[2].default(),
        Some(PythonExpressionItem::Plain(_))
    ));
}

#[test]
fn comprehension_view_groups_generators_and_filters() {
    let tree = crate::parser()
        .with_top("Expression")
        .unwrap()
        .with_strict(true)
        .parse("[value async for first, second in source if predicate for third in other if final]")
        .unwrap();
    let expression = PythonExpression::downcast_from(tree.top_node()).unwrap();
    let Some(PythonExpressionNode::ArrayComprehension(array)) = expression.body() else {
        panic!("expected an array comprehension");
    };
    let comprehension = array.comprehension().unwrap();
    assert!(matches!(
        comprehension.head(),
        PythonComprehensionHead::Element(PythonExpressionItem::Plain(_))
    ));
    assert_eq!(comprehension.generators().len(), 2);
    assert!(comprehension.generators()[0].is_async());
    assert_eq!(comprehension.generators()[0].targets().len(), 2);
    assert_eq!(comprehension.generators()[0].filters().len(), 1);
    assert!(!comprehension.generators()[1].is_async());
    assert_eq!(comprehension.generators()[1].targets().len(), 1);
    assert_eq!(comprehension.generators()[1].filters().len(), 1);
}

#[test]
fn try_view_separates_exception_group_else_and_finally_clauses() {
    let tree = crate::parser()
        .with_strict(true)
        .parse(
            "try:\n    operation()\nexcept* (First, Second) as error:\n    recover(error)\nelse:\n    complete()\nfinally:\n    cleanup()\n",
        )
        .unwrap();
    let module = PythonModule::downcast_from(tree.top_node()).unwrap();
    let Some(PythonStatementNode::Try(statement)) = module.statements().next() else {
        panic!("expected a try statement");
    };
    let clauses = statement.clauses().unwrap();
    assert_eq!(clauses.body().statements().count(), 1);
    assert_eq!(clauses.handlers().len(), 1);
    assert!(clauses.handlers()[0].is_exception_group());
    assert_eq!(clauses.handlers()[0].types().len(), 1);
    assert!(clauses.handlers()[0].name().is_some());
    assert_eq!(clauses.handlers()[0].body().statements().count(), 1);
    assert!(clauses.orelse().is_some());
    assert!(clauses.finalbody().is_some());
}

#[test]
fn sequence_pattern_view_distinguishes_brackets_from_grouping_parentheses() {
    let source = "match subject:\n    case [first] | (second):\n        pass\n";
    let tree = crate::parser().with_strict(true).parse(source).unwrap();
    let module = PythonModule::downcast_from(tree.top_node()).unwrap();
    let Some(PythonStatementNode::Match(statement)) = module.statements().next() else {
        panic!("expected a match statement");
    };
    let clause = statement
        .body()
        .unwrap()
        .clauses()
        .next()
        .expect("one match clause");
    let Some(PythonPatternNode::Or(or_pattern)) = clause.patterns().next() else {
        panic!("expected an or pattern");
    };
    let alternatives = or_pattern.patterns().collect::<Vec<_>>();
    let [
        PythonPatternNode::Sequence(bracketed),
        PythonPatternNode::Sequence(grouped),
    ] = alternatives.as_slice()
    else {
        panic!("expected two sequence-pattern syntax nodes");
    };
    assert!(matches!(
        bracketed.sequence().unwrap(),
        PythonSequencePatternView::Sequence(_)
    ));
    assert!(matches!(
        grouped.sequence().unwrap(),
        PythonSequencePatternView::Grouped(_)
    ));
}

#[test]
fn control_flow_views_assign_conditions_targets_and_else_bodies() {
    let tree = crate::parser()
        .with_strict(true)
        .parse(
            "if first:\n    pass\nelif second:\n    pass\nelse:\n    pass\nwhile condition:\n    pass\nelse:\n    pass\nasync for first, *rest in source, other:\n    pass\nelse:\n    pass\n",
        )
        .unwrap();
    let module = PythonModule::downcast_from(tree.top_node()).unwrap();
    let statements = module.statements().collect::<Vec<_>>();
    let PythonStatementNode::If(if_statement) = &statements[0] else {
        panic!("expected if");
    };
    let conditional = if_statement.conditional().unwrap();
    assert_eq!(conditional.clauses().len(), 2);
    assert!(conditional.orelse().is_some());

    let PythonStatementNode::While(while_statement) = &statements[1] else {
        panic!("expected while");
    };
    let while_parts = while_statement.while_parts().unwrap();
    assert!(while_parts.orelse().is_some());

    let PythonStatementNode::For(for_statement) = &statements[2] else {
        panic!("expected for");
    };
    let for_parts = for_statement.for_parts().unwrap();
    assert!(for_parts.is_async());
    assert_eq!(for_parts.targets().len(), 2);
    assert!(matches!(
        for_parts.targets()[1],
        PythonExpressionItem::Starred { .. }
    ));
    assert_eq!(for_parts.iterators().len(), 2);
    assert!(for_parts.orelse().is_some());
}

#[test]
fn import_view_preserves_relative_modules_dotted_names_and_aliases() {
    let source = "import package.module as alias, local\nfrom ...parent.module import first, second as other\nfrom . import *\n";
    let tree = crate::parser().with_strict(true).parse(source).unwrap();
    let module = PythonModule::downcast_from(tree.top_node()).unwrap();
    let statements = module.statements().collect::<Vec<_>>();

    let PythonStatementNode::Import(direct) = &statements[0] else {
        panic!("expected direct import");
    };
    let direct = direct.import(source).unwrap();
    assert!(direct.from().is_none());
    assert_eq!(direct.aliases().len(), 2);
    assert_eq!(direct.aliases()[0].name().len(), 2);
    assert!(direct.aliases()[0].as_name().is_some());

    let PythonStatementNode::Import(relative) = &statements[1] else {
        panic!("expected relative import");
    };
    let relative = relative.import(source).unwrap();
    assert_eq!(relative.from().unwrap().level(), 3);
    assert_eq!(relative.from().unwrap().module().len(), 2);
    assert_eq!(relative.aliases().len(), 2);

    let PythonStatementNode::Import(wildcard) = &statements[2] else {
        panic!("expected wildcard import");
    };
    let wildcard = wildcard.import(source).unwrap();
    assert_eq!(wildcard.from().unwrap().level(), 1);
    assert!(wildcard.from().unwrap().module().is_empty());
    assert!(wildcard.aliases()[0].is_wildcard());
}
