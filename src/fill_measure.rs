//! Fill-measure code action.
//!
//! Offered on track lines; the edit needs the cursor's compiled tick from
//! `ctrmml-cmd find-cursor-tick`, so it is built only when needed. What a
//! fill inserts is decided in `ctrmml_lang_core::fill_measure`.

use std::collections::HashMap;

use ctrmml_lang_core::{
    fill_measure_edit as core_fill_measure_edit, fill_measure_plan, utf16_character_to_byte_offset,
    CursorTickData,
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

/// The fill-measure action at an LSP position, still without its edit.
pub(crate) fn fill_measure_action(
    doc_text: &str,
    line_zero_based: u32,
    character: u32,
) -> Option<CodeAction> {
    let plan = fill_measure_plan(doc_text, line_zero_based, character)?;
    let what = if plan.after_bar_line {
        "insert empty measure"
    } else {
        "fill measure with rests"
    };
    Some(CodeAction {
        title: format!("ctrmml: {what} ({}/{})", plan.numerator, plan.denominator),
        kind: Some(CodeActionKind::REFACTOR_REWRITE),
        ..CodeAction::default()
    })
}

/// The edit filling the measure. Runs `ctrmml-cmd find-cursor-tick`;
/// `None` when the position cannot take a fill or the song does not
/// compile.
pub(crate) async fn fill_measure_edit(
    cmd_path: &str,
    uri: &Url,
    doc_text: &str,
    line_zero_based: u32,
    character: u32,
) -> Option<WorkspaceEdit> {
    fill_measure_plan(doc_text, line_zero_based, character)?;
    let timing = fetch_cursor_tick(cmd_path, uri, doc_text, line_zero_based, character).await?;
    let fill = core_fill_measure_edit(
        doc_text,
        line_zero_based,
        character,
        timing.tick,
        timing.ppqn,
    )?;
    let range = fill.edit.range;
    let text_edit = TextEdit {
        range: Range {
            start: Position::new(range.start.line, range.start.character),
            end: Position::new(range.end.line, range.end.character),
        },
        new_text: fill.edit.new_text,
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

    #[test]
    fn action_titles_follow_the_bar_position() {
        let after = fill_measure_action("A o4 cdef |\n", 0, 11).expect("action");
        assert_eq!(after.title, "ctrmml: insert empty measure (4/4)");
        assert!(after.edit.is_none());
        let mid = fill_measure_action("A o4 cdef\n", 0, 9).expect("action");
        assert_eq!(mid.title, "ctrmml: fill measure with rests (4/4)");
        assert!(fill_measure_action("#timesig no\nA cdef |\n", 1, 8).is_none());
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
}
