use rezel_common::{CodePoint, ParseError};
use rezel_lr::{ExternalTokenizer, ExternalTokenizerStart, InputStream, Stack, TokenizerFlags};

use crate::terms;

mod accessor;
mod attribute;
mod catch;
mod closure;
mod condition;
mod generic;
mod layout;
pub(crate) mod lexical;
mod literal;
mod lookahead;
mod name;
mod operator;
mod switch_case;
mod type_lookahead;

use lexical::{
    is_identifier_continue, is_identifier_start, is_operator_start, scan_lookahead_identifier,
    scan_lookahead_identifier_after_first,
};

#[cfg(test)]
use lookahead::next_nontrivia;
use lookahead::{current, first_nontrivia_on_same_line, peek, starts_member_access_continuation};
#[cfg(test)]
use operator::{OperatorFixity, operator_has_fixed_role};

const SLASH: u32 = b'/' as u32;
const BACKTICK: u32 = b'`' as u32;
const AT_SIGN: u32 = b'@' as u32;
const POUND: u32 = b'#' as u32;
const EXCLAMATION: u32 = b'!' as u32;
const LEFT_ANGLE: u32 = b'<' as u32;
const LEFT_BRACE: u32 = b'{' as u32;
const LEFT_BRACKET: u32 = b'[' as u32;
const LEFT_PAREN: u32 = b'(' as u32;
const COMMA: u32 = b',' as u32;
const PERIOD: u32 = b'.' as u32;
const QUESTION: u32 = b'?' as u32;
const UNDERSCORE: u32 = b'_' as u32;

const CONTEXTUAL_TOKENIZER: TokenizerFlags = TokenizerFlags {
    contextual: true,
    fallback: false,
    extend: false,
};

const METATYPE_TOKENIZER: TokenizerFlags = TokenizerFlags {
    contextual: false,
    fallback: false,
    extend: false,
};

const KEYWORD_IDENTIFIER_TOKENIZER: TokenizerFlags = TokenizerFlags {
    contextual: false,
    fallback: false,
    extend: true,
};

// The runtime consults these conservative source domains before entering the
// corresponding Swift decision. Keep them alongside registration: adding a
// new accepting branch to a scanner also requires extending its domain here.
const TRIVIA_START: ExternalTokenizerStart = ExternalTokenizerStart::NONE
    .with_ascii_range(b'\t'..=b'\r')
    .with_ascii(b' ')
    .with_ascii(b'/');

const METATYPE_START: ExternalTokenizerStart = ExternalTokenizerStart::NONE
    .with_ascii(b'P')
    .with_ascii(b'T');

const KEYWORD_IDENTIFIER_START: ExternalTokenizerStart = ExternalTokenizerStart::NONE
    .with_ascii_range(b'A'..=b'Z')
    .with_ascii_range(b'a'..=b'z');

const ACCESSOR_LOOKAHEAD_START: ExternalTokenizerStart = ExternalTokenizerStart::NONE
    .with_ascii(b'@')
    .with_ascii(b'_')
    .with_ascii(b'a')
    .with_ascii(b'b')
    .with_ascii(b'c')
    .with_ascii(b'd')
    .with_ascii(b'g')
    .with_ascii(b'i')
    .with_ascii(b'm')
    .with_ascii(b'n')
    .with_ascii(b'r')
    .with_ascii(b's')
    .with_ascii(b'u')
    .with_ascii(b'w')
    .with_ascii(b'y')
    .with_ascii(b'{');

const TYPE_PATH_LOOKAHEAD_START: ExternalTokenizerStart = ExternalTokenizerStart::NONE
    .with_ascii(b'.')
    .with_ascii(b'<')
    .with_ascii(b'A');

const ATTRIBUTE_START: ExternalTokenizerStart = ExternalTokenizerStart::NONE
    .with_ascii(b'#')
    .with_ascii(b'@');

const LITERAL_START: ExternalTokenizerStart = ExternalTokenizerStart::NONE
    .with_ascii(b'"')
    .with_ascii(b'#')
    .with_ascii(b'/');

