//! Swift attribute names and structural attribute-list recognition.
//!
//! The parent module retains runtime tokenizer registration and dispatch
//! priority. This module owns attribute name tokenization, attribute-only
//! conditional-compilation classification, and source-only attribute
//! traversal shared by type and accessor lookaheads.

use rezel_common::{CodePoint, ParseError};
use rezel_lr::{InputStream, Stack};

use super::lexical::{is_identifier_continue, is_identifier_start, scan_lookahead_identifier};
use crate::terms;

use super::lookahead::{
    current, skip_balanced_angles, skip_balanced_parentheses, skip_trivia_with_line_break,
};
use super::name::starts_module_selector;

const CARRIAGE_RETURN: u32 = b'\r' as u32;
const LINE_FEED: u32 = b'\n' as u32;
const AT_SIGN: u32 = b'@' as u32;
const POUND: u32 = b'#' as u32;
const BACKTICK: u32 = b'`' as u32;
const LEFT_ANGLE: u32 = b'<' as u32;
const LEFT_PAREN: u32 = b'(' as u32;
const COLON: u32 = b':' as u32;
const PERIOD: u32 = b'.' as u32;
const RIGHT_ANGLE: u32 = b'>' as u32;
const DOLLAR: u32 = b'$' as u32;

pub(super) fn scan_if_config_lookahead(
    input: &mut InputStream,
    stack: &Stack,
) -> Result<(), ParseError> {
    if current(input) != Some(POUND) {
        return Ok(());
    }
    let can_shift_nonempty = stack.can_shift(terms::attributeIfConfigNonemptyLookahead);
    let can_shift_structural = stack.can_shift(terms::attributeIfConfigStructuralLookahead);
    if !can_shift_nonempty && !can_shift_structural {
        return Ok(());
    }

    let classification = {
        let mut lookahead = input.lookahead().map(CodePoint::as_u32).peekable();
        classify_attribute_if_config(&mut lookahead)
    };
    let Some(classification) = classification else {
        return Ok(());
    };
    let Some(marker) =
        attribute_if_config_marker(classification, can_shift_nonempty, can_shift_structural)
    else {
        return Ok(());
    };

    let term = match marker {
        AttributeIfConfigMarker::Nonempty => terms::attributeIfConfigNonemptyLookahead,
        AttributeIfConfigMarker::Structural => terms::attributeIfConfigStructuralLookahead,
    };
    input.accept_token(term)?;
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AttributeIfConfigClassification {
    Empty,
    Nonempty,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AttributeIfConfigMarker {
    Nonempty,
    Structural,
}

fn attribute_if_config_marker(
    classification: AttributeIfConfigClassification,
    can_shift_nonempty: bool,
    can_shift_structural: bool,
) -> Option<AttributeIfConfigMarker> {
    match (classification, can_shift_nonempty, can_shift_structural) {
        (AttributeIfConfigClassification::Nonempty, true, _) => {
            Some(AttributeIfConfigMarker::Nonempty)
        }
        (_, _, true) => Some(AttributeIfConfigMarker::Structural),
        _ => None,
    }
}

fn classify_attribute_if_config(
    input: &mut std::iter::Peekable<impl Iterator<Item = u32>>,
) -> Option<AttributeIfConfigClassification> {
    if scan_if_config_directive(input)? != IfConfigDirective::If {
        return None;
    }
    if !skip_if_config_directive_line(input) {
        return None;
    }

    let mut depth = 1usize;
    let mut has_attribute = false;
    let mut at_start_of_line = true;
    loop {
        let trivia_has_line_break = skip_trivia_with_line_break(input)?;
        at_start_of_line |= trivia_has_line_break;

        match input.peek().copied() {
            Some(AT_SIGN) => {
                if !scan_lookahead(input) {
                    return None;
                }
                has_attribute = true;
                at_start_of_line = false;
            }
            Some(POUND) => {
                if !at_start_of_line {
                    return None;
                }
                match scan_if_config_directive(input) {
                    Some(IfConfigDirective::If) => {
                        if !skip_if_config_directive_line(input) {
                            return None;
                        }
                        depth = depth.checked_add(1)?;
                        at_start_of_line = true;
                    }
                    Some(IfConfigDirective::Elseif | IfConfigDirective::Else) => {
                        if !skip_if_config_directive_line(input) {
                            return None;
                        }
                        at_start_of_line = true;
                    }
                    Some(IfConfigDirective::Endif) => {
                        depth -= 1;
                        if depth == 0 {
                            return Some(if has_attribute {
                                AttributeIfConfigClassification::Nonempty
                            } else {
                                AttributeIfConfigClassification::Empty
                            });
                        }
                        at_start_of_line = false;
                    }
                    None => return None,
                }
            }
            _ => return None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum IfConfigDirective {
    If,
    Elseif,
    Else,
    Endif,
}

fn scan_if_config_directive(
    input: &mut std::iter::Peekable<impl Iterator<Item = u32>>,
) -> Option<IfConfigDirective> {
    (input.next() == Some(POUND)).then_some(())?;
    let word = scan_lookahead_identifier(input)?;
    if word.is(b"if") {
        Some(IfConfigDirective::If)
    } else if word.is(b"elseif") {
        Some(IfConfigDirective::Elseif)
    } else if word.is(b"else") {
        Some(IfConfigDirective::Else)
    } else if word.is(b"endif") {
        Some(IfConfigDirective::Endif)
    } else {
        None
    }
}

fn skip_if_config_directive_line(
    input: &mut std::iter::Peekable<impl Iterator<Item = u32>>,
) -> bool {
    for next in input.by_ref() {
        if matches!(next, LINE_FEED | CARRIAGE_RETURN) {
            return true;
        }
    }
    false
}

pub(super) fn scan_lookahead(input: &mut std::iter::Peekable<impl Iterator<Item = u32>>) -> bool {
    if input.next() != Some(AT_SIGN) || scan_lookahead_identifier(input).is_none() {
        return false;
    }
    if !skip_name_tail(input) {
        return false;
    }
    input.peek() != Some(&LEFT_PAREN) || skip_balanced_parentheses(input)
}

pub(super) fn skip_name_tail(input: &mut std::iter::Peekable<impl Iterator<Item = u32>>) -> bool {
    loop {
        if input.peek() == Some(&LEFT_ANGLE) && !skip_balanced_angles(input) {
            return false;
        }
        if input.peek() != Some(&PERIOD) {
            return true;
        }
        input.next();
        if scan_lookahead_identifier(input).is_none() {
            return false;
        }
    }
}

pub(super) fn scan_name(input: &mut InputStream, stack: &Stack) -> Result<(), ParseError> {
    input.advance(1);
    let at_sign_end = input.mark();
    let Some(first_component) = scan_attribute_component(input) else {
        return Ok(());
    };
    if current(input) == Some(COLON) && stack.can_shift(terms::moduleSelectedAttributeAtSign) {
        let starts_selector = {
            let mut lookahead = input.lookahead().map(CodePoint::as_u32).peekable();
            starts_module_selector(&mut lookahead)
        };
        if starts_selector {
            input.accept_token_to(terms::moduleSelectedAttributeAtSign, at_sign_end)?;
            return Ok(());
        }
    }
    let mut argument_syntax = first_component;
    loop {
        if current(input) == Some(LEFT_ANGLE) {
            argument_syntax = AttributeNameComponent::Custom;
            if !scan_angle_clause(input) {
                return Ok(());
            }
        }
        if current(input) != Some(PERIOD) {
            break;
        }
        argument_syntax = AttributeNameComponent::Custom;
        input.advance(1);
        if scan_attribute_component(input).is_none() {
            return Ok(());
        }
    }
    let term = if current(input) == Some(LEFT_PAREN) {
        match argument_syntax {
            AttributeNameComponent::Custom => terms::customAttributeNameWithArguments,
            AttributeNameComponent::Effects => terms::effectsAttributeNameWithArguments,
            AttributeNameComponent::Specialize => terms::specializeAttributeNameWithArguments,
            AttributeNameComponent::Specialized => terms::specializedAttributeNameWithArguments,
            AttributeNameComponent::Differentiable => {
                terms::differentiableAttributeNameWithArguments
            }
            AttributeNameComponent::Derivative => terms::derivativeAttributeNameWithArguments,
            AttributeNameComponent::Abi => terms::abiAttributeNameWithArguments,
            AttributeNameComponent::SpecialSyntax => terms::AttributeNameWithArguments,
        }
    } else {
        terms::AttributeName
    };
    if stack.can_shift(term) {
        input.accept_token(term)?;
    }
    Ok(())
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum AttributeNameComponent {
    Custom,
    Effects,
    Specialize,
    Specialized,
    Differentiable,
    Derivative,
    Abi,
    SpecialSyntax,
}

fn scan_attribute_component(input: &mut InputStream) -> Option<AttributeNameComponent> {
    let first = current(input)?;
    if first == BACKTICK {
        input.advance(1);
        let mut length = 0usize;
        while let Some(next) = current(input) {
            if next == BACKTICK {
                input.advance(1);
                return (length > 0).then_some(AttributeNameComponent::Custom);
            }
            if matches!(next, LINE_FEED | CARRIAGE_RETURN) {
                return None;
            }
            input.advance(1);
            length += 1;
        }
        return None;
    }
    if first == DOLLAR {
        input.advance(1);
        let mut length = 0_usize;
        let mut all_digits = true;
        while let Some(next) = current(input).filter(|value| is_identifier_continue(*value)) {
            input.advance(1);
            length += 1;
            all_digits &= (u32::from(b'0')..=u32::from(b'9')).contains(&next);
        }
        return (length == 0 || !all_digits).then_some(AttributeNameComponent::Custom);
    }
    if !is_identifier_start(first) {
        return None;
    }

    let mut spelling = [0_u8; 24];
    let mut length = 0usize;
    let mut is_ascii = true;
    while let Some(next) = current(input).filter(|value| is_identifier_continue(*value)) {
        if next < 0x80 && length < spelling.len() {
            spelling[length] = u8::try_from(next).expect("ASCII code point fits in a byte");
        } else {
            is_ascii = false;
        }
        input.advance(1);
        length += 1;
    }
    if !is_ascii {
        return Some(AttributeNameComponent::Custom);
    }
    let spelling = &spelling[..length];
    match spelling {
        b"_effects" => return Some(AttributeNameComponent::Effects),
        b"_specialize" => return Some(AttributeNameComponent::Specialize),
        b"specialized" => return Some(AttributeNameComponent::Specialized),
        b"differentiable" => return Some(AttributeNameComponent::Differentiable),
        b"derivative" | b"transpose" => return Some(AttributeNameComponent::Derivative),
        b"abi" => return Some(AttributeNameComponent::Abi),
        _ => {}
    }
    Some(if is_declaration_attribute_with_special_syntax(spelling) {
        AttributeNameComponent::SpecialSyntax
    } else {
        AttributeNameComponent::Custom
    })
}

// Mirrors SwiftSyntax 60e8eb850721's
// `DeclarationAttributeWithSpecialSyntax`. All other attribute argument lists
// use the ordinary labeled-expression parser.
fn is_declaration_attribute_with_special_syntax(spelling: &[u8]) -> bool {
    matches!(
        spelling,
        b"_backDeploy"
            | b"_documentation"
            | b"_dynamicReplacement"
            | b"_effects"
            | b"_implements"
            | b"_originallyDefinedIn"
            | b"specialized"
            | b"_specialize"
            | b"_spi_available"
            | b"rethrows"
            | b"abi"
            | b"attached"
            | b"available"
            | b"backDeployed"
            | b"derivative"
            | b"differentiable"
            | b"freestanding"
            | b"objc"
            | b"Sendable"
            | b"transpose"
    )
}

fn scan_angle_clause(input: &mut InputStream) -> bool {
    let mut depth = 0usize;
    while let Some(next) = current(input) {
        input.advance(1);
        if next == LEFT_ANGLE {
            depth += 1;
        } else if next == RIGHT_ANGLE {
            depth -= 1;
            if depth == 0 {
                return true;
            }
        } else if matches!(next, LINE_FEED | CARRIAGE_RETURN) {
            return false;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::{AttributeIfConfigClassification, classify_attribute_if_config};

    #[test]
    fn nested_if_config_scan_stops_at_the_closing_directive() {
        const DEPTH: usize = 128;
        const ALLOWED_BOUNDARY_READS: usize = 2;

        let mut prefix = "#if OUTER\n".repeat(DEPTH);
        prefix.push_str("@MainActor\n");
        prefix.push_str(&"#endif\n".repeat(DEPTH));
        let source = format!("{prefix}{}", " sentinel".repeat(4_096));
        let inspected = Cell::new(0usize);
        let input = source.chars().map(u32::from).inspect(|_| {
            inspected.set(inspected.get() + 1);
        });

        assert_eq!(
            classify_attribute_if_config(&mut input.peekable()),
            Some(AttributeIfConfigClassification::Nonempty)
        );
        assert!(
            inspected.get() <= prefix.len() + ALLOWED_BOUNDARY_READS,
            "attribute if-config inspected {} code points for a {}-point prefix",
            inspected.get(),
            prefix.len(),
        );
    }
}
