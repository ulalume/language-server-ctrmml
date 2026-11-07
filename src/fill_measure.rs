//! Fill-measure code action.
//!
//! When the cursor sits on a track-content line, offer an "insert
//! rests up to the next bar line" code action. The rest sequence is
//! computed from the cursor's compiled playback tick (`ctrmml-cmd
//! find-cursor-tick`) and the song's PPQN, so partial-measure fill
//! works correctly — `o4 c4 d4 [cursor]` in 4/4 inserts the two
//! quarter rests needed to reach the next bar line.
//!
//! Offering the action only runs cheap checks; the subprocess runs when
//! its edit is needed.

use std::collections::HashMap;

use ctrmml_lang_core::{
    find_enclosing_track_selector, find_fm_block_at, find_psg_block_at, generate_measure_rests,
    is_after_bar_line, scan_time_signature, track_selector::LineReader,
    utf16_character_to_byte_offset, BorrowedLinesModel, CursorTickData, TimeSignature,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tower_lsp::lsp_types::{
    CodeAction, CodeActionKind, Position, Range, TextEdit, Url, WorkspaceEdit,
};

use crate::ctrmml_cmd::run_ctrmml_cmd;
use crate::utils::{line_at, uri_to_path};

/// JSON payload from `ctrmml-cmd find-cursor-tick`.
#[derive(Debug, Deserialize)]
struct CursorTickResponse {
    cursor_tick: i32,
    ppqn: u32,
}

/// Ask `ctrmml-cmd` for the compiled tick and PPQN at an LSP position.
/// Compilation failures and positions without an event are ordinary absence.
pub(crate) async fn fetch_cursor_tick(
    cmd_path: &str,
    uri: &Url,
    doc_text: &str,
    line_zero_based: u32,
    character: u32,
) -> Option<CursorTickData> {
    let path = uri_to_path(uri.as_ref());
    let col = utf16_character_to_byte_offset(line_at(doc_text, line_zero_based), character);
    let output = run_ctrmml_cmd(cmd_path, "find-cursor-tick", Some(doc_text), |cmd| {
        cmd.arg("find-cursor-tick").arg("--stdin");
        if let Some(p) = path.as_deref() {
            cmd.arg("--path").arg(p);
        }
        cmd.arg("--line")
            .arg(line_zero_based.to_string())
            .arg("--col")
            .arg(col.to_string());
    })
    .await
    .ok()?;

    if !output.status.success() {
        return None;
    }

    let response: CursorTickResponse = serde_json::from_slice(&output.stdout).ok()?;
    if response.cursor_tick < 0 {
        return None;
    }
    Some(CursorTickData {
        tick: response.cursor_tick as u32,
        ppqn: response.ppqn,
    })
}

/// Cheap pre-check: the time signature and whether the cursor follows a
/// bar line, when the position can take a fill.
///
/// `None` when:
/// - Cursor is inside an `@N fm` / `@N psg` block (digit data, no
///   rests applicable).
/// - Cursor is outside any enclosing track selector (file header /
///   meta region).
/// - `#timesig no` is set (measure lines explicitly disabled).
fn pre_check(
    doc_text: &str,
    line_zero_based: u32,
    character: u32,
) -> Option<(TimeSignature, bool)> {
    let line = line_zero_based + 1;
    let model = BorrowedLinesModel::from_text(doc_text);
    let col = utf16_character_to_byte_offset(model.get_line_content(line), character) as u32 + 1;

    if find_fm_block_at(&model, line).is_some() {
        return None;
    }
    if find_psg_block_at(&model, line).is_some() {
        return None;
    }
    find_enclosing_track_selector(&model, line)?;
    let time_sig = scan_time_signature(doc_text)?;

    let line_content = model.get_line_content(line);
    let after_bar = is_after_bar_line(line_content, col);
    Some((time_sig, after_bar))
}

/// The fill-measure action at an LSP position, still without its edit.
pub(crate) fn fill_measure_action(
    doc_text: &str,
    line_zero_based: u32,
    character: u32,
) -> Option<CodeAction> {
    let (time_sig, after_bar) = pre_check(doc_text, line_zero_based, character)?;
    let what = if after_bar {
        "insert empty measure"
    } else {
        "fill measure with rests"
    };
    Some(CodeAction {
        title: format!(
            "ctrmml: {what} ({}/{})",
            time_sig.numerator, time_sig.denominator
        ),
        kind: Some(CodeActionKind::REFACTOR_REWRITE),
        ..CodeAction::default()
    })
}

/// The edit inserting the rests up to the next bar line. Runs `ctrmml-cmd
/// find-cursor-tick`; `None` when the position cannot take a fill, the
/// song does not compile, or the cursor already sits on a bar line.
pub(crate) async fn fill_measure_edit(
    cmd_path: &str,
    uri: &Url,
    doc_text: &str,
    line_zero_based: u32,
    character: u32,
) -> Option<WorkspaceEdit> {
    let (time_sig, after_bar) = pre_check(doc_text, line_zero_based, character)?;
    let timing = fetch_cursor_tick(cmd_path, uri, doc_text, line_zero_based, character).await?;
    let rests = generate_measure_rests(timing.tick, timing.ppqn, Some(time_sig), after_bar);
    if rests.is_empty() {
        return None;
    }

    // Rests then a trailing `|`, so the next bar marker is in place.
    let position = Position {
        line: line_zero_based,
        character,
    };
    let text_edit = TextEdit {
        range: Range {
            start: position,
            end: position,
        },
        new_text: format!("{rests} |"),
    };
    let mut changes: HashMap<Url, Vec<TextEdit>> = HashMap::new();
    changes.insert(uri.clone(), vec![text_edit]);
    Some(WorkspaceEdit {
        changes: Some(changes),
        document_changes: None,
        change_annotations: None,
    })
}

/// Where a fill-measure action left for `codeAction/resolve` applies.
#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct FillMeasureTarget {
    pub(crate) uri: Url,
    pub(crate) line: u32,
    pub(crate) character: u32,
}

