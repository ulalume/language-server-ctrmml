//! wasm-bindgen bridge exposing `ctrmml-lang-core` to JavaScript /
//! TypeScript callers.
//!
//! This is the Phase 4 deliverable from `web-ctrmml/RUST_PORT_PLAN.md`.
//! The goal is **not** to mirror the entire core API verbatim — it's to
//! offer JS-friendly entry points that web-ctrmml's existing TS modules
//! can flip to one at a time, with primitive parameters and
//! `#[wasm_bindgen]` value types that JS consumes without manual
//! deserialisation.
//!
//! Each exported function is documented with the TS-side call site that
//! it replaces, so the migration in Phase 5 is a mechanical
//! file-by-file swap.

use ctrmml_lang_core::{
    byte_offset_to_utf16_character,
    chord::{
        render_chord as core_render_chord, render_generic_chord as core_render_generic_chord,
        render_generic_diatonic_dyad as core_render_generic_diatonic_dyad,
        render_stacked_chord as core_render_stacked_chord,
        render_stacked_generic_chord as core_render_stacked_generic_chord,
        render_stacked_generic_diatonic_dyad as core_render_stacked_generic_diatonic_dyad,
        ChordSize, RootAccidental, CHORDS_3, CHORDS_4,
    },
    track_selector::LineReader,
    transpose::{transpose_selection, Direction, Selection},
    utf16_character_to_byte_offset, BorrowedLinesModel, KeySig,
};
use wasm_bindgen::prelude::*;

// Columns cross this boundary in UTF-16 code units, as JavaScript strings
// and editors count them. The core counts UTF-8 bytes.

/// The byte column for a 1-based UTF-16 `column` of `line_text`, 1-based.
fn byte_column_1(line_text: &str, column: u32) -> u32 {
    utf16_character_to_byte_offset(line_text, column.saturating_sub(1)) as u32 + 1
}

/// The UTF-16 column for a 1-based byte `column` of `line_text`, 1-based.
fn utf16_column_1(line_text: &str, column: u32) -> u32 {
    byte_offset_to_utf16_character(line_text, column.saturating_sub(1) as usize) + 1
}

// ---------------------------------------------------------------------------
// Build-time hello: lets JS verify the bundle loaded.
// ---------------------------------------------------------------------------

/// Returns the crate version baked in at compile time. Useful for sanity
/// checks and for displaying in the editor's "about" panel.
#[wasm_bindgen]
pub fn version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

// ---------------------------------------------------------------------------
// Transpose — replaces web-ctrmml/src/mml/transpose.ts
// ---------------------------------------------------------------------------

/// JS-facing result type for [`transpose`]. All ranges are 1-based,
/// matching the existing TS API (`TransposeEdit` in transpose.ts).
#[wasm_bindgen]
pub struct TransposeResult {
    pub start_line: u32,
    pub start_column: u32,
    pub end_line: u32,
    pub end_column: u32,
    text: String,
}

#[wasm_bindgen]
impl TransposeResult {
    /// The new selection text — what the editor should replace the
    /// selected range with.
    #[wasm_bindgen(getter)]
    pub fn text(&self) -> String {
        self.text.clone()
    }
}

/// Transpose every note in the 1-based selection by `direction`
/// semitones (`+1` or `-1`). Columns are 1-based UTF-16, in and out.
/// Returns `null` (JS undefined) when the selection contains no notes or
/// when the rewritten text would match the original.
///
/// Replaces `transposeSelection` from `web-ctrmml/src/mml/transpose.ts`.
#[wasm_bindgen]
pub fn transpose(
    doc_text: &str,
    start_line: u32,
    start_column: u32,
    end_line: u32,
    end_column: u32,
    direction: i32,
) -> Option<TransposeResult> {
    let model = BorrowedLinesModel::from_text(doc_text);
    let dir = if direction >= 0 {
        Direction::Up
    } else {
        Direction::Down
    };
    let line = |number: u32| model.get_line_content(number);
    let sel = Selection {
        start_line_number: start_line,
        start_column: byte_column_1(line(start_line), start_column),
        end_line_number: end_line,
        end_column: byte_column_1(line(end_line), end_column),
    };
    let edit = transpose_selection(&model, sel, dir)?;
    Some(TransposeResult {
        start_line: edit.start_line_number,
        start_column: utf16_column_1(line(edit.start_line_number), edit.start_column),
        end_line: edit.end_line_number,
        end_column: utf16_column_1(line(edit.end_line_number), edit.end_column),
        text: edit.text,
    })
}

// ---------------------------------------------------------------------------
// Time signature — replaces web-ctrmml/src/mml/timesig.ts
// ---------------------------------------------------------------------------

/// JS-facing time signature. `numerator == 0` is reserved to mean
/// "disabled" (`#timesig no`) so the optional return value in TS maps
/// to a single struct here without a separate null channel.
#[wasm_bindgen]
pub struct TimeSignatureResult {
    pub numerator: u32,
    pub denominator: u32,
}

/// Scan MML text for the first `#timesig` line. Returns `null` (JS
/// undefined) when the document explicitly sets `#timesig no` —
/// measure lines are disabled. Returns the default 4/4 when no
/// `#timesig` line is present.
///
/// Replaces `scanTimeSignature` from `web-ctrmml/src/mml/timesig.ts`.
#[wasm_bindgen]
pub fn scan_time_signature(text: &str) -> Option<TimeSignatureResult> {
    let sig = ctrmml_lang_core::scan_time_signature(text)?;
    Some(TimeSignatureResult {
        numerator: sig.numerator,
        denominator: sig.denominator,
    })
}

// ---------------------------------------------------------------------------
// Beat-fill — replaces web-ctrmml/src/mml/beat-fill.ts
// ---------------------------------------------------------------------------

/// Generate MML rest notation to fill from `cursor_tick` to the next
/// bar line.
///
/// `numerator` / `denominator` describe the time signature; pass `4`
/// and `4` for the default. When `numerator == 0` the rest sequence is
/// always empty (matches the TS "measure lines disabled" branch).
///
/// `after_bar_line` controls behavior when the cursor sits exactly on
/// a measure boundary: `true` fills a full next measure, `false`
/// returns an empty string.
///
/// Replaces `generateMeasureRests` from `web-ctrmml/src/mml/beat-fill.ts`.
#[wasm_bindgen]
pub fn generate_measure_rests(
    cursor_tick: u32,
    ppqn: u32,
    numerator: u32,
    denominator: u32,
    after_bar_line: bool,
) -> String {
    if numerator == 0 || denominator == 0 {
        return String::new();
    }
    let ts = ctrmml_lang_core::TimeSignature {
        numerator,
        denominator,
    };
    ctrmml_lang_core::generate_measure_rests(cursor_tick, ppqn, Some(ts), after_bar_line)
}

/// True when the text before a 1-based UTF-16 `column` ends (ignoring
/// trailing whitespace) with a `|` bar-line marker.
#[wasm_bindgen]
pub fn is_after_bar_line(line_content: &str, column: u32) -> bool {
    ctrmml_lang_core::is_after_bar_line(line_content, byte_column_1(line_content, column))
}

