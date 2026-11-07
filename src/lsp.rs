use serde_json::Value;
use tower_lsp::{
    jsonrpc::Result,
    lsp_types::{
        CodeAction, CodeActionContext, CodeActionKind, CodeActionOptions, CodeActionOrCommand,
        CodeActionParams, CodeActionProviderCapability, CodeActionTriggerKind, CodeLens,
        CodeLensOptions, CodeLensParams, Command, CompletionItem, CompletionItemKind,
        CompletionItemLabelDetails, CompletionList, CompletionOptions, CompletionParams,
        CompletionResponse, CompletionTextEdit, DidChangeTextDocumentParams,
        DidCloseTextDocumentParams, DidOpenTextDocumentParams, DidSaveTextDocumentParams,
        Documentation, ExecuteCommandOptions, ExecuteCommandParams, GotoDefinitionParams,
        GotoDefinitionResponse, Hover, HoverContents, HoverParams, HoverProviderCapability,
        InitializeParams, InitializeResult, InsertTextFormat, InsertTextMode, Location,
        MarkupContent, MarkupKind, MessageActionItem, MessageType, OneOf, Position, Range,
        SaveOptions, ServerCapabilities, TextDocumentSyncCapability, TextDocumentSyncKind,
        TextDocumentSyncOptions, TextDocumentSyncSaveOptions, TextEdit,
    },
    LanguageServer,
};

use std::time::Duration;

use crate::backend::{Backend, Session};
use crate::check::CHECK_DEBOUNCE;
use crate::config::{
    apply_completion_client_defaults, completion_settings_from_value, config_from_value,
    ClientKind, Config,
};
use crate::diagnostics::publish_check_diagnostics;
use crate::export::ExportFormat;
use crate::fill_measure::{
    fetch_cursor_tick, fill_measure_action, fill_measure_edit, FillMeasureTarget,
};
use crate::link::{relative_path_display, LinkSource, LinkTool};
use crate::lsp_commands::{
    code_actions, command_ids, transpose_code_action, CMD_EXPORT_VGM, CMD_EXPORT_WAV,
    CMD_MDSLINK_DIRECTORY, CMD_MDSLINK_FILE, CMD_MDSLINK_FROM_CONFIG, CMD_MDSLINK_MENU,
    CMD_PATCH_FORMATS, CMD_PLAY, CMD_PLAY_FROM_CURSOR, CMD_PREVIEW_PATCH, CMD_QUICKROM_DIRECTORY,
    CMD_QUICKROM_FILE, CMD_QUICKROM_FROM_CONFIG, CMD_QUICKROM_MENU, CMD_SAVE_PATCH, CMD_STOP,
};
use crate::note_hover::note_hover_text;
use crate::utils::{is_mml_uri, line_at};
use ctrmml_lang_core::completion::{
    completion_plan, completion_resolve, CompletionPlan, CoreCommand, CoreCompletionList, CoreItem,
    CoreItemKind, DataPayload, DataRequest, EditRange, InsertFormat, Pos,
};
use ctrmml_lang_core::transpose::Direction;
use ctrmml_lang_core::{
    build_preview_mml, byte_offset_to_utf16_character, code_lens_with_config,
    extract_instrument_block, hover_at, instrument_header::strip_keyword, is_in_comment, token_at,
    utf16_character_to_byte_offset, CodeLensConfig, ExtractedBlock, IconStyle, InstrumentType,
};

