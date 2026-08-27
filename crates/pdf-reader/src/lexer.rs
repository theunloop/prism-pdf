//! The PDF lexer (ISO 32000-1 §7.2): turns a byte slice into a stream of [`Token`]s.
//!
//! Scope is purely *lexical*. The lexer recognises the character classes of §7.2.2 (white space
//! and delimiters), strips comments (§7.2.4), and emits the token-level shapes of the object
//! grammar (§7.3): numbers, strings, names, the array/dictionary brackets, and bare keyword runs
//! (`obj`, `R`, `true`, `stream`, …). It does **not** assemble objects, follow references, or know
//! about the xref — that is the parser's job (§7.3, next slice).
//!
//! Leaves are decoded to their canonical byte form here, matching the COS contract (ADR-0003):
//! literal and hex strings collapse to one [`Token::String`] of decoded bytes, and `#`-escapes in
//! names (§7.3.5) are resolved into [`Token::Name`] bytes.
//!
//! Hostile input is the norm (DESIGN.md §3.4): every method is total — it returns a
//! [`ReaderError`] rather than panicking, never indexes past the end, and contains no `unwrap`.

// The §7.2.2 character classes are normative and shared with the content, filter and writer
// tokenizers, so they have exactly one definition — in `pdf-cos`.
use pdf_cos::syntax::{hex_value, is_regular, is_whitespace};

use crate::error::{ErrorKind, ReaderError, Result};

/// A single lexical token of the PDF object grammar (§7.2/§7.3).
///
/// String and name payloads are already **decoded** to their canonical bytes (ADR-0003): the
/// literal-vs-hex distinction and `#`-escapes do not survive lexing.
#[derive(Clone, PartialEq, Debug)]
pub enum Token {
    /// An integer number (§7.3.3).
    Integer(i64),
    /// A real number (§7.3.3). PDF has no exponent notation; only `[+-]?digits.digits` forms.
    Real(f64),
    /// A string (§7.3.4), decoded from either literal `(...)` or hex `<...>` syntax.
    String(Vec<u8>),
    /// A name (§7.3.5), with `#XX` escapes resolved, *without* the leading solidus.
    Name(Vec<u8>),
    /// `[` — array start (§7.3.6).
    ArrayOpen,
    /// `]` — array end (§7.3.6).
    ArrayClose,
    /// `<<` — dictionary start (§7.3.7).
    DictOpen,
    /// `>>` — dictionary end (§7.3.7).
    DictClose,
    /// A bare keyword: a run of regular characters that is not a number — `obj`, `endobj`,
    /// `stream`, `endstream`, `R`, `true`, `false`, `null`, `xref`, `trailer`, `startxref`, the
    /// `n`/`f` of xref entries, etc. The parser (§7.3) interprets it in context.
    Keyword(Vec<u8>),
}

/// A cursor over PDF bytes producing [`Token`]s (§7.2).
#[derive(Clone, Debug)]
pub struct Lexer<'a> {
    input: &'a [u8],
    pos: usize,
}

impl<'a> Lexer<'a> {
    /// Create a lexer positioned at the start of `input`.
    #[must_use]
    pub fn new(input: &'a [u8]) -> Self {
        Self { input, pos: 0 }
    }

    /// The current byte offset into the input.
    #[must_use]
    pub fn offset(&self) -> usize {
        self.pos
    }

