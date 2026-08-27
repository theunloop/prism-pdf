//! PDF date strings (ISO 32000 §7.9.4): `D:YYYYMMDDHHmmSSOHH'mm`.
//!
//! A date is an ASCII string where every field after the year is optional, each present only if
//! all preceding fields are (defaults: January 1st, midnight). The offset marker `O` is `+`, `-`
//! or `Z`; ISO 32000-1 wrote the offset as `HH'mm'` while ISO 32000-2 drops the trailing
//! apostrophe (`HH'mm`) — parsing accepts both, plus the apostrophe-free `HHmm` some producers
//! emit. Parsing is total (hostile input yields `None`, never a panic) and strict on ranges and
//! trailing garbage.

use core::fmt;

/// A parsed PDF date string (§7.9.4), as used by `/Info` `CreationDate`/`ModDate`, a signature's
/// `/M`, and embedded-file `/Params /ModDate`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PdfDate {
    /// Four-digit year.
    pub year: u16,
    /// Month 1–12 (default 1 when absent).
    pub month: u8,
    /// Day 1–31 (default 1 when absent).
    pub day: u8,
    /// Hour 0–23 (default 0 when absent).
    pub hour: u8,
    /// Minute 0–59 (default 0 when absent).
    pub minute: u8,
    /// Second 0–59 (default 0 when absent).
    pub second: u8,
    /// Offset of local time from UTC in minutes (`Z` → `Some(0)`); `None` when the string
    /// declares no relationship to UTC.
    pub utc_offset_minutes: Option<i16>,
}

impl PdfDate {
    /// Parse a PDF date string (§7.9.4). The `D:` prefix is required by the spec but commonly
    /// omitted, so it is optional here; ASCII whitespace around the value is ignored. Returns
    /// `None` for anything malformed or out of range.
    #[must_use]
    pub fn parse(bytes: &[u8]) -> Option<Self> {
        let mut s = bytes.trim_ascii();
        if let Some(rest) = s.strip_prefix(b"D:") {
            s = rest;
        }

        let year = take_digits(&mut s, 4)?;
        let month = opt_field(&mut s, 1)?;
        let day = opt_field(&mut s, 1)?;
        let hour = opt_field(&mut s, 0)?;
        let minute = opt_field(&mut s, 0)?;
        let second = opt_field(&mut s, 0)?;

        // Offset part: absent, or `Z`, or `+`/`-` followed by hours and optional minutes, with
        // the apostrophe separators of either edition tolerated.
        let utc_offset_minutes = match s.first() {
            None => None,
            Some(&marker @ (b'Z' | b'+' | b'-')) => {
                s = &s[1..];
                let hh = if s.first().is_some_and(u8::is_ascii_digit) {
                    take_digits(&mut s, 2)?
                } else if marker == b'Z' {
                    0
                } else {
                    return None; // `+`/`-` must carry offset hours
                };
                if s.first() == Some(&b'\'') {
                    s = &s[1..];
                }
                let mm = if s.first().is_some_and(u8::is_ascii_digit) {
                    take_digits(&mut s, 2)?
                } else {
                    0
                };
                if s.first() == Some(&b'\'') {
                    s = &s[1..];
                }
                if hh > 23 || mm > 59 {
                    return None;
                }
                let total = i16::try_from(hh * 60 + mm).ok()?;
                match marker {
                    b'Z' if total != 0 => return None, // `Z` is UTC; a nonzero offset contradicts it
                    b'-' => Some(-total),
                    _ => Some(total),
                }
            }
            Some(_) => return None,
        };
        if !s.is_empty() {
            return None; // trailing garbage
        }

        let date = Self {
            year,
            month: u8::try_from(month).ok()?,
            day: u8::try_from(day).ok()?,
            hour: u8::try_from(hour).ok()?,
            minute: u8::try_from(minute).ok()?,
            second: u8::try_from(second).ok()?,
            utc_offset_minutes,
        };
        ((1..=12).contains(&date.month)
            && (1..=31).contains(&date.day)
            && date.hour <= 23
            && date.minute <= 59
            && date.second <= 59)
            .then_some(date)
    }
}

/// An optional two-digit field: absent (next byte is not a digit) yields `default`; a present
/// field must be exactly two digits.
fn opt_field(s: &mut &[u8], default: u16) -> Option<u16> {
    if s.first().is_some_and(u8::is_ascii_digit) {
        take_digits(s, 2)
    } else {
        Some(default)
    }
}

