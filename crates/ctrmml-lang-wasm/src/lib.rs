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

#![allow(clippy::too_many_arguments)]

use ctrmml_lang_core::{
    transpose::{transpose_selection, Direction, Selection},
    LinesModel,
};
use wasm_bindgen::prelude::*;

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
/// semitones (`+1` or `-1`). Returns `null` (JS undefined) when the
/// selection contains no notes or when the rewritten text would match
/// the original.
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
    let model = LinesModel::from_text(doc_text);
    let dir = if direction >= 0 {
        Direction::Up
    } else {
        Direction::Down
    };
    let sel = Selection {
        start_line_number: start_line,
        start_column,
        end_line_number: end_line,
        end_column,
    };
    let edit = transpose_selection(&model, sel, dir)?;
    Some(TransposeResult {
        start_line: edit.start_line_number,
        start_column: edit.start_column,
        end_line: edit.end_line_number,
        end_column: edit.end_column,
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

/// True when the text before a 1-based `column` ends (ignoring trailing
/// whitespace) with a `|` bar-line marker. Replaces `isAfterBarLine`
/// from `web-ctrmml/src/mml/beat-fill.ts`.
#[wasm_bindgen]
pub fn is_after_bar_line(line_content: &str, column: u32) -> bool {
    ctrmml_lang_core::is_after_bar_line(line_content, column)
}

// ---------------------------------------------------------------------------
// Text-scan helpers — replace web-ctrmml/src/editor/mml-docs.ts's
// isInComment / isInKeySig
// ---------------------------------------------------------------------------

/// Returns `true` when the 0-based column falls inside a `;` line
/// comment, respecting `"..."` and `'...'` string contexts.
#[wasm_bindgen]
pub fn is_in_comment(line: &str, col: u32) -> bool {
    ctrmml_lang_core::is_in_comment(line, col as usize)
}

/// Returns `true` when the 0-based column falls inside a `_{...}` key
/// signature block.
#[wasm_bindgen]
pub fn is_in_key_sig(line: &str, col: u32) -> bool {
    ctrmml_lang_core::is_in_key_sig(line, col as usize)
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

    #[test]
    fn is_in_comment_after_semicolon() {
        assert!(is_in_comment("abc ; comment", 7));
    }

    #[test]
    fn is_in_key_sig_inside_underscore_block() {
        assert!(is_in_key_sig("A _{+c} cd", 5));
    }
}
