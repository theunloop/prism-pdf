//! Content-stream parser (ISO 32000-1 §7.8.2): groups operands and operators into [`Operation`]s.
//!
//! A content stream is postfix: operands accumulate, then an operator consumes them
//! (`/F1 12 Tf`). This parser is **lenient** — it is total and recovers from malformed input by
//! skipping rather than failing, because partial content should still yield partial text
//! (DESIGN.md §3 robustness). Inline images (`BI … EI`, §8.9.7) are skipped wholesale.

use pdf_cos::{Array, Dictionary, Name, Object, PdfString};

use crate::lexer::{Lexer, Token};

/// One content-stream operation: an operator and the operands that preceded it (§7.8.2).
#[derive(Clone, PartialEq, Debug)]
pub struct Operation {
    /// The operator token (e.g. `Tj`, `TJ`, `BT`, `Td`).
    pub operator: String,
    /// The operands that preceded the operator, in order.
    pub operands: Vec<Object>,
}

/// Maximum operand stack depth and composite-object nesting before input is treated as hostile.
const MAX_OPERANDS: usize = 65_536;
const MAX_DEPTH: usize = 64;

/// Parse a decoded content stream into its sequence of operations (§7.8.2). Total on any input.
#[must_use]
pub fn parse_content(data: &[u8]) -> Vec<Operation> {
    let mut lexer = Lexer::new(data);
    let mut operands: Vec<Object> = Vec::new();
    let mut operations = Vec::new();

    loop {
        match lexer.next_token() {
            Ok(None) => break,
            Ok(Some(token)) => match token {
                Token::ArrayOpen => operands.push(build_array(&mut lexer, 1)),
                Token::DictOpen => operands.push(build_dict(&mut lexer, 1)),
                // A stray closing bracket at the top level: ignore it.
                Token::ArrayClose | Token::DictClose => {}
                Token::Keyword(kw) => match kw.as_slice() {
                    b"true" => operands.push(Object::Boolean(true)),
                    b"false" => operands.push(Object::Boolean(false)),
                    b"null" => operands.push(Object::Null),
                    // Inline image (§8.9.7): parse its dictionary and capture the binary body,
                    // surfacing it as a `BI` operation rather than discarding it.
                    b"BI" => {
                        operands.clear();
                        operations.push(read_inline_image(&mut lexer));
                    }
                    _ => operations.push(Operation {
                        operator: String::from_utf8_lossy(&kw).into_owned(),
                        operands: std::mem::take(&mut operands),
                    }),
                },
                other => {
                    if let Some(value) = scalar(other) {
                        operands.push(value);
                    }
                }
            },
            // Lexical error: skip one byte to guarantee progress and resynchronise.
            Err(_) => {
                let next = lexer.offset() + 1;
                lexer.set_offset(next);
                operands.clear();
            }
        }

        // An operand run that never hits an operator must not grow without bound.
        if operands.len() > MAX_OPERANDS {
            operands.clear();
        }
    }
    operations
}

/// Parse an inline image (§8.9.7) after its `BI`: collect the abbreviated dictionary key/value
/// tokens up to `ID`, then capture the raw sample bytes up to the delimited `EI`. Returns a `BI`
/// [`Operation`] carrying `[dict, data]` (the data as a byte string). Best-effort and total.
fn read_inline_image(lexer: &mut Lexer<'_>) -> Operation {
    let mut dict = Dictionary::new();
    loop {
        match lexer.next_token() {
            Ok(Some(Token::Keyword(kw))) if kw == b"ID" => break,
            Ok(Some(Token::Name(key))) => {
                let key = Name::from(key);
                match lexer.next_token() {
                    Ok(Some(token)) => {
                        dict.insert(key, value_from(lexer, token, 1).unwrap_or(Object::Null));
                    }
                    Ok(None) | Err(_) => break,
                }
            }
            // A stray non-name token before ID: ignore (lenient). EOF/error: stop.
            Ok(Some(_)) => {}
            Ok(None) | Err(_) => break,
        }
    }
    let data = lexer.read_inline_image_data();
    Operation {
        operator: "BI".to_string(),
        operands: vec![
            Object::Dictionary(dict),
            Object::String(PdfString::from(data)),
        ],
    }
}

/// Convert a scalar token into an operand, or `None` for non-scalar tokens.
fn scalar(token: Token) -> Option<Object> {
    match token {
        Token::Integer(n) => Some(Object::Integer(n)),
        Token::Real(r) => Some(Object::Real(r)),
        Token::String(b) => Some(Object::String(PdfString::from(b))),
        Token::Name(b) => Some(Object::Name(Name::from(b))),
        _ => None,
    }
}

/// Build an array operand after its `[` (depth-guarded).
fn build_array(lexer: &mut Lexer<'_>, depth: usize) -> Object {
    let mut items = Vec::new();
    // Stops on `]`/`>>`, end of input, or a lexical error (the `while let` pattern fails).
    while let Ok(Some(token)) = lexer.next_token() {
        match token {
            Token::ArrayClose | Token::DictClose => break,
            other => {
                if let Some(value) = value_from(lexer, other, depth) {
                    items.push(value);
                }
            }
        }
    }
    Object::Array(Array::from_vec(items))
}

/// Build a dictionary operand after its `<<` (depth-guarded). Non-name keys end the dictionary.
fn build_dict(lexer: &mut Lexer<'_>, depth: usize) -> Object {
    let mut dict = Dictionary::new();
    // Each iteration reads a name key; `DictClose`, EOF, a lexical error, or a non-name key stops.
    while let Ok(Some(Token::Name(bytes))) = lexer.next_token() {
        let key = Name::from(bytes);
        let value = match lexer.next_token() {
            Ok(Some(Token::DictClose)) | Ok(None) | Err(_) => break,
            Ok(Some(token)) => value_from(lexer, token, depth).unwrap_or(Object::Null),
        };
        dict.insert(key, value);
    }
    Object::Dictionary(dict)
}

