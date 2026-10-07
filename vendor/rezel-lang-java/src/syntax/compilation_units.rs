use rezel_common::{SyntaxLanguage, TypedNode};

use crate::typed::{JavaKind, JavaLanguage, JavaTopLevelDeclaration, JavaUnitBeforeFirstMethod};

pub(crate) enum CompactMemberKind {
    Field,
    Method,
    NestedType,
}

pub(crate) struct CompactMember {
    pub(crate) syntax: rezel_common::SyntaxNode,
    pub(crate) kind: CompactMemberKind,
}

pub(crate) enum UnitMember {
    TopLevel(JavaTopLevelDeclaration),
    Compact(CompactMember),
}

pub(crate) fn is_compact_unit(unit: &JavaUnitBeforeFirstMethod) -> Option<bool> {
    contains_compact_member(unit.syntax())
}

pub(crate) fn try_for_each_unit_member<E>(
    unit: &JavaUnitBeforeFirstMethod,
    mut visit: impl FnMut(UnitMember) -> Result<(), E>,
) -> Result<bool, E> {
    collect(unit.syntax(), &mut visit)
}

fn collect<E>(
    node: &rezel_common::SyntaxNode,
    visit: &mut impl FnMut(UnitMember) -> Result<(), E>,
) -> Result<bool, E> {
    for child in node.children() {
        let Some(kind) = JavaLanguage::kind(&child) else {
            return Ok(false);
        };
        match kind {
            JavaKind::TopLevelDeclaration => {
                let Ok(declaration) = JavaTopLevelDeclaration::downcast_from(child) else {
                    return Ok(false);
                };
                visit(UnitMember::TopLevel(declaration))?;
            }
            JavaKind::FieldDeclaration => visit(UnitMember::Compact(CompactMember {
                syntax: child,
                kind: CompactMemberKind::Field,
            }))?,
            JavaKind::MethodDeclaration => visit(UnitMember::Compact(CompactMember {
                syntax: child,
                kind: CompactMemberKind::Method,
            }))?,
            JavaKind::ClassDeclaration
            | JavaKind::RecordDeclaration
            | JavaKind::InterfaceDeclaration
            | JavaKind::AnnotationTypeDeclaration
            | JavaKind::EnumDeclaration => visit(UnitMember::Compact(CompactMember {
                syntax: child,
                kind: CompactMemberKind::NestedType,
            }))?,
            JavaKind::UnitBeforeFirstMethod
            | JavaKind::CompactBeforeFirstMethod
            | JavaKind::CompactMemberDeclarationNoMethod
            | JavaKind::CompactMemberDeclaration => {
                if !collect(&child, visit)? {
                    return Ok(false);
                }
            }
            JavaKind::Semicolon | JavaKind::LineComment | JavaKind::BlockComment => {}
            _ => return Ok(false),
        }
    }
    Ok(true)
}

fn contains_compact_member(node: &rezel_common::SyntaxNode) -> Option<bool> {
    for child in node.children() {
        let kind = JavaLanguage::kind(&child)?;
        match kind {
            JavaKind::FieldDeclaration
            | JavaKind::MethodDeclaration
            | JavaKind::CompactBeforeFirstMethod
            | JavaKind::CompactMemberDeclarationNoMethod
            | JavaKind::CompactMemberDeclaration => return Some(true),
            JavaKind::UnitBeforeFirstMethod if contains_compact_member(&child)? => {
                return Some(true);
            }
            _ => {}
        }
    }
    Some(false)
}
