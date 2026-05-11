//! Leading track selector parsing — ported from
//! `web-ctrmml/src/mml/track-selector.ts`.
//!
//! A ctrmml track line starts with a leading selector that names one or
//! more tracks the following MML applies to. The encoding mirrors ctrmml's
//! own parser:
//!
//! - `A`..`Z` map to `*0`..`*25`
//! - bare digits map to `*26`..`*35`
//! - `*<num>` addresses an explicit track
//!
//! The selector must be followed by whitespace (or end-of-line) to be valid.

/// One letter/digit/star token within a leading selector.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LeadingTrackSpan {
    pub track_id: u32,
    /// Byte offset of the span start within the line.
    pub start: usize,
    /// Byte offset just past the span end.
    pub end: usize,
}

/// A leading track selector parsed from the start of a line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LeadingTrackSelector {
    pub spans: Vec<LeadingTrackSpan>,
    /// Byte offset just past the last span (before any trailing whitespace).
    pub end: usize,
}

/// Source of line content addressed by 1-based line number — mirrors the
/// TS `LeadingTrackSelectorLineReader` shape used by
/// [`find_enclosing_track_selector`].
///
/// Implementors that don't know their total line count (e.g. infinite or
/// lazy streams) can fall back to the default
/// [`LineReader::get_line_count`] which reports `u32::MAX`; callers that
/// need a bounded forward scan can check for empty lines instead.
pub trait LineReader {
    fn get_line_content(&self, line_number: u32) -> &str;

    /// Total number of lines, 1-based. Defaults to `u32::MAX` so existing
    /// implementors keep working unchanged; consumers like
    /// [`crate::block_finder`] that walk forward to a boundary may use
    /// this to terminate.
    fn get_line_count(&self) -> u32 {
        u32::MAX
    }
}

/// Parse a ctrmml leading track selector starting exactly at column 0 of
/// `line`. Returns `None` when the line does not begin with a valid
/// selector.
///
/// The function operates on byte offsets and is ASCII-only by construction
/// (the selector alphabet is `A-Z`, `0-9`, `*`, and whitespace terminator).
/// Non-ASCII bytes in the selector region cause it to bail out, matching
/// the TS behavior which would also fail the char-range checks.
pub fn parse_leading_track_selector(line: &str) -> Option<LeadingTrackSelector> {
    let bytes = line.as_bytes();
    if bytes.is_empty() {
        return None;
    }
    let first = bytes[0];
    // The TS guard `first <= " "` rejects every control char up to and
    // including space; replicate by treating bytes <= 0x20 as terminators.
    if first <= b' ' || first == b';' || first == b'#' || first == b'@' {
        return None;
    }

    let mut spans: Vec<LeadingTrackSpan> = Vec::new();
    let mut idx = 0usize;

    while idx < bytes.len() {
        let ch = bytes[idx];
        if ch == b' ' || ch == b'\t' {
            break;
        }

        if ch.is_ascii_uppercase() {
            spans.push(LeadingTrackSpan {
                track_id: (ch - b'A') as u32,
                start: idx,
                end: idx + 1,
            });
            idx += 1;
            continue;
        }

        if ch.is_ascii_digit() {
            spans.push(LeadingTrackSpan {
                track_id: 26 + (ch - b'0') as u32,
                start: idx,
                end: idx + 1,
            });
            idx += 1;
            continue;
        }

        if ch == b'*' {
            let mut end = idx + 1;
            while end < bytes.len() && bytes[end].is_ascii_digit() {
                end += 1;
            }
            if end == idx + 1 {
                return None;
            }
            let track_id: u32 = line[idx + 1..end].parse().ok()?;
            spans.push(LeadingTrackSpan {
                track_id,
                start: idx,
                end,
            });
            idx = end;
            continue;
        }

        return None;
    }

    if spans.is_empty() {
        return None;
    }
    Some(LeadingTrackSelector { spans, end: idx })
}

/// Walk backward from `line_number` to find the nearest line that begins
/// with a leading track selector. Returns `None` if no selector is found
/// on or before `line_number`. `line_number` is 1-based.
pub fn find_enclosing_track_selector(
    model: &dyn LineReader,
    line_number: u32,
) -> Option<LeadingTrackSelector> {
    find_enclosing_track_selector_at(model, line_number).map(|(sel, _)| sel)
}

