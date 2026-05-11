//! Note hover — Phase 3.2.2.
//!
//! When the cursor is on a note letter (`a`..`h`) within a track,
//! show a tooltip with the note's effective spelling, absolute MIDI
//! pitch, and the ambient key-signature context. The semantics are
//! intentionally identical to what transpose's Lift phase computes
//! for each note token — same accidental + key-sig + channel-octave
//! resolution chain.

use ctrmml_lang_core::{
    chord::chord_natural_semitones, find_enclosing_track_selector, find_fm_block_at,
    find_psg_block_at, scan_brace_state_at, scan_key_sig_at, LinesModel,
};

const NOTE_NAMES_SHARP: [&str; 12] = [
    "C", "C♯", "D", "D♯", "E", "F", "F♯", "G", "G♯", "A", "A♯", "B",
];

/// Find the note-letter token at 0-based `col` on `line`, returning
/// `(letter, explicit_accidental, start_col, end_col)`.
///
/// `explicit_accidental` is `1` for `+`, `-1` for `-`, `0` for `=`,
/// or `None` when the user typed no explicit accidental.
fn note_at(line: &str, col: usize) -> Option<(char, Option<i32>, usize, usize)> {
    let bytes = line.as_bytes();
    if col > bytes.len() {
        return None;
    }
    // Find the start of the note token. If the cursor sits on the
    // accidental, walk one char back to reach the letter.
    let start = if col < bytes.len() && matches!(bytes[col], b'+' | b'-' | b'=') {
        col.saturating_sub(1)
    } else {
        col
    };
    if start >= bytes.len() {
        return None;
    }
    let ch = bytes[start];
    if !matches!(ch, b'a'..=b'h') {
        return None;
    }
    // Reject when the preceding letter is another non-note letter
    // (keyword like `psg`, `pcm`, `rest`). Mirrors transpose's
    // note-detection guard.
    if start > 0 {
        let prev = bytes[start - 1];
        if prev.is_ascii_alphabetic() && !matches!(prev, b'a'..=b'h' | b'A'..=b'H') {
            return None;
        }
    }
    let letter = ch as char;
    let mut end = start + 1;
    let mut explicit_acc: Option<i32> = None;
    if end < bytes.len() {
        match bytes[end] {
            b'+' => {
                explicit_acc = Some(1);
                end += 1;
            }
            b'-' => {
                explicit_acc = Some(-1);
                end += 1;
            }
            b'=' => {
                explicit_acc = Some(0);
                end += 1;
            }
            _ => {}
        }
    }
    Some((letter, explicit_acc, start, end))
}

