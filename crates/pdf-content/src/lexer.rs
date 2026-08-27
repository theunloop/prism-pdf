//! Content-stream tokenizer (ISO 32000-1 §7.8.2 / §7.2).
//!
//! A content stream uses the same lexical grammar as the file body (§7.2) — white space,
//! delimiters, numbers, strings, names, array/dict brackets — but its bare keyword runs are
//! *operators* (`Tj`, `BT`, `re`, …) rather than `obj`/`R`, and there are no indirect references.
//! The content layer cannot depend on the reader (architecture: `content → cos`), so this is its
//! own tokenizer rather than a reuse of `pdf-reader`'s — but the §7.2.2 character classes it
//! shares with the reader are normative, so both take them from [`pdf_cos::syntax`].
//!
//! It is total on any input (DESIGN.md §3.4): every method returns rather than panicking, and a
//! lexical error never advances zero bytes, so callers can resynchronise.

use pdf_cos::syntax::{hex_value, is_delimiter, is_regular, is_whitespace};

/// A content-stream token (§7.2/§7.8.2). String and name payloads are decoded to canonical bytes.
#[derive(Clone, PartialEq, Debug)]
pub enum Token {
    /// An integer (§7.3.3).
    Integer(i64),
    /// A real number (§7.3.3).
    Real(f64),
    /// A string operand (§7.3.4), decoded from literal or hex syntax.
    String(Vec<u8>),
    /// A name operand (§7.3.5), without the leading solidus.
    Name(Vec<u8>),
    /// `[` — array start.
    ArrayOpen,
    /// `]` — array end.
    ArrayClose,
    /// `<<` — dictionary start.
    DictOpen,
    /// `>>` — dictionary end.
    DictClose,
    /// A bare keyword: an operator, or the operand keywords `true`/`false`/`null`.
    Keyword(Vec<u8>),
}

/// A lexical error in a content stream. Carries no detail: the parser's only response is to skip
/// a byte and resynchronise (content parsing is lenient, §7.8.2).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct LexError;

/// A cursor over content-stream bytes producing [`Token`]s.
#[derive(Clone, Debug)]
pub struct Lexer<'a> {
    input: &'a [u8],
    pos: usize,
}

impl<'a> Lexer<'a> {
    /// Create a lexer over `input`.
    #[must_use]
    pub fn new(input: &'a [u8]) -> Self {
        Self { input, pos: 0 }
    }

    /// The current byte offset.
    #[must_use]
    pub fn offset(&self) -> usize {
        self.pos
    }

    /// Reposition the cursor (clamped), e.g. to resynchronise after a lexical error.
    pub fn set_offset(&mut self, pos: usize) {
        self.pos = pos.min(self.input.len());
    }

    fn peek(&self) -> Option<u8> {
        self.input.get(self.pos).copied()
    }

    fn peek_next(&self) -> Option<u8> {
        self.input.get(self.pos + 1).copied()
    }

    fn skip_whitespace_and_comments(&mut self) {
        while let Some(b) = self.peek() {
            if is_whitespace(b) {
                self.pos += 1;
            } else if b == b'%' {
                self.pos += 1;
                while let Some(c) = self.peek() {
                    if c == b'\n' || c == b'\r' {
                        break;
                    }
                    self.pos += 1;
                }
            } else {
                break;
            }
        }
    }

