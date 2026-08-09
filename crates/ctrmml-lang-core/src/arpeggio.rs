//! Sequential arpeggio rendering, ported from megamml's `mml/arpeggio.ts`.
//!
//! Chord definitions are resolved to absolute pitches first, then a pattern
//! indexes those pitches while the renderer threads ctrmml `>` / `<` octave
//! shifts through the resulting note sequence.

use crate::chord::{
    chord_natural_semitones, spell_tone, ChordDef, RootAccidental, SpelledTone, CHORDS_3, CHORDS_4,
    CHORD_LETTERS,
};
use crate::completion::ArpeggioPattern;
use crate::key_sig::KeySig;

/// The five arpeggio patterns in their settings/display order.
pub const PATTERNS: [ArpeggioPattern; 5] = [
    ArpeggioPattern::Up,
    ArpeggioPattern::Down,
    ArpeggioPattern::UpDown,
    ArpeggioPattern::DownUp,
    ArpeggioPattern::Alberti,
];

impl ArpeggioPattern {
    /// Short identifier used in completion detail text.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Up => "Up",
            Self::Down => "Down",
            Self::UpDown => "UpDown",
            Self::DownUp => "DownUp",
            Self::Alberti => "Alberti",
        }
    }

    /// Human-readable description of the traversal.
    pub const fn detail(self) -> &'static str {
        match self {
            Self::Up => "Ascending",
            Self::Down => "Descending",
            Self::UpDown => "Up then down (no endpoint repeat)",
            Self::DownUp => "Down then up (no endpoint repeat)",
            Self::Alberti => "Alberti bass (low-high-mid-high)",
        }
    }

    /// Minimum number of chord tones required by this pattern.
    pub const fn min_notes(self) -> usize {
        match self {
            Self::Up | Self::Down => 2,
            Self::UpDown | Self::DownUp | Self::Alberti => 3,
        }
    }

    /// Generate indices into a chord-tone list of `size` notes.
    pub fn indices(self, size: usize) -> Vec<usize> {
        match self {
            Self::Up => (0..size).collect(),
            Self::Down => (0..size).rev().collect(),
            Self::UpDown => {
                let mut out: Vec<usize> = (0..size).collect();
                if size >= 2 {
                    out.extend((1..size - 1).rev());
                }
                out
            }
            Self::DownUp => {
                let mut out: Vec<usize> = (0..size).rev().collect();
                if size >= 2 {
                    out.extend(1..size - 1);
                }
                out
            }
            Self::Alberti if size == 4 => vec![0, 3, 1, 2],
            Self::Alberti if size >= 3 => vec![0, 2, 1, 2],
            Self::Alberti => Vec::new(),
        }
    }
}

/// Look up a pattern by its case-sensitive display name.
pub fn pattern_by_name(name: &str) -> Option<ArpeggioPattern> {
    PATTERNS
        .iter()
        .copied()
        .find(|pattern| pattern.name() == name)
}

/// One absolute chord tone with its intended diatonic spelling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResolvedNote {
    /// Natural letter (`c..b`).
    pub letter: char,
    /// Accidental relative to the natural letter.
    pub accidental: i32,
    /// Absolute MML octave number.
    pub octave: i32,
    /// Abstract pitch (`octave * 12 + pitch class`).
    pub pitch: i32,
}

fn root_accidental_value(accidental: RootAccidental) -> i32 {
    match accidental {
        RootAccidental::Sharp => 1,
        RootAccidental::Natural => 0,
        RootAccidental::Flat => -1,
    }
}

fn letter_index(letter: char) -> Option<usize> {
    CHORD_LETTERS
        .iter()
        .position(|candidate| *candidate == letter)
}

