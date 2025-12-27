use std::{collections::HashMap, collections::HashSet, env, fs, io, path::{Path, PathBuf}, sync::Arc};

use serde::Deserialize;
use serde_json::{json, Value};

use dirs::cache_dir;
use flate2::read::GzDecoder;
use reqwest::Client as HttpClient;
use tar::Archive;
use zip::ZipArchive;

use pathdiff::diff_paths;
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    process::Command as TokioCommand,
    sync::{Mutex, RwLock},
};
use tower_lsp::{
    jsonrpc::Result,
    lsp_types::{
        CodeAction, CodeActionOrCommand, CodeActionParams, CodeActionProviderCapability,
        Command, CompletionItem, CompletionItemKind, CompletionOptions, CompletionParams,
        CompletionResponse, Diagnostic, DiagnosticSeverity, Documentation, ExecuteCommandParams,
        ExecuteCommandOptions, InitializeParams, InitializeResult, Position, Range,
        ServerCapabilities, TextDocumentSyncCapability, TextDocumentSyncKind,
    },
    Client, LanguageServer, LspService, Server,
};
use walkdir::WalkDir;

const META_KEYWORDS: &[&str] = &[
    "#title",
    "#composer",
    "#author",
    "#date",
    "#comment",
    "#platform",
    "#option",
    "#game",
    "#composerj",
    "#programmer",
];

const COMMAND_KEYWORDS: &[&str] = &[
    "o",
    "l",
    "Q",
    "q",
    "C",
    "R",
    "L",
    "s",
    "t",
    "T",
    "v",
    "V",
    "p",
    "k",
    "K",
    "E",
    "M",
    "P",
    "G",
    "D",
    "r",
    "^",
    "&",
];

const PLATFORM_VALUES: &[&str] = &["megadrive", "mdsdrv"];
const INSTRUMENT_TYPES: &[&str] = &["pcm", "fm", "psg", "2op"];

const CTRMML_CMD_REPO: &str = "ulalume/ctrmml-cmd";
const CTRMML_CMD_NAME: &str = "ctrmml-cmd";

#[derive(Clone, Default)]
struct Config {
    command_path: Option<String>,
}

struct Playback {
    uri: String,
    child: tokio::process::Child,
    temp_path: Option<PathBuf>,
}

#[derive(Deserialize)]
struct HighlightMessage {
    #[serde(rename = "type")]
    kind: String,
    ticks: u32,
    positions: Vec<HighlightPosition>,
}

#[derive(Deserialize)]
struct HighlightPosition {
    line: u32,
    col: u32,
}

struct Backend {
    client: Client,
    docs: Arc<RwLock<HashMap<String, String>>>,
    roots: Arc<RwLock<Vec<PathBuf>>>,
    config: Arc<RwLock<Config>>,
    playback: Arc<Mutex<Option<Playback>>>,
    playback_seq: Arc<Mutex<u64>>,
    last_doc: Arc<RwLock<Option<String>>>,
}

#[tower_lsp::async_trait]
impl LanguageServer for Backend {
    async fn initialize(&self, params: InitializeParams) -> Result<InitializeResult> {
        let mut roots = Vec::new();
        if let Some(folders) = params.workspace_folders {
            for folder in folders {
                if let Ok(path) = folder.uri.to_file_path() {
                    roots.push(path);
                }
            }
        } else if let Some(uri) = params.root_uri {
            if let Ok(path) = uri.to_file_path() {
                roots.push(path);
            }
        }
        *self.roots.write().await = roots;

        if let Some(options) = params.initialization_options {
            if let Some(config) = config_from_value(&options) {
                *self.config.write().await = config;
            }
        }

        Ok(InitializeResult {
            capabilities: ServerCapabilities {
                text_document_sync: Some(TextDocumentSyncCapability::Kind(
                    TextDocumentSyncKind::FULL,
                )),
                completion_provider: Some(CompletionOptions {
                    resolve_provider: Some(false),
                    trigger_characters: Some(vec![
                        "#".into(),
                        "@".into(),
                        "\"".into(),
                        " ".into(),
                        "/".into(),
                        ".".into(),
                    ]),
                    ..CompletionOptions::default()
                }),
                execute_command_provider: Some(ExecuteCommandOptions {
                    commands: vec![
                        "ctrmml.play".to_string(),
                        "ctrmml.playFromCursor".to_string(),
                        "ctrmml.stop".to_string(),
                        "ctrmml.exportVgm".to_string(),
                        "ctrmml.exportWav".to_string(),
                    ],
                    ..ExecuteCommandOptions::default()
                }),
                code_action_provider: Some(CodeActionProviderCapability::Simple(true)),
                ..ServerCapabilities::default()
            },
            ..InitializeResult::default()
        })
    }

