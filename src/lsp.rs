use serde_json::Value;
use tower_lsp::{
    jsonrpc::Result,
    lsp_types::{
        CodeActionOrCommand, CodeActionParams, CodeActionProviderCapability, CompletionList,
        CompletionOptions, CompletionParams, CompletionResponse, DidSaveTextDocumentParams,
        ExecuteCommandOptions,
        ExecuteCommandParams, GotoDefinitionParams, GotoDefinitionResponse, Hover, HoverContents,
        HoverParams, HoverProviderCapability, InitializeParams, InitializeResult, Location,
        MarkupContent, MarkupKind, MessageActionItem, MessageType, OneOf, Position, Range,
        SaveOptions, ServerCapabilities, TextDocumentSyncCapability, TextDocumentSyncKind,
        TextDocumentSyncOptions, TextDocumentSyncSaveOptions,
    },
    LanguageServer,
};

use crate::backend::Backend;
use crate::chord_completion::chord_completion_items;
use crate::fill_measure::fill_measure_code_action;
use crate::note_hover::note_hover_text;
use crate::completion::{
    at_meta_completion_items, command_items, complete_pcm_paths, fm_instrument_context,
    instrument_items, is_at_meta_context, is_instrument_definition_context,
    is_meta_keyword_context, is_meta_value_context, is_platform_command_context,
    is_rate_offset_context, meta_completion_items, option_items, platform_command_items,
    platform_items, rate_offset_items,
};
use crate::config::config_from_value;
use crate::export::ExportFormat;
use ctrmml_lang_core::hover_at;
use crate::lsp_commands::{
    code_actions, command_ids, transpose_code_action, CMD_EXPORT_VGM, CMD_EXPORT_WAV,
    CMD_MDSLINK_DIRECTORY, CMD_MDSLINK_FILE, CMD_MDSLINK_FROM_CONFIG, CMD_MDSLINK_MENU, CMD_PLAY,
    CMD_PLAY_FROM_CURSOR, CMD_QUICKROM_DIRECTORY, CMD_QUICKROM_FILE, CMD_QUICKROM_FROM_CONFIG,
    CMD_QUICKROM_MENU, CMD_STOP,
};
use ctrmml_lang_core::transpose::Direction;
use crate::mdslink::MdslinkRunResult;
use crate::mml::{is_in_comment, token_at};
use crate::quickrom::QuickromRunResult;
use crate::utils::{is_mml_uri, line_at};

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

        if let Some(info) = &params.client_info {
            let name = info.name.to_lowercase();
            *self.supports_hierarchy.write().await =
                name.contains("visual studio code") || name.contains("vscode");
        }

        Ok(InitializeResult {
            capabilities: ServerCapabilities {
                text_document_sync: Some(TextDocumentSyncCapability::Options(
                    TextDocumentSyncOptions {
                        open_close: Some(true),
                        change: Some(TextDocumentSyncKind::FULL),
                        save: Some(TextDocumentSyncSaveOptions::SaveOptions(SaveOptions {
                            include_text: Some(false),
                        })),
                        ..TextDocumentSyncOptions::default()
                    },
                )),
                hover_provider: Some(HoverProviderCapability::Simple(true)),
                completion_provider: Some(CompletionOptions {
                    resolve_provider: Some(false),
                    trigger_characters: Some(vec![
                        "#".into(),
                        "@".into(),
                        "\"".into(),
                        " ".into(),
                        "'".into(),
                        "/".into(),
                        ".".into(),
                        "{".into(),
                        "+".into(),
                        "-".into(),
                        "=".into(),
                    ]),
                    ..CompletionOptions::default()
                }),
                definition_provider: Some(OneOf::Left(true)),
                execute_command_provider: Some(ExecuteCommandOptions {
                    commands: command_ids(),
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
        *self.last_doc.write().await = Some(uri.clone());
        let _ = self.run_check(uri).await;
    }

    async fn did_change(&self, params: tower_lsp::lsp_types::DidChangeTextDocumentParams) {
        let uri = params.text_document.uri.to_string();
        if let Some(change) = params.content_changes.into_iter().last() {
            self.docs.write().await.insert(uri.clone(), change.text);
            *self.last_doc.write().await = Some(uri.clone());
        }
        let _ = self.run_check(uri).await;
    }

    async fn did_save(&self, params: DidSaveTextDocumentParams) {
        let uri = params.text_document.uri.to_string();
        let _ = self.run_check(uri).await;
    }

    async fn hover(&self, params: HoverParams) -> Result<Option<Hover>> {
        let uri = params
            .text_document_position_params
            .text_document
            .uri
            .to_string();
        let position = params.text_document_position_params.position;
        let text = self
            .docs
            .read()
            .await
            .get(&uri)
            .cloned()
            .unwrap_or_default();
        let line = line_at(&text, position.line).unwrap_or_default();

        // Generic hover (commands, key sigs, platform commands, FM/2op/PSG
        // params, instrument definitions, …) lives in `ctrmml-lang-core` so
        // web-ctrmml, vscode-ctrmml, and zed-ctrmml all share the same
        // behaviour. It already short-circuits on comments.
        if let Some(info) = hover_at(&text, position.line, position.character) {
            return Ok(Some(Hover {
                contents: HoverContents::Markup(MarkupContent {
                    kind: MarkupKind::Markdown,
                    value: info.markdown,
                }),
                range: Some(Range {
                    start: Position::new(info.line, info.start),
                    end: Position::new(info.line, info.end),
                }),
            }));
        }

        // Last layer: note hover. Resolves the absolute MIDI pitch
        // plus the ambient key-sig context for a note letter at the
        // cursor. Suppressed inside FM/PSG blocks and outside any
        // track selector.
        if let Some((value, start, end)) =
            note_hover_text(&text, position.line, position.character, &line)
        {
            return Ok(Some(Hover {
                contents: HoverContents::Markup(MarkupContent {
                    kind: MarkupKind::Markdown,
                    value,
                }),
                range: Some(Range {
                    start: Position::new(position.line, start as u32),
                    end: Position::new(position.line, end as u32),
                }),
            }));
        }

        Ok(None)
    }

    async fn goto_definition(
        &self,
        params: GotoDefinitionParams,
    ) -> Result<Option<GotoDefinitionResponse>> {
        let uri = params
            .text_document_position_params
            .text_document
            .uri
            .to_string();
        if !is_mml_uri(&uri) {
            return Ok(None);
        }
        let position = params.text_document_position_params.position;
        let text = self
            .docs
            .read()
            .await
            .get(&uri)
            .cloned()
            .unwrap_or_default();
        let line = line_at(&text, position.line).unwrap_or_default();
        let col = position.character as usize;
        if is_in_comment(&line, col) {
            return Ok(None);
        }

        let target = match definition_target_at(&line, col) {
            Some(value) => value,
            None => return Ok(None),
        };

        let range = match target {
            DefinitionTarget::Instrument(num) => find_instrument_definition(&text, &num),
            DefinitionTarget::AtMeta { prefix, num } => {
                find_prefixed_definition(&text, prefix, &num)
            }
            DefinitionTarget::Track(num) => find_track_definition(&text, &num),
        };

        if let Some(range) = range {
            let location = Location {
                uri: params.text_document_position_params.text_document.uri,
                range,
            };
            return Ok(Some(GotoDefinitionResponse::Scalar(location)));
        }

        Ok(None)
    }

    async fn completion(&self, params: CompletionParams) -> Result<Option<CompletionResponse>> {
        let uri = params.text_document_position.text_document.uri.to_string();
        let position = params.text_document_position.position;
        let text = self
            .docs
            .read()
            .await
            .get(&uri)
            .cloned()
            .unwrap_or_default();

        let line = line_at(&text, position.line).unwrap_or_default();
        let col = position.character as usize;
        if is_in_comment(&line, col) {
            return Ok(None);
        }

        if is_platform_command_context(&line, col) {
            let items = platform_command_items(&line, col, position.line);
            return Ok(Some(CompletionResponse::Array(items)));
        }

        let roots = self.roots.read().await.clone();
        if let Some(items) = complete_pcm_paths(&line, col, &uri, &roots, position.line) {
            return Ok(Some(CompletionResponse::Array(items)));
        }

        if is_meta_value_context(&line, col, "#platform") {
            return Ok(Some(CompletionResponse::Array(platform_items())));
        }

        if is_meta_value_context(&line, col, "#option") {
            return Ok(Some(CompletionResponse::Array(option_items())));
        }

        if is_meta_keyword_context(&line, col) {
            let items = meta_completion_items(&line, col, position.line);
            return Ok(Some(CompletionResponse::Array(items)));
        }

        if line.trim_start().starts_with('#') {
            return Ok(None);
        }

        if let Some(fm_kind) = fm_instrument_context(&line, col) {
            if let Ok(items) = self
                .complete_fm_instruments(
                    &uri,
                    &roots,
                    &fm_kind,
                    position.line,
                    position.character,
                )
                .await
            {
                if !items.is_empty() {
                    return Ok(Some(CompletionResponse::Array(items)));
                }
            }
        }

        if is_rate_offset_context(&line, col) {
            return Ok(Some(CompletionResponse::Array(rate_offset_items())));
        }

        if is_instrument_definition_context(&line, col) {
            return Ok(Some(CompletionResponse::Array(instrument_items())));
        }

        if is_at_meta_context(&line, col) {
            let items = at_meta_completion_items(&line, col, position.line);
            return Ok(Some(CompletionResponse::Array(items)));
        }

        if line.trim_start().starts_with('@') {
            return Ok(None);
        }

        if let Some(items) =
            chord_completion_items(&text, position.line, position.character)
        {
            return Ok(Some(CompletionResponse::Array(items)));
        }
        // Mark the command fallback incomplete so editors re-query on each
        // keystroke instead of filtering this list locally. Without this,
        // typing `{c` would keep showing the cached `C<ticks>` command from
        // when the user first hit `{` (the chord context wasn't satisfied
        // yet) — and `{a` would show nothing at all, since no command starts
        // with `a`. Matches web-ctrmml's `incomplete: true` (mml-completions.ts).
        Ok(Some(CompletionResponse::List(CompletionList {
            is_incomplete: true,
            items: command_items(),
        })))
    }

    async fn code_action(
        &self,
        params: CodeActionParams,
    ) -> Result<Option<Vec<CodeActionOrCommand>>> {
        let uri_str = params.text_document.uri.to_string();
        if !is_mml_uri(&uri_str) {
            return Ok(None);
        }
        let start = params.range.start;
        let mut actions = code_actions(&uri_str, start);

        // Document-aware actions: transpose and fill-measure both
        // need the live document text. Take a single snapshot for
        // both rather than re-locking docs twice.
        let doc_text = self.docs.read().await.get(&uri_str).cloned();
        if let Some(text) = doc_text {
            // Transpose only makes sense for a non-empty selection.
            if params.range.start != params.range.end {
                for direction in [Direction::Up, Direction::Down] {
                    if let Some(action) = transpose_code_action(
                        &params.text_document.uri,
                        params.range,
                        &text,
                        direction,
                    ) {
                        actions.push(action);
                    }
                }
            }
            // Fill-measure spawns a `ctrmml-cmd find-cursor-tick`
            // subprocess to compute the cursor's playback tick;
            // pre-checks inside the function gate that so the typical
            // outside-track / inside-FM case skips the spawn.
            if let Ok(cmd_path) = self.command_path().await {
                if let Some(action) = fill_measure_code_action(
                    &cmd_path,
                    &params.text_document.uri,
                    &text,
                    params.range.start.line,
                    params.range.start.character,
                )
                .await
                {
                    actions.push(action);
                }
            }
        }

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
            CMD_PLAY => {
                let uri = match self.resolve_uri_arg(&args).await {
                    Ok(uri) => uri,
                    Err(err) => {
                        let _ = self.client.show_message(MessageType::ERROR, err).await;
                        return Ok(None);
                    }
                };
                if let Err(err) = self.start_playback(uri, None).await {
                    let _ = self.client.show_message(MessageType::ERROR, err).await;
                }
            }
            CMD_PLAY_FROM_CURSOR => {
                let uri = match self.resolve_uri_arg(&args).await {
                    Ok(uri) => uri,
                    Err(err) => {
                        let _ = self.client.show_message(MessageType::ERROR, err).await;
                        return Ok(None);
                    }
                };
                let line = args.get(1).and_then(|v| v.as_u64()).unwrap_or(0) as u32;
                let col = args.get(2).and_then(|v| v.as_u64()).unwrap_or(0) as u32;
                if let Err(err) = self.start_playback(uri, Some((line, col))).await {
                    let _ = self.client.show_message(MessageType::ERROR, err).await;
                }
            }
            CMD_STOP => {
                self.stop_playback().await;
            }
            CMD_EXPORT_VGM => {
                let uri = match self.resolve_uri_arg(&args).await {
                    Ok(uri) => uri,
                    Err(err) => {
                        let _ = self.client.show_message(MessageType::ERROR, err).await;
                        return Ok(None);
                    }
                };
                match self.run_export(uri, ExportFormat::Vgm).await {
                    Ok(path) => {
                        let roots = self.roots.read().await.clone();
                        let display = relative_path_display(&path, &roots);
                        let _ = self
                            .client
                            .show_message(MessageType::INFO, format!("exported {display}"))
                            .await;
                    }
                    Err(err) => {
                        let _ = self.client.show_message(MessageType::ERROR, err).await;
                    }
                }
            }
            CMD_EXPORT_WAV => {
                let uri = match self.resolve_uri_arg(&args).await {
                    Ok(uri) => uri,
                    Err(err) => {
                        let _ = self.client.show_message(MessageType::ERROR, err).await;
                        return Ok(None);
                    }
                };
                match self.run_export(uri, ExportFormat::Wav).await {
                    Ok(path) => {
                        let roots = self.roots.read().await.clone();
                        let display = relative_path_display(&path, &roots);
                        let _ = self
                            .client
                            .show_message(MessageType::INFO, format!("exported {display}"))
                            .await;
                    }
                    Err(err) => {
                        let _ = self.client.show_message(MessageType::ERROR, err).await;
                    }
                }
            }
            CMD_MDSLINK_FILE => {
                let uri = match self.resolve_uri_arg(&args).await {
                    Ok(uri) => uri,
                    Err(err) => {
                        let _ = self.client.show_message(MessageType::ERROR, err).await;
                        return Ok(None);
                    }
                };
                run_mdslink_command(self, CMD_MDSLINK_FILE, uri).await;
            }
            CMD_MDSLINK_DIRECTORY => {
                let uri = match self.resolve_uri_arg(&args).await {
                    Ok(uri) => uri,
                    Err(err) => {
                        let _ = self.client.show_message(MessageType::ERROR, err).await;
                        return Ok(None);
                    }
                };
                run_mdslink_command(self, CMD_MDSLINK_DIRECTORY, uri).await;
            }
            CMD_MDSLINK_FROM_CONFIG => {
                let uri = match self.resolve_uri_arg(&args).await {
                    Ok(uri) => uri,
                    Err(err) => {
                        let _ = self.client.show_message(MessageType::ERROR, err).await;
                        return Ok(None);
                    }
                };
                run_mdslink_command(self, CMD_MDSLINK_FROM_CONFIG, uri).await;
            }
            CMD_MDSLINK_MENU => {
                let uri = match self.resolve_uri_arg(&args).await {
                    Ok(uri) => uri,
                    Err(err) => {
                        let _ = self.client.show_message(MessageType::ERROR, err).await;
                        return Ok(None);
                    }
                };
                if let Some(command) = select_menu_command(
                    self,
                    "mdslink",
                    &[
                        ("mdslink file", CMD_MDSLINK_FILE),
                        ("mdslink directory", CMD_MDSLINK_DIRECTORY),
                        ("mdslink from mdslink.json", CMD_MDSLINK_FROM_CONFIG),
                    ],
                )
                .await
                {
                    run_mdslink_command(self, command, uri).await;
                }
            }
            CMD_QUICKROM_FILE => {
                let uri = match self.resolve_uri_arg(&args).await {
                    Ok(uri) => uri,
                    Err(err) => {
                        let _ = self.client.show_message(MessageType::ERROR, err).await;
                        return Ok(None);
                    }
                };
                run_quickrom_command(self, CMD_QUICKROM_FILE, uri).await;
            }
            CMD_QUICKROM_DIRECTORY => {
                let uri = match self.resolve_uri_arg(&args).await {
                    Ok(uri) => uri,
                    Err(err) => {
                        let _ = self.client.show_message(MessageType::ERROR, err).await;
                        return Ok(None);
                    }
                };
                run_quickrom_command(self, CMD_QUICKROM_DIRECTORY, uri).await;
            }
            CMD_QUICKROM_FROM_CONFIG => {
                let uri = match self.resolve_uri_arg(&args).await {
                    Ok(uri) => uri,
                    Err(err) => {
                        let _ = self.client.show_message(MessageType::ERROR, err).await;
                        return Ok(None);
                    }
                };
                run_quickrom_command(self, CMD_QUICKROM_FROM_CONFIG, uri).await;
            }
            CMD_QUICKROM_MENU => {
                let uri = match self.resolve_uri_arg(&args).await {
                    Ok(uri) => uri,
                    Err(err) => {
                        let _ = self.client.show_message(MessageType::ERROR, err).await;
                        return Ok(None);
                    }
                };
                if let Some(command) = select_menu_command(
                    self,
                    "quickrom",
                    &[
                        ("quickrom file", CMD_QUICKROM_FILE),
                        ("quickrom directory", CMD_QUICKROM_DIRECTORY),
                        ("quickrom from quickrom.json", CMD_QUICKROM_FROM_CONFIG),
                    ],
                )
                .await
                {
                    run_quickrom_command(self, command, uri).await;
                }
            }
            _ => {}
        }
        Ok(None)
    }

    async fn shutdown(&self) -> Result<()> {
        Ok(())
    }
}

async fn select_menu_command(
    backend: &Backend,
    menu_name: &str,
    choices: &[(&'static str, &'static str)],
) -> Option<&'static str> {
    let actions: Vec<MessageActionItem> = choices
        .iter()
        .map(|(title, _)| MessageActionItem {
            title: (*title).to_string(),
            properties: Default::default(),
        })
        .collect();
    let selected = match backend
        .client
        .show_message_request(
            MessageType::INFO,
            format!("ctrmml: {menu_name}"),
            Some(actions),
        )
        .await
    {
        Ok(item) => item,
        Err(err) => {
            let _ = backend
                .client
                .show_message(
                    MessageType::ERROR,
                    format!("failed to open {menu_name} menu: {err}"),
                )
                .await;
            return None;
        }
    }?;

    choices
        .iter()
        .find(|(title, _)| *title == selected.title.as_str())
        .map(|(_, command)| *command)
}

async fn run_mdslink_command(backend: &Backend, command: &str, uri: String) {
    let result = match command {
        CMD_MDSLINK_FILE => backend.run_mdslink_single(uri).await,
        CMD_MDSLINK_DIRECTORY => backend.run_mdslink_directory(uri).await,
        CMD_MDSLINK_FROM_CONFIG => backend.run_mdslink_config(uri).await,
        _ => {
            let _ = backend
                .client
                .show_message(
                    MessageType::ERROR,
                    format!("unsupported mdslink command: {command}"),
                )
                .await;
            return;
        }
    };
    handle_mdslink_result(backend, result).await;
}

async fn run_quickrom_command(backend: &Backend, command: &str, uri: String) {
    let result = match command {
        CMD_QUICKROM_FILE => backend.run_quickrom_single(uri).await,
        CMD_QUICKROM_DIRECTORY => backend.run_quickrom_directory(uri).await,
        CMD_QUICKROM_FROM_CONFIG => backend.run_quickrom_config(uri).await,
        _ => {
            let _ = backend
                .client
                .show_message(
                    MessageType::ERROR,
                    format!("unsupported quickrom command: {command}"),
                )
                .await;
            return;
        }
    };
    handle_quickrom_result(backend, result).await;
}

async fn handle_mdslink_result(
    backend: &Backend,
    result: std::result::Result<MdslinkRunResult, String>,
) {
    match result {
        Ok(result) => {
            let roots = backend.roots.read().await.clone();
            let seq = relative_path_display(&result.outputs.seq_output, &roots);
            let pcm = relative_path_display(&result.outputs.pcm_output, &roots);
            let inc = relative_path_display(&result.outputs.asm_header_output, &roots);
            let header = relative_path_display(&result.outputs.c_header_output, &roots);
            let mut message = format!("mdslink outputs: {seq}, {pcm}, {inc}, {header}");
            if let Some(warning) = result.warning {
                message.push_str(";\n\n**warning**: ");
                message.push_str(&warning);
            }
            let _ = backend
                .client
                .show_message(MessageType::INFO, message)
                .await;
        }
        Err(err) => {
            let _ = backend.client.show_message(MessageType::ERROR, err).await;
        }
    }
}

async fn handle_quickrom_result(
    backend: &Backend,
    result: std::result::Result<QuickromRunResult, String>,
) {
    match result {
        Ok(result) => {
            let roots = backend.roots.read().await.clone();
            let rom = relative_path_display(&result.rom_output, &roots);
            let mut message = format!("quickrom output: {rom}");
            if let Some(warning) = result.warning {
                message.push_str(";\n\n**warning**: ");
                message.push_str(&warning);
            }
            let _ = backend
                .client
                .show_message(MessageType::INFO, message)
                .await;
        }
        Err(err) => {
            let _ = backend.client.show_message(MessageType::ERROR, err).await;
        }
    }
}

fn relative_path_display(path: &std::path::Path, roots: &[std::path::PathBuf]) -> String {
    if let Some(root) = best_workspace_root(path, roots) {
        if let Some(rel) = pathdiff::diff_paths(path, root) {
            return rel.to_string_lossy().to_string();
        }
    }
    path.to_string_lossy().to_string()
}

fn best_workspace_root(
    path: &std::path::Path,
    roots: &[std::path::PathBuf],
) -> Option<std::path::PathBuf> {
    let mut best: Option<std::path::PathBuf> = None;
    for root in roots {
        if path.starts_with(root) {
            let replace = match &best {
                Some(existing) => root.components().count() > existing.components().count(),
                None => true,
            };
            if replace {
                best = Some(root.clone());
            }
        }
    }
    best
}

enum DefinitionTarget {
    Instrument(String),
    AtMeta { prefix: &'static str, num: String },
    Track(String),
}

fn definition_target_at(line: &str, col: usize) -> Option<DefinitionTarget> {
    let (token, _start, _end) = token_at(line, col)?;
    let mut chars = token.chars();
    let first = chars.next()?;
    if first == '@' {
        let rest: String = chars.collect();
        if let Some(stripped) = rest.strip_prefix('E') {
            let digits = stripped
                .chars()
                .take_while(|ch| ch.is_ascii_digit())
                .collect::<String>();
            if !digits.is_empty() {
                return Some(DefinitionTarget::AtMeta {
                    prefix: "@E",
                    num: digits,
                });
            }
        }
        if let Some(stripped) = rest.strip_prefix('M') {
            let digits = stripped
                .chars()
                .take_while(|ch| ch.is_ascii_digit())
                .collect::<String>();
            if !digits.is_empty() {
                return Some(DefinitionTarget::AtMeta {
                    prefix: "@M",
                    num: digits,
                });
            }
        }
        if let Some(stripped) = rest.strip_prefix('P') {
            let digits = stripped
                .chars()
                .take_while(|ch| ch.is_ascii_digit())
                .collect::<String>();
            if !digits.is_empty() {
                return Some(DefinitionTarget::AtMeta {
                    prefix: "@P",
                    num: digits,
                });
            }
        }
        let digits = rest
            .chars()
            .take_while(|ch| ch.is_ascii_digit())
            .collect::<String>();
        if !digits.is_empty() {
            return Some(DefinitionTarget::Instrument(digits));
        }
        return None;
    }
    if first == '*' {
        let digits = chars
            .take_while(|ch| ch.is_ascii_digit())
            .collect::<String>();
        if !digits.is_empty() {
            return Some(DefinitionTarget::Track(digits));
        }
    }
    None
}

fn find_instrument_definition(text: &str, target: &str) -> Option<Range> {
    find_prefixed_definition(text, "@", target)
}

fn find_prefixed_definition(text: &str, prefix: &str, target: &str) -> Option<Range> {
    for (idx, line) in text.lines().enumerate() {
        let trimmed = line.trim_start();
        if trimmed.is_empty() || trimmed.starts_with(';') {
            continue;
        }
        if !trimmed.starts_with(prefix) {
            continue;
        }
        let mut end = prefix.len();
        while end < trimmed.len() {
            let ch = trimmed.as_bytes()[end] as char;
            if ch.is_ascii_digit() {
                end += 1;
            } else {
                break;
            }
        }
        if end == prefix.len() {
            continue;
        }
        if trimmed.get(prefix.len()..end)? != target {
            continue;
        }
        let start_col = line.len() - trimmed.len();
        let range = Range {
            start: Position::new(idx as u32, start_col as u32),
            end: Position::new(idx as u32, (start_col + end) as u32),
        };
        return Some(range);
    }
    None
}

fn find_track_definition(text: &str, target: &str) -> Option<Range> {
    for (idx, line) in text.lines().enumerate() {
        let trimmed = line.trim_start();
        if trimmed.is_empty() || trimmed.starts_with(';') {
            continue;
        }
        if !trimmed.starts_with('*') {
            continue;
        }
        let mut end = 1;
        while end < trimmed.len() {
            let ch = trimmed.as_bytes()[end] as char;
            if ch.is_ascii_digit() {
                end += 1;
            } else {
                break;
            }
        }
        if end == 1 {
            continue;
        }
        if trimmed.get(1..end)? != target {
            continue;
        }
        let start_col = line.len() - trimmed.len();
        let range = Range {
            start: Position::new(idx as u32, start_col as u32),
            end: Position::new(idx as u32, (start_col + end) as u32),
        };
        return Some(range);
    }
    None
}