/// Build a markdown hover string for a note at `(line, character)`,
/// or `None` when the cursor isn't on a note in a track context.
pub(crate) fn note_hover_text(
    doc_text: &str,
    line_zero_based: u32,
    character: u32,
    line_content: &str,
) -> Option<(String, usize, usize)> {
    let (letter, explicit_acc, start, end) =
        note_at(line_content, character as usize)?;

    let line = line_zero_based + 1;
    let col = character + 1;
    let model = LinesModel::from_text(doc_text);

    // Suppress inside FM/PSG instrument blocks — bytes like `b` in
    // `1,0,12,7,0,28,b,0,5,0` aren't notes.
    if find_fm_block_at(&model, line).is_some() {
        return None;
    }
    if find_psg_block_at(&model, line).is_some() {
        return None;
    }
    let selector = find_enclosing_track_selector(&model, line)?;
    let num_channels = selector.spans.len().max(1);

    let state = scan_brace_state_at(&model, line, col, num_channels, None);
    let key_sig = scan_key_sig_at(&model, line, col);

    // Normalise `h` → `b` for semitone lookup; key sig and pitch are
    // tracked under the `b` slot in both representations.
    let normalised = if letter == 'h' { 'b' } else { letter };
    let natural_sem = chord_natural_semitones(normalised)?;

    let effective_acc = explicit_acc.unwrap_or_else(|| key_sig.get_or_zero(normalised) as i32);
    let octave = state.current_octave();
    let midi = (octave - 1) * 12 + natural_sem + effective_acc;

    // Spell the pitch class for the tooltip header. Octave follows
    // ctrmml's `oN` convention (the user-facing track octave) rather
    // than the MIDI division, so `o5 c` reads as "C5" even though
    // its MIDI value (48) sits in MIDI-octave 4.
    let pitch_class = midi.rem_euclid(12) as usize;
    let display_octave = octave;
    let spelled = NOTE_NAMES_SHARP[pitch_class];

    // Markdown table: pitch summary on top, then context details.
    let acc_glyph = match effective_acc {
        2 => " (𝄪)",
        1 => " (♯)",
        0 => "",
        -1 => " (♭)",
        -2 => " (𝄫)",
        _ => " (?)",
    };
    let mut md = String::new();
    md.push_str(&format!(
        "**{spelled}{display_octave}**{acc_glyph}  ·  MIDI **{midi}**\n\n"
    ));
    md.push_str(&format!(
        "- Letter: `{letter}`{}\n",
        match explicit_acc {
            Some(1) => "`+`",
            Some(-1) => "`-`",
            Some(0) => "`=`",
            _ => "",
        }
    ));
    md.push_str(&format!("- Channel octave: `o{octave}`"));
    if let Some(c) = state.active_channel() {
        md.push_str(&format!(" (branch {c} inside `{{...}}`)"));
    }
    md.push('\n');
    let key_acc = key_sig.get_or_zero(normalised);
    if key_acc != 0 {
        md.push_str(&format!(
            "- Key signature on `{normalised}`: `{}`\n",
            if key_acc > 0 { "+" } else { "-" }
        ));
    } else {
        md.push_str(&format!("- Key signature on `{normalised}`: natural\n"));
    }
    Some((md, start, end))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn note_at_basic_letter() {
        let (letter, acc, start, end) = note_at("o4 cdefg", 3).unwrap();
        assert_eq!(letter, 'c');
        assert_eq!(acc, None);
        assert_eq!((start, end), (3, 4));
    }

    #[test]
    fn note_at_with_sharp() {
        let (letter, acc, start, end) = note_at("o4 c+def", 3).unwrap();
        assert_eq!(letter, 'c');
        assert_eq!(acc, Some(1));
        assert_eq!((start, end), (3, 5));
    }

    #[test]
    fn note_at_cursor_on_accidental_walks_back() {
        // Cursor on the `+` — should still resolve the c note.
        let (letter, acc, _, _) = note_at("o4 c+def", 4).unwrap();
        assert_eq!(letter, 'c');
        assert_eq!(acc, Some(1));
    }

    #[test]
    fn note_at_rejects_inside_keyword() {
        // `psg` — the `s` at col 4 has a non-note letter before it.
        assert!(note_at("@1 psg 15", 4).is_none());
    }

    #[test]
    fn note_at_returns_none_when_not_a_note() {
        assert!(note_at("o4 cdefg", 0).is_none()); // 'o'
        assert!(note_at("@1 fm", 0).is_none()); // '@'
    }

    #[test]
    fn hover_text_on_a_track_note() {
        let doc = "A o5 c\n";
        // cursor at col 5 = the 'c'
        let (md, _, _) = note_hover_text(doc, 0, 5, "A o5 c").unwrap();
        // Expect octave 5, midi = (5-1)*12 + 0 = 48.
        assert!(md.contains("MIDI **48**"), "missing midi in {md:?}");
        assert!(md.contains("C5"), "missing C5 in {md:?}");
    }

    #[test]
    fn hover_text_key_sig_applies_when_no_explicit_accidental() {
        // F major flats b. A bare 'b' at the cursor should resolve to B♭.
        let doc = "A _{F} o4 b\n";
        let (md, _, _) = note_hover_text(doc, 0, 10, "A _{F} o4 b").unwrap();
        // (4-1)*12 + 11 + (-1) = 36 + 10 = 46.
        assert!(md.contains("MIDI **46**"), "missing midi 46 in {md:?}");
    }

    #[test]
    fn hover_text_none_outside_track() {
        // Header line — no enclosing track selector.
        let doc = "#title \"x\"\n";
        assert!(note_hover_text(doc, 0, 0, "#title \"x\"").is_none());
    }

    #[test]
    fn hover_text_none_inside_fm_block() {
        let doc = "A cdefg\n@1 fm\n\t31,0,12,7,0,28,0,0,5,0\n";
        // cursor on what looks like a 'b' inside FM data, but it's not
        // a note. Use position 2,0 which is inside the tab-indented row.
        assert!(note_hover_text(doc, 2, 1, "\t31,0,12,7,0,28,0,0,5,0").is_none());
    }
}
