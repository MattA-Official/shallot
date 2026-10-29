//! Tokens and the WGSL lexer.

use crate::{Error, Span};

#[derive(Debug, Clone, PartialEq)]
pub enum Token {
    Ident(String),
    Int(i64, IntSuffix),
    Float(f64, FloatSuffix),
    Punct(&'static str),
    Eof,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntSuffix {
    None,
    I,
    U,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FloatSuffix {
    None,
    F,
    H,
}

/// Longest first, so that `>>=` wins over `>>` and `>`.
const PUNCTUATION: &[&str] = &[
    ">>=", "<<=", "->", "&&", "||", "==", "!=", ">=", "<=", ">>", "<<", "++", "--", "+=", "-=",
    "*=", "/=", "%=", "&=", "|=", "^=", "&", "@", "/", "!", "[", "]", "{", "}", ":", ",", "=", ">",
    "<", "%", "-", ".", "+", "|", "(", ")", ";", "*", "~", "^",
];

pub fn tokenize(source: &str) -> Result<Vec<(Token, Span)>, Error> {
    let bytes = source.as_bytes();
    let mut tokens = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        if c.is_ascii_whitespace() {
            i += 1;
            continue;
        }
        if source[i..].starts_with("//") {
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        if source[i..].starts_with("/*") {
            let start = i;
            let mut depth = 0;
            while i < bytes.len() {
                if source[i..].starts_with("/*") {
                    depth += 1;
                    i += 2;
                } else if source[i..].starts_with("*/") {
                    depth -= 1;
                    i += 2;
                    if depth == 0 {
                        break;
                    }
                } else {
                    i += 1;
                }
            }
            if depth != 0 {
                return Err(Error::new(
                    "unterminated block comment",
                    Span::new(start, i),
                ));
            }
            continue;
        }
        let start = i;
        if c.is_ascii_alphabetic() || c == b'_' || c >= 0x80 {
            while i < bytes.len()
                && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_' || bytes[i] >= 0x80)
            {
                i += 1;
            }
            tokens.push((
                Token::Ident(source[start..i].to_string()),
                Span::new(start, i),
            ));
            continue;
        }
        if c.is_ascii_digit() || (c == b'.' && bytes.get(i + 1).is_some_and(u8::is_ascii_digit)) {
            let (token, end) = number(source, start)?;
            tokens.push((token, Span::new(start, end)));
            i = end;
            continue;
        }
        match PUNCTUATION.iter().find(|p| source[i..].starts_with(**p)) {
            Some(p) => {
                i += p.len();
                tokens.push((Token::Punct(p), Span::new(start, i)));
            }
            None => {
                let ch = source[i..].chars().next().unwrap();
                return Err(Error::new(
                    format!("unexpected character {ch:?}"),
                    Span::new(start, start + ch.len_utf8()),
                ));
            }
        }
    }
    tokens.push((Token::Eof, Span::new(source.len(), source.len())));
    Ok(tokens)
}

fn number(source: &str, start: usize) -> Result<(Token, usize), Error> {
    let bytes = source.as_bytes();
    let at = |i: usize| bytes.get(i).copied().unwrap_or(0);
    let mut i = start;

    if at(i) == b'0' && (at(i + 1) == b'x' || at(i + 1) == b'X') {
        i += 2;
        let digits_start = i;
        while at(i).is_ascii_hexdigit() {
            i += 1;
        }
        let int_end = i;
        let mut is_float = false;
        if at(i) == b'.' {
            is_float = true;
            i += 1;
            while at(i).is_ascii_hexdigit() {
                i += 1;
            }
        }
        if at(i) == b'p' || at(i) == b'P' {
            is_float = true;
            i += 1;
            if at(i) == b'+' || at(i) == b'-' {
                i += 1;
            }
            while at(i).is_ascii_digit() {
                i += 1;
            }
        }
        if is_float {
            let value = parse_hex_float(&source[digits_start..i])
                .ok_or_else(|| Error::new("invalid hex float literal", Span::new(start, i)))?;
            let (suffix, end) = float_suffix(bytes, i);
            return Ok((Token::Float(value, suffix), end));
        }
        let value = i64::from_str_radix(&source[digits_start..int_end], 16)
            .map_err(|_| Error::new("integer literal out of range", Span::new(start, i)))?;
        let (suffix, end) = int_suffix(bytes, i);
        return Ok((Token::Int(value, suffix), end));
    }

    while at(i).is_ascii_digit() {
        i += 1;
    }
    let mut is_float = false;
    if at(i) == b'.' {
        is_float = true;
        i += 1;
        while at(i).is_ascii_digit() {
            i += 1;
        }
    }
    if (at(i) == b'e' || at(i) == b'E')
        && (at(i + 1).is_ascii_digit()
            || ((at(i + 1) == b'+' || at(i + 1) == b'-') && at(i + 2).is_ascii_digit()))
    {
        is_float = true;
        i += 2;
        while at(i).is_ascii_digit() {
            i += 1;
        }
    }
    let text = &source[start..i];
    if !is_float && (at(i) == b'f' || at(i) == b'h') {
        is_float = true;
    }
    if is_float {
        let value: f64 = text
            .parse()
            .map_err(|_| Error::new("invalid float literal", Span::new(start, i)))?;
        let (suffix, end) = float_suffix(bytes, i);
        return Ok((Token::Float(value, suffix), end));
    }
    if text.len() > 1 && text.starts_with('0') {
        return Err(Error::new(
            "integer literals cannot have leading zeros",
            Span::new(start, i),
        ));
    }
    let value: i64 = text
        .parse()
        .map_err(|_| Error::new("integer literal out of range", Span::new(start, i)))?;
    let (suffix, end) = int_suffix(bytes, i);
    Ok((Token::Int(value, suffix), end))
}

fn int_suffix(bytes: &[u8], i: usize) -> (IntSuffix, usize) {
    match bytes.get(i) {
        Some(b'i') if !continues_ident(bytes, i + 1) => (IntSuffix::I, i + 1),
        Some(b'u') if !continues_ident(bytes, i + 1) => (IntSuffix::U, i + 1),
        _ => (IntSuffix::None, i),
    }
}

fn float_suffix(bytes: &[u8], i: usize) -> (FloatSuffix, usize) {
    match bytes.get(i) {
        Some(b'f') if !continues_ident(bytes, i + 1) => (FloatSuffix::F, i + 1),
        Some(b'h') if !continues_ident(bytes, i + 1) => (FloatSuffix::H, i + 1),
        _ => (FloatSuffix::None, i),
    }
}

fn continues_ident(bytes: &[u8], i: usize) -> bool {
    bytes
        .get(i)
        .is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'_')
}

fn parse_hex_float(text: &str) -> Option<f64> {
    let (mantissa, exponent) = match text.find(['p', 'P']) {
        Some(p) => (&text[..p], text[p + 1..].parse::<i32>().ok()?),
        None => (text, 0),
    };
    let (int_part, frac_part) = match mantissa.find('.') {
        Some(d) => (&mantissa[..d], &mantissa[d + 1..]),
        None => (mantissa, ""),
    };
    let mut value = 0.0f64;
    for c in int_part.chars() {
        value = value * 16.0 + c.to_digit(16)? as f64;
    }
    let mut scale = 1.0 / 16.0;
    for c in frac_part.chars() {
        value += c.to_digit(16)? as f64 * scale;
        scale /= 16.0;
    }
    Some(value * 2f64.powi(exponent))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(source: &str) -> Vec<Token> {
        tokenize(source)
            .unwrap()
            .into_iter()
            .map(|(t, _)| t)
            .collect()
    }

    #[test]
    fn literals() {
        assert_eq!(
            kinds("1 2u 3i 0x10 1.5 1f .5 1e3 2.5h 0x1p4"),
            vec![
                Token::Int(1, IntSuffix::None),
                Token::Int(2, IntSuffix::U),
                Token::Int(3, IntSuffix::I),
                Token::Int(16, IntSuffix::None),
                Token::Float(1.5, FloatSuffix::None),
                Token::Float(1.0, FloatSuffix::F),
                Token::Float(0.5, FloatSuffix::None),
                Token::Float(1000.0, FloatSuffix::None),
                Token::Float(2.5, FloatSuffix::H),
                Token::Float(16.0, FloatSuffix::None),
                Token::Eof,
            ]
        );
    }

    #[test]
    fn punctuation_prefers_longest() {
        assert_eq!(
            kinds("a>>=b->c"),
            vec![
                Token::Ident("a".into()),
                Token::Punct(">>="),
                Token::Ident("b".into()),
                Token::Punct("->"),
                Token::Ident("c".into()),
                Token::Eof,
            ]
        );
    }

    #[test]
    fn nested_comments() {
        assert_eq!(kinds("a /* x /* y */ z */ b // c\n d").len(), 4);
        assert!(tokenize("/* open").is_err());
    }
}
