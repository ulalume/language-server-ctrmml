//! `initializationOptions` and workspace configuration.
//!
//! Option keys are snake_case; camelCase spellings are accepted too.

use serde::de::DeserializeOwned;
use serde_json::{Map, Value};

use ctrmml_lang_core::completion::CompletionSettings;
use ctrmml_lang_core::{CodeLensConfig, IconStyle};

use crate::lsp_commands::{CMD_LOAD_PATCH, CMD_PREVIEW_PATCH, CMD_SAVE_PATCH};

/// Settings a client may change while the server runs.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Settings {
    pub(crate) command_path: Option<String>,
    pub(crate) completion: CompletionSettings,
}

/// What the client handles itself, fixed at initialization.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ClientOptions {
    pub(crate) code_lens: CodeLensConfig,
    /// Offer play, stop, export, quickrom and mdslink as code actions.
    pub(crate) command_code_actions: bool,
    /// Commands the client registers itself; the server does not
    /// advertise them.
    pub(crate) client_commands: Vec<String>,
}

/// Whether the client name is VS Code's; only consulted for option keys
/// the client does not send.
fn is_vscode(client_name: Option<&str>) -> bool {
    client_name.is_some_and(|name| {
        let name = name.to_lowercase();
        name.contains("visual studio code") || name.contains("vscode")
    })
}

impl ClientOptions {
    /// The options a client gets for keys it does not send: VS Code's
    /// extension handles the lens commands itself, every other client gets
    /// plain lenses.
    fn guessed(vscode: bool) -> Self {
        if vscode {
            Self {
                code_lens: CodeLensConfig::default(),
                command_code_actions: true,
                client_commands: [CMD_PREVIEW_PATCH, CMD_LOAD_PATCH, CMD_SAVE_PATCH]
                    .map(str::to_string)
                    .to_vec(),
            }
        } else {
            Self {
                code_lens: CodeLensConfig {
                    icon_style: IconStyle::None,
                    include_file_actions: false,
                },
                command_code_actions: true,
                client_commands: Vec::new(),
            }
        }
    }
}

/// What `initialize` settles.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Initialization {
    pub(crate) client: ClientOptions,
    pub(crate) settings: Settings,
    /// The FM picker style when configuration leaves it to the server.
    pub(crate) fm_picker_hierarchy_default: bool,
}

/// Read `initializationOptions`. Keys the client leaves out fall back to
/// what its name suggests.
pub(crate) fn initialization(options: Option<&Value>, client_name: Option<&str>) -> Initialization {
    let options = options.and_then(Value::as_object);
    let vscode = is_vscode(client_name);
    let guessed = ClientOptions::guessed(vscode);
    let mut warn = warn_invalid;
    let code_lens = CodeLensConfig {
        icon_style: field(options, "code_lens_icons", "codeLensIcons", &mut warn)
            .unwrap_or(guessed.code_lens.icon_style),
        include_file_actions: field(
            options,
            "code_lens_file_actions",
            "codeLensFileActions",
            &mut warn,
        )
        .unwrap_or(guessed.code_lens.include_file_actions),
    };
    let client = ClientOptions {
        code_lens,
        command_code_actions: field(
            options,
            "command_code_actions",
            "commandCodeActions",
            &mut warn,
        )
        .unwrap_or(guessed.command_code_actions),
        client_commands: field(options, "client_commands", "clientCommands", &mut warn)
            .unwrap_or(guessed.client_commands),
    };

    let fm_picker_hierarchy_default = field(
        options,
        "fm_picker_hierarchy",
        "fmPickerHierarchy",
        &mut warn,
    )
    .unwrap_or(vscode);
    let mut settings = Settings {
        command_path: None,
        completion: CompletionSettings {
            fm_picker_hierarchy: fm_picker_hierarchy_default,
            ..CompletionSettings::default()
        },
    };
    if let Some(options) = options {
        apply_flat(&mut settings, options, &mut warn);
    }
    Initialization {
        client,
        settings,
        fm_picker_hierarchy_default,
    }
}