    async fn did_open(&self, params: tower_lsp::lsp_types::DidOpenTextDocumentParams) {
        let uri = params.text_document.uri.to_string();
        let text = params.text_document.text;
        self.docs.write().await.insert(uri.clone(), text);
        *self.last_doc.write().await = Some(uri);
    }

    async fn did_change(&self, params: tower_lsp::lsp_types::DidChangeTextDocumentParams) {
        let uri = params.text_document.uri.to_string();
        if let Some(change) = params.content_changes.into_iter().last() {
            self.docs.write().await.insert(uri.clone(), change.text);
            *self.last_doc.write().await = Some(uri);
        }
    }

    async fn completion(&self, params: CompletionParams) -> Result<Option<CompletionResponse>> {
        let uri = params
            .text_document_position
            .text_document
            .uri
            .to_string();
        let position = params.text_document_position.position;
        let text = self.docs.read().await.get(&uri).cloned().unwrap_or_default();

        let line = line_at(&text, position.line).unwrap_or_default();
        let col = position.character as usize;
        if is_in_comment(&line, col) {
            return Ok(None);
        }

        let roots = self.roots.read().await.clone();
        if let Some(items) = complete_pcm_paths(&line, col, &uri, &roots, position.line) {
            return Ok(Some(CompletionResponse::Array(items)));
        }

        if line.trim_start().starts_with("#platform") {
            let items = PLATFORM_VALUES
                .iter()
                .map(|value| platform_item(value))
                .collect();
            return Ok(Some(CompletionResponse::Array(items)));
        }

        if line.trim_start().starts_with('#') {
            let items = meta_completion_items(&line, col, position.line);
            return Ok(Some(CompletionResponse::Array(items)));
        }

        if is_rate_offset_context(&line, col) {
            let items = ["rate=", "offset="]
                .iter()
                .map(|kw| rate_offset_item(kw))
                .collect();
            return Ok(Some(CompletionResponse::Array(items)));
        }

        if is_instrument_definition_context(&line, col) {
            let items = INSTRUMENT_TYPES
                .iter()
                .map(|value| instrument_item(value))
                .collect();
            return Ok(Some(CompletionResponse::Array(items)));
        }

        if is_at_meta_context(&line, col) {
            let items = at_meta_completion_items(&line, col, position.line);
            return Ok(Some(CompletionResponse::Array(items)));
        }

        if line.trim_start().starts_with('@') {
            return Ok(None);
        }

        let items = COMMAND_KEYWORDS
            .iter()
            .map(|kw| command_item(kw))
            .collect();
        Ok(Some(CompletionResponse::Array(items)))
    }

    async fn code_action(&self, params: CodeActionParams) -> Result<Option<Vec<CodeActionOrCommand>>> {
        let uri = params.text_document.uri.to_string();
        if !is_mml_uri(&uri) {
            return Ok(None);
        }
        let start = params.range.start;
        let actions = vec![
            command_action("ctrmml: play", "ctrmml.play", vec![json!(uri.clone())]),
            command_action(
                "ctrmml: play from cursor",
                "ctrmml.playFromCursor",
                vec![json!(uri.clone()), json!(start.line), json!(start.character)],
            ),
            command_action("ctrmml: stop", "ctrmml.stop", vec![]),
            command_action("ctrmml: export vgm", "ctrmml.exportVgm", vec![json!(uri.clone())]),
            command_action("ctrmml: export wav", "ctrmml.exportWav", vec![json!(uri)]),
        ];
        Ok(Some(
            actions
                .into_iter()
                .map(CodeActionOrCommand::CodeAction)
                .collect(),
        ))
    }

    async fn execute_command(&self, params: ExecuteCommandParams) -> Result<Option<Value>> {
        let args = params.arguments;
        match params.command.as_str() {
            "ctrmml.play" => {
                let uri = self.resolve_uri_arg(&args).await.map_err(lsp_err)?;
                self.start_playback(uri, None).await.map_err(lsp_err)?;
            }
            "ctrmml.playFromCursor" => {
                let uri = self.resolve_uri_arg(&args).await.map_err(lsp_err)?;
                let line = args.get(1).and_then(|v| v.as_u64()).unwrap_or(0) as u32;
                let col = args.get(2).and_then(|v| v.as_u64()).unwrap_or(0) as u32;
                self.start_playback(uri, Some((line, col))).await.map_err(lsp_err)?;
            }
            "ctrmml.stop" => {
                self.stop_playback().await;
            }
            "ctrmml.exportVgm" => {
                let uri = self.resolve_uri_arg(&args).await.map_err(lsp_err)?;
                self.run_export(uri, ExportFormat::Vgm).await.map_err(lsp_err)?;
            }
            "ctrmml.exportWav" => {
                let uri = self.resolve_uri_arg(&args).await.map_err(lsp_err)?;
                self.run_export(uri, ExportFormat::Wav).await.map_err(lsp_err)?;
            }
            _ => {}
        }
        Ok(None)
    }

