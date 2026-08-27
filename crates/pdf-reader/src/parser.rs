//! The PDF object parser (ISO 32000-1 §7.3): assembles the [`Lexer`]'s [`Token`]s into
//! [`pdf_cos::Object`] values.
//!
//! Covers the direct object grammar — null/boolean/numeric/string/name (§7.3.2–§7.3.5), arrays
//! (§7.3.6), dictionaries (§7.3.7) — plus indirect references `n g R` (§7.3.10), indirect object
//! definitions `n g obj … endobj`, and streams (§7.3.8). It does **not** resolve references or
//! follow the xref (ADR-0001): an `n g R` becomes an [`Object::Reference`] carrying its
//! [`ObjectId`], nothing more.
//!
//! Hostile input is assumed (DESIGN.md §3.4): parsing is fallible and total — no panics, no
//! unbounded recursion (nesting is capped by [`Limits`]), and the cursor always advances.

use pdf_cos::{Array, Dictionary, Name, Object, ObjectId, PdfString, Stream};

use crate::error::{ErrorKind, ReaderError, Result};
use crate::lexer::{Lexer, Token};

/// Anti-DoS bounds applied while parsing untrusted input (DESIGN.md §3.4).
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    /// Maximum nesting depth of arrays and dictionaries before parsing is refused. Guards against
    /// stack exhaustion from pathological inputs like a megabyte of `[`.
    pub max_depth: usize,
    /// Maximum number of objects in a single object stream (`/N`, §7.5.7). Guards against an
    /// allocation/loop driven by an attacker-chosen `/N` that is far larger than the stream's data.
    pub max_objstm_objects: usize,
    /// Maximum number of cross-reference entries a document may declare (§7.5). Bounds the memory a
    /// hostile file can force — whether through huge xref sections or a recovery scan that finds
    /// millions of fabricated `n g obj` headers.
    pub max_objects: usize,
    /// Maximum bytes a single filter stage may decode to (§7.4) — the decompression-bomb guard. An
    /// embedder running untrusted files in a constrained worker will want this well below the
    /// default.
    pub max_decoded_stream: usize,
    /// Maximum number of stages in one stream's `/Filter` chain (§7.4). Every stage re-processes the
    /// whole of the previous stage's output, so without this the work a stream can demand is the
    /// product of its body size and its chain length — both attacker-chosen.
    pub max_filter_chain: usize,
}

impl Default for Limits {
    fn default() -> Self {
        // Real documents nest only a handful of levels deep; 512 is far above any legitimate file
        // while still bounding recursion well below the stack limit. A real object stream holds at
        // most a few thousand objects; 1M is generous headroom while still bounding `/N`. A whole
        // document rarely exceeds a few hundred thousand objects; 2M leaves ample room. The two
        // filter bounds take `pdf-filters`' own defaults, which the crate documents.
        Self {
            max_depth: 512,
            max_objstm_objects: 1 << 20,
            max_objects: 1 << 21,
            max_decoded_stream: pdf_filters::DEFAULT_MAX_DECODED,
            max_filter_chain: pdf_filters::DEFAULT_MAX_FILTER_CHAIN,
        }
    }
}

impl Limits {
    /// Decode `stream`'s `/Filter` chain under these limits — the single place the reader turns a
    /// [`Limits`] into the two ceilings `pdf-filters` takes.
    pub(crate) fn decode(&self, stream: &pdf_cos::Stream) -> pdf_filters::Result<Vec<u8>> {
        pdf_filters::decode_stream_with_limits(
            stream,
            self.max_decoded_stream,
            self.max_filter_chain,
        )
    }
}

/// A recursive-descent parser over a PDF byte slice (§7.3).
///
/// Wraps a [`Lexer`] with a small token look-ahead buffer (needed to distinguish a bare integer
/// from the start of an `n g R` reference).
#[derive(Debug)]
pub struct Parser<'a> {
    lexer: Lexer<'a>,
    /// Tokens read ahead of the cursor, oldest first.
    lookahead: Vec<Token>,
    limits: Limits,
}

impl<'a> Parser<'a> {
    /// Create a parser over `input` with default [`Limits`].
    #[must_use]
    pub fn new(input: &'a [u8]) -> Self {
        Self::with_limits(input, Limits::default())
    }

    /// Create a parser over `input` with explicit anti-DoS [`Limits`].
    #[must_use]
    pub fn with_limits(input: &'a [u8], limits: Limits) -> Self {
        Self {
            lexer: Lexer::new(input),
            lookahead: Vec::new(),
            limits,
        }
    }

