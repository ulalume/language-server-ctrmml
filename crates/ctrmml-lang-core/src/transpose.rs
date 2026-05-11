//! Semitone transposition for MML selections — ported from
//! `web-ctrmml/src/mml/transpose.ts`.
//!
//! Algorithm — Lift → Transform → Lower:
//!  1. Forward-scan the selection, tracking octave state per channel.
//!     Compute the absolute MIDI pitch for every note token.
//!  2. Shift all MIDI values by ±1.
//!  3. Rebuild the selection text: replace note letters/accidentals,
//!     regenerate `>` / `<` for octave boundary crossings, keep
//!     everything else (lengths, rests, commands) untouched.

use std::collections::{HashMap, HashSet};

use crate::brace_state::BraceState;
use crate::chord::chord_natural_semitones;
use crate::key_sig::{parse_key_sig, scan_key_sig_at, KeySig};
use crate::octave_scan::scan_brace_state_at;
use crate::text_scan::{is_in_comment, is_in_key_sig};
use crate::track_selector::{find_enclosing_track_selector, LineReader};

// ---------------------------------------------------------------------------
// Shared constants
// ---------------------------------------------------------------------------

/// Preferred sharp spellings for semitones 0..=11: `(letter, accidental)`.
const SPELL_SHARP: [(char, &str); 12] = [
    ('c', ""),
    ('c', "+"),
    ('d', ""),
    ('d', "+"),
    ('e', ""),
    ('f', ""),
    ('f', "+"),
    ('g', ""),
    ('g', "+"),
    ('a', ""),
    ('a', "+"),
    ('b', ""),
];

/// Preferred flat spellings.
const SPELL_FLAT: [(char, &str); 12] = [
    ('c', ""),
    ('d', "-"),
    ('d', ""),
    ('e', "-"),
    ('e', ""),
    ('f', ""),
    ('g', "-"),
    ('g', ""),
    ('a', "-"),
    ('a', ""),
    ('b', "-"),
    ('b', ""),
];

const NOTE_LETTERS: [char; 7] = ['c', 'd', 'e', 'f', 'g', 'a', 'b'];

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Transposition direction. The TS uses the literal type `1 | -1`; we use
/// an enum so callers can't pass invalid values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Up,
    Down,
}

impl Direction {
    #[inline]
    fn as_i32(self) -> i32 {
        match self {
            Direction::Up => 1,
            Direction::Down => -1,
        }
    }
}

/// 1-based selection range (Monaco convention) addressing a sub-region of
/// the text source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Selection {
    pub start_line_number: u32,
    pub start_column: u32,
    pub end_line_number: u32,
    pub end_column: u32,
}

/// Edit operation returned by [`transpose_selection`]: replace the
/// selection range with `text`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransposeEdit {
    pub start_line_number: u32,
    pub start_column: u32,
    pub end_line_number: u32,
    pub end_column: u32,
    pub text: String,
}

// ---------------------------------------------------------------------------
// Spell helper
// ---------------------------------------------------------------------------

/// Choose the best MML spelling (letter + accidental string) for a semitone
/// given the ambient key signature and the transposition direction.
///
/// Priority:
///  1. A letter whose key-sig-adjusted pitch matches → no accidental.
///  2. A letter whose natural pitch matches → emit `=` to neutralise key sig.
///  3. Direction-preferred table (sharp for up, flat for down).
fn spell_semitone(semi: i32, key_sig: &KeySig, direction: Direction) -> (char, &'static str) {
    let semi = semi.rem_euclid(12);

    // 1. Natural match via key sig (no explicit accidental needed).
    for &letter in &NOTE_LETTERS {
        let adjusted = (chord_natural_semitones(letter).unwrap()
            + key_sig.get_or_zero(letter) as i32)
            .rem_euclid(12);
        if adjusted == semi {
            return (letter, "");
        }
    }

    // 2. Natural pitch matches but key sig shifts it away → use `=`.
    for &letter in &NOTE_LETTERS {
        if chord_natural_semitones(letter).unwrap() == semi
            && key_sig.get_or_zero(letter) != 0
        {
            return (letter, "=");
        }
    }

    // 3. Direction-preferred table.
    let table = match direction {
        Direction::Up => &SPELL_SHARP,
        Direction::Down => &SPELL_FLAT,
    };
    let (letter, base_acc) = table[semi as usize];
    let sig_val = key_sig.get_or_zero(letter) as i32;
    let needed_acc = match base_acc {
        "+" => 1,
        "-" => -1,
        _ => 0,
    };
    if sig_val == needed_acc {
        return (letter, "");
    }
    if needed_acc == 0 && sig_val != 0 {
        return (letter, "=");
    }
    (letter, base_acc)
}

