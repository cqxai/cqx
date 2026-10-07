//! Hand-written declaration syntax views.

use rezel_common::{TextRange, TypedNode};

use crate::{PythonExpressionNode, PythonFunctionDefinition, PythonTypeParam, PythonTypeParamList};

use super::{PythonCstInvariantError, PythonExpressionItem, collect_group};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PythonTypeParameterKind {
    TypeVar,
    TypeVarTuple,
    ParamSpec,
}

#[derive(Clone, Debug)]
pub(crate) struct PythonTypeParameter {
    kind: PythonTypeParameterKind,
    name: crate::PythonVariableName,
    bound: Option<PythonExpressionNode>,
    default: Option<PythonExpressionItem>,
    range: TextRange,
}

impl PythonTypeParameter {
    #[must_use]
    pub(crate) const fn kind(&self) -> PythonTypeParameterKind {
        self.kind
    }

    #[must_use]
    pub(crate) fn name(&self) -> &crate::PythonVariableName {
        &self.name
    }

    #[must_use]
    pub(crate) fn bound(&self) -> Option<&PythonExpressionNode> {
        self.bound.as_ref()
    }

    #[must_use]
    pub(crate) fn default(&self) -> Option<&PythonExpressionItem> {
        self.default.as_ref()
    }

    #[must_use]
    pub(crate) const fn range(&self) -> TextRange {
        self.range
    }
}

impl PythonTypeParamList {
    /// # Errors
    ///
    /// Returns an error when a strict type-parameter CST violates its grammatical shape.
    pub(crate) fn type_parameters(
        &self,
    ) -> Result<Vec<PythonTypeParameter>, PythonCstInvariantError> {
        self.parameters()
            .map(|parameter| type_parameter(&parameter))
            .collect()
    }
}

impl PythonFunctionDefinition {
    #[must_use]
    pub(crate) fn is_async(&self) -> bool {
        self.syntax()
            .children()
            .any(|child| child.name().as_ref() == "async")
    }
}

fn type_parameter(
    parameter: &PythonTypeParam,
) -> Result<PythonTypeParameter, PythonCstInvariantError> {
    let children = parameter.syntax().children().collect::<Vec<_>>();
    let kind = match children.first().map(rezel_common::SyntaxNode::name) {
        Some(name) if name.as_ref() == "*" => PythonTypeParameterKind::TypeVarTuple,
        Some(name) if name.as_ref() == "**" => PythonTypeParameterKind::ParamSpec,
        _ => PythonTypeParameterKind::TypeVar,
    };
    let name = parameter
        .name()
        .ok_or(PythonCstInvariantError::new("a type parameter name"))?;
    let bound = parameter.bound().and_then(|bound| bound.annotation());
    if kind != PythonTypeParameterKind::TypeVar && bound.is_some() {
        return Err(PythonCstInvariantError::new(
            "a bound only on a regular type variable",
        ));
    }
    let default = children
        .iter()
        .position(|child| child.name().as_ref() == "AssignOp")
        .map(|index| {
            let items = collect_group(children[index + 1..].iter().cloned())?;
            let [item] = items.as_slice() else {
                return Err(PythonCstInvariantError::new("one type parameter default"));
            };
            Ok(item.clone())
        })
        .transpose()?;
    Ok(PythonTypeParameter {
        kind,
        name,
        bound,
        default,
        range: parameter.syntax().range(),
    })
}