    async fn shutdown(&self) -> Result<()> {
        Ok(())
    }
}

enum ExportFormat {
    Vgm,
    Wav,
}

impl Backend {
    async fn materialize_mml(&self, uri: &str) -> std::result::Result<(PathBuf, Option<PathBuf>), String> {
        let path = uri_to_path(uri).ok_or_else(|| "invalid file uri".to_string())?;
        let text = self.docs.read().await.get(uri).cloned();
        if let Some(text) = text {
            let dir = path.parent().ok_or_else(|| "invalid file uri".to_string())?;
            let filename = ".now-playing".to_string();
            let tmp_path = dir.join(filename);
            std::fs::write(&tmp_path, text).map_err(|e| format!("failed to write temp file: {e}"))?;
            return Ok((tmp_path.clone(), Some(tmp_path)));
        }
        Ok((path, None))
    }
    async fn resolve_uri_arg(&self, args: &[Value]) -> std::result::Result<String, String> {
        if let Some(Value::String(uri)) = args.get(0) {
            return Ok(uri.clone());
        }
        if let Some(uri) = self.last_doc.read().await.clone() {
            return Ok(uri);
        }
        Err("no active document".to_string())
    }

    async fn command_path(&self) -> std::result::Result<String, String> {
        if let Some(path) = self.config.read().await.command_path.clone() {
            if let Some(existing) = resolve_existing_command(&path) {
                return Ok(existing);
            }
        }
        if let Ok(path) = env::var("CTRMML_CMD_PATH") {
            if !path.is_empty() {
                if let Some(existing) = resolve_existing_command(&path) {
                    return Ok(existing);
                }
            }
        }
        if let Some(path) = which_in_path(CTRMML_CMD_NAME) {
            return Ok(path);
        }
        match download_ctrmml_cmd().await {
            Ok(Some(path)) => Ok(path.to_string_lossy().to_string()),
            Ok(None) => Ok(CTRMML_CMD_NAME.to_string()),
            Err(err) => Err(err),
        }
    }