    /// The full input slice the lexer is reading.
    #[must_use]
    pub(crate) fn input(&self) -> &'a [u8] {
        self.input
    }

    /// Reposition the cursor to an absolute byte offset, clamped to the end of input.
    ///
    /// Used by the parser to resynchronise after reading a stream body (§7.3.8) at the byte level
    /// — stream data is raw bytes, not a token sequence.
    pub(crate) fn set_offset(&mut self, pos: usize) {
        self.pos = pos.min(self.input.len());
    }

    /// The byte at the current position without consuming it.
    fn peek(&self) -> Option<u8> {
        self.input.get(self.pos).copied()
    }

    /// The byte one past the current position without consuming it.
    fn peek_next(&self) -> Option<u8> {
        self.input.get(self.pos + 1).copied()
    }

    /// Consume white space and comments (§7.2.4) until the next token or end of input.
    ///
    /// A comment runs from `%` to the next end-of-line marker (§7.2.4); the EOL itself is left for
    /// the white-space loop to absorb.
    pub fn skip_whitespace_and_comments(&mut self) {
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

    /// Produce the next token, or `Ok(None)` at end of input (after skipping white space and
    /// comments). Never panics regardless of input (DESIGN.md §3.4).
    pub fn next_token(&mut self) -> Result<Option<Token>> {
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
                    // A lone `>` is not part of the grammar (§7.2.2).
                    return Err(ReaderError::new(ErrorKind::UnexpectedByte, self.pos));
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
            b'/' => self.lex_name()?,
            // `)`, `{`, `}` are delimiters that never *open* a token in the object grammar
            // (§7.3); `{`/`}` belong only to type-4 function streams (§7.10, EPIC 8).
            b')' | b'{' | b'}' => {
                return Err(ReaderError::new(ErrorKind::UnexpectedByte, self.pos));
            }
            _ => self.lex_regular_run()?,
        };
        Ok(Some(token))
    }

    /// Lex a literal string `(...)` (§7.3.4.2), decoding escapes and normalising end-of-line
    /// markers to a single LF. The opening `(` is at the cursor.
    fn lex_literal_string(&mut self) -> Result<Token> {
        let start = self.pos;
        self.pos += 1; // consume `(`
        let mut out = Vec::new();
        let mut depth: u32 = 1; // balanced unescaped parens are part of the string

        while let Some(b) = self.peek() {
            self.pos += 1;
            match b {
                b'\\' => {
                    let Some(esc) = self.peek() else {
                        return Err(ReaderError::new(ErrorKind::UnexpectedEof, self.pos));
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
                        // `\` then EOL is a line continuation producing no bytes (§7.3.4.2);
                        // swallow a CRLF pair as one.
                        b'\r' => {
                            if self.peek() == Some(b'\n') {
                                self.pos += 1;
                            }
                        }
                        b'\n' => {}
                        // `\ddd` octal, 1–3 digits, taken mod 256 (§7.3.4.2).
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
                        // Any other escaped char: the backslash is ignored (§7.3.4.2).
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
                // A raw EOL inside a string is a single LF, regardless of CR/LF/CRLF (§7.3.4.2).
                b'\r' => {
                    if self.peek() == Some(b'\n') {
                        self.pos += 1;
                    }
                    out.push(b'\n');
                }
                _ => out.push(b),
            }
        }
        Err(ReaderError::new(ErrorKind::UnexpectedEof, start))
    }

    /// Lex a hexadecimal string `<...>` (§7.3.4.3). White space between digits is ignored; an odd
    /// final digit is paired with an implied trailing `0`. The opening `<` is at the cursor.
    fn lex_hex_string(&mut self) -> Result<Token> {
        let start = self.pos;
        self.pos += 1; // consume `<`
        let mut out = Vec::new();
        let mut hi: Option<u8> = None;

        while let Some(b) = self.peek() {
            self.pos += 1;
            if b == b'>' {
                if let Some(h) = hi {
                    out.push(h << 4); // odd digit: implied trailing 0 (§7.3.4.3)
                }
                return Ok(Token::String(out));
            }
            if is_whitespace(b) {
                continue;
            }
            let Some(v) = hex_value(b) else {
                return Err(ReaderError::new(ErrorKind::InvalidHexDigit, self.pos - 1));
            };
            match hi.take() {
                None => hi = Some(v),
                Some(h) => out.push((h << 4) | v),
            }
        }
        Err(ReaderError::new(ErrorKind::UnexpectedEof, start))
    }

    /// Lex a name (§7.3.5), resolving `#XX` two-digit hex escapes. The leading `/` is at the
    /// cursor and is not included in the result. A `#` not followed by two hex digits is kept
    /// literally (lenient on hostile input rather than erroring).
    fn lex_name(&mut self) -> Result<Token> {
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
        Ok(Token::Name(out))
    }

    /// Lex a run of regular characters and classify it as a number (§7.3.3) or a keyword.
    fn lex_regular_run(&mut self) -> Result<Token> {
        let start = self.pos;
        while let Some(b) = self.peek() {
            if !is_regular(b) {
                break;
            }
            self.pos += 1;
        }
        let run = &self.input[start..self.pos];
        classify_run(run, start)
    }
}

/// Classify a regular-character run as an [`Integer`](Token::Integer), [`Real`](Token::Real) or
/// [`Keyword`](Token::Keyword) (§7.3.3). A run that *looks* numeric but is malformed (`--`,
/// `1.2.3`) degrades to a keyword rather than erroring; an in-range integer that overflows `i64`
/// is promoted to a real, as Adobe's readers do.
fn classify_run(run: &[u8], offset: usize) -> Result<Token> {
    debug_assert!(
        !run.is_empty(),
        "next_token only calls this on a non-empty run"
    );
    let first = run[0];
    let looks_numeric = matches!(first, b'+' | b'-' | b'.' | b'0'..=b'9');
    if !looks_numeric {
        return Ok(Token::Keyword(run.to_vec()));
    }

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
    if !well_formed || !seen_digit {
        // Starts like a number but isn't one (e.g. `-`, `.`, `1.2.3`): keep as a keyword.
        return Ok(Token::Keyword(run.to_vec()));
    }

    // The run is now known to be pure ASCII `[+-.0-9]`, so UTF-8 decoding cannot fail.
    let Ok(text) = std::str::from_utf8(run) else {
        return Err(ReaderError::new(ErrorKind::InvalidNumber, offset));
    };
    if !seen_dot && let Ok(i) = text.parse::<i64>() {
        return Ok(Token::Integer(i));
    }
    // Integer too large for i64: fall through and represent it as a real.
    match text.parse::<f64>() {
        Ok(r) => Ok(Token::Real(r)),
        Err(_) => Err(ReaderError::new(ErrorKind::InvalidNumber, offset)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Collect every token from `input`, asserting none error.
    fn lex_all(input: &[u8]) -> Vec<Token> {
        let mut lexer = Lexer::new(input);
        let mut tokens = Vec::new();
        while let Some(tok) = lexer.next_token().expect("lexing should succeed") {
            tokens.push(tok);
        }
        tokens
    }

    #[test]
    fn empty_and_whitespace_only_yield_no_tokens() {
        // §7.2.2 white space (incl. NUL) carries no tokens.
        assert!(lex_all(b"").is_empty());
        assert!(lex_all(b" \t\r\n\x0c\x00").is_empty());
    }

    #[test]
    fn comments_are_skipped() {
        // §7.2.4: a comment runs from `%` to the EOL.
        assert_eq!(
            lex_all(b"% a comment\n42 % trailing\n"),
            vec![Token::Integer(42)]
        );
    }

    #[test]
    fn integers_and_reals() {
        // §7.3.3 numeric objects, including the `.5`, `4.`, signed and `+` forms.
        assert_eq!(
            lex_all(b"0 +17 -98 34"),
            vec![
                Token::Integer(0),
                Token::Integer(17),
                Token::Integer(-98),
                Token::Integer(34)
            ]
        );
        let reals = lex_all(b"34.5 -3.62 +123.6 4. -.002 0.0");
        assert_eq!(reals[0], Token::Real(34.5));
        assert_eq!(reals[3], Token::Real(4.0));
        assert_eq!(reals[4], Token::Real(-0.002));
    }

    #[test]
    fn integer_overflow_promotes_to_real() {
        // §7.3.3 note: out-of-range integers are read as reals rather than failing.
        match lex_all(b"99999999999999999999999").as_slice() {
            [Token::Real(_)] => {}
            other => panic!("expected a Real, got {other:?}"),
        }
    }

    #[test]
    fn malformed_number_degrades_to_keyword() {
        // Hostile input: `1.2.3` and a bare `-` are not numbers — keep lexing, don't error.
        assert_eq!(lex_all(b"1.2.3"), vec![Token::Keyword(b"1.2.3".to_vec())]);
        assert_eq!(lex_all(b"-"), vec![Token::Keyword(b"-".to_vec())]);
    }

    #[test]
    fn keywords() {
        // §7.3.10 (`R`/`obj`), §7.3.2 (`true`/`false`), §7.3.9 (`null`).
        assert_eq!(
            lex_all(b"true false null obj endobj R stream"),
            vec![
                Token::Keyword(b"true".to_vec()),
                Token::Keyword(b"false".to_vec()),
                Token::Keyword(b"null".to_vec()),
                Token::Keyword(b"obj".to_vec()),
                Token::Keyword(b"endobj".to_vec()),
                Token::Keyword(b"R".to_vec()),
                Token::Keyword(b"stream".to_vec()),
            ]
        );
    }

    #[test]
    fn literal_string_escapes() {
        // §7.3.4.2: named escapes, octal, balanced parens, ignored backslash.
        assert_eq!(
            lex_all(b"(Hello \\(World\\)\\n\\101)"),
            vec![Token::String(b"Hello (World)\nA".to_vec())]
        );
        // Balanced unescaped parens are part of the string.
        assert_eq!(lex_all(b"(a(b)c)"), vec![Token::String(b"a(b)c".to_vec())]);
        // Unknown escape: the backslash is dropped.
        assert_eq!(lex_all(b"(\\q)"), vec![Token::String(b"q".to_vec())]);
    }

    #[test]
    fn literal_string_eol_normalisation_and_continuation() {
        // §7.3.4.2: a raw CRLF inside a string becomes one LF; `\<EOL>` is a line continuation.
        assert_eq!(lex_all(b"(a\r\nb)"), vec![Token::String(b"a\nb".to_vec())]);
        assert_eq!(lex_all(b"(a\\\r\nb)"), vec![Token::String(b"ab".to_vec())]);
    }

    #[test]
    fn hex_strings() {
        // §7.3.4.3: pairs of hex digits; white space ignored; odd final digit pads with 0.
        assert_eq!(
            lex_all(b"<48656C6C6F>"),
            vec![Token::String(b"Hello".to_vec())]
        );
        assert_eq!(lex_all(b"<48 65 6c>"), vec![Token::String(b"Hel".to_vec())]);
        assert_eq!(lex_all(b"<41A>"), vec![Token::String(vec![0x41, 0xA0])]);
    }

    #[test]
    fn names_with_escapes() {
        // §7.3.5: `#XX` escapes; a malformed `#` is kept literally.
        assert_eq!(lex_all(b"/Type"), vec![Token::Name(b"Type".to_vec())]);
        assert_eq!(lex_all(b"/A#20B"), vec![Token::Name(b"A B".to_vec())]);
        assert_eq!(lex_all(b"/"), vec![Token::Name(Vec::new())]);
        assert_eq!(lex_all(b"/a#zz"), vec![Token::Name(b"a#zz".to_vec())]);
    }

    #[test]
    fn array_and_dictionary_delimiters() {
        // §7.3.6 arrays, §7.3.7 dictionaries.
        assert_eq!(
            lex_all(b"[<< /K 1 >>]"),
            vec![
                Token::ArrayOpen,
                Token::DictOpen,
                Token::Name(b"K".to_vec()),
                Token::Integer(1),
                Token::DictClose,
                Token::ArrayClose,
            ]
        );
    }

    #[test]
    fn unterminated_string_and_lone_gt_error_without_panic() {
        // DESIGN.md §3.4: hostile input errors, never panics.
        assert_eq!(
            Lexer::new(b"(unterminated")
                .next_token()
                .unwrap_err()
                .kind(),
            ErrorKind::UnexpectedEof
        );
        assert_eq!(
            Lexer::new(b"<deadbeef").next_token().unwrap_err().kind(),
            ErrorKind::UnexpectedEof
        );
        assert_eq!(
            Lexer::new(b">").next_token().unwrap_err().kind(),
            ErrorKind::UnexpectedByte
        );
        assert_eq!(
            Lexer::new(b"<xy>").next_token().unwrap_err().kind(),
            ErrorKind::InvalidHexDigit
        );
    }

    #[test]
    fn realistic_indirect_object_header_tokenises() {
        // §7.3.10: `12 0 obj << /Length 5 >> ... endobj` — the shape M1's parser will consume.
        let toks = lex_all(b"12 0 obj\n<< /Length 5 /Type /Page >>\nendobj");
        assert_eq!(toks[0], Token::Integer(12));
        assert_eq!(toks[1], Token::Integer(0));
        assert_eq!(toks[2], Token::Keyword(b"obj".to_vec()));
        assert_eq!(toks[3], Token::DictOpen);
        assert_eq!(*toks.last().unwrap(), Token::Keyword(b"endobj".to_vec()));
    }
}