    /// The current byte offset, accounting for buffered look-ahead. Equal to the lexer cursor when
    /// nothing is buffered.
    #[must_use]
    pub fn offset(&self) -> usize {
        self.lexer.offset()
    }

    /// Reposition the parser to an absolute byte offset (clamped to the input length), discarding
    /// any buffered look-ahead. Used to parse an indirect object located via the cross-reference
    /// table (§7.5).
    pub fn seek(&mut self, offset: usize) {
        self.lookahead.clear();
        self.lexer.set_offset(offset);
    }

    /// Ensure at least `n` tokens are buffered, stopping early at end of input.
    fn fill(&mut self, n: usize) -> Result<()> {
        while self.lookahead.len() < n {
            match self.lexer.next_token()? {
                Some(tok) => self.lookahead.push(tok),
                None => break,
            }
        }
        Ok(())
    }

    /// Look at the `i`-th buffered token (0 = next) without consuming it.
    fn peek(&mut self, i: usize) -> Result<Option<&Token>> {
        self.fill(i + 1)?;
        Ok(self.lookahead.get(i))
    }

    /// Consume and return the next token, or `None` at end of input.
    fn bump(&mut self) -> Result<Option<Token>> {
        self.fill(1)?;
        if self.lookahead.is_empty() {
            Ok(None)
        } else {
            Ok(Some(self.lookahead.remove(0)))
        }
    }

    /// Parse a single direct object (§7.3), or `Ok(None)` at end of input. References (`n g R`)
    /// are recognised and returned as [`Object::Reference`].
    pub fn parse_object(&mut self) -> Result<Option<Object>> {
        let offset = self.offset();
        match self.bump()? {
            None => Ok(None),
            Some(tok) => self.value_from(tok, offset, 0).map(Some),
        }
    }

    /// Parse the value introduced by an already-consumed `tok` at byte `offset`, tracking
    /// recursion `depth` for the nesting limit.
    fn value_from(&mut self, tok: Token, offset: usize, depth: usize) -> Result<Object> {
        match tok {
            Token::Integer(n) => self.integer_or_reference(n),
            Token::Real(r) => Ok(Object::Real(r)),
            Token::String(bytes) => Ok(Object::String(PdfString::from(bytes))),
            Token::Name(bytes) => Ok(Object::Name(Name::from(bytes))),
            Token::Keyword(kw) => match kw.as_slice() {
                b"true" => Ok(Object::Boolean(true)),
                b"false" => Ok(Object::Boolean(false)),
                b"null" => Ok(Object::Null),
                // `R`, `obj`, `endobj`, `stream`, … cannot *start* a value.
                _ => Err(ReaderError::new(ErrorKind::UnexpectedToken, offset)),
            },
            Token::ArrayOpen => self.parse_array(depth),
            Token::DictOpen => self.parse_dictionary(depth),
            // A closing bracket or stray `R` where a value was expected.
            Token::ArrayClose | Token::DictClose => {
                Err(ReaderError::new(ErrorKind::UnexpectedToken, offset))
            }
        }
    }

    /// Given a just-consumed integer `n`, decide whether it begins an `n g R` reference (§7.3.10)
    /// and, if so, consume the `g` and `R`. Otherwise it is a plain integer and the look-ahead is
    /// left intact.
    fn integer_or_reference(&mut self, n: i64) -> Result<Object> {
        let is_reference = matches!(self.peek(0)?, Some(Token::Integer(_)))
            && matches!(self.peek(1)?, Some(Token::Keyword(kw)) if kw.as_slice() == b"R");
        if !is_reference {
            return Ok(Object::Integer(n));
        }
        let Some(Token::Integer(g)) = self.bump()? else {
            unreachable!("peek(0) just confirmed an Integer")
        };
        let _ = self.bump()?; // the `R` keyword

        match (u32::try_from(n), u16::try_from(g)) {
            (Ok(number), Ok(generation)) => {
                Ok(Object::Reference(ObjectId::new(number, generation)))
            }
            // Object/generation numbers out of range (§7.3.10): not a valid reference. Keep the
            // leading integer as the value; the malformed `g R` is dropped.
            _ => Ok(Object::Integer(n)),
        }
    }