const GENERIC_VALUE_LOOKAHEAD_START: ExternalTokenizerStart = ExternalTokenizerStart::NONE
    .with_ascii(b'(')
    .with_ascii(b'[');

const SYNTAX_LOOKAHEAD_START: ExternalTokenizerStart = ExternalTokenizerStart::NONE
    .with_ascii(b'(')
    .with_ascii(b'.')
    .with_ascii(b'<')
    .with_ascii(b'@')
    .with_ascii(b'a')
    .with_ascii(b'e')
    .with_ascii(b'i')
    .with_ascii(b'n')
    .with_ascii(b'u')
    .with_ascii(b'{');

const OPERATOR_START: ExternalTokenizerStart = ExternalTokenizerStart::NONE
    .with_ascii(b'!')
    .with_ascii(b'%')
    .with_ascii(b'&')
    .with_ascii(b'*')
    .with_ascii(b'+')
    .with_ascii(b'-')
    .with_ascii(b'.')
    .with_ascii(b'/')
    .with_ascii(b'<')
    .with_ascii(b'=')
    .with_ascii(b'>')
    .with_ascii(b'?')
    .with_ascii(b'^')
    .with_ascii(b'|')
    .with_ascii(b'~')
    .with_non_ascii();

const CLOSURE_SIGNATURE_START: ExternalTokenizerStart = ExternalTokenizerStart::NONE
    .with_ascii_range(b'\t'..=b'\r')
    .with_ascii(b' ')
    .with_ascii(b'/')
    .with_ascii(b'@')
    .with_ascii(b'[')
    .with_ascii(b'(')
    .with_ascii(b'`')
    .with_ascii_range(b'A'..=b'Z')
    .with_ascii(b'_')
    .with_ascii_range(b'a'..=b'z')
    .with_non_ascii();

pub(crate) static GENERIC_REQUIREMENT_CONTINUATION: ExternalTokenizer =
    ExternalTokenizer::new(scan_generic_requirement_continuation, CONTEXTUAL_TOKENIZER)
        .with_start(TRIVIA_START);

pub(crate) static CODE_ITEM_LAYOUT: ExternalTokenizer =
    ExternalTokenizer::new(scan_code_item_separator, CONTEXTUAL_TOKENIZER).with_start(TRIVIA_START);

pub(crate) static METATYPE_TOKENS: ExternalTokenizer =
    ExternalTokenizer::new(scan_metatype, METATYPE_TOKENIZER).with_start(METATYPE_START);

pub(crate) static KEYWORD_IDENTIFIERS: ExternalTokenizer =
    ExternalTokenizer::new(scan_keyword_identifier, KEYWORD_IDENTIFIER_TOKENIZER)
        .with_start(KEYWORD_IDENTIFIER_START);

pub(crate) static ACCESSOR_LOOKAHEADS: ExternalTokenizer =
    ExternalTokenizer::new(scan_accessor_lookahead, CONTEXTUAL_TOKENIZER)
        .with_start(ACCESSOR_LOOKAHEAD_START);

pub(crate) static TYPE_PATH_LOOKAHEADS: ExternalTokenizer =
    ExternalTokenizer::new(scan_type_path_lookahead, CONTEXTUAL_TOKENIZER)
        .with_start(TYPE_PATH_LOOKAHEAD_START);

pub(crate) static ATTRIBUTE_TOKENS: ExternalTokenizer =
    ExternalTokenizer::new(scan_attribute, CONTEXTUAL_TOKENIZER).with_start(ATTRIBUTE_START);

pub(crate) static LITERALS: ExternalTokenizer =
    ExternalTokenizer::new(scan_literal, CONTEXTUAL_TOKENIZER).with_start(LITERAL_START);

pub(crate) static GENERIC_VALUE_LOOKAHEADS: ExternalTokenizer =
    ExternalTokenizer::new(scan_generic_value_lookahead, CONTEXTUAL_TOKENIZER)
        .with_start(GENERIC_VALUE_LOOKAHEAD_START);

