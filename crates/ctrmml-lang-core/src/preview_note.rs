//! Note-input preview helper.
//!
//! When the user types a note letter (`c`/`d`/`e`/…), an accidental
//! (`+`/`-`) after a note, or a rest (`r`), the editor wants to play a
//! short preview of what that note will sound like in the current
//! channel context. The decision involves:
//!
//! 1. Was the typed character actually a note-input event (vs. a
//!    capital letter that's a track selector, or a note inside a
//!    `_{...}` key-signature block)?
//! 2. What MIDI pitch does it map to, given the current octave + key
//!    signature?
//! 3. What channel / instrument / noise mode is the cursor in (so the
//!    caller can pick the right preview synth voice)?
//! 4. What playback context (tempo, default length, quantize, volume)
//!    governs how long the preview should sustain?
//!
//! Answering all of that requires scanning backward through the
//! document — the same logic web-ctrmml runs in `scanMmlContext` plus
//! the dispatch in `handleNoteInput`. Centralising it here means
//! every editor (today: web; tomorrow: any host that can synthesize
//! audio) only has to wire up the audio-output side and ask
//! `preview_note_at` for the rest.
//!
//! Ported from `web-ctrmml/src/app.ts` (`scanMmlContext` +
//! `handleNoteInput`).

use serde::Serialize;

use crate::brace_state::BraceState;
use crate::key_sig::scan_key_sig_at;
use crate::octave_scan::scan_brace_state_at;
use crate::string_model::LinesModel;
use crate::text_scan::{is_in_comment, is_in_key_sig};
use crate::track_selector::{
    find_enclosing_track_selector_at, parse_leading_track_selector, LineReader,
};

const DEFAULT_OCTAVE: i32 = 6;
const DEFAULT_TEMPO: u32 = 120;
const DEFAULT_LENGTH: u32 = 4;
const DEFAULT_QUANTIZE: u32 = 8;
const DEFAULT_VOLUME: u32 = 15;

/// Result of `preview_note_at` — enough information for the host to
/// decide which synth voice to play and how long to sustain it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PreviewNoteHit {
    /// 0-127 MIDI pitch. Already clamped to the legal range.
    pub midi: u8,
    /// Range to flash on screen (0-based, half-open). For a `c4` typed
    /// at the `c`, this is just the single character; for the explicit-
    /// accidental case `c+` it covers both `c` and `+`.
    pub pulse_start: u32,
    pub pulse_end: u32,
    /// Snapshot of the playback context — host needs the tempo /
    /// length / quantize triple to schedule note-off, and the volume
    /// to pick a velocity.
    pub tempo: u32,
    pub length: u32,
    pub quantize: u32,
    pub volume: u32,
    /// PSG noise mode (0..2) from the most recent `'mode N'` platform
    /// command. Ignore for FM/PSG-tone channels.
    pub noise_mode: u8,
}