/// `initial` updated by a `ctrmml` configuration object: VS Code's nested
/// `completion.*` keys, then the flat option keys, then VS Code's
/// `languageServer.initializationOptions` object. A `ctrmml` wrapper
/// around the object is accepted. Keys left out keep their initial value.
pub(crate) fn configured_settings(
    initial: &Settings,
    value: &Value,
    fm_picker_hierarchy_default: bool,
) -> Settings {
    let mut settings = initial.clone();
    let Some(mut object) = value.as_object() else {
        return settings;
    };
    if let Some(inner) = object.get("ctrmml").and_then(Value::as_object) {
        object = inner;
    }
    let mut warn = warn_invalid;

    if let Some(completion) = object.get("completion").and_then(Value::as_object) {
        let arpeggio = completion.get("arpeggio").and_then(Value::as_object);
        let completion_settings = &mut settings.completion;
        if let Some(value) = field(arpeggio, "enabled", "enabled", &mut warn) {
            completion_settings.arpeggio_enabled = value;
        }
        if let Some(value) = field(arpeggio, "pattern", "pattern", &mut warn) {
            completion_settings.arpeggio_pattern = value;
        }
        if let Some(value) = field(
            Some(completion),
            "chordStackMode",
            "chordStackMode",
            &mut warn,
        ) {
            completion_settings.chord_stack_mode = value;
        }
        match completion.get("fmPickerHierarchy").and_then(Value::as_str) {
            Some("on") => completion_settings.fm_picker_hierarchy = true,
            Some("off") => completion_settings.fm_picker_hierarchy = false,
            Some(_) => completion_settings.fm_picker_hierarchy = fm_picker_hierarchy_default,
            None => {}
        }
    }
    apply_flat(&mut settings, object, &mut warn);
    if let Some(raw) = object
        .get("languageServer")
        .and_then(|server| server.get("initializationOptions"))
        .and_then(Value::as_object)
    {
        apply_flat(&mut settings, raw, &mut warn);
    }
    settings
}

/// The flat keys: `command_path` and the completion settings.
fn apply_flat(
    settings: &mut Settings,
    options: &Map<String, Value>,
    warn: &mut impl FnMut(&str, &serde_json::Error),
) {
    let options = Some(options);
    if let Some(path) = field::<String>(options, "command_path", "commandPath", warn) {
        settings.command_path = Some(path).filter(|path| !path.trim().is_empty());
    }
    let completion = &mut settings.completion;
    if let Some(value) = field(options, "arpeggio_enabled", "arpeggioEnabled", warn) {
        completion.arpeggio_enabled = value;
    }
    if let Some(value) = field(options, "arpeggio_pattern", "arpeggioPattern", warn) {
        completion.arpeggio_pattern = value;
    }
    if let Some(value) = field(options, "chord_stack_mode", "chordStackMode", warn) {
        completion.chord_stack_mode = value;
    }
    if let Some(value) = field(options, "fm_picker_hierarchy", "fmPickerHierarchy", warn) {
        completion.fm_picker_hierarchy = value;
    }
}

/// The value under either spelling; `None` when absent or invalid.
fn field<T: DeserializeOwned>(
    options: Option<&Map<String, Value>>,
    snake_case: &str,
    camel_case: &str,
    warn: &mut impl FnMut(&str, &serde_json::Error),
) -> Option<T> {
    let value = options?
        .get(snake_case)
        .or_else(|| options?.get(camel_case))?;
    serde_json::from_value(value.clone())
        .map_err(|error| warn(snake_case, &error))
        .ok()
}

fn warn_invalid(field: &str, error: &serde_json::Error) {
    eprintln!("invalid setting `{field}`: {error}; ignored");
}

#[cfg(test)]
mod tests {
    use ctrmml_lang_core::completion::{ArpeggioPattern, ChordStackMode};
    use serde_json::json;

    use super::*;

    #[test]
    fn explicit_options_override_the_name_guess() {
        let init = initialization(
            Some(&json!({
                "code_lens_icons": "codicon",
                "codeLensFileActions": true,
                "command_code_actions": false,
                "client_commands": ["mml.previewPatch"],
                "fm_picker_hierarchy": false,
            })),
            Some("Zed"),
        );
        assert_eq!(init.client.code_lens, CodeLensConfig::default());
        assert!(!init.client.command_code_actions);
        assert_eq!(init.client.client_commands, ["mml.previewPatch"]);
        assert!(!init.fm_picker_hierarchy_default);
        assert!(!init.settings.completion.fm_picker_hierarchy);
    }

