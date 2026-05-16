//! Chord rendering — ported from `web-ctrmml/src/mml/chord.ts`.
//!
//! The output strings are ctrmml branch expressions (e.g. `"c/e-/g"`) and
//! must match the TypeScript implementation byte-for-byte; tests below
//! enforce that.

use crate::key_sig::KeySig;

/// Explicit accidental typed after the root letter.
///
/// Mirrors the TS `RootAccidental = 1 | 0 | -1 | null`. We use an enum to
/// preserve the three-way distinction between "explicitly natural" (`=`),
/// "no explicit accidental" (`None`), and the sharp/flat cases.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum RootAccidental {
    Sharp,
    Natural,
    Flat,
}

impl RootAccidental {
    #[inline]
    fn as_i32(self) -> i32 {
        match self {
            RootAccidental::Sharp => 1,
            RootAccidental::Natural => 0,
            RootAccidental::Flat => -1,
        }
    }
}

/// Render the typed-accidental glyph for a root-position accidental.
///
/// Matches the TS `accidentalChar`:
///   `+1 → "+"`, `-1 → "-"`, `0 → "="`, `None → ""`.
pub fn accidental_char(acc: Option<RootAccidental>) -> &'static str {
    match acc {
        Some(RootAccidental::Sharp) => "+",
        Some(RootAccidental::Flat) => "-",
        Some(RootAccidental::Natural) => "=",
        None => "",
    }
}

/// Triad vs seventh — the size discriminant for the generic-chord helpers.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum ChordSize {
    Triad,
    Seventh,
}

impl ChordSize {
    #[inline]
    fn steps(self) -> &'static [i32] {
        match self {
            ChordSize::Triad => &[0, 2, 4],
            ChordSize::Seventh => &[0, 2, 4, 6],
        }
    }
}

/// One named chord definition (suffix, letter-step layout, semitone interval
/// layout, description).
#[derive(Debug, Clone)]
pub struct ChordDef {
    /// Label suffix shown after the root letter (e.g. `"m"`, `"M7"`, `""`).
    pub suffix: &'static str,
    /// Letter-step offset from root for each chord tone (0, 2, 4 = triad).
    pub letter_steps: &'static [i32],
    /// Semitone interval from root for each chord tone.
    pub intervals: &'static [i32],
    /// Short description shown in the completion detail.
    pub detail: &'static str,
}

pub const CHORDS_3: &[ChordDef] = &[
    ChordDef { suffix: "",     letter_steps: &[0, 2, 4], intervals: &[0, 4, 7],  detail: "Major triad" },
    ChordDef { suffix: "m",    letter_steps: &[0, 2, 4], intervals: &[0, 3, 7],  detail: "Minor triad" },
    ChordDef { suffix: "dim",  letter_steps: &[0, 2, 4], intervals: &[0, 3, 6],  detail: "Diminished triad" },
    ChordDef { suffix: "aug",  letter_steps: &[0, 2, 4], intervals: &[0, 4, 8],  detail: "Augmented triad" },
    ChordDef { suffix: "sus2", letter_steps: &[0, 1, 4], intervals: &[0, 2, 7],  detail: "Suspended 2nd" },
    ChordDef { suffix: "sus4", letter_steps: &[0, 3, 4], intervals: &[0, 5, 7],  detail: "Suspended 4th" },
    ChordDef { suffix: "4th",  letter_steps: &[0, 3, 6], intervals: &[0, 5, 10], detail: "Quartal triad" },
];

