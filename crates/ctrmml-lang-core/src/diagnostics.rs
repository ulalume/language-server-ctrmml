//! `ctrmml-cmd` check reports and playback positions as UTF-16 ranges.
//!
//! The compiler counts columns in UTF-8 bytes; editors count UTF-16 code
//! units. These conversions are the only place the two meet.

use serde::{Deserialize, Serialize};

use crate::completion::{byte_offset_to_utf16_character, EditRange, Pos};

/// The JSON report of `ctrmml-cmd check --json`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct CheckReport {
    #[serde(default)]
    pub errors: Vec<CheckMessage>,
    #[serde(default)]
    pub warnings: Vec<CheckMessage>,
}

/// One check message: a 1-based line, a 1-based byte column and a byte
/// length.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct CheckMessage {
    #[serde(default)]
    pub message: String,
    #[serde(default)]
    pub line: u32,
    #[serde(default)]
    pub col: u32,
    #[serde(default)]
    pub length: u32,
    #[serde(default)]
    pub code: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Error,
    Warning,
}

/// A check message placed on the document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CheckDiagnostic {
    pub severity: Severity,
    pub message: String,
    pub code: Option<String>,
    pub range: EditRange,
}

/// Errors then warnings of `report`, with UTF-16 ranges in `text`.
/// Messages with empty text are dropped. A message spans at least one
/// character.
pub fn check_report_to_diagnostics(text: &str, report: &CheckReport) -> Vec<CheckDiagnostic> {
    let lines = split_lines(text);
    let errors = report.errors.iter().map(|msg| (msg, Severity::Error));
    let warnings = report.warnings.iter().map(|msg| (msg, Severity::Warning));
    errors
        .chain(warnings)
        .filter(|(msg, _)| !msg.message.trim().is_empty())
        .map(|(msg, severity)| {
            let line_idx = msg.line.saturating_sub(1);
            let line = lines.get(line_idx as usize).copied().unwrap_or("");
            let start = floor_char_boundary(line, msg.col.saturating_sub(1) as usize);
            let end = if line.is_empty() {
                start
            } else {
                ceil_char_boundary(line, start + msg.length.max(1) as usize)
            };
            CheckDiagnostic {
                severity,
                message: msg.message.clone(),
                code: (!msg.code.trim().is_empty()).then(|| msg.code.clone()),
                range: utf16_range(line, line_idx, start, end),
            }
        })
        .collect()
}

/// A sounding event's position: 0-based line and 0-based byte column.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PlaybackPosition {
    pub line: u32,
    pub col: u32,
}

/// One-character UTF-16 ranges for `positions`, without repeats. Positions
/// past the last line are dropped.
pub fn playback_ranges(text: &str, positions: &[PlaybackPosition]) -> Vec<EditRange> {
    let lines = split_lines(text);
    let mut seen = Vec::new();
    let mut out = Vec::new();
    for pos in positions {
        let Some(line) = lines.get(pos.line as usize) else {
            continue;
        };
        if seen.contains(pos) {
            continue;
        }
        seen.push(*pos);
        let start = floor_char_boundary(line, pos.col as usize);
        let end = ceil_char_boundary(line, start + 1);
        out.push(utf16_range(line, pos.line, start, end));
    }
    out
}

fn split_lines(text: &str) -> Vec<&str> {
    text.split('\n')
        .map(|line| line.strip_suffix('\r').unwrap_or(line))
        .collect()
}

fn floor_char_boundary(line: &str, index: usize) -> usize {
    let mut index = index.min(line.len());
    while !line.is_char_boundary(index) {
        index -= 1;
    }
    index
}

fn ceil_char_boundary(line: &str, index: usize) -> usize {
    let mut index = index.min(line.len());
    while !line.is_char_boundary(index) {
        index += 1;
    }
    index
}

fn utf16_range(line: &str, line_idx: u32, start: usize, end: usize) -> EditRange {
    EditRange::new(
        Pos {
            line: line_idx,
            character: byte_offset_to_utf16_character(line, start),
        },
        Pos {
            line: line_idx,
            character: byte_offset_to_utf16_character(line, end),
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message(line: u32, col: u32, length: u32) -> CheckMessage {
        CheckMessage {
            message: "unterminated '[]' loop".to_string(),
            line,
            col,
            length,
            code: "parse_error".to_string(),
        }
    }

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
    fn check_columns_after_japanese_text_count_utf16_units() {
        // ctrmml-cmd reports the `]` of `A 'あ' c ] e` at byte column 11.
        let report = CheckReport {
            errors: vec![message(1, 11, 0)],
            warnings: Vec::new(),
        };
        let diagnostics = check_report_to_diagnostics("A 'あ' c ] e\n", &report);
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].range, range(0, 8, 9));
        assert_eq!(diagnostics[0].severity, Severity::Error);
        assert_eq!(diagnostics[0].code.as_deref(), Some("parse_error"));
    }

    #[test]
    fn errors_come_before_warnings_and_empty_messages_are_dropped() {
        let mut warning = message(2, 1, 2);
        warning.message = "w".to_string();
        warning.code = " ".to_string();
        let mut empty = message(1, 1, 1);
        empty.message = "  ".to_string();
        let report = CheckReport {
            errors: vec![empty, message(1, 3, 1)],
            warnings: vec![warning],
        };
        let diagnostics = check_report_to_diagnostics("A c\nB d\n", &report);
        assert_eq!(diagnostics.len(), 2);
        assert_eq!(diagnostics[0].severity, Severity::Error);
        assert_eq!(diagnostics[1].severity, Severity::Warning);
        assert_eq!(diagnostics[1].range, range(1, 0, 2));
        assert_eq!(diagnostics[1].code, None);
    }

    #[test]
    fn a_column_inside_a_character_widens_to_the_whole_character() {
        let report = CheckReport {
            errors: vec![message(1, 4, 1)],
            warnings: Vec::new(),
        };
        let diagnostics = check_report_to_diagnostics("A 'あ'", &report);
        assert_eq!(diagnostics[0].range, range(0, 3, 4));
    }

    #[test]
    fn messages_on_missing_or_empty_lines_stay_at_their_start() {
        let report = CheckReport {
            errors: vec![message(0, 0, 3), message(9, 4, 1)],
            warnings: Vec::new(),
        };
        let diagnostics = check_report_to_diagnostics("A c\n", &report);
        assert_eq!(diagnostics[0].range, range(0, 0, 3));
        assert_eq!(diagnostics[1].range, range(8, 0, 0));
    }

    #[test]
    fn playback_positions_after_japanese_text_count_utf16_units() {
        let positions = [
            PlaybackPosition { line: 0, col: 8 },
            PlaybackPosition { line: 0, col: 8 },
            PlaybackPosition { line: 3, col: 0 },
        ];
        assert_eq!(
            playback_ranges("A 'あ' c d\n", &positions),
            [range(0, 6, 7)]
        );
    }
}
