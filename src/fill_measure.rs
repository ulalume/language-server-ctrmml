//! Fill-measure code action — Phase 3.2.3.
//!
//! When the cursor sits directly after a `|` bar marker on a track
//! line, offer a code action that inserts one full measure of rests
//! followed by another `|`. This is the common workflow of "I just
//! finished a measure and want to start a new empty one".
//!
//! A more powerful fill-from-cursor variant would need the cursor's
//! compiled tick position (and the active ppqn) — neither is
//! available today without a subprocess call to `ctrmml-cmd`. The
//! restricted "right after `|`" trigger sidesteps that entirely: we
//! know we're on a measure boundary, so the rest length is exactly
//! one full measure.

use std::collections::HashMap;

use ctrmml_lang_core::{
    find_enclosing_track_selector, find_fm_block_at, find_psg_block_at,
    generate_measure_rests, is_after_bar_line, scan_time_signature,
    track_selector::LineReader, LinesModel, TimeSignature, DEFAULT_TIME_SIGNATURE,
};
use tower_lsp::lsp_types::{
    CodeAction, CodeActionKind, Position, Range, TextEdit, Url, WorkspaceEdit,
};

/// ctrmml's standard PPQN. The `#ppqn` meta command can override this,
/// but is rarely used in practice; if we ever need to honour an
/// override we can scan the document for it the same way
/// `scan_time_signature` walks the header.
const DEFAULT_PPQN: u32 = 48;

/// Build the fill-measure code action for `(line, character)` on
/// `doc_text`, or `None` when the trigger conditions aren't met.
///
/// Triggers only when **all** of the following hold:
///
/// - The cursor is at column ≥ 1 and the text before it ends
///   (ignoring trailing whitespace) with `|`.
/// - The cursor is inside an enclosing track selector.
/// - The cursor is **not** inside an `@N fm` / `@N psg` block.
/// - `scan_time_signature` returns `Some(...)` (i.e. measure lines
///   aren't disabled via `#timesig no`).
pub(crate) fn fill_measure_code_action(
    uri: &Url,
    doc_text: &str,
    line_zero_based: u32,
    character: u32,
) -> Option<CodeAction> {
    let line = line_zero_based + 1;
    let col = character + 1;
    let model = LinesModel::from_text(doc_text);

    if find_fm_block_at(&model, line).is_some() {
        return None;
    }
    if find_psg_block_at(&model, line).is_some() {
        return None;
    }
    if find_enclosing_track_selector(&model, line).is_none() {
        return None;
    }

    let line_content = model.get_line_content(line);
    if !is_after_bar_line(line_content, col) {
        return None;
    }

    let time_sig: TimeSignature = scan_time_signature(doc_text).unwrap_or(DEFAULT_TIME_SIGNATURE);
    // cursor_tick = 0 modulo the measure (the `|` placed us on a
    // boundary), after_bar_line = true asks for a full next measure.
    let rests = generate_measure_rests(0, DEFAULT_PPQN, Some(time_sig), true);
    if rests.is_empty() {
        return None;
    }
    // The conventional shape is "<rests> |" so the next bar marker is
    // already in place for the user.
    let insert_text = format!("{rests} |");

    let edit_position = Position {
        line: line_zero_based,
        character,
    };
    let text_edit = TextEdit {
        range: Range {
            start: edit_position,
            end: edit_position,
        },
        new_text: insert_text,
    };
    let mut changes: HashMap<Url, Vec<TextEdit>> = HashMap::new();
    changes.insert(uri.clone(), vec![text_edit]);

    Some(CodeAction {
        title: format!(
            "ctrmml: insert empty measure ({}/{})",
            time_sig.numerator, time_sig.denominator
        ),
        kind: Some(CodeActionKind::REFACTOR_REWRITE),
        edit: Some(WorkspaceEdit {
            changes: Some(changes),
            document_changes: None,
            change_annotations: None,
        }),
        ..CodeAction::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn uri() -> Url {
        Url::parse("file:///x.mml").unwrap()
    }

    #[test]
    fn fires_after_bar_line_on_track_line() {
        let doc = "A o4 cdef |\n";
        // cursor right after `|` (col 12, character 11 in 0-based)
        let action = fill_measure_code_action(&uri(), doc, 0, 11).expect("action expected");
        assert!(action.title.contains("4/4"));
        let workspace_edit = action.edit.expect("workspace edit");
        let edit = workspace_edit
            .changes
            .as_ref()
            .and_then(|m| m.values().next())
            .and_then(|edits| edits.first())
            .expect("text edit");
        assert_eq!(edit.new_text, "r1 |");
        assert_eq!(edit.range.start, edit.range.end);
    }

    #[test]
    fn honours_explicit_time_signature() {
        let doc = "#timesig 3/8\nA o4 cdef |\n";
        let action = fill_measure_code_action(&uri(), doc, 1, 11).expect("action expected");
        assert!(action.title.contains("3/8"));
        let edit = action
            .edit
            .unwrap()
            .changes
            .unwrap()
            .into_values()
            .next()
            .unwrap()
            .pop()
            .unwrap();
        // 3/8 at ppqn=48 → 72 ticks per measure → "r4."
        assert_eq!(edit.new_text, "r4. |");
    }

    #[test]
    fn suppressed_when_no_bar_marker_before_cursor() {
        let doc = "A o4 cdef\n";
        assert!(fill_measure_code_action(&uri(), doc, 0, 9).is_none());
    }

    #[test]
    fn suppressed_outside_track_selector() {
        let doc = "#title \"x\" |\n";
        assert!(fill_measure_code_action(&uri(), doc, 0, 12).is_none());
    }

    #[test]
    fn suppressed_inside_fm_block() {
        let doc = "A cdefg\n@1 fm\n\t31,0,12,7,0,28,0,0,5,0 |\n";
        assert!(fill_measure_code_action(&uri(), doc, 2, 25).is_none());
    }

    #[test]
    fn suppressed_when_timesig_no() {
        let doc = "#timesig no\nA cdef |\n";
        assert!(fill_measure_code_action(&uri(), doc, 1, 7).is_none());
    }
}