/// Resolve a chord definition into absolute notes beginning at `base_octave`.
pub fn resolve_chord_notes(
    root_letter: char,
    root_accidental: Option<RootAccidental>,
    def: &ChordDef,
    key_sig: &KeySig,
    base_octave: i32,
) -> Option<Vec<ResolvedNote>> {
    let root_index = letter_index(root_letter)?;
    let root_accidental = root_accidental
        .map(root_accidental_value)
        .unwrap_or_else(|| key_sig.get_or_zero(root_letter) as i32);
    let root_pitch = base_octave * 12 + chord_natural_semitones(root_letter)? + root_accidental;

    Some(
        def.intervals
            .iter()
            .enumerate()
            .map(|(index, interval)| {
                let target_pitch = root_pitch + interval;
                let letter_position = root_index + def.letter_steps[index] as usize;
                let octave = base_octave + (letter_position / CHORD_LETTERS.len()) as i32;
                let letter = CHORD_LETTERS[letter_position % CHORD_LETTERS.len()];
                let natural_pitch = octave * 12 + chord_natural_semitones(letter).unwrap_or(0);
                let accidental = target_pitch - natural_pitch;
                ResolvedNote {
                    letter,
                    accidental,
                    octave,
                    pitch: target_pitch,
                }
            })
            .collect(),
    )
}

fn render_mml_note_with_policy(
    note: ResolvedNote,
    current_octave: i32,
    key_sig: &KeySig,
    preserve_spelling: bool,
) -> Option<(String, i32)> {
    let spelled = if preserve_spelling {
        SpelledTone {
            letter: note.letter,
            accidental: note.accidental,
            octave: note.octave,
        }
    } else {
        spell_tone(note.letter, note.accidental, note.octave)?
    };
    let octave_shift = spelled.octave - current_octave;
    let (shift, count) = if octave_shift > 0 {
        ('>', octave_shift as usize)
    } else {
        ('<', octave_shift.unsigned_abs() as usize)
    };
    let mut mml = String::with_capacity(count + 2);
    for _ in 0..count {
        mml.push(shift);
    }
    mml.push(spelled.letter);

    if spelled.accidental != key_sig.get_or_zero(spelled.letter) as i32 {
        match spelled.accidental {
            1 => mml.push('+'),
            -1 => mml.push('-'),
            0 => mml.push('='),
            _ => {}
        }
    }
    Some((mml, spelled.octave))
}

/// Render one non-root resolved note while updating the current octave.
pub fn render_mml_note(
    note: ResolvedNote,
    current_octave: i32,
    key_sig: &KeySig,
) -> Option<(String, i32)> {
    render_mml_note_with_policy(note, current_octave, key_sig, false)
}

/// Render a pattern-indexed sequence of resolved chord tones.
pub fn render_arpeggio_body(
    notes: &[ResolvedNote],
    pattern_indices: &[usize],
    starting_octave: i32,
    key_sig: &KeySig,
) -> Option<String> {
    let mut current_octave = starting_octave;
    let mut result = String::new();
    for &index in pattern_indices {
        let Some(note) = notes.get(index) else {
            continue;
        };
        let (mml, new_octave) =
            render_mml_note_with_policy(*note, current_octave, key_sig, index == 0)?;
        result.push_str(&mml);
        current_octave = new_octave;
    }

    let net_displacement = result.bytes().fold(0i32, |net, byte| match byte {
        b'>' => net + 1,
        b'<' => net - 1,
        _ => net,
    });
    if net_displacement > 0 {
        result.push_str(&"<".repeat(net_displacement as usize));
    } else if net_displacement < 0 {
        result.push_str(&">".repeat((-net_displacement) as usize));
    }

    Some(result)
}

/// Render a named chord as a plain ascending arpeggio.
pub fn render_chord_arpeggio(
    root_letter: char,
    root_accidental: Option<RootAccidental>,
    def: &ChordDef,
    key_sig: &KeySig,
    starting_octave: i32,
) -> Option<String> {
    let notes = resolve_chord_notes(root_letter, root_accidental, def, key_sig, starting_octave)?;
    let indices: Vec<usize> = (0..notes.len()).collect();
    render_arpeggio_body(&notes, &indices, starting_octave, key_sig)
}

