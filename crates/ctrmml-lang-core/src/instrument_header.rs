//! `@N <type>` instrument definition headers.
//!
//! A header is `@N` at the start of a line followed by the type keyword,
//! either on the same line (`@1 fm`) or on the first non-blank,
//! non-comment line among the next three (`@1` / `fm`). Type keywords are
//! compared case-insensitively, as ctrmml does.

use serde::{Deserialize, Serialize};

use crate::track_selector::LineReader;

/// Lines after a bare `@N` that may hold its type keyword.
const KEYWORD_LOOKAHEAD: u32 = 3;

/// Instrument type of an `@N` definition.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum InstrumentType {
    Fm,
    Psg,
    Pcm,
}

impl InstrumentType {
    pub const ALL: [Self; 3] = [Self::Fm, Self::Psg, Self::Pcm];

    pub const fn keyword(self) -> &'static str {
        match self {
            Self::Fm => "fm",
            Self::Psg => "psg",
            Self::Pcm => "pcm",
        }
    }

    pub fn parse(keyword: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|ty| keyword.eq_ignore_ascii_case(ty.keyword()))
    }
}

/// A parsed `@N <type>` header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InstrumentHeader<'a> {
    pub number: u32,
    /// The type keyword as written; empty when there is none.
    pub keyword: &'a str,
    /// 1-based line that holds the keyword (the `@N` line when there is none).
    pub keyword_line: u32,
    /// Byte offset of the keyword within `keyword_line`.
    pub keyword_start: usize,
}

impl InstrumentHeader<'_> {
    pub fn instrument_type(&self) -> Option<InstrumentType> {
        InstrumentType::parse(self.keyword)
    }

    pub fn is(&self, keyword: &str) -> bool {
        self.keyword.eq_ignore_ascii_case(keyword)
    }

    pub fn keyword_end(&self) -> usize {
        self.keyword_start + self.keyword.len()
    }
}

/// Parse `@N` at the start of `line` (leading whitespace allowed). Returns
/// the number and the byte offset just past its digits. The digits must
/// be followed by the end of the line, whitespace or `;`.
pub fn parse_at_number(line: &str) -> Option<(u32, usize)> {
    let start = line.len() - line.trim_start().len();
    let rest = line[start..].strip_prefix('@')?;
    let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
    if digits == 0 {
        return None;
    }
    let end = start + 1 + digits;
    match line[end..].chars().next() {
        None | Some(';') => {}
        Some(ch) if ch.is_whitespace() => {}
        Some(_) => return None,
    }
    let number = rest[..digits].parse().ok()?;
    Some((number, end))
}

/// Strip a case-insensitive `keyword` prefix from `text`.
pub fn strip_keyword<'a>(text: &'a str, keyword: &str) -> Option<&'a str> {
    let head = text.get(..keyword.len())?;
    head.eq_ignore_ascii_case(keyword)
        .then(|| &text[keyword.len()..])
}

/// Parse the header whose `@N` sits on `line_number` (1-based).
pub fn instrument_header_at(
    model: &dyn LineReader,
    line_number: u32,
) -> Option<InstrumentHeader<'_>> {
    let line = model.get_line_content(line_number);
    let (number, after_number) = parse_at_number(line)?;
    let (keyword_start, keyword) = first_token(line, after_number);
    if !keyword.is_empty() {
        return Some(InstrumentHeader {
            number,
            keyword,
            keyword_line: line_number,
            keyword_start,
        });
    }
    let last = line_number
        .saturating_add(KEYWORD_LOOKAHEAD)
        .min(model.get_line_count());
    for next in line_number + 1..=last {
        let content = model.get_line_content(next);
        if is_blank_or_comment(content) {
            continue;
        }
        let (keyword_start, keyword) = first_token(content, 0);
        return Some(InstrumentHeader {
            number,
            keyword,
            keyword_line: next,
            keyword_start,
        });
    }
    Some(InstrumentHeader {
        number,
        keyword: "",
        keyword_line: line_number,
        keyword_start: line.len(),
    })
}

