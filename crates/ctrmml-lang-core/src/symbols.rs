//! Definitions and the document outline.
//!
//! Positions are 0-based lines and UTF-16 characters.

use serde::Serialize;

use crate::block_finder::find_block_at;
use crate::completion::{
    byte_offset_to_utf16_character, utf16_character_to_byte_offset, EditRange, Pos,
};
use crate::docs::CHANNEL_DESCRIPTIONS;
use crate::instrument_header::{
    instrument_header_at, instrument_name, parse_at_number, strip_keyword,
};
use crate::string_model::BorrowedLinesModel;
use crate::text_scan::{is_in_comment, token_at};
use crate::track_selector::{parse_leading_track_selector, LineReader};

/// Definition prefixes, longest first: `@E1`, `@M1`, `@P1` tables, `@1`
/// instruments and `*1` tracks.
const DEFINITION_PREFIXES: [&str; 5] = ["@E", "@M", "@P", "@", "*"];

/// The definition of the `@N`, `@EN`, `@MN`, `@PN` or `*N` reference at the
/// position: the first line that starts with it, followed by whitespace,
/// `;` or the end of the line. Returns the defining name's range.
pub fn definition_at(text: &str, line: u32, character: u32) -> Option<EditRange> {
    let model = BorrowedLinesModel::from_text(text);
    let content = model.get_line_content(line.saturating_add(1));
    let col = utf16_character_to_byte_offset(content, character);
    if is_in_comment(content, col) {
        return None;
    }
    let (token, _, _) = token_at(content, col)?;
    let prefix = DEFINITION_PREFIXES.into_iter().find(|prefix| {
        token
            .strip_prefix(prefix)
            .is_some_and(|rest| rest.starts_with(|ch: char| ch.is_ascii_digit()))
    })?;
    let digits_len = token[prefix.len()..]
        .bytes()
        .take_while(u8::is_ascii_digit)
        .count();
    let name = &token[..prefix.len() + digits_len];

    (1..=model.get_line_count()).find_map(|number| {
        let content = model.get_line_content(number);
        let start = content.len() - content.trim_start().len();
        let after = strip_keyword(&content[start..], name)?;
        if !after.is_empty() && !after.starts_with(|ch: char| ch.is_whitespace() || ch == ';') {
            return None;
        }
        Some(span(content, number - 1, start, start + name.len()))
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SymbolKind {
    /// `#title`, `#platform`, …
    Meta,
    /// `@N <type>`.
    Instrument,
    /// `@EN`, `@MN`, `@PN` tables.
    Envelope,
    /// The first line of each track selector (`A`, `GHI`, `*40`).
    Track,
}

/// One outline entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DocumentSymbol {
    pub name: String,
    pub detail: Option<String>,
    pub kind: SymbolKind,
    /// The whole definition: an instrument's block, or the line.
    pub range: EditRange,
    /// The name as written.
    pub selection_range: EditRange,
}

/// The document's definitions in order: meta lines, instruments, envelope
/// tables and the first line of each distinct track selector.
pub fn document_symbols(text: &str) -> Vec<DocumentSymbol> {
    let model = BorrowedLinesModel::from_text(text);
    let mut out = Vec::new();
    let mut tracks_seen: Vec<&str> = Vec::new();
    for number in 1..=model.get_line_count() {
        let line = model.get_line_content(number);
        let index = number - 1;
        let start = line.len() - line.trim_start().len();
        let trimmed = &line[start..];

        if let Some(selector) = parse_leading_track_selector(line) {
            let name = &line[..selector.end];
            if tracks_seen.contains(&name) {
                continue;
            }
            tracks_seen.push(name);
            let channels: Vec<&str> = selector
                .spans
                .iter()
                .filter_map(|span| CHANNEL_DESCRIPTIONS.get(span.track_id as usize).copied())
                .filter(|description| *description != "Dummy")
                .collect();
            out.push(DocumentSymbol {
                name: name.to_string(),
                detail: (!channels.is_empty()).then(|| channels.join(", ")),
                kind: SymbolKind::Track,
                range: whole_line(line, index),
                selection_range: span(line, index, 0, selector.end),
            });
        } else if trimmed.starts_with('#') {
            let end = start + trimmed.find(char::is_whitespace).unwrap_or(trimmed.len());
            let value = line[end..].trim();
            out.push(DocumentSymbol {
                name: line[start..end].to_string(),
                detail: (!value.is_empty()).then(|| value.to_string()),
                kind: SymbolKind::Meta,
                range: whole_line(line, index),
                selection_range: span(line, index, start, end),
            });
        } else if let Some(header) = instrument_header_at(&model, number) {
            let end_line = header
                .instrument_type()
                .and_then(|ty| find_block_at(&model, number, ty))
                .map_or(header.keyword_line, |block| block.end_line);
            let detail = match (header.keyword, instrument_name(line)) {
                ("", None) => None,
                ("", Some(comment)) => Some(comment.to_string()),
                (keyword, None) => Some(keyword.to_string()),
                (keyword, Some(comment)) => Some(format!("{keyword} {comment}")),
            };
            let name_end = parse_at_number(line).map_or(line.len(), |(_, end)| end);
            let last = model.get_line_content(end_line);
            out.push(DocumentSymbol {
                name: line[start..name_end].to_string(),
                detail,
                kind: SymbolKind::Instrument,
                range: EditRange::new(
                    Pos {
                        line: index,
                        character: 0,
                    },
                    Pos {
                        line: end_line - 1,
                        character: byte_offset_to_utf16_character(last, last.len()),
                    },
                ),
                selection_range: span(line, index, start, name_end),
            });
        } else if let Some(name_end) = table_name_end(trimmed) {
            out.push(DocumentSymbol {
                name: trimmed[..name_end].to_string(),
                detail: None,
                kind: SymbolKind::Envelope,
                range: whole_line(line, index),
                selection_range: span(line, index, start, start + name_end),
            });
        }
    }
    out
}

/// The end of a leading `@EN` / `@MN` / `@PN` name, when it is followed by
/// whitespace, `;` or the end of the line.
fn table_name_end(trimmed: &str) -> Option<usize> {
    let rest = ["@E", "@M", "@P"]
        .into_iter()
        .find_map(|prefix| strip_keyword(trimmed, prefix))?;
    let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
    if digits == 0 {
        return None;
    }
    match rest[digits..].chars().next() {
        None | Some(';') => {}
        Some(ch) if ch.is_whitespace() => {}
        Some(_) => return None,
    }
    Some(2 + digits)
}

fn whole_line(line: &str, index: u32) -> EditRange {
    span(line, index, 0, line.len())
}

fn span(line: &str, index: u32, start: usize, end: usize) -> EditRange {
    EditRange::new(
        Pos {
            line: index,
            character: byte_offset_to_utf16_character(line, start),
        },
        Pos {
            line: index,
            character: byte_offset_to_utf16_character(line, end),
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn range(line: u32, start: u32, end: u32) -> EditRange {
        EditRange::new(
            Pos {
                line,
                character: start,
            },
            Pos {
                line,
                character: end,
            },
        )
    }

    #[test]
    fn definitions_are_found_by_prefix_and_number() {
        let text = "@10 fm\n@1 psg 15\n@E1 1 2\n*3 c d\nA @1 @E1 *3\n";
        assert_eq!(definition_at(text, 4, 3), Some(range(1, 0, 2)));
        assert_eq!(definition_at(text, 4, 6), Some(range(2, 0, 3)));
        assert_eq!(definition_at(text, 4, 10), Some(range(3, 0, 2)));
    }

    #[test]
    fn definition_positions_count_utf16_units() {
        // `あ` and the ideographic space are one UTF-16 unit and three bytes.
        let text = "\u{3000}@1 fm\nA 'あ' @1\n";
        assert_eq!(definition_at(text, 1, 8), Some(range(0, 1, 3)));
    }

    #[test]
    fn a_definition_needs_a_boundary_after_its_number() {
        assert_eq!(definition_at("@1fm\nA @1\n", 1, 3), None);
        assert_eq!(definition_at("@12 fm\nA @1\n", 1, 3), None);
    }

    #[test]
    fn references_in_comments_have_no_definition() {
        assert_eq!(definition_at("@1 fm\nA c ; @1\n", 1, 7), None);
    }

    #[test]
    fn symbols_list_meta_instruments_tables_and_first_track_lines() {
        let text = concat!(
            "#title Song\n",
            "@1 fm ; Bass\n",
            " 3 4\n",
            " 31 0 0 0 0 0 0 0 0 0\n",
            "\n",
            "@2 psg 15 0\n",
            "@M1 0 12\n",
            "@3\n",
            "  2op 1 2 3 4 0\n",
            "A t120 @1 c\n",
            "B c\n",
            "A d\n",
            "*40 c\n",
        );
        let symbols = document_symbols(text);
        let summary: Vec<(&str, Option<&str>, SymbolKind)> = symbols
            .iter()
            .map(|symbol| (symbol.name.as_str(), symbol.detail.as_deref(), symbol.kind))
            .collect();
        assert_eq!(
            summary,
            [
                ("#title", Some("Song"), SymbolKind::Meta),
                ("@1", Some("fm Bass"), SymbolKind::Instrument),
                ("@2", Some("psg"), SymbolKind::Instrument),
                ("@M1", None, SymbolKind::Envelope),
                ("@3", Some("2op"), SymbolKind::Instrument),
                ("A", Some("FM1"), SymbolKind::Track),
                ("B", Some("FM2"), SymbolKind::Track),
                ("*40", None, SymbolKind::Track),
            ]
        );
        assert_eq!(
            symbols[1].range,
            EditRange::new(
                Pos {
                    line: 1,
                    character: 0
                },
                Pos {
                    line: 3,
                    character: 21
                }
            )
        );
        assert_eq!(symbols[1].selection_range, range(1, 0, 2));
        assert_eq!(symbols[4].range.end.line, 8);
        assert_eq!(symbols[0].selection_range, range(0, 0, 6));
    }

    #[test]
    fn symbol_ranges_count_utf16_units() {
        let symbols = document_symbols("#title あい\n");
        assert_eq!(symbols[0].range, range(0, 0, 9));
        assert_eq!(symbols[0].detail.as_deref(), Some("あい"));
    }
}