/// Render a named chord using one of the five configured patterns.
pub fn render_chord_arpeggio_with_pattern(
    root_letter: char,
    root_accidental: Option<RootAccidental>,
    def: &ChordDef,
    key_sig: &KeySig,
    starting_octave: i32,
    pattern: ArpeggioPattern,
) -> Option<String> {
    let notes = resolve_chord_notes(root_letter, root_accidental, def, key_sig, starting_octave)?;
    if notes.len() < pattern.min_notes() {
        return None;
    }
    let indices = pattern.indices(notes.len());
    if indices.is_empty() {
        return None;
    }
    render_arpeggio_body(&notes, &indices, starting_octave, key_sig)
}

/// Render a generic diatonic triad or seventh as a patterned arpeggio.
pub fn render_generic_arpeggio(
    root_letter: char,
    root_accidental: Option<RootAccidental>,
    size: usize,
    key_sig: &KeySig,
    starting_octave: i32,
    pattern: ArpeggioPattern,
) -> Option<String> {
    let root_index = letter_index(root_letter)?;
    let letter_steps: &[usize] = match size {
        3 => &[0, 2, 4],
        4 => &[0, 2, 4, 6],
        _ => return None,
    };
    let mut notes = Vec::with_capacity(letter_steps.len());
    let mut previous_pitch: Option<i32> = None;

    for (index, step) in letter_steps.iter().copied().enumerate() {
        let letter = CHORD_LETTERS[(root_index + step) % CHORD_LETTERS.len()];
        let natural_pc = chord_natural_semitones(letter)?;
        let accidental = if index == 0 {
            root_accidental
                .map(root_accidental_value)
                .unwrap_or_else(|| key_sig.get_or_zero(letter) as i32)
        } else {
            key_sig.get_or_zero(letter) as i32
        };
        let mut octave = previous_pitch
            .map(|pitch| pitch.div_euclid(12))
            .unwrap_or(starting_octave);
        let mut pitch = octave * 12 + natural_pc + accidental;
        while previous_pitch.is_some_and(|previous| pitch <= previous) {
            octave += 1;
            pitch = octave * 12 + natural_pc + accidental;
        }
        notes.push(ResolvedNote {
            letter,
            accidental,
            octave,
            pitch,
        });
        previous_pitch = Some(pitch);
    }

    if notes.len() < pattern.min_notes() {
        return None;
    }
    let indices = pattern.indices(notes.len());
    if indices.is_empty() {
        return None;
    }
    render_arpeggio_body(&notes, &indices, starting_octave, key_sig)
}

/// Parse an MML note sequence into resolved notes.
pub fn parse_note_sequence(
    text: &str,
    starting_octave: i32,
    key_sig: &KeySig,
) -> Option<Vec<ResolvedNote>> {
    let bytes = text.as_bytes();
    let mut notes = Vec::new();
    let mut octave = starting_octave;
    let mut index = 0usize;

    while index < bytes.len() {
        match bytes[index] {
            b'>' => {
                octave += 1;
                index += 1;
                continue;
            }
            b'<' => {
                octave -= 1;
                index += 1;
                continue;
            }
            _ => {}
        }
        let letter = (bytes[index] as char).to_ascii_lowercase();
        chord_natural_semitones(letter)?;
        index += 1;
        let accidental = match bytes.get(index).copied() {
            Some(b'+') => {
                index += 1;
                1
            }
            Some(b'-') => {
                index += 1;
                -1
            }
            Some(b'=') => {
                index += 1;
                0
            }
            _ => key_sig.get_or_zero(letter) as i32,
        };
        let natural_pc = chord_natural_semitones(letter)?;
        notes.push(ResolvedNote {
            letter,
            accidental,
            octave,
            pitch: octave * 12 + natural_pc + accidental,
        });
    }
    (!notes.is_empty()).then_some(notes)
}

