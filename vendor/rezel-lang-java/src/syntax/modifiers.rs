use rezel_common::{SyntaxChildren, SyntaxLanguage, TypedNode};

use crate::typed::{JavaAnnotationNode, JavaKind, JavaLanguage};

pub(crate) enum ModifierItem {
    Annotation(JavaAnnotationNode),
    Keyword(JavaKind),
}

pub(crate) fn modifier_items(node: &rezel_common::SyntaxNode) -> ModifierItems {
    ModifierItems {
        children: node.children(),
    }
}

pub(crate) struct ModifierItems {
    children: SyntaxChildren,
}

impl Iterator for ModifierItems {
    type Item = ModifierItem;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let node = self.children.next()?;
            if let Ok(annotation) = JavaAnnotationNode::downcast_from(node.clone()) {
                return Some(ModifierItem::Annotation(annotation));
            }
            let Some(kind) = JavaLanguage::kind(&node) else {
                continue;
            };
            if matches!(
                kind,
                JavaKind::Abstract
                    | JavaKind::Default
                    | JavaKind::Final
                    | JavaKind::Native
                    | JavaKind::NonSealedModifier
                    | JavaKind::Private
                    | JavaKind::Protected
                    | JavaKind::Public
                    | JavaKind::Sealed
                    | JavaKind::Static
                    | JavaKind::Strictfp
                    | JavaKind::Synchronized
                    | JavaKind::Transient
                    | JavaKind::Volatile
            ) {
                return Some(ModifierItem::Keyword(kind));
            }
        }
    }
}