// ---------------------------------------------------------------------------
// Core: transpose_selection
// ---------------------------------------------------------------------------

#[derive(Debug)]
struct NoteToken {
    /// Offset within the flattened selection text where this note starts.
    offset: usize,
    /// Length in bytes of the original note text (letter + optional acc).
    length: usize,
    /// Absolute MIDI pitch computed during Lift.
    midi: i32,
}

#[derive(Debug)]
struct OctaveShiftToken {
    offset: usize,
}

/// Per-channel state snapshot at a `/` or `}` that closes a branch inside
/// `{...}`.
#[derive(Debug, Clone, Copy)]
struct BranchEnd {
    channel: usize,
    orig_octave: i32,
}

#[derive(Debug)]
struct Replacement {
    offset: usize,
    length: usize,
    text: String,
}

/// Compute the edit to transpose every note in `selection` by `direction`
/// semitones. Returns `None` when there are no notes to transpose or when
/// the rewritten text matches the original.
pub fn transpose_selection(
    model: &dyn LineReader,
    selection: Selection,
    direction: Direction,
) -> Option<TransposeEdit> {
    // -- Pre-selection context ---------------------------------------------
    let start_line = selection.start_line_number;
    let start_col = selection.start_column;

    // Key sig — initialized from pre-selection context, then updated when
    // `_{...}` blocks are encountered during the forward scan.
    let mut key_sig = scan_key_sig_at(model, start_line, start_col);

    let selector = find_enclosing_track_selector(model, start_line);
    let num_channels = selector.map(|s| s.spans.len()).unwrap_or(1).max(1);

    // Brace/branch/channel/octave state at the selection start, in a
    // single forward walk from the enclosing track selector.
    let start_ctx = scan_brace_state_at(model, start_line, start_col, num_channels, None);

    // -- Extract selection text + full lines -------------------------------
    let mut full_lines: Vec<String> = Vec::new();
    let mut sel_lines: Vec<String> = Vec::new();
    for ln in selection.start_line_number..=selection.end_line_number {
        let full = model.get_line_content(ln).to_string();
        let from = if ln == selection.start_line_number {
            (selection.start_column as usize).saturating_sub(1).min(full.len())
        } else {
            0
        };
        let to = if ln == selection.end_line_number {
            (selection.end_column as usize).saturating_sub(1).min(full.len())
        } else {
            full.len()
        };
        sel_lines.push(full[from..to].to_string());
        full_lines.push(full);
    }
    let sel_text = sel_lines.join("\n");
    let col_offset = (selection.start_column as usize).saturating_sub(1);
    let bytes = sel_text.as_bytes();

    // -- Phase 1: Lift -----------------------------------------------------
    let mut notes: Vec<NoteToken> = Vec::new();
    let mut octave_shifts: Vec<OctaveShiftToken> = Vec::new();
    let mut branch_ends: Vec<(usize, BranchEnd)> = Vec::new();

    let mut state = start_ctx.clone();

    let mut i: usize = 0;
    let mut line_idx: usize = 0;
    let mut col_in_line: usize = 0;

    while i < bytes.len() {
        let ch = bytes[i];

        if ch == b'\n' {
            i += 1;
            line_idx += 1;
            col_in_line = 0;
            continue;
        }

        let abs_col = if line_idx == 0 {
            col_in_line + col_offset
        } else {
            col_in_line
        };
        let full_line = full_lines.get(line_idx).map(String::as_str).unwrap_or("");

        if is_in_comment(full_line, abs_col) {
            let remain = sel_lines[line_idx].len() - col_in_line;
            i += remain;
            col_in_line += remain;
            continue;
        }

        if is_in_key_sig(full_line, abs_col) {
            i += 1;
            col_in_line += 1;
            continue;
        }

        // `_{...}` key-sig block: parse content and update key_sig.
        if ch == b'_' && i + 1 < bytes.len() && bytes[i + 1] == b'{' {
            let open_idx = i + 2;
            if let Some(rel) = bytes[open_idx..].iter().position(|&b| b == b'}') {
                let close_idx = open_idx + rel;
                let content = &sel_text[open_idx..close_idx];
                key_sig = parse_key_sig(content, &key_sig);
                let skip = close_idx + 1 - i;
                i += skip;
                col_in_line += skip;
                continue;
            }
        }

        if ch == b'"' {
            i += 1;
            col_in_line += 1;
            while i < bytes.len() && bytes[i] != b'"' && bytes[i] != b'\n' {
                i += 1;
                col_in_line += 1;
            }
            if i < bytes.len() && bytes[i] == b'"' {
                i += 1;
                col_in_line += 1;
            }
            continue;
        }

        if (ch == b'o' || ch == b'O') && i + 1 < bytes.len() {
            if let Some((oct, len)) = parse_leading_u32(&bytes[i + 1..]) {
                state.on_octave_set(oct as i32);
                let total = 1 + len;
                i += total;
                col_in_line += total;
                continue;
            }
        }

        if ch == b'>' || ch == b'<' {
            let delta: i32 = if ch == b'>' { 1 } else { -1 };
            state.on_octave_shift(delta);
            octave_shifts.push(OctaveShiftToken { offset: i });
            i += 1;
            col_in_line += 1;
            continue;
        }

        if ch == b'{' {
            let prev = if i > 0 { bytes[i - 1] } else { 0 };
            state.on_open_brace(prev);
            i += 1;
            col_in_line += 1;
            continue;
        }

        if ch == b'/' && state.brace_depth() > 0 {
            if let Some((channel, orig_octave)) = state.on_slash() {
                branch_ends.push((
                    i,
                    BranchEnd {
                        channel,
                        orig_octave,
                    },
                ));
            }
            i += 1;
            col_in_line += 1;
            continue;
        }

        if ch == b'}' {
            if state.brace_depth() > 0 {
                if let Some((channel, orig_octave)) = state.on_close_brace() {
                    branch_ends.push((
                        i,
                        BranchEnd {
                            channel,
                            orig_octave,
                        },
                    ));
                }
            }
            i += 1;
            col_in_line += 1;
            continue;
        }

        // Note detection. Guards mirror transpose.ts:374-401 precisely.
        let lower = ch.to_ascii_lowercase();
        let prev_ch = if i > 0 { bytes[i - 1] } else { 0 };
        let preceded_by_non_note =
            is_ascii_letter(prev_ch) && !is_note_letter_byte(prev_ch);
        let mut inside_keyword = false;
        if preceded_by_non_note {
            let prev2 = if i > 1 { bytes[i - 2] } else { 0 };
            if is_ascii_letter(prev2) && !is_note_letter_byte(prev2) {
                inside_keyword = true;
            } else {
                let mut look = i + 1;
                if look < bytes.len() && is_accidental_byte(bytes[look]) {
                    look += 1;
                }
                let after = if look < bytes.len() { bytes[look] } else { 0 };
                if is_ascii_letter(after) && !is_note_letter_byte(after) {
                    inside_keyword = true;
                }
            }
        }

        // `is_note_letter` already gates the eight lowercase letters
        // a..=h; the 'h' alias is normalized to 'b' below so chord.rs's
        // semitone table is sufficient at the call site.
        let is_note_letter = (b'a'..=b'h').contains(&ch);
        if is_note_letter && !inside_keyword {
            let note_start = i;
            let letter = if lower == b'h' { 'b' } else { lower as char };
            i += 1;
            col_in_line += 1;
            let mut explicit_acc: Option<i32> = None;
            if i < bytes.len() {
                match bytes[i] {
                    b'+' => {
                        explicit_acc = Some(1);
                        i += 1;
                        col_in_line += 1;
                    }
                    b'-' => {
                        explicit_acc = Some(-1);
                        i += 1;
                        col_in_line += 1;
                    }
                    b'=' => {
                        explicit_acc = Some(0);
                        i += 1;
                        col_in_line += 1;
                    }
                    _ => {}
                }
            }
            // Guard 3: bare `f` starting the `fm` instrument keyword.
            //
            // The TS source readjusts `i` and `colInLine` here, but since
            // `explicit_acc` is `None` they were never advanced past the
            // letter — the rewind is a no-op and we can just continue.
            if letter == 'f'
                && explicit_acc.is_none()
                && i < bytes.len()
                && bytes[i] == b'm'
                && !bytes
                    .get(i + 1)
                    .copied()
                    .map(|c| c.is_ascii_alphanumeric())
                    .unwrap_or(false)
            {
                continue;
            }
            let effective_acc = explicit_acc.unwrap_or_else(|| key_sig.get_or_zero(letter) as i32);
            let oct = state.current_octave();
            let midi =
                (oct - 1) * 12 + chord_natural_semitones(letter).unwrap() + effective_acc;
            notes.push(NoteToken {
                offset: note_start,
                length: i - note_start,
                midi,
            });
            continue;
        }

        i += 1;
        col_in_line += 1;
    }

    if notes.is_empty() {
        return None;
    }

    // Snapshot Lift's end-of-selection state. Compensation in the Lower
    // phase targets this state so notes after the selection keep their
    // original pitch.
    let lift_end_state = state;

    // -- Phase 2: Transform -------------------------------------------------
    let delta = direction.as_i32();
    for note in notes.iter_mut() {
        note.midi += delta;
    }

    // -- Phase 3: Lower — rebuild selection text ----------------------------
    let mut replacements: Vec<Replacement> = Vec::new();

    // Remove all original `>` / `<` (will be regenerated).
    for shift in &octave_shifts {
        replacements.push(Replacement {
            offset: shift.offset,
            length: 1,
            text: String::new(),
        });
    }

    let mut lower = start_ctx.clone();
    let mut j: usize = 0;

    let shift_set: HashSet<usize> = octave_shifts.iter().map(|s| s.offset).collect();
    let branch_end_by_offset: HashMap<usize, BranchEnd> = branch_ends.into_iter().collect();

    let advance_lower_state = |j: &mut usize,
                               until: usize,
                               lower: &mut BraceState,
                               replacements: &mut Vec<Replacement>| {
        while *j < until {
            let c = bytes[*j];
            if shift_set.contains(j) {
                *j += 1;
                continue;
            }
            if (c == b'o' || c == b'O') && *j + 1 < bytes.len() {
                if let Some((oct, len)) = parse_leading_u32(&bytes[*j + 1..]) {
                    lower.on_octave_set(oct as i32);
                    *j += 1 + len;
                    continue;
                }
            }
            if c == b'{' {
                let prev = if *j > 0 { bytes[*j - 1] } else { 0 };
                lower.on_open_brace(prev);
            } else if c == b'/' && lower.brace_depth() > 0 {
                apply_branch_end_comp(*j, &branch_end_by_offset, lower, replacements);
                lower.on_slash();
            } else if c == b'}' && lower.brace_depth() > 0 {
                apply_branch_end_comp(*j, &branch_end_by_offset, lower, replacements);
                lower.on_close_brace();
            }
            *j += 1;
        }
    };

    let mut last_note_rep: Option<usize> = None;
    for note in &notes {
        advance_lower_state(&mut j, note.offset, &mut lower, &mut replacements);

        let new_semi = note.midi.rem_euclid(12);
        let new_oct = note.midi.div_euclid(12) + 1;
        let (letter, acc_str) = spell_semitone(new_semi, &key_sig, direction);

        let cur_oct = lower.current_octave();
        let mut text = String::new();
        if new_oct > cur_oct {
            for _ in 0..(new_oct - cur_oct) {
                text.push('>');
            }
        } else if new_oct < cur_oct {
            for _ in 0..(cur_oct - new_oct) {
                text.push('<');
            }
        }
        lower.on_octave_set(new_oct);

        text.push(letter);
        text.push_str(acc_str);
        replacements.push(Replacement {
            offset: note.offset,
            length: note.length,
            text,
        });
        last_note_rep = Some(replacements.len() - 1);
        j = note.offset + note.length;
    }

    advance_lower_state(&mut j, bytes.len(), &mut lower, &mut replacements);

    // Compensate for any net octave drift between original and rewritten.
    if let Some(idx) = last_note_rep {
        let shift_count = compute_end_drift(&lift_end_state, &lower);
        if shift_count > 0 {
            for _ in 0..shift_count {
                replacements[idx].text.push('>');
            }
        } else if shift_count < 0 {
            for _ in 0..(-shift_count) {
                replacements[idx].text.push('<');
            }
        }
    }

    replacements.sort_by_key(|r| r.offset);

    let mut result = String::with_capacity(bytes.len());
    let mut pos = 0usize;
    for rep in &replacements {
        result.push_str(&sel_text[pos..rep.offset]);
        result.push_str(&rep.text);
        pos = rep.offset + rep.length;
    }
    result.push_str(&sel_text[pos..]);

    if result == sel_text {
        return None;
    }

    Some(TransposeEdit {
        start_line_number: selection.start_line_number,
        start_column: selection.start_column,
        end_line_number: selection.end_line_number,
        end_column: selection.end_column,
        text: result,
    })
}