// ---------------------------------------------------------------------------
// Chord rendering — replaces web-ctrmml/src/mml/chord.ts
// ---------------------------------------------------------------------------
//
// KeySig is passed as a 7-element `&[i8]` indexed `[c, d, e, f, g, a, b]`,
// where each element is the accidental offset in semitones (-1 / 0 / 1).
// On the TS side this is a fixed-shape Int8Array; the old object-keyed
// representation can be flattened with two trivial lines of mapping.
//
// `root_accidental` is encoded as a sentinel `i32` to sidestep the
// fact that wasm-bindgen marshals `Option<i32>` as a tagged JS value
// (workable but adds glue). Sentinel scheme:
//
//     -1 = flat (`-`)
//      0 = natural (`=`)
//      1 = sharp (`+`)
//   i32::MIN = none (use the ambient key-sig accidental)
//
// Anything else also collapses to "none".

const ROOT_ACCIDENTAL_NONE: i32 = i32::MIN;

fn decode_root_accidental(v: i32) -> Option<RootAccidental> {
    match v {
        -1 => Some(RootAccidental::Flat),
        0 => Some(RootAccidental::Natural),
        1 => Some(RootAccidental::Sharp),
        _ => None,
    }
}

fn key_sig_from_slice(arr: &[i8]) -> KeySig {
    let mut k = KeySig::new();
    for (i, &val) in arr.iter().enumerate().take(7) {
        let letter = b"cdefgab"[i] as char;
        k.set(letter, val);
    }
    k
}

/// Lookup a chord definition by `(suffix, size)`. The `"4th"` suffix
/// appears in both CHORDS_3 (quartal triad) and CHORDS_4 (quartal
/// 7th); the size discriminator picks the right one.
fn chord_def_by_suffix(suffix: &str, size: u32) -> Option<&'static ctrmml_lang_core::ChordDef> {
    let table: &[ctrmml_lang_core::ChordDef] = match size {
        3 => CHORDS_3,
        4 => CHORDS_4,
        _ => return None,
    };
    table.iter().find(|d| d.suffix == suffix)
}

fn chord_size_from_u32(size: u32) -> Option<ChordSize> {
    match size {
        3 => Some(ChordSize::Triad),
        4 => Some(ChordSize::Seventh),
        _ => None,
    }
}

/// Render a generic diatonic chord (every other letter, no chord-name
/// lookup). `size` must be 3 (triad) or 4 (seventh).
///
/// Replaces `renderGenericChord` from `web-ctrmml/src/mml/chord.ts`.
#[wasm_bindgen]
pub fn render_generic_chord(root: char, root_accidental: i32, size: u32) -> Option<String> {
    let size = chord_size_from_u32(size)?;
    let acc = decode_root_accidental(root_accidental);
    core_render_generic_chord(root, acc, size)
}

/// Render a named chord by its suffix (e.g. `""`, `"m"`, `"7"`, `"M7"`,
/// `"dim7"`, `"sus2"`, ...). `size` is `3` for a triad or `4` for a
/// seventh chord — required because the `"4th"` suffix exists in
/// both tables and can't be disambiguated by name alone.
///
/// Returns `null` for an unknown suffix, an invalid size, or an
/// invalid root.
///
/// `key_sig` is a 7-element `Int8Array` indexed `[c, d, e, f, g, a, b]`.
/// `root_accidental` is the sentinel scheme described in the module
/// docstring.
///
/// Replaces `renderChord` from `web-ctrmml/src/mml/chord.ts`.
#[wasm_bindgen]
pub fn render_chord(
    root: char,
    root_accidental: i32,
    suffix: &str,
    size: u32,
    key_sig: &[i8],
) -> Option<String> {
    let def = chord_def_by_suffix(suffix, size)?;
    let acc = decode_root_accidental(root_accidental);
    let ks = key_sig_from_slice(key_sig);
    core_render_chord(root, acc, def, &ks)
}

/// Render a named chord, stacked bottom-up. See [`render_chord`] for
/// the `size` discriminator semantics.
///
/// Replaces `renderStackedChord` from `web-ctrmml/src/mml/chord.ts`.
#[wasm_bindgen]
pub fn render_stacked_chord(
    root: char,
    root_accidental: i32,
    suffix: &str,
    size: u32,
    key_sig: &[i8],
    channel_octaves: &[i32],
    compensate: bool,
) -> Option<String> {
    let def = chord_def_by_suffix(suffix, size)?;
    let acc = decode_root_accidental(root_accidental);
    let ks = key_sig_from_slice(key_sig);
    core_render_stacked_chord(root, acc, def, &ks, channel_octaves, compensate)
}

/// Diatonic 2-note dyad: root letter + the letter `step` positions away.
/// `step` is 1..=6 for 2nd through 7th. Letters only; accidentals come
/// from the ambient key signature at playback.
#[wasm_bindgen]
pub fn render_generic_diatonic_dyad(root: char, root_accidental: i32, step: u32) -> Option<String> {
    let acc = decode_root_accidental(root_accidental);
    core_render_generic_diatonic_dyad(root, acc, step as i32)
}

/// Stacked variant of [`render_generic_diatonic_dyad`].
#[wasm_bindgen]
pub fn render_stacked_generic_diatonic_dyad(
    root: char,
    root_accidental: i32,
    step: u32,
    key_sig: &[i8],
    channel_octaves: &[i32],
    compensate: bool,
) -> Option<String> {
    let acc = decode_root_accidental(root_accidental);
    let ks = key_sig_from_slice(key_sig);
    core_render_stacked_generic_diatonic_dyad(
        root,
        acc,
        step as i32,
        &ks,
        channel_octaves,
        compensate,
    )
}

/// Diatonic chord by letter only (no forced interval table), stacked
/// bottom-up. Each tone's accidental comes from the ambient key
/// signature.
///
/// Replaces `renderStackedGenericChord` from `web-ctrmml/src/mml/chord.ts`.
#[wasm_bindgen]
pub fn render_stacked_generic_chord(
    root: char,
    root_accidental: i32,
    size: u32,
    key_sig: &[i8],
    channel_octaves: &[i32],
    compensate: bool,
) -> Option<String> {
    let size = chord_size_from_u32(size)?;
    let acc = decode_root_accidental(root_accidental);
    let ks = key_sig_from_slice(key_sig);
    core_render_stacked_generic_chord(root, acc, size, &ks, channel_octaves, compensate)
}

/// The sentinel value to pass for `root_accidental` when the user typed
/// no explicit `+`/`-`/`=` after the root letter. Exposed as a constant
/// getter so JS callers don't have to hard-code `i32::MIN` and can stay
/// in sync if the encoding changes.
#[wasm_bindgen]
pub fn root_accidental_none() -> i32 {
    ROOT_ACCIDENTAL_NONE
}

// ---------------------------------------------------------------------------
// Octave / brace scanner — replaces web-ctrmml/src/mml/octave-scan.ts
// ---------------------------------------------------------------------------

/// JS-facing result of [`scan_channel_context`]. Mirrors the TS
/// `ChannelContext` shape: per-channel octaves plus the active branch.
///
/// `active_channel` is `None` (JS undefined) when the cursor is outside
/// any `{...}` brace.
#[wasm_bindgen]
pub struct ChannelContextResult {
    /// Branch the cursor sits in (0-based), or undefined when outside
    /// any `{...}`.
    pub active_channel: Option<u32>,
    octaves: Vec<i32>,
}