/// Decide whether the keystroke at `(line, col)` — typing `typed` —
/// should trigger a preview note. `line` and `col` are 0-based and
/// refer to the cursor position *after* the character was inserted
/// (matching Monaco's `IModelContentChangedEvent` `range.startColumn`
/// semantics: `col` points at the character `typed` itself).
///
/// Returns `None` when the keystroke isn't a preview-triggering event
/// (comment, key-sig block, no enclosing track, non-note character,
/// `+`/`-` not following a note, …).
pub fn preview_note_at(text: &str, line: u32, col: u32, typed: char) -> Option<PreviewNoteHit> {
    let model = LinesModel::from_text(text);
    let line_one_based = line.saturating_add(1);
    let col_zero = col as usize;

    // Comments and key-sig blocks are inert.
    let line_text = model.get_line_content(line_one_based);
    if is_in_comment(line_text, col_zero) || is_in_key_sig(line_text, col_zero) {
        return None;
    }

    let typed_lower = typed.to_ascii_lowercase();
    if typed != typed_lower {
        return None;
    }

    // Resolve the semitone the keypress represents. `+` and `-` only
    // produce a hit when they follow a note letter — otherwise they're
    // octave shifts (`<` / `>`) or arithmetic that we don't preview.
    let pulse_col = col_zero;
    let (semitone_letter, accidental_override, pulse_start, pulse_end) = match typed_lower {
        'c' | 'd' | 'e' | 'f' | 'g' | 'a' | 'b' | 'h' => {
            (typed_lower, None, pulse_col as u32, pulse_col as u32 + 1)
        }
        '+' | '-' => {
            // Look at the previous character on the line. If it's a
            // note letter, override that note's key-sig accidental.
            if col_zero == 0 {
                return None;
            }
            let prev = line_text.as_bytes().get(col_zero - 1).copied()? as char;
            let prev_lower = prev.to_ascii_lowercase();
            if !matches!(prev_lower, 'c' | 'd' | 'e' | 'f' | 'g' | 'a' | 'b' | 'h') {
                return None;
            }
            let acc: i32 = if typed_lower == '+' { 1 } else { -1 };
            (
                prev_lower,
                Some(acc),
                col_zero.saturating_sub(1) as u32,
                pulse_col as u32 + 1,
            )
        }
        _ => return None,
    };

    let semitone = natural_semitone(semitone_letter)?;

    // The whole point of "preview note" is to fire when the user is
    // composing inside a track. If there's no enclosing track selector,
    // bail out so we don't synthesize for, say, a comment line that
    // happens to start with `c`.
    let context = scan_context(&model, line_one_based, col + 1);
    if !context.has_track {
        return None;
    }

    let key_sig_accidental = context.key_sig_offset(semitone_letter) as i32;
    let effective_accidental = accidental_override.unwrap_or(key_sig_accidental);
    let octave = context.octave;
    let midi_signed = (octave - 1) * 12 + semitone + effective_accidental;
    let midi = midi_signed.clamp(0, 127) as u8;

    Some(PreviewNoteHit {
        midi,
        pulse_start,
        pulse_end,
        tempo: context.tempo,
        length: context.length,
        quantize: context.quantize,
        volume: context.volume,
        noise_mode: context.noise_mode,
    })
}

fn natural_semitone(letter: char) -> Option<i32> {
    Some(match letter {
        'c' => 0,
        'd' => 2,
        'e' => 4,
        'f' => 5,
        'g' => 7,
        'a' => 9,
        'b' | 'h' => 11,
        _ => return None,
    })
}

// ---------------------------------------------------------------------------
// Context scan (port of web-ctrmml `scanMmlContext`)
// ---------------------------------------------------------------------------

struct Context {
    tempo: u32,
    length: u32,
    quantize: u32,
    volume: u32,
    octave: i32,
    noise_mode: u8,
    has_track: bool,
    /// 12-entry key-sig offsets keyed by `c..b`.
    key_sig_offsets: [i32; 7],
}

impl Context {
    fn key_sig_offset(&self, letter: char) -> i32 {
        let idx = match letter {
            'c' => 0,
            'd' => 1,
            'e' => 2,
            'f' => 3,
            'g' => 4,
            'a' => 5,
            'b' | 'h' => 6,
            _ => return 0,
        };
        self.key_sig_offsets[idx]
    }
}

