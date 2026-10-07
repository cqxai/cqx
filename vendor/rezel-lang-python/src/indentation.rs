use std::cmp::Ordering;

use rezel_common::{ParseError, ParseErrorKind, TextSize};

const SPACE: u32 = 32;
const TAB: u32 = 9;
const FORM_FEED: u32 = 12;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct IndentColumns {
    visual: usize,
    alternate: usize,
}

impl IndentColumns {
    pub(crate) const fn visual(self) -> usize {
        self.visual
    }

    pub(crate) fn advance(&mut self, character: u32) -> bool {
        match character {
            SPACE => {
                self.visual += 1;
                self.alternate += 1;
            }
            TAB => {
                self.visual += 8 - self.visual % 8;
                self.alternate += 1;
            }
            FORM_FEED => {
                self.visual = 0;
                self.alternate = 0;
            }
            _ => return false,
        }
        true
    }

    pub(crate) fn from_whitespace(whitespace: &str) -> Option<Self> {
        let mut columns = Self::default();
        for byte in whitespace.bytes() {
            if !columns.advance(u32::from(byte)) {
                return None;
            }
        }
        Some(columns)
    }

    fn prefix(line: &str) -> (usize, Self) {
        let mut bytes = 0;
        let mut columns = Self::default();
        for byte in line.bytes() {
            if !columns.advance(u32::from(byte)) {
                break;
            }
            bytes += 1;
        }
        (bytes, columns)
    }
}

struct IndentationLevels {
    levels: Vec<IndentColumns>,
}

impl IndentationLevels {
    fn new() -> Self {
        Self {
            levels: vec![IndentColumns::default()],
        }
    }

    fn observe(
        &mut self,
        indentation: IndentColumns,
        may_indent: bool,
        position: usize,
    ) -> Result<(), ParseError> {
        let current = *self.levels.last().expect("base indentation level");
        match indentation.visual.cmp(&current.visual) {
            Ordering::Equal => {
                if indentation.alternate != current.alternate {
                    return Err(indentation_error(
                        position,
                        "inconsistent use of tabs and spaces",
                    ));
                }
            }
            Ordering::Greater => {
                if !may_indent {
                    return Err(indentation_error(position, "unexpected indent"));
                }
                if indentation.alternate <= current.alternate {
                    return Err(indentation_error(
                        position,
                        "inconsistent use of tabs and spaces",
                    ));
                }
                self.levels.push(indentation);
            }
            Ordering::Less => {
                let Some(index) = self
                    .levels
                    .iter()
                    .rposition(|candidate| candidate.visual == indentation.visual)
                else {
                    return Err(indentation_error(
                        position,
                        "unindent does not match any outer indentation level",
                    ));
                };
                if self.levels[index].alternate != indentation.alternate {
                    return Err(indentation_error(
                        position,
                        "inconsistent use of tabs and spaces",
                    ));
                }
                self.levels.truncate(index + 1);
            }
        }
        Ok(())
    }
}

#[derive(Clone, Copy)]
enum Quote {
    Single,
    Double,
}

impl Quote {
    fn from_character(character: char) -> Option<Self> {
        match character {
            '\'' => Some(Self::Single),
            '"' => Some(Self::Double),
            _ => None,
        }
    }

    const fn character(self) -> char {
        match self {
            Self::Single => '\'',
            Self::Double => '"',
        }
    }

    const fn triple_delimiter(self) -> &'static str {
        match self {
            Self::Single => "'''",
            Self::Double => "\"\"\"",
        }
    }
}

#[derive(Clone, Copy)]
enum StringState {
    Triple(Quote),
    Continued(Quote),
}

#[derive(Default)]
struct LexicalState {
    string: Option<StringState>,
    bracket_depth: usize,
    explicitly_continued: bool,
}

impl LexicalState {
    fn at_logical_boundary(&self) -> bool {
        self.string.is_none() && self.bracket_depth == 0 && !self.explicitly_continued
    }

    fn scan(&mut self, line: &str, start: usize) -> LineScan {
        let scan = scan_physical_line(line, start, self);
        self.explicitly_continued = scan.explicit_continuation;
        scan
    }
}

struct IndentationValidator {
    levels: IndentationLevels,
    lexical: LexicalState,
    may_indent: bool,
    byte_offset: usize,
}

impl IndentationValidator {
    fn new() -> Self {
        Self {
            levels: IndentationLevels::new(),
            lexical: LexicalState::default(),
            may_indent: false,
            byte_offset: 0,
        }
    }