#[wasm_bindgen]
impl ChannelContextResult {
    /// Per-channel octaves at the cursor, in branch order. Returns an
    /// `Int32Array` of length `num_channels` (clamped to ≥ 1).
    #[wasm_bindgen(getter)]
    pub fn octaves(&self) -> Vec<i32> {
        self.octaves.clone()
    }
}

/// Compute each channel's effective octave at `(line, column)` (1-based,
/// UTF-16 column) plus which `{...}` branch the cursor sits in.
///
/// `track_line` should be `0` when unknown — the function will walk
/// backward to locate the enclosing leading track selector. Pass a
/// nonzero value to skip that scan when the caller already knows it.
///
/// Replaces `scanChannelContextAt` from `web-ctrmml/src/mml/octave-scan.ts`.
#[wasm_bindgen]
pub fn scan_channel_context(
    doc_text: &str,
    line: u32,
    column: u32,
    num_channels: u32,
    track_line: u32,
) -> ChannelContextResult {
    let model = BorrowedLinesModel::from_text(doc_text);
    let track_line = if track_line == 0 {
        None
    } else {
        Some(track_line)
    };
    let column = byte_column_1(model.get_line_content(line), column);
    let ctx = ctrmml_lang_core::scan_channel_context_at(
        &model,
        line,
        column,
        num_channels as usize,
        track_line,
    );
    ChannelContextResult {
        active_channel: ctx.active_channel.map(|c| c as u32),
        octaves: ctx.octaves,
    }
}

// ---------------------------------------------------------------------------
// Key signature scanner — replaces web-ctrmml/src/mml/key-sig.ts's
// scanKeySigAt
// ---------------------------------------------------------------------------

/// Compute the effective key signature at `(line, column)` (1-based,
/// UTF-16 column) by walking backward to the most recent scale-reset or enclosing track-selector
/// boundary. Returns a 7-element `Int8Array` indexed
/// `[c, d, e, f, g, a, b]`, matching the parameter shape used by
/// [`render_chord`] and friends.
///
/// Replaces `scanKeySigAt` from `web-ctrmml/src/mml/key-sig.ts`.
#[wasm_bindgen]
pub fn scan_key_sig(doc_text: &str, line: u32, column: u32) -> Vec<i8> {
    let model = BorrowedLinesModel::from_text(doc_text);
    let column = byte_column_1(model.get_line_content(line), column);
    let sig = ctrmml_lang_core::scan_key_sig_at(&model, line, column);
    let letters = ['c', 'd', 'e', 'f', 'g', 'a', 'b'];
    letters.iter().map(|&l| sig.get_or_zero(l)).collect()
}

// ---------------------------------------------------------------------------
// PSG envelope parser / serializer — replaces web-ctrmml/src/mml/psg-parser.ts
// ---------------------------------------------------------------------------
//
// The PSG envelope is a structured value (instrument number, ordered
// nodes, sustain/loop indices, default length). Rather than splitting
// it across many getter functions or adding serde to the core crate,
// we hand back an opaque `PsgEnvelopeHandle` whose methods cover every
// field the TS PSG editor reads. The PSG editor panel can drive the
// handle directly; for round-trip serialisation it calls
// [`PsgEnvelopeHandle::serialize`].

/// Opaque handle wrapping a parsed PSG envelope. All field access goes
/// through the getter methods below.
#[wasm_bindgen]
pub struct PsgEnvelopeHandle {
    inner: ctrmml_lang_core::PsgEnvelope,
}

#[wasm_bindgen]
impl PsgEnvelopeHandle {
    #[wasm_bindgen(getter)]
    pub fn instrument_number(&self) -> u32 {
        self.inner.instrument_number
    }

    #[wasm_bindgen(getter)]
    pub fn default_length(&self) -> u32 {
        self.inner.default_length
    }

    /// `None` when the envelope has no `/` sustain marker.
    #[wasm_bindgen(getter)]
    pub fn sustain_pos(&self) -> Option<u32> {
        self.inner.sustain_pos.map(|p| p as u32)
    }

    /// `None` when the envelope has no `|` loop marker.
    #[wasm_bindgen(getter)]
    pub fn loop_pos(&self) -> Option<u32> {
        self.inner.loop_pos.map(|p| p as u32)
    }

    #[wasm_bindgen(getter)]
    pub fn node_count(&self) -> u32 {
        self.inner.nodes.len() as u32
    }

    /// Volume at envelope node `index` (0..=15). Returns `i32::MIN` when
    /// `index` is out of range — JS callers should bounds-check against
    /// [`PsgEnvelopeHandle::node_count`] first.
    pub fn node_value(&self, index: u32) -> i32 {
        self.inner
            .nodes
            .get(index as usize)
            .map(|n| n.value as i32)
            .unwrap_or(i32::MIN)
    }

    /// Slide target for envelope node `index`, or `None` when the node
    /// holds at `node_value`.
    pub fn node_target(&self, index: u32) -> Option<u32> {
        self.inner
            .nodes
            .get(index as usize)
            .and_then(|n| n.target.map(|v| v as u32))
    }

    /// Explicit length for envelope node `index`, or `None` to fall
    /// back to the envelope's `default_length`.
    pub fn node_length(&self, index: u32) -> Option<u32> {
        self.inner.nodes.get(index as usize).and_then(|n| n.length)
    }

    /// Effective duration in frames for envelope node `index`. Resolves
    /// the fallback chain (explicit > slide distance > default).
    pub fn node_effective_length(&self, index: u32) -> u32 {
        let default = self.inner.default_length;
        self.inner
            .nodes
            .get(index as usize)
            .map(|n| ctrmml_lang_core::node_effective_length(n, default))
            .unwrap_or(0)
    }

    /// Serialise the envelope back to a single-line MML definition
    /// using the supplied instrument number (which may differ from the
    /// one captured at parse time when the editor renumbers
    /// instruments).
    pub fn serialize(&self, instrument_number: u32) -> String {
        ctrmml_lang_core::serialize_psg_mml(&self.inner, instrument_number)
    }

    /// Total envelope duration in frames.
    pub fn total_duration(&self) -> u32 {
        ctrmml_lang_core::total_duration(&self.inner)
    }

    /// Frame offset where node `index` starts.
    pub fn node_start_frame(&self, index: u32) -> u32 {
        ctrmml_lang_core::node_start_frame(&self.inner, index as usize)
    }
}

/// Parse a `@N psg ...` MML definition. Returns `None` when no `psg`
/// keyword is found in `text`.
///
/// Replaces `parsePsgMml` from `web-ctrmml/src/mml/psg-parser.ts`.
#[wasm_bindgen]
pub fn parse_psg_mml(text: &str) -> Option<PsgEnvelopeHandle> {
    ctrmml_lang_core::parse_psg_mml(text).map(|env| PsgEnvelopeHandle { inner: env })
}

// ---------------------------------------------------------------------------
// Block finder — replaces web-ctrmml/src/mml/block-finder.ts
// ---------------------------------------------------------------------------

