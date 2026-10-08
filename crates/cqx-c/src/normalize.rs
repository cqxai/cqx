//! Declaration decorations only. This is not macro expansion: expressions,
//! comments, strings and macro definitions must retain their original meaning.
use std::{borrow::Cow, ops::Range};

#[derive(Clone)]
struct Token<'a> {
    text: &'a str,
    range: Range<usize>,
}
fn identifier(s: &str) -> bool {
    s.bytes()
        .next()
        .is_some_and(|b| b.is_ascii_alphabetic() || b == b'_')
}
fn capitals(s: &str) -> bool {
    identifier(s)
        && s.bytes().any(|b| b.is_ascii_uppercase())
        && s.bytes().all(|b| !b.is_ascii_lowercase())
}
fn tokens(source: &str) -> Vec<Token<'_>> {
    let b = source.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i].is_ascii_whitespace() {
            i += 1;
            continue;
        }
        if b[i] == b'#'
            && source[..i]
                .rsplit('\n')
                .next()
                .unwrap_or("")
                .trim()
                .is_empty()
        {
            loop {
                let end = source[i..].find('\n').map(|n| i + n).unwrap_or(b.len());
                let continued = source[i..end].trim_end_matches('\r').ends_with('\\');
                i = (end + 1).min(b.len());
                if !continued || i == b.len() {
                    break;
                }
            }
            continue;
        }
        if b[i..].starts_with(b"//") {
            i = source[i..].find('\n').map(|n| i + n).unwrap_or(b.len());
            continue;
        }
        if b[i..].starts_with(b"/*") {
            i = source[i + 2..]
                .find("*/")
                .map(|n| i + n + 4)
                .unwrap_or(b.len());
            continue;
        }
        let start = i;
        let raw_prefix = ["R\"", "u8R\"", "uR\"", "UR\"", "LR\""]
            .iter()
            .find(|prefix| source[i..].starts_with(**prefix));
        if let Some(prefix) = raw_prefix {
            let delimiter_start = i + prefix.len();
            if let Some(open) = source[delimiter_start..].find('(') {
                let delimiter = &source[delimiter_start..delimiter_start + open];
                let ending = format!("){delimiter}\"");
                i = source[delimiter_start + open + 1..]
                    .find(&ending)
                    .map(|n| delimiter_start + open + 1 + n + ending.len())
                    .unwrap_or(b.len());
            } else {
                i = b.len();
            }
        } else if b[i].is_ascii_digit() {
            i += 1;
            while i < b.len()
                && (b[i].is_ascii_alphanumeric() || matches!(b[i], b'_' | b'.' | b'\''))
            {
                i += 1;
            }
        } else if matches!(b[i], b'"' | b'\'') {
            let quote = b[i];
            i += 1;
            while i < b.len() {
                if b[i] == b'\\' {
                    i = (i + 2).min(b.len());
                } else {
                    let end = b[i] == quote;
                    i += 1;
                    if end {
                        break;
                    }
                }
            }
        } else if b[i].is_ascii_alphabetic() || b[i] == b'_' {
            i += 1;
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_') {
                i += 1;
            }
        } else {
            // Keep UTF-8 intact even outside the ASCII declaration vocabulary.
            i += source[i..].chars().next().unwrap().len_utf8();
        }
        out.push(Token {
            text: &source[start..i],
            range: start..i,
        });
    }
    out
}
fn close(tokens: &[Token<'_>], start: usize, open: &str, shut: &str) -> Option<usize> {
    let mut depth = 0;
    for (i, t) in tokens.iter().enumerate().skip(start) {
        if t.text == open {
            depth += 1;
        }
        if t.text == shut {
            depth -= 1;
            if depth == 0 {
                return Some(i);
            }
        }
    }
    None
}
fn mask(bytes: &mut [u8], range: Range<usize>) {
    for b in &mut bytes[range] {
        if !matches!(*b, b'\n' | b'\r') {
            *b = b' ';
        }
    }
}

pub fn source(source: &str) -> Cow<'_, str> {
    let t = tokens(source);
    let mut masks = Vec::new();
    let mut expression_scopes = Vec::new();
    for (i, token) in t.iter().enumerate() {
        if token.text == "}" {
            expression_scopes.pop();
        }
        let expression = expression_scopes.last().copied().unwrap_or(false);
        if token.text == "{" {
            expression_scopes.push(expression || i > 0 && t[i - 1].text == ")");
        }
        if expression {
            continue;
        }
        // Conditional extern "C" braces are grammar-breaking in C and when
        // #ifdef splits a C++ linkage block. Keep everything inside the block.
        if token.text == "extern"
            && t.get(i + 1).is_some_and(|t| t.text == "\"C\"")
            && t.get(i + 2).is_some_and(|t| t.text == "{")
        {
            if let Some(end) = close(&t, i + 2, "{", "}") {
                masks.push(token.range.start..t[i + 2].range.end);
                masks.push(t[end].range.clone());
            }
        }
        // An export decoration between class/struct and its actual name.
        if capitals(token.text)
            && i > 0
            && matches!(t[i - 1].text, "class" | "struct")
            && t.get(i + 1).is_some_and(|t| identifier(t.text))
        {
            masks.push(token.range.clone());
            continue;
        }
        let boundary = i == 0
            || matches!(
                t[i - 1].text,
                ";" | "{" | "}" | "static" | "extern" | "inline"
            )
            || source[t[i - 1].range.end..token.range.start].contains('\n');
        if !boundary || !identifier(token.text) {
            continue;
        }
        // IDENT(type) name(args): retain the type argument, blank the wrapper.
        // Restrict the type vocabulary so expression calls are never rewritten.
        if !matches!(
            token.text,
            "decltype"
                | "typeof"
                | "__typeof__"
                | "_Atomic"
                | "alignas"
                | "_Alignas"
                | "__declspec"
                | "__attribute__"
        ) && t.get(i + 1).is_some_and(|t| t.text == "(")
        {
            if let Some(end) = close(&t, i + 1, "(", ")") {
                let type_arg = end > i + 2
                    && t[i + 2..end]
                        .iter()
                        .all(|t| identifier(t.text) || matches!(t.text, "*" | "&" | ":"));
                if type_arg
                    && t.get(end + 1).is_some_and(|t| identifier(t.text))
                    && t.get(end + 2).is_some_and(|t| t.text == "(")
                {
                    masks.push(token.range.clone());
                    masks.push(t[i + 1].range.clone());
                    masks.push(t[end].range.clone());
                    continue;
                }
            }
        }
        // ALL_CAPS return_type function(args). Require both type and function
        // identifiers; an uppercase return type alone is not a decoration.
        if capitals(token.text) {
            let mut names = 0;
            for next in t.iter().skip(i + 1) {
                if identifier(next.text) {
                    if !matches!(
                        next.text,
                        "const"
                            | "volatile"
                            | "restrict"
                            | "__restrict"
                            | "static"
                            | "extern"
                            | "inline"
                    ) {
                        names += 1;
                    }
                } else if next.text == "(" {
                    if names >= 2 {
                        masks.push(token.range.clone());
                    }
                    break;
                } else if !matches!(next.text, "*" | "&" | ":") {
                    break;
                }
            }
        }
    }
    if masks.is_empty() {
        return Cow::Borrowed(source);
    }
    let mut bytes = source.as_bytes().to_vec();
    for range in masks {
        mask(&mut bytes, range);
    }
    Cow::Owned(String::from_utf8(bytes).expect("masked source remains UTF-8"))
}

#[cfg(test)]
mod tests {
    #[test]
    fn qualified_uppercase_return_types_keep_their_ast_type() {
        let input = "typedef int STATUS;\nSTATUS const f(void) { return 0; }";
        for grammar in [tree_sitter_c::LANGUAGE, tree_sitter_cpp::LANGUAGE] {
            let mut parser = cqx_syntax::Parser::new();
            parser.set_language(&grammar.into()).unwrap();
            let normalized = super::source(input);
            let tree = parser.parse(normalized.as_ref(), None).unwrap();
            let function = cqx_syntax::nodes(tree.root_node())
                .into_iter()
                .find(|n| n.kind() == "function_definition")
                .unwrap();
            let ty = function
                .child_by_field_name("type")
                .expect("return type preserved");
            assert_eq!(cqx_syntax::text(&normalized, ty), "STATUS");
        }
    }
}
