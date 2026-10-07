//! Hand-written pattern syntax views.

use rezel_common::{SyntaxNode, TypedNode};

use crate::{
    PythonCapturePattern, PythonExpressionNode, PythonLiteralPattern, PythonMappingPattern,
    PythonPatternNode, PythonSequencePattern, PythonVariableName,
};

use super::{PythonCstInvariantError, groups::try_for_each_comma_group};

#[derive(Clone, Debug)]
pub(crate) struct PythonPatternLiteral {
    values: Vec<PythonExpressionNode>,
    operators: Vec<SyntaxNode>,
}

impl PythonPatternLiteral {
    #[must_use]
    pub(crate) fn values(&self) -> &[PythonExpressionNode] {
        &self.values
    }

    #[must_use]
    pub(crate) fn operators(&self) -> &[SyntaxNode] {
        &self.operators
    }
}

impl PythonLiteralPattern {
    /// # Errors
    ///
    /// Returns an error when a strict literal-pattern CST has no literal value.
    pub(crate) fn literal(&self) -> Result<PythonPatternLiteral, PythonCstInvariantError> {
        let values = self.values().collect::<Vec<_>>();
        if values.is_empty() {
            return Err(PythonCstInvariantError::new("a literal pattern value"));
        }
        let operators = self
            .syntax()
            .children()
            .filter(|child| child.name().as_ref() == "ArithOp")
            .collect();
        Ok(PythonPatternLiteral { values, operators })
    }
}

#[derive(Clone, Debug)]
pub(crate) enum PythonSequencePatternView {
    Grouped(PythonPatternNode),
    Sequence(Vec<PythonPatternNode>),
}

impl PythonSequencePattern {
    /// # Errors
    ///
    /// Returns an error when a strict grouped pattern does not contain exactly one pattern.
    pub(crate) fn sequence(&self) -> Result<PythonSequencePatternView, PythonCstInvariantError> {
        let patterns = self.patterns().collect::<Vec<_>>();
        let mut comma = false;
        let mut square = false;
        for child in self.syntax().children() {
            match child.name().as_ref() {
                "," => comma = true,
                "[" => square = true,
                _ => {}
            }
        }
        if !comma && !square {
            let [pattern] = patterns.as_slice() else {
                return Err(PythonCstInvariantError::new(
                    "one grouped pattern without a comma",
                ));
            };
            return Ok(PythonSequencePatternView::Grouped(pattern.clone()));
        }
        Ok(PythonSequencePatternView::Sequence(patterns))
    }
}

#[derive(Clone, Debug)]
pub(crate) enum PythonMappingKey {
    Literal(PythonLiteralPattern),
    Value(PythonVariableName),
}

#[derive(Clone, Debug)]
pub(crate) struct PythonMappingEntry {
    key: PythonMappingKey,
    pattern: PythonPatternNode,
}

impl PythonMappingEntry {
    #[must_use]
    pub(crate) fn key(&self) -> &PythonMappingKey {
        &self.key
    }

    #[must_use]
    pub(crate) fn pattern(&self) -> &PythonPatternNode {
        &self.pattern
    }
}

#[derive(Clone, Debug)]
pub(crate) struct PythonMapping {
    entries: Vec<PythonMappingEntry>,
    rest: Option<PythonCapturePattern>,
}

impl PythonMapping {
    #[must_use]
    pub(crate) fn entries(&self) -> &[PythonMappingEntry] {
        &self.entries
    }

    #[must_use]
    pub(crate) fn rest(&self) -> Option<&PythonCapturePattern> {
        self.rest.as_ref()
    }
}

impl PythonMappingPattern {
    /// # Errors
    ///
    /// Returns an error when a strict mapping-pattern CST contains an incomplete entry.
    pub(crate) fn mapping(&self) -> Result<PythonMapping, PythonCstInvariantError> {
        let mut entries = Vec::new();
        let mut rest = None;
        try_for_each_comma_group(self.syntax(), &["{", "}"], |group| {
            if group
                .first()
                .is_some_and(|child| child.name().as_ref() == "**")
            {
                let capture = group
                    .iter()
                    .find_map(|child| PythonCapturePattern::downcast_from(child.clone()).ok())
                    .ok_or(PythonCstInvariantError::new(
                        "a capture after mapping double star",
                    ))?;
                if rest.replace(capture).is_some() {
                    return Err(PythonCstInvariantError::new(
                        "at most one mapping rest capture",
                    ));
                }
                return Ok(());
            }
            if rest.is_some() {
                return Err(PythonCstInvariantError::new(
                    "the mapping rest capture as the final item",
                ));
            }
            let key = group
                .iter()
                .find_map(|child| {
                    PythonLiteralPattern::downcast_from(child.clone())
                        .map(PythonMappingKey::Literal)
                        .or_else(|_| {
                            PythonVariableName::downcast_from(child.clone())
                                .map(PythonMappingKey::Value)
                        })
                        .ok()
                })
                .ok_or(PythonCstInvariantError::new("a mapping pattern key"))?;
            let pattern = group
                .iter()
                .rev()
                .find_map(|child| PythonPatternNode::downcast_from(child.clone()).ok())
                .ok_or(PythonCstInvariantError::new("a mapping pattern value"))?;
            entries.push(PythonMappingEntry { key, pattern });
            Ok(())
        })?;
        Ok(PythonMapping { entries, rest })
    }
}
