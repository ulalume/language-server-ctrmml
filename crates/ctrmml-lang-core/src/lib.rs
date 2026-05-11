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

pub mod beat_fill;
pub mod chord;
pub mod key_sig;
pub mod octave_scan;
pub mod text_scan;
pub mod timesig;
pub mod track_selector;
pub mod transpose;

#[cfg(test)]
mod test_util;

pub use chord::{
    accidental_char, chord_natural_semitones, render_chord, render_generic_chord,
    render_stacked_chord, render_stacked_generic_chord, ChordDef, ChordSize, RootAccidental,
    CHORDS_3, CHORDS_4, CHORD_LETTERS,
};
pub use key_sig::{default_key_sig, parse_key_sig, scale_for_name, scan_key_sig_at, KeySig};
pub use octave_scan::{scan_channel_context_at, ChannelContext};
pub use beat_fill::{
    generate_measure_rests, is_after_bar_line, measure_remainder_ticks, ticks_to_mml_rest,
};
pub use timesig::{
    parse_time_signature, scan_time_signature, TimeSignature, DEFAULT_TIME_SIGNATURE,
};
pub use text_scan::{is_in_comment, is_in_key_sig};
pub use transpose::{transpose_selection, Direction, Selection, TransposeEdit};
pub use track_selector::{
    find_enclosing_track_selector, find_enclosing_track_selector_at,
    parse_leading_track_selector, LeadingTrackSelector, LeadingTrackSpan, LineReader,
};
