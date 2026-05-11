//! Key signature parsing — ported from `web-ctrmml/src/mml/key-sig.ts`.
//!
//! Mirrors the semantics used by the ctrmml compiler (track.cpp L500-522):
//!
//! - A scale name (e.g. `C`, `G`, `D`, `F`, `B-`, `c+`) resets the full signature.
//! - A modifier expression (starting with `+`, `-`, or `=`) adjusts individual
//!   note letters on top of the current signature. Example: `+cfg` sharpens
//!   C/F/G; `-b` flats B; `=f` naturalises F.

use crate::track_selector::{parse_leading_track_selector, LineReader};

/// Accidental offset (in semitones) per natural letter (`'a'`..=`'g'`).
///
/// A missing entry means "no override at this position" — callers normally
/// read via [`KeySig::get_or_zero`] which collapses absent entries to `0`,
/// mirroring the TS `keySig[letter] ?? 0` pattern.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct KeySig {
    /// Indexed by `(letter - 'a')`; only `c/d/e/f/g/a/b` are meaningful.
    offsets: [Option<i8>; 7],
}

impl KeySig {
    /// Empty key signature — every letter implicit zero.
    #[inline]
    pub const fn new() -> Self {
        Self {
            offsets: [None; 7],
        }
    }

    /// Look up the explicit accidental for `letter` (`'a'`..=`'g'`).
    ///
    /// Returns `None` when no entry is set. Callers normally want
    /// [`KeySig::get_or_zero`].
    #[inline]
    pub fn get(&self, letter: char) -> Option<i8> {
        Self::index(letter).and_then(|i| self.offsets[i])
    }

    /// Look up the accidental, defaulting to `0` when unset
    /// (matches the TS `keySig[letter] ?? 0` pattern).
    #[inline]
    pub fn get_or_zero(&self, letter: char) -> i8 {
        self.get(letter).unwrap_or(0)
    }

    /// Set the accidental for `letter`. Non-natural letters are ignored.
    #[inline]
    pub fn set(&mut self, letter: char, acc: i8) {
        if let Some(i) = Self::index(letter) {
            self.offsets[i] = Some(acc);
        }
    }

    /// Builder-style setter — useful in tests.
    #[inline]
    pub fn with(mut self, letter: char, acc: i8) -> Self {
        self.set(letter, acc);
        self
    }

    /// Iterate `(letter, accidental)` pairs for letters with an explicit
    /// entry. Letters are yielded in `'a'..='g'` order.
    pub fn iter(&self) -> impl Iterator<Item = (char, i8)> + '_ {
        self.offsets
            .iter()
            .enumerate()
            .filter_map(|(i, slot)| slot.map(|v| ((b'a' + i as u8) as char, v)))
    }

    #[inline]
    fn index(letter: char) -> Option<usize> {
        match letter {
            'a'..='g' => Some((letter as u8 - b'a') as usize),
            _ => None,
        }
    }
}

/// The signature with every natural letter explicitly set to zero.
///
/// Matches the TS `DEFAULT_KEY_SIG` shape (every letter present, value 0)
/// — useful when downstream code distinguishes "explicit natural" from
/// "no entry", but most code can use [`KeySig::new`] interchangeably
/// because [`KeySig::get_or_zero`] returns `0` either way.
pub fn default_key_sig() -> KeySig {
    let mut k = KeySig::new();
    for letter in ['c', 'd', 'e', 'f', 'g', 'a', 'b'] {
        k.set(letter, 0);
    }
    k
}

/// Look up the named scale and return its key signature, or `None` if the
/// name is not a known scale. Matches the TS `KEY_SIG_SCALES` table
/// (track.cpp L500-522).
///
/// Returned signatures always have all seven naturals explicitly set
/// (matching the TS table shape).
pub fn scale_for_name(name: &str) -> Option<KeySig> {
    let sharps: &[char] = match name {
        // Sharp keys (and their relative minors)
        "C" | "a" => &[],
        "G" | "e" => &['f'],
        "D" | "b" => &['f', 'c'],
        "A" | "f+" => &['f', 'c', 'g'],
        "E" | "c+" => &['f', 'c', 'g', 'd'],
        "B" | "g+" => &['f', 'c', 'g', 'd', 'a'],
        "F+" | "d+" => &['f', 'c', 'g', 'd', 'a', 'e'],
        "C+" | "a+" => &['f', 'c', 'g', 'd', 'a', 'e', 'b'],
        _ => {
            let flats: &[char] = match name {
                "F" | "d" => &['b'],
                "B-" | "g" => &['b', 'e'],
                "E-" | "c" => &['b', 'e', 'a'],
                "A-" | "f" => &['b', 'e', 'a', 'd'],
                "D-" | "b-" => &['b', 'e', 'a', 'd', 'g'],
                "G-" | "e-" => &['b', 'e', 'a', 'd', 'g', 'c'],
                "C-" | "a-" => &['b', 'e', 'a', 'd', 'g', 'c', 'f'],
                _ => return None,
            };
            let mut sig = default_key_sig();
            for &l in flats {
                sig.set(l, -1);
            }
            return Some(sig);
        }
    };
    let mut sig = default_key_sig();
    for &l in sharps {
        sig.set(l, 1);
    }
    Some(sig)
}