pub(crate) static SYNTAX_LOOKAHEADS: ExternalTokenizer =
    ExternalTokenizer::new(scan_syntax_lookahead_token, CONTEXTUAL_TOKENIZER)
        .with_start(SYNTAX_LOOKAHEAD_START);

pub(crate) static OPERATORS: ExternalTokenizer =
    ExternalTokenizer::new(scan_operator, CONTEXTUAL_TOKENIZER).with_start(OPERATOR_START);

pub(crate) static CATCH_PATTERN_CONTEXT: ExternalTokenizer =
    ExternalTokenizer::new(scan_catch_pattern_context, CONTEXTUAL_TOKENIZER);

pub(crate) static CLOSURE_SIGNATURE_CONTEXT: ExternalTokenizer =
    ExternalTokenizer::new(scan_closure_signature_context, CONTEXTUAL_TOKENIZER)
        .with_start(CLOSURE_SIGNATURE_START);

fn starts_exact_ascii_identifier(input: &InputStream, expected: &[u8]) -> bool {
    let mut offset = 0_isize;
    for expected_byte in expected {
        if peek(input, offset) != Some(u32::from(*expected_byte)) {
            return false;
        }
        offset += 1;
    }
    !peek(input, offset).is_some_and(is_identifier_continue)
}

fn scan_metatype(input: &mut InputStream, _stack: &Stack) -> Result<(), ParseError> {
    let (term, length) = if starts_exact_ascii_identifier(input, b"Type") {
        (terms::TypeKeyword, "Type".len())
    } else if starts_exact_ascii_identifier(input, b"Protocol") {
        (terms::ProtocolKeyword, "Protocol".len())
    } else {
        return Ok(());
    };
    input.advance(length);
    input.accept_token(term)
}

fn scan_keyword_identifier(input: &mut InputStream, _stack: &Stack) -> Result<(), ParseError> {
    let Some(length) = keyword_identifier_length(input) else {
        return Ok(());
    };
    input.advance(length);
    input.accept_token(terms::keywordIdentifierToken)
}

fn keyword_identifier_length(input: &InputStream) -> Option<usize> {
    let mut lookahead = input.lookahead().map(CodePoint::as_u32).peekable();
    let identifier = scan_lookahead_identifier(&mut lookahead)?;
    identifier
        .is_lexer_keyword()
        .then(|| identifier.ascii_length())
        .flatten()
}

fn starts_contextual_any_type(input: &InputStream) -> bool {
    const ANY: &[u8] = b"any";
    if !starts_exact_ascii_identifier(input, ANY) {
        return false;
    }
    let mut lookahead = input.lookahead().map(CodePoint::as_u32).peekable();
    for _ in ANY {
        lookahead.next();
    }
    match first_nontrivia_on_same_line(&mut lookahead) {
        Some(next) if next == u32::from(b'~') => lookahead.peek() != Some(&u32::from(b'>')),
        Some(BACKTICK | LEFT_BRACKET) => true,
        Some(next) if next == UNDERSCORE || is_identifier_start(next) => {
            let Some(identifier) = scan_lookahead_identifier_after_first(next, &mut lookahead)
            else {
                return false;
            };
            !identifier.is(b"as") && !identifier.is(b"is")
        }
        Some(_) | None => false,
    }
}

fn starts_using_declaration(input: &InputStream) -> bool {
    const USING: &[u8] = b"using";
    if !starts_exact_ascii_identifier(input, USING) {
        return false;
    }
    let mut lookahead = input.lookahead().map(CodePoint::as_u32).peekable();
    for _ in USING {
        lookahead.next();
    }
    first_nontrivia_on_same_line(&mut lookahead)
        .is_some_and(|next| matches!(next, AT_SIGN | BACKTICK) || is_identifier_start(next))
}

