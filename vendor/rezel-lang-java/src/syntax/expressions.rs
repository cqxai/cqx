use rezel_common::{SyntaxLanguage, TypedNode};

use crate::typed::{
    JavaExplicitConstructorInvocation, JavaExpression, JavaFieldAccess, JavaIdentifier, JavaKind,
    JavaLanguage, JavaThisExpression,
};

pub(crate) enum FieldAccessSelection {
    Identifier(JavaIdentifier),
    This(JavaThisExpression),
}

pub(crate) fn field_access_selection(access: &JavaFieldAccess) -> Option<FieldAccessSelection> {
    access.syntax().children().filter_map(selection).last()
}

fn selection(node: rezel_common::SyntaxNode) -> Option<FieldAccessSelection> {
    match JavaIdentifier::downcast_from(node) {
        Ok(identifier) => Some(FieldAccessSelection::Identifier(identifier)),
        Err(node) => JavaThisExpression::downcast_from(node)
            .ok()
            .map(FieldAccessSelection::This),
    }
}

pub(crate) struct ConstructorInvocationTarget {
    pub(crate) qualifier: Option<rezel_common::SyntaxNode>,
    pub(crate) target: rezel_common::SyntaxNode,
}

pub(crate) fn constructor_invocation_target(
    invocation: &JavaExplicitConstructorInvocation,
) -> Option<ConstructorInvocationTarget> {
    let mut qualifier = None;
    for node in invocation.syntax().children() {
        if matches!(
            JavaLanguage::kind(&node),
            Some(JavaKind::This | JavaKind::Super)
        ) {
            return Some(ConstructorInvocationTarget {
                qualifier,
                target: node,
            });
        }
        if JavaExpression::downcast_from(node.clone()).is_ok()
            || matches!(
                JavaLanguage::kind(&node),
                Some(JavaKind::This | JavaKind::Super)
            )
        {
            qualifier = Some(node);
        }
    }
    None
}
