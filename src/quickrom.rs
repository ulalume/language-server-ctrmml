use serde::Deserialize;
use std::path::{Path, PathBuf};

use crate::backend::Backend;
use crate::ctrmml_cmd::{output_message, run_ctrmml_cmd};
use crate::utils::uri_to_path;

#[derive(Deserialize)]
struct QuickromConfig {
    inputs: Vec<String>,
    output: Option<QuickromOutput>,
}

#[derive(Deserialize)]
struct QuickromOutput {
    rom: Option<String>,
}

struct QuickromResolved {
    inputs: Vec<PathBuf>,
    rom_output: PathBuf,
}

pub(crate) struct QuickromRunResult {
    pub(crate) rom_output: PathBuf,
    pub(crate) warning: Option<String>,
}

impl Backend {
    pub(crate) async fn run_quickrom_single(
        &self,
        uri: String,
    ) -> std::result::Result<QuickromRunResult, String> {
        let input_path = uri_to_path(&uri).ok_or_else(|| "invalid file uri".to_string())?;
        let rom_output = input_path.with_extension("bin");
        self.run_quickrom_with_output(vec![input_path], rom_output, None)
            .await
    }

    pub(crate) async fn run_quickrom_config(
        &self,
        uri: String,
    ) -> std::result::Result<QuickromRunResult, String> {
        let start_path = uri_to_path(&uri).ok_or_else(|| "invalid file uri".to_string())?;
        let roots = self.roots.read().await.clone();
        let start_dir = if start_path.is_dir() {
            start_path.clone()
        } else {
            start_path
                .parent()
                .ok_or_else(|| "invalid file path".to_string())?
                .to_path_buf()
        };
        let workspace_root = find_workspace_root(&start_dir, &roots);
        let config_path = match find_quickrom_config(&start_path, &roots) {
            Some(path) => path,
            None => {
                let root_label = workspace_root
                    .as_ref()
                    .map(|root| root.display().to_string())
                    .unwrap_or_else(|| "filesystem root".to_string());
                return Err(format!(
                    "quickrom.json not found (searched from {} up to {})",
                    start_dir.display(),
                    root_label
                ));
            }
        };
        let resolved = read_quickrom_config(&config_path)?;
        self.run_quickrom_with_output(resolved.inputs, resolved.rom_output, None)
            .await
    }

    pub(crate) async fn run_quickrom_directory(
        &self,
        uri: String,
    ) -> std::result::Result<QuickromRunResult, String> {
        let start_path = uri_to_path(&uri).ok_or_else(|| "invalid file uri".to_string())?;
        let dir = if start_path.is_dir() {
            start_path
        } else {
            start_path
                .parent()
                .ok_or_else(|| "invalid file path".to_string())?
                .to_path_buf()
        };

        let entries = std::fs::read_dir(&dir)
            .map_err(|e| format!("failed to read directory {}: {e}", dir.display()))?;
        let mut inputs = Vec::new();
        let mut ignored_dirs = Vec::new();
        for entry in entries {
            let entry = match entry {
                Ok(entry) => entry,
                Err(_) => continue,
            };
            let path = entry.path();
            if path.is_file() {
                if is_quickrom_input(&path) {
                    inputs.push(path);
                }
                continue;
            }
            if !path.is_dir() {
                continue;
            }
            if let Some(name) = path.file_name().and_then(|v| v.to_str()) {
                if directory_has_quickrom_inputs(&path) {
                    ignored_dirs.push(name.to_string());
                }
            }
        }

        if inputs.is_empty() {
            return Err(format!("no .mml or .mds files found in {}", dir.display()));
        }

        inputs.sort_by(|a, b| a.to_string_lossy().cmp(&b.to_string_lossy()));

        let warning = if !ignored_dirs.is_empty() {
            ignored_dirs.sort();
            ignored_dirs.dedup();
            Some(format!(
                "quickrom directory is non-recursive; ignored files in subdirectories: {}",
                ignored_dirs.join(", ")
            ))
        } else {
            None
        };

        let rom_output = dir.join("quickrom.bin");
        self.run_quickrom_with_output(inputs, rom_output, warning)
            .await
    }