/// JS-facing instrument-block region. Mirrors the TS `InstrumentBlock`
/// interface (start_line / end_line / instrumentNumber).
#[wasm_bindgen]
pub struct InstrumentBlockResult {
    pub start_line: u32,
    pub end_line: u32,
    pub instrument_number: u32,
}

/// Find an FM instrument block (`@N fm`) enclosing `line` (1-based).
/// Returns `null` when the cursor sits outside any FM block.
///
/// Replaces `findFmBlockAt` from `web-ctrmml/src/mml/block-finder.ts`.
#[wasm_bindgen]
pub fn find_fm_block_at(doc_text: &str, line: u32) -> Option<InstrumentBlockResult> {
    let model = BorrowedLinesModel::from_text(doc_text);
    ctrmml_lang_core::find_fm_block_at(&model, line).map(|b| InstrumentBlockResult {
        start_line: b.start_line,
        end_line: b.end_line,
        instrument_number: b.instrument_number,
    })
}

/// Find a PSG instrument block (`@N psg`) enclosing `line`.
///
/// Replaces `findPsgBlockAt` from `web-ctrmml/src/mml/block-finder.ts`.
#[wasm_bindgen]
pub fn find_psg_block_at(doc_text: &str, line: u32) -> Option<InstrumentBlockResult> {
    let model = BorrowedLinesModel::from_text(doc_text);
    ctrmml_lang_core::find_psg_block_at(&model, line).map(|b| InstrumentBlockResult {
        start_line: b.start_line,
        end_line: b.end_line,
        instrument_number: b.instrument_number,
    })
}

// ---------------------------------------------------------------------------
// Track selector parsing — replaces web-ctrmml/src/mml/track-selector.ts
// ---------------------------------------------------------------------------

/// Number of selector spans on `line`'s leading track selector, or `0`
/// if the line doesn't begin with one. Lightweight alternative to
/// [`parse_leading_track_selector`] for callers that only need the
/// channel count.
#[wasm_bindgen]
pub fn leading_track_selector_channel_count(line: &str) -> u32 {
    ctrmml_lang_core::parse_leading_track_selector(line)
        .map(|s| s.spans.len() as u32)
        .unwrap_or(0)
}

/// Opaque handle exposing the full result of parsing a leading track
/// selector. Span data is delivered via index-based getters so JS
/// callers don't need a serde round trip.
#[wasm_bindgen]
pub struct LeadingTrackSelectorResult {
    end: u32,
    spans: Vec<ctrmml_lang_core::LeadingTrackSpan>,
}

#[wasm_bindgen]
impl LeadingTrackSelectorResult {
    /// UTF-16 offset just past the last span (before any trailing
    /// whitespace).
    #[wasm_bindgen(getter)]
    pub fn end(&self) -> u32 {
        self.end
    }

    /// Number of selector spans on the line.
    #[wasm_bindgen(getter)]
    pub fn span_count(&self) -> u32 {
        self.spans.len() as u32
    }

    /// Track id for span `index`. Returns `u32::MAX` when `index` is
    /// out of range — JS callers should bounds-check against
    /// [`Self::span_count`] first.
    pub fn span_track_id(&self, index: u32) -> u32 {
        self.spans
            .get(index as usize)
            .map(|s| s.track_id)
            .unwrap_or(u32::MAX)
    }

    /// UTF-16 start offset for span `index`.
    pub fn span_start(&self, index: u32) -> u32 {
        self.spans
            .get(index as usize)
            .map(|s| s.start as u32)
            .unwrap_or(u32::MAX)
    }

    /// UTF-16 end offset for span `index` (exclusive).
    pub fn span_end(&self, index: u32) -> u32 {
        self.spans
            .get(index as usize)
            .map(|s| s.end as u32)
            .unwrap_or(u32::MAX)
    }
}

/// Parse a ctrmml leading track selector at the start of `line`.
/// Returns `null` when the line doesn't begin with a valid selector.
///
/// Replaces `parseLeadingTrackSelector` from
/// `web-ctrmml/src/mml/track-selector.ts`.
#[wasm_bindgen]
pub fn parse_leading_track_selector(line: &str) -> Option<LeadingTrackSelectorResult> {
    let utf16 = |byte: usize| byte_offset_to_utf16_character(line, byte) as usize;
    ctrmml_lang_core::parse_leading_track_selector(line).map(|sel| LeadingTrackSelectorResult {
        end: utf16(sel.end) as u32,
        spans: sel
            .spans
            .into_iter()
            .map(|span| ctrmml_lang_core::LeadingTrackSpan {
                start: utf16(span.start),
                end: utf16(span.end),
                ..span
            })
            .collect(),
    })
}

/// Walk backward from `line` to find the nearest line beginning with a
/// leading track selector. Returns `0` if no selector exists at or
/// above `line`.
///
/// Replaces `findEnclosingTrackSelector` from
/// `web-ctrmml/src/mml/track-selector.ts` (when the caller only wants
/// the line number rather than the full selector span list).
#[wasm_bindgen]
pub fn find_track_selector_line(doc_text: &str, line: u32) -> u32 {
    let model = BorrowedLinesModel::from_text(doc_text);
    ctrmml_lang_core::find_enclosing_track_selector_at(&model, line)
        .map(|(_sel, ln)| ln)
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// Text-scan helpers — replace web-ctrmml/src/editor/mml-docs.ts's
// isInComment / isInKeySig
// ---------------------------------------------------------------------------

/// Returns `true` when the 0-based UTF-16 column falls inside a `;` line
/// comment, respecting `"..."` and `'...'` string contexts.
#[wasm_bindgen]
pub fn is_in_comment(line: &str, col: u32) -> bool {
    ctrmml_lang_core::is_in_comment(line, utf16_character_to_byte_offset(line, col))
}

/// Returns `true` when the 0-based UTF-16 column falls inside a `_{...}`
/// key signature block.
#[wasm_bindgen]
pub fn is_in_key_sig(line: &str, col: u32) -> bool {
    ctrmml_lang_core::is_in_key_sig(line, utf16_character_to_byte_offset(line, col))
}

/// The 0-based UTF-16 column of 0-based byte column `byte_col` of `line`,
/// for columns that `ctrmml-cmd` reports in bytes.
#[wasm_bindgen]
pub fn utf16_column(line: &str, byte_col: u32) -> u32 {
    byte_offset_to_utf16_character(line, byte_col as usize)
}

/// The 0-based byte column of 0-based UTF-16 column `utf16_col` of `line`,
/// for `ctrmml-cmd` calls that take byte columns.
#[wasm_bindgen]
pub fn byte_column(line: &str, utf16_col: u32) -> u32 {
    utf16_character_to_byte_offset(line, utf16_col) as u32
}

/// Return all completion / hover documentation tables as a single JSON
/// blob. Entries have the shape `{ key, label, insert, detail, doc }`
/// (string fields, empty when not applicable). See
/// `ctrmml_lang_core::docs::AllDocs` for the full field list.
#[wasm_bindgen]
pub fn docs_json() -> String {
    serde_json::to_string(&ctrmml_lang_core::docs::all_docs())
        .expect("docs::all_docs serializes to JSON")
}

/// Resolve a hover at `(line, col)` (zero-based, `col` in UTF-16 code
/// units) in `text`. Returns a JSON object `{ markdown, line, start, end }`
/// with the range in UTF-16 code units, or the literal `"null"` when the
/// cursor isn't on a documented construct.
#[wasm_bindgen]
pub fn hover_at_json(text: &str, line: u32, col: u32) -> String {
    serde_json::to_string(&ctrmml_lang_core::hover_at_utf16(text, line, col))
        .expect("hover info serializes to JSON")
}

/// Enumerate code-lens entries for `text` as a JSON array. Each entry is
/// `{ line, title, command_id, arguments }` where `command_id` is `null`
/// for informational labels.
#[wasm_bindgen]
pub fn code_lens_json(text: &str) -> String {
    serde_json::to_string(&ctrmml_lang_core::code_lens(text))
        .expect("code lens entries serialize to JSON")
}

// ---------------------------------------------------------------------------
// Completion — stateless two-call adapter for JavaScript / TypeScript
// ---------------------------------------------------------------------------

#[derive(serde::Serialize)]
struct JsonError<'a> {
    error: &'a str,
}