    fn validate_line(&mut self, physical_line: &str) -> Result<(), ParseError> {
        let line = line_without_ending(physical_line);
        let logical_start = self.lexical.at_logical_boundary();
        let (indent_bytes, indentation) = IndentColumns::prefix(line);
        let content = &line[indent_bytes..];
        let blank = content.is_empty() || content.starts_with('#');

        if logical_start && !blank {
            self.levels.observe(
                indentation,
                self.may_indent,
                self.byte_offset + indent_bytes,
            )?;
        }

        let scan_start = if logical_start { indent_bytes } else { 0 };
        let scan = self.lexical.scan(line, scan_start);
        if self.lexical.at_logical_boundary() && !blank {
            self.may_indent = scan.last_significant == Some(':');
        }
        self.byte_offset += physical_line.len();
        Ok(())
    }
}

pub(crate) fn validate(source: &str) -> Result<(), ParseError> {
    let mut validator = IndentationValidator::new();
    for physical_line in physical_lines(source) {
        validator.validate_line(physical_line)?;
    }
    Ok(())
}

fn physical_lines(mut source: &str) -> impl Iterator<Item = &str> {
    std::iter::from_fn(move || {
        if source.is_empty() {
            return None;
        }

        let bytes = source.as_bytes();
        let ending = bytes.iter().position(|byte| matches!(byte, b'\n' | b'\r'));
        let end = match ending {
            Some(index) if bytes[index] == b'\r' && bytes.get(index + 1) == Some(&b'\n') => {
                index + 2
            }
            Some(index) => index + 1,
            None => source.len(),
        };
        let (line, rest) = source.split_at(end);
        source = rest;
        Some(line)
    })
}

fn line_without_ending(physical_line: &str) -> &str {
    let line = physical_line.strip_suffix('\n').unwrap_or(physical_line);
    line.strip_suffix('\r').unwrap_or(line)
}

#[derive(Clone, Copy)]
struct LineScan {
    last_significant: Option<char>,
    explicit_continuation: bool,
}

enum QuotedLineEnd {
    Closed,
    Continued,
    Unterminated,
}

fn scan_physical_line(line: &str, start: usize, state: &mut LexicalState) -> LineScan {
    let mut cursor = start;
    let mut last_significant = None;
    let mut explicit_continuation = false;
    while cursor < line.len() {
        match state.string {
            Some(StringState::Triple(quote)) => {
                let delimiter = quote.triple_delimiter();
                if line[cursor..].starts_with(delimiter) {
                    state.string = None;
                    cursor += delimiter.len();
                    continue;
                }

                let character = line[cursor..].chars().next().expect("character boundary");
                cursor += character.len_utf8();
                if character == '\\' && cursor < line.len() {
                    let escaped = line[cursor..].chars().next().expect("character boundary");
                    cursor += escaped.len_utf8();
                }
                continue;
            }
            Some(StringState::Continued(quote)) => {
                let end = scan_quoted_line(line, &mut cursor, quote);
                match end {
                    QuotedLineEnd::Closed => {
                        state.string = None;
                        last_significant = Some(quote.character());
                    }
                    QuotedLineEnd::Continued => {}
                    QuotedLineEnd::Unterminated => state.string = None,
                }
                continue;
            }
            None => {}
        }

        let character = line[cursor..].chars().next().expect("character boundary");
        if character == '#' {
            break;
        }
        if let Some(quote) = Quote::from_character(character) {
            let delimiter = quote.triple_delimiter();
            if line[cursor..].starts_with(delimiter) {
                state.string = Some(StringState::Triple(quote));
                cursor += delimiter.len();
                continue;
            }

            cursor += character.len_utf8();
            if matches!(
                scan_quoted_line(line, &mut cursor, quote),
                QuotedLineEnd::Continued
            ) {
                state.string = Some(StringState::Continued(quote));
            }
            last_significant = Some(character);
            continue;
        }
        match character {
            '(' | '[' | '{' => state.bracket_depth += 1,
            ')' | ']' | '}' => state.bracket_depth = state.bracket_depth.saturating_sub(1),
            '\\' if line[cursor + character.len_utf8()..].trim().is_empty() => {
                explicit_continuation = true;
            }
            _ => {}
        }
        if !character.is_whitespace() {
            last_significant = Some(character);
        }
        cursor += character.len_utf8();
    }
    LineScan {
        last_significant,
        explicit_continuation,
    }
}

fn scan_quoted_line(line: &str, cursor: &mut usize, quote: Quote) -> QuotedLineEnd {
    while *cursor < line.len() {
        let character = line[*cursor..].chars().next().expect("character boundary");
        *cursor += character.len_utf8();
        if character == '\\' {
            if *cursor == line.len() {
                return QuotedLineEnd::Continued;
            }
            let escaped = line[*cursor..].chars().next().expect("character boundary");
            *cursor += escaped.len_utf8();
        } else if character == quote.character() {
            return QuotedLineEnd::Closed;
        }
    }
    QuotedLineEnd::Unterminated
}

fn indentation_error(position: usize, message: &'static str) -> ParseError {
    let position = TextSize::try_from(position).ok();
    ParseError::new(ParseErrorKind::Syntax, position, message)
}