/// Consume exactly `n` ASCII digits from the front of `s`.
fn take_digits(s: &mut &[u8], n: usize) -> Option<u16> {
    if s.len() < n || !s[..n].iter().all(u8::is_ascii_digit) {
        return None;
    }
    let mut value = 0u16;
    for &b in &s[..n] {
        value = value.checked_mul(10)?.checked_add(u16::from(b - b'0'))?;
    }
    *s = &s[n..];
    Some(value)
}

impl fmt::Display for PdfDate {
    /// The canonical ISO 32000-2 form: `D:YYYYMMDDHHmmSS` plus `Z`, nothing, or `±HH'mm`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "D:{:04}{:02}{:02}{:02}{:02}{:02}",
            self.year, self.month, self.day, self.hour, self.minute, self.second
        )?;
        match self.utc_offset_minutes {
            None => Ok(()),
            Some(0) => write!(f, "Z"),
            Some(off) => {
                let sign = if off < 0 { '-' } else { '+' };
                let abs = off.unsigned_abs();
                write!(f, "{sign}{:02}'{:02}", abs / 60, abs % 60)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_full_acrobat_form() {
        // ISO 32000-1 form with trailing apostrophe, as Acrobat writes it.
        let d = PdfDate::parse(b"D:20260817143005+02'00'").unwrap();
        assert_eq!(
            d,
            PdfDate {
                year: 2026,
                month: 8,
                day: 17,
                hour: 14,
                minute: 30,
                second: 5,
                utc_offset_minutes: Some(120),
            }
        );
        // ISO 32000-2 form (no trailing apostrophe) and the apostrophe-free variant.
        assert_eq!(PdfDate::parse(b"D:20260817143005+02'00"), Some(d));
        assert_eq!(PdfDate::parse(b"D:20260817143005+0200"), Some(d));
    }

    #[test]
    fn optional_fields_default_and_truncate() {
        let d = PdfDate::parse(b"D:2026").unwrap();
        assert_eq!((d.year, d.month, d.day), (2026, 1, 1));
        assert_eq!((d.hour, d.minute, d.second), (0, 0, 0));
        assert_eq!(d.utc_offset_minutes, None);
        assert_eq!(PdfDate::parse(b"D:202608").unwrap().month, 8);
        assert_eq!(PdfDate::parse(b"D:20260817").unwrap().day, 17);
    }

    #[test]
    fn prefix_is_optional_and_whitespace_tolerated() {
        assert_eq!(
            PdfDate::parse(b" 20260817120000Z "),
            PdfDate::parse(b"D:20260817120000Z")
        );
    }

    #[test]
    fn zulu_and_negative_offsets() {
        assert_eq!(
            PdfDate::parse(b"D:20260817120000Z")
                .unwrap()
                .utc_offset_minutes,
            Some(0)
        );
        // A redundant zero offset after Z is tolerated; a nonzero one contradicts Z.
        assert_eq!(
            PdfDate::parse(b"D:20260817120000Z00'00'")
                .unwrap()
                .utc_offset_minutes,
            Some(0)
        );
        assert_eq!(PdfDate::parse(b"D:20260817120000Z02'00"), None);
        assert_eq!(
            PdfDate::parse(b"D:20260817120000-05'30")
                .unwrap()
                .utc_offset_minutes,
            Some(-330)
        );
    }

    #[test]
    fn rejects_out_of_range_and_malformed() {
        assert_eq!(PdfDate::parse(b"D:20261317120000"), None); // month 13
        assert_eq!(PdfDate::parse(b"D:20260832"), None); // day 32
        assert_eq!(PdfDate::parse(b"D:20260817250000"), None); // hour 25
        assert_eq!(PdfDate::parse(b"D:2026081712006 "), None); // 1-digit second
        assert_eq!(PdfDate::parse(b"D:202"), None); // short year
        assert_eq!(PdfDate::parse(b"D:20260817+"), None); // sign without hours
        assert_eq!(PdfDate::parse(b"D:20260817120000+25'00"), None); // offset hours 25
        assert_eq!(PdfDate::parse(b"D:20260817120000junk"), None); // trailing garbage
        assert_eq!(PdfDate::parse(b"not a date"), None);
        assert_eq!(PdfDate::parse(b""), None);
    }

    #[test]
    fn display_round_trips_canonically() {
        for (input, canonical) in [
            (&b"D:20260817143005+02'00'"[..], "D:20260817143005+02'00"),
            (b"D:20260817120000Z", "D:20260817120000Z"),
            (b"D:2026", "D:20260101000000"),
            (b"D:20260817120000-05'30", "D:20260817120000-05'30"),
        ] {
            let d = PdfDate::parse(input).unwrap();
            assert_eq!(d.to_string(), canonical);
            assert_eq!(PdfDate::parse(d.to_string().as_bytes()), Some(d));
        }
    }
}