/// Turn a token into an operand value inside an array/dictionary, recursing for nested composites.
fn value_from(lexer: &mut Lexer<'_>, token: Token, depth: usize) -> Option<Object> {
    match token {
        Token::ArrayOpen if depth < MAX_DEPTH => Some(build_array(lexer, depth + 1)),
        Token::DictOpen if depth < MAX_DEPTH => Some(build_dict(lexer, depth + 1)),
        Token::Keyword(kw) => match kw.as_slice() {
            b"true" => Some(Object::Boolean(true)),
            b"false" => Some(Object::Boolean(false)),
            b"null" => Some(Object::Null),
            _ => None, // an operator has no place inside a composite
        },
        scalar_token => scalar(scalar_token),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn groups_operands_with_operators() {
        let ops = parse_content(b"BT /F1 12 Tf (Hello) Tj ET");
        assert_eq!(ops.len(), 4);
        assert_eq!(ops[0].operator, "BT");
        assert_eq!(ops[1].operator, "Tf");
        assert_eq!(
            ops[1].operands,
            vec![Object::Name(Name::from("F1")), Object::Integer(12)]
        );
        assert_eq!(ops[2].operator, "Tj");
        assert_eq!(
            ops[2].operands,
            vec![Object::String(PdfString::from(b"Hello".to_vec()))]
        );
    }

    #[test]
    fn reads_an_inline_image() {
        // BI <dict> ID <binary> EI — the dict and data are surfaced, not discarded.
        let ops = parse_content(b"q BI /W 2 /H 1 /CS /RGB /BPC 8 ID \x00\xFF\x01\xFE\x02\xFD EI Q");
        let bi = ops
            .iter()
            .find(|o| o.operator == "BI")
            .expect("BI surfaced");
        let dict = match &bi.operands[0] {
            Object::Dictionary(d) => d,
            other => panic!("expected dict, got {other:?}"),
        };
        assert_eq!(dict.get_integer(&Name::from("W")), Some(2));
        assert_eq!(dict.get_integer(&Name::from("H")), Some(1));
        assert_eq!(
            dict.get_name(&Name::from("CS")).map(Name::as_bytes),
            Some(&b"RGB"[..])
        );
        assert_eq!(
            bi.operands[1],
            Object::String(PdfString::from(b"\x00\xFF\x01\xFE\x02\xFD".to_vec()))
        );
        // The body did not leak into later operators: `Q` still parses.
        assert!(ops.iter().any(|o| o.operator == "Q"));
    }

    #[test]
    fn parses_tj_array_operand() {
        let ops = parse_content(b"[(A) -250 (B)] TJ");
        assert_eq!(ops.len(), 1);
        assert_eq!(ops[0].operator, "TJ");
        let Some(Object::Array(arr)) = ops[0].operands.first() else {
            panic!("expected an array operand");
        };
        assert_eq!(arr.len(), 3);
    }

    #[test]
    fn inline_image_body_does_not_leak() {
        // The binary between ID and EI must not produce spurious operations: BI is one operation
        // (carrying the dict + data), and the surrounding q/Q are intact.
        let ops = parse_content(b"q BI /W 1 /H 1 ID \xDE\xAD\xBE\xEF EI Q");
        let names: Vec<&str> = ops.iter().map(|o| o.operator.as_str()).collect();
        assert_eq!(names, vec!["q", "BI", "Q"]);
    }

    #[test]
    fn recovers_from_garbage() {
        // A stray `)` is a lexical error; parsing continues and still finds the operator.
        let ops = parse_content(b") (ok) Tj");
        assert_eq!(ops.last().map(|o| o.operator.as_str()), Some("Tj"));
    }

    #[test]
    fn boolean_and_null_operands() {
        let ops = parse_content(b"true false null SCN");
        assert_eq!(ops.len(), 1);
        assert_eq!(
            ops[0].operands,
            vec![Object::Boolean(true), Object::Boolean(false), Object::Null]
        );
    }

    #[test]
    fn dictionary_operand_for_marked_content() {
        // BDC takes a tag name and a properties dictionary (§14.6).
        let ops = parse_content(b"/Span << /MCID 0 /Lang (en) >> BDC");
        assert_eq!(ops.len(), 1);
        assert_eq!(ops[0].operator, "BDC");
        assert!(matches!(
            ops[0].operands.get(1),
            Some(Object::Dictionary(_))
        ));
    }

    #[test]
    fn nested_arrays_in_operands() {
        let ops = parse_content(b"[[1 2] [3 [4]]] x");
        let Some(Object::Array(outer)) = ops[0].operands.first() else {
            panic!("expected array");
        };
        assert_eq!(outer.len(), 2);
    }

    #[test]
    fn stray_closers_are_ignored_at_top_level() {
        // A `]` or `>>` with no opener is dropped; surrounding operators still parse.
        let ops = parse_content(b"] (a) Tj >>");
        assert_eq!(ops.len(), 1);
        assert_eq!(ops[0].operator, "Tj");
    }

    #[test]
    fn unterminated_dict_operand_is_safe() {
        // A dictionary that never closes must not loop; it just ends the input.
        let ops = parse_content(b"<< /K 1");
        // No operator follows, so no operation is emitted, but parsing terminates.
        assert!(ops.is_empty());
    }

    #[test]
    fn dict_with_non_name_key_stops() {
        let ops = parse_content(b"<< 1 2 >> q");
        // The malformed dict is still an operand of `q` (built defensively).
        assert_eq!(ops.last().map(|o| o.operator.as_str()), Some("q"));
    }
}
