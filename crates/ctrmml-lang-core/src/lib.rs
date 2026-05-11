//! ctrmml-lang-core — pure MML language semantics.
//!
//! This crate is the shared semantic layer for ctrmml tooling. It is
//! deliberately I/O-free (no std::fs, std::net, std::process, std::thread)
//! so it can compile both natively (for the LSP binary) and to
//! wasm32-unknown-unknown (for the web editor).
//!
//! Ported from the TypeScript reference implementation in
//! `web-ctrmml/src/mml/`.

#![forbid(unsafe_code)]

pub mod chord;
pub mod key_sig;

pub use chord::{
    accidental_char, chord_natural_semitones, render_chord, render_generic_chord,
    render_stacked_chord, render_stacked_generic_chord, ChordDef, ChordSize, RootAccidental,
    CHORDS_3, CHORDS_4, CHORD_LETTERS,
};
pub use key_sig::KeySig;