    async fn run_quickrom_with_output(
        &self,
        inputs: Vec<PathBuf>,
        rom_output: PathBuf,
        warning: Option<String>,
    ) -> std::result::Result<QuickromRunResult, String> {
        if inputs.is_empty() {
            return Err("no input specified".to_string());
        }
        let cmd_path = self.command_path().await?;
        let output = run_ctrmml_cmd(&cmd_path, "quickrom", None, |cmd| {
            cmd.arg("quickrom").arg("--out").arg(&rom_output);
            for input in inputs {
                cmd.arg(input);
            }
        })
        .await?;
        if !output.status.success() {
            if let Some(message) = output_message(&output) {
                return Err(message);
            }
            return Err("ctrmml-cmd quickrom failed".to_string());
        }

        Ok(QuickromRunResult {
            rom_output,
            warning,
        })
    }
}

fn find_quickrom_config(start_path: &Path, roots: &[PathBuf]) -> Option<PathBuf> {
    let start_dir = if start_path.is_dir() {
        start_path.to_path_buf()
    } else {
        start_path.parent()?.to_path_buf()
    };

    let limit = find_workspace_root(&start_dir, roots);
    let mut current = start_dir;
    loop {
        let candidate = current.join("quickrom.json");
        if candidate.is_file() {
            return Some(candidate);
        }
        if let Some(ref root) = limit {
            if current == *root {
                break;
            }
        }
        let parent = match current.parent() {
            Some(parent) => parent.to_path_buf(),
            None => break,
        };
        current = parent;
    }
    None
}

fn find_workspace_root(start_dir: &Path, roots: &[PathBuf]) -> Option<PathBuf> {
    let start = canonicalize_or_current(start_dir)?;
    let mut best: Option<PathBuf> = None;
    for root in roots {
        let root = match canonicalize_or_current(root) {
            Some(path) => path,
            None => continue,
        };
        if start.starts_with(&root) {
            let replace = match &best {
                Some(existing) => root.components().count() > existing.components().count(),
                None => true,
            };
            if replace {
                best = Some(root);
            }
        }
    }
    best
}

fn canonicalize_or_current(path: &Path) -> Option<PathBuf> {
    path.canonicalize()
        .ok()
        .or_else(|| Some(path.to_path_buf()))
}

fn read_quickrom_config(config_path: &Path) -> std::result::Result<QuickromResolved, String> {
    let text = std::fs::read_to_string(config_path)
        .map_err(|e| format!("failed to read {}: {e}", config_path.display()))?;
    let config: QuickromConfig = serde_json::from_str(&text)
        .map_err(|e| format!("failed to parse {}: {e}", config_path.display()))?;

    if config.inputs.is_empty() {
        return Err(format!(
            "quickrom.json has no inputs: {}",
            config_path.display()
        ));
    }

    let base_dir = config_path
        .parent()
        .ok_or_else(|| "invalid quickrom.json path".to_string())?;

    let inputs = config
        .inputs
        .iter()
        .map(|input| resolve_path(base_dir, input))
        .collect::<Vec<_>>();

    let rom_output = config
        .output
        .as_ref()
        .and_then(|output| output.rom.as_ref())
        .map(|value| resolve_path(base_dir, value))
        .unwrap_or_else(|| base_dir.join("quickrom.bin"));

    Ok(QuickromResolved { inputs, rom_output })
}

fn resolve_path(base_dir: &Path, value: &str) -> PathBuf {
    let path = PathBuf::from(value);
    if path.is_absolute() {
        path
    } else {
        base_dir.join(path)
    }
}

fn is_quickrom_input(path: &Path) -> bool {
    match path.extension().and_then(|ext| ext.to_str()) {
        Some(ext) => ext.eq_ignore_ascii_case("mml") || ext.eq_ignore_ascii_case("mds"),
        None => false,
    }
}

fn directory_has_quickrom_inputs(path: &Path) -> bool {
    let entries = match std::fs::read_dir(path) {
        Ok(entries) => entries,
        Err(_) => return false,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_file() && is_quickrom_input(&path) {
            return true;
        }
    }
    false
}
