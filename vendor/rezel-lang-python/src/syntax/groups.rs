//! Shared traversal for flattened comma-separated syntax groups.

use rezel_common::SyntaxNode;

use super::PythonCstInvariantError;

/// Visit comma-separated direct CST children without materializing a nested
/// collection.
///
/// Comments are visible CST children even though the grammar skips them. They
/// must not become a phantom item after a trailing comma.
pub(super) fn try_for_each_comma_group(
    node: &SyntaxNode,
    delimiters: &[&str],
    mut visit: impl FnMut(&[SyntaxNode]) -> Result<(), PythonCstInvariantError>,
) -> Result<(), PythonCstInvariantError> {
    let mut group = Vec::new();
    for child in node.children() {
        let name = child.name();
        if name.as_ref() == "Comment" || delimiters.contains(&name.as_ref()) {
            continue;
        }
        if name.as_ref() == "," {
            if !group.is_empty() {
                visit(&group)?;
                group.clear();
            }
            continue;
        }
        group.push(child);
    }
    if !group.is_empty() {
        visit(&group)?;
    }
    Ok(())
}