pub const CHORDS_4: &[ChordDef] = &[
    ChordDef { suffix: "M7",   letter_steps: &[0, 2, 4, 6], intervals: &[0, 4, 7, 11], detail: "Major 7th" },
    ChordDef { suffix: "7",    letter_steps: &[0, 2, 4, 6], intervals: &[0, 4, 7, 10], detail: "Dominant 7th" },
    ChordDef { suffix: "m7",   letter_steps: &[0, 2, 4, 6], intervals: &[0, 3, 7, 10], detail: "Minor 7th" },
    ChordDef { suffix: "mM7",  letter_steps: &[0, 2, 4, 6], intervals: &[0, 3, 7, 11], detail: "Minor-major 7th" },
    ChordDef { suffix: "m7b5", letter_steps: &[0, 2, 4, 6], intervals: &[0, 3, 6, 10], detail: "Half-diminished 7th" },
    // dim7's 4th tone is enharmonically `bbb`; ctrmml can't spell that, so use
    // the a-letter (dim6 equivalent) and rely on the natural a matching semitone 9.
    ChordDef { suffix: "dim7", letter_steps: &[0, 2, 4, 5], intervals: &[0, 3, 6, 9],  detail: "Diminished 7th" },
    ChordDef { suffix: "6",    letter_steps: &[0, 2, 4, 5], intervals: &[0, 4, 7, 9],  detail: "Major 6th" },
    ChordDef { suffix: "add9", letter_steps: &[0, 2, 4, 1], intervals: &[0, 4, 7, 2],  detail: "Major add9" },
    ChordDef { suffix: "4th",  letter_steps: &[0, 3, 6, 2], intervals: &[0, 5, 10, 3], detail: "Quartal 7th" },
];

/// The 7 natural letters, in pitch order from `c`.
pub const CHORD_LETTERS: [char; 7] = ['c', 'd', 'e', 'f', 'g', 'a', 'b'];

/// Semitones above `c` for each natural letter.
#[inline]
pub fn chord_natural_semitones(letter: char) -> Option<i32> {
    match letter {
        'c' => Some(0),
        'd' => Some(2),
        'e' => Some(4),
        'f' => Some(5),
        'g' => Some(7),
        'a' => Some(9),
        'b' => Some(11),
        _ => None,
    }
}

/// Index of `letter` within [`CHORD_LETTERS`], with `'h'` aliased to `'b'`
/// (German-style notation).
fn root_index(root_letter_lower: char) -> Option<usize> {
    let effective = if root_letter_lower == 'h' { 'b' } else { root_letter_lower };
    CHORD_LETTERS.iter().position(|&c| c == effective)
}

/// Resolve root index + the root pitch in semitones after applying the
/// user-typed accidental or the ambient key signature.
///
/// Returns `(root_idx, root_adjusted_sem)`.
fn root_state(
    root_letter_lower: char,
    root_accidental: Option<RootAccidental>,
    key_sig: &KeySig,
) -> Option<(usize, i32)> {
    let root_idx = root_index(root_letter_lower)?;
    let effective_root = CHORD_LETTERS[root_idx];
    let root_sem = chord_natural_semitones(effective_root)?;
    let root_acc = match root_accidental {
        Some(a) => a.as_i32(),
        None => key_sig.get_or_zero(effective_root) as i32,
    };
    Some((root_idx, root_sem + root_acc))
}

/// Render the accidental glyph **for a non-root tone**, given its accidental
/// offset and the ambient key signature.
///
/// Returns `""` when the tone's accidental matches the key signature
/// (so no override is needed), otherwise `"="`, `"+"`, or `"-"`.
fn non_root_accidental_glyph(acc: i32, letter: char, key_sig: &KeySig) -> &'static str {
    if acc == key_sig.get_or_zero(letter) as i32 {
        ""
    } else if acc == 0 {
        "="
    } else if acc > 0 {
        "+"
    } else {
        "-"
    }
}

/// Generic diatonic chord: stack letters by thirds (every other letter),
/// no key-sig-aware accidentals. Returns `None` for unknown root letters.
///
/// If the user typed an explicit root accidental, preserve it on the root
/// only.
///
/// Port of `renderGenericChord` in `web-ctrmml/src/mml/chord.ts`.
pub fn render_generic_chord(
    root_letter_lower: char,
    root_accidental: Option<RootAccidental>,
    size: ChordSize,
) -> Option<String> {
    let root_idx = root_index(root_letter_lower)?;
    let steps = size.steps();
    let mut parts: Vec<String> = Vec::with_capacity(steps.len());
    for (i, s) in steps.iter().enumerate() {
        let letter = CHORD_LETTERS[(root_idx + *s as usize) % CHORD_LETTERS.len()];
        if i == 0 {
            let mut p = String::new();
            p.push(letter);
            p.push_str(accidental_char(root_accidental));
            parts.push(p);
        } else {
            parts.push(letter.to_string());
        }
    }
    Some(parts.join("/"))
}