fn scan_type_path_lookahead(input: &mut InputStream, stack: &Stack) -> Result<(), ParseError> {
    if current(input) == Some(u32::from(b'A'))
        && stack.can_shift(terms::qualifiedDeclTypeLookahead)
        && starts_exact_ascii_identifier(input, b"Any")
    {
        return input.accept_token(terms::qualifiedDeclTypeLookahead);
    }
    if let Some(term) = type_path_lookahead_term(input, stack) {
        input.accept_token_to(term, input.mark())?;
    }
    Ok(())
}

fn scan_accessor_lookahead(input: &mut InputStream, stack: &Stack) -> Result<(), ParseError> {
    let first = current(input);
    if first == Some(LEFT_BRACE) && accessor::scan_initialized_property_block(input, stack)? {
        return Ok(());
    }
    if first.is_some_and(accessor::is_block_start) && accessor::scan_block(input, stack)? {
        return Ok(());
    }
    Ok(())
}

fn type_path_lookahead_term(input: &InputStream, stack: &Stack) -> Option<u16> {
    let first = current(input)?;
    match first {
        LEFT_ANGLE => (stack.can_shift(terms::keyPathGenericArgumentLookahead)
            && generic::starts_clause(input, generic::ArgumentContext::Type))
        .then_some(terms::keyPathGenericArgumentLookahead),
        PERIOD => type_path_period_lookahead(input, stack),
        _ => None,
    }
}

fn type_path_period_lookahead(input: &InputStream, stack: &Stack) -> Option<u16> {
    if stack.can_shift(terms::qualifiedDeclTypeLookahead) {
        let mut lookahead = input.lookahead().map(CodePoint::as_u32).peekable();
        if type_lookahead::starts_qualified_decl_type_member(&mut lookahead) {
            // This term preceded the type-path group before the callbacks were
            // combined, so an accepted qualified prefix must still win.
            return Some(terms::qualifiedDeclTypeLookahead);
        }
    }

    let mut lookahead = input.lookahead().map(CodePoint::as_u32).peekable();
    if starts_dotted_key_path_optional_component(&mut lookahead)
        && stack.can_shift(terms::keyPathDottedOptionalComponentLookahead)
    {
        return Some(terms::keyPathDottedOptionalComponentLookahead);
    }

    None
}

fn scan_literal(input: &mut InputStream, stack: &Stack) -> Result<(), ParseError> {
    literal::scan_literal(input, stack).map(|_| ())
}

fn scan_generic_value_lookahead(input: &mut InputStream, stack: &Stack) -> Result<(), ParseError> {
    generic::scan_value_lookahead(input, stack).map(|_| ())
}

fn scan_syntax_lookahead_token(input: &mut InputStream, stack: &Stack) -> Result<(), ParseError> {
    let Some(first) = current(input) else {
        return Ok(());
    };
    scan_syntax_lookahead(input, stack, first).map(|_| ())
}

fn scan_operator(input: &mut InputStream, stack: &Stack) -> Result<(), ParseError> {
    let Some(first) = current(input) else {
        return Ok(());
    };
    if first == QUESTION && operator::scan_question_mark(input, stack)? {
        return Ok(());
    }
    if !is_operator_start(first) {
        return Ok(());
    }
    operator::scan(input, stack, first)
}