    /// Parse the body of an array after its `[` (§7.3.6), up to and including `]`.
    fn parse_array(&mut self, depth: usize) -> Result<Object> {
        let depth = self.deepen(depth)?;
        let mut items = Vec::new();
        loop {
            let offset = self.offset();
            match self.bump()? {
                None => return Err(ReaderError::new(ErrorKind::UnexpectedEof, offset)),
                Some(Token::ArrayClose) => return Ok(Object::Array(Array::from_vec(items))),
                Some(tok) => items.push(self.value_from(tok, offset, depth)?),
            }
        }
    }

    /// Parse the body of a dictionary after its `<<` (§7.3.7), up to and including `>>`. Keys must
    /// be names; a non-name key is rejected.
    fn parse_dictionary(&mut self, depth: usize) -> Result<Object> {
        let depth = self.deepen(depth)?;
        let mut dict = Dictionary::new();
        loop {
            let key_offset = self.offset();
            let key = match self.bump()? {
                None => return Err(ReaderError::new(ErrorKind::UnexpectedEof, key_offset)),
                Some(Token::DictClose) => return Ok(Object::Dictionary(dict)),
                Some(Token::Name(bytes)) => Name::from(bytes),
                // §7.3.7: dictionary keys shall be names.
                Some(_) => return Err(ReaderError::new(ErrorKind::UnexpectedToken, key_offset)),
            };
            let val_offset = self.offset();
            let value = match self.bump()? {
                None => return Err(ReaderError::new(ErrorKind::UnexpectedEof, val_offset)),
                Some(tok) => self.value_from(tok, val_offset, depth)?,
            };
            dict.insert(key, value);
        }
    }

    /// Increment and check the nesting depth (anti-DoS, DESIGN.md §3.4).
    fn deepen(&self, depth: usize) -> Result<usize> {
        if depth >= self.limits.max_depth {
            return Err(ReaderError::new(ErrorKind::NestingTooDeep, self.offset()));
        }
        Ok(depth + 1)
    }

    /// Parse an indirect object definition `n g obj … endobj` (§7.3.10). The value may be a stream
    /// (§7.3.8), in which case the dictionary is paired with its raw body. The trailing `endobj`
    /// is consumed when present but not required (real files sometimes omit it).
    ///
    /// Returns the [`ObjectId`] from the header and the parsed [`Object`].
    pub fn parse_indirect_object(&mut self) -> Result<(ObjectId, Object)> {
        let offset = self.offset();
        let number = self.expect_unsigned_integer(offset)?;
        let gen_offset = self.offset();
        let generation = self.expect_unsigned_integer(gen_offset)?;
        self.expect_keyword(b"obj", self.offset())?;

        let val_offset = self.offset();
        let value = match self.bump()? {
            None => return Err(ReaderError::new(ErrorKind::UnexpectedEof, val_offset)),
            Some(tok) => self.value_from(tok, val_offset, 0)?,
        };

        // A dictionary immediately followed by `stream` is a stream object (§7.3.8).
        let value = if let Object::Dictionary(dict) = value {
            if matches!(self.peek(0)?, Some(Token::Keyword(kw)) if kw.as_slice() == b"stream") {
                let _ = self.bump()?; // the `stream` keyword
                Object::Stream(self.parse_stream_body(dict)?)
            } else {
                Object::Dictionary(dict)
            }
        } else {
            value
        };

        // Consume `endobj` if it is next; tolerate its absence.
        if matches!(self.peek(0)?, Some(Token::Keyword(kw)) if kw.as_slice() == b"endobj") {
            let _ = self.bump()?;
        }

        let object_number = u32::try_from(number)
            .map_err(|_| ReaderError::new(ErrorKind::UnexpectedToken, offset))?;
        let object_gen = u16::try_from(generation)
            .map_err(|_| ReaderError::new(ErrorKind::UnexpectedToken, gen_offset))?;
        Ok((ObjectId::new(object_number, object_gen), value))
    }