/// Render a named chord as a ctrmml branch expression (e.g. `"c/e-/g"`).
///
/// Every chord tone's pitch equals the root pitch (user-typed accidental
/// or the ambient key-sig value) plus the named interval. Non-root tones
/// emit an accidental only when it differs from the ambient key signature
/// spelling.
///
/// Port of `renderChord` in `web-ctrmml/src/mml/chord.ts`.
pub fn render_chord(
    root_letter_lower: char,
    root_accidental: Option<RootAccidental>,
    def: &ChordDef,
    key_sig: &KeySig,
) -> Option<String> {
    let (root_idx, root_adjusted_sem) = root_state(root_letter_lower, root_accidental, key_sig)?;

    let mut parts: Vec<String> = Vec::with_capacity(def.letter_steps.len());
    for (k, &step) in def.letter_steps.iter().enumerate() {
        let letter_idx = (root_idx + step as usize) % CHORD_LETTERS.len();
        let letter = CHORD_LETTERS[letter_idx];
        let letter_sem = chord_natural_semitones(letter)?;
        let mut acc = root_adjusted_sem + def.intervals[k] - letter_sem;
        if acc > 6 {
            acc -= 12;
        }
        if acc < -6 {
            acc += 12;
        }

        let mut note_text = String::new();
        note_text.push(letter);
        if k == 0 && root_accidental.is_some() {
            note_text.push_str(accidental_char(root_accidental));
        } else {
            note_text.push_str(non_root_accidental_glyph(acc, letter, key_sig));
        }
        parts.push(note_text);
    }
    Some(parts.join("/"))
}

#[derive(Debug, Clone, Copy)]
struct StackedTone {
    letter: char,
    /// Accidental relative to the natural letter (positive = sharp).
    acc: i32,
}

fn stack_tones(
    tones: &[StackedTone],
    root_accidental: Option<RootAccidental>,
    key_sig: &KeySig,
    channel_octaves: &[i32],
    compensate: bool,
) -> String {
    // Build the working chans array. If fewer entries than tones, extend by
    // repeating the last value (TS: `chans[chans.length - 1] ?? 6`).
    let mut chans: Vec<i32> = channel_octaves.to_vec();
    while chans.len() < tones.len() {
        let last = chans.last().copied().unwrap_or(6);
        chans.push(last);
    }
    let original = chans.clone();

    let mut parts: Vec<String> = Vec::with_capacity(tones.len());
    let mut prev_pitch: i32 = -1;
    for (k, tone) in tones.iter().enumerate() {
        let letter = tone.letter;
        let acc = tone.acc;
        let letter_sem = chord_natural_semitones(letter).unwrap_or(0);

        // Natural-letter octave (without the accidental) is what the `>` / `<`
        // prefix needs to shift the channel to; the accidental is emitted as a
        // suffix on the letter and doesn't alter the channel octave.
        let target_oct: i32;
        let target_pitch: i32;
        if k == 0 {
            target_oct = chans[0];
            target_pitch = (target_oct - 1) * 12 + letter_sem + acc;
        } else {
            // `Math.floor((prevPitch - letterSem) / 12) + 1`. `div_euclid`
            // matches `Math.floor` for the positive divisor 12, where Rust's
            // built-in `/` would truncate toward zero on negative dividends.
            let mut t_oct = (prev_pitch - letter_sem).div_euclid(12) + 1;
            let mut t_pitch = (t_oct - 1) * 12 + letter_sem + acc;
            while t_pitch < prev_pitch {
                t_oct += 1;
                t_pitch += 12;
            }
            target_oct = t_oct;
            target_pitch = t_pitch;
        }

        let shift = target_oct - chans[k];
        chans[k] = target_oct;

        let mut note_text = String::with_capacity(shift.unsigned_abs() as usize + 2);
        let (glyph, count) = if shift > 0 {
            ('>', shift as usize)
        } else {
            ('<', shift.unsigned_abs() as usize)
        };
        for _ in 0..count {
            note_text.push(glyph);
        }
        note_text.push(letter);
        if k == 0 && root_accidental.is_some() {
            note_text.push_str(accidental_char(root_accidental));
        } else {
            note_text.push_str(non_root_accidental_glyph(acc, letter, key_sig));
        }
        parts.push(note_text);
        prev_pitch = target_pitch;
    }

    if compensate {
        for k in 0..parts.len() {
            let diff = chans[k] - original[k];
            if diff > 0 {
                for _ in 0..diff {
                    parts[k].push('<');
                }
            } else if diff < 0 {
                for _ in 0..(-diff) {
                    parts[k].push('>');
                }
            }
        }
    }

    parts.join("/")
}