fn scan_syntax_lookahead(
    input: &mut InputStream,
    stack: &Stack,
    first: u32,
) -> Result<bool, ParseError> {
    if first == u32::from(b'a')
        && stack.can_shift(terms::contextualAnyTypeLookahead)
        && starts_contextual_any_type(input)
    {
        input.accept_token(terms::contextualAnyTypeLookahead)?;
        return Ok(true);
    }
    if first == u32::from(b'u')
        && stack.can_shift(terms::usingDeclarationLookahead)
        && starts_using_declaration(input)
    {
        input.accept_token(terms::usingDeclarationLookahead)?;
        return Ok(true);
    }
    if first == LEFT_PAREN && stack.can_shift(terms::declNameArgumentsLookahead) {
        let starts_arguments = {
            let mut lookahead = input.lookahead().map(CodePoint::as_u32).peekable();
            name::starts_decl_name_arguments(&mut lookahead)
        };
        if starts_arguments {
            input.accept_token_to(terms::declNameArgumentsLookahead, input.mark())?;
            return Ok(true);
        }
    }
    if let Some(term) = condition_lookahead_term(input, stack, first) {
        input.accept_token(term)?;
        return Ok(true);
    }
    if first == PERIOD {
        let starts_member = {
            let mut lookahead = input.lookahead().map(CodePoint::as_u32).peekable();
            lookahead.next();
            starts_member_access_continuation(&mut lookahead)
        };
        if starts_member && stack.can_shift(terms::postfixIfConfigClauseLookahead) {
            input.accept_token_to(terms::postfixIfConfigClauseLookahead, input.mark())?;
            return Ok(true);
        }
        if starts_member && stack.can_shift(terms::postfixMemberLookahead) {
            input.accept_token_to(terms::postfixMemberLookahead, input.mark())?;
            return Ok(true);
        }
    }
    if starts_type_lookahead(input, first) {
        type_lookahead::scan(input, stack)?;
        return Ok(true);
    }
    if first == LEFT_ANGLE
        && stack.can_shift(terms::genericArgumentLookahead)
        && generic::starts_clause(input, generic::ArgumentContext::Expression)
    {
        input.accept_token_to(terms::genericArgumentLookahead, input.mark())?;
        return Ok(true);
    }
    Ok(false)
}

fn starts_type_lookahead(input: &InputStream, first: u32) -> bool {
    matches!(first, LEFT_PAREN | AT_SIGN)
        || (first == u32::from(b'i') && starts_exact_ascii_identifier(input, b"inout"))
        || (first == u32::from(b'n') && starts_exact_ascii_identifier(input, b"nonisolated"))
}

fn condition_lookahead_term(input: &InputStream, stack: &Stack, first: u32) -> Option<u16> {
    let starts_list_end = first == LEFT_BRACE
        || (first == u32::from(b'e') && starts_exact_ascii_identifier(input, b"else"));
    let can_end_list = starts_list_end && stack.can_shift(terms::conditionListEndLookahead);
    if can_end_list && starts_list_end && condition::list_ends_here(input) {
        return Some(terms::conditionListEndLookahead);
    }
    if first == LEFT_BRACE
        && stack.can_shift(terms::conditionTrailingClosureLookahead)
        && condition::allows_trailing_closure(input)
    {
        return Some(terms::conditionTrailingClosureLookahead);
    }
    None
}

fn starts_dotted_key_path_optional_component(
    input: &mut std::iter::Peekable<impl Iterator<Item = u32>>,
) -> bool {
    input.next() == Some(PERIOD) && matches!(input.next(), Some(QUESTION | EXCLAMATION))
}

fn scan_attribute(input: &mut InputStream, stack: &Stack) -> Result<(), ParseError> {
    match current(input) {
        Some(AT_SIGN) => {
            if stack.can_shift(terms::switchCaseAttributeLookahead)
                && switch_case::starts_attribute(input)
            {
                input.accept_token(terms::switchCaseAttributeLookahead)?;
                return Ok(());
            }
            attribute::scan_name(input, stack)
        }
        Some(POUND) => {
            if scan_switch_case_if_config_lookahead(input, stack)? {
                return Ok(());
            }
            if scan_switch_case_diagnostic_lookahead(input, stack)? {
                return Ok(());
            }
            if peek(input, 1) == Some(u32::from(b's'))
                && starts_exact_ascii_identifier(input, b"#sourceLocation")
                && stack.can_shift(terms::poundSourceLocation)
            {
                input.advance("#sourceLocation".len());
                input.accept_token(terms::poundSourceLocation)?;
                return Ok(());
            }
            attribute::scan_if_config_lookahead(input, stack)
        }
        _ => Ok(()),
    }
}