/// Per-branch compensation helper. When the Lower phase reaches a `/` or
/// `}` whose original Lift-side branch ended at a different octave, emit
/// `>` / `<` glyphs before the boundary so the next chord's branches
/// resume from the original position.
fn apply_branch_end_comp(
    offset: usize,
    branch_end_by_offset: &HashMap<usize, BranchEnd>,
    lower: &mut BraceState,
    replacements: &mut Vec<Replacement>,
) {
    let be = match branch_end_by_offset.get(&offset) {
        Some(b) => *b,
        None => return,
    };
    if lower.active_channel() != Some(be.channel) {
        return;
    }
    let diff = be.orig_octave - lower.channel_octave()[be.channel];
    if diff == 0 {
        return;
    }
    let text = if diff > 0 {
        ">".repeat(diff as usize)
    } else {
        "<".repeat((-diff) as usize)
    };
    replacements.push(Replacement {
        offset,
        length: 0,
        text,
    });
    // Restore the channel's original octave so subsequent state stays
    // aligned with the Lift snapshot.
    lower.on_octave_set(be.orig_octave);
}

/// End-of-selection compensation amount: positive emits trailing `>`,
/// negative emits trailing `<`. Returns 0 when no compensation is needed
/// (either the channels don't match up or the octaves already agree).
fn compute_end_drift(lift_end: &BraceState, lower: &BraceState) -> i32 {
    match (lift_end.active_channel(), lower.active_channel()) {
        (None, None) => lift_end.shared_octave() - lower.shared_octave(),
        (Some(a), Some(b)) if a == b => {
            lift_end.channel_octave()[a] - lower.channel_octave()[a]
        }
        _ => 0,
    }
}