/// All named chord definitions available as arpeggio starting points.
pub fn chord_defs_for_arpeggio() -> impl Iterator<Item = (&'static ChordDef, usize)> {
    CHORDS_3
        .iter()
        .map(|def| (def, 3))
        .chain(CHORDS_4.iter().map(|def| (def, 4)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chord::{
        render_chord, render_generic_chord, render_generic_diatonic_dyad, render_stacked_chord,
        render_stacked_generic_chord, render_stacked_generic_diatonic_dyad, ChordSize, DYADS,
    };
    use std::collections::BTreeSet;

    fn no_key() -> KeySig {
        KeySig::new()
    }

    fn named(suffix: &str) -> &'static ChordDef {
        CHORDS_3
            .iter()
            .chain(CHORDS_4)
            .find(|def| def.suffix == suffix)
            .unwrap()
    }

    #[derive(Debug, Clone, Copy)]
    enum OracleKeySig {
        None,
        SharpSide,
        FlatSide,
    }

    impl OracleKeySig {
        fn accidental(self, letter: char) -> i32 {
            match (self, letter) {
                (Self::SharpSide, 'f') => 1,
                (Self::FlatSide, 'b') => -1,
                _ => 0,
            }
        }

        fn production(self) -> KeySig {
            match self {
                Self::None => KeySig::new(),
                Self::SharpSide => KeySig::new().with('f', 1),
                Self::FlatSide => KeySig::new().with('b', -1),
            }
        }
    }

    /// Evaluate rendered ctrmml text without consulting production note
    /// resolution. Slash-delimited chord branches each restart from the
    /// channel octave; an arpeggio has one branch, so octave shifts persist.
    fn evaluate_rendered(
        text: &str,
        starting_channel_octave: i32,
        key_sig: OracleKeySig,
    ) -> BTreeSet<(i32, i32)> {
        let mut pitches = BTreeSet::new();

        for branch in text.split('/') {
            assert!(!branch.is_empty(), "empty MML branch in {text:?}");
            let mut chars = branch.chars().peekable();
            let mut octave = starting_channel_octave;

            while let Some(character) = chars.next() {
                match character {
                    '>' => octave += 1,
                    '<' => octave -= 1,
                    letter @ ('a'..='g') => {
                        let accidental = match chars.peek().copied() {
                            Some('+') => {
                                chars.next();
                                1
                            }
                            Some('-') => {
                                chars.next();
                                -1
                            }
                            Some('=') => {
                                chars.next();
                                0
                            }
                            _ => key_sig.accidental(letter),
                        };
                        let pitch =
                            octave * 12 + chord_natural_semitones(letter).unwrap() + accidental;
                        pitches.insert((pitch.rem_euclid(12), pitch.div_euclid(12)));
                    }
                    other => panic!("unexpected MML character {other:?} in {text:?}"),
                }
            }
        }

        pitches
    }

    fn assert_rendered_sets_match(
        chord: &str,
        arpeggio: &str,
        starting_channel_octave: i32,
        key_sig: OracleKeySig,
        compare_absolute_pitches: bool,
        context: &str,
    ) {
        let chord_pitches = evaluate_rendered(chord, starting_channel_octave, key_sig);
        let arpeggio_pitches = evaluate_rendered(arpeggio, starting_channel_octave, key_sig);

        if compare_absolute_pitches {
            assert_eq!(
                chord_pitches, arpeggio_pitches,
                "{context}: chord={chord:?}, arpeggio={arpeggio:?}"
            );
        } else {
            let pitch_classes = |pitches: &BTreeSet<(i32, i32)>| {
                pitches
                    .iter()
                    .map(|&(pitch_class, _octave)| pitch_class)
                    .collect::<BTreeSet<_>>()
            };
            assert_eq!(
                pitch_classes(&chord_pitches),
                pitch_classes(&arpeggio_pitches),
                "{context}: chord={chord:?}, arpeggio={arpeggio:?}"
            );
        }
    }

    fn render_oracle_named_arpeggio(
        root: char,
        root_accidental: Option<RootAccidental>,
        def: &ChordDef,
        key_sig: OracleKeySig,
        stacked: bool,
    ) -> String {
        const LETTERS: [char; 7] = ['c', 'd', 'e', 'f', 'g', 'a', 'b'];
        let root_index = LETTERS.iter().position(|letter| *letter == root).unwrap();
        let root_accidental_value = match root_accidental {
            Some(RootAccidental::Sharp) => 1,
            Some(RootAccidental::Flat) => -1,
            Some(RootAccidental::Natural) => 0,
            None => key_sig.accidental(root),
        };
        let root_natural = chord_natural_semitones(root).unwrap();
        let root_pitch = 4 * 12 + root_natural + root_accidental_value;
        let mut previous_pitch = root_pitch;
        let notes = def
            .letter_steps
            .iter()
            .zip(def.intervals)
            .enumerate()
            .map(|(index, (&step, &interval))| {
                if index == 0 {
                    return ResolvedNote {
                        letter: root,
                        accidental: root_accidental_value,
                        octave: 4,
                        pitch: root_pitch,
                    };
                }

                let letter_position = root_index + step as usize;
                let letter = LETTERS[letter_position % LETTERS.len()];
                let natural = chord_natural_semitones(letter).unwrap();
                let mut accidental = root_natural + root_accidental_value + interval - natural;
                if accidental > 6 {
                    accidental -= 12;
                }
                if accidental < -6 {
                    accidental += 12;
                }
                let mut pitch = root_pitch + interval;
                if stacked {
                    while pitch < previous_pitch {
                        pitch += 12;
                    }
                }
                let octave = (pitch - natural - accidental).div_euclid(12);
                previous_pitch = pitch;
                ResolvedNote {
                    letter,
                    accidental,
                    octave,
                    pitch,
                }
            })
            .collect::<Vec<_>>();

        render_arpeggio_body(
            &notes,
            &(0..notes.len()).collect::<Vec<_>>(),
            4,
            &key_sig.production(),
        )
        .unwrap()
    }

    #[test]
    fn resolves_named_chord_octaves_and_accidentals() {
        let c_major = resolve_chord_notes('c', None, named(""), &no_key(), 4).unwrap();
        assert_eq!(
            c_major,
            vec![
                ResolvedNote {
                    letter: 'c',
                    accidental: 0,
                    octave: 4,
                    pitch: 48
                },
                ResolvedNote {
                    letter: 'e',
                    accidental: 0,
                    octave: 4,
                    pitch: 52
                },
                ResolvedNote {
                    letter: 'g',
                    accidental: 0,
                    octave: 4,
                    pitch: 55
                },
            ]
        );
        let f_major = resolve_chord_notes('f', None, named(""), &no_key(), 4).unwrap();
        assert_eq!(f_major[2].letter, 'c');
        assert_eq!(f_major[2].octave, 5);
        assert_eq!(
            resolve_chord_notes('c', None, named("m"), &no_key(), 4).unwrap()[1].accidental,
            -1
        );
    }

    #[test]
    fn renders_plain_ascending_named_chords() {
        assert_eq!(
            render_chord_arpeggio('c', None, named(""), &no_key(), 4).as_deref(),
            Some("ceg")
        );
        assert_eq!(
            render_chord_arpeggio('f', None, named(""), &no_key(), 4).as_deref(),
            Some("fa>c<")
        );
        assert_eq!(
            render_chord_arpeggio('c', None, named("m"), &no_key(), 4).as_deref(),
            Some("ce-g")
        );
        assert_eq!(
            render_chord_arpeggio('f', None, named("M7"), &no_key(), 4).as_deref(),
            Some("fa>ce<")
        );
    }

    #[test]
    fn omits_accidental_already_supplied_by_key_signature() {
        let key = KeySig::new().with('e', -1);
        assert_eq!(
            render_chord_arpeggio('c', None, named("m"), &key, 4).as_deref(),
            Some("ceg")
        );
    }

    #[test]
    fn renders_every_pattern_with_threaded_octaves() {
        let notes = resolve_chord_notes('f', None, named(""), &no_key(), 4).unwrap();
        let render = |pattern: ArpeggioPattern| {
            render_arpeggio_body(&notes, &pattern.indices(3), 4, &no_key())
        };
        assert_eq!(render(ArpeggioPattern::Up).as_deref(), Some("fa>c<"));
        assert_eq!(render(ArpeggioPattern::Down).as_deref(), Some(">c<af"));
        assert_eq!(render(ArpeggioPattern::UpDown).as_deref(), Some("fa>c<a"));
        assert_eq!(render(ArpeggioPattern::DownUp).as_deref(), Some(">c<afa"));
        assert_eq!(
            render(ArpeggioPattern::Alberti).as_deref(),
            Some("f>c<a>c<")
        );
    }

    #[test]
    fn alberti_four_note_extension_uses_every_tone() {
        assert_eq!(ArpeggioPattern::Alberti.indices(4), vec![0, 3, 1, 2]);
        assert_eq!(
            render_chord_arpeggio_with_pattern(
                'c',
                None,
                named("M7"),
                &no_key(),
                4,
                ArpeggioPattern::Alberti,
            )
            .as_deref(),
            Some("cbeg")
        );
    }

    #[test]
    fn renders_generic_patterned_arpeggios() {
        assert_eq!(
            render_generic_arpeggio('f', None, 3, &no_key(), 4, ArpeggioPattern::Up).as_deref(),
            Some("fa>c<")
        );
        assert_eq!(
            render_generic_arpeggio('c', None, 4, &no_key(), 4, ArpeggioPattern::Alberti)
                .as_deref(),
            Some("cbeg")
        );
        let key = KeySig::new().with('e', -1);
        assert_eq!(
            render_generic_arpeggio('c', None, 3, &key, 4, ArpeggioPattern::Up).as_deref(),
            Some("ceg")
        );
    }

    #[test]
    fn respells_named_chords_that_require_double_accidentals() {
        // B-sharp major resolves to B-sharp, D-double-sharp, F-double-sharp.
        assert_eq!(
            render_chord_arpeggio('b', Some(RootAccidental::Sharp), named(""), &no_key(), 4,)
                .as_deref(),
            Some("b+>eg<")
        );

        // C-flat minor resolves to C-flat, E-double-flat, G-flat; diminished
        // resolves to C-flat, E-double-flat, G-double-flat.
        assert_eq!(
            render_chord_arpeggio('c', Some(RootAccidental::Flat), named("m"), &no_key(), 4,)
                .as_deref(),
            Some("c-dg-")
        );
        assert_eq!(
            render_chord_arpeggio('c', Some(RootAccidental::Flat), named("dim"), &no_key(), 4,)
                .as_deref(),
            Some("c-df")
        );

        let e_sharp_key = KeySig::new().with('e', 1);
        assert_eq!(
            render_chord_arpeggio('b', Some(RootAccidental::Sharp), named(""), &e_sharp_key, 4,)
                .as_deref(),
            Some("b+>e=g<")
        );
    }

    #[test]
    fn all_arpeggio_bodies_are_octave_neutral() {
        let roots = [
            ('c', None, "c"),
            ('a', None, "a"),
            ('g', None, "g"),
            ('b', Some(RootAccidental::Sharp), "b+"),
            ('e', Some(RootAccidental::Flat), "e-"),
        ];
        let displacement = |body: &str| {
            body.bytes().fold(0i32, |net, byte| match byte {
                b'>' => net + 1,
                b'<' => net - 1,
                _ => net,
            })
        };
        let mut bodies = 0usize;

        for pattern in PATTERNS {
            for &(root, root_accidental, root_name) in &roots {
                for def in CHORDS_3.iter().chain(CHORDS_4) {
                    let body = render_chord_arpeggio_with_pattern(
                        root,
                        root_accidental,
                        def,
                        &no_key(),
                        4,
                        pattern,
                    )
                    .unwrap();
                    assert_eq!(
                        displacement(&body),
                        0,
                        "named {root_name}{} {} body {body:?}",
                        def.suffix,
                        pattern.name(),
                    );
                    bodies += 1;
                }

                for size in [3, 4] {
                    let body =
                        render_generic_arpeggio(root, root_accidental, size, &no_key(), 4, pattern)
                            .unwrap();
                    assert_eq!(
                        displacement(&body),
                        0,
                        "generic {size}-note {root_name} {} body {body:?}",
                        pattern.name(),
                    );
                    bodies += 1;
                }
            }
        }

        assert_eq!(bodies, 450);
    }

    #[test]
    fn rendered_chord_and_arpeggio_pitch_sets_match_across_every_renderer() {
        const LETTERS: [char; 7] = ['c', 'd', 'e', 'f', 'g', 'a', 'b'];
        let roots = [
            ('b', Some(RootAccidental::Sharp), "b+"),
            ('c', None, "c"),
            ('e', Some(RootAccidental::Flat), "e-"),
            ('g', Some(RootAccidental::Sharp), "g+"),
            ('e', Some(RootAccidental::Sharp), "e+"),
            ('c', Some(RootAccidental::Flat), "c-"),
            ('f', Some(RootAccidental::Flat), "f-"),
        ];
        let key_sigs = [
            OracleKeySig::None,
            OracleKeySig::SharpSide,
            OracleKeySig::FlatSide,
        ];
        let modes = [(false, "plain"), (true, "stacked")];
        let mut comparisons = 0usize;

        for &oracle_key_sig in &key_sigs {
            let key_sig = oracle_key_sig.production();
            for &(root, root_accidental, root_name) in &roots {
                for def in CHORDS_3.iter().chain(CHORDS_4) {
                    for &(stacked, mode_name) in &modes {
                        let arpeggio = render_oracle_named_arpeggio(
                            root,
                            root_accidental,
                            def,
                            oracle_key_sig,
                            stacked,
                        );
                        let chord = if stacked {
                            render_stacked_chord(
                                root,
                                root_accidental,
                                def,
                                &key_sig,
                                &vec![4; def.intervals.len()],
                                false,
                            )
                        } else {
                            render_chord(root, root_accidental, def, &key_sig)
                        }
                        .unwrap();
                        assert_rendered_sets_match(
                            &chord,
                            &arpeggio,
                            4,
                            oracle_key_sig,
                            stacked,
                            &format!(
                                "named {mode_name} {root_name}{} under {oracle_key_sig:?}",
                                def.suffix
                            ),
                        );
                        comparisons += 1;
                    }
                }

                for &(size, size_number) in
                    &[(ChordSize::Triad, 3usize), (ChordSize::Seventh, 4usize)]
                {
                    let arpeggio = render_generic_arpeggio(
                        root,
                        root_accidental,
                        size_number,
                        &key_sig,
                        4,
                        ArpeggioPattern::Up,
                    )
                    .unwrap();
                    for &(stacked, mode_name) in &modes {
                        let chord = if stacked {
                            render_stacked_generic_chord(
                                root,
                                root_accidental,
                                size,
                                &key_sig,
                                &vec![4; size_number],
                                false,
                            )
                        } else {
                            render_generic_chord(root, root_accidental, size)
                        }
                        .unwrap();
                        assert_rendered_sets_match(
                            &chord,
                            &arpeggio,
                            4,
                            oracle_key_sig,
                            stacked,
                            &format!(
                                "generic {mode_name} {root_name}/{size_number} under {oracle_key_sig:?}"
                            ),
                        );
                        comparisons += 1;
                    }
                }

                let root_index = LETTERS.iter().position(|letter| *letter == root).unwrap();
                let root_accidental_value = match root_accidental {
                    Some(RootAccidental::Sharp) => 1,
                    Some(RootAccidental::Flat) => -1,
                    Some(RootAccidental::Natural) => 0,
                    None => oracle_key_sig.accidental(root),
                };
                let root_pitch =
                    4 * 12 + chord_natural_semitones(root).unwrap() + root_accidental_value;

                for dyad in DYADS {
                    let upper_position = root_index + dyad.step as usize;
                    let upper_letter = LETTERS[upper_position % LETTERS.len()];
                    let upper_accidental = oracle_key_sig.accidental(upper_letter);

                    for &(stacked, mode_name) in &modes {
                        let upper_natural = chord_natural_semitones(upper_letter).unwrap();
                        let mut upper_octave = if stacked {
                            (root_pitch - upper_natural).div_euclid(12)
                        } else {
                            4 + (upper_position / LETTERS.len()) as i32
                        };
                        let mut upper_pitch = upper_octave * 12 + upper_natural + upper_accidental;
                        if stacked {
                            while upper_pitch < root_pitch {
                                upper_octave += 1;
                                upper_pitch += 12;
                            }
                        }
                        let notes = [
                            ResolvedNote {
                                letter: root,
                                accidental: root_accidental_value,
                                octave: 4,
                                pitch: root_pitch,
                            },
                            ResolvedNote {
                                letter: upper_letter,
                                accidental: upper_accidental,
                                octave: upper_octave,
                                pitch: upper_pitch,
                            },
                        ];
                        let arpeggio = render_arpeggio_body(&notes, &[0, 1], 4, &key_sig).unwrap();
                        let chord = if stacked {
                            render_stacked_generic_diatonic_dyad(
                                root,
                                root_accidental,
                                dyad.step,
                                &key_sig,
                                &[4, 4],
                                false,
                            )
                        } else {
                            render_generic_diatonic_dyad(root, root_accidental, dyad.step)
                        }
                        .unwrap();
                        assert_rendered_sets_match(
                            &chord,
                            &arpeggio,
                            4,
                            oracle_key_sig,
                            stacked,
                            &format!(
                                "dyad {mode_name} {root_name}{} under {oracle_key_sig:?}",
                                dyad.name
                            ),
                        );
                        comparisons += 1;
                    }
                }
            }
        }

        assert_eq!(comparisons, 1_008);
    }

    #[test]
    fn parses_sequences_and_rejects_unexpected_characters() {
        assert_eq!(
            parse_note_sequence("fa>c", 4, &no_key()).unwrap(),
            vec![
                ResolvedNote {
                    letter: 'f',
                    accidental: 0,
                    octave: 4,
                    pitch: 53
                },
                ResolvedNote {
                    letter: 'a',
                    accidental: 0,
                    octave: 4,
                    pitch: 57
                },
                ResolvedNote {
                    letter: 'c',
                    accidental: 0,
                    octave: 5,
                    pitch: 60
                },
            ]
        );
        assert_eq!(
            parse_note_sequence("ce-g", 4, &no_key()).unwrap()[1].accidental,
            -1
        );
        assert!(parse_note_sequence("c8", 4, &no_key()).is_none());
    }

    #[test]
    fn parse_note_sequence_preserves_accidental_octave_carry_in_absolute_pitch() {
        assert_eq!(
            parse_note_sequence("b+", 4, &no_key()).unwrap()[0].pitch,
            60
        );
        assert_eq!(
            parse_note_sequence("c-", 4, &no_key()).unwrap()[0].pitch,
            47
        );
        assert_eq!(
            parse_note_sequence(">c-<b+", 4, &no_key())
                .unwrap()
                .iter()
                .map(|note| note.pitch)
                .collect::<Vec<_>>(),
            vec![59, 60]
        );
    }

    #[test]
    fn render_parse_repattern_round_trip() {
        let inserted = render_chord_arpeggio('f', None, named(""), &no_key(), 4).unwrap();
        let parsed = parse_note_sequence(&inserted, 4, &no_key()).unwrap();
        assert_eq!(
            render_arpeggio_body(
                &parsed,
                &ArpeggioPattern::Alberti.indices(parsed.len()),
                4,
                &no_key(),
            )
            .as_deref(),
            Some("f>c<a>c<")
        );
    }

    #[test]
    fn pattern_lookup_is_case_sensitive() {
        assert_eq!(pattern_by_name("Up"), Some(ArpeggioPattern::Up));
        assert_eq!(pattern_by_name("up"), None);
    }
}