    /// After an `ID` operator, read an inline image's raw sample bytes up to the delimited `EI`
    /// (§8.9.7), consuming `EI`. Skips the single whitespace separator that follows `ID`, and returns
    /// the data between it and `EI`. Best-effort: stops at the first `EI` bounded by white space, or
    /// at end of input. The binary body is never tokenised.
    pub fn read_inline_image_data(&mut self) -> Vec<u8> {
        // Exactly one whitespace byte separates ID from the sample data.
        if let Some(&b) = self.input.get(self.pos)
            && is_whitespace(b)
        {
            self.pos += 1;
        }
        let start = self.pos;
        while self.pos + 2 <= self.input.len() {
            if &self.input[self.pos..self.pos + 2] == b"EI" {
                let before_ok = self.pos == 0 || is_whitespace(self.input[self.pos - 1]);
                let after_ok = match self.input.get(self.pos + 2) {
                    None => true,
                    Some(&b) => is_whitespace(b) || is_delimiter(b),
                };
                if before_ok && after_ok {
                    // `EI` must be preceded by white space (§8.9.7) — that one byte is the
                    // delimiter, not sample data, so drop it from the captured body.
                    let mut end = self.pos;
                    if end > start && is_whitespace(self.input[end - 1]) {
                        end -= 1;
                    }
                    let data = self.input[start..end].to_vec();
                    self.pos += 2;
                    return data;
                }
            }
            self.pos += 1;
        }
        let data = self.input[start..].to_vec();
        self.pos = self.input.len();
        data
    }

    /// Produce the next token, `Ok(None)` at end of input, or `Err(LexError)` on a lexical error (the
    /// caller resynchronises). Never panics.
    pub fn next_token(&mut self) -> Result<Option<Token>, LexError> {
        self.skip_whitespace_and_comments();
        let Some(b) = self.peek() else {
            return Ok(None);
        };
        let token = match b {
            b'(' => self.lex_literal_string()?,
            b'<' => {
                if self.peek_next() == Some(b'<') {
                    self.pos += 2;
                    Token::DictOpen
                } else {
                    self.lex_hex_string()?
                }
            }
            b'>' => {
                if self.peek_next() == Some(b'>') {
                    self.pos += 2;
                    Token::DictClose
                } else {
                    return Err(LexError);
                }
            }
            b'[' => {
                self.pos += 1;
                Token::ArrayOpen
            }
            b']' => {
                self.pos += 1;
                Token::ArrayClose
            }
            b'/' => self.lex_name(),
            b')' | b'{' | b'}' => return Err(LexError),
            _ => self.lex_regular_run(),
        };
        Ok(Some(token))
    }

    fn lex_literal_string(&mut self) -> Result<Token, LexError> {
        self.pos += 1; // consume `(`
        let mut out = Vec::new();
        let mut depth: u32 = 1;
        while let Some(b) = self.peek() {
            self.pos += 1;
            match b {
                b'\\' => {
                    let Some(esc) = self.peek() else {
                        return Err(LexError);
                    };
                    self.pos += 1;
                    match esc {
                        b'n' => out.push(b'\n'),
                        b'r' => out.push(b'\r'),
                        b't' => out.push(b'\t'),
                        b'b' => out.push(0x08),
                        b'f' => out.push(0x0C),
                        b'(' => out.push(b'('),
                        b')' => out.push(b')'),
                        b'\\' => out.push(b'\\'),
                        b'\r' => {
                            if self.peek() == Some(b'\n') {
                                self.pos += 1;
                            }
                        }
                        b'\n' => {}
                        b'0'..=b'7' => {
                            let mut val = u16::from(esc - b'0');
                            for _ in 0..2 {
                                match self.peek() {
                                    Some(d @ b'0'..=b'7') => {
                                        self.pos += 1;
                                        val = val * 8 + u16::from(d - b'0');
                                    }
                                    _ => break,
                                }
                            }
                            out.push((val & 0xFF) as u8);
                        }
                        other => out.push(other),
                    }
                }
                b'(' => {
                    depth += 1;
                    out.push(b'(');
                }
                b')' => {
                    depth -= 1;
                    if depth == 0 {
                        return Ok(Token::String(out));
                    }
                    out.push(b')');
                }
                b'\r' => {
                    if self.peek() == Some(b'\n') {
                        self.pos += 1;
                    }
                    out.push(b'\n');
                }
                _ => out.push(b),
            }
        }
        Err(LexError)
    }

