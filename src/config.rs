use serde_json::Value;

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
    let hierarchy_explicit = value
        .as_object()
        .is_some_and(|options| options.contains_key("fm_picker_hierarchy"));
    let settings = serde_json::from_value(value.clone()).unwrap_or_default();
    (settings, hierarchy_explicit)
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
}
