//! `ctrmml-cmd mdslink` and `quickrom`: link one file, the files of a
//! directory, or the inputs listed in `mdslink.json` / `quickrom.json`.

use std::path::{Path, PathBuf};

use serde::Deserialize;
use tokio::process::Command as TokioCommand;

use crate::backend::Backend;
use crate::ctrmml_cmd::{output_message, run_ctrmml_cmd};
use crate::utils::uri_to_path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LinkTool {
    Mdslink,
    Quickrom,
}

/// What a link run reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LinkSource {
    File,
    Directory,
    Config,
}

impl LinkTool {
    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::Mdslink => "mdslink",
            Self::Quickrom => "quickrom",
        }
    }

    fn config_name(self) -> &'static str {
        match self {
            Self::Mdslink => "mdslink.json",
            Self::Quickrom => "quickrom.json",
        }
    }

    /// Menu titles for each source.
    pub(crate) fn source_titles(self) -> [(String, LinkSource); 3] {
        let name = self.name();
        [
            (format!("{name} file"), LinkSource::File),
            (format!("{name} directory"), LinkSource::Directory),
            (
                format!("{name} from {}", self.config_name()),
                LinkSource::Config,
            ),
        ]
    }

    /// The default outputs for files linked in `dir`.
    fn outputs_in(self, dir: &Path) -> LinkOutputs {
        match self {
            Self::Mdslink => LinkOutputs::Mdslink {
                seq: dir.join("mdsseq.bin"),
                pcm: dir.join("mdspcm.bin"),
            },
            Self::Quickrom => LinkOutputs::Quickrom {
                rom: dir.join("quickrom.bin"),
            },
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
enum LinkOutputs {
    Mdslink { seq: PathBuf, pcm: PathBuf },
    Quickrom { rom: PathBuf },
}

impl LinkOutputs {
    fn add_args(&self, cmd: &mut TokioCommand) {
        match self {
            Self::Mdslink { seq, pcm } => {
                cmd.arg("--output")
                    .arg(seq)
                    .arg(pcm)
                    .arg("--asm-header")
                    .arg(seq.with_extension("inc"))
                    .arg("--c-header")
                    .arg(seq.with_extension("h"));
            }
            Self::Quickrom { rom } => {
                cmd.arg("--out").arg(rom);
            }
        }
    }

    fn report(&self, roots: &[PathBuf]) -> String {
        let show = |path: &Path| relative_path_display(path, roots);
        match self {
            Self::Mdslink { seq, pcm } => format!(
                "mdslink outputs: {}, {}, {}, {}",
                show(seq),
                show(pcm),
                show(&seq.with_extension("inc")),
                show(&seq.with_extension("h"))
            ),
            Self::Quickrom { rom } => format!("quickrom output: {}", show(rom)),
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
struct LinkJob {
    inputs: Vec<PathBuf>,
    outputs: LinkOutputs,
    warning: Option<String>,
}

#[derive(Deserialize)]
struct LinkConfig {
    inputs: Vec<String>,
    output: Option<LinkConfigOutput>,
}

#[derive(Deserialize)]
struct LinkConfigOutput {
    seq: Option<String>,
    pcm: Option<String>,
    rom: Option<String>,
}

impl Backend {
    /// Run `tool` on `source` around the document `uri` and describe what
    /// it wrote.
    pub(crate) async fn run_link(
        &self,
        tool: LinkTool,
        source: LinkSource,
        uri: &str,
    ) -> std::result::Result<String, String> {
        let path = uri_to_path(uri).ok_or_else(|| "invalid file uri".to_string())?;
        let roots = self.session().roots.clone();
        let plan_roots = roots.clone();
        let job = tokio::task::spawn_blocking(move || plan(tool, source, &path, &plan_roots))
            .await
            .map_err(|e| format!("{} failed: {e}", tool.name()))??;

        let cmd_path = self.command_path().await?;
        let output = run_ctrmml_cmd(&cmd_path, tool.name(), None, |cmd| {
            cmd.arg(tool.name());
            job.outputs.add_args(cmd);
            cmd.args(&job.inputs);
        })
        .await?;
        if !output.status.success() {
            return Err(output_message(&output)
                .unwrap_or_else(|| format!("ctrmml-cmd {} failed", tool.name())));
        }

        let mut message = job.outputs.report(&roots);
        if let Some(warning) = job.warning {
            message.push_str(";\n\n**warning**: ");
            message.push_str(&warning);
        }
        Ok(message)
    }
}

fn plan(
    tool: LinkTool,
    source: LinkSource,
    path: &Path,
    roots: &[PathBuf],
) -> std::result::Result<LinkJob, String> {
    match source {
        LinkSource::File => {
            let dir = path
                .parent()
                .ok_or_else(|| "invalid file path".to_string())?;
            let outputs = match tool {
                LinkTool::Mdslink => tool.outputs_in(dir),
                LinkTool::Quickrom => LinkOutputs::Quickrom {
                    rom: path.with_extension("bin"),
                },
            };
            Ok(LinkJob {
                inputs: vec![path.to_path_buf()],
                outputs,
                warning: None,
            })
        }
        LinkSource::Directory => {
            let dir = containing_dir(path)?;
            let (inputs, warning) = directory_inputs(tool, &dir)?;
            Ok(LinkJob {
                inputs,
                outputs: tool.outputs_in(&dir),
                warning,
            })
        }
        LinkSource::Config => {
            let start_dir = containing_dir(path)?;
            let root = workspace_root(&start_dir, roots);
            let config_path = find_config(tool, &start_dir, root.as_deref()).ok_or_else(|| {
                let root_label = root
                    .as_ref()
                    .map(|root| root.display().to_string())
                    .unwrap_or_else(|| "filesystem root".to_string());
                format!(
                    "{} not found (searched from {} up to {})",
                    tool.config_name(),
                    start_dir.display(),
                    root_label
                )
            })?;
            read_config(tool, &config_path)
        }
    }
}

/// `path` itself when it is a directory, else its parent.
fn containing_dir(path: &Path) -> std::result::Result<PathBuf, String> {
    if path.is_dir() {
        return Ok(path.to_path_buf());
    }
    path.parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| "invalid file path".to_string())
}

/// The `.mml` / `.mds` files directly in `dir`, sorted, and a warning
/// naming subdirectories whose inputs are left out.
fn directory_inputs(
    tool: LinkTool,
    dir: &Path,
) -> std::result::Result<(Vec<PathBuf>, Option<String>), String> {
    let entries = std::fs::read_dir(dir)
        .map_err(|e| format!("failed to read directory {}: {e}", dir.display()))?;
    let mut inputs = Vec::new();
    let mut ignored_dirs = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_file() {
            if is_link_input(&path) {
                inputs.push(path);
            }
        } else if path.is_dir() && directory_has_inputs(&path) {
            if let Some(name) = path.file_name().and_then(|name| name.to_str()) {
                ignored_dirs.push(name.to_string());
            }
        }
    }
    if inputs.is_empty() {
        return Err(format!("no .mml or .mds files found in {}", dir.display()));
    }
    inputs.sort_by(|a, b| a.to_string_lossy().cmp(&b.to_string_lossy()));
    ignored_dirs.sort();
    ignored_dirs.dedup();
    let warning = (!ignored_dirs.is_empty()).then(|| {
        format!(
            "{} directory is non-recursive; ignored files in subdirectories: {}",
            tool.name(),
            ignored_dirs.join(", ")
        )
    });
    Ok((inputs, warning))
}

fn is_link_input(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("mml") || ext.eq_ignore_ascii_case("mds"))
}

fn directory_has_inputs(dir: &Path) -> bool {
    std::fs::read_dir(dir).is_ok_and(|entries| {
        entries.flatten().any(|entry| {
            let path = entry.path();
            path.is_file() && is_link_input(&path)
        })
    })
}

/// The tool's config file in `start_dir` or the nearest directory above
/// it, stopping at `limit`.
fn find_config(tool: LinkTool, start_dir: &Path, limit: Option<&Path>) -> Option<PathBuf> {
    let mut current = start_dir;
    loop {
        let candidate = current.join(tool.config_name());
        if candidate.is_file() {
            return Some(candidate);
        }
        if limit == Some(current) {
            return None;
        }
        current = current.parent()?;
    }
}

fn read_config(tool: LinkTool, config_path: &Path) -> std::result::Result<LinkJob, String> {
    let text = std::fs::read_to_string(config_path)
        .map_err(|e| format!("failed to read {}: {e}", config_path.display()))?;
    let config: LinkConfig = serde_json::from_str(&text)
        .map_err(|e| format!("failed to parse {}: {e}", config_path.display()))?;
    if config.inputs.is_empty() {
        return Err(format!(
            "{} has no inputs: {}",
            tool.config_name(),
            config_path.display()
        ));
    }
    let base_dir = config_path
        .parent()
        .ok_or_else(|| format!("invalid {} path", tool.config_name()))?;
    let inputs = config
        .inputs
        .iter()
        .map(|input| resolve_path(base_dir, input))
        .collect();
    let configured = |pick: fn(&LinkConfigOutput) -> Option<&String>| {
        config
            .output
            .as_ref()
            .and_then(pick)
            .map(|value| resolve_path(base_dir, value))
    };
    let outputs = match tool.outputs_in(base_dir) {
        LinkOutputs::Mdslink { seq, pcm } => LinkOutputs::Mdslink {
            seq: configured(|output| output.seq.as_ref()).unwrap_or(seq),
            pcm: configured(|output| output.pcm.as_ref()).unwrap_or(pcm),
        },
        LinkOutputs::Quickrom { rom } => LinkOutputs::Quickrom {
            rom: configured(|output| output.rom.as_ref()).unwrap_or(rom),
        },
    };
    Ok(LinkJob {
        inputs,
        outputs,
        warning: None,
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

/// The deepest workspace root containing `path`, compared after resolving
/// symlinks.
pub(crate) fn workspace_root(path: &Path, roots: &[PathBuf]) -> Option<PathBuf> {
    let canonical = |path: &Path| path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let path = canonical(path);
    roots
        .iter()
        .filter(|root| path.starts_with(canonical(root)))
        .max_by_key(|root| root.components().count())
        .cloned()
}

/// `path` relative to its workspace root, or in full outside any root.
pub(crate) fn relative_path_display(path: &Path, roots: &[PathBuf]) -> String {
    workspace_root(path, roots)
        .and_then(|root| pathdiff::diff_paths(path, root))
        .unwrap_or_else(|| path.to_path_buf())
        .to_string_lossy()
        .into_owned()
}

#[cfg(test)]
mod tests {
    use std::time::SystemTime;

    use super::*;

    fn temp_test_dir(label: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "ctrmml-link-{label}-{}-{unique}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).expect("create test directory");
        dir.canonicalize().expect("canonical test directory")
    }

    #[test]
    fn single_file_outputs_follow_the_tool() {
        let song = Path::new("/w/songs/a.mml");
        let mdslink = plan(LinkTool::Mdslink, LinkSource::File, song, &[]).expect("plan");
        assert_eq!(
            mdslink.outputs,
            LinkOutputs::Mdslink {
                seq: PathBuf::from("/w/songs/mdsseq.bin"),
                pcm: PathBuf::from("/w/songs/mdspcm.bin"),
            }
        );
        let quickrom = plan(LinkTool::Quickrom, LinkSource::File, song, &[]).expect("plan");
        assert_eq!(
            quickrom.outputs,
            LinkOutputs::Quickrom {
                rom: PathBuf::from("/w/songs/a.bin"),
            }
        );
    }

    #[test]
    fn directory_inputs_are_sorted_and_subdirectories_reported() {
        let root = temp_test_dir("dir");
        std::fs::create_dir_all(root.join("extra")).expect("create subdirectory");
        for file in ["b.mml", "a.MDS", "notes.txt", "extra/c.mml"] {
            std::fs::write(root.join(file), b"").expect("write file");
        }

        let job = plan(
            LinkTool::Quickrom,
            LinkSource::Directory,
            &root.join("b.mml"),
            &[],
        )
        .expect("plan");
        assert_eq!(job.inputs, [root.join("a.MDS"), root.join("b.mml")]);
        assert_eq!(
            job.warning.as_deref(),
            Some("quickrom directory is non-recursive; ignored files in subdirectories: extra")
        );
        assert_eq!(
            job.outputs,
            LinkOutputs::Quickrom {
                rom: root.join("quickrom.bin"),
            }
        );

        std::fs::remove_dir_all(root).expect("remove test directory");
    }

    #[test]
    fn config_is_found_above_the_document_within_the_workspace() {
        let root = temp_test_dir("config");
        std::fs::create_dir_all(root.join("songs")).expect("create songs");
        std::fs::write(
            root.join("mdslink.json"),
            r#"{"inputs":["songs/a.mml"],"output":{"seq":"out/seq.bin"}}"#,
        )
        .expect("write config");

        let job = plan(
            LinkTool::Mdslink,
            LinkSource::Config,
            &root.join("songs/a.mml"),
            std::slice::from_ref(&root),
        )
        .expect("plan");
        assert_eq!(job.inputs, [root.join("songs/a.mml")]);
        assert_eq!(
            job.outputs,
            LinkOutputs::Mdslink {
                seq: root.join("out/seq.bin"),
                pcm: root.join("mdspcm.bin"),
            }
        );

        let missing = plan(
            LinkTool::Quickrom,
            LinkSource::Config,
            &root.join("songs/a.mml"),
            std::slice::from_ref(&root),
        )
        .expect_err("no quickrom.json");
        assert!(missing.starts_with("quickrom.json not found"), "{missing}");

        std::fs::remove_dir_all(root).expect("remove test directory");
    }

    #[test]
    fn displayed_paths_are_relative_to_the_deepest_root() {
        let roots = [PathBuf::from("/w"), PathBuf::from("/w/songs")];
        assert_eq!(
            relative_path_display(Path::new("/w/songs/a.vgm"), &roots),
            "a.vgm"
        );
        assert_eq!(
            relative_path_display(Path::new("/elsewhere/a.vgm"), &roots),
            "/elsewhere/a.vgm"
        );
    }
}