/// The header whose type keyword sits alone on `line_number`, below a bare
/// `@N` line.
pub fn instrument_header_for_keyword_line(
    model: &dyn LineReader,
    line_number: u32,
) -> Option<InstrumentHeader<'_>> {
    let first = line_number.saturating_sub(KEYWORD_LOOKAHEAD).max(1);
    for previous in (first..line_number).rev() {
        if is_blank_or_comment(model.get_line_content(previous)) {
            continue;
        }
        return instrument_header_at(model, previous)
            .filter(|header| header.keyword_line == line_number && !header.keyword.is_empty());
    }
    None
}

fn is_blank_or_comment(line: &str) -> bool {
    let trimmed = line.trim_start();
    trimmed.is_empty() || trimmed.starts_with(';')
}

/// The first tag token at or after `from`, delimited as ctrmml splits tag
/// values: whitespace, `"`, `,` and `;` (which also ends the line).
fn first_token(line: &str, from: usize) -> (usize, &str) {
    let rest = &line[from..];
    let start = from + (rest.len() - rest.trim_start().len());
    let token = &line[start..];
    let end = token
        .find(|ch: char| ch.is_whitespace() || matches!(ch, '"' | ',' | ';'))
        .unwrap_or(token.len());
    (start, &token[..end])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::string_model::LinesModel;

    fn header(lines: &[&str], line: u32) -> Option<(u32, String, u32, usize)> {
        let model = LinesModel::new(lines.iter().copied());
        instrument_header_at(&model, line).map(|header| {
            (
                header.number,
                header.keyword.to_string(),
                header.keyword_line,
                header.keyword_start,
            )
        })
    }

    #[test]
    fn at_number_needs_a_boundary_after_the_digits() {
        assert_eq!(parse_at_number("@12 fm"), Some((12, 3)));
        assert_eq!(parse_at_number("  @3"), Some((3, 4)));
        assert_eq!(parse_at_number("@3; lead"), Some((3, 2)));
        assert_eq!(parse_at_number("@1fm"), None);
        assert_eq!(parse_at_number("@fm"), None);
        assert_eq!(parse_at_number("A @1 fm"), None);
    }

    #[test]
    fn single_line_header() {
        assert_eq!(header(&["@5 fm 1 1"], 1), Some((5, "fm".to_string(), 1, 3)));
        assert_eq!(
            header(&["@5\tPSG,15"], 1),
            Some((5, "PSG".to_string(), 1, 3))
        );
        assert_eq!(
            header(&["@5 pcm\"a.wav\""], 1),
            Some((5, "pcm".to_string(), 1, 3))
        );
        assert_eq!(header(&["@5 fmt"], 1).map(|h| h.1), Some("fmt".into()));
    }

    #[test]
    fn multi_line_header_skips_blank_and_comment_lines() {
        let lines = ["@1", "; comment", "", "  fm ; type", "\t1 2"];
        assert_eq!(header(&lines, 1), Some((1, "fm".to_string(), 4, 2)));
    }

    #[test]
    fn multi_line_lookahead_stops_after_three_lines() {
        let lines = ["@1", "", "", "", "fm"];
        assert_eq!(header(&lines, 1), Some((1, String::new(), 1, 2)));
    }

    #[test]
    fn keyword_line_maps_back_to_its_header() {
        let model = LinesModel::new(["@1 ; lead", "; comment", "FM", "\t1 2"]);
        let found = instrument_header_for_keyword_line(&model, 3).expect("header");
        assert_eq!(found.number, 1);
        assert_eq!(found.instrument_type(), Some(InstrumentType::Fm));
        assert!(instrument_header_for_keyword_line(&model, 4).is_none());
    }

    #[test]
    fn type_keywords_are_case_insensitive() {
        assert_eq!(InstrumentType::parse("Fm"), Some(InstrumentType::Fm));
        assert_eq!(InstrumentType::parse("PSG"), Some(InstrumentType::Psg));
        assert_eq!(InstrumentType::parse("2op"), None);
    }
}
