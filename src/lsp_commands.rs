use std::collections::HashMap;

use ctrmml_lang_core::{
    transpose::{transpose_selection, Direction, Selection},
    LinesModel,
};
use serde_json::{json, Value};
use tower_lsp::lsp_types::{
    CodeAction, CodeActionKind, Command, Position, Range, TextEdit, Url, WorkspaceEdit,
};

pub(crate) struct CommandDef {
    pub(crate) id: &'static str,
    pub(crate) title: &'static str,
}

pub(crate) const CMD_PLAY: &str = "ctrmml.play";
pub(crate) const CMD_PLAY_FROM_CURSOR: &str = "ctrmml.playFromCursor";
pub(crate) const CMD_STOP: &str = "ctrmml.stop";
pub(crate) const CMD_EXPORT_VGM: &str = "ctrmml.exportVgm";
pub(crate) const CMD_EXPORT_WAV: &str = "ctrmml.exportWav";
pub(crate) const CMD_MDSLINK_FILE: &str = "ctrmml.mdslinkFile";
pub(crate) const CMD_MDSLINK_DIRECTORY: &str = "ctrmml.mdslinkDirectory";
pub(crate) const CMD_MDSLINK_FROM_CONFIG: &str = "ctrmml.mdslinkFromConfig";
pub(crate) const CMD_MDSLINK_MENU: &str = "ctrmml.mdslinkMenu";
pub(crate) const CMD_QUICKROM_FILE: &str = "ctrmml.quickromFile";
pub(crate) const CMD_QUICKROM_DIRECTORY: &str = "ctrmml.quickromDirectory";
pub(crate) const CMD_QUICKROM_FROM_CONFIG: &str = "ctrmml.quickromFromConfig";
pub(crate) const CMD_QUICKROM_MENU: &str = "ctrmml.quickromMenu";
/// Code-lens preview command. The web editor wires the lens chip to the
/// `mml.` namespace; vscode-ctrmml / zed-ctrmml forward clicks to the
/// LSP which builds a preview MML from the selected `@N <type>` block
/// and plays it via the existing playback infrastructure.
pub(crate) const CMD_PREVIEW_PATCH: &str = "mml.previewPatch";
/// Code-lens save command — converts the `@N fm` block to a patch
/// file format (DMP / INS / TFI / …) via `ym2612_convert`.
pub(crate) const CMD_SAVE_PATCH: &str = "mml.savePatch";

pub(crate) const COMMANDS: &[CommandDef] = &[
    CommandDef {
        id: CMD_PLAY,
        title: "ctrmml: play",
    },
    CommandDef {
        id: CMD_PLAY_FROM_CURSOR,
        title: "ctrmml: play from cursor",
    },
    CommandDef {
        id: CMD_STOP,
        title: "ctrmml: stop",
    },
    CommandDef {
        id: CMD_EXPORT_VGM,
        title: "ctrmml: export vgm",
    },
    CommandDef {
        id: CMD_EXPORT_WAV,
        title: "ctrmml: export wav",
    },
    CommandDef {
        id: CMD_MDSLINK_FILE,
        title: "ctrmml: mdslink file",
    },
    CommandDef {
        id: CMD_MDSLINK_DIRECTORY,
        title: "ctrmml: mdslink directory",
    },
    CommandDef {
        id: CMD_MDSLINK_FROM_CONFIG,
        title: "ctrmml: mdslink from mdslink.json",
    },
    CommandDef {
        id: CMD_MDSLINK_MENU,
        title: "ctrmml: mdslink...",
    },
    CommandDef {
        id: CMD_QUICKROM_FILE,
        title: "ctrmml: quickrom file",
    },
    CommandDef {
        id: CMD_QUICKROM_DIRECTORY,
        title: "ctrmml: quickrom directory",
    },
    CommandDef {
        id: CMD_QUICKROM_FROM_CONFIG,
        title: "ctrmml: quickrom from quickrom.json",
    },
    CommandDef {
        id: CMD_QUICKROM_MENU,
        title: "ctrmml: quickrom...",
    },
    CommandDef {
        id: CMD_PREVIEW_PATCH,
        title: "ctrmml: preview instrument patch",
    },
    CommandDef {
        id: CMD_SAVE_PATCH,
        title: "ctrmml: save instrument patch",
    },
];