fn json_error(message: &str) -> String {
    serde_json::to_string(&JsonError { error: message })
        .expect("completion error message serializes to JSON")
}

fn completion_trigger(trigger: &str) -> Option<char> {
    trigger.chars().next()
}

/// Run the first stage of completion at the zero-based UTF-16 position.
///
/// `trigger` is either a one-character string or `""` when completion was
/// invoked without a trigger character. `settings_json` is a serialized
/// [`ctrmml_lang_core::CompletionSettings`]; omitted fields use their defaults.
/// Returns a serialized [`ctrmml_lang_core::CompletionPlan`]. Malformed settings
/// JSON does not trap: it returns `{ "error": "..." }` instead.
#[wasm_bindgen]
pub fn completion_plan_json(
    text: &str,
    line: u32,
    character: u32,
    trigger: &str,
    settings_json: &str,
) -> String {
    let settings: ctrmml_lang_core::CompletionSettings = match serde_json::from_str(settings_json) {
        Ok(settings) => settings,
        Err(error) => return json_error(&format!("invalid settings_json: {error}")),
    };
    let plan = ctrmml_lang_core::completion_plan(
        text,
        ctrmml_lang_core::Pos { line, character },
        completion_trigger(trigger),
        &settings,
    );
    serde_json::to_string(&plan).expect("completion plan serializes to JSON")
}

/// Resolve a data-bearing completion at the zero-based UTF-16 position.
///
/// `trigger` and `settings_json` follow [`completion_plan_json`]. `data_json`
/// is a serialized [`ctrmml_lang_core::DataPayload`]. Returns a serialized
/// [`ctrmml_lang_core::CoreCompletionList`]. Malformed settings or payload JSON
/// does not trap: it returns `{ "error": "..." }` instead.
#[wasm_bindgen]
pub fn completion_resolve_json(
    text: &str,
    line: u32,
    character: u32,
    trigger: &str,
    settings_json: &str,
    data_json: &str,
) -> String {
    let settings: ctrmml_lang_core::CompletionSettings = match serde_json::from_str(settings_json) {
        Ok(settings) => settings,
        Err(error) => return json_error(&format!("invalid settings_json: {error}")),
    };
    let data: ctrmml_lang_core::DataPayload = match serde_json::from_str(data_json) {
        Ok(data) => data,
        Err(error) => return json_error(&format!("invalid data_json: {error}")),
    };
    let list = ctrmml_lang_core::completion_resolve(
        text,
        ctrmml_lang_core::Pos { line, character },
        completion_trigger(trigger),
        &settings,
        data,
    );
    serde_json::to_string(&list).expect("completion list serializes to JSON")
}

/// Extract the `@N <type>` instrument block that covers `anchor_line`
/// (0-based). `instrument_type` must be one of `"fm"` / `"psg"` /
/// `"pcm"`. Returns a JSON object `{ instrument_number, instrument_type,
/// mml_text, start_line, end_line }` or `"null"`.
#[wasm_bindgen]
pub fn extract_instrument_block_json(
    text: &str,
    anchor_line: u32,
    instrument_type: &str,
) -> String {
    let ty = match ctrmml_lang_core::InstrumentType::parse(instrument_type) {
        Some(ty) => ty,
        None => return "null".to_string(),
    };
    let block = ctrmml_lang_core::extract_instrument_block(text, anchor_line, ty);
    serde_json::to_string(&block).expect("block serializes to JSON")
}

/// Build a self-contained preview MML for `block` playing on `channel`.
/// `block_json` is the JSON shape returned by
/// `extract_instrument_block_json`.
#[wasm_bindgen]
pub fn build_preview_mml(text: &str, block_json: &str, channel: &str) -> String {
    let block: ctrmml_lang_core::ExtractedBlock = match serde_json::from_str(block_json) {
        Ok(b) => b,
        Err(_) => return String::new(),
    };
    ctrmml_lang_core::build_preview_mml(text, &block, channel)
}

/// Decide whether the keystroke at `(line, col)` (zero-based, UTF-16
/// column) typing `typed_char` should trigger a note preview. Returns a
/// JSON object `{ midi, pulse_start, pulse_end, tempo, length, quantize,
/// volume, noise_mode }` with UTF-16 pulse columns on hit, or `"null"`
/// when no preview should fire.
#[wasm_bindgen]
pub fn preview_note_at_json(text: &str, line: u32, col: u32, typed_char: &str) -> String {
    let Some(typed) = typed_char.chars().next() else {
        return "null".to_string();
    };
    let model = BorrowedLinesModel::from_text(text);
    let line_text = model.get_line_content(line.saturating_add(1));
    let byte_col = utf16_character_to_byte_offset(line_text, col) as u32;
    let hit = ctrmml_lang_core::preview_note_at(text, line, byte_col, typed).map(|mut hit| {
        hit.pulse_start = byte_offset_to_utf16_character(line_text, hit.pulse_start as usize);
        hit.pulse_end = byte_offset_to_utf16_character(line_text, hit.pulse_end as usize);
        hit
    });
    serde_json::to_string(&hit).expect("preview note serializes to JSON")
}

/// Place a `ctrmml-cmd check --json` report on `text`. Returns a JSON
/// array of `{ severity: "error" | "warning", message, code, range }`,
/// errors first, with `range` as `{ start: { line, character }, end }`
/// (0-based, UTF-16). Malformed JSON returns `{ "error": "..." }`.
#[wasm_bindgen]
pub fn check_report_to_diagnostics_json(text: &str, check_json: &str) -> String {
    let report: ctrmml_lang_core::CheckReport = match serde_json::from_str(check_json) {
        Ok(report) => report,
        Err(error) => return json_error(&format!("invalid check_json: {error}")),
    };
    serde_json::to_string(&ctrmml_lang_core::check_report_to_diagnostics(
        text, &report,
    ))
    .expect("diagnostics serialize to JSON")
}

