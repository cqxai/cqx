use std::borrow::Borrow;
use std::collections::HashMap;
use std::hash::Hash;
use std::sync::Arc;

use rezel_common::{TextRange, TextSize};

use super::{PythonAstField, PythonAstKind};

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct AstNodeId(u32);

impl AstNodeId {
    #[must_use]
    pub const fn index(self) -> usize {
        self.0 as usize
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct StringId(u32);

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct PythonStringId(u32);

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct BytesId(u32);

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct PythonSourceRange {
    start: Option<TextSize>,
    end: Option<TextSize>,
}

impl PythonSourceRange {
    #[must_use]
    pub const fn new(start: Option<TextSize>, end: Option<TextSize>) -> Self {
        Self { start, end }
    }

    #[must_use]
    pub const fn start(self) -> Option<TextSize> {
        self.start
    }

    #[must_use]
    pub const fn end(self) -> Option<TextSize> {
        self.end
    }

    #[must_use]
    pub const fn byte_range(self) -> Option<TextRange> {
        match (self.start, self.end) {
            (Some(start), Some(end)) => Some(TextRange::new(start, end)),
            _ => None,
        }
    }
}

impl From<TextRange> for PythonSourceRange {
    fn from(value: TextRange) -> Self {
        Self::new(Some(value.start()), Some(value.end()))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum PythonConstant {
    None,
    Bool(bool),
    Integer(StringId),
    Float(u64),
    Complex { real: u64, imaginary: u64 },
    String(PythonStringId),
    Bytes(BytesId),
    Ellipsis,
}

#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum PythonAstValue {
    None,
    Bool(bool),
    Integer(StringId),
    String(StringId),
    Strings(Vec<StringId>),
    Constant(PythonConstant),
    Node(AstNodeId),
    Nodes(Vec<AstNodeId>),
    OptionalNodes(Vec<Option<AstNodeId>>),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PythonAstFieldValue {
    field: PythonAstField,
    value: PythonAstValue,
}

impl PythonAstFieldValue {
    #[must_use]
    pub const fn field(&self) -> PythonAstField {
        self.field
    }

    #[must_use]
    pub const fn value(&self) -> &PythonAstValue {
        &self.value
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PythonAstNode {
    kind: PythonAstKind,
    source_range: PythonSourceRange,
    fields: Vec<PythonAstFieldValue>,
}

impl PythonAstNode {
    #[must_use]
    pub const fn kind(&self) -> PythonAstKind {
        self.kind
    }

    #[must_use]
    pub const fn source_range(&self) -> PythonSourceRange {
        self.source_range
    }

    #[must_use]
    pub fn fields(&self) -> &[PythonAstFieldValue] {
        &self.fields
    }
}

#[derive(Debug)]
pub struct PythonAst {
    pub(super) nodes: Vec<PythonAstNode>,
    pub(super) strings: Vec<Arc<str>>,
    pub(super) python_strings: Vec<Arc<[u32]>>,
    pub(super) bytes: Vec<Arc<[u8]>>,
    pub(super) root: AstNodeId,
}

impl PythonAst {
    #[must_use]
    pub fn root(&self) -> &PythonAstNode {
        &self.nodes[self.root.index()]
    }

    #[must_use]
    pub const fn root_id(&self) -> AstNodeId {
        self.root
    }

    #[must_use]
    pub fn nodes(&self) -> &[PythonAstNode] {
        &self.nodes
    }

    #[must_use]
    pub fn node(&self, id: AstNodeId) -> Option<&PythonAstNode> {
        self.nodes.get(id.index())
    }

    #[must_use]
    pub fn string(&self, id: StringId) -> Option<&str> {
        self.strings.get(id.0 as usize).map(Arc::as_ref)
    }

    #[must_use]
    pub fn python_string(&self, id: PythonStringId) -> Option<&[u32]> {
        self.python_strings.get(id.0 as usize).map(Arc::as_ref)
    }

    #[must_use]
    pub fn bytes(&self, id: BytesId) -> Option<&[u8]> {
        self.bytes.get(id.0 as usize).map(Arc::as_ref)
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PythonAstOptions {
    pub type_comments: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum AstError {
    RecoveryTree,
    SourceTooLarge,
    UnsupportedSyntax {
        kind: String,
    },
    InvalidLiteral {
        kind: &'static str,
        spelling: String,
    },
    InvalidSyntax {
        position: TextSize,
        message: &'static str,
    },
    InconsistentCst {
        context: &'static str,
        expected: &'static str,
    },
    IndexOverflow,
}

impl std::fmt::Display for AstError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::RecoveryTree => formatter.write_str("cannot lower a recovery tree"),
            Self::SourceTooLarge => formatter.write_str("Python source is too large"),
            Self::UnsupportedSyntax { kind } => {
                write!(
                    formatter,
                    "Python AST lowering does not support CST kind {kind}"
                )
            }
            Self::InvalidLiteral { kind, spelling } => {
                write!(formatter, "invalid Python {kind} literal {spelling:?}")
            }
            Self::InvalidSyntax { position, message } => {
                write!(
                    formatter,
                    "invalid Python syntax at byte {}: {message}",
                    usize::from(*position)
                )
            }
            Self::InconsistentCst { context, expected } => {
                write!(formatter, "inconsistent {context}; expected {expected}")
            }
            Self::IndexOverflow => formatter.write_str("Python AST arena index overflow"),
        }
    }
}

impl std::error::Error for AstError {}

struct OwnedInterner<T: ?Sized> {
    values_by_id: Vec<Arc<T>>,
    ids_by_value: HashMap<Arc<T>, u32>,
}

impl<T> OwnedInterner<T>
where
    T: Eq + Hash + ?Sized,
{
    fn new() -> Self {
        Self {
            values_by_id: Vec::new(),
            ids_by_value: HashMap::new(),
        }
    }

    fn intern<Q>(&mut self, value: &Q) -> Result<u32, AstError>
    where
        Arc<T>: Borrow<Q>,
        for<'a> Arc<T>: From<&'a Q>,
        Q: Eq + Hash + ?Sized,
    {
        if let Some(id) = self.ids_by_value.get(value).copied() {
            return Ok(id);
        }

        let id = u32::try_from(self.values_by_id.len()).map_err(|_| AstError::IndexOverflow)?;
        let owned = Arc::<T>::from(value);
        self.values_by_id.push(Arc::clone(&owned));
        let previous = self.ids_by_value.insert(owned, id);
        debug_assert!(previous.is_none());
        Ok(id)
    }

    fn into_values(self) -> Vec<Arc<T>> {
        self.values_by_id
    }
}

pub(super) struct AstBuilder {
    nodes: Vec<PythonAstNode>,
    strings: OwnedInterner<str>,
    python_strings: OwnedInterner<[u32]>,
    bytes: OwnedInterner<[u8]>,
}

impl AstBuilder {
    pub(super) fn new() -> Self {
        Self {
            nodes: Vec::new(),
            strings: OwnedInterner::new(),
            python_strings: OwnedInterner::new(),
            bytes: OwnedInterner::new(),
        }
    }

    pub(super) fn push_node(
        &mut self,
        kind: PythonAstKind,
        range: PythonSourceRange,
    ) -> Result<AstNodeId, AstError> {
        let raw = u32::try_from(self.nodes.len()).map_err(|_| AstError::IndexOverflow)?;
        self.nodes.push(PythonAstNode {
            kind,
            source_range: range,
            fields: Vec::new(),
        });
        Ok(AstNodeId(raw))
    }

    pub(super) fn push_field(
        &mut self,
        node: AstNodeId,
        field: PythonAstField,
        value: PythonAstValue,
    ) -> Result<(), AstError> {
        self.nodes
            .get_mut(node.index())
            .ok_or(AstError::IndexOverflow)?
            .fields
            .push(PythonAstFieldValue { field, value });
        Ok(())
    }

    pub(super) fn intern(&mut self, value: &str) -> Result<StringId, AstError> {
        let id = self.strings.intern(value)?;
        Ok(StringId(id))
    }

    pub(super) fn intern_python_string(
        &mut self,
        value: &[u32],
    ) -> Result<PythonStringId, AstError> {
        let id = self.python_strings.intern(value)?;
        Ok(PythonStringId(id))
    }

    pub(super) fn intern_bytes(&mut self, value: &[u8]) -> Result<BytesId, AstError> {
        let id = self.bytes.intern(value)?;
        Ok(BytesId(id))
    }

    pub(super) fn finish(self, root: AstNodeId) -> PythonAst {
        PythonAst {
            nodes: self.nodes,
            strings: self.strings.into_values(),
            python_strings: self.python_strings.into_values(),
            bytes: self.bytes.into_values(),
            root,
        }
    }
}
