//! Minimal `KeySig` type required by the chord module.
//!
//! This is the bare surface needed by `chord.rs`. The full key-signature
//! scanning / line-reader logic (port of `web-ctrmml/src/mml/key-sig.ts`)
//! will land in a follow-up commit.

/// Accidental offset (in semitones) per natural letter (`'c'`..=`'b'`).
///
/// Mirrors the TypeScript `KeySig = Record<string, number>` representation,
/// but constrained to the seven natural letters. A missing entry means
/// "natural" (zero offset).
///
/// `i8` is sufficient: accidentals are in `[-2, +2]` in practice.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct KeySig {
    // Indexed by (letter - 'a'); only c/d/e/f/g/a/b are meaningful.
    offsets: [Option<i8>; 7],
}

impl KeySig {
    /// Empty key signature (all letters natural, no explicit assignment).
    #[inline]
    pub const fn new() -> Self {
        Self {
            offsets: [None; 7],
        }
    }

    /// Look up the explicit accidental for `letter` (`'a'`..=`'g'`).
    ///
    /// Returns `None` when no entry is set — the caller decides whether to
    /// treat this as natural (`0`) or as "no override". The TypeScript code
    /// uses the `?? 0` pattern at every call site, mirrored here by
    /// [`KeySig::get_or_zero`].
    #[inline]
    pub fn get(&self, letter: char) -> Option<i8> {
        Self::index(letter).and_then(|i| self.offsets[i])
    }

    /// Look up the accidental, defaulting to `0` (natural) when unset.
    ///
    /// Matches the TS pattern `keySig[letter] ?? 0`.
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

    /// Builder-style setter for `const`-free construction in tests.
    #[inline]
    pub fn with(mut self, letter: char, acc: i8) -> Self {
        self.set(letter, acc);
        self
    }

    #[inline]
    fn index(letter: char) -> Option<usize> {
        match letter {
            'a'..='g' => Some((letter as u8 - b'a') as usize),
            _ => None,
        }
    }
}
