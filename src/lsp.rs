use serde_json::{json, Value};
use tower_lsp::{
    jsonrpc::Result,
    lsp_types::{
        CodeAction, CodeActionOrCommand, CodeActionParams, CodeActionProviderCapability,
        Command, CompletionOptions, CompletionParams, CompletionResponse, ExecuteCommandParams,
        ExecuteCommandOptions, InitializeParams, InitializeResult, DidSaveTextDocumentParams,
        SaveOptions, ServerCapabilities, TextDocumentSyncCapability, TextDocumentSyncKind,
        TextDocumentSyncOptions, TextDocumentSyncSaveOptions,
    },
    LanguageServer,
};

use crate::backend::Backend;
use crate::completion::{
    at_meta_completion_items, command_items, complete_pcm_paths, instrument_items,
    is_at_meta_context, is_in_comment, is_instrument_definition_context, is_rate_offset_context,
    meta_completion_items, platform_items, rate_offset_items,
};
use crate::config::config_from_value;
use crate::export::ExportFormat;
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
            return Ok(Some(CompletionResponse::Array(platform_items())));
        }

        if line.trim_start().starts_with('#') {
            let items = meta_completion_items(&line, col, position.line);
            return Ok(Some(CompletionResponse::Array(items)));
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

        Ok(Some(CompletionResponse::Array(command_items())))
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
                self.start_playback(uri, Some((line, col)))
                    .await
                    .map_err(lsp_err)?;
            }
            "ctrmml.stop" => {
                self.stop_playback().await;
            }
            "ctrmml.exportVgm" => {
                let uri = self.resolve_uri_arg(&args).await.map_err(lsp_err)?;
                self.run_export(uri, ExportFormat::Vgm)
                    .await
                    .map_err(lsp_err)?;
            }
            "ctrmml.exportWav" => {
                let uri = self.resolve_uri_arg(&args).await.map_err(lsp_err)?;
                self.run_export(uri, ExportFormat::Wav)
                    .await
                    .map_err(lsp_err)?;
            }
            _ => {}
        }
        Ok(None)
    }

    async fn shutdown(&self) -> Result<()> {
        Ok(())
    }
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
