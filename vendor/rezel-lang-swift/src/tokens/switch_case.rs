//! Source recognition at Swift switch-case ownership boundaries.
//!
//! The parent attribute tokenizer retains parser-state checks and chooses the
//! list or body term. This module only classifies the source prefix shared by
//! attributed cases, conditional-compilation clauses, and diagnostics.

use rezel_common::CodePoint;
use rezel_lr::InputStream;

use super::lexical::scan_lookahead_identifier;

use super::lookahead::{skip_directive_line, skip_trivia};

const AT_SIGN: u32 = b'@' as u32;
const POUND: u32 = b'#' as u32;

pub(super) fn starts_attribute(input: &InputStream) -> bool {
    let mut lookahead = input.lookahead().map(CodePoint::as_u32).peekable();
    starts_attribute_from(&mut lookahead)
}

pub(super) fn starts_if_config(input: &InputStream) -> bool {
    let mut lookahead = input.lookahead().map(CodePoint::as_u32).peekable();
    starts_pound_if_from(&mut lookahead)
}

pub(super) fn if_config_belongs_to_list(input: &InputStream) -> bool {
    let mut lookahead = input.lookahead().map(CodePoint::as_u32).peekable();
    if_config_belongs_to_list_from(&mut lookahead)
}

pub(super) fn starts_diagnostic(input: &InputStream) -> bool {
    let mut lookahead = input.lookahead().map(CodePoint::as_u32).peekable();
    starts_diagnostic_from(&mut lookahead)
}

fn starts_attribute_from(input: &mut std::iter::Peekable<impl Iterator<Item = u32>>) -> bool {
    if input.next() != Some(AT_SIGN) || scan_lookahead_identifier(input).is_none() {
        return false;
    }
    skip_trivia(input);
    let Some(label) = scan_lookahead_identifier(input) else {
        return false;
    };
    label.is(b"case") || label.is(b"default")
}

fn starts_pound_if_from(input: &mut std::iter::Peekable<impl Iterator<Item = u32>>) -> bool {
    input.next() == Some(POUND)
        && scan_lookahead_identifier(input).is_some_and(|directive| directive.is(b"if"))
}

fn starts_diagnostic_from(input: &mut std::iter::Peekable<impl Iterator<Item = u32>>) -> bool {
    if input.next() != Some(POUND) {
        return false;
    }
    scan_lookahead_identifier(input).is_some_and(|name| name.is(b"warning") || name.is(b"error"))
}

fn if_config_belongs_to_list_from(
    input: &mut std::iter::Peekable<impl Iterator<Item = u32>>,
) -> bool {
    if !starts_pound_if_from(input) {
        return false;
    }
    loop {
        if !skip_directive_line(input) {
            return false;
        }
        skip_trivia(input);
        if input.peek() != Some(&POUND) {
            break;
        }
        input.next();
        let Some(directive) = scan_lookahead_identifier(input) else {
            return false;
        };
        if !directive.is(b"if") && !directive.is(b"elseif") && !directive.is(b"else") {
            return false;
        }
    }

    if input.peek() == Some(&AT_SIGN) {
        return starts_attribute_from(input);
    }
    scan_lookahead_identifier(input).is_some_and(|label| label.is(b"case") || label.is(b"default"))
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::{if_config_belongs_to_list_from, starts_attribute_from, starts_diagnostic_from};

    fn starts_attribute(source: &str) -> bool {
        starts_attribute_from(&mut source.chars().map(u32::from).peekable())
    }

    fn if_config_belongs_to_list(source: &str) -> bool {
        if_config_belongs_to_list_from(&mut source.chars().map(u32::from).peekable())
    }

    fn starts_diagnostic(source: &str) -> bool {
        starts_diagnostic_from(&mut source.chars().map(u32::from).peekable())
    }

    fn counted_code_points<'a>(
        source: &'a str,
        inspected: &'a Cell<usize>,
    ) -> impl Iterator<Item = u32> + 'a {
        source.chars().map(u32::from).inspect(move |_| {
            inspected.set(inspected.get() + 1);
        })
    }

    #[test]
    fn attribute_lookahead_has_a_narrow_boundary() {
        let cases = [
            ("case", "@unknown case _:", true),
            ("default", "@unknown default:", true),
            ("comment trivia", "@unknown /* marker */ default:", true),
            ("backticked attribute", "@`unknown` case _:", true),
            ("arguments", "@unknown(flag) default:", false),
            ("qualified name", "@Swift.unknown case _:", false),
            ("generic name", "@unknown<T> case _:", false),
            ("declaration", "@MainActor func local() {}", false),
            ("word boundary", "@unknown caseValue:", false),
            ("second attribute", "@unknown @available case _:", false),
        ];

        for (name, source, expected) in cases {
            assert_eq!(starts_attribute(source), expected, "{name}: {source}");
        }
    }

    #[test]
    fn conditional_lookahead_follows_the_first_clause_shape() {
        let cases = [
            ("case", "#if FLAG\ncase .value:", true),
            ("attributed default", "#if FLAG\n@unknown default:", true),
            (
                "empty leading clauses",
                "#if FIRST\n#elseif SECOND\n#else\ncase .value:",
                true,
            ),
            (
                "nested directive",
                "#if FIRST\n#if SECOND\ncase .value:",
                true,
            ),
            (
                "comment trivia",
                "#if FLAG // condition\n/* before */ default:",
                true,
            ),
            ("ordinary body", "#if FLAG\nlet value = 0", false),
            ("diagnostic body", "#if FLAG\n#warning(\"message\")", false),
            ("empty body", "#if FLAG\n#endif", false),
            ("word boundary", "#ifdef FLAG\ncase .value:", false),
        ];

        for (name, source, expected) in cases {
            assert_eq!(
                if_config_belongs_to_list(source),
                expected,
                "{name}: {source}"
            );
        }
    }

    #[test]
    fn conditional_lookahead_does_not_scan_past_the_first_body_head() {
        const ALLOWED_BOUNDARY_READS: usize = 2;

        let prefix = "#if FIRST\n#elseif SECOND\n#else\ncase";
        let source = format!("{prefix}{}", " sentinel".repeat(4_096));
        let inspected = Cell::new(0usize);
        let input = counted_code_points(&source, &inspected);

        assert!(if_config_belongs_to_list_from(&mut input.peekable()));
        assert!(
            inspected.get() <= prefix.len() + ALLOWED_BOUNDARY_READS,
            "conditional switch case inspected {} code points for a {}-point prefix",
            inspected.get(),
            prefix.len(),
        );
    }

    #[test]
    fn diagnostic_lookahead_has_an_exact_boundary() {
        for (source, expected) in [
            ("#warning(\"message\")", true),
            ("#error(\"message\")", true),
            ("#warningValue", false),
            ("# error(\"message\")", false),
            ("#if FLAG", false),
            ("#macro", false),
        ] {
            assert_eq!(starts_diagnostic(source), expected, "{source}");
        }
    }
}