/// Named chord, stacked bottom-up using `>` / `<` per branch. Pitch of each
/// tone is determined by the chord's interval table.
///
/// If `compensate` is true, each branch whose channel octave changed also
/// gets a trailing `<` / `>` to restore its prior state — use this when a
/// plain note follows the chord and its pitch must be preserved.
///
/// Port of `renderStackedChord` in `web-ctrmml/src/mml/chord.ts`.
pub fn render_stacked_chord(
    root_letter_lower: char,
    root_accidental: Option<RootAccidental>,
    def: &ChordDef,
    key_sig: &KeySig,
    channel_octaves: &[i32],
    compensate: bool,
) -> Option<String> {
    let (root_idx, root_adjusted_sem) = root_state(root_letter_lower, root_accidental, key_sig)?;

    let tones: Vec<StackedTone> = def
        .letter_steps
        .iter()
        .enumerate()
        .map(|(k, &step)| {
            let letter = CHORD_LETTERS[(root_idx + step as usize) % CHORD_LETTERS.len()];
            let mut acc =
                root_adjusted_sem + def.intervals[k] - chord_natural_semitones(letter).unwrap_or(0);
            if acc > 6 {
                acc -= 12;
            }
            if acc < -6 {
                acc += 12;
            }
            StackedTone { letter, acc }
        })
        .collect();

    Some(stack_tones(
        &tones,
        root_accidental,
        key_sig,
        channel_octaves,
        compensate,
    ))
}

/// Generic diatonic dyad: root letter + the letter `step` positions away.
/// Letters only; accidentals follow the ambient key signature at playback
/// (no override emitted, mirroring [`render_generic_chord`]).
///
/// `step` is `1..=6` for a 2nd through 7th. Returns `None` for an unknown
/// root letter or out-of-range step.
pub fn render_generic_diatonic_dyad(
    root_letter_lower: char,
    root_accidental: Option<RootAccidental>,
    step: i32,
) -> Option<String> {
    if !(1..=6).contains(&step) {
        return None;
    }
    let root_idx = root_index(root_letter_lower)?;
    let upper_letter = CHORD_LETTERS[(root_idx + step as usize) % CHORD_LETTERS.len()];
    let mut out = String::with_capacity(4);
    out.push(root_letter_lower);
    out.push_str(accidental_char(root_accidental));
    out.push('/');
    out.push(upper_letter);
    Some(out)
}