pub(crate) fn command_ids() -> Vec<String> {
    COMMANDS.iter().map(|entry| entry.id.to_string()).collect()
}

pub(crate) fn command_title(command_id: &str) -> &str {
    COMMANDS
        .iter()
        .find(|entry| entry.id == command_id)
        .map(|entry| entry.title)
        .unwrap_or(command_id)
}

/// Build a transpose-by-one-semitone code action for `range` in
/// `doc_text`. Returns `None` when the selection contains no notes (in
/// which case the edit would be a no-op).
///
/// The action is delivered as a `WorkspaceEdit` rather than a command
/// round-trip, so the client applies it immediately.
pub(crate) fn transpose_code_action(
    uri: &Url,
    range: Range,
    doc_text: &str,
    direction: Direction,
) -> Option<CodeAction> {
    let model = LinesModel::from_text(doc_text);
    let sel = Selection {
        // LSP Position is 0-based; ctrmml-lang-core Selection is 1-based.
        start_line_number: range.start.line + 1,
        start_column: range.start.character + 1,
        end_line_number: range.end.line + 1,
        end_column: range.end.character + 1,
    };
    let edit = transpose_selection(&model, sel, direction)?;
    let title = match direction {
        Direction::Up => "ctrmml: transpose up (semitone)",
        Direction::Down => "ctrmml: transpose down (semitone)",
    };
    let text_edit = TextEdit {
        range: Range {
            start: Position {
                line: edit.start_line_number - 1,
                character: edit.start_column - 1,
            },
            end: Position {
                line: edit.end_line_number - 1,
                character: edit.end_column - 1,
            },
        },
        new_text: edit.text,
    };
    let mut changes: HashMap<Url, Vec<TextEdit>> = HashMap::new();
    changes.insert(uri.clone(), vec![text_edit]);
    Some(CodeAction {
        title: title.to_string(),
        kind: Some(CodeActionKind::REFACTOR_REWRITE),
        edit: Some(WorkspaceEdit {
            changes: Some(changes),
            document_changes: None,
            change_annotations: None,
        }),
        ..CodeAction::default()
    })
}

pub(crate) fn code_actions(uri: &str, start: Position) -> Vec<CodeAction> {
    vec![
        command_action(command_title(CMD_PLAY), CMD_PLAY, vec![json!(uri)]),
        command_action(
            command_title(CMD_PLAY_FROM_CURSOR),
            CMD_PLAY_FROM_CURSOR,
            vec![json!(uri), json!(start.line), json!(start.character)],
        ),
        command_action(command_title(CMD_STOP), CMD_STOP, vec![]),
        command_action(
            command_title(CMD_EXPORT_VGM),
            CMD_EXPORT_VGM,
            vec![json!(uri)],
        ),
        command_action(
            command_title(CMD_EXPORT_WAV),
            CMD_EXPORT_WAV,
            vec![json!(uri)],
        ),
        command_action(
            command_title(CMD_QUICKROM_MENU),
            CMD_QUICKROM_MENU,
            vec![json!(uri)],
        ),
        command_action(command_title(CMD_MDSLINK_MENU), CMD_MDSLINK_MENU, vec![json!(uri)]),
    ]
}

fn command_action(title: &str, command: &str, arguments: Vec<Value>) -> CodeAction {
    let args = if arguments.is_empty() {
        None
    } else {
        Some(arguments)
    };
    CodeAction {
        title: title.to_string(),
        command: Some(Command {
            title: title.to_string(),
            command: command.to_string(),
            arguments: args,
        }),
        ..CodeAction::default()
    }
}
