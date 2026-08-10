use serde_json::Value;
use tower_lsp::{
    jsonrpc::Result,
    lsp_types::{
        CodeActionOrCommand, CodeActionParams, CodeActionProviderCapability, CodeLens,
        CodeLensOptions, CodeLensParams, Command, CompletionItem, CompletionItemKind,
        CompletionItemLabelDetails, CompletionList, CompletionOptions, CompletionParams,
        CompletionResponse, CompletionTextEdit, DidSaveTextDocumentParams, Documentation,
        ExecuteCommandOptions, ExecuteCommandParams, GotoDefinitionParams, GotoDefinitionResponse,
        Hover, HoverContents, HoverParams, HoverProviderCapability, InitializeParams,
        InitializeResult, InsertTextFormat, InsertTextMode, Location, MarkupContent, MarkupKind,
        MessageActionItem, MessageType, OneOf, Position, Range, SaveOptions, ServerCapabilities,
        TextDocumentSyncCapability, TextDocumentSyncKind, TextDocumentSyncOptions,
        TextDocumentSyncSaveOptions, TextEdit,
    },
    LanguageServer,
};

use crate::backend::Backend;
use crate::completion::scan_pcm_paths;
use crate::config::{
    apply_completion_client_defaults, completion_settings_from_value, config_from_value, ClientKind,
};
use crate::export::ExportFormat;
use crate::fill_measure::{fetch_cursor_tick, fill_measure_code_action};
use crate::lsp_commands::{
    code_actions, command_ids, transpose_code_action, CMD_EXPORT_VGM, CMD_EXPORT_WAV,
    CMD_MDSLINK_DIRECTORY, CMD_MDSLINK_FILE, CMD_MDSLINK_FROM_CONFIG, CMD_MDSLINK_MENU, CMD_PLAY,
    CMD_PLAY_FROM_CURSOR, CMD_PREVIEW_PATCH, CMD_QUICKROM_DIRECTORY, CMD_QUICKROM_FILE,
    CMD_QUICKROM_FROM_CONFIG, CMD_QUICKROM_MENU, CMD_SAVE_PATCH, CMD_STOP,
};
use crate::mdslink::MdslinkRunResult;
use crate::note_hover::note_hover_text;
use crate::quickrom::QuickromRunResult;
use crate::utils::{is_mml_uri, line_at};
use crate::ym2612_convert::convert_mml_to_file;
use ctrmml_lang_core::completion::{
    completion_plan, completion_resolve, CompletionPlan, CoreCommand, CoreCompletionList, CoreItem,
    CoreItemKind, DataPayload, DataRequest, EditRange, InsertFormat, Pos,
};
use ctrmml_lang_core::transpose::Direction;
use ctrmml_lang_core::{
    build_preview_mml, code_lens_with_config, extract_instrument_block, hover_at, is_in_comment,
    token_at, CodeLensConfig, IconStyle, InstrumentType,
};

