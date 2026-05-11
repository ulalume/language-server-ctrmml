//! Per-channel octave context at a cursor — ported from
//! `web-ctrmml/src/mml/octave-scan.ts`.
//!
//! `scan_channel_context_at` walks forward from the enclosing track
//! selector to the cursor, tracking the effective octave for each channel
//! and which branch (channel) of any `{...}` block the cursor sits in.
//! `>`/`<` and `oN` inside a `{.../...}` branch affect only that branch's
//! channel; outside braces they affect every channel.

use crate::track_selector::{parse_leading_track_selector, LineReader};

const DEFAULT_OCTAVE: i32 = 6;

/// Per-channel octave plus the cursor's active branch.
///
/// `active_channel` is `-1` when the cursor is outside any `{...}` block,
/// otherwise a 0-based branch index. The `-1` sentinel mirrors the TS API
/// shape so existing callers' `< 0` checks port unchanged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelContext {
    /// Octave for each channel (0-indexed). `len() == max(num_channels, 1)`.
    pub octaves: Vec<i32>,
    /// Branch index the cursor sits in (0-based), or `-1` when outside.
    pub active_channel: i32,
}

/// Compute each channel's effective octave at `(line_number, column)` by
/// walking forward from the enclosing track selector.
///
/// Pass `track_line` to skip the backward scan when the caller already
/// knows it (typically from
/// [`crate::track_selector::find_enclosing_track_selector_at`]).
pub fn scan_channel_context_at(
    model: &dyn LineReader,
    line_number: u32,
    column: u32,
    num_channels: usize,
    track_line: Option<u32>,
) -> ChannelContext {
    let track_line = track_line.unwrap_or_else(|| {
        let mut ln = line_number;
        loop {
            if parse_leading_track_selector(model.get_line_content(ln)).is_some() {
                return ln;
            }
            if ln == 1 {
                return 1;
            }
            ln -= 1;
        }
    });

    let width = num_channels.max(1);
    let mut channels: Vec<i32> = vec![DEFAULT_OCTAVE; width];
    let mut brace_depth: u32 = 0;
    let mut branch_idx: usize = 0;
    let mut cur_channel: i32 = -1;

    for ln in track_line..=line_number {
        let line = model.get_line_content(ln);
        let bytes = line.as_bytes();
        let end_col = if ln == line_number {
            (column.saturating_sub(1) as usize).min(bytes.len())
        } else {
            bytes.len()
        };
        let eff_end = bytes[..end_col]
            .iter()
            .position(|&b| b == b';')
            .unwrap_or(end_col);

        let mut in_double = false;
        let mut in_single = false;
        let mut i = 0usize;
        while i < eff_end {
            let ch = bytes[i];
            if ch == b'"' && !in_single {
                in_double = !in_double;
                i += 1;
                continue;
            }
            if ch == b'\'' && !in_double {
                in_single = !in_single;
                i += 1;
                continue;
            }
            if in_double || in_single {
                i += 1;
                continue;
            }

            if ch == b'_' && i + 1 < eff_end && bytes[i + 1] == b'{' {
                if let Some(rel) = bytes[i + 2..eff_end].iter().position(|&b| b == b'}') {
                    i = i + 2 + rel + 1;
                    continue;
                }
            }

            if ch == b'o' || ch == b'O' {
                if let Some((oct, len)) = parse_leading_digits(&bytes[i + 1..eff_end]) {
                    apply_set(&mut channels, cur_channel, oct);
                    i += 1 + len;
                    continue;
                }
            }
            if ch == b'>' {
                apply_shift(&mut channels, cur_channel, 1);
                i += 1;
                continue;
            }
            if ch == b'<' {
                apply_shift(&mut channels, cur_channel, -1);
                i += 1;
                continue;
            }
            if ch == b'{' {
                brace_depth += 1;
                branch_idx = 0;
                cur_channel = if num_channels > 1 { 0 } else { -1 };
                i += 1;
                continue;
            }
            if ch == b'/' && brace_depth > 0 {
                branch_idx += 1;
                if num_channels > 1 && branch_idx < num_channels {
                    cur_channel = branch_idx as i32;
                }
                i += 1;
                continue;
            }
            if ch == b'}' && brace_depth > 0 {
                brace_depth -= 1;
                if brace_depth == 0 {
                    cur_channel = -1;
                }
                i += 1;
                continue;
            }
            i += 1;
        }
    }

    ChannelContext {
        octaves: channels,
        active_channel: cur_channel,
    }
}

#[inline]
fn apply_shift(channels: &mut [i32], cur_channel: i32, delta: i32) {
    if cur_channel < 0 {
        for c in channels.iter_mut() {
            *c += delta;
        }
    } else if (cur_channel as usize) < channels.len() {
        channels[cur_channel as usize] += delta;
    }
}

#[inline]
fn apply_set(channels: &mut [i32], cur_channel: i32, oct: i32) {
    if cur_channel < 0 {
        for c in channels.iter_mut() {
            *c = oct;
        }
    } else if (cur_channel as usize) < channels.len() {
        channels[cur_channel as usize] = oct;
    }
}