    #[test]
    fn without_options_the_client_name_decides() {
        let vscode = initialization(None, Some("Visual Studio Code"));
        assert_eq!(vscode.client.code_lens, CodeLensConfig::default());
        assert!(vscode.client.command_code_actions);
        assert!(vscode
            .client
            .client_commands
            .contains(&CMD_PREVIEW_PATCH.to_string()));
        assert!(vscode.settings.completion.fm_picker_hierarchy);

        for name in [Some("Zed"), Some("Helix"), None] {
            let other = initialization(Some(&json!({})), name);
            assert_eq!(other.client.code_lens.icon_style, IconStyle::None);
            assert!(!other.client.code_lens.include_file_actions);
            assert!(other.client.command_code_actions);
            assert!(other.client.client_commands.is_empty());
            assert!(!other.settings.completion.fm_picker_hierarchy);
        }
    }

    #[test]
    fn an_explicit_hierarchy_is_kept_for_vscode() {
        let init = initialization(
            Some(&json!({"fmPickerHierarchy": false})),
            Some("Visual Studio Code"),
        );
        assert!(!init.fm_picker_hierarchy_default);
        assert!(!init.settings.completion.fm_picker_hierarchy);
    }

    #[test]
    fn initialization_reads_completion_settings_and_command_path() {
        let init = initialization(
            Some(&json!({
                "arpeggio_enabled": true,
                "arpeggioPattern": "UpDown",
                "chordStackMode": "stack-up",
                "commandPath": "/tmp/ctrmml-cmd",
            })),
            None,
        );
        let completion = &init.settings.completion;
        assert!(completion.arpeggio_enabled);
        assert_eq!(completion.arpeggio_pattern, ArpeggioPattern::UpDown);
        assert_eq!(completion.chord_stack_mode, ChordStackMode::StackUp);
        assert_eq!(
            init.settings.command_path.as_deref(),
            Some("/tmp/ctrmml-cmd")
        );
    }

    #[test]
    fn an_invalid_value_keeps_its_siblings() {
        let init = initialization(
            Some(&json!({"arpeggio_enabled": true, "arpeggio_pattern": "BOGUS"})),
            None,
        );
        assert!(init.settings.completion.arpeggio_enabled);
        assert_eq!(
            init.settings.completion.arpeggio_pattern,
            ArpeggioPattern::Up
        );
    }

    #[test]
    fn vscode_configuration_is_read_with_raw_options_last() {
        let initial = initialization(None, Some("Visual Studio Code")).settings;
        let value = json!({
            "languageServer": {
                "path": "",
                "initializationOptions": {"chordStackMode": "plain"}
            },
            "completion": {
                "arpeggio": {"enabled": true, "pattern": "down"},
                "chordStackMode": "stack_up",
                "fmPickerHierarchy": "off"
            }
        });
        let settings = configured_settings(&initial, &value, true);
        assert!(settings.completion.arpeggio_enabled);
        assert_eq!(settings.completion.arpeggio_pattern, ArpeggioPattern::Down);
        assert_eq!(settings.completion.chord_stack_mode, ChordStackMode::Plain);
        assert!(!settings.completion.fm_picker_hierarchy);

        let auto = json!({"ctrmml": {"completion": {"fmPickerHierarchy": "auto"}}});
        assert!(
            configured_settings(&settings, &auto, true)
                .completion
                .fm_picker_hierarchy
        );
    }

    #[test]
    fn flat_configuration_overrides_initial_settings_key_by_key() {
        let initial = initialization(
            Some(&json!({"arpeggio_enabled": true, "command_path": "/a"})),
            Some("Zed"),
        )
        .settings;
        let settings = configured_settings(
            &initial,
            &json!({"arpeggio_pattern": "alberti", "command_path": "/b"}),
            false,
        );
        assert!(settings.completion.arpeggio_enabled);
        assert_eq!(
            settings.completion.arpeggio_pattern,
            ArpeggioPattern::Alberti
        );
        assert_eq!(settings.command_path.as_deref(), Some("/b"));

        // Removing a key from the configuration restores the initial value.
        let settings = configured_settings(&initial, &json!({}), false);
        assert_eq!(settings, initial);
        assert_eq!(configured_settings(&initial, &Value::Null, false), initial);
    }
}