/// Stacked variant of [`render_generic_diatonic_dyad`] using `>` / `<` to
/// keep the upper tone above the root.
pub fn render_stacked_generic_diatonic_dyad(
    root_letter_lower: char,
    root_accidental: Option<RootAccidental>,
    step: i32,
    key_sig: &KeySig,
    channel_octaves: &[i32],
    compensate: bool,
) -> Option<String> {
    if !(1..=6).contains(&step) {
        return None;
    }
    let root_idx = root_index(root_letter_lower)?;
    let upper_letter = CHORD_LETTERS[(root_idx + step as usize) % CHORD_LETTERS.len()];
    let root_acc = match root_accidental {
        Some(a) => a.as_i32(),
        None => key_sig.get_or_zero(root_letter_lower) as i32,
    };
    let tones = [
        StackedTone {
            letter: root_letter_lower,
            acc: root_acc,
        },
        StackedTone {
            letter: upper_letter,
            acc: key_sig.get_or_zero(upper_letter) as i32,
        },
    ];
    Some(stack_tones(
        &tones,
        root_accidental,
        key_sig,
        channel_octaves,
        compensate,
    ))
}

/// Diatonic chord by letter only (no forced interval table), stacked
/// bottom-up. Each tone's accidental comes from the ambient key signature.
/// Mirrors [`render_generic_chord`] but uses `>` / `<` to keep each tone
/// above the previous.
///
/// Port of `renderStackedGenericChord` in `web-ctrmml/src/mml/chord.ts`.
pub fn render_stacked_generic_chord(
    root_letter_lower: char,
    root_accidental: Option<RootAccidental>,
    size: ChordSize,
    key_sig: &KeySig,
    channel_octaves: &[i32],
    compensate: bool,
) -> Option<String> {
    let root_idx = root_index(root_letter_lower)?;
    let steps = size.steps();
    let tones: Vec<StackedTone> = steps
        .iter()
        .enumerate()
        .map(|(k, &step)| {
            let letter = CHORD_LETTERS[(root_idx + step as usize) % CHORD_LETTERS.len()];
            let default_acc = key_sig.get_or_zero(letter) as i32;
            let acc = if k == 0 {
                match root_accidental {
                    Some(a) => a.as_i32(),
                    None => default_acc,
                }
            } else {
                default_acc
            };
            StackedTone { letter, acc }
        })
        .collect();

    Some(stack_tones(
        &tones,
        root_accidental,
        key_sig,
        channel_octaves,
        compensate,
    ))
}