/// One-character highlight ranges for playback positions, given as
/// parallel arrays of 0-based lines and 0-based byte columns. Returns a
/// flat `Uint32Array` of `[line, start, end, ...]` triples in UTF-16 units,
/// without repeats; positions past the last line are dropped.
#[wasm_bindgen]
pub fn playback_ranges(text: &str, lines: &[u32], cols: &[u32]) -> Vec<u32> {
    let positions: Vec<ctrmml_lang_core::PlaybackPosition> = lines
        .iter()
        .zip(cols)
        .map(|(&line, &col)| ctrmml_lang_core::PlaybackPosition { line, col })
        .collect();
    ctrmml_lang_core::playback_ranges(text, &positions)
        .into_iter()
        .flat_map(|range| [range.start.line, range.start.character, range.end.character])
        .collect()
}

// ---------------------------------------------------------------------------
// Native-target sanity tests
// ---------------------------------------------------------------------------
//
// `cargo test -p ctrmml-lang-wasm` runs on the host target; #[wasm_bindgen]
// attributes are no-ops there, so the wrapper functions are just plain
// Rust. The tests below verify that our parameter marshalling and
// re-exports are correct end-to-end.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_is_non_empty() {
        assert!(!version().is_empty());
    }

    #[test]
    fn hover_json_keeps_its_shape() {
        let value: serde_json::Value =
            serde_json::from_str(&hover_at_json("A @1 c", 0, 2)).expect("hover JSON");
        let object = value.as_object().expect("hover object");
        let mut keys: Vec<&str> = object.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(keys, ["end", "line", "markdown", "start"]);
        assert_eq!(
            (value["line"].as_u64(), value["start"].as_u64()),
            (Some(0), Some(2))
        );
        assert_eq!(hover_at_json("; c", 0, 2), "null");
    }

    #[test]
    fn transpose_up_round_trip() {
        // "A o4 [c] d" → "A o4 c+ d" (replicates a TS test from
        // transpose.test.ts, with the brackets stripped and the
        // selection computed manually).
        let doc = "A o4 c d";
        let result = transpose(doc, 1, 6, 1, 7, 1).unwrap();
        assert_eq!(result.text, "c+");
        assert_eq!(result.start_line, 1);
        assert_eq!(result.start_column, 6);
        assert_eq!(result.end_line, 1);
        assert_eq!(result.end_column, 7);
    }

    #[test]
    fn transpose_no_notes_returns_none() {
        let doc = "A o4 r4 d";
        assert!(transpose(doc, 1, 6, 1, 9, 1).is_none());
    }

    #[test]
    fn transpose_negative_direction_goes_down() {
        let doc = "A o4 c d";
        // c down crosses octave → "<b>"
        let result = transpose(doc, 1, 6, 1, 7, -1).unwrap();
        assert_eq!(result.text, "<b>");
    }

    #[test]
    fn time_signature_default() {
        let sig = scan_time_signature("A o4 cdefg").unwrap();
        assert_eq!(sig.numerator, 4);
        assert_eq!(sig.denominator, 4);
    }

    #[test]
    fn time_signature_explicit() {
        let sig = scan_time_signature("#timesig 3/4\nA c").unwrap();
        assert_eq!(sig.numerator, 3);
        assert_eq!(sig.denominator, 4);
    }

    #[test]
    fn time_signature_no_returns_null() {
        assert!(scan_time_signature("#timesig no\nA c").is_none());
    }

    #[test]
    fn generate_measure_rests_partial() {
        // ppqn=48, 4/4 → tpm=192. Cursor at tick 48 → 144 left → "r2."
        assert_eq!(generate_measure_rests(48, 48, 4, 4, false), "r2.");
    }

    #[test]
    fn generate_measure_rests_disabled_meter() {
        assert_eq!(generate_measure_rests(48, 48, 0, 4, true), "");
    }

    #[test]
    fn is_after_bar_line_yes() {
        assert!(is_after_bar_line("abc |", 6));
    }

    #[test]
    fn is_after_bar_line_no() {
        assert!(!is_after_bar_line("abc def", 8));
    }

    // ---- UTF-16 columns -----------------------------------------------------
    //
    // `あ` is one UTF-16 unit and three UTF-8 bytes.

    #[test]
    fn preview_note_after_japanese_text_reads_the_accidental() {
        let text = "A 'あ' o4 cd+";
        let hit: serde_json::Value =
            serde_json::from_str(&preview_note_at_json(text, 0, 11, "+")).expect("hit JSON");
        // o4 d+ is MIDI 39 (o5c is 48).
        assert_eq!(hit["midi"], 39);
        assert_eq!(
            (hit["pulse_start"].as_u64(), hit["pulse_end"].as_u64()),
            (Some(10), Some(12))
        );
    }

    #[test]
    fn bar_line_after_japanese_text_is_found() {
        assert!(is_after_bar_line("A 'あ' c4 |", 11));
        assert!(!is_after_bar_line("A 'あ' c4 |", 10));
    }

    #[test]
    fn check_error_after_japanese_text_is_placed_in_utf16_units() {
        let report = r#"{"ok":false,"errors":[{"message":"unterminated '[]' loop","path":"<stdin>","line":1,"col":11,"length":0,"code":"parse_error"}],"warnings":[]}"#;
        let value: serde_json::Value =
            serde_json::from_str(&check_report_to_diagnostics_json("A 'あ' c ] e\n", report))
                .expect("diagnostics JSON");
        assert_eq!(
            value,
            serde_json::json!([{
                "severity": "error",
                "message": "unterminated '[]' loop",
                "code": "parse_error",
                "range": {
                    "start": {"line": 0, "character": 8},
                    "end": {"line": 0, "character": 9}
                }
            }])
        );
        assert!(check_report_to_diagnostics_json("", "{").starts_with("{\"error\""));
    }

    #[test]
    fn playback_ranges_after_japanese_text_are_flat_utf16_triples() {
        assert_eq!(
            playback_ranges("A 'あ' c d\nB e", &[0, 0, 1, 5], &[8, 8, 2, 0]),
            [0, 6, 7, 1, 2, 3]
        );
    }

    #[test]
    fn column_conversions_round_trip() {
        assert_eq!(utf16_column("A 'あ' c", 8), 6);
        assert_eq!(byte_column("A 'あ' c", 6), 8);
    }

    #[test]
    fn comment_and_key_sig_columns_after_japanese_text() {
        // The `;` is UTF-16 column 6, byte column 8.
        assert!(!is_in_comment("A 'あ' ;c", 6));
        assert!(is_in_comment("A 'あ' ;c", 7));
        assert!(is_in_key_sig("A 'あ' _{+c} c", 9));
        assert!(!is_in_key_sig("A 'あ' _{+c} c", 12));
    }

    #[test]
    fn scanners_after_japanese_text_take_utf16_columns() {
        // UTF-16 column 13 (1-based) is just after `>`.
        let ctx = scan_channel_context("A 'あ' o4 > c", 1, 12, 1, 0);
        assert_eq!(ctx.octaves, vec![5]);
        let ks = scan_key_sig("A 'あ' _{+c} c", 1, 14);
        assert_eq!(ks[0], 1);
    }

    #[test]
    fn transpose_after_japanese_text_uses_utf16_columns() {
        let result = transpose("A 'あ' o4 cd", 1, 10, 1, 12, 1).expect("edit");
        assert_eq!(result.text, "c+d+");
        assert_eq!((result.start_column, result.end_column), (10, 12));
    }

    #[test]
    fn track_selector_spans_are_utf16_offsets() {
        let sel = parse_leading_track_selector("AB c").expect("selector");
        assert_eq!((sel.span_start(1), sel.span_end(1), sel.end()), (1, 2, 2));
    }

    #[test]
    fn is_in_comment_after_semicolon() {
        assert!(is_in_comment("abc ; comment", 7));
    }

    #[test]
    fn is_in_key_sig_inside_underscore_block() {
        assert!(is_in_key_sig("A _{+c} cd", 5));
    }

    // ---- chord rendering -----------------------------------------------------

    fn empty_key_sig() -> [i8; 7] {
        [0; 7]
    }

    #[test]
    fn generic_triad() {
        let none = root_accidental_none();
        assert_eq!(render_generic_chord('c', none, 3).as_deref(), Some("c/e/g"));
    }

    #[test]
    fn generic_seventh() {
        let none = root_accidental_none();
        assert_eq!(
            render_generic_chord('c', none, 4).as_deref(),
            Some("c/e/g/b")
        );
    }

    #[test]
    fn generic_rejects_invalid_size() {
        let none = root_accidental_none();
        assert!(render_generic_chord('c', none, 5).is_none());
    }

    #[test]
    fn named_minor_chord() {
        let none = root_accidental_none();
        let ks = empty_key_sig();
        assert_eq!(
            render_chord('c', none, "m", 3, &ks).as_deref(),
            Some("c/e-/g")
        );
    }

    #[test]
    fn named_chord_unknown_suffix_returns_null() {
        let none = root_accidental_none();
        let ks = empty_key_sig();
        assert!(render_chord('c', none, "nonsense", 3, &ks).is_none());
    }

    #[test]
    fn named_chord_invalid_size_returns_null() {
        let none = root_accidental_none();
        let ks = empty_key_sig();
        assert!(render_chord('c', none, "m", 5, &ks).is_none());
    }

    #[test]
    fn named_chord_4th_disambiguates_by_size() {
        // "4th" exists in both CHORDS_3 (triad) and CHORDS_4 (7th).
        let none = root_accidental_none();
        let ks = empty_key_sig();
        assert_eq!(
            render_chord('c', none, "4th", 3, &ks).as_deref(),
            Some("c/f/b-"),
            "triad form"
        );
        assert_eq!(
            render_chord('c', none, "4th", 4, &ks).as_deref(),
            Some("c/f/b-/e-"),
            "seventh form"
        );
    }

    #[test]
    fn named_chord_respects_key_sig() {
        let none = root_accidental_none();
        let mut ks = empty_key_sig();
        ks[2] = -1; // e flat (index 2 is 'e')
        assert_eq!(
            render_chord('c', none, "m", 3, &ks).as_deref(),
            Some("c/e/g")
        );
    }

    #[test]
    fn stacked_chord_lifts_third_tone() {
        let none = root_accidental_none();
        let ks = empty_key_sig();
        let chans = [4, 4, 4];
        assert_eq!(
            render_stacked_chord('f', none, "", 3, &ks, &chans, false).as_deref(),
            Some("f/a/>c")
        );
    }

    #[test]
    fn stacked_generic_full_chain() {
        // Replicates the "f diatonic triad wraps c above" TS test.
        let none = root_accidental_none();
        let ks = empty_key_sig();
        let chans = [4, 4, 4];
        assert_eq!(
            render_stacked_generic_chord('f', none, 3, &ks, &chans, false).as_deref(),
            Some("f/a/>c")
        );
    }

    #[test]
    fn root_accidental_sharp_decodes_correctly() {
        // Explicit `+` on the root letter — should preserve it.
        let ks = empty_key_sig();
        assert_eq!(
            render_chord('c', 1, "", 3, &ks).as_deref(),
            Some("c+/e+/g+")
        );
    }

    // ---- scanners ------------------------------------------------------------

    #[test]
    fn scan_channel_context_defaults_to_six() {
        // No track header, no `oN` → default octave 6 for every channel.
        let ctx = scan_channel_context("", 1, 1, 3, 0);
        assert_eq!(ctx.octaves, vec![6, 6, 6]);
        assert_eq!(ctx.active_channel, None);
    }

    #[test]
    fn scan_channel_context_picks_up_o_command() {
        let ctx = scan_channel_context("A o4 c", 1, 7, 3, 0);
        assert_eq!(ctx.octaves, vec![4, 4, 4]);
    }

    #[test]
    fn scan_channel_context_inside_brace() {
        // Cursor after the first `/` — active_channel should be 1.
        let ctx = scan_channel_context("A o4 {c/", 1, 9, 3, 0);
        assert_eq!(ctx.active_channel, Some(1));
    }

    #[test]
    fn scan_channel_context_honors_explicit_track_line() {
        let doc = "; header only\nABC o4\n";
        let ctx = scan_channel_context(doc, 3, 1, 3, 2);
        assert_eq!(ctx.octaves, vec![4, 4, 4]);
    }

    #[test]
    fn scan_key_sig_default_is_natural() {
        let ks = scan_key_sig("A o4 c", 1, 7);
        assert_eq!(ks, vec![0; 7]);
    }

    #[test]
    fn scan_key_sig_picks_up_modifier() {
        // `_{+c}` sharpens c at the cursor.
        let ks = scan_key_sig("A _{+c} c", 1, 9);
        // index 0 = c
        assert_eq!(ks[0], 1);
        // Others remain natural.
        assert_eq!(ks[1..], [0; 6]);
    }

    #[test]
    fn scan_key_sig_picks_up_scale_reset() {
        // F major flats b (index 6).
        let ks = scan_key_sig("A _{F} c", 1, 8);
        assert_eq!(ks[6], -1); // b flat
                               // c, d, e, f, g, a remain natural (F major only affects b).
        assert_eq!(&ks[0..6], &[0, 0, 0, 0, 0, 0]);
    }

    // ---- PSG parser ----------------------------------------------------------

    #[test]
    fn parse_psg_simple() {
        let env = parse_psg_mml("@5 psg 15 10 5 0").unwrap();
        assert_eq!(env.instrument_number(), 5);
        assert_eq!(env.node_count(), 4);
        assert_eq!(env.node_value(0), 15);
        assert_eq!(env.sustain_pos(), None);
        assert_eq!(env.loop_pos(), None);
        assert_eq!(env.default_length(), 1);
    }

    #[test]
    fn parse_psg_slide_and_length() {
        let env = parse_psg_mml("@1 psg 15>0:20").unwrap();
        assert_eq!(env.node_value(0), 15);
        assert_eq!(env.node_target(0), Some(0));
        assert_eq!(env.node_length(0), Some(20));
        assert_eq!(env.node_effective_length(0), 20);
    }

    #[test]
    fn parse_psg_slide_default_length() {
        let env = parse_psg_mml("@1 psg 5>10").unwrap();
        // |10 - 5| + 1 = 6
        assert_eq!(env.node_effective_length(0), 6);
    }

    #[test]
    fn parse_psg_with_markers() {
        let env = parse_psg_mml("@1 psg 15 | 10 / 0").unwrap();
        assert_eq!(env.loop_pos(), Some(1));
        assert_eq!(env.sustain_pos(), Some(2));
    }

    #[test]
    fn parse_psg_no_keyword_returns_null() {
        assert!(parse_psg_mml("@1 fm 1 1 1 1").is_none());
    }

    #[test]
    fn parse_psg_serialize_round_trip() {
        let env = parse_psg_mml("@7 psg 15>10:5 / 8 8 | 4 0").unwrap();
        let out = env.serialize(env.instrument_number());
        let again = parse_psg_mml(&out).unwrap();
        assert_eq!(again.instrument_number(), 7);
        assert_eq!(again.node_count(), env.node_count());
        assert_eq!(again.sustain_pos(), env.sustain_pos());
        assert_eq!(again.loop_pos(), env.loop_pos());
    }

    #[test]
    fn psg_out_of_range_node_returns_sentinel() {
        let env = parse_psg_mml("@1 psg 15").unwrap();
        assert_eq!(env.node_value(99), i32::MIN);
        assert_eq!(env.node_target(99), None);
        assert_eq!(env.node_length(99), None);
    }

    // ---- completion JSON adapter -------------------------------------------

    const FULL_COMPLETION_SETTINGS: &str = r#"{
        "arpeggio_enabled": false,
        "arpeggio_pattern": "up",
        "chord_stack_mode": "stack_up",
        "fm_picker_hierarchy": false
    }"#;

    #[test]
    fn completion_plan_done_round_trip() {
        let json = completion_plan_json("", 0, 0, "", FULL_COMPLETION_SETTINGS);
        let plan: ctrmml_lang_core::CompletionPlan = serde_json::from_str(&json).unwrap();
        let ctrmml_lang_core::CompletionPlan::Done(list) = plan else {
            panic!("command fallback should complete during planning");
        };
        assert!(list.is_incomplete);
        assert_eq!(list.items.len(), 37);
        assert_eq!(list.items[0].label, "cdefgabh");
    }

    #[test]
    fn completion_plan_needs_fm_patches_shape() {
        let json = completion_plan_json("@1 fm", 0, 5, "", FULL_COMPLETION_SETTINGS);
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(
            value,
            serde_json::json!({"needs": {"fm_patches": {"fragment": null}}})
        );
    }

    #[test]
    fn completion_resolve_fm_patches_round_trip() {
        let data = serde_json::json!({
            "fm_patches": [{
                "rel_path": "inst/bass.dmp",
                "name": "Bass",
                "mml": "@1 fm ; Bass\n; ALG  FB\n    4   7\n",
                "has_macros": false
            }]
        });
        let json = completion_resolve_json(
            "@1 fm ",
            0,
            6,
            " ",
            FULL_COMPLETION_SETTINGS,
            &data.to_string(),
        );
        let list: ctrmml_lang_core::CoreCompletionList = serde_json::from_str(&json).unwrap();
        assert!(!list.is_incomplete);
        assert_eq!(list.items.len(), 2);
        assert_eq!(list.items[0].label, "inst/bass.dmp — Bass");
        assert_eq!(list.items[1].label, "Default FM template");
    }

    #[test]
    fn completion_settings_allow_empty_partial_object() {
        let json = completion_plan_json("@1 fm", 0, 5, "", "{}");
        let plan: ctrmml_lang_core::CompletionPlan = serde_json::from_str(&json).unwrap();
        assert_eq!(
            plan,
            ctrmml_lang_core::CompletionPlan::NeedsData(ctrmml_lang_core::DataRequest::FmPatches {
                fragment: None
            })
        );
    }

    #[test]
    fn completion_settings_accept_camel_case_and_variant_aliases() {
        let settings = serde_json::json!({
            "arpeggioEnabled": true,
            "arpeggioPattern": "UpDown",
            "chordStackMode": "stack-up",
            "fmPickerHierarchy": false
        });
        let json = completion_plan_json("A c", 0, 3, "", &settings.to_string());
        let plan: ctrmml_lang_core::CompletionPlan = serde_json::from_str(&json).unwrap();

        let ctrmml_lang_core::CompletionPlan::Done(list) = plan else {
            panic!("arpeggio completion should resolve without host data");
        };
        assert!(!list.items.is_empty());
    }

    #[test]
    fn completion_malformed_json_returns_error_objects() {
        let settings_error = completion_plan_json("", 0, 0, "", "{");
        let settings_value: serde_json::Value = serde_json::from_str(&settings_error).unwrap();
        assert!(settings_value["error"]
            .as_str()
            .unwrap()
            .starts_with("invalid settings_json:"));

        let data_error = completion_resolve_json("@1 fm", 0, 5, "", FULL_COMPLETION_SETTINGS, "{");
        let data_value: serde_json::Value = serde_json::from_str(&data_error).unwrap();
        assert!(data_value["error"]
            .as_str()
            .unwrap()
            .starts_with("invalid data_json:"));
    }

    // ---- block finder + track selector helpers ------------------------------

    #[test]
    fn find_fm_block_simple() {
        let doc = "@1 fm\n\t31,0,12,7,0,28,0,0,5,0\n";
        let block = find_fm_block_at(doc, 2).unwrap();
        assert_eq!(block.start_line, 1);
        assert_eq!(block.instrument_number, 1);
    }

    #[test]
    fn find_fm_block_outside_returns_null() {
        let doc = "A o4 cdefg";
        assert!(find_fm_block_at(doc, 1).is_none());
    }

    #[test]
    fn find_psg_block_simple() {
        let doc = "@3 psg\n\t15>0:5";
        let block = find_psg_block_at(doc, 2).unwrap();
        assert_eq!(block.start_line, 1);
        assert_eq!(block.instrument_number, 3);
    }

    #[test]
    fn track_selector_channel_count() {
        assert_eq!(leading_track_selector_channel_count("ABC cdefg"), 3);
        assert_eq!(leading_track_selector_channel_count("@1 fm"), 0);
        assert_eq!(leading_track_selector_channel_count("; comment"), 0);
    }

    #[test]
    fn find_track_selector_line_locates_track() {
        let doc = "#title \"x\"\nABC cdefg\n  more notes";
        assert_eq!(find_track_selector_line(doc, 3), 2);
    }

    #[test]
    fn find_track_selector_line_returns_zero_when_none() {
        let doc = "; just comments\n@0 fm";
        assert_eq!(find_track_selector_line(doc, 2), 0);
    }

    #[test]
    fn parse_leading_track_selector_full() {
        let sel = parse_leading_track_selector("ABC cdefg").unwrap();
        assert_eq!(sel.span_count(), 3);
        assert_eq!(sel.span_track_id(0), 0); // A
        assert_eq!(sel.span_track_id(1), 1); // B
        assert_eq!(sel.span_track_id(2), 2); // C
        assert_eq!(sel.end(), 3);
    }

    #[test]
    fn parse_leading_track_selector_returns_null_on_non_selector() {
        assert!(parse_leading_track_selector("; comment").is_none());
        assert!(parse_leading_track_selector("@0 fm").is_none());
    }

    #[test]
    fn parse_leading_track_selector_star_explicit_track() {
        let sel = parse_leading_track_selector("*42 cdefg").unwrap();
        assert_eq!(sel.span_count(), 1);
        assert_eq!(sel.span_track_id(0), 42);
        assert_eq!(sel.span_start(0), 0);
        assert_eq!(sel.span_end(0), 3);
    }
}