    /// Read the raw bytes of a stream (§7.3.8) whose `stream` keyword was just consumed, and build
    /// the [`Stream`]. Precondition: the look-ahead buffer is empty, so the lexer cursor sits
    /// exactly after `stream`.
    ///
    /// Length handling: a direct integer `/Length` is trusted when the bytes it points at are
    /// actually followed by `endstream`; otherwise (indirect or wrong `/Length`) the body is found
    /// by scanning for `endstream`, which is also the recovery path for real-world files that lie
    /// about their length.
    fn parse_stream_body(&mut self, dict: Dictionary) -> Result<Stream> {
        debug_assert!(
            self.lookahead.is_empty(),
            "stream body must be read with an empty look-ahead so the cursor is exact"
        );
        let input = self.lexer.input();
        // §7.3.8.1: `stream` is followed by CRLF or LF. Be lenient about stray spaces/tabs and a
        // lone CR that some malformed writers emit.
        let mut pos = self.lexer.offset();
        while matches!(input.get(pos), Some(b' ' | b'\t')) {
            pos += 1;
        }
        if input.get(pos) == Some(&b'\r') {
            pos += 1;
        }
        if input.get(pos) == Some(&b'\n') {
            pos += 1;
        }
        let data_start = pos;

        let declared = dict
            .get_integer(&Name::from("Length"))
            .and_then(|len| usize::try_from(len).ok());

        let (data_end, resume) = match declared {
            Some(len)
                if data_start + len <= input.len()
                    && keyword_follows(input, data_start + len, b"endstream") =>
            {
                let end = data_start + len;
                let resume = skip_eol(input, end) + b"endstream".len();
                (end, resume)
            }
            _ => {
                let Some(kw_at) = find(input, data_start, b"endstream") else {
                    return Err(ReaderError::new(ErrorKind::UnterminatedStream, data_start));
                };
                // The EOL just before `endstream` belongs to the keyword, not the data (§7.3.8.1).
                (
                    strip_trailing_eol(input, data_start, kw_at),
                    kw_at + b"endstream".len(),
                )
            }
        };

        let raw = input[data_start..data_end].to_vec();
        self.lexer.set_offset(resume);
        Ok(Stream::new(dict, raw))
    }

    /// Consume the next token, requiring it to be a non-negative integer.
    fn expect_unsigned_integer(&mut self, offset: usize) -> Result<i64> {
        match self.bump()? {
            Some(Token::Integer(n)) if n >= 0 => Ok(n),
            _ => Err(ReaderError::new(ErrorKind::UnexpectedToken, offset)),
        }
    }

    /// Consume the next token, requiring it to be exactly the keyword `kw`.
    fn expect_keyword(&mut self, kw: &[u8], offset: usize) -> Result<()> {
        match self.bump()? {
            Some(Token::Keyword(found)) if found.as_slice() == kw => Ok(()),
            _ => Err(ReaderError::new(ErrorKind::UnexpectedToken, offset)),
        }
    }
}

/// Whether `needle` appears at `pos` in `hay`, after skipping one optional EOL marker.
fn keyword_follows(hay: &[u8], pos: usize, needle: &[u8]) -> bool {
    let at = skip_eol(hay, pos);
    hay.get(at..at + needle.len()) == Some(needle)
}

/// Skip a single end-of-line marker (CRLF, LF or CR) at `pos`, returning the new offset.
fn skip_eol(hay: &[u8], pos: usize) -> usize {
    match hay.get(pos) {
        Some(b'\r') if hay.get(pos + 1) == Some(&b'\n') => pos + 2,
        Some(b'\r' | b'\n') => pos + 1,
        _ => pos,
    }
}

/// The end offset of stream data ending at `kw_at`, with one trailing EOL stripped (§7.3.8.1).
fn strip_trailing_eol(hay: &[u8], start: usize, kw_at: usize) -> usize {
    let mut end = kw_at;
    if end > start && hay.get(end - 1) == Some(&b'\n') {
        end -= 1;
    }
    if end > start && hay.get(end - 1) == Some(&b'\r') {
        end -= 1;
    }
    end
}