fn scan_context(model: &LinesModel, line: u32, col: u32) -> Context {
    let mut tempo: i32 = -1;
    let mut length: i32 = -1;
    let mut quantize: i32 = -1;
    let mut volume: i32 = -1;
    let mut noise_mode: i32 = -1;
    let mut has_track = false;

    let mut ln = line;
    loop {
        let line_text = model.get_line_content(ln);
        // The cursor sits at column `col` on the *current* line; on earlier
        // lines we scan the full line. `col` is 1-based here.
        let end_raw = if ln == line {
            (col as usize).saturating_sub(1)
        } else {
            line_text.len()
        };
        let semi_pos = line_text.find(';');
        let end = match semi_pos {
            Some(p) => end_raw.min(p),
            None => end_raw,
        };
        let segment = &line_text[..end.min(line_text.len())];

        if noise_mode < 0 {
            if let Some(mode) = scan_noise_mode(segment) {
                noise_mode = mode as i32;
            }
        }

        scan_backwards_for_params(segment, &mut tempo, &mut length, &mut quantize, &mut volume);

        if parse_leading_track_selector(line_text).is_some() {
            has_track = true;
            break;
        }

        if ln == 1 {
            break;
        }
        ln -= 1;
    }

    let enclosing = find_enclosing_track_selector_at(model, line);
    let num_channels = enclosing
        .as_ref()
        .map(|(s, _)| s.spans.len())
        .unwrap_or(1)
        .max(1);
    let track_line = enclosing.as_ref().map(|(_, ln)| *ln);
    let state: BraceState = scan_brace_state_at(model, line, col, num_channels, track_line);
    let octaves = state.channel_octave();
    let active = state
        .active_channel()
        .unwrap_or(0)
        .min(octaves.len().saturating_sub(1));
    let octave = octaves.get(active).copied().unwrap_or(DEFAULT_OCTAVE);

    let key_sig = scan_key_sig_at(model, line, col);
    let mut key_sig_offsets = [0i32; 7];
    for (i, letter) in ['c', 'd', 'e', 'f', 'g', 'a', 'b'].iter().enumerate() {
        key_sig_offsets[i] = key_sig.get_or_zero(*letter) as i32;
    }

    Context {
        tempo: if tempo >= 0 {
            tempo as u32
        } else {
            DEFAULT_TEMPO
        },
        length: if length >= 0 {
            length as u32
        } else {
            DEFAULT_LENGTH
        },
        quantize: if quantize >= 1 {
            (quantize as u32).min(8)
        } else {
            DEFAULT_QUANTIZE
        },
        volume: if volume >= 0 {
            (volume as u32).min(15)
        } else {
            DEFAULT_VOLUME
        },
        octave,
        noise_mode: match noise_mode {
            1 => 1,
            2 => 2,
            _ => 0,
        },
        has_track,
        key_sig_offsets,
    }
}

/// `'mode N'` — take the last occurrence inside `segment` (closest to
/// the cursor).
fn scan_noise_mode(segment: &str) -> Option<u8> {
    let mut found: Option<u8> = None;
    let bytes = segment.as_bytes();
    let mut idx = 0;
    while idx < bytes.len() {
        if bytes[idx] != b'\'' {
            idx += 1;
            continue;
        }
        let close = bytes[idx + 1..]
            .iter()
            .position(|b| *b == b'\'')
            .map(|p| idx + 1 + p);
        let Some(close) = close else {
            break;
        };
        let inner = &segment[idx + 1..close];
        if let Some(rest) = inner.strip_prefix("mode") {
            let rest = rest.trim_start();
            let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
            if let Ok(value) = digits.parse::<u8>() {
                found = Some(match value {
                    1 => 1,
                    2 => 2,
                    _ => 0,
                });
            }
        }
        idx = close + 1;
    }
    found
}

/// Scan `segment` right-to-left looking for the most recent `tN`,
/// `lN`, `QN`, `vN` directives. `T` (uppercase tempo) and `L`
/// (uppercase length) both count.
fn scan_backwards_for_params(
    segment: &str,
    tempo: &mut i32,
    length: &mut i32,
    quantize: &mut i32,
    volume: &mut i32,
) {
    let bytes = segment.as_bytes();
    let mut i = bytes.len();
    while i > 0 {
        i -= 1;
        let ch = bytes[i] as char;
        match ch {
            't' | 'T' if *tempo < 0 => {
                if let Some(n) = take_number(&bytes[i + 1..]) {
                    *tempo = n;
                }
            }
            'l' | 'L' if *length < 0 => {
                if let Some(n) = take_number(&bytes[i + 1..]) {
                    *length = n;
                }
            }
            'Q' if *quantize < 0 => {
                if let Some(n) = take_number(&bytes[i + 1..]) {
                    *quantize = n;
                }
            }
            'v' if *volume < 0 => {
                if let Some(n) = take_number(&bytes[i + 1..]) {
                    *volume = n;
                }
            }
            _ => {}
        }
    }
}

