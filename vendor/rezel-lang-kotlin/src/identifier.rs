use rezel_common::first_invalid_identifier_offset;
use rezel_lr::{StrictTokenValidationError, StrictTokenValidator};

use crate::terms;

pub(crate) static STRICT_TOKEN_VALIDATORS: [StrictTokenValidator; 1] =
    [StrictTokenValidator::new(terms::identifier, validate)];

fn validate(spelling: &str) -> Result<(), StrictTokenValidationError> {
    if spelling.starts_with('`') {
        return validate_escaped(spelling);
    }
    let invalid = first_invalid_identifier_offset(
        spelling,
        |character| character == '_' || unicode_ident::is_xid_start(character),
        unicode_ident::is_xid_continue,
    );
    match invalid {
        Some(offset) => Err(StrictTokenValidationError::new(
            offset,
            "invalid Kotlin identifier",
        )),
        None => Ok(()),
    }
}

fn validate_escaped(spelling: &str) -> Result<(), StrictTokenValidationError> {
    let valid = spelling
        .strip_prefix('`')
        .and_then(|value| value.strip_suffix('`'))
        .is_some_and(|value| !value.is_empty() && !value.contains(['`', '\n', '\r']));
    if valid {
        return Ok(());
    }
    Err(StrictTokenValidationError::new(
        0.into(),
        "invalid escaped Kotlin identifier",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_xid_and_escaped_identifiers() {
        for valid in ["name", "_name", "λ2", "`hello world`"] {
            assert!(validate(valid).is_ok(), "{valid:?}");
        }
        for invalid in ["2name", "name😀", "``"] {
            assert!(validate(invalid).is_err(), "{invalid:?}");
        }
    }

    #[test]
    fn parser_applies_validation_only_in_strict_mode() {
        let source = "val name😀 = 0\n";
        crate::parser()
            .parse(source)
            .expect("recovering Kotlin accepts the broad identifier candidate");
        let error = crate::parser()
            .with_strict(true)
            .parse(source)
            .expect_err("strict Kotlin validates the selected identifier token");
        assert_eq!(error.message(), "invalid Kotlin identifier");
    }
}