fn scan_switch_case_if_config_lookahead(
    input: &mut InputStream,
    stack: &Stack,
) -> Result<bool, ParseError> {
    let can_shift_list = stack.can_shift(terms::switchCaseIfConfigLookahead);
    let can_shift_body = stack.can_shift(terms::switchCaseBodyIfConfigLookahead);
    if !can_shift_list && !can_shift_body {
        return Ok(false);
    }

    if !switch_case::starts_if_config(input) {
        return Ok(false);
    }

    let belongs_to_list = if can_shift_list && can_shift_body {
        switch_case::if_config_belongs_to_list(input)
    } else {
        can_shift_list
    };
    let term = if belongs_to_list {
        terms::switchCaseIfConfigLookahead
    } else {
        terms::switchCaseBodyIfConfigLookahead
    };
    input.accept_token(term)?;
    Ok(true)
}

fn scan_switch_case_diagnostic_lookahead(
    input: &mut InputStream,
    stack: &Stack,
) -> Result<bool, ParseError> {
    let can_shift_list = stack.can_shift(terms::switchCaseDiagnosticLookahead);
    let can_shift_body = stack.can_shift(terms::switchCaseBodyDiagnosticLookahead);
    if !can_shift_list && !can_shift_body {
        return Ok(false);
    }

    if !switch_case::starts_diagnostic(input) {
        return Ok(false);
    }

    // SwiftSyntax keeps a diagnostic after a case label in that case's body.
    // The list marker is selected only when no body entry is available, such
    // as at the start of the switch or after a list-level `#if` declaration.
    let term = if can_shift_body {
        terms::switchCaseBodyDiagnosticLookahead
    } else {
        terms::switchCaseDiagnosticLookahead
    };
    input.accept_token(term)?;
    Ok(true)
}

fn scan_code_item_separator(input: &mut InputStream, stack: &Stack) -> Result<(), ParseError> {
    let stop_after_line_break = lookahead::previous(input) == Some(COMMA);
    let boundary =
        layout::classify_code_item_boundary(input, stop_after_line_break, |role| match role {
            layout::ShiftRole::DeclarationEffect => {
                stack.can_shift(terms::declarationEffectLineLookahead)
            }
            layout::ShiftRole::EnumCaseParameter => {
                stack.can_shift(terms::enumCaseParameterLineLookahead)
            }
            layout::ShiftRole::PostfixIfConfig => stack.can_shift(terms::postfixIfConfigLookahead),
            layout::ShiftRole::PostfixMember => stack.can_shift(terms::postfixMemberLookahead),
            layout::ShiftRole::BinaryOperator => stack.can_shift(terms::binaryCustomOperator),
            layout::ShiftRole::AsKeyword => stack.can_shift(terms::_as),
            layout::ShiftRole::IsKeyword => stack.can_shift(terms::is),
        });
    match boundary {
        layout::CodeItemBoundary::DeclarationEffect => {
            input.accept_token(terms::declarationEffectLineLookahead)?;
        }
        layout::CodeItemBoundary::EnumCaseParameter => {
            input.accept_token(terms::enumCaseParameterLineLookahead)?;
        }
        layout::CodeItemBoundary::PostfixIfConfig => {
            input.accept_token(terms::postfixIfConfigLookahead)?;
        }
        layout::CodeItemBoundary::LineBreak => input.accept_token(terms::lineBreak)?,
        layout::CodeItemBoundary::None => {}
    }
    Ok(())
}

fn scan_generic_requirement_continuation(
    input: &mut InputStream,
    _stack: &Stack,
) -> Result<(), ParseError> {
    // `whitespace` combines physical newlines with surrounding indentation,
    // so mark the continuation before the ordinary skip token consumes it.
    // The grammar can shift this zero-width token only once after a comma and
    // then requires another GenericRequirement, which guarantees progress.
    if layout::has_line_break(input) {
        input.accept_token(terms::genericRequirementContinuation)?;
    }
    Ok(())
}

fn scan_catch_pattern_context(input: &mut InputStream, _stack: &Stack) -> Result<(), ParseError> {
    catch::scan(input)
}