// ---------------------------------------------------------------------------
// Tests — ported from `web-ctrmml/src/mml/chord.test.ts`.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn by_name(suffix: &str) -> &'static ChordDef {
        CHORDS_3
            .iter()
            .chain(CHORDS_4.iter())
            .find(|d| d.suffix == suffix)
            .unwrap_or_else(|| panic!("no chord def for {:?}", suffix))
    }

    fn ks() -> KeySig {
        KeySig::new()
    }

    // ----- renderGenericChord ------------------------------------------------

    #[test]
    fn generic_c_triad() {
        assert_eq!(
            render_generic_chord('c', None, ChordSize::Triad).as_deref(),
            Some("c/e/g")
        );
    }

    #[test]
    fn generic_f_triad_uses_plain_letters() {
        assert_eq!(
            render_generic_chord('f', None, ChordSize::Triad).as_deref(),
            Some("f/a/c")
        );
    }

    #[test]
    fn generic_preserves_root_accidental() {
        assert_eq!(
            render_generic_chord('c', Some(RootAccidental::Sharp), ChordSize::Triad).as_deref(),
            Some("c+/e/g")
        );
    }

    // ----- renderGenericDiatonicDyad -----------------------------------------

    #[test]
    fn dyad_c_p5_is_c_over_g() {
        assert_eq!(
            render_generic_diatonic_dyad('c', None, 4).as_deref(),
            Some("c/g")
        );
    }

    #[test]
    fn dyad_b_p5_wraps_to_b_over_f() {
        assert_eq!(
            render_generic_diatonic_dyad('b', None, 4).as_deref(),
            Some("b/f")
        );
    }

    #[test]
    fn dyad_c_2nd_is_c_over_d() {
        assert_eq!(
            render_generic_diatonic_dyad('c', None, 1).as_deref(),
            Some("c/d")
        );
    }

    #[test]
    fn dyad_preserves_root_accidental() {
        assert_eq!(
            render_generic_diatonic_dyad('c', Some(RootAccidental::Sharp), 4).as_deref(),
            Some("c+/g")
        );
    }

    #[test]
    fn dyad_rejects_out_of_range_step() {
        assert!(render_generic_diatonic_dyad('c', None, 0).is_none());
        assert!(render_generic_diatonic_dyad('c', None, 7).is_none());
    }

    #[test]
    fn stacked_dyad_c_p5_at_o4_is_c_over_g() {
        assert_eq!(
            render_stacked_generic_diatonic_dyad('c', None, 4, &ks(), &[4, 4], false).as_deref(),
            Some("c/g")
        );
    }

    #[test]
    fn stacked_dyad_b_p5_at_o4_lifts_upper() {
        // b4 to f needs a `>` so the upper tone stays above b4.
        assert_eq!(
            render_stacked_generic_diatonic_dyad('b', None, 4, &ks(), &[4, 4], false).as_deref(),
            Some("b/>f")
        );
    }

    #[test]
    fn stacked_dyad_compensates_per_branch() {
        // After lifting the upper f, compensate=true restores its channel
        // octave with a trailing `<` so a plain note following the dyad
        // keeps its original pitch.
        let s = render_stacked_generic_diatonic_dyad('b', None, 4, &ks(), &[4, 4], true).unwrap();
        assert_eq!(s, "b/>f<");
    }

    // ----- renderChord (named, key-sig aware) -------------------------------

    #[test]
    fn named_c_major() {
        assert_eq!(
            render_chord('c', None, by_name(""), &ks()).as_deref(),
            Some("c/e/g")
        );
    }

    #[test]
    fn named_c_minor() {
        assert_eq!(
            render_chord('c', None, by_name("m"), &ks()).as_deref(),
            Some("c/e-/g")
        );
    }

    #[test]
    fn named_c_dim() {
        assert_eq!(
            render_chord('c', None, by_name("dim"), &ks()).as_deref(),
            Some("c/e-/g-")
        );
    }

    #[test]
    fn named_omits_accidental_when_keysig_provides_it() {
        let key = KeySig::new().with('e', -1);
        assert_eq!(
            render_chord('c', None, by_name("m"), &key).as_deref(),
            Some("c/e/g")
        );
    }

    #[test]
    fn named_c_sus2() {
        assert_eq!(
            render_chord('c', None, by_name("sus2"), &ks()).as_deref(),
            Some("c/d/g")
        );
    }

    #[test]
    fn named_c_4th_triad() {
        assert_eq!(
            render_chord('c', None, by_name("4th"), &ks()).as_deref(),
            Some("c/f/b-")
        );
    }

    #[test]
    fn named_c_dim7() {
        // Uses dim6 letter for unspellable bbb.
        assert_eq!(
            render_chord('c', None, by_name("dim7"), &ks()).as_deref(),
            Some("c/e-/g-/a")
        );
    }

    #[test]
    fn named_c_add9() {
        let def = CHORDS_4.iter().find(|d| d.suffix == "add9").unwrap();
        assert_eq!(
            render_chord('c', None, def, &ks()).as_deref(),
            Some("c/e/g/d")
        );
    }

    #[test]
    fn named_c_4th_seventh() {
        let def = CHORDS_4.iter().find(|d| d.suffix == "4th").unwrap();
        assert_eq!(
            render_chord('c', None, def, &ks()).as_deref(),
            Some("c/f/b-/e-")
        );
    }

    // ----- renderStackedChord (named) ---------------------------------------

    #[test]
    fn stacked_c_major_o4() {
        let major = &CHORDS_3[0];
        assert_eq!(
            render_stacked_chord('c', None, major, &ks(), &[4, 4, 4], false).as_deref(),
            Some("c/e/g")
        );
    }

    #[test]
    fn stacked_f_major_lifts_third_tone() {
        let major = &CHORDS_3[0];
        assert_eq!(
            render_stacked_chord('f', None, major, &ks(), &[4, 4, 4], false).as_deref(),
            Some("f/a/>c")
        );
    }

    #[test]
    fn stacked_f_m7_lifts_both_upper_tones() {
        let m7 = &CHORDS_4[0];
        assert_eq!(
            render_stacked_chord('f', None, m7, &ks(), &[4, 4, 4, 4], false).as_deref(),
            Some("f/a/>c/>e")
        );
    }

    #[test]
    fn stacked_f_major_compensates() {
        let major = &CHORDS_3[0];
        assert_eq!(
            render_stacked_chord('f', None, major, &ks(), &[4, 4, 4], true).as_deref(),
            Some("f/a/>c<")
        );
    }

    #[test]
    fn stacked_compensation_is_noop_when_no_branch_changed() {
        let major = &CHORDS_3[0];
        assert_eq!(
            render_stacked_chord('c', None, major, &ks(), &[4, 4, 4], true).as_deref(),
            Some("c/e/g")
        );
    }

    // ----- renderStackedGenericChord ----------------------------------------

    #[test]
    fn stacked_generic_c_diatonic_triad() {
        assert_eq!(
            render_stacked_generic_chord('c', None, ChordSize::Triad, &ks(), &[4, 4, 4], false)
                .as_deref(),
            Some("c/e/g")
        );
    }

    #[test]
    fn stacked_generic_f_wraps_c_above() {
        assert_eq!(
            render_stacked_generic_chord('f', None, ChordSize::Triad, &ks(), &[4, 4, 4], false)
                .as_deref(),
            Some("f/a/>c")
        );
    }

    #[test]
    fn stacked_generic_g_when_ch3_at_o5() {
        assert_eq!(
            render_stacked_generic_chord('g', None, ChordSize::Triad, &ks(), &[4, 4, 5], false)
                .as_deref(),
            Some("g/b/d")
        );
    }

    #[test]
    fn stacked_generic_c_pulls_elevated_channels_down() {
        assert_eq!(
            render_stacked_generic_chord('c', None, ChordSize::Triad, &ks(), &[4, 5, 5], false)
                .as_deref(),
            Some("c/<e/<g")
        );
    }

    #[test]
    fn stacked_generic_compensates_g() {
        assert_eq!(
            render_stacked_generic_chord('g', None, ChordSize::Triad, &ks(), &[4, 4, 4], true)
                .as_deref(),
            Some("g/b/>d<")
        );
    }

    #[test]
    fn stacked_generic_full_chain() {
        // Walks c-d-e-f-g-a-b-c, accumulating octave shifts after each branch,
        // and expects an exact sequence of outputs.
        let mut ch = [4_i32, 4, 4];
        let roots = ['c', 'd', 'e', 'f', 'g', 'a', 'b', 'c'];
        let mut got: Vec<String> = Vec::with_capacity(roots.len());
        for &root in &roots {
            let s = render_stacked_generic_chord(
                root,
                None,
                ChordSize::Triad,
                &ks(),
                &ch,
                false,
            )
            .unwrap();
            // Mutate ch according to leading >/< glyphs on each branch.
            for (k, branch) in s.split('/').enumerate() {
                let mut shift = 0_i32;
                for c in branch.chars() {
                    match c {
                        '>' => shift += 1,
                        '<' => shift -= 1,
                        _ => break,
                    }
                }
                ch[k] += shift;
            }
            got.push(s);
        }
        assert_eq!(
            got,
            vec![
                "c/e/g",
                "d/f/a",
                "e/g/b",
                "f/a/>c",
                "g/b/d",
                "a/>c/e",
                "b/d/f",
                "c/<e/<g",
            ]
        );
    }

    // ----- accidental_char --------------------------------------------------

    #[test]
    fn accidental_char_glyphs() {
        assert_eq!(accidental_char(Some(RootAccidental::Sharp)), "+");
        assert_eq!(accidental_char(Some(RootAccidental::Flat)), "-");
        assert_eq!(accidental_char(Some(RootAccidental::Natural)), "=");
        assert_eq!(accidental_char(None), "");
    }

}
