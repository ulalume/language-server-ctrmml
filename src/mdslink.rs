use serde::Deserialize;
use std::path::{Path, PathBuf};

use crate::backend::Backend;
use crate::ctrmml_cmd::{output_message, run_ctrmml_cmd};
use crate::utils::uri_to_path;

#[derive(Deserialize)]
struct MdslinkConfig {
    inputs: Vec<String>,
    output: Option<MdslinkOutput>,
}

#[derive(Deserialize)]
struct MdslinkOutput {
    seq: Option<String>,
    pcm: Option<String>,
}

struct MdslinkResolved {
    inputs: Vec<PathBuf>,
    seq_output: PathBuf,
    pcm_output: PathBuf,
}

pub(crate) struct MdslinkOutputs {
    pub(crate) seq_output: PathBuf,
    pub(crate) pcm_output: PathBuf,
    pub(crate) asm_header_output: PathBuf,
    pub(crate) c_header_output: PathBuf,
}

pub(crate) struct MdslinkRunResult {
    pub(crate) outputs: MdslinkOutputs,
    pub(crate) warning: Option<String>,
}

impl Backend {
    pub(crate) async fn run_mdslink_single(
        &self,
        uri: String,
    ) -> std::result::Result<MdslinkRunResult, String> {
        let input_path = uri_to_path(&uri).ok_or_else(|| "invalid file uri".to_string())?;
        let base_dir = input_path
            .parent()
            .ok_or_else(|| "invalid file path".to_string())?;
        let seq_output = base_dir.join("mdsseq.bin");
        let pcm_output = base_dir.join("mdspcm.bin");

        let outputs = self.run_mdslink(vec![input_path], seq_output, pcm_output).await?;
        Ok(MdslinkRunResult {
            outputs,
            warning: None,
        })
    }

    pub(crate) async fn run_mdslink_config(
        &self,
        uri: String,
    ) -> std::result::Result<MdslinkRunResult, String> {
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
        let config_path = match find_mdslink_config(&start_path, &roots) {
            Some(path) => path,
            None => {
                let root_label = workspace_root
                    .as_ref()
                    .map(|root| root.display().to_string())
                    .unwrap_or_else(|| "filesystem root".to_string());
                return Err(format!(
                    "mdslink.json not found (searched from {} up to {})",
                    start_dir.display(),
                    root_label
                ));
            }
        };
        let resolved = read_mdslink_config(&config_path)?;

        let outputs =
            self.run_mdslink(resolved.inputs, resolved.seq_output, resolved.pcm_output)
                .await?;
        Ok(MdslinkRunResult {
            outputs,
            warning: None,
        })
    }

    pub(crate) async fn run_mdslink_directory(
        &self,
        uri: String,
    ) -> std::result::Result<MdslinkRunResult, String> {
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
                if is_mdslink_input(&path) {
                    inputs.push(path);
                }
                continue;
            }
            if !path.is_dir() {
                continue;
            }
            if let Some(name) = path.file_name().and_then(|v| v.to_str()) {
                if directory_has_mdslink_inputs(&path) {
                    ignored_dirs.push(name.to_string());
                }
            }
        }

        if inputs.is_empty() {
            return Err(format!(
                "no .mml or .mds files found in {}",
                dir.display()
            ));
        }

        inputs.sort_by(|a, b| a.to_string_lossy().cmp(&b.to_string_lossy()));

        let warning = if !ignored_dirs.is_empty() {
            ignored_dirs.sort();
            ignored_dirs.dedup();
            Some(format!(
                "mdslink directory is non-recursive; ignored files in subdirectories: {}",
                ignored_dirs.join(", ")
            ))
        } else {
            None
        };

        let seq_output = dir.join("mdsseq.bin");
        let pcm_output = dir.join("mdspcm.bin");

        let outputs = self.run_mdslink(inputs, seq_output, pcm_output).await?;
        Ok(MdslinkRunResult { outputs, warning })
    }

    async fn run_mdslink(
        &self,
        inputs: Vec<PathBuf>,
        seq_output: PathBuf,
        pcm_output: PathBuf,
    ) -> std::result::Result<MdslinkOutputs, String> {
        if inputs.is_empty() {
            return Err("no input specified".to_string());
        }
        let cmd_path = self.command_path().await?;
        let asm_header_output = seq_output.with_extension("inc");
        let c_header_output = seq_output.with_extension("h");
        let output = run_ctrmml_cmd(&cmd_path, "mdslink", None, |cmd| {
            cmd.arg("mdslink")
                .arg("--output")
                .arg(&seq_output)
                .arg(&pcm_output)
                .arg("--asm-header")
                .arg(&asm_header_output)
                .arg("--c-header")
                .arg(&c_header_output);
            for input in inputs {
                cmd.arg(input);
            }
        })
        .await?;
        if !output.status.success() {
            if let Some(message) = output_message(&output) {
                return Err(message);
            }
            return Err("ctrmml-cmd mdslink failed".to_string());
        }
        Ok(MdslinkOutputs {
            seq_output,
            pcm_output,
            asm_header_output,
            c_header_output,
        })
    }
}

fn find_mdslink_config(start_path: &Path, roots: &[PathBuf]) -> Option<PathBuf> {
    let start_dir = if start_path.is_dir() {
        start_path.to_path_buf()
    } else {
        start_path.parent()?.to_path_buf()
    };

    let limit = find_workspace_root(&start_dir, roots);
    let mut current = start_dir;
    loop {
        let candidate = current.join("mdslink.json");
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
    path.canonicalize().ok().or_else(|| Some(path.to_path_buf()))
}

fn read_mdslink_config(config_path: &Path) -> std::result::Result<MdslinkResolved, String> {
    let text = std::fs::read_to_string(config_path)
        .map_err(|e| format!("failed to read {}: {e}", config_path.display()))?;
    let config: MdslinkConfig = serde_json::from_str(&text)
        .map_err(|e| format!("failed to parse {}: {e}", config_path.display()))?;
    if config.inputs.is_empty() {
        return Err(format!(
            "mdslink.json has no inputs: {}",
            config_path.display()
        ));
    }

    let base_dir = config_path
        .parent()
        .ok_or_else(|| "invalid mdslink.json path".to_string())?;
    let inputs = config
        .inputs
        .iter()
        .map(|input| resolve_path(base_dir, input))
        .collect::<Vec<_>>();

    let seq_output = config
        .output
        .as_ref()
        .and_then(|output| output.seq.as_ref())
        .map(|value| resolve_path(base_dir, value))
        .unwrap_or_else(|| base_dir.join("mdsseq.bin"));
    let pcm_output = config
        .output
        .as_ref()
        .and_then(|output| output.pcm.as_ref())
        .map(|value| resolve_path(base_dir, value))
        .unwrap_or_else(|| base_dir.join("mdspcm.bin"));

    Ok(MdslinkResolved {
        inputs,
        seq_output,
        pcm_output,
    })
}

fn resolve_path(base_dir: &Path, value: &str) -> PathBuf {
    let path = PathBuf::from(value);
    if path.is_absolute() {
        path
    } else {
        base_dir.join(path)
    }
}

fn is_mdslink_input(path: &Path) -> bool {
    match path.extension().and_then(|ext| ext.to_str()) {
        Some(ext) => ext.eq_ignore_ascii_case("mml") || ext.eq_ignore_ascii_case("mds"),
        None => false,
    }
}

fn directory_has_mdslink_inputs(path: &Path) -> bool {
    let entries = match std::fs::read_dir(path) {
        Ok(entries) => entries,
        Err(_) => return false,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_file() && is_mdslink_input(&path) {
            return true;
        }
    }
    false
}