    async fn start_playback(&self, uri: String, start: Option<(u32, u32)>) -> std::result::Result<(), String> {
        self.stop_playback().await;

        let (file_path, temp_path) = self.materialize_mml(&uri).await?;
        let cmd_path = self.command_path().await?;
        let mut cmd = TokioCommand::new(cmd_path);
        cmd.arg("play").arg(file_path).arg("--follow");
        if let Some((line, col)) = start {
            cmd.arg("--start").arg(format!("{line}:{col}"));
        }
        cmd.stdout(std::process::Stdio::piped());

        let mut child = cmd
            .spawn()
            .map_err(|e| format!("failed to spawn ctrmml-cmd: {e}"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "failed to capture ctrmml-cmd stdout".to_string())?;

        let token = {
            let mut seq = self.playback_seq.lock().await;
            *seq += 1;
            *seq
        };

        {
            let mut slot = self.playback.lock().await;
            *slot = Some(Playback {
                uri: uri.clone(),
                child,
                temp_path,
            });
        }

        let client = self.client.clone();
        let docs = self.docs.clone();
        let seq = self.playback_seq.clone();
        let uri_clone = uri.clone();
        tokio::spawn(async move {
            let mut reader = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = reader.next_line().await {
                if *seq.lock().await != token {
                    break;
                }
                let msg = match serde_json::from_str::<HighlightMessage>(&line) {
                    Ok(msg) => msg,
                    Err(_) => continue,
                };
                if msg.kind != "highlight" {
                    continue;
                }

                let text = docs
                    .read()
                    .await
                    .get(&uri_clone)
                    .cloned()
                    .or_else(|| read_file_text(&uri_clone))
                    .unwrap_or_default();
                let diags = diagnostics_for_positions(&text, &msg.positions);
                if let Ok(uri) = uri_clone.parse() {
                    let _ = client.publish_diagnostics(uri, diags, None).await;
                }
            }

            if *seq.lock().await == token {
                if let Ok(uri) = uri_clone.parse() {
                    let _ = client.publish_diagnostics(uri, Vec::new(), None).await;
                }
            }
        });

        Ok(())
    }

    async fn stop_playback(&self) {
        {
            let mut seq = self.playback_seq.lock().await;
            *seq += 1;
        }
        let mut slot = self.playback.lock().await;
        if let Some(mut playback) = slot.take() {
            let _ = playback.child.kill().await;
            if let Some(path) = playback.temp_path {
                let _ = std::fs::remove_file(path);
            }
            if let Ok(uri) = playback.uri.parse() {
                let _ = self.client.publish_diagnostics(uri, Vec::new(), None).await;
            }
        }
    }

    async fn run_export(&self, uri: String, format: ExportFormat) -> std::result::Result<(), String> {
        let original_path = uri_to_path(&uri).ok_or_else(|| "invalid file uri".to_string())?;
        let (file_path, temp_path) = self.materialize_mml(&uri).await?;
        let out_path = match format {
            ExportFormat::Vgm => original_path.with_extension("vgm"),
            ExportFormat::Wav => original_path.with_extension("wav"),
        };

        let cmd_path = self.command_path().await?;
        let mut cmd = TokioCommand::new(cmd_path);
        cmd.arg("export").arg(file_path);
        match format {
            ExportFormat::Vgm => cmd.arg("--vgm"),
            ExportFormat::Wav => cmd.arg("--wav"),
        };
        cmd.arg("--out").arg(out_path);

        let status = cmd
            .status()
            .await
            .map_err(|e| format!("failed to run ctrmml-cmd: {e}"))?;
        if let Some(path) = temp_path {
            let _ = std::fs::remove_file(path);
        }
        if !status.success() {
            return Err("ctrmml-cmd export failed".to_string());
        }
        Ok(())
    }
}


#[derive(Deserialize)]
struct GithubRelease {
    tag_name: String,
    assets: Vec<GithubAsset>,
}

#[derive(Deserialize)]
struct GithubAsset {
    name: String,
    browser_download_url: String,
}

fn which_in_path(cmd: &str) -> Option<String> {
    let path_var = env::var_os("PATH")?;
    for entry in env::split_paths(&path_var) {
        let candidate = entry.join(cmd);
        if candidate.is_file() {
            return Some(candidate.to_string_lossy().to_string());
        }
        if cfg!(windows) {
            let candidate = entry.join(format!("{cmd}.exe"));
            if candidate.is_file() {
                return Some(candidate.to_string_lossy().to_string());
            }
        }
    }
    None
}

fn cache_base_dir() -> PathBuf {
    cache_dir().unwrap_or_else(env::temp_dir).join("ctrmml-cmd")
}

fn resolve_existing_command(path: &str) -> Option<String> {
    let candidate = Path::new(path);
    if candidate.is_file() {
        return Some(candidate.to_string_lossy().to_string());
    }
    if cfg!(windows) && !path.to_ascii_lowercase().ends_with(".exe") {
        let with_exe = candidate.with_extension("exe");
        if with_exe.is_file() {
            return Some(with_exe.to_string_lossy().to_string());
        }
    }
    None
}

fn platform_asset_parts() -> std::result::Result<(String, String, String), String> {
    let os = match env::consts::OS {
        "macos" => "macos",
        "linux" => "linux",
        "windows" => "windows",
        other => return Err(format!("unsupported platform {other}")),
    };
    let arch = match env::consts::ARCH {
        "aarch64" => "arm64",
        "x86_64" => "x64",
        other => return Err(format!("unsupported architecture {other}")),
    };
    let ext = if os == "windows" { "zip" } else { "tar.gz" };
    Ok((os.to_string(), arch.to_string(), ext.to_string()))
}

fn extract_zip(archive_path: &Path, out_dir: &Path) -> std::result::Result<(), String> {
    let file = fs::File::open(archive_path)
        .map_err(|e| format!("failed to open zip: {e}"))?;
    let mut zip = ZipArchive::new(file).map_err(|e| format!("invalid zip: {e}"))?;
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i).map_err(|e| format!("zip read failed: {e}"))?;
        let out_path = out_dir.join(entry.name());
        if entry.is_dir() {
            fs::create_dir_all(&out_path)
                .map_err(|e| format!("failed to create dir: {e}"))?;
        } else {
            if let Some(parent) = out_path.parent() {
                fs::create_dir_all(parent)
                    .map_err(|e| format!("failed to create dir: {e}"))?;
            }
            let mut out = fs::File::create(&out_path)
                .map_err(|e| format!("failed to create file: {e}"))?;
            io::copy(&mut entry, &mut out)
                .map_err(|e| format!("failed to extract file: {e}"))?;
        }
    }
    Ok(())
}