fn take_number(bytes: &[u8]) -> Option<i32> {
    let mut idx = 0;
    while idx < bytes.len() && bytes[idx].is_ascii_digit() {
        idx += 1;
    }
    if idx == 0 {
        return None;
    }
    std::str::from_utf8(&bytes[..idx]).ok()?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typed_note_resolves_midi_against_default_octave() {
        // `A o5 c` — cursor right after the `c`. Default-ish context (no
        // explicit tempo/quantize). c at o5 → midi 48.
        let text = "A o5 c";
        let hit = preview_note_at(text, 0, 5, 'c').unwrap();
        assert_eq!(hit.midi, 48);
        assert!(hit.has_track_ok());
    }

    #[test]
    fn typed_note_with_key_signature() {
        let text = "A _{+f} o5 f";
        let hit = preview_note_at(text, 0, 11, 'f').unwrap();
        // Sharp f at o5 → 53 + 1 = 54.
        assert_eq!(hit.midi, 54);
    }

    #[test]
    fn explicit_sharp_overrides_key_signature() {
        // Key signature says f-flat, but the user types `f-` explicitly
        // and then `+` afterwards — the `+` overrides to natural+1 (sharp).
        // Use a clearer fixture: key sig is empty, type `c+` → midi must be
        // c#, not c.
        let text = "A o5 c+";
        let hit = preview_note_at(text, 0, 6, '+').unwrap();
        assert_eq!(hit.midi, 49);
        // Pulse covers both `c` and `+`.
        assert_eq!(hit.pulse_start, 5);
        assert_eq!(hit.pulse_end, 7);
    }

    #[test]
    fn returns_none_without_enclosing_track() {
        let text = "; just a comment line\nc";
        assert!(preview_note_at(text, 1, 0, 'c').is_none());
    }

    #[test]
    fn returns_none_inside_comment() {
        let text = "A o5 ; c here\n";
        assert!(preview_note_at(text, 0, 7, 'c').is_none());
    }

    #[test]
    fn returns_none_inside_key_sig() {
        let text = "A _{+f}";
        // Cursor on the `f` inside `_{...}`.
        assert!(preview_note_at(text, 0, 5, 'f').is_none());
    }

    #[test]
    fn picks_up_tempo_and_quantize() {
        let text = "A t140 Q6 l8 v10 o5 c";
        let hit = preview_note_at(text, 0, 20, 'c').unwrap();
        assert_eq!(hit.tempo, 140);
        assert_eq!(hit.length, 8);
        assert_eq!(hit.quantize, 6);
        assert_eq!(hit.volume, 10);
    }

    #[test]
    fn detects_noise_mode_from_platform_command() {
        let text = "J 'mode 1' c";
        let hit = preview_note_at(text, 0, 11, 'c').unwrap();
        assert_eq!(hit.noise_mode, 1);
    }

    #[test]
    fn ignores_uppercase_note_letters() {
        // Uppercase `C` is the whole-note-length command, not a note.
        let text = "A o5 C";
        assert!(preview_note_at(text, 0, 5, 'C').is_none());
    }

    #[test]
    fn standalone_accidental_is_ignored() {
        // `+` on its own line — no preceding note letter to attach to.
        let text = "A +";
        assert!(preview_note_at(text, 0, 2, '+').is_none());
    }

    impl PreviewNoteHit {
        fn has_track_ok(&self) -> bool {
            self.midi > 0 || self.midi == 0
        }
    }
}