#[tower_lsp::async_trait]
impl LanguageServer for Backend {
    async fn initialize(&self, params: InitializeParams) -> Result<InitializeResult> {
        let client_kind =
            ClientKind::from_name(params.client_info.as_ref().map(|info| info.name.as_str()));
        let supports_completion_as_is = params
            .capabilities
            .text_document
            .as_ref()
            .and_then(|text_document| text_document.completion.as_ref())
            .and_then(|completion| completion.completion_item.as_ref())
            .and_then(|item| item.insert_text_mode_support.as_ref())
            .is_some_and(|support| support.value_set.contains(&InsertTextMode::AS_IS));
        let supports_code_action_resolve = params
            .capabilities
            .text_document
            .as_ref()
            .and_then(|text_document| text_document.code_action.as_ref())
            .is_some_and(|code_action| {
                code_action.data_support == Some(true)
                    && code_action
                        .resolve_support
                        .as_ref()
                        .is_some_and(|support| support.properties.iter().any(|p| p == "edit"))
            });

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

        let mut config = Config::default();
        let mut completion_settings = Default::default();
        let mut hierarchy_explicit = false;
        if let Some(options) = params.initialization_options {
            if let Some(options_config) = config_from_value(&options) {
                config = options_config;
            }
            (completion_settings, hierarchy_explicit) = completion_settings_from_value(&options);
        }

        // Compatibility until native extensions send completion settings:
        // client-name sniffing may fill only an omitted hierarchy flag.
        apply_completion_client_defaults(&mut completion_settings, hierarchy_explicit, client_kind);
        self.start_session(Session {
            client_kind,
            roots,
            config,
            completion_settings,
            supports_completion_as_is,
            supports_code_action_resolve,
        });

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
                        "|".into(),
                    ]),
                    ..CompletionOptions::default()
                }),
                definition_provider: Some(OneOf::Left(true)),
                execute_command_provider: Some(ExecuteCommandOptions {
                    commands: command_ids(client_kind),
                    ..ExecuteCommandOptions::default()
                }),
                code_action_provider: Some(if supports_code_action_resolve {
                    CodeActionProviderCapability::Options(CodeActionOptions {
                        resolve_provider: Some(true),
                        ..CodeActionOptions::default()
                    })
                } else {
                    CodeActionProviderCapability::Simple(true)
                }),
                code_lens_provider: Some(CodeLensOptions {
                    resolve_provider: Some(false),
                }),
                ..ServerCapabilities::default()
            },
            ..InitializeResult::default()
        })
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        let uri = params.text_document.uri.to_string();
        let text = params.text_document.text;
        self.docs.write().await.insert(uri.clone(), text);
        *self.last_doc.write().await = Some(uri.clone());
        self.schedule_check(uri, Duration::ZERO);
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        let uri = params.text_document.uri.to_string();
        let mut latest_text: Option<String> = None;
        if let Some(change) = params.content_changes.into_iter().last() {
            self.docs
                .write()
                .await
                .insert(uri.clone(), change.text.clone());
            *self.last_doc.write().await = Some(uri.clone());
            latest_text = Some(change.text);
        }
        // Forward the new text to `ctrmml-cmd` if it's currently playing
        // this document — the running renderer will pick the changes up
        // mid-playback via `relink_song` without restarting.
        if let Some(text) = latest_text.as_ref() {
            self.push_playback_update(&uri, text).await;
        }
        self.schedule_check(uri, CHECK_DEBOUNCE);
    }

    async fn did_save(&self, params: DidSaveTextDocumentParams) {
        self.schedule_check(params.text_document.uri.to_string(), Duration::ZERO);
    }

    /// Forget the document and its check results. A playback started from
    /// it keeps playing until stopped.
    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        let uri = params.text_document.uri.to_string();
        self.docs.write().await.remove(&uri);
        self.cancel_check(&uri);
        {
            let mut last_doc = self.last_doc.write().await;
            if last_doc.as_deref() == Some(uri.as_str()) {
                *last_doc = None;
            }
        }
        publish_check_diagnostics(&self.client, &self.diagnostics, &uri, Vec::new()).await;
    }

    async fn hover(&self, params: HoverParams) -> Result<Option<Hover>> {
        let uri = params
            .text_document_position_params
            .text_document
            .uri
            .to_string();
        let position = params.text_document_position_params.position;
        let docs = self.docs.read().await;
        let Some(text) = docs.get(&uri) else {
            return Ok(None);
        };
        let line = line_at(text, position.line);
        let col = utf16_character_to_byte_offset(line, position.character);

        // Generic hover (commands, key sigs, platform commands, FM/2op/PSG
        // params, instrument definitions, …) lives in `ctrmml-lang-core` so
        // web-ctrmml, vscode-ctrmml, and zed-ctrmml all share the same
        // behaviour. It already short-circuits on comments. Note hover is
        // the last layer.
        let (value, start, end) = match hover_at(text, position.line, col as u32) {
            Some(info) => (info.markdown, info.start as usize, info.end as usize),
            None => match note_hover_text(text, position.line, col as u32, line) {
                Some(found) => found,
                None => return Ok(None),
            },
        };
        Ok(Some(Hover {
            contents: HoverContents::Markup(MarkupContent {
                kind: MarkupKind::Markdown,
                value,
            }),
            range: Some(Range {
                start: Position::new(position.line, byte_offset_to_utf16_character(line, start)),
                end: Position::new(position.line, byte_offset_to_utf16_character(line, end)),
            }),
        }))
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
        let docs = self.docs.read().await;
        let Some(text) = docs.get(&uri) else {
            return Ok(None);
        };
        let line = line_at(text, position.line);
        let col = utf16_character_to_byte_offset(line, position.character);
        if is_in_comment(line, col) {
            return Ok(None);
        }
        let Some(range) =
            definition_target_at(line, col).and_then(|target| find_definition(text, &target))
        else {
            return Ok(None);
        };
        Ok(Some(GotoDefinitionResponse::Scalar(Location {
            uri: params.text_document_position_params.text_document.uri,
            range,
        })))
    }

    async fn completion(&self, params: CompletionParams) -> Result<Option<CompletionResponse>> {
        let document_uri = params.text_document_position.text_document.uri;
        let uri = document_uri.to_string();
        if !is_mml_uri(&uri) {
            return Ok(None);
        }
        let position = params.text_document_position.position;
        let text = self
            .docs
            .read()
            .await
            .get(&uri)
            .cloned()
            .unwrap_or_default();
        let settings = self.session().completion_settings.clone();
        let pos = Pos {
            line: position.line,
            character: position.character,
        };
        let trigger = params
            .context
            .and_then(|context| context.trigger_character)
            .and_then(single_trigger_character);

        let list = match completion_plan(&text, pos, trigger, &settings) {
            CompletionPlan::Done(list) => list,
            CompletionPlan::NeedsData(request) => {
                let payload = match request {
                    DataRequest::PcmPaths => DataPayload::PcmPaths(self.pcm_paths(&uri).await),
                    DataRequest::PcmFiles => DataPayload::PcmFiles(self.pcm_paths(&uri).await),
                    DataRequest::FmPatches { .. } => {
                        DataPayload::FmPatches(self.fetch_fm_patches(&uri).await)
                    }
                    DataRequest::CursorTick => {
                        let timing = match self.command_path().await {
                            Ok(command_path) => {
                                fetch_cursor_tick(
                                    &command_path,
                                    &document_uri,
                                    &text,
                                    position.line,
                                    position.character,
                                )
                                .await
                            }
                            Err(_) => None,
                        };
                        DataPayload::CursorTick(timing)
                    }
                };
                completion_resolve(&text, pos, trigger, &settings, payload)
            }
        };
        let supports_as_is = self.session().supports_completion_as_is;
        Ok(Some(core_completion_response(list, supports_as_is)))
    }

    async fn code_lens(&self, params: CodeLensParams) -> Result<Option<Vec<CodeLens>>> {
        let uri = params.text_document.uri.to_string();
        if !is_mml_uri(&uri) {
            return Ok(None);
        }
        let text = self
            .docs
            .read()
            .await
            .get(&uri)
            .cloned()
            .unwrap_or_default();
        let config = code_lens_config(self.session().client_kind);
        let mut out = Vec::new();
        for lens in code_lens_with_config(&text, config) {
            // The lens's anchor span is the entire line; clients are free
            // to position the chip however they like.
            let line = lens.line;
            let range = Range {
                start: Position::new(line, 0),
                end: Position::new(line, 0),
            };
            let command = lens.command_id.map(|id| {
                // Prepend the document URI so client-side command handlers
                // know which file to operate on without extra context.
                let mut args: Vec<Value> = Vec::with_capacity(lens.arguments.len() + 1);
                args.push(Value::String(uri.clone()));
                args.extend(lens.arguments.into_iter().map(Value::String));
                Command {
                    title: lens.title.clone(),
                    command: id,
                    arguments: Some(args),
                }
            });
            out.push(CodeLens {
                range,
                command: command.or(Some(Command {
                    title: lens.title,
                    command: String::new(),
                    arguments: None,
                })),
                data: None,
            });
        }
        Ok(Some(out))
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
            // Fill-measure's edit needs a `ctrmml-cmd find-cursor-tick`
            // run, so it is computed only when the client resolves the
            // action or the user asked for actions explicitly.
            if let Some(mut action) = fill_measure_action(&text, start.line, start.character) {
                let target = FillMeasureTarget {
                    uri: params.text_document.uri.clone(),
                    line: start.line,
                    character: start.character,
                };
                if self.session().supports_code_action_resolve {
                    action.data = Some(target.into_data());
                    actions.push(action);
                } else if is_explicit_request(&params.context) {
                    action.edit = self.fill_measure_edit(&target, &text).await;
                    if action.edit.is_some() {
                        actions.push(action);
                    }
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

    async fn code_action_resolve(&self, mut action: CodeAction) -> Result<CodeAction> {
        let Some(target) = action.data.as_ref().and_then(FillMeasureTarget::from_data) else {
            return Ok(action);
        };
        if let Some(text) = self.document_text(target.uri.as_str()).await {
            action.edit = self.fill_measure_edit(&target, &text).await;
        }
        Ok(action)
    }

    async fn execute_command(&self, params: ExecuteCommandParams) -> Result<Option<Value>> {
        if params.command == CMD_PATCH_FORMATS {
            return Ok(Some(
                serde_json::to_value(ym2612_format::formats()).unwrap_or(Value::Null),
            ));
        }
        match self.run_command(&params.command, &params.arguments).await {
            Ok(Some(message)) => self.client.show_message(MessageType::INFO, message).await,
            Ok(None) => {}
            Err(message) => self.client.show_message(MessageType::ERROR, message).await,
        }
        Ok(None)
    }

    async fn shutdown(&self) -> Result<()> {
        Ok(())
    }
}

impl Backend {
    /// Run an `executeCommand`. `Ok(Some(text))` is a message for the user.
    async fn run_command(
        &self,
        command: &str,
        args: &[Value],
    ) -> std::result::Result<Option<String>, String> {
        if let Some((tool, source)) = link_command(command) {
            let uri = self.resolve_uri_arg(args).await?;
            return self.run_link(tool, source, &uri).await.map(Some);
        }
        match command {
            CMD_PLAY => {
                let uri = self.resolve_uri_arg(args).await?;
                self.start_playback(uri, None).await?;
            }
            CMD_PLAY_FROM_CURSOR => {
                let uri = self.resolve_uri_arg(args).await?;
                let line = args.get(1).and_then(Value::as_u64).unwrap_or(0) as u32;
                let character = args.get(2).and_then(Value::as_u64).unwrap_or(0) as u32;
                self.start_playback(uri, Some((line, character))).await?;
            }
            CMD_STOP => self.stop_playback().await,
            CMD_EXPORT_VGM | CMD_EXPORT_WAV => {
                let format = if command == CMD_EXPORT_VGM {
                    ExportFormat::Vgm
                } else {
                    ExportFormat::Wav
                };
                let uri = self.resolve_uri_arg(args).await?;
                let path = self.run_export(uri, format).await?;
                let display = relative_path_display(&path, &self.session().roots);
                return Ok(Some(format!("exported {display}")));
            }
            CMD_MDSLINK_MENU | CMD_QUICKROM_MENU => {
                let tool = if command == CMD_MDSLINK_MENU {
                    LinkTool::Mdslink
                } else {
                    LinkTool::Quickrom
                };
                let uri = self.resolve_uri_arg(args).await?;
                let Some(source) = self.select_link_source(tool).await? else {
                    return Ok(None);
                };
                return self.run_link(tool, source, &uri).await.map(Some);
            }
            CMD_PREVIEW_PATCH => {
                let patch = parse_patch_args(args);
                let uri = self.resolve_uri_arg(args).await?;
                let (text, block) = self.patch_block(&uri, &patch, "previewPatch").await?;
                let preview = build_preview_mml(&text, &block, &patch.target);
                self.start_playback_with_text(uri, preview, None).await?;
            }
            CMD_SAVE_PATCH => {
                // The client picks the target file and may name its format.
                let patch = parse_patch_args(args);
                if patch.target.is_empty() {
                    return Err("savePatch: missing target path".to_string());
                }
                let uri = self.resolve_uri_arg(args).await?;
                let (_, block) = self.patch_block(&uri, &patch, "savePatch").await?;
                let target = std::path::PathBuf::from(&patch.target);
                let format = args
                    .get(4)
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .or_else(|| {
                        target
                            .extension()
                            .and_then(|extension| extension.to_str())
                            .map(str::to_ascii_lowercase)
                    })
                    .ok_or_else(|| "savePatch: missing target format".to_string())?;
                let data = ym2612_format::convert(
                    block.mml_text.as_bytes(),
                    "input.mml",
                    Some("mml"),
                    0,
                    &format,
                )
                .map_err(|err| format!("savePatch: {err}"))?;
                tokio::fs::write(&target, data)
                    .await
                    .map_err(|err| format!("savePatch: {err}"))?;
                let name = target
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or(patch.target);
                return Ok(Some(format!("saved {name}")));
            }
            _ => {}
        }
        Ok(None)
    }

    /// The document text and the `@N <type>` block the patch arguments name.
    async fn patch_block(
        &self,
        uri: &str,
        patch: &PatchArgs,
        command: &str,
    ) -> std::result::Result<(String, ExtractedBlock), String> {
        let ty = InstrumentType::parse(&patch.instrument_type).ok_or_else(|| {
            format!(
                "{command}: unknown instrument type `{}`",
                patch.instrument_type
            )
        })?;
        let text = self
            .document_text(uri)
            .await
            .ok_or_else(|| format!("{command}: failed to read the document"))?;
        let block = extract_instrument_block(&text, patch.line, ty).ok_or_else(|| {
            format!(
                "{command}: no @N {} block at line {}",
                patch.instrument_type, patch.line
            )
        })?;
        Ok((text, block))
    }

    async fn select_link_source(
        &self,
        tool: LinkTool,
    ) -> std::result::Result<Option<LinkSource>, String> {
        let choices = tool.source_titles();
        let actions = choices
            .iter()
            .map(|(title, _)| MessageActionItem {
                title: title.clone(),
                properties: Default::default(),
            })
            .collect();
        let name = tool.name();
        let selected = self
            .client
            .show_message_request(MessageType::INFO, format!("ctrmml: {name}"), Some(actions))
            .await
            .map_err(|err| format!("failed to open {name} menu: {err}"))?;
        Ok(selected.and_then(|selected| {
            choices
                .into_iter()
                .find(|(title, _)| *title == selected.title)
                .map(|(_, source)| source)
        }))
    }

    async fn fill_measure_edit(
        &self,
        target: &FillMeasureTarget,
        text: &str,
    ) -> Option<tower_lsp::lsp_types::WorkspaceEdit> {
        let cmd_path = self.command_path().await.ok()?;
        fill_measure_edit(&cmd_path, &target.uri, text, target.line, target.character).await
    }
}

/// Whether the user asked for code actions (rather than the client asking
/// on a cursor move), or asked for refactorings by kind.
fn is_explicit_request(context: &CodeActionContext) -> bool {
    let rewrite = CodeActionKind::REFACTOR_REWRITE;
    let rewrite = rewrite.as_str();
    context.trigger_kind == Some(CodeActionTriggerKind::INVOKED)
        || context.only.as_ref().is_some_and(|kinds| {
            kinds.iter().any(|kind| {
                let kind = kind.as_str();
                rewrite == kind || rewrite.starts_with(&format!("{kind}."))
            })
        })
}

fn code_lens_config(client_kind: ClientKind) -> CodeLensConfig {
    if client_kind.is_vscode() {
        CodeLensConfig::default()
    } else {
        CodeLensConfig {
            icon_style: IconStyle::None,
            include_file_actions: false,
        }
    }
}

/// Arguments of the patch commands, `[uri, line, type, target, ...]`. The
/// line may be a number or a string; `target` is the channel to preview on
/// or the file to save to.
#[derive(Debug, PartialEq, Eq)]
struct PatchArgs {
    uri: Option<String>,
    line: u32,
    instrument_type: String,
    target: String,
}

fn parse_patch_args(args: &[Value]) -> PatchArgs {
    let line = args
        .get(1)
        .and_then(|value| {
            value
                .as_u64()
                .or_else(|| value.as_str().and_then(|line| line.parse::<u64>().ok()))
        })
        .unwrap_or(0) as u32;
    let text = |index: usize| {
        args.get(index)
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string()
    };
    PatchArgs {
        uri: args.first().and_then(Value::as_str).map(str::to_string),
        line,
        instrument_type: text(2),
        target: text(3),
    }
}

fn link_command(command: &str) -> Option<(LinkTool, LinkSource)> {
    Some(match command {
        CMD_MDSLINK_FILE => (LinkTool::Mdslink, LinkSource::File),
        CMD_MDSLINK_DIRECTORY => (LinkTool::Mdslink, LinkSource::Directory),
        CMD_MDSLINK_FROM_CONFIG => (LinkTool::Mdslink, LinkSource::Config),
        CMD_QUICKROM_FILE => (LinkTool::Quickrom, LinkSource::File),
        CMD_QUICKROM_DIRECTORY => (LinkTool::Quickrom, LinkSource::Directory),
        CMD_QUICKROM_FROM_CONFIG => (LinkTool::Quickrom, LinkSource::Config),
        _ => return None,
    })
}

fn single_trigger_character(value: String) -> Option<char> {
    let mut chars = value.chars();
    let character = chars.next()?;
    chars.next().is_none().then_some(character)
}

fn core_completion_response(list: CoreCompletionList, supports_as_is: bool) -> CompletionResponse {
    CompletionResponse::List(CompletionList {
        is_incomplete: list.is_incomplete,
        items: list
            .items
            .into_iter()
            .map(|item| core_completion_item(item, supports_as_is))
            .collect(),
    })
}

fn core_completion_item(item: CoreItem, supports_as_is: bool) -> CompletionItem {
    let edit = TextEdit {
        range: core_edit_range(item.edit_range),
        new_text: item.insert.text.clone(),
    };
    let additional_text_edits = (!item.additional_edits.is_empty()).then(|| {
        item.additional_edits
            .into_iter()
            .map(|edit| TextEdit {
                range: core_edit_range(edit.range),
                new_text: edit.new_text,
            })
            .collect()
    });

    CompletionItem {
        label: item.label,
        label_details: item
            .label_description
            .map(|description| CompletionItemLabelDetails {
                detail: None,
                description: Some(description),
            }),
        kind: Some(core_completion_kind(item.kind)),
        detail: item.detail,
        documentation: item.documentation.map(Documentation::String),
        insert_text: None,
        insert_text_format: (item.insert.format == InsertFormat::Snippet)
            .then_some(InsertTextFormat::SNIPPET),
        insert_text_mode: (item.insert.as_is && supports_as_is).then_some(InsertTextMode::AS_IS),
        text_edit: Some(CompletionTextEdit::Edit(edit)),
        additional_text_edits,
        filter_text: item.filter_text,
        sort_text: item.sort_text,
        preselect: Some(item.preselect),
        command: item.command.map(|command| match command {
            CoreCommand::TriggerSuggest => Command {
                title: "Trigger suggest".to_string(),
                command: "editor.action.triggerSuggest".to_string(),
                arguments: None,
            },
        }),
        ..CompletionItem::default()
    }
}

fn core_completion_kind(kind: CoreItemKind) -> CompletionItemKind {
    match kind {
        CoreItemKind::Function => CompletionItemKind::FUNCTION,
        CoreItemKind::Keyword => CompletionItemKind::KEYWORD,
        CoreItemKind::Value => CompletionItemKind::VALUE,
        CoreItemKind::Property => CompletionItemKind::PROPERTY,
        CoreItemKind::TypeParameter => CompletionItemKind::TYPE_PARAMETER,
        CoreItemKind::Struct => CompletionItemKind::STRUCT,
        CoreItemKind::File => CompletionItemKind::FILE,
        CoreItemKind::Snippet => CompletionItemKind::SNIPPET,
        CoreItemKind::Text => CompletionItemKind::TEXT,
    }
}

fn core_edit_range(range: EditRange) -> Range {
    Range {
        start: Position::new(range.start.line, range.start.character),
        end: Position::new(range.end.line, range.end.character),
    }
}

/// A `prefix<digits>` reference: `@N` instruments, `@EN` / `@MN` / `@PN`
/// definitions and `*N` tracks.
struct DefinitionTarget {
    prefix: &'static str,
    digits: String,
}

fn definition_target_at(line: &str, col: usize) -> Option<DefinitionTarget> {
    let (token, _start, _end) = token_at(line, col)?;
    let prefix = ["@E", "@M", "@P", "@", "*"].into_iter().find(|prefix| {
        token
            .strip_prefix(prefix)
            .is_some_and(|rest| rest.starts_with(|ch: char| ch.is_ascii_digit()))
    })?;
    let digits = token[prefix.len()..]
        .chars()
        .take_while(|ch| ch.is_ascii_digit())
        .collect();
    Some(DefinitionTarget { prefix, digits })
}

/// The first line that defines `target`: `prefix<digits>` at the start of
/// the line, followed by whitespace, `;` or the end of the line.
fn find_definition(text: &str, target: &DefinitionTarget) -> Option<Range> {
    for (idx, line) in text.lines().enumerate() {
        let start = line.len() - line.trim_start().len();
        let Some(rest) = strip_keyword(&line[start..], target.prefix) else {
            continue;
        };
        let Some(after) = rest.strip_prefix(target.digits.as_str()) else {
            continue;
        };
        if !after.is_empty() && !after.starts_with(|ch: char| ch.is_whitespace() || ch == ';') {
            continue;
        }
        let end = start + target.prefix.len() + target.digits.len();
        return Some(Range {
            start: Position::new(idx as u32, byte_offset_to_utf16_character(line, start)),
            end: Position::new(idx as u32, byte_offset_to_utf16_character(line, end)),
        });
    }
    None
}

#[cfg(test)]
mod tests {
    use ctrmml_lang_core::completion::{CoreTextEdit, InsertSpec};

    use super::*;

    #[test]
    fn fill_measure_runs_on_explicit_requests_only() {
        let context = |trigger_kind, only: Option<Vec<CodeActionKind>>| CodeActionContext {
            diagnostics: Vec::new(),
            only,
            trigger_kind,
        };
        assert!(!is_explicit_request(&context(
            Some(CodeActionTriggerKind::AUTOMATIC),
            None
        )));
        assert!(!is_explicit_request(&context(None, None)));
        assert!(is_explicit_request(&context(
            Some(CodeActionTriggerKind::INVOKED),
            None
        )));
        assert!(is_explicit_request(&context(
            None,
            Some(vec![CodeActionKind::REFACTOR])
        )));
        assert!(!is_explicit_request(&context(
            None,
            Some(vec![CodeActionKind::QUICKFIX])
        )));
    }

    #[test]
    fn patch_format_values_carry_the_fields_clients_read() {
        let formats = serde_json::to_value(ym2612_format::formats()).expect("serialize formats");
        let formats = formats.as_array().expect("array").clone();
        assert!(!formats.is_empty());
        for format in &formats {
            for field in ["format", "name", "extension"] {
                assert!(format[field].is_string(), "{field} in {format}");
            }
            for field in ["can_read", "can_write", "is_text"] {
                assert!(format[field].is_boolean(), "{field} in {format}");
            }
        }
        assert!(formats
            .iter()
            .any(|format| format["can_write"] == serde_json::json!(true)));
    }

    fn core_item(kind: CoreItemKind) -> CoreItem {
        CoreItem {
            label: "item".to_string(),
            label_description: None,
            kind,
            detail: None,
            documentation: None,
            insert: InsertSpec {
                text: "value".to_string(),
                format: InsertFormat::PlainText,
                as_is: false,
            },
            filter_text: None,
            sort_text: None,
            preselect: false,
            edit_range: EditRange::new(Pos::default(), Pos::default()),
            additional_edits: Vec::new(),
            command: None,
        }
    }

    #[test]
    fn completion_kind_mapping_covers_core_contract() {
        let cases = [
            (CoreItemKind::Function, CompletionItemKind::FUNCTION),
            (CoreItemKind::Keyword, CompletionItemKind::KEYWORD),
            (CoreItemKind::Value, CompletionItemKind::VALUE),
            (CoreItemKind::Property, CompletionItemKind::PROPERTY),
            (
                CoreItemKind::TypeParameter,
                CompletionItemKind::TYPE_PARAMETER,
            ),
            (CoreItemKind::Struct, CompletionItemKind::STRUCT),
            (CoreItemKind::File, CompletionItemKind::FILE),
            (CoreItemKind::Snippet, CompletionItemKind::SNIPPET),
            (CoreItemKind::Text, CompletionItemKind::TEXT),
        ];
        for (core, lsp) in cases {
            assert_eq!(core_completion_kind(core), lsp);
        }
    }

    #[test]
    fn completion_item_maps_all_transport_fields() {
        let mut item = core_item(CoreItemKind::Snippet);
        item.label_description = Some("inline detail".to_string());
        item.detail = Some("detail".to_string());
        item.documentation = Some("literal *plain* text".to_string());
        item.insert = InsertSpec {
            text: "  ${1:value}".to_string(),
            format: InsertFormat::Snippet,
            as_is: true,
        };
        item.filter_text = Some("filter".to_string());
        item.sort_text = Some("001".to_string());
        item.preselect = true;
        item.edit_range = EditRange::new(
            Pos {
                line: 2,
                character: 3,
            },
            Pos {
                line: 2,
                character: 7,
            },
        );
        item.additional_edits.push(CoreTextEdit {
            range: EditRange::new(
                Pos {
                    line: 3,
                    character: 0,
                },
                Pos {
                    line: 4,
                    character: 0,
                },
            ),
            new_text: String::new(),
        });
        item.command = Some(CoreCommand::TriggerSuggest);

        let mapped = core_completion_item(item, true);
        assert_eq!(mapped.insert_text, None);
        assert_eq!(mapped.kind, Some(CompletionItemKind::SNIPPET));
        assert_eq!(mapped.insert_text_format, Some(InsertTextFormat::SNIPPET));
        assert_eq!(mapped.insert_text_mode, Some(InsertTextMode::AS_IS));
        assert_eq!(mapped.preselect, Some(true));
        assert_eq!(
            mapped.documentation,
            Some(Documentation::String("literal *plain* text".to_string()))
        );
        assert_eq!(
            mapped.label_details.and_then(|details| details.description),
            Some("inline detail".to_string())
        );
        assert_eq!(
            mapped.additional_text_edits.expect("additional edit").len(),
            1
        );
        assert_eq!(
            mapped.command.expect("trigger command").command,
            "editor.action.triggerSuggest"
        );
        let CompletionTextEdit::Edit(edit) = mapped.text_edit.expect("primary edit") else {
            panic!("expected simple text edit");
        };
        assert_eq!(edit.range.start, Position::new(2, 3));
        assert_eq!(edit.range.end, Position::new(2, 7));
        assert_eq!(edit.new_text, "  ${1:value}");
    }

    #[test]
    fn completion_item_omits_unsupported_as_is_and_plain_format() {
        let mut item = core_item(CoreItemKind::Text);
        item.insert.as_is = true;
        let mapped = core_completion_item(item, false);
        assert_eq!(mapped.insert_text_format, None);
        assert_eq!(mapped.insert_text_mode, None);
        assert_eq!(mapped.additional_text_edits, None);
    }

    #[test]
    fn completion_response_preserves_incomplete_flag() {
        let response = core_completion_response(
            CoreCompletionList {
                items: vec![core_item(CoreItemKind::Text)],
                is_incomplete: true,
            },
            false,
        );
        let CompletionResponse::List(list) = response else {
            panic!("expected completion list");
        };
        assert!(list.is_incomplete);
        assert_eq!(list.items.len(), 1);
    }

    #[test]
    fn code_lens_policy_preserves_vscode_and_limits_other_clients_to_preview() {
        let vscode = code_lens_with_config("@1 fm\n", code_lens_config(ClientKind::VsCode));
        let vscode_titles: Vec<&str> = vscode.iter().map(|lens| lens.title.as_str()).collect();
        assert_eq!(
            vscode_titles,
            ["$(folder-opened) Load", "$(save) Save", "$(play) FM"]
        );

        let other = code_lens_with_config("@1 fm\n", code_lens_config(ClientKind::Other));
        assert_eq!(other.len(), 1);
        // Codicon markers are dropped, but the play glyph stays so the lens
        // still reads as a button in clients that render titles verbatim.
        assert_eq!(other[0].title, "▷ FM");
        assert_eq!(other[0].command_id.as_deref(), Some(CMD_PREVIEW_PATCH));
    }

    fn definition(text: &str, line: &str, col: usize) -> Option<Range> {
        find_definition(text, &definition_target_at(line, col)?)
    }

    #[test]
    fn definitions_are_found_by_prefix_and_number() {
        let text = "@10 fm\n@1 psg 15\n@E1 1 2\n*3 c d\nA @1 @E1 *3\n";
        let line = "A @1 @E1 *3";
        assert_eq!(definition(text, line, 3).map(|r| r.start.line), Some(1));
        assert_eq!(definition(text, line, 6).map(|r| r.start.line), Some(2));
        assert_eq!(definition(text, line, 10).map(|r| r.start.line), Some(3));
        assert_eq!(
            definition(text, line, 3).map(|r| (r.start.character, r.end.character)),
            Some((0, 2))
        );
    }

    #[test]
    fn definition_ranges_count_utf16_units() {
        // `あ` is one UTF-16 unit and three bytes.
        let text = "\u{3000}@1 fm\nA @1\n";
        let range = definition(text, "A @1", 3).expect("definition");
        assert_eq!((range.start.character, range.end.character), (1, 3));
    }

    #[test]
    fn a_definition_needs_a_boundary_after_its_number() {
        assert_eq!(definition("@1fm\nA @1\n", "A @1", 3), None);
    }

    #[test]
    fn preview_lens_arguments_round_trip_through_handler_parser() {
        let lens = code_lens_with_config("@7 fm\n", code_lens_config(ClientKind::Other))
            .into_iter()
            .find(|lens| lens.command_id.as_deref() == Some(CMD_PREVIEW_PATCH))
            .expect("FM preview lens");
        assert_eq!(lens.arguments, ["0", "fm", "A", "7"]);

        let mut wire_args = vec![Value::String("file:///song.mml".to_string())];
        wire_args.extend(lens.arguments.into_iter().map(Value::String));
        assert_eq!(
            parse_patch_args(&wire_args),
            PatchArgs {
                uri: Some("file:///song.mml".to_string()),
                line: 0,
                instrument_type: "fm".to_string(),
                target: "A".to_string(),
            }
        );
    }
}