/// Apply the contents of a single `_{...}` block on top of `current`.
///
/// If `content` is a scale name, returns a fresh signature (full reset).
/// Otherwise treats `content` as a sequence of `+`/`-`/`=` followed by
/// note letters.
///
/// Port of `parseKeySig` in `web-ctrmml/src/mml/key-sig.ts`.
pub fn parse_key_sig(content: &str, current: &KeySig) -> KeySig {
    if let Some(scale) = scale_for_name(content) {
        return scale;
    }
    let first = match content.chars().next() {
        Some(c) => c,
        None => return current.clone(),
    };
    if first != '+' && first != '-' && first != '=' {
        return current.clone();
    }
    let mut result = current.clone();
    let mut modifier: i8 = 0;
    for ch in content.chars() {
        match ch {
            '+' => modifier = 1,
            '-' => modifier = -1,
            '=' => modifier = 0,
            other => {
                // Only the 7 naturals are valid targets — matches the TS
                // `if (result[note] !== undefined)` guard, since the JS
                // default sig only has those keys.
                let lower = other.to_ascii_lowercase();
                if matches!(lower, 'a'..='g') {
                    result.set(lower, modifier);
                }
            }
        }
    }
    result
}

/// Walk backward from `(line_number, column)` to determine the effective key
/// signature at that position. Stops at the most recent scale-name reset,
/// at the leading track selector of the current track, or at the start of
/// file. `column` is 1-based (Monaco convention); characters at
/// `column - 1` and later on the starting line are treated as "after the
/// cursor" and ignored.
///
/// Port of `scanKeySigAt` in `web-ctrmml/src/mml/key-sig.ts`.
pub fn scan_key_sig_at(model: &dyn LineReader, line_number: u32, column: u32) -> KeySig {
    // Collected newest-first across the document; we replay them in
    // document-forward order at the end.
    let mut commands: Vec<String> = Vec::new();
    let mut done = false;

    let mut ln = line_number;
    loop {
        let line_text = model.get_line_content(ln);
        let end_raw = if ln == line_number {
            (column.saturating_sub(1) as usize).min(line_text.len())
        } else {
            line_text.len()
        };
        // Strip comments: only scan up to the first ';'.
        let end = match line_text.as_bytes()[..end_raw].iter().position(|&b| b == b';') {
            Some(p) => p,
            None => end_raw,
        };
        let segment = &line_text[..end];

        if !done {
            let line_blocks = collect_keysig_blocks(segment);
            // Push newest-first across the document: on each line, reverse
            // the per-line forward order so the most recent block lands
            // first.
            for content in line_blocks.into_iter().rev() {
                let first = content.chars().next();
                let is_scale_reset = matches!(first, Some(c) if c != '+' && c != '-' && c != '=');
                commands.push(content);
                if is_scale_reset {
                    done = true;
                    break;
                }
            }
        }

        if done {
            break;
        }
        // Leading track selector marks a track boundary; earlier lines
        // belong to other tracks (or the file header).
        if parse_leading_track_selector(line_text).is_some() {
            break;
        }
        if ln == 1 {
            break;
        }
        ln -= 1;
    }

    // Apply in document-forward (chronological) order.
    let mut key_sig = default_key_sig();
    for content in commands.iter().rev() {
        key_sig = parse_key_sig(content, &key_sig);
    }
    key_sig
}