/// Parse a leading run of ASCII digits as an `i32`. Returns
/// `(value, byte_length)` or `None` when the slice doesn't start with a
/// digit.
fn parse_leading_digits(bytes: &[u8]) -> Option<(i32, usize)> {
    let mut end = 0;
    let mut value: i32 = 0;
    while end < bytes.len() && bytes[end].is_ascii_digit() {
        value = value
            .checked_mul(10)?
            .checked_add((bytes[end] - b'0') as i32)?;
        end += 1;
    }
    if end == 0 {
        None
    } else {
        Some((value, end))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::LinesModel;

    /// Parse `"...|..."` where `|` marks the cursor, returning the
    /// stripped text plus 1-based (line, col).
    fn cursor(text: &str) -> (String, u32, u32) {
        let idx = text.find('|').expect("cursor marker `|` missing");
        let mut stripped = String::with_capacity(text.len() - 1);
        stripped.push_str(&text[..idx]);
        stripped.push_str(&text[idx + 1..]);
        let before = &text[..idx];
        let newlines = before.bytes().filter(|&b| b == b'\n').count() as u32;
        let last_nl = before.rfind('\n');
        let col = match last_nl {
            Some(p) => before.len() - p - 1,
            None => before.len(),
        } as u32
            + 1;
        (stripped, newlines + 1, col)
    }

    fn scan(input: &str, num_channels: usize) -> Vec<i32> {
        let (text, line, col) = cursor(input);
        let model = LinesModel::new(text.split('\n'));
        scan_channel_context_at(&model, line, col, num_channels, None).octaves
    }

    fn ctx(input: &str, num_channels: usize) -> ChannelContext {
        let (text, line, col) = cursor(input);
        let model = LinesModel::new(text.split('\n'));
        scan_channel_context_at(&model, line, col, num_channels, None)
    }

    // ---------- octaves ------------------------------------------------------

    #[test]
    fn defaults_to_six_with_no_header_or_o() {
        assert_eq!(scan("|", 3), vec![6, 6, 6]);
    }

    #[test]
    fn picks_up_shared_o_command() {
        assert_eq!(scan("A o4 |c", 3), vec![4, 4, 4]);
    }

    #[test]
    fn shared_gt_bumps_all_channels() {
        assert_eq!(scan("A o4 >|c", 3), vec![5, 5, 5]);
    }

    #[test]
    fn gt_inside_branch_affects_only_that_channel() {
        assert_eq!(scan("A o4 {c/e/>g} |", 3), vec![4, 4, 5]);
    }

    #[test]
    fn state_accumulates_across_chord_blocks() {
        assert_eq!(scan("A o4 {f/a/>c} {g/b/d} |", 3), vec![4, 4, 5]);
    }

    #[test]
    fn lt_inside_branch_drops_only_that_channel() {
        assert_eq!(scan("A o5 {c/<e/<g} |", 3), vec![5, 4, 4]);
    }

    #[test]
    fn ignores_content_inside_double_quotes() {
        assert_eq!(scan("A o4 \"oops>>>\" |", 3), vec![4, 4, 4]);
    }

    #[test]
    fn ignores_content_inside_single_quotes() {
        assert_eq!(scan("A o4 'oops>>>' |", 3), vec![4, 4, 4]);
    }

    #[test]
    fn ignores_content_after_semicolon_comment() {
        assert_eq!(scan("A o4 ; >>>\n|", 3), vec![4, 4, 4]);
    }

    // ---------- active_channel ----------------------------------------------

    #[test]
    fn active_channel_negative_one_outside_brace() {
        assert_eq!(
            ctx("ABC o4 |", 3),
            ChannelContext {
                octaves: vec![4, 4, 4],
                active_channel: -1
            }
        );
    }

    #[test]
    fn active_channel_zero_inside_branch_zero() {
        assert_eq!(
            ctx("ABC o4 {|c/e/g}", 3),
            ChannelContext {
                octaves: vec![4, 4, 4],
                active_channel: 0
            }
        );
    }

    #[test]
    fn active_channel_one_after_first_slash() {
        assert_eq!(
            ctx("ABC o4 {c/|e/g}", 3),
            ChannelContext {
                octaves: vec![4, 4, 4],
                active_channel: 1
            }
        );
    }

    #[test]
    fn reports_third_channel_elevated_in_branch_zero_of_next_chord() {
        assert_eq!(
            ctx("ABC o4 {f/a/>c} {|g}", 3),
            ChannelContext {
                octaves: vec![4, 4, 5],
                active_channel: 0
            }
        );
    }

    #[test]
    fn honors_explicit_track_line_hint() {
        let model = LinesModel::new(["; header only", "ABC o4", ""]);
        let got = scan_channel_context_at(&model, 3, 1, 3, Some(2));
        assert_eq!(
            got,
            ChannelContext {
                octaves: vec![4, 4, 4],
                active_channel: -1
            }
        );
    }
}
