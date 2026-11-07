//! Instrument block finder — ported from
//! `web-ctrmml/src/mml/block-finder.ts`.
//!
//! Locates `@N fm`, `@N psg` or `@N pcm` blocks at a cursor line. Headers
//! are parsed by [`crate::instrument_header`].
//!
//! Block layout the finder recognises:
//!
//! ```text
//! @5 fm                ; single-line header
//! 1 1 1 1
//!  ...
//! ```
//!
//! ```text
//! @5                   ; multi-line header — `@N` alone, then `fm`/`psg`
//! fm                   ;   appears within the next three non-blank
//! 1 1 1 1              ;   non-comment lines.
//! ```
//!
//! Block end is the line *before* any of: `@`/`#` header, leading track
//! selector, or two consecutive blank lines.

use crate::instrument_header::{instrument_header_at, InstrumentType};
use crate::track_selector::{parse_leading_track_selector, LineReader};

/// One instrument-block region.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InstrumentBlock {
    /// 1-based line where the `@N` header lives.
    pub start_line: u32,
    /// 1-based line of the last content line in the block.
    pub end_line: u32,
    /// The instrument number parsed from the `@N` header.
    pub instrument_number: u32,
}

/// Find an instrument block of type `target` at `line_number` (1-based).
///
/// Scans upward to locate the header line, then downward to find the
/// block's end. Returns `None` if the cursor is not inside a matching
/// block or another header is met first.
pub fn find_block_at(
    model: &dyn LineReader,
    line_number: u32,
    target: InstrumentType,
) -> Option<InstrumentBlock> {
    let total_lines = model.get_line_count();

    // -- Upward scan: locate the header line ---------------------------
    let mut header_line: u32 = 0;
    let mut instrument_number: u32 = 0;

    let mut i = line_number;
    loop {
        if let Some(header) = instrument_header_at(model, i) {
            if header.instrument_type() != Some(target) {
                return None;
            }
            header_line = i;
            instrument_number = header.number;
            break;
        }

        // Hard stops: any other `@` or `#` line, or a leading track selector.
        let raw = model.get_line_content(i);
        let line = raw.trim();
        if line.starts_with('@')
            || line.starts_with('#')
            || parse_leading_track_selector(raw).is_some()
        {
            return None;
        }

        if i <= 1 {
            break;
        }
        i -= 1;
    }
    if header_line == 0 {
        return None;
    }

    // -- Downward scan: find block end ---------------------------------
    let mut end_line = header_line;
    let mut consecutive_blanks: u32 = 0;
    for i in (header_line + 1)..=total_lines {
        let raw = model.get_line_content(i);
        let line = raw.trim();
        if line.starts_with('@')
            || line.starts_with('#')
            || parse_leading_track_selector(raw).is_some()
        {
            break;
        }
        if line.is_empty() {
            consecutive_blanks += 1;
            if consecutive_blanks >= 2 {
                break;
            }
        } else if line.starts_with(';') {
            // Comment lines neither count as blanks nor reset the counter.
        } else {
            consecutive_blanks = 0;
        }
        end_line = i;
    }

    // Trim trailing blank / comment-only lines.
    while end_line > header_line {
        let line = model.get_line_content(end_line);
        let trimmed = line.trim();
        if !trimmed.is_empty() && !trimmed.starts_with(';') {
            break;
        }
        end_line -= 1;
    }

    if line_number < header_line || line_number > end_line {
        return None;
    }
    Some(InstrumentBlock {
        start_line: header_line,
        end_line,
        instrument_number,
    })
}

/// Find an FM instrument block (`@N fm`) at `line_number`.
pub fn find_fm_block_at(model: &dyn LineReader, line_number: u32) -> Option<InstrumentBlock> {
    find_block_at(model, line_number, InstrumentType::Fm)
}

/// Find a PSG instrument block (`@N psg`) at `line_number`.
pub fn find_psg_block_at(model: &dyn LineReader, line_number: u32) -> Option<InstrumentBlock> {
    find_block_at(model, line_number, InstrumentType::Psg)
}