fn extract_targz(archive_path: &Path, out_dir: &Path) -> std::result::Result<(), String> {
    let file = fs::File::open(archive_path)
        .map_err(|e| format!("failed to open tar.gz: {e}"))?;
    let decoder = GzDecoder::new(file);
    let mut archive = Archive::new(decoder);
    archive
        .unpack(out_dir)
        .map_err(|e| format!("failed to unpack tar.gz: {e}"))?;
    Ok(())
}

#[cfg(unix)]
fn make_executable(path: &Path) -> std::result::Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = fs::metadata(path)
        .map_err(|e| format!("failed to read permissions: {e}"))?
        .permissions();
    perms.set_mode(0o755);
    fs::set_permissions(path, perms)
        .map_err(|e| format!("failed to set permissions: {e}"))?;
    Ok(())
}

#[cfg(not(unix))]
fn make_executable(_path: &Path) -> std::result::Result<(), String> {
    Ok(())
}

async fn download_ctrmml_cmd() -> std::result::Result<Option<PathBuf>, String> {
    let (os, arch, ext) = platform_asset_parts()?;
    let client = HttpClient::builder()
        .user_agent("ctrmml-lsp")
        .build()
        .map_err(|e| format!("failed to build http client: {e}"))?;
    let url = format!("https://api.github.com/repos/{CTRMML_CMD_REPO}/releases/latest");
    let release = client
        .get(url)
        .send()
        .await
        .map_err(|e| format!("failed to fetch release: {e}"))?
        .error_for_status()
        .map_err(|e| format!("failed to fetch release: {e}"))?
        .json::<GithubRelease>()
        .await
        .map_err(|e| format!("failed to parse release: {e}"))?;

    let version = release.tag_name.trim_start_matches('v');
    let asset_name = format!(
        "{name}-{version}-{os}-{arch}.{ext}",
        name = CTRMML_CMD_NAME,
        version = version,
        os = os,
        arch = arch,
        ext = ext,
    );
    let asset = release
        .assets
        .iter()
        .find(|asset| asset.name == asset_name)
        .ok_or_else(|| format!("no asset found matching {asset_name}"))?;

    let base = cache_base_dir();
    let version_dir = base.join(format!("{CTRMML_CMD_NAME}-{}", release.tag_name));
    let bin_name = if os == "windows" {
        format!("{CTRMML_CMD_NAME}.exe")
    } else {
        CTRMML_CMD_NAME.to_string()
    };
    let bin_path = version_dir.join(&bin_name);
    if bin_path.is_file() {
        return Ok(Some(bin_path));
    }

    fs::create_dir_all(&version_dir)
        .map_err(|e| format!("failed to create cache dir: {e}"))?;

    let tmp_path = version_dir.join(format!("download.{ext}"));
    let bytes = client
        .get(&asset.browser_download_url)
        .send()
        .await
        .map_err(|e| format!("failed to download asset: {e}"))?
        .error_for_status()
        .map_err(|e| format!("failed to download asset: {e}"))?
        .bytes()
        .await
        .map_err(|e| format!("failed to read asset: {e}"))?;
    fs::write(&tmp_path, &bytes).map_err(|e| format!("failed to write asset: {e}"))?;

    if ext == "zip" {
        extract_zip(&tmp_path, &version_dir)?;
    } else {
        extract_targz(&tmp_path, &version_dir)?;
    }
    let _ = fs::remove_file(&tmp_path);

    if !bin_path.is_file() {
        return Err(format!("ctrmml-cmd binary not found after extracting {asset_name}"));
    }
    make_executable(&bin_path)?;
    Ok(Some(bin_path))
}


