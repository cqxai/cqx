//! Hand-written statement syntax views.

use rezel_common::TypedNode;

use crate::{PythonBody, PythonExpressionNode, PythonWithStatement};

use super::{PythonCstInvariantError, PythonExpressionItem, collect_group, expression_items};

#[derive(Clone, Debug)]
pub(crate) struct PythonWith {
    asynchronous: bool,
    items: Vec<PythonWithItem>,
    body: PythonBody,
}

impl PythonWith {
    #[must_use]
    pub(crate) const fn is_async(&self) -> bool {
        self.asynchronous
    }

    #[must_use]
    pub(crate) fn items(&self) -> &[PythonWithItem] {
        &self.items
    }

    #[must_use]
    pub(crate) fn body(&self) -> &PythonBody {
        &self.body
    }
}

#[derive(Clone, Debug)]
pub(crate) struct PythonWithItem {
    context: PythonExpressionNode,
    target: Option<PythonExpressionItem>,
}

impl PythonWithItem {
    #[must_use]
    pub(crate) fn context(&self) -> &PythonExpressionNode {
        &self.context
    }

    #[must_use]
    pub(crate) fn target(&self) -> Option<&PythonExpressionItem> {
        self.target.as_ref()
    }
}

impl PythonWithStatement {
    /// # Errors
    ///
    /// Returns an error when a strict `with` CST lacks an item, target, or body.
    pub(crate) fn with_items(&self) -> Result<PythonWith, PythonCstInvariantError> {
        let children = self.syntax().children().collect::<Vec<_>>();
        let body = self
            .body()
            .ok_or(PythonCstInvariantError::new("a with body"))?;
        let body_index = children
            .iter()
            .position(|child| child.range() == body.syntax().range())
            .ok_or(PythonCstInvariantError::new(
                "the typed with body among direct children",
            ))?;
        let asynchronous = children[..body_index]
            .iter()
            .any(|child| child.name().as_ref() == "async");
        let mut items = Vec::new();
        let mut index = 0;
        while index < body_index {
            let Ok(context) = PythonExpressionNode::downcast_from(children[index].clone()) else {
                index += 1;
                continue;
            };
            index += 1;
            let target = if children
                .get(index)
                .is_some_and(|child| child.name().as_ref() == "as")
            {
                index += 1;
                let target_start = index;
                while index < body_index
                    && children[index].name().as_ref() != ","
                    && PythonBody::downcast_from(children[index].clone()).is_err()
                {
                    index += 1;
                }
                let targets = collect_group(children[target_start..index].iter().cloned())?;
                let [target] = targets.as_slice() else {
                    return Err(PythonCstInvariantError::new(
                        "one assignment target after with as",
                    ));
                };
                let target = target.clone();
                Some(target)
            } else {
                None
            };
            if target.is_none()
                && let PythonExpressionNode::Tuple(tuple) = &context
            {
                let contexts = expression_items(tuple.syntax())?;
                for context in contexts {
                    let PythonExpressionItem::Plain(context) = context else {
                        return Err(PythonCstInvariantError::new(
                            "unstarred parenthesized with items",
                        ));
                    };
                    items.push(PythonWithItem {
                        context,
                        target: None,
                    });
                }
            } else {
                items.push(PythonWithItem { context, target });
            }
        }
        if items.is_empty() {
            return Err(PythonCstInvariantError::new(
                "at least one with context manager",
            ));
        }
        Ok(PythonWith {
            asynchronous,
            items,
            body,
        })
    }
}