/// The offset of the first occurrence of `needle` in `hay[from..]`, or `None`.
fn find(hay: &[u8], from: usize, needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || from >= hay.len() {
        return None;
    }
    hay[from..]
        .windows(needle.len())
        .position(|w| w == needle)
        .map(|p| from + p)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_one(input: &[u8]) -> Object {
        Parser::new(input)
            .parse_object()
            .expect("parse should succeed")
            .expect("expected an object")
    }

    #[test]
    fn scalars() {
        // §7.3.2 boolean, §7.3.9 null, §7.3.3 numeric.
        assert_eq!(parse_one(b"true"), Object::Boolean(true));
        assert_eq!(parse_one(b"false"), Object::Boolean(false));
        assert_eq!(parse_one(b"null"), Object::Null);
        assert_eq!(parse_one(b"42"), Object::Integer(42));
        assert_eq!(parse_one(b"-3.5"), Object::Real(-3.5));
    }

    #[test]
    fn strings_and_names() {
        // §7.3.4 string, §7.3.5 name.
        assert_eq!(
            parse_one(b"(hi)"),
            Object::String(PdfString::from(b"hi".to_vec()))
        );
        assert_eq!(
            parse_one(b"<41>"),
            Object::String(PdfString::from(vec![0x41]))
        );
        assert_eq!(parse_one(b"/Type"), Object::Name(Name::from("Type")));
    }

    #[test]
    fn reference_vs_integers() {
        // §7.3.10: `1 0 R` is a reference; `1 0` are two integers in an array.
        assert_eq!(parse_one(b"3 0 R"), Object::Reference(ObjectId::new(3, 0)));
        let Object::Array(arr) = parse_one(b"[1 0 2]") else {
            panic!("expected array");
        };
        assert_eq!(
            arr.iter().cloned().collect::<Vec<_>>(),
            vec![Object::Integer(1), Object::Integer(0), Object::Integer(2)]
        );
    }

    #[test]
    fn nested_array_with_references() {
        // §7.3.6 array mixing scalars and `n g R` references.
        let Object::Array(arr) = parse_one(b"[1 2.5 (x) /N 7 0 R [true]]") else {
            panic!("expected array");
        };
        let items: Vec<_> = arr.iter().cloned().collect();
        assert_eq!(items.len(), 6);
        assert_eq!(items[4], Object::Reference(ObjectId::new(7, 0)));
        assert!(matches!(items[5], Object::Array(_)));
    }

    #[test]
    fn dictionary() {
        // §7.3.7: keys are names, value may be a reference.
        let Object::Dictionary(d) = parse_one(b"<< /Type /Page /Count 3 /Kids 4 0 R >>") else {
            panic!("expected dictionary");
        };
        assert_eq!(d.get_name(&Name::from("Type")), Some(&Name::from("Page")));
        assert_eq!(d.get_integer(&Name::from("Count")), Some(3));
        assert_eq!(
            d.get_reference(&Name::from("Kids")),
            Some(ObjectId::new(4, 0))
        );
    }

    #[test]
    fn nesting_limit_rejected_without_overflow() {
        // Anti-DoS (DESIGN.md §3.4): deep nesting errors instead of blowing the stack.
        let bombs = b"[".repeat(10_000);
        let limits = Limits {
            max_depth: 64,
            ..Limits::default()
        };
        let err = Parser::with_limits(&bombs, limits)
            .parse_object()
            .unwrap_err();
        assert_eq!(err.kind(), ErrorKind::NestingTooDeep);
    }

    #[test]
    fn indirect_object_with_dictionary() {
        // §7.3.10: `n g obj <dict> endobj`.
        let (id, obj) = Parser::new(b"12 0 obj << /Length 5 >> endobj")
            .parse_indirect_object()
            .unwrap();
        assert_eq!(id, ObjectId::new(12, 0));
        assert!(matches!(obj, Object::Dictionary(_)));
    }

    #[test]
    fn stream_with_correct_length() {
        // §7.3.8: trust a direct /Length that lands on `endstream`.
        let input = b"5 0 obj << /Length 11 >>\nstream\nHello World\nendstream\nendobj";
        let (id, obj) = Parser::new(input).parse_indirect_object().unwrap();
        assert_eq!(id, ObjectId::new(5, 0));
        let Object::Stream(s) = obj else {
            panic!("expected stream")
        };
        assert_eq!(s.raw().as_ref(), b"Hello World");
    }

    #[test]
    fn stream_with_wrong_length_falls_back_to_scan() {
        // Real files lie about /Length; recovery scans for `endstream` (DESIGN.md §3).
        let input = b"5 0 obj << /Length 2 >>\nstream\nHello World\nendstream\nendobj";
        let (_, obj) = Parser::new(input).parse_indirect_object().unwrap();
        let Object::Stream(s) = obj else {
            panic!("expected stream")
        };
        assert_eq!(s.raw().as_ref(), b"Hello World");
    }

    #[test]
    fn unterminated_constructs_error_without_panic() {
        // DESIGN.md §3.4: truncated input errors cleanly.
        assert_eq!(
            Parser::new(b"[1 2 3").parse_object().unwrap_err().kind(),
            ErrorKind::UnexpectedEof
        );
        assert_eq!(
            Parser::new(b"<< /K 1").parse_object().unwrap_err().kind(),
            ErrorKind::UnexpectedEof
        );
    }

    #[test]
    fn empty_input_yields_no_object() {
        assert_eq!(Parser::new(b"   ").parse_object().unwrap(), None);
    }
}
