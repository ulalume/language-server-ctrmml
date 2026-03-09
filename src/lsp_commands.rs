use serde_json::{json, Value};
use tower_lsp::lsp_types::{CodeAction, Command, Position};

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
];

pub(crate) fn command_ids() -> Vec<String> {
    COMMANDS.iter().map(|entry| entry.id.to_string()).collect()
}

pub(crate) fn command_title<'a>(command_id: &'a str) -> &'a str {
    COMMANDS
        .iter()
        .find(|entry| entry.id == command_id)
        .map(|entry| entry.title)
        .unwrap_or(command_id)
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
