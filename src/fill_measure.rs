//! Fill-measure code action — Phase 3.2.3 (full variant).
//!
//! When the cursor sits on a track-content line, offer an "insert
//! rests up to the next bar line" code action. The rest sequence is
//! computed from the cursor's compiled playback tick (`ctrmml-cmd
//! find-cursor-tick`) and the song's PPQN, so partial-measure fill
//! works correctly — `o4 c4 d4 [cursor]` in 4/4 inserts the two
//! quarter rests needed to reach the next bar line.
//!
//! Cheap pre-checks run synchronously; the subprocess to `ctrmml-cmd`
//! only fires when those pre-checks pass, so the typical
//! cursor-in-header / cursor-in-FM-block case has zero subprocess
//! cost.

use std::collections::HashMap;

use ctrmml_lang_core::{
    find_enclosing_track_selector, find_fm_block_at, find_psg_block_at,
    generate_measure_rests, is_after_bar_line, scan_time_signature,
    track_selector::LineReader, LinesModel, TimeSignature,
};
use serde::Deserialize;
use tower_lsp::lsp_types::{
    CodeAction, CodeActionKind, Position, Range, TextEdit, Url, WorkspaceEdit,
};

use crate::ctrmml_cmd::{output_message, run_ctrmml_cmd};
use crate::utils::uri_to_path;

/// JSON payload from `ctrmml-cmd find-cursor-tick`.
#[derive(Debug, Deserialize)]
struct CursorTickResponse {
    cursor_tick: i32,
    ppqn: u32,
}

/// Cheap synchronous pre-check: returns `Some((line_content, after_bar))`
/// when the cursor position is a candidate for fill-measure.
///
/// `None` when:
/// - Cursor is inside an `@N fm` / `@N psg` block (digit data, no
///   rests applicable).
/// - Cursor is outside any enclosing track selector (file header /
///   meta region).
/// - `#timesig no` is set (measure lines explicitly disabled).
fn pre_check<'a>(
    doc_text: &'a str,
    line_zero_based: u32,
    character: u32,
) -> Option<(TimeSignature, bool)> {
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
    let time_sig = scan_time_signature(doc_text)?;

    let line_content = model.get_line_content(line);
    let after_bar = is_after_bar_line(line_content, col);
    Some((time_sig, after_bar))
}

/// Async entry point. Runs the pre-check, then spawns `ctrmml-cmd
/// find-cursor-tick` to get the cursor's playback tick + song PPQN,
/// and finally builds a `CodeAction` whose `WorkspaceEdit` inserts
/// the fill text at the cursor.
///
/// Returns `None` for any pre-check failure or subprocess error
/// (compilation failed, cursor doesn't map to a compiled event).
pub(crate) async fn fill_measure_code_action(
    cmd_path: &str,
    uri: &Url,
    doc_text: &str,
    line_zero_based: u32,
    character: u32,
) -> Option<CodeAction> {
    let (time_sig, after_bar) = pre_check(doc_text, line_zero_based, character)?;

    // Spawn ctrmml-cmd to compute the cursor's compiled tick + ppqn.
    // The CLI takes 0-based line/col (matching the WASM API), which
    // is what we already have from LSP.
    let path = uri_to_path(&uri.to_string());
    let output = run_ctrmml_cmd(cmd_path, "find-cursor-tick", Some(doc_text), |cmd| {
        cmd.arg("find-cursor-tick").arg("--stdin");
        if let Some(p) = path.as_deref() {
            cmd.arg("--path").arg(p);
        }
        cmd.arg("--line")
            .arg(line_zero_based.to_string())
            .arg("--col")
            .arg(character.to_string());
    })
    .await
    .ok()?;

    if !output.status.success() {
        // Compile failure or invalid input — silently skip rather
        // than surfacing the error in the code-action menu.
        let _ = output_message(&output);
        return None;
    }

    let response: CursorTickResponse = serde_json::from_slice(&output.stdout).ok()?;
    if response.cursor_tick < 0 {
        // The cursor doesn't map to any compiled event (e.g. inside
        // a comment or an unreachable region).
        return None;
    }

    let rests = generate_measure_rests(
        response.cursor_tick as u32,
        response.ppqn,
        Some(time_sig),
        after_bar,
    );
    if rests.is_empty() {
        // Already on a bar boundary and not "after-bar" — nothing to
        // insert.
        return None;
    }

    // Conventional shape: rests then a trailing `|` so the next bar
    // marker is already in place. Same form for both the after-bar
    // and mid-measure cases — only the action title differs.
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

    let title = if after_bar {
        format!(
            "ctrmml: insert empty measure ({}/{})",
            time_sig.numerator, time_sig.denominator
        )
    } else {
        format!(
            "ctrmml: fill measure with rests ({}/{})",
            time_sig.numerator, time_sig.denominator
        )
    };

    Some(CodeAction {
        title,
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
    fn pre_check_picks_up_explicit_timesig() {
        let doc = "#timesig 3/8\nA cdef |\n";
        let (ts, _) = pre_check(doc, 1, 7).expect("pre-check should pass");
        assert_eq!(ts.numerator, 3);
        assert_eq!(ts.denominator, 8);
    }
}