/// Same as [`find_enclosing_track_selector`] but also returns the 1-based
/// line number where the selector was found — useful for forward-scanners
/// that resume from that line without re-walking backward.
pub fn find_enclosing_track_selector_at(
    model: &dyn LineReader,
    line_number: u32,
) -> Option<(LeadingTrackSelector, u32)> {
    let mut ln = line_number;
    loop {
        if let Some(sel) = parse_leading_track_selector(model.get_line_content(ln)) {
            return Some((sel, ln));
        }
        if ln == 1 {
            return None;
        }
        ln -= 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::string_model::LinesModel;

    // ---------- parse_leading_track_selector ---------------------------------

    #[test]
    fn rejects_empty_and_whitespace() {
        assert!(parse_leading_track_selector("").is_none());
        assert!(parse_leading_track_selector(" A").is_none());
        assert!(parse_leading_track_selector("\tA").is_none());
    }

    #[test]
    fn rejects_lines_starting_with_special_chars() {
        assert!(parse_leading_track_selector(";comment").is_none());
        assert!(parse_leading_track_selector("#title foo").is_none());
        assert!(parse_leading_track_selector("@0 ...").is_none());
    }

    #[test]
    fn single_uppercase_letter() {
        let sel = parse_leading_track_selector("A cdefg").unwrap();
        assert_eq!(sel.spans.len(), 1);
        assert_eq!(sel.spans[0].track_id, 0);
        assert_eq!(sel.spans[0].start, 0);
        assert_eq!(sel.spans[0].end, 1);
        assert_eq!(sel.end, 1);
    }

    #[test]
    fn z_maps_to_25() {
        let sel = parse_leading_track_selector("Z ").unwrap();
        assert_eq!(sel.spans[0].track_id, 25);
    }

    #[test]
    fn digit_maps_to_26_plus() {
        let sel = parse_leading_track_selector("0 ").unwrap();
        assert_eq!(sel.spans[0].track_id, 26);
        let sel = parse_leading_track_selector("9 ").unwrap();
        assert_eq!(sel.spans[0].track_id, 35);
    }

    #[test]
    fn star_number_explicit_track() {
        let sel = parse_leading_track_selector("*42 cdefg").unwrap();
        assert_eq!(sel.spans.len(), 1);
        assert_eq!(sel.spans[0].track_id, 42);
        assert_eq!(sel.spans[0].start, 0);
        assert_eq!(sel.spans[0].end, 3);
        assert_eq!(sel.end, 3);
    }

    #[test]
    fn bare_star_without_digits_rejected() {
        assert!(parse_leading_track_selector("* cdefg").is_none());
    }

    #[test]
    fn multiple_spans_packed() {
        // "ABCD cdefg" — four tracks 0..=3.
        let sel = parse_leading_track_selector("ABCD cdefg").unwrap();
        assert_eq!(sel.spans.len(), 4);
        assert_eq!(
            sel.spans.iter().map(|s| s.track_id).collect::<Vec<_>>(),
            vec![0, 1, 2, 3]
        );
        assert_eq!(sel.end, 4);
    }

    #[test]
    fn mixed_letters_digits_stars() {
        let sel = parse_leading_track_selector("A0*5 cdefg").unwrap();
        let ids: Vec<u32> = sel.spans.iter().map(|s| s.track_id).collect();
        assert_eq!(ids, vec![0, 26, 5]);
        assert_eq!(sel.end, 4);
    }

    #[test]
    fn lowercase_letter_rejected() {
        // Track selectors only accept A-Z; lowercase is a note letter.
        assert!(parse_leading_track_selector("a cdefg").is_none());
    }

    #[test]
    fn star_followed_by_non_digit_rejected() {
        assert!(parse_leading_track_selector("*A").is_none());
    }

    #[test]
    fn end_of_line_terminates() {
        // No trailing whitespace; the selector still parses with end == len.
        let sel = parse_leading_track_selector("AB").unwrap();
        assert_eq!(sel.spans.len(), 2);
        assert_eq!(sel.end, 2);
    }

    // ---------- find_enclosing_track_selector --------------------------------

    #[test]
    fn finds_selector_on_same_line() {
        let model = LinesModel(vec!["A cdefg".into()]);
        let sel = find_enclosing_track_selector(&model, 1).unwrap();
        assert_eq!(sel.spans[0].track_id, 0);
    }

    #[test]
    fn walks_back_past_non_track_lines() {
        let model = LinesModel(vec![
            "#title \"Song\"".into(),
            "A cdefg".into(),
            "  more notes".into(),
            "  c d e f".into(),
        ]);
        let (sel, ln) = find_enclosing_track_selector_at(&model, 4).unwrap();
        assert_eq!(sel.spans[0].track_id, 0);
        assert_eq!(ln, 2);
    }

    #[test]
    fn returns_none_when_no_selector_exists() {
        let model = LinesModel(vec![
            "#title \"x\"".into(),
            ";comment".into(),
            "@0 fm".into(),
        ]);
        assert!(find_enclosing_track_selector(&model, 3).is_none());
    }
}