/// Extract every `_{...}` block's inner contents from `segment`, in
/// forward order. Mirrors the TS regex `/_{([^}]*)}/g`.
fn collect_keysig_blocks(segment: &str) -> Vec<String> {
    let bytes = segment.as_bytes();
    let mut out: Vec<String> = Vec::new();
    let mut i = 0usize;
    while i + 1 < bytes.len() {
        if bytes[i] == b'_' && bytes[i + 1] == b'{' {
            let inner_start = i + 2;
            if let Some(rel) = bytes[inner_start..].iter().position(|&b| b == b'}') {
                let inner_end = inner_start + rel;
                // Safe: inner_start and inner_end are both byte indices
                // returned from ASCII-only searches on `segment`.
                out.push(segment[inner_start..inner_end].to_string());
                i = inner_end + 1;
                continue;
            }
            break;
        }
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::string_model::LinesModel;

    // ---------- scale_for_name -------------------------------------------------

    #[test]
    fn c_major_has_no_accidentals() {
        let sig = scale_for_name("C").unwrap();
        for letter in ['c', 'd', 'e', 'f', 'g', 'a', 'b'] {
            assert_eq!(sig.get(letter), Some(0), "letter {letter}");
        }
    }

    #[test]
    fn g_major_sharpens_f() {
        let sig = scale_for_name("G").unwrap();
        assert_eq!(sig.get_or_zero('f'), 1);
        assert_eq!(sig.get_or_zero('c'), 0);
    }

    #[test]
    fn f_major_flats_b() {
        let sig = scale_for_name("F").unwrap();
        assert_eq!(sig.get_or_zero('b'), -1);
        assert_eq!(sig.get_or_zero('f'), 0);
    }

    #[test]
    fn b_flat_major_flats_b_and_e() {
        let sig = scale_for_name("B-").unwrap();
        assert_eq!(sig.get_or_zero('b'), -1);
        assert_eq!(sig.get_or_zero('e'), -1);
        assert_eq!(sig.get_or_zero('a'), 0);
    }

    #[test]
    fn relative_minor_matches_major() {
        // a minor == C major
        let major = scale_for_name("C").unwrap();
        let minor = scale_for_name("a").unwrap();
        assert_eq!(major, minor);
        // e minor == G major
        assert_eq!(scale_for_name("e").unwrap(), scale_for_name("G").unwrap());
        // d minor == F major
        assert_eq!(scale_for_name("d").unwrap(), scale_for_name("F").unwrap());
    }

    #[test]
    fn all_seven_sharps() {
        let sig = scale_for_name("C+").unwrap();
        for letter in ['f', 'c', 'g', 'd', 'a', 'e', 'b'] {
            assert_eq!(sig.get_or_zero(letter), 1, "letter {letter}");
        }
    }

    #[test]
    fn all_seven_flats() {
        let sig = scale_for_name("C-").unwrap();
        for letter in ['b', 'e', 'a', 'd', 'g', 'c', 'f'] {
            assert_eq!(sig.get_or_zero(letter), -1, "letter {letter}");
        }
    }

    #[test]
    fn unknown_scale_name_returns_none() {
        assert!(scale_for_name("xyz").is_none());
        assert!(scale_for_name("").is_none());
    }

    // ---------- parse_key_sig --------------------------------------------------

    #[test]
    fn parse_scale_name_resets_signature() {
        // Start with all sharpened, parse "C" → reset to no accidentals.
        let prior = scale_for_name("C+").unwrap();
        let after = parse_key_sig("C", &prior);
        for letter in ['c', 'd', 'e', 'f', 'g', 'a', 'b'] {
            assert_eq!(after.get_or_zero(letter), 0);
        }
    }

    #[test]
    fn parse_plus_sharpens_listed_letters() {
        let after = parse_key_sig("+cfg", &default_key_sig());
        assert_eq!(after.get_or_zero('c'), 1);
        assert_eq!(after.get_or_zero('f'), 1);
        assert_eq!(after.get_or_zero('g'), 1);
        assert_eq!(after.get_or_zero('d'), 0);
    }

    #[test]
    fn parse_minus_flats_listed_letters() {
        let after = parse_key_sig("-b", &default_key_sig());
        assert_eq!(after.get_or_zero('b'), -1);
    }

    #[test]
    fn parse_equals_naturalises() {
        let prior = scale_for_name("G").unwrap();
        let after = parse_key_sig("=f", &prior);
        assert_eq!(after.get_or_zero('f'), 0);
    }

    #[test]
    fn parse_modifier_stacks_on_current() {
        // Start with G major (f#), add flat b.
        let prior = scale_for_name("G").unwrap();
        let after = parse_key_sig("-b", &prior);
        assert_eq!(after.get_or_zero('f'), 1);
        assert_eq!(after.get_or_zero('b'), -1);
    }

    #[test]
    fn parse_modifier_switches_within_string() {
        // "+c-b" → c#, b-flat.
        let after = parse_key_sig("+c-b", &default_key_sig());
        assert_eq!(after.get_or_zero('c'), 1);
        assert_eq!(after.get_or_zero('b'), -1);
    }

    #[test]
    fn parse_empty_returns_current_clone() {
        let prior = scale_for_name("G").unwrap();
        let after = parse_key_sig("", &prior);
        assert_eq!(after, prior);
    }

    #[test]
    fn parse_invalid_no_leading_modifier_returns_current() {
        // "xyz" is neither a scale nor a modifier expression.
        let prior = scale_for_name("G").unwrap();
        let after = parse_key_sig("xyz", &prior);
        assert_eq!(after, prior);
    }

    #[test]
    fn parse_modifier_ignores_non_natural_letters() {
        // 'h' / 'z' aren't in the c..b table.
        let after = parse_key_sig("+ch", &default_key_sig());
        assert_eq!(after.get_or_zero('c'), 1);
        // No way to verify 'h' isn't set since it's outside the table —
        // but the operation must not panic.
    }

    // ---------- scan_key_sig_at -----------------------------------------------

    #[test]
    fn scan_finds_keysig_on_current_line_before_cursor() {
        // Cursor at column 12 (after `_{+c} `).
        let model = LinesModel(vec!["A _{+c} cdefg".into()]);
        let sig = scan_key_sig_at(&model, 1, 12);
        assert_eq!(sig.get_or_zero('c'), 1);
    }

    #[test]
    fn scan_ignores_keysig_after_cursor() {
        // Cursor at column 3 (right after "A "). The _{+c} comes after.
        let model = LinesModel(vec!["A _{+c} cdefg".into()]);
        let sig = scan_key_sig_at(&model, 1, 3);
        assert_eq!(sig.get_or_zero('c'), 0);
    }

    #[test]
    fn scan_scale_name_blocks_earlier_modifiers() {
        // Earlier line sets +c; later block resets to F major. Effective at
        // end is F major (no c-sharp).
        let model = LinesModel(vec![
            "A _{+c} cdefg".into(),
            "  _{F} cdefg".into(),
        ]);
        let sig = scan_key_sig_at(&model, 2, 20);
        assert_eq!(sig.get_or_zero('c'), 0);
        assert_eq!(sig.get_or_zero('b'), -1);
    }

    #[test]
    fn scan_modifiers_stack_across_lines() {
        let model = LinesModel(vec![
            "A _{+c} cdefg".into(),
            "  _{-b} cdefg".into(),
        ]);
        let sig = scan_key_sig_at(&model, 2, 20);
        assert_eq!(sig.get_or_zero('c'), 1);
        assert_eq!(sig.get_or_zero('b'), -1);
    }

    #[test]
    fn scan_stops_at_track_boundary() {
        // The +c is in a different track; we shouldn't pick it up.
        let model = LinesModel(vec![
            "A _{+c} cdefg".into(),
            "B cdefg".into(),
        ]);
        let sig = scan_key_sig_at(&model, 2, 8);
        assert_eq!(sig.get_or_zero('c'), 0);
    }

    #[test]
    fn scan_ignores_keysig_in_comment() {
        let model = LinesModel(vec!["A cdefg ; _{+c}".into()]);
        let sig = scan_key_sig_at(&model, 1, 16);
        assert_eq!(sig.get_or_zero('c'), 0);
    }

    #[test]
    fn scan_picks_latest_block_on_a_line() {
        let model = LinesModel(vec!["A _{+c} cd _{-b} ef".into()]);
        let sig = scan_key_sig_at(&model, 1, 20);
        assert_eq!(sig.get_or_zero('c'), 1);
        assert_eq!(sig.get_or_zero('b'), -1);
    }

    #[test]
    fn scan_default_when_no_blocks() {
        let model = LinesModel(vec!["A cdefg".into()]);
        let sig = scan_key_sig_at(&model, 1, 8);
        for letter in ['c', 'd', 'e', 'f', 'g', 'a', 'b'] {
            assert_eq!(sig.get_or_zero(letter), 0);
        }
    }

    // ---------- collect_keysig_blocks -----------------------------------------

    #[test]
    fn collect_finds_multiple_blocks() {
        assert_eq!(
            collect_keysig_blocks("A _{+c} cd _{-b} ef"),
            vec!["+c".to_string(), "-b".to_string()]
        );
    }

    #[test]
    fn collect_handles_unterminated_block() {
        // No closing `}` → bail out.
        assert!(collect_keysig_blocks("_{+c no close").is_empty());
    }

    #[test]
    fn collect_empty_content_block() {
        assert_eq!(collect_keysig_blocks("_{}"), vec![String::new()]);
    }
}