fn scan_closure_signature_context(
    input: &mut InputStream,
    stack: &Stack,
) -> Result<(), ParseError> {
    closure::scan(input, stack)
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::generic::{ArgumentContext, starts_clause_from};
    use super::{OperatorFixity, next_nontrivia, operator_has_fixed_role};

    fn counted_code_points<'a>(
        source: &'a str,
        inspected: &'a Cell<usize>,
    ) -> impl Iterator<Item = u32> + 'a {
        source.chars().map(u32::from).inspect(move |_| {
            inspected.set(inspected.get() + 1);
        })
    }

    #[test]
    fn rejected_generic_arguments_do_not_scan_the_remaining_source() {
        for prefix in ["< 0 {", "< value {"] {
            let source = format!("{prefix}{}", " tail".repeat(4_096));
            let consumed = Cell::new(0usize);
            let input = counted_code_points(&source, &consumed);
            assert!(!starts_clause_from(
                &mut input.peekable(),
                ArgumentContext::Expression,
            ));
            assert!(
                consumed.get() < 32,
                "{prefix}: inspected {} code points",
                consumed.get()
            );
        }
    }

    #[test]
    fn deep_structural_lookaheads_do_not_scan_the_remaining_source() {
        const DEPTH: usize = 128;
        const ALLOWED_BOUNDARY_READS: usize = 2;

        let unrelated_tail = " sentinel".repeat(4_096);

        let type_prefix = format!("{}Int{}", "[".repeat(DEPTH), "]".repeat(DEPTH));
        let type_source = format!("{type_prefix}{unrelated_tail}");
        let type_inspected = Cell::new(0usize);
        let type_input = counted_code_points(&type_source, &type_inspected);
        assert!(super::type_lookahead::parse(&mut type_input.peekable()));
        assert!(
            type_inspected.get() <= type_prefix.len() + ALLOWED_BOUNDARY_READS,
            "nested type inspected {} code points for a {}-point prefix",
            type_inspected.get(),
            type_prefix.len(),
        );
    }

    fn first_nontrivia(source: &str) -> Option<char> {
        next_nontrivia(&mut source.chars().map(u32::from).peekable()).and_then(char::from_u32)
    }

    fn has_fixed_operator_role(source: &str, fixity: OperatorFixity) -> bool {
        let mut prefix = [0_u32; 3];
        let mut length = 0_usize;
        for character in source.chars() {
            if length < prefix.len() {
                prefix[length] = u32::from(character);
            }
            length += 1;
        }
        operator_has_fixed_role(prefix, length, fixity)
    }

    #[test]
    fn trivia_cursor_preserves_noncomment_slashes() {
        let cases = [
            ("plain slash", "\n/ 1", Some('/')),
            ("regex-shaped slash", "\n/^ x/", Some('/')),
            ("line comment", "\n// comment\nvalue", Some('v')),
            (
                "nested block comment",
                "/* outer /* inner */ */value",
                Some('v'),
            ),
            ("only trivia", " // comment", None),
        ];

        for (name, source, expected) in cases {
            assert_eq!(first_nontrivia(source), expected, "{name}: {source:?}");
        }
    }

    #[test]
    fn fixed_operator_roles_do_not_mask_other_fixities() {
        let cases = [
            ("+", OperatorFixity::Prefix, true),
            ("+", OperatorFixity::Binary, true),
            ("+", OperatorFixity::Postfix, false),
            ("/", OperatorFixity::Prefix, false),
            ("/", OperatorFixity::Binary, true),
            ("/", OperatorFixity::Postfix, false),
            ("!", OperatorFixity::Prefix, true),
            ("!", OperatorFixity::Binary, false),
            ("!", OperatorFixity::Postfix, true),
            ("=", OperatorFixity::Prefix, true),
            ("=", OperatorFixity::Binary, true),
            ("=", OperatorFixity::Postfix, true),
            ("->", OperatorFixity::Prefix, true),
            ("*/", OperatorFixity::Binary, true),
            ("^", OperatorFixity::Binary, false),
        ];

        for (source, fixity, expected) in cases {
            assert_eq!(
                has_fixed_operator_role(source, fixity),
                expected,
                "{source}"
            );
        }
    }
}
