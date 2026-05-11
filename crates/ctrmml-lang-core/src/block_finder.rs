//! Instrument block finder — ported from
//! `web-ctrmml/src/mml/block-finder.ts`.
//!
//! Locates `@N fm` or `@N psg` blocks at a cursor line. The TS version
//! exposes a fully generic [`find_block_at`] (parameterized by header
//! and stop matchers) plus FM- and PSG-specialized wrappers; this port
//! mirrors that shape, using closures in place of TypeScript's
//! `RegExp` arguments.
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

/// Which instrument category to look for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstrumentKind {
    Fm,
    Psg,
}

/// Single-line header match: returns `Some(instrument_number)` when the
/// **trimmed** `line` is an `@N kind` header for `kind`, otherwise
/// `None`.
///
/// The TS regex is `/^@(\d+)\s+<kind>\b/i`. We implement that without
/// pulling in the `regex` crate: strip the leading `@`, read digits,
/// require whitespace, then read the keyword followed by a non-word
/// boundary.
fn match_header(line_trimmed: &str, kind: InstrumentKind) -> Option<u32> {
    let rest = line_trimmed.strip_prefix('@')?;
    let bytes = rest.as_bytes();
    let mut i = 0;
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        i += 1;
    }
    if i == 0 {
        return None;
    }
    let num: u32 = rest[..i].parse().ok()?;
    // Require at least one whitespace separator.
    let after_num = &rest[i..];
    let trimmed_after = after_num.trim_start();
    if trimmed_after.len() == after_num.len() {
        return None;
    }
    let keyword: &[u8] = match kind {
        InstrumentKind::Fm => b"fm",
        InstrumentKind::Psg => b"psg",
    };
    let kbytes = trimmed_after.as_bytes();
    if kbytes.len() < keyword.len() {
        return None;
    }
    for (a, b) in kbytes[..keyword.len()].iter().zip(keyword.iter()) {
        if a.to_ascii_lowercase() != *b {
            return None;
        }
    }
    // Word boundary: next char (if any) must not be alphanumeric or `_`.
    if let Some(&next) = kbytes.get(keyword.len()) {
        if next == b'_' || next.is_ascii_alphanumeric() {
            return None;
        }
    }
    Some(num)
}

/// Returns `true` when the trimmed line is a header of any of the kinds
/// listed in `stop_kinds`. Used to detect when an instrument block ends
/// because a header of a different category begins.
fn matches_any_stop(line_trimmed: &str, stop_kinds: &[InstrumentKind]) -> bool {
    stop_kinds
        .iter()
        .any(|k| match_header(line_trimmed, *k).is_some())
}

/// Returns `Some(n)` when the trimmed line is `@N` alone (optionally
/// followed by a `;` comment), as in a multi-line header layout.
fn match_bare_at_n(line_trimmed: &str) -> Option<u32> {
    let rest = line_trimmed.strip_prefix('@')?;
    let bytes = rest.as_bytes();
    let mut i = 0;
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        i += 1;
    }
    if i == 0 {
        return None;
    }
    let num: u32 = rest[..i].parse().ok()?;
    let after = rest[i..].trim_start();
    if after.is_empty() || after.starts_with(';') {
        Some(num)
    } else {
        None
    }
}

/// Find an instrument block of `kind` at `line_number` (1-based).
///
/// Scans upward to locate the header line, then downward to find the
/// block's end. Returns `None` if the cursor is not inside a matching
/// block, or if a `stop_kinds` header is encountered before finding the
/// target.
pub fn find_block_at(
    model: &dyn LineReader,
    line_number: u32,
    target: InstrumentKind,
    stop_kinds: &[InstrumentKind],
) -> Option<InstrumentBlock> {
    let total_lines = model.get_line_count();

    // -- Upward scan: locate the header line ---------------------------
    let mut header_line: u32 = 0;
    let mut instrument_number: u32 = 0;

    let mut i = line_number;
    loop {
        let raw = model.get_line_content(i);
        let line = raw.trim();

        // Single-line header.
        if let Some(n) = match_header(line, target) {
            header_line = i;
            instrument_number = n;
            break;
        }

        // Multi-line header: `@N` alone, then keyword on a later non-blank
        // non-comment line within three lines.
        if let Some(n) = match_bare_at_n(line) {
            let mut found = false;
            let probe_end = (i + 3).min(total_lines);
            for j in (i + 1)..=probe_end {
                let next_trimmed = model.get_line_content(j).trim();
                if next_trimmed.is_empty() || next_trimmed.starts_with(';') {
                    continue;
                }
                let synthetic = format!("@{n} {next_trimmed}");
                if let Some(syn_n) = match_header(&synthetic, target) {
                    header_line = i;
                    instrument_number = syn_n;
                    found = true;
                }
                break;
            }
            if found {
                break;
            }
            // `@N` for a different instrument category — stop.
            return None;
        }

        // Hard stops: any `@` or `#` header, the stop-kind list, or a
        // leading track selector marker.
        if line.starts_with('@')
            || line.starts_with('#')
            || matches_any_stop(line, stop_kinds)
            || parse_leading_track_selector(raw).is_some()
        {
            return None;
        }

        if i == 1 {
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
pub fn find_fm_block_at(
    model: &dyn LineReader,
    line_number: u32,
) -> Option<InstrumentBlock> {
    find_block_at(
        model,
        line_number,
        InstrumentKind::Fm,
        &[InstrumentKind::Psg],
    )
}

/// Find a PSG instrument block (`@N psg`) at `line_number`.
pub fn find_psg_block_at(
    model: &dyn LineReader,
    line_number: u32,
) -> Option<InstrumentBlock> {
    find_block_at(
        model,
        line_number,
        InstrumentKind::Psg,
        &[InstrumentKind::Fm],
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::string_model::LinesModel;

    // ---- match_header --------------------------------------------------------

    #[test]
    fn matches_inline_fm_header() {
        assert_eq!(match_header("@5 fm", InstrumentKind::Fm), Some(5));
    }

    #[test]
    fn matches_fm_with_trailing_data() {
        assert_eq!(match_header("@5 fm 1 1 1 1", InstrumentKind::Fm), Some(5));
    }

    #[test]
    fn rejects_wrong_kind() {
        assert!(match_header("@5 psg", InstrumentKind::Fm).is_none());
    }

    #[test]
    fn rejects_word_boundary_violation() {
        // `fmt` should not match `fm` (no word boundary).
        assert!(match_header("@5 fmt", InstrumentKind::Fm).is_none());
    }

    #[test]
    fn case_insensitive() {
        assert_eq!(match_header("@7 FM", InstrumentKind::Fm), Some(7));
        assert_eq!(match_header("@7 Psg", InstrumentKind::Psg), Some(7));
    }

    #[test]
    fn rejects_missing_whitespace() {
        assert!(match_header("@5fm", InstrumentKind::Fm).is_none());
    }

    // ---- match_bare_at_n -----------------------------------------------------

    #[test]
    fn bare_at_n_matches_with_trailing_comment() {
        assert_eq!(match_bare_at_n("@12 ; lead patch"), Some(12));
    }

    #[test]
    fn bare_at_n_matches_just_digits() {
        assert_eq!(match_bare_at_n("@3"), Some(3));
    }

    #[test]
    fn bare_at_n_rejects_with_keyword() {
        assert!(match_bare_at_n("@3 fm").is_none());
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
        let model = LinesModel::new([
            "@9 psg",
            "\t15>10:5",
            "\t31,0,12,7,0,28,0,0,5,0",
        ]);
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
