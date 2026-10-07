use rezel_common::first_invalid_identifier_offset;
use rezel_lr::{StrictTokenValidationError, StrictTokenValidator};

use crate::terms;
use crate::tokens::lexical::{
    is_identifier_continue as is_swift_identifier_continue,
    is_identifier_start as is_swift_identifier_start, is_operator_continue, is_operator_start,
};

pub(crate) static STRICT_TOKEN_VALIDATORS: [StrictTokenValidator; 1] =
    [StrictTokenValidator::new(terms::identifier, validate)];

fn validate(spelling: &str) -> Result<(), StrictTokenValidationError> {
    if spelling.starts_with('`') {
        return validate_raw(spelling);
    }
    validate_ordinary(spelling)
}

fn validate_ordinary(spelling: &str) -> Result<(), StrictTokenValidationError> {
    if let Some(rest) = spelling.strip_prefix('$') {
        let valid = rest.is_empty()
            || rest.chars().any(|character| !character.is_ascii_digit())
                && first_invalid_identifier_offset(
                    rest,
                    is_identifier_continue,
                    is_identifier_continue,
                )
                .is_none();
        return valid.then_some(()).ok_or_else(|| invalid(0));
    }
    let offset =
        first_invalid_identifier_offset(spelling, is_identifier_start, is_identifier_continue);
    if offset.is_none() && spelling != "_" {
        return Ok(());
    }
    Err(StrictTokenValidationError::new(
        offset.unwrap_or_else(|| 0.into()),
        "invalid Swift identifier",
    ))
}

fn is_identifier_start(character: char) -> bool {
    unicode_ident::is_xid_start(character) || is_swift_identifier_start(u32::from(character))
}

fn is_identifier_continue(character: char) -> bool {
    unicode_ident::is_xid_continue(character) || is_swift_identifier_continue(u32::from(character))
}

fn validate_raw(spelling: &str) -> Result<(), StrictTokenValidationError> {
    let Some(content) = spelling
        .strip_prefix('`')
        .and_then(|value| value.strip_suffix('`'))
    else {
        return Err(invalid(0));
    };
    if content.is_empty() {
        return Err(invalid(1));
    }

    let mut has_non_operator = false;
    let mut has_non_whitespace = false;
    for (offset, character) in content.char_indices() {
        let code_point = u32::from(character);
        if character == '`'
            || matches!(character, '\n' | '\r' | '\\')
            || is_forbidden_raw_whitespace(code_point)
            || is_unprintable_ascii(code_point)
        {
            return Err(invalid(offset + 1));
        }
        if !is_permitted_raw_whitespace(code_point) {
            has_non_whitespace = true;
        }
        if (offset == 0 && !is_operator_start(code_point)) || !is_operator_continue(code_point) {
            has_non_operator = true;
        }
    }
    if has_non_operator && has_non_whitespace {
        Ok(())
    } else {
        Err(invalid(1))
    }
}

fn invalid(offset: usize) -> StrictTokenValidationError {
    let offset = rezel_common::TextSize::try_from(offset).expect("token offsets fit in TextSize");
    StrictTokenValidationError::new(offset, "invalid Swift identifier")
}

const fn is_forbidden_raw_whitespace(code_point: u32) -> bool {
    code_point >= 0x0009 && code_point <= 0x000D
        || code_point == 0x0085
        || code_point == 0x00A0
        || code_point == 0x1680
        || code_point >= 0x2000 && code_point <= 0x200A
        || code_point >= 0x2028 && code_point <= 0x2029
        || code_point == 0x202F
        || code_point == 0x205F
        || code_point == 0x3000
}

const fn is_permitted_raw_whitespace(code_point: u32) -> bool {
    matches!(code_point, 0x0020 | 0x200E | 0x200F)
}

const fn is_unprintable_ascii(code_point: u32) -> bool {
    code_point < 0x20 || code_point == 0x7F
}

#[cfg(test)]
mod tests {
    use super::validate;

    #[test]
    fn combines_xid_with_swift_identifier_forms() {
        for valid in ["name", "_name", "你好", "$", "$0name", "`hello world`"] {
            assert!(validate(valid).is_ok(), "{valid:?}");
        }
        for invalid in ["_", "$123", "name suffix", "`+++`"] {
            assert!(validate(invalid).is_err(), "{invalid:?}");
        }
    }

    #[test]
    fn parser_applies_validation_only_in_strict_mode() {
        let source = "let name\u{00a0}suffix = 0\n";
        crate::parser()
            .parse(source)
            .expect("recovering Swift accepts the broad identifier candidate");
        let error = crate::parser()
            .with_strict(true)
            .parse(source)
            .expect_err("strict Swift validates the selected identifier token");
        assert_eq!(error.message(), "invalid Swift identifier");
    }
}