/// Find a PCM instrument block (`@N pcm`) at `line_number`.
pub fn find_pcm_block_at(model: &dyn LineReader, line_number: u32) -> Option<InstrumentBlock> {
    find_block_at(model, line_number, InstrumentType::Pcm)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::string_model::LinesModel;

    // ---- headers -------------------------------------------------------------

    #[test]
    fn header_type_is_case_insensitive() {
        let model = LinesModel::new(["@7 FM", "\t31,0,12,7,0,28,0,0,5,0"]);
        assert_eq!(
            find_fm_block_at(&model, 2).map(|b| b.instrument_number),
            Some(7)
        );
    }

    #[test]
    fn header_needs_whitespace_after_the_number() {
        let model = LinesModel::new(["@5fm", "\t31,0,12,7,0,28,0,0,5,0"]);
        assert!(find_fm_block_at(&model, 2).is_none());
    }

    #[test]
    fn header_keyword_needs_a_token_boundary() {
        let model = LinesModel::new(["@5 fmt", "\t31,0,12,7,0,28,0,0,5,0"]);
        assert!(find_fm_block_at(&model, 2).is_none());
    }

    // ---- find_fm_block_at / find_psg_block_at -------------------------------

    // Real-world FM/PSG data lines are always indented (tab or space)
    // so `parse_leading_track_selector` doesn't misread them as track
    // headers — the leading whitespace short-circuits that scanner.
    // These tests mirror the layout used in
    // `web-ctrmml/presets/*/<song>.mml`.

    #[test]
    fn finds_simple_fm_block() {
        let model = LinesModel::new([
            "@1 fm",
            "\t31,0,12,7,0,28,0,0,5,0",
            "\t31,0,2,6,0,0,0,0,2,0",
            "",
            "A o4 cdefg",
        ]);
        let block = find_fm_block_at(&model, 2).unwrap();
        assert_eq!(block.start_line, 1);
        assert_eq!(block.instrument_number, 1);
        assert_eq!(block.end_line, 3);
    }

    #[test]
    fn finds_block_from_header_line() {
        let model = LinesModel::new(["@1 fm", "\t31,0,12,7,0,28,0,0,5,0"]);
        let block = find_fm_block_at(&model, 1).unwrap();
        assert_eq!(block.start_line, 1);
        assert_eq!(block.end_line, 2);
    }

    #[test]
    fn multi_line_header_form() {
        let model = LinesModel::new([
            "@2",
            "; some comment",
            "fm",
            "\t31,0,12,7,0,28,0,0,5,0",
            "",
            "A c",
        ]);
        let block = find_fm_block_at(&model, 4).unwrap();
        assert_eq!(block.start_line, 1);
        assert_eq!(block.instrument_number, 2);
    }

    #[test]
    fn stops_at_other_instrument_kind() {
        // PSG block immediately above; FM lookup must not span across it.
        let model = LinesModel::new(["@9 psg", "\t15>10:5", "\t31,0,12,7,0,28,0,0,5,0"]);
        assert!(find_fm_block_at(&model, 3).is_none());
    }

    #[test]
    fn stops_at_track_selector() {
        // Track selector `A` blocks the upward scan from line 2.
        let model = LinesModel::new(["A o4 cdefg", "\t31,0,12,7,0,28,0,0,5,0"]);
        assert!(find_fm_block_at(&model, 2).is_none());
    }

    #[test]
    fn returns_none_when_cursor_past_block_end() {
        let model = LinesModel::new([
            "@1 fm",
            "\t31,0,12,7,0,28,0,0,5,0",
            "",
            "",
            "; well past the block",
        ]);
        // After two blank lines the block ended at line 2; cursor at line 5
        // is outside.
        assert!(find_fm_block_at(&model, 5).is_none());
    }

    #[test]
    fn trims_trailing_blank_and_comment_lines_from_end() {
        let model = LinesModel::new([
            "@1 fm",
            "\t31,0,12,7,0,28,0,0,5,0",
            "; trailing comment",
            "; another comment",
        ]);
        let block = find_fm_block_at(&model, 2).unwrap();
        assert_eq!(block.end_line, 2);
    }

    #[test]
    fn finds_psg_block() {
        let model = LinesModel::new(["@3 psg", "\t15>0:5", "", "A c"]);
        let block = find_psg_block_at(&model, 2).unwrap();
        assert_eq!(block.start_line, 1);
        assert_eq!(block.instrument_number, 3);
        assert_eq!(block.end_line, 2);
    }

    #[test]
    fn two_consecutive_blanks_end_block() {
        let model = LinesModel::new([
            "@1 fm",
            "\t31,0,12,7,0,28,0,0,5,0",
            "",
            "",
            "\t5,5,5,5", // past the boundary
        ]);
        let block = find_fm_block_at(&model, 2).unwrap();
        // Two consecutive blanks closed the block at line 2.
        assert_eq!(block.end_line, 2);
    }
}