fn lsp_err(err: impl Into<String>) -> tower_lsp::jsonrpc::Error {
    tower_lsp::jsonrpc::Error::invalid_params(err.into())
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

fn config_from_value(value: &Value) -> Option<Config> {
    let obj = value.as_object()?;
    let command_path = obj
        .get("command_path")
        .or_else(|| obj.get("commandPath"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    Some(Config { command_path })
}

fn is_mml_uri(uri: &str) -> bool {
    if let Some(path) = uri_to_path(uri) {
        if let Some(ext) = path.extension().and_then(|ext| ext.to_str()) {
            return ext.eq_ignore_ascii_case("mml");
        }
    }
    false
}

fn diagnostics_for_positions(text: &str, positions: &[HighlightPosition]) -> Vec<Diagnostic> {
    let lines: Vec<&str> = text.lines().collect();
    let mut seen = HashSet::new();
    let mut out = Vec::new();

    for pos in positions {
        let line = pos.line as usize;
        if line >= lines.len() {
            continue;
        }
        let line_len = lines[line].len() as u32;
        let mut col = pos.col;
        if col > line_len {
            col = line_len;
        }
        let end = (col + 1).min(line_len);
        let key = (pos.line as u64) << 32 | pos.col as u64;
        if !seen.insert(key) {
            continue;
        }
        out.push(Diagnostic {
            range: Range {
                start: Position::new(pos.line, col),
                end: Position::new(pos.line, end),
            },
            severity: Some(DiagnosticSeverity::HINT),
            source: Some("ctrmml-playback".to_string()),
            message: "playback".to_string(),
            ..Diagnostic::default()
        });
    }

    out
}

fn read_file_text(uri: &str) -> Option<String> {
    let path = uri_to_path(uri)?;
    std::fs::read_to_string(path).ok()
}

fn line_at(text: &str, line_index: u32) -> Option<String> {
    text.lines().nth(line_index as usize).map(|s| s.to_string())
}

fn documented_item(label: &str, kind: CompletionItemKind, doc: Option<&'static str>) -> CompletionItem {
    CompletionItem {
        label: label.to_string(),
        kind: Some(kind),
        documentation: doc.map(|text| Documentation::String(text.to_string())),
        ..CompletionItem::default()
    }
}

fn meta_item(label: &str) -> CompletionItem {
    let doc = match label {
        "#title" | "#composer" | "#author" | "#date" | "#comment" => Some("Song metadata."),
        "#platform" => Some("Sets the MML target platform."),
        "#option" => Some("Sets platform options."),
        _ => None,
    };
    documented_item(label, CompletionItemKind::KEYWORD, doc)
}

fn meta_completion_items(line: &str, col: usize, line_index: u32) -> Vec<CompletionItem> {
    let start_col = meta_prefix_start_col(line, col);
    let range = Range {
        start: Position::new(line_index, start_col),
        end: Position::new(line_index, col as u32),
    };

    META_KEYWORDS
        .iter()
        .map(|kw| {
            let insert = kw.strip_prefix('#').unwrap_or(kw);
            let mut item = meta_item(kw);
            let edit = tower_lsp::lsp_types::TextEdit {
                range,
                new_text: insert.to_string(),
            };
            item.text_edit = Some(tower_lsp::lsp_types::CompletionTextEdit::Edit(edit));
            item.insert_text = Some(insert.to_string());
            item.filter_text = Some(kw.to_string());
            item
        })
        .collect()
}

fn meta_prefix_start_col(line: &str, col: usize) -> u32 {
    let prefix = match line.get(..col) {
        Some(text) => text,
        None => return col as u32,
    };
    prefix.rfind('#').map(|idx| (idx + 1) as u32).unwrap_or(col as u32)
}

fn platform_item(label: &str) -> CompletionItem {
    let doc = match label {
        "megadrive" => Some(
            "Use VGM datablocks and DAC stream commands to play back samples.",
        ),
        "mdsdrv" => Some(
            "Simulate MDSDRV's PCM driver (2-3 channel mixing). Sample rate is fixed to ~2 kHz steps.",
        ),
        _ => None,
    };
    documented_item(label, CompletionItemKind::KEYWORD, doc)
}

fn instrument_item(label: &str) -> CompletionItem {
    let doc = match label {
        "fm" => Some("FM instruments are defined as below."),
        "2op" => Some(
            "Instrument type `2op` is used to duplicate FM instruments, modifying the operators' multiply ratios and setting a transpose.",
        ),
        "psg" => Some("PSG instruments (envelopes) are defined as a sequence of values."),
        "pcm" => Some(
            "PCM samples are defined as instruments. The first parameter is the path to the sample (relative to that of the MML file).",
        ),
        _ => None,
    };
    documented_item(label, CompletionItemKind::TYPE_PARAMETER, doc)
}

fn rate_offset_item(label: &str) -> CompletionItem {
    let doc = match label {
        "rate=" => Some("Override the sample rate."),
        "offset=" => Some("Adjust the start position."),
        _ => None,
    };
    documented_item(label, CompletionItemKind::PROPERTY, doc)
}

fn command_item(label: &str) -> CompletionItem {
    let doc = match label {
        "o" => Some("Set octave."),
        "l" => Some("Set default duration, used if not specified by notes, rests, `R` or `~` commands."),
        "Q" => Some("Quantize. Used to set articulation. Note length is param/8."),
        "q" => Some("Set early release. Used to set articulation."),
        "C" => Some("Set the length of a measure (or a whole note) in ticks."),
        "R" => Some("Reverse rest. This subtracts the value from the previous note or rest."),
        "L" => Some("Set loop point (segno). If this is present, playback resumes at this point when the end of the track is reached."),
        "s" => Some("Set shuffle. The specified number of ticks will be added to the the next note, rest or tie, then subtracted from the next."),
        "t" => Some("Set tempo in BPM."),
        "T" => Some("Set tempo using the platform's native timer values."),
        "v" => Some("Set volume."),
        "V" => Some("Set volume (fine), or modify volume (fine) depending on parameter range."),
        "p" => Some("Set panning."),
        "k" => Some("Set transpose. Default behavior is the same as the `_` command."),
        "K" => Some("Set detune."),
        "E" => Some("Set envelope. 0 to disable."),
        "M" => Some("Set pitch envelope. 0 to disable."),
        "P" => Some("Set pan envelope or macro track. 0 to disable."),
        "G" => Some("Set portamento. 0 to disable."),
        "D" => Some("Set drum mode. 0 disables drum mode."),
        "r" => Some("Rest. Optionally set duration after the rest."),
        "^" => Some("Tie. Extends duration of previous note."),
        "&" => Some("Slur. Used to connect two notes (legato)."),
        _ => None,
    };
    documented_item(label, CompletionItemKind::KEYWORD, doc)
}


fn at_meta_completion_items(line: &str, col: usize, line_index: u32) -> Vec<CompletionItem> {
    let start_col = at_prefix_start_col(line, col);
    let range = Range {
        start: Position::new(line_index, start_col),
        end: Position::new(line_index, col as u32),
    };
    vec![
        at_meta_item(
            "@<num>",
            "Defines an instrument. Parameters are platform-specific.",
            "${1:num}",
            range,
        ),
        at_meta_item("@E<num>", "Defines an envelope.", "E${1:num}", range),
        at_meta_item("@M<num>", "Defines a pitch envelope.", "M${1:num}", range),
        at_meta_item("@P<num>", "Defines a pan envelope.", "P${1:num}", range),
    ]
}

fn at_meta_item(label: &str, doc: &'static str, insert_text: &str, range: Range) -> CompletionItem {
    let edit = tower_lsp::lsp_types::TextEdit {
        range,
        new_text: insert_text.to_string(),
    };
    CompletionItem {
        label: label.to_string(),
        kind: Some(CompletionItemKind::KEYWORD),
        documentation: Some(Documentation::String(doc.to_string())),
        insert_text: Some(insert_text.to_string()),
        insert_text_format: Some(tower_lsp::lsp_types::InsertTextFormat::SNIPPET),
        text_edit: Some(tower_lsp::lsp_types::CompletionTextEdit::Edit(edit)),
        filter_text: Some(label.to_string()),
        ..CompletionItem::default()
    }
}

fn at_prefix_start_col(line: &str, col: usize) -> u32 {
    let prefix = match line.get(..col) {
        Some(text) => text,
        None => return col as u32,
    };
    prefix.rfind('@').map(|idx| (idx + 1) as u32).unwrap_or(col as u32)
}

fn is_at_meta_context(line: &str, col: usize) -> bool {
    let prefix = match line.get(..col) {
        Some(text) => text,
        None => return false,
    };
    let trimmed = prefix.trim_start();
    if !trimmed.starts_with('@') {
        return false;
    }
    if trimmed.chars().any(|ch| ch.is_whitespace()) {
        return false;
    }
    !trimmed.chars().skip(1).any(|ch| ch.is_ascii_digit())
}

fn complete_pcm_paths(
    line: &str,
    col: usize,
    uri: &str,
    roots: &[PathBuf],
    line_index: u32,
) -> Option<Vec<CompletionItem>> {
    let (prefix, start_col) = string_prefix(line, col)?;
    if !has_pcm_token_before(line, col) {
        return None;
    }

    let base_dir = uri_to_dir(uri)?;
    let mut search_roots = Vec::new();
    search_roots.push(base_dir.clone());
    search_roots.extend(roots.iter().cloned());
    let mut seen = HashSet::new();
    search_roots.retain(|path| seen.insert(path.clone()));
    let mut items = Vec::new();
    let mut seen_items = HashSet::new();

    for root in search_roots {
        for entry in WalkDir::new(&root).into_iter().filter_map(|e| e.ok()) {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            if !is_wav(path) {
                continue;
            }

            if let Some(rel) = diff_paths(path, &base_dir) {
                let rel_str = rel.to_string_lossy().replace('\\', "/");
                if !prefix.is_empty() && !rel_str.starts_with(&prefix) {
                    continue;
                }

                let suffix = match line.get(col..).and_then(|s| s.chars().next()) {
                    Some('"') => "",
                    _ => "\"",
                };
                let insert_text = format!("{rel_str}{suffix}");
                let edit = tower_lsp::lsp_types::TextEdit {
                    range: Range {
                        start: Position::new(line_index, start_col as u32),
                        end: Position::new(line_index, col as u32),
                    },
                    new_text: insert_text.clone(),
                };

                if seen_items.insert(rel_str.clone()) {
                    items.push(CompletionItem {
                        label: rel_str.clone(),
                        kind: Some(CompletionItemKind::FILE),
                        insert_text: Some(insert_text),
                        filter_text: Some(rel_str.clone()),
                        text_edit: Some(tower_lsp::lsp_types::CompletionTextEdit::Edit(edit)),
                        ..CompletionItem::default()
                    });
                }
            }
        }
    }

    Some(items)
}

fn string_prefix(line: &str, col: usize) -> Option<(String, usize)> {
    let before = line.get(..col)?;
    let quote_count = before.chars().filter(|c| *c == '"').count();
    if quote_count % 2 == 0 {
        return None;
    }

    let last_quote = before.rfind('"')? + 1;
    let prefix = before.get(last_quote..)?.to_string();
    Some((prefix, last_quote))
}

fn has_pcm_token_before(line: &str, col: usize) -> bool {
    line.get(..col)
        .map(|s| s.split_whitespace().any(|tok| tok == "pcm"))
        .unwrap_or(false)
}

fn is_instrument_definition_context(line: &str, col: usize) -> bool {
    let prefix = match line.get(..col) {
        Some(text) => text,
        None => return false,
    };
    let trimmed = prefix.trim_start();
    if !trimmed.starts_with('@') {
        return false;
    }
    let rest = &trimmed[1..];
    let digits_len = rest
        .chars()
        .take_while(|ch| ch.is_ascii_digit())
        .count();
    if digits_len == 0 {
        return false;
    }
    let after_digits = &rest[digits_len..];
    after_digits == " "
}

fn is_rate_offset_context(line: &str, col: usize) -> bool {
    let prefix = match line.get(..col) {
        Some(text) => text,
        None => return false,
    };

    if !prefix.ends_with(' ') {
        return false;
    }

    let tokens = tokenize_outside_quotes(prefix);
    if tokens.len() != 3 {
        return false;
    }

    if !is_at_number(&tokens[0]) {
        return false;
    }
    if tokens[1] != "pcm" {
        return false;
    }
    is_quoted(&tokens[2])
}

fn is_in_comment(line: &str, col: usize) -> bool {
    let prefix = match line.get(..col) {
        Some(text) => text,
        None => return false,
    };
    let mut in_string = false;
    for ch in prefix.chars() {
        if ch == '"' {
            in_string = !in_string;
            continue;
        }
        if ch == ';' && !in_string {
            return true;
        }
    }
    false
}

fn tokenize_outside_quotes(input: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut in_string = false;

    for ch in input.chars() {
        if ch == '"' {
            current.push(ch);
            in_string = !in_string;
            continue;
        }

        if ch.is_whitespace() && !in_string {
            if !current.is_empty() {
                tokens.push(current.clone());
                current.clear();
            }
            continue;
        }

        current.push(ch);
    }

    if !current.is_empty() {
        tokens.push(current);
    }

    tokens
}

fn is_at_number(token: &str) -> bool {
    let mut chars = token.chars();
    if chars.next() != Some('@') {
        return false;
    }
    let rest: String = chars.collect();
    !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit())
}

fn is_quoted(token: &str) -> bool {
    token.len() >= 2 && token.starts_with('"') && token.ends_with('"')
}

fn uri_to_path(uri: &str) -> Option<PathBuf> {
    let url = url::Url::parse(uri).ok()?;
    url.to_file_path().ok()
}

fn uri_to_dir(uri: &str) -> Option<PathBuf> {
    let path = uri_to_path(uri)?;
    path.parent().map(|p| p.to_path_buf())
}

fn is_wav(path: &std::path::Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.eq_ignore_ascii_case("wav"))
        .unwrap_or(false)
}

#[tokio::main]
async fn main() {
    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();

    let (service, socket) = LspService::new(|client| Backend {
        client: client,
        docs: Arc::new(RwLock::new(HashMap::new())),
        roots: Arc::new(RwLock::new(Vec::new())),
        config: Arc::new(RwLock::new(Config::default())),
        playback: Arc::new(Mutex::new(None)),
        playback_seq: Arc::new(Mutex::new(0)),
        last_doc: Arc::new(RwLock::new(None)),
    });
    Server::new(stdin, stdout, socket).serve(service).await;
}