    fn lex_hex_string(&mut self) -> Result<Token, LexError> {
        self.pos += 1; // consume `<`
        let mut out = Vec::new();
        let mut hi: Option<u8> = None;
        while let Some(b) = self.peek() {
            self.pos += 1;
            if b == b'>' {
                if let Some(h) = hi {
                    out.push(h << 4);
                }
                return Ok(Token::String(out));
            }
            if is_whitespace(b) {
                continue;
            }
            let Some(v) = hex_value(b) else {
                return Err(LexError);
            };
            match hi.take() {
                None => hi = Some(v),
                Some(h) => out.push((h << 4) | v),
            }
        }
        Err(LexError)
    }

    fn lex_name(&mut self) -> Token {
        self.pos += 1; // consume `/`
        let mut out = Vec::new();
        while let Some(b) = self.peek() {
            if !is_regular(b) {
                break;
            }
            self.pos += 1;
            if b == b'#' {
                match (
                    self.peek().and_then(hex_value),
                    self.peek_next().and_then(hex_value),
                ) {
                    (Some(h), Some(l)) => {
                        self.pos += 2;
                        out.push((h << 4) | l);
                    }
                    _ => out.push(b'#'),
                }
            } else {
                out.push(b);
            }
        }
        Token::Name(out)
    }

    fn lex_regular_run(&mut self) -> Token {
        let start = self.pos;
        while let Some(b) = self.peek() {
            if !is_regular(b) {
                break;
            }
            self.pos += 1;
        }
        let run = &self.input[start..self.pos];
        classify_run(run)
    }
}

