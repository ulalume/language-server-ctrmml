use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{atomic::AtomicU64, Arc, OnceLock},
};

use serde_json::Value;
use tokio::sync::{Mutex, OnceCell, RwLock};
use tower_lsp::Client;

use ctrmml_lang_core::completion::CompletionSettings;

use crate::check::CheckQueue;
use crate::config::{ClientKind, Config};
use crate::ctrmml_cmd::resolve_command_path;
use crate::diagnostics::DiagnosticStoreHandle;
use crate::fm_completion::FmPatchCache;
use crate::playback::Playback;
use crate::utils::read_file_text;
use crate::workspace_files::WorkspaceFiles;

/// Client facts and settings fixed by `initialize`.
#[derive(Default)]
pub(crate) struct Session {
    pub(crate) client_kind: ClientKind,
    pub(crate) roots: Vec<PathBuf>,
    pub(crate) config: Config,
    pub(crate) completion_settings: CompletionSettings,
    pub(crate) supports_completion_as_is: bool,
    pub(crate) supports_code_action_resolve: bool,
}

/// Server state. Cloning shares it, so handlers can hand work to tasks.
#[derive(Clone)]
pub(crate) struct Backend {
    pub(crate) client: Client,
    pub(crate) docs: Arc<RwLock<HashMap<String, String>>>,
    session: Arc<OnceLock<Session>>,
    command_path: Arc<OnceCell<String>>,
    pub(crate) workspace_files: Arc<WorkspaceFiles>,
    pub(crate) fm_patches: Arc<FmPatchCache>,
    pub(crate) diagnostics: DiagnosticStoreHandle,
    pub(crate) checks: Arc<std::sync::Mutex<CheckQueue>>,
    pub(crate) playback: Arc<Mutex<Option<Playback>>>,
    pub(crate) playback_seq: Arc<AtomicU64>,
    pub(crate) last_doc: Arc<RwLock<Option<String>>>,
}

impl Backend {
    pub(crate) fn new(client: Client) -> Self {
        Self {
            client,
            docs: Arc::default(),
            session: Arc::default(),
            command_path: Arc::default(),
            workspace_files: Arc::default(),
            fm_patches: Arc::default(),
            diagnostics: Arc::default(),
            checks: Arc::default(),
            playback: Arc::default(),
            playback_seq: Arc::default(),
            last_doc: Arc::default(),
        }
    }

    pub(crate) fn session(&self) -> &Session {
        self.session.get_or_init(Session::default)
    }

    pub(crate) fn start_session(&self, session: Session) {
        let _ = self.session.set(session);
    }

    pub(crate) async fn resolve_uri_arg(
        &self,
        args: &[Value],
    ) -> std::result::Result<String, String> {
        if let Some(Value::String(uri)) = args.first() {
            return Ok(uri.clone());
        }
        if let Some(uri) = self.last_doc.read().await.clone() {
            return Ok(uri);
        }
        Err("no active document".to_string())
    }

    /// The open document's text, else the file's.
    pub(crate) async fn document_text(&self, uri: &str) -> Option<String> {
        if let Some(text) = self.docs.read().await.get(uri) {
            return Some(text.clone());
        }
        let uri = uri.to_string();
        tokio::task::spawn_blocking(move || read_file_text(&uri))
            .await
            .ok()
            .flatten()
    }

    /// The `ctrmml-cmd` to run, resolved once per session.
    pub(crate) async fn command_path(&self) -> std::result::Result<String, String> {
        self.command_path
            .get_or_try_init(|| resolve_command_path(self.session().config.command_path.clone()))
            .await
            .cloned()
    }
}
