use std::iter::Peekable;

use rezel_common::{SyntaxChildren, SyntaxLanguage, TextRange, TypedNode};

use crate::typed::{JavaKind, JavaLanguage, JavaResource, JavaResourceSpecification};

pub(crate) struct ResourceEntry {
    pub(crate) resource: JavaResource,
    pub(crate) range: TextRange,
}

pub(crate) struct ResourceEntries {
    children: Peekable<SyntaxChildren>,
}

pub(crate) fn resource_entries(specification: &JavaResourceSpecification) -> ResourceEntries {
    ResourceEntries {
        children: specification.syntax().children().peekable(),
    }
}

impl Iterator for ResourceEntries {
    type Item = ResourceEntry;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let syntax = self.children.next()?;
            let Ok(resource) = JavaResource::downcast_from(syntax) else {
                continue;
            };
            let syntax = resource.syntax();
            let end = self.children.peek().map_or_else(
                || syntax.to(),
                |next| {
                    if JavaLanguage::kind(next) == Some(JavaKind::Semicolon) {
                        next.to()
                    } else {
                        syntax.to()
                    }
                },
            );
            return Some(ResourceEntry {
                range: TextRange::new(syntax.from(), end),
                resource,
            });
        }
    }
}
