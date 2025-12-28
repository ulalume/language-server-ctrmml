use serde_json::Value;

#[derive(Clone, Default)]
pub(crate) struct Config {
    pub(crate) command_path: Option<String>,
}

pub(crate) fn config_from_value(value: &Value) -> Option<Config> {
    let obj = value.as_object()?;
    let command_path = obj
        .get("command_path")
        .or_else(|| obj.get("commandPath"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    Some(Config { command_path })
}
