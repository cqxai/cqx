use rezel_common::TypedNode;

use crate::typed::{JavaIdentifier, JavaName, JavaScopedIdentifier};

pub(crate) fn scoped_name_parts(
    scoped: &JavaScopedIdentifier,
) -> Option<(JavaName, JavaIdentifier)> {
    let qualifier = scoped
        .syntax()
        .children()
        .find_map(|node| JavaName::downcast_from(node).ok())?;
    let selected = scoped
        .syntax()
        .children()
        .filter_map(|node| JavaIdentifier::downcast_from(node).ok())
        .last()?;
    Some((qualifier, selected))
}