// (scan_octave_at and scan_brace_context_at retired — see
// `octave_scan::scan_brace_state_at`, which subsumes both with a single
// forward walk from the enclosing track selector to the cursor.)

// ---------------------------------------------------------------------------
// Small byte-classification helpers — avoid pulling in `regex`.
// ---------------------------------------------------------------------------

#[inline]
fn is_ascii_letter(b: u8) -> bool {
    b.is_ascii_alphabetic()
}

#[inline]
fn is_note_letter_byte(b: u8) -> bool {
    matches!(b, b'a'..=b'h' | b'A'..=b'H')
}

#[inline]
fn is_accidental_byte(b: u8) -> bool {
    matches!(b, b'+' | b'-' | b'=')
}

/// Parse a leading run of ASCII digits as a `u32`. Returns
/// `(value, byte_length)` or `None` if the slice doesn't start with a digit.
fn parse_leading_u32(bytes: &[u8]) -> Option<(u32, usize)> {
    let mut end = 0;
    let mut value: u32 = 0;
    while end < bytes.len() && bytes[end].is_ascii_digit() {
        value = value
            .checked_mul(10)?
            .checked_add((bytes[end] - b'0') as u32)?;
        end += 1;
    }
    if end == 0 {
        None
    } else {
        Some((value, end))
    }
}