impl FillMeasureTarget {
    pub(crate) fn into_data(self) -> Value {
        json!({ FILL_MEASURE_DATA: self })
    }

    pub(crate) fn from_data(data: &Value) -> Option<Self> {
        serde_json::from_value(data.get(FILL_MEASURE_DATA)?.clone()).ok()
    }
}

const FILL_MEASURE_DATA: &str = "fillMeasure";

#[cfg(test)]
mod tests {
    use super::*;

    // Pre-check tests don't need the subprocess so they live here.
    // End-to-end tests with the subprocess require ctrmml-cmd on the
    // host and live in integration tests / manual QA — they aren't
    // hermetic enough to run on CI without the binary.

    #[test]
    fn pre_check_passes_on_track_line() {
        let doc = "A o4 cdef |\n";
        let (ts, after_bar) = pre_check(doc, 0, 11).expect("pre-check should pass");
        assert_eq!(ts.numerator, 4);
        assert_eq!(ts.denominator, 4);
        assert!(after_bar);
    }

    #[test]
    fn pre_check_passes_mid_measure() {
        let doc = "A o4 cdef\n";
        let (_, after_bar) = pre_check(doc, 0, 9).expect("pre-check should pass");
        assert!(!after_bar);
    }

    #[test]
    fn pre_check_rejects_outside_track() {
        let doc = "#title \"x\" |\n";
        assert!(pre_check(doc, 0, 12).is_none());
    }

    #[test]
    fn pre_check_rejects_inside_fm_block() {
        let doc = "A cdefg\n@1 fm\n\t31,0,12,7,0,28,0,0,5,0 |\n";
        assert!(pre_check(doc, 2, 25).is_none());
    }

    #[test]
    fn pre_check_rejects_timesig_no() {
        let doc = "#timesig no\nA cdef |\n";
        assert!(pre_check(doc, 1, 7).is_none());
    }

    #[test]
    fn pre_check_reads_the_column_in_utf16_units() {
        // `あ` is one UTF-16 unit and three bytes; the cursor sits after `|`.
        let doc = "A 'あ' cdef |\n";
        let (_, after_bar) = pre_check(doc, 0, 13).expect("pre-check should pass");
        assert!(after_bar);
    }

    #[test]
    fn action_titles_follow_the_bar_position() {
        let after = fill_measure_action("A o4 cdef |\n", 0, 11).expect("action");
        assert_eq!(after.title, "ctrmml: insert empty measure (4/4)");
        assert!(after.edit.is_none());
        let mid = fill_measure_action("A o4 cdef\n", 0, 9).expect("action");
        assert_eq!(mid.title, "ctrmml: fill measure with rests (4/4)");
    }

    #[test]
    fn resolve_data_round_trips() {
        let target = || FillMeasureTarget {
            uri: Url::parse("file:///song.mml").expect("uri"),
            line: 3,
            character: 7,
        };
        assert_eq!(
            FillMeasureTarget::from_data(&target().into_data()),
            Some(target())
        );
        assert_eq!(FillMeasureTarget::from_data(&json!({ "other": 1 })), None);
    }

    #[test]
    fn pre_check_picks_up_explicit_timesig() {
        let doc = "#timesig 3/8\nA cdef |\n";
        let (ts, _) = pre_check(doc, 1, 7).expect("pre-check should pass");
        assert_eq!(ts.numerator, 3);
        assert_eq!(ts.denominator, 8);
    }
}