/// Classify a regular-character run as a number or a keyword (operator/`true`/`false`/`null`).
fn classify_run(run: &[u8]) -> Token {
    let first = run[0];
    let looks_numeric = matches!(first, b'+' | b'-' | b'.' | b'0'..=b'9');
    if looks_numeric {
        let mut seen_dot = false;
        let mut seen_digit = false;
        let mut well_formed = true;
        for (i, &b) in run.iter().enumerate() {
            match b {
                b'+' | b'-' if i == 0 => {}
                b'.' if !seen_dot => seen_dot = true,
                b'0'..=b'9' => seen_digit = true,
                _ => {
                    well_formed = false;
                    break;
                }
            }
        }
        if well_formed
            && seen_digit
            && let Ok(text) = std::str::from_utf8(run)
        {
            if !seen_dot && let Ok(i) = text.parse::<i64>() {
                return Token::Integer(i);
            }
            if let Ok(r) = text.parse::<f64>() {
                return Token::Real(r);
            }
        }
    }
    Token::Keyword(run.to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lex_all(input: &[u8]) -> Vec<Token> {
        let mut lexer = Lexer::new(input);
        let mut out = Vec::new();
        while let Ok(Some(tok)) = lexer.next_token() {
            out.push(tok);
        }
        out
    }

    #[test]
    fn tokenises_a_text_object() {
        // §9.4: `BT /F1 12 Tf (Hi) Tj ET`.
        assert_eq!(
            lex_all(b"BT /F1 12 Tf (Hi) Tj ET"),
            vec![
                Token::Keyword(b"BT".to_vec()),
                Token::Name(b"F1".to_vec()),
                Token::Integer(12),
                Token::Keyword(b"Tf".to_vec()),
                Token::String(b"Hi".to_vec()),
                Token::Keyword(b"Tj".to_vec()),
                Token::Keyword(b"ET".to_vec()),
            ]
        );
    }

    #[test]
    fn tj_array_and_reals() {
        assert_eq!(
            lex_all(b"[(A) -250 (B)] TJ"),
            vec![
                Token::ArrayOpen,
                Token::String(b"A".to_vec()),
                Token::Integer(-250),
                Token::String(b"B".to_vec()),
                Token::ArrayClose,
                Token::Keyword(b"TJ".to_vec()),
            ]
        );
        assert_eq!(
            lex_all(b"0.5 1. -.25 g"),
            vec![
                Token::Real(0.5),
                Token::Real(1.0),
                Token::Real(-0.25),
                Token::Keyword(b"g".to_vec()),
            ]
        );
    }

    #[test]
    fn read_inline_image_data_captures_body_and_resumes() {
        // §8.9.7: BI … ID <binary> EI — the binary is captured, not tokenised, and lexing resumes.
        let mut lexer = Lexer::new(b"BI /W 2 ID \x00\xFF\x01\xFE EI Q");
        // Consume tokens up to and including the ID operator.
        loop {
            match lexer.next_token().unwrap() {
                Some(Token::Keyword(k)) if k == b"ID" => break,
                Some(_) => {}
                None => panic!("ID not found"),
            }
        }
        assert_eq!(lexer.read_inline_image_data(), b"\x00\xFF\x01\xFE");
        assert_eq!(
            lexer.next_token().unwrap(),
            Some(Token::Keyword(b"Q".to_vec()))
        );
    }

    #[test]
    fn comments_and_whitespace_are_skipped() {
        assert_eq!(
            lex_all(b"% comment\n  42 \t %trailing"),
            vec![Token::Integer(42)]
        );
        assert!(lex_all(b"   \r\n\t \x00").is_empty());
    }

    #[test]
    fn literal_string_escapes_and_eol() {
        // Named, octal, balanced parens, ignored escape, EOL normalisation, line continuation.
        assert_eq!(
            lex_all(b"(a\\n\\t\\(\\)\\\\\\101)"),
            vec![Token::String(b"a\n\t()\\A".to_vec())]
        );
        assert_eq!(lex_all(b"(x(y)z)"), vec![Token::String(b"x(y)z".to_vec())]);
        assert_eq!(lex_all(b"(\\q)"), vec![Token::String(b"q".to_vec())]);
        assert_eq!(lex_all(b"(a\r\nb)"), vec![Token::String(b"a\nb".to_vec())]);
        assert_eq!(lex_all(b"(a\\\r\nb)"), vec![Token::String(b"ab".to_vec())]);
        assert_eq!(lex_all(b"(\\b\\f)"), vec![Token::String(vec![0x08, 0x0C])]);
    }

    #[test]
    fn name_escapes_and_empty() {
        assert_eq!(lex_all(b"/A#20B"), vec![Token::Name(b"A B".to_vec())]);
        assert_eq!(lex_all(b"/"), vec![Token::Name(Vec::new())]);
        assert_eq!(lex_all(b"/a#zz"), vec![Token::Name(b"a#zz".to_vec())]);
    }

    #[test]
    fn number_classification() {
        assert_eq!(
            lex_all(b"0 +1 -2"),
            vec![Token::Integer(0), Token::Integer(1), Token::Integer(-2)]
        );
        // Malformed numeric run degrades to a keyword; a huge integer becomes a real.
        assert_eq!(lex_all(b"1.2.3"), vec![Token::Keyword(b"1.2.3".to_vec())]);
        match lex_all(b"99999999999999999999").as_slice() {
            [Token::Real(_)] => {}
            other => panic!("expected Real, got {other:?}"),
        }
    }

    #[test]
    fn lexical_errors_are_reported() {
        assert!(Lexer::new(b">").next_token().is_err()); // lone >
        assert!(Lexer::new(b")").next_token().is_err()); // stray )
        assert!(Lexer::new(b"{").next_token().is_err());
        assert!(Lexer::new(b"(unterminated").next_token().is_err());
        assert!(Lexer::new(b"<dead").next_token().is_err()); // hex EOF
        assert!(Lexer::new(b"<zz>").next_token().is_err()); // bad hex digit
        assert!(Lexer::new(b"(\\").next_token().is_err()); // escape at EOF
    }

    #[test]
    fn offset_and_set_offset() {
        let mut lexer = Lexer::new(b"abc def");
        let _ = lexer.next_token();
        assert_eq!(lexer.offset(), 3);
        lexer.set_offset(999); // clamps to len
        assert_eq!(lexer.next_token().unwrap(), None);
    }

    #[test]
    fn hex_string_odd_digit_padding() {
        assert_eq!(lex_all(b"<4>"), vec![Token::String(vec![0x40])]);
        assert_eq!(lex_all(b"<4 1>"), vec![Token::String(vec![0x41])]);
    }
}
