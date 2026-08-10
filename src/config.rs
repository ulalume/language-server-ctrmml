use serde::de::DeserializeOwned;
use serde_json::{Map, Value};

use ctrmml_lang_core::completion::CompletionSettings;

#[derive(Clone, Default)]
pub(crate) struct Config {
    pub(crate) command_path: Option<String>,
    pub(crate) ym2612_convert_path: Option<String>,
}

pub(crate) fn config_from_value(value: &Value) -> Option<Config> {
    let obj = value.as_object()?;
    let command_path = obj
        .get("command_path")
        .or_else(|| obj.get("commandPath"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let ym2612_convert_path = obj
        .get("ym2612_convert_path")
        .or_else(|| obj.get("ym2612ConvertPath"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    Some(Config {
        command_path,
        ym2612_convert_path,
    })
}

/// Parse completion preferences from the LSP `initializationOptions` object.
///
/// `CompletionSettings` uses serde defaults, so clients may send any subset of
/// the four settings. The boolean records whether hierarchy was explicitly
/// supplied; initialize uses it to limit the temporary vscode compatibility
/// fallback to that one setting.
pub(crate) fn completion_settings_from_value(value: &Value) -> (CompletionSettings, bool) {
    completion_settings_from_value_with_warning(value, |field, error| {
        eprintln!("invalid completion setting `{field}`: {error}; using default");
    })
}

fn completion_settings_from_value_with_warning(
    value: &Value,
    mut warning: impl FnMut(&str, &serde_json::Error),
) -> (CompletionSettings, bool) {
    let options = value.as_object();
    let defaults = CompletionSettings::default();
    let hierarchy_explicit = options.is_some_and(|options| {
        options.contains_key("fm_picker_hierarchy") || options.contains_key("fmPickerHierarchy")
    });
    let settings = CompletionSettings {
        arpeggio_enabled: completion_field(
            options,
            "arpeggio_enabled",
            "arpeggioEnabled",
            defaults.arpeggio_enabled,
            &mut warning,
        ),
        arpeggio_pattern: completion_field(
            options,
            "arpeggio_pattern",
            "arpeggioPattern",
            defaults.arpeggio_pattern,
            &mut warning,
        ),
        chord_stack_mode: completion_field(
            options,
            "chord_stack_mode",
            "chordStackMode",
            defaults.chord_stack_mode,
            &mut warning,
        ),
        fm_picker_hierarchy: completion_field(
            options,
            "fm_picker_hierarchy",
            "fmPickerHierarchy",
            defaults.fm_picker_hierarchy,
            &mut warning,
        ),
    };
    (settings, hierarchy_explicit)
}

fn completion_field<T: DeserializeOwned>(
    options: Option<&Map<String, Value>>,
    snake_case: &str,
    camel_case: &str,
    default: T,
    warning: &mut impl FnMut(&str, &serde_json::Error),
) -> T {
    let Some(value) =
        options.and_then(|options| options.get(snake_case).or_else(|| options.get(camel_case)))
    else {
        return default;
    };
    match serde_json::from_value(value.clone()) {
        Ok(value) => value,
        Err(error) => {
            warning(snake_case, &error);
            default
        }
    }
}

pub(crate) fn apply_completion_client_defaults(
    settings: &mut CompletionSettings,
    hierarchy_explicit: bool,
    client_kind: ClientKind,
) {
    if hierarchy_explicit {
        return;
    }
    settings.fm_picker_hierarchy = client_kind.is_vscode();
}

/// LSP client behavior relevant to server capabilities and presentation.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum ClientKind {
    VsCode,
    #[default]
    Other,
}

impl ClientKind {
    pub(crate) fn from_name(client_name: Option<&str>) -> Self {
        let Some(name) = client_name else {
            return Self::Other;
        };
        let name = name.to_lowercase();
        if name.contains("visual studio code") || name.contains("vscode") {
            Self::VsCode
        } else {
            Self::Other
        }
    }

    pub(crate) const fn is_vscode(self) -> bool {
        matches!(self, Self::VsCode)
    }
}

#[cfg(test)]
mod tests {
    use ctrmml_lang_core::completion::{ArpeggioPattern, ChordStackMode};
    use serde_json::json;

    use super::*;

    #[test]
    fn completion_settings_parse_partial_initialization_options() {
        let (settings, hierarchy_explicit) = completion_settings_from_value(&json!({
            "arpeggio_enabled": true,
            "arpeggio_pattern": "downup",
            "commandPath": "/tmp/ctrmml-cmd"
        }));

        assert!(settings.arpeggio_enabled);
        assert_eq!(settings.arpeggio_pattern, ArpeggioPattern::DownUp);
        assert_eq!(settings.chord_stack_mode, ChordStackMode::StackUp);
        assert!(!settings.fm_picker_hierarchy);
        assert!(!hierarchy_explicit);
    }

    #[test]
    fn completion_settings_detect_explicit_hierarchy() {
        let (settings, hierarchy_explicit) = completion_settings_from_value(&json!({
            "fm_picker_hierarchy": true
        }));

        assert!(settings.fm_picker_hierarchy);
        assert!(hierarchy_explicit);
    }

    #[test]
    fn completion_settings_parse_case_insensitive_aliases_and_camel_case() {
        let (settings, hierarchy_explicit) = completion_settings_from_value(&json!({
            "arpeggioEnabled": true,
            "arpeggioPattern": "UpDown",
            "chordStackMode": "stack-up",
            "fmPickerHierarchy": true
        }));

        assert!(settings.arpeggio_enabled);
        assert_eq!(settings.arpeggio_pattern, ArpeggioPattern::UpDown);
        assert_eq!(settings.chord_stack_mode, ChordStackMode::StackUp);
        assert!(settings.fm_picker_hierarchy);
        assert!(hierarchy_explicit);
    }

    #[test]
    fn invalid_completion_field_falls_back_without_resetting_siblings_and_warns() {
        let mut warnings = Vec::new();
        let (settings, _) = completion_settings_from_value_with_warning(
            &json!({
                "arpeggio_enabled": true,
                "arpeggio_pattern": "BOGUS"
            }),
            |field, error| warnings.push((field.to_string(), error.to_string())),
        );

        assert!(settings.arpeggio_enabled);
        assert_eq!(settings.arpeggio_pattern, ArpeggioPattern::Up);
        assert_eq!(warnings.len(), 1);
        assert_eq!(warnings[0].0, "arpeggio_pattern");
        assert!(warnings[0].1.contains("unknown variant"));
    }

    #[test]
    fn camel_case_hierarchy_is_not_overridden_for_vscode() {
        let (mut settings, hierarchy_explicit) = completion_settings_from_value(&json!({
            "fmPickerHierarchy": false
        }));
        apply_completion_client_defaults(
            &mut settings,
            hierarchy_explicit,
            ClientKind::from_name(Some("Visual Studio Code")),
        );

        assert!(!settings.fm_picker_hierarchy);
        assert!(hierarchy_explicit);
    }

    #[test]
    fn client_kind_reuses_vscode_name_sniff() {
        assert_eq!(
            ClientKind::from_name(Some("Visual Studio Code")),
            ClientKind::VsCode
        );
        assert_eq!(
            ClientKind::from_name(Some("vscode-ctrmml")),
            ClientKind::VsCode
        );
        assert_eq!(ClientKind::from_name(Some("Zed")), ClientKind::Other);
        assert_eq!(ClientKind::from_name(None), ClientKind::Other);
    }
}