// ---------------------------------------------------------------------------
// Tests — ported from `web-ctrmml/src/mml/transpose.test.ts`.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::LinesModel;

    /// Apply `transpose_selection` to every occurrence of `[...]` in
    /// `input` in turn, returning the resulting text. The brackets mark
    /// the selection; they are stripped before the edit is applied.
    /// Mirrors the TS test helper `apply`.
    fn apply(input: &str, direction: Direction) -> String {
        let start = input.find('[').expect("selection [ required");
        let end = input.find(']').expect("selection ] required");
        let stripped: String = {
            let mut s = String::with_capacity(input.len() - 2);
            s.push_str(&input[..start]);
            s.push_str(&input[start + 1..end]);
            s.push_str(&input[end + 1..]);
            s
        };
        let model = LinesModel(stripped.split('\n').map(String::from).collect());

        let pre_start = &stripped[..start];
        let pre_end = &stripped[..end - 1];
        let line_of = |s: &str| -> u32 { s.split('\n').count() as u32 };
        let col_of = |s: &str| -> u32 {
            let i = s.rfind('\n');
            (if let Some(p) = i {
                s.len() - p - 1
            } else {
                s.len()
            } as u32)
                + 1
        };
        let sel = Selection {
            start_line_number: line_of(pre_start),
            start_column: col_of(pre_start),
            end_line_number: line_of(pre_end),
            end_column: col_of(pre_end),
        };

        let edit = match transpose_selection(&model, sel, direction) {
            Some(e) => e,
            None => return stripped,
        };

        let lines: Vec<&str> = stripped.split('\n').collect();
        let mut before = String::new();
        for (idx, line) in lines
            .iter()
            .take((edit.start_line_number - 1) as usize)
            .enumerate()
        {
            if idx > 0 {
                before.push('\n');
            }
            before.push_str(line);
        }
        let head_line = lines[(edit.start_line_number - 1) as usize];
        if edit.start_line_number > 1 {
            before.push('\n');
        }
        before.push_str(&head_line[..(edit.start_column - 1) as usize]);

        let tail_line = lines[(edit.end_line_number - 1) as usize];
        let mut after = String::new();
        after.push_str(&tail_line[(edit.end_column - 1) as usize..]);
        for line in lines.iter().skip(edit.end_line_number as usize) {
            after.push('\n');
            after.push_str(line);
        }

        let mut out = before;
        out.push_str(&edit.text);
        out.push_str(&after);
        out
    }

    // ---------- basic semitone shifts ----------------------------------------

    #[test]
    fn c_up_to_c_sharp() {
        assert_eq!(apply("A o4 [c] d", Direction::Up), "A o4 c+ d");
    }

    #[test]
    fn c_down_crosses_octave() {
        assert_eq!(apply("A o4 [c] d", Direction::Down), "A o4 <b> d");
    }

    #[test]
    fn consecutive_notes_all_shift() {
        assert_eq!(
            apply("A o4 [c c c] d", Direction::Up),
            "A o4 c+ c+ c+ d"
        );
    }

    // ---------- octave-cross compensation ------------------------------------

    #[test]
    fn b_up_crosses_octave_compensates() {
        assert_eq!(apply("A o4 [b] d", Direction::Up), "A o4 >c< d");
    }

    #[test]
    fn b_sharp_up_inside_braces() {
        assert_eq!(
            apply("A o4 {[b+]} {c}", Direction::Up),
            "A o4 {>c+<} {c}"
        );
    }

    #[test]
    fn selection_with_embedded_gt_restored_as_suffix() {
        assert_eq!(
            apply("A o4 {[b>]} {c}", Direction::Down),
            "A o4 {b->} {c}"
        );
    }

    #[test]
    fn per_branch_compensation_in_multi_channel_chord() {
        assert_eq!(
            apply(
                "ABC o4 l8 {e/g/b} [{g/b/>d<}] {f/a/>c}",
                Direction::Up
            ),
            "ABC o4 l8 {e/g/b} {g+/>c</>d+<} {f/a/>c}"
        );
    }

    #[test]
    fn non_crossing_shift_no_compensation() {
        assert_eq!(
            apply("A o4 [c d e] f", Direction::Up),
            "A o4 c+ d+ f f"
        );
    }

    // ---------- keyword guards -----------------------------------------------

    #[test]
    fn r_after_note_does_not_block_preceding() {
        assert_eq!(apply("A o4 [er] c", Direction::Up), "A o4 fr c");
    }

    #[test]
    fn l_after_note_does_not_block_preceding() {
        assert_eq!(apply("A o4 [el4] c", Direction::Up), "A o4 fl4 c");
    }

    #[test]
    fn note_after_r_is_transposed() {
        assert_eq!(apply("A o4 [rc] d", Direction::Up), "A o4 rc+ d");
    }

    #[test]
    fn three_letters_independently() {
        assert_eq!(apply("A o4 [erf] c", Direction::Up), "A o4 frf+ c");
    }

    #[test]
    fn fm_keyword_f_is_not_a_note() {
        assert_eq!(apply("[@1 fm]\nA c", Direction::Up), "@1 fm\nA c");
    }

    #[test]
    fn psg_keyword_g_is_not_a_note() {
        assert_eq!(apply("A o4 [psg] d", Direction::Up), "A o4 psg d");
    }

    #[test]
    fn pcm_keyword_c_is_not_a_note() {
        assert_eq!(apply("A o4 [pcm] d", Direction::Up), "A o4 pcm d");
    }

    // ---------- return value -------------------------------------------------

    #[test]
    fn returns_none_when_no_notes_in_selection() {
        let model = LinesModel(vec!["A o4 r4 d".into()]);
        let edit = transpose_selection(
            &model,
            Selection {
                start_line_number: 1,
                start_column: 6,
                end_line_number: 1,
                end_column: 9,
            },
            Direction::Up,
        );
        assert!(edit.is_none());
    }

    // ---------- selection starting inside a multi-channel chord --------------

    #[test]
    fn three_channel_mid_chord_selection_up() {
        assert_eq!(
            apply("ABC o6 {c/e/a} {e/a/[>c<} {e/g/b} ]", Direction::Up),
            "ABC o6 {c/e/a} {e/a/>c+<} {f/g+/>c<} "
        );
    }

    #[test]
    fn three_channel_mid_chord_selection_down() {
        assert_eq!(
            apply("ABC o6 {c/e/a} {e/a/[>c<} {e/g/b} ]", Direction::Down),
            "ABC o6 {c/e/a} {e/a/b} {e-/g-/b-} "
        );
    }

    #[test]
    fn selection_covers_only_tail_branch() {
        assert_eq!(
            apply("ABC o6 {c/e/a} {e/a/[>c<]}", Direction::Up),
            "ABC o6 {c/e/a} {e/a/>c+<}"
        );
    }

    #[test]
    fn selection_starts_in_middle_branch() {
        assert_eq!(
            apply("ABC o6 {c/e/a} {e/[a/>c<]}", Direction::Up),
            "ABC o6 {c/e/a} {e/a+/>c+<}"
        );
    }

    #[test]
    fn selection_begins_at_closing_brace() {
        assert_eq!(
            apply(
                "ABC o6 {c/e/a} {e/a/>c<[} {e/g/b}]",
                Direction::Up
            ),
            "ABC o6 {c/e/a} {e/a/>c<} {f/g+/>c<}"
        );
    }

    #[test]
    fn two_channel_mid_chord_selection() {
        assert_eq!(
            apply("AB o6 {c/e} {e/[>g<} {c/e}]", Direction::Up),
            "AB o6 {c/e} {e/>g+<} {c+/f}"
        );
    }

    #[test]
    fn single_channel_chord_notation() {
        assert_eq!(
            apply("A o6 {c e [>g<} {c e}]", Direction::Up),
            "A o6 {c e >g+} {<c+ f}"
        );
    }

    // ---------- selection ending inside a multi-channel chord ----------------

    #[test]
    fn selection_ends_mid_chord_before_brace() {
        assert_eq!(
            apply("ABC o6 {c/e/a} [{e/g]/b}", Direction::Up),
            "ABC o6 {c/e/a} {f/g+/b}"
        );
    }

    #[test]
    fn selection_ends_mid_tail_branch_after_gt() {
        assert_eq!(
            apply("ABC o6 {c/e/a} {e/a/[>c]<}", Direction::Up),
            "ABC o6 {c/e/a} {e/a/>c+<}"
        );
    }

    // ---------- octave context across multi-channel boundaries ---------------

    #[test]
    fn explicit_octave_before_brace_propagates() {
        assert_eq!(
            apply("ABC o3 [{c/e/g}]", Direction::Up),
            "ABC o3 {c+/f/g+}"
        );
    }

    #[test]
    fn gt_between_chords_lifts_shared_only() {
        assert_eq!(
            apply("ABC o4 {c/e/g} > [{c/e/g}]", Direction::Up),
            "ABC o4 {c/e/g} > {c+/f/g+}"
        );
    }

    #[test]
    fn balanced_shift_in_earlier_branch_does_not_leak() {
        assert_eq!(
            apply("ABC o4 {c/e/>g<} [{c/e/g}]", Direction::Up),
            "ABC o4 {c/e/>g<} {c+/f/g+}"
        );
    }

    // ---------- regression: mid-chord selection with leading `<` -------------

    #[test]
    fn selecting_lt_c_gt_inside_branch() {
        assert_eq!(
            apply("ABC o6 {c/e/a} {e/a/[<c>]}", Direction::Up),
            "ABC o6 {c/e/a} {e/a/<c+>}"
        );
    }
}