#[tower_lsp::async_trait]
impl LanguageServer for Backend {
    async fn initialize(&self, params: InitializeParams) -> Result<InitializeResult> {
        let client_kind =
            ClientKind::from_name(params.client_info.as_ref().map(|info| info.name.as_str()));
        *self.client_kind.write().await = client_kind;
        let supports_completion_as_is = params
            .capabilities
            .text_document
            .as_ref()
            .and_then(|text_document| text_document.completion.as_ref())
            .and_then(|completion| completion.completion_item.as_ref())
            .and_then(|item| item.insert_text_mode_support.as_ref())
            .is_some_and(|support| support.value_set.contains(&InsertTextMode::AS_IS));
        *self.supports_completion_as_is.write().await = supports_completion_as_is;

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

        let mut completion_settings = Default::default();
        let mut hierarchy_explicit = false;
        if let Some(options) = params.initialization_options {
            if let Some(config) = config_from_value(&options) {
                *self.config.write().await = config;
            }
            (completion_settings, hierarchy_explicit) = completion_settings_from_value(&options);
        }

        // Compatibility until native extensions send completion settings:
        // client-name sniffing may fill only an omitted hierarchy flag.
        apply_completion_client_defaults(&mut completion_settings, hierarchy_explicit, client_kind);
        *self.completion_settings.write().await = completion_settings;

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
                code_action_provider: Some(CodeActionProviderCapability::Simple(true)),
                code_lens_provider: Some(CodeLensOptions {
                    resolve_provider: Some(false),
                }),
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
        let roots = self.roots.read().await.clone();
        let settings = self.completion_settings.read().await.clone();
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
                    DataRequest::PcmPaths => DataPayload::PcmPaths(scan_pcm_paths(&uri, &roots)),
                    DataRequest::PcmFiles => DataPayload::PcmFiles(scan_pcm_paths(&uri, &roots)),
                    DataRequest::FmPatches { .. } => {
                        DataPayload::FmPatches(self.fetch_fm_patches(&uri, &roots).await)
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
        let supports_as_is = *self.supports_completion_as_is.read().await;
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
        let client_kind = *self.client_kind.read().await;
        let config = code_lens_config(client_kind);
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
            CMD_PREVIEW_PATCH => {
                // Args from the code-lens dispatch in `ctrmml-lang-core`:
                //   [uri, line_str, type, channel, instrument_number_str]
                // The LSP layer prepended the uri before forwarding; the
                // lang-core side keeps the line as a string in LSP wire
                // form (zero-based).
                // lang-core emits the line as a string; vscode-ctrmml may
                // forward it as a number once it grows its own typed
                // wrapper. Accept either shape.
                let parsed = parse_preview_patch_args(&args);
                let uri = match parsed.uri {
                    Some(uri) => uri,
                    None => match self.resolve_uri_arg(&args).await {
                        Ok(uri) => uri,
                        Err(err) => {
                            let _ = self.client.show_message(MessageType::ERROR, err).await;
                            return Ok(None);
                        }
                    },
                };
                let ty = match InstrumentType::parse(&parsed.instrument_type) {
                    Some(ty) => ty,
                    None => {
                        let _ = self
                            .client
                            .show_message(
                                MessageType::ERROR,
                                format!(
                                    "previewPatch: unknown instrument type `{}`",
                                    parsed.instrument_type
                                ),
                            )
                            .await;
                        return Ok(None);
                    }
                };
                let doc_text = self.docs.read().await.get(&uri).cloned();
                let Some(doc_text) = doc_text else {
                    let _ = self
                        .client
                        .show_message(MessageType::ERROR, "previewPatch: document not in cache")
                        .await;
                    return Ok(None);
                };
                let Some(block) = extract_instrument_block(&doc_text, parsed.line, ty) else {
                    let _ = self
                        .client
                        .show_message(
                            MessageType::ERROR,
                            format!(
                                "previewPatch: no @N {} block at line {}",
                                parsed.instrument_type, parsed.line
                            ),
                        )
                        .await;
                    return Ok(None);
                };
                let preview = build_preview_mml(&doc_text, &block, &parsed.channel);
                if let Err(err) = self.start_playback_with_text(uri, preview, None).await {
                    let _ = self.client.show_message(MessageType::ERROR, err).await;
                }
            }
            CMD_SAVE_PATCH => {
                // Args: [uri, line, type, target_path, format?]. The client
                // shows the save dialog so it knows where the user wants
                // the patch to land; the LSP then runs the conversion.
                let uri = match self.resolve_uri_arg(&args).await {
                    Ok(uri) => uri,
                    Err(err) => {
                        let _ = self.client.show_message(MessageType::ERROR, err).await;
                        return Ok(None);
                    }
                };
                let line = args
                    .get(1)
                    .and_then(|v| {
                        v.as_u64()
                            .or_else(|| v.as_str().and_then(|s| s.parse::<u64>().ok()))
                    })
                    .unwrap_or(0) as u32;
                let type_str = args.get(2).and_then(|v| v.as_str()).unwrap_or("");
                let target_str = args.get(3).and_then(|v| v.as_str()).unwrap_or("");
                let format_override = args.get(4).and_then(|v| v.as_str());
                if target_str.is_empty() {
                    let _ = self
                        .client
                        .show_message(MessageType::ERROR, "savePatch: missing target path")
                        .await;
                    return Ok(None);
                }
                let ty = match InstrumentType::parse(type_str) {
                    Some(ty) => ty,
                    None => {
                        let _ = self
                            .client
                            .show_message(
                                MessageType::ERROR,
                                format!("savePatch: unknown instrument type `{type_str}`"),
                            )
                            .await;
                        return Ok(None);
                    }
                };
                let doc_text = self.docs.read().await.get(&uri).cloned();
                let Some(doc_text) = doc_text else {
                    let _ = self
                        .client
                        .show_message(MessageType::ERROR, "savePatch: document not in cache")
                        .await;
                    return Ok(None);
                };
                let Some(block) = extract_instrument_block(&doc_text, line, ty) else {
                    let _ = self
                        .client
                        .show_message(
                            MessageType::ERROR,
                            format!("savePatch: no @N {type_str} block at line {line}"),
                        )
                        .await;
                    return Ok(None);
                };
                let cmd_path = match self.ym2612_convert_path().await {
                    Ok(p) => p,
                    Err(err) => {
                        let _ = self.client.show_message(MessageType::ERROR, err).await;
                        return Ok(None);
                    }
                };
                let target = std::path::PathBuf::from(target_str);
                match convert_mml_to_file(&cmd_path, &block.mml_text, &target, format_override)
                    .await
                {
                    Ok(()) => {
                        let display = target_str
                            .rsplit(std::path::MAIN_SEPARATOR)
                            .next()
                            .unwrap_or(target_str);
                        let _ = self
                            .client
                            .show_message(MessageType::INFO, format!("saved {display}"))
                            .await;
                    }
                    Err(err) => {
                        let _ = self.client.show_message(MessageType::ERROR, err).await;
                    }
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

#[derive(Debug, PartialEq, Eq)]
struct PreviewPatchArgs {
    uri: Option<String>,
    line: u32,
    instrument_type: String,
    channel: String,
}

fn parse_preview_patch_args(args: &[Value]) -> PreviewPatchArgs {
    let line = args
        .get(1)
        .and_then(|value| {
            value
                .as_u64()
                .or_else(|| value.as_str().and_then(|line| line.parse::<u64>().ok()))
        })
        .unwrap_or(0) as u32;
    PreviewPatchArgs {
        uri: args.first().and_then(Value::as_str).map(str::to_string),
        line,
        instrument_type: args
            .get(2)
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        channel: args
            .get(3)
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
    }
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

#[cfg(test)]
mod tests {
    use ctrmml_lang_core::completion::{CoreTextEdit, InsertSpec};

    use super::*;

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
        assert_eq!(other[0].title, "FM");
        assert_eq!(other[0].command_id.as_deref(), Some(CMD_PREVIEW_PATCH));
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
            parse_preview_patch_args(&wire_args),
            PreviewPatchArgs {
                uri: Some("file:///song.mml".to_string()),
                line: 0,
                instrument_type: "fm".to_string(),
                channel: "A".to_string(),
            }
        );
    }
}
