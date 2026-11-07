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
use crate::config::{ClientOptions, Settings};
use crate::ctrmml_cmd::resolve_command_path;
use crate::diagnostics::DiagnosticStoreHandle;
use crate::fm_completion::FmPatchCache;
use crate::playback::Playback;
use crate::utils::read_file_text;
use crate::workspace_files::WorkspaceFiles;

/// Client facts fixed by `initialize`.
pub(crate) struct Session {
    pub(crate) client: ClientOptions,
    pub(crate) roots: Vec<PathBuf>,
    /// Settings from `initializationOptions`; configuration applies on top.
    pub(crate) initial_settings: Settings,
    pub(crate) fm_picker_hierarchy_default: bool,
    pub(crate) supports_completion_as_is: bool,
    pub(crate) supports_code_action_resolve: bool,
    pub(crate) supports_configuration: bool,
    pub(crate) supports_configuration_registration: bool,
}

impl Default for Session {
    fn default() -> Self {
        let init = crate::config::initialization(None, None);
        Self {
            client: init.client,
            roots: Vec::new(),
            initial_settings: init.settings,
            fm_picker_hierarchy_default: init.fm_picker_hierarchy_default,
            supports_completion_as_is: false,
            supports_code_action_resolve: false,
            supports_configuration: false,
            supports_configuration_registration: false,
        }
    }
}

/// The resolved `ctrmml-cmd`, for one configured path.
#[derive(Default)]
struct CommandPath {
    configured: Option<String>,
    resolved: Arc<OnceCell<String>>,
}

/// Server state. Cloning shares it, so handlers can hand work to tasks.
#[derive(Clone)]
pub(crate) struct Backend {
    pub(crate) client: Client,
    pub(crate) docs: Arc<RwLock<HashMap<String, String>>>,
    session: Arc<OnceLock<Session>>,
    settings: Arc<std::sync::RwLock<Settings>>,
    /// Counts configuration changes, so a slow read cannot overwrite a newer one.
    pub(crate) settings_seq: Arc<AtomicU64>,
    command_path: Arc<std::sync::Mutex<CommandPath>>,
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
            settings: Arc::default(),
            settings_seq: Arc::default(),
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
        self.set_settings(session.initial_settings.clone());
        let _ = self.session.set(session);
    }

    pub(crate) fn settings(&self) -> Settings {
        self.settings.read().expect("settings lock").clone()
    }

    pub(crate) fn completion_settings(&self) -> CompletionSettings {
        self.settings
            .read()
            .expect("settings lock")
            .completion
            .clone()
    }

    pub(crate) fn set_settings(&self, settings: Settings) {
        *self.settings.write().expect("settings lock") = settings;
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

    /// The `ctrmml-cmd` to run, resolved once per configured path.
    /// Resolution runs in its own task, so a cancelled caller does not
    /// abort a download.
    pub(crate) async fn command_path(&self) -> std::result::Result<String, String> {
        let config_path = self.settings().command_path;
        let cell = {
            let mut current = self.command_path.lock().expect("command path lock");
            if current.configured != config_path {
                *current = CommandPath {
                    configured: config_path.clone(),
                    resolved: Arc::default(),
                };
            }
            Arc::clone(&current.resolved)
        };
        if let Some(path) = cell.get() {
            return Ok(path.clone());
        }
        tokio::spawn(async move {
            cell.get_or_try_init(|| resolve_command_path(config_path))
                .await
                .cloned()
        })
        .await
        .map_err(|e| format!("failed to resolve ctrmml-cmd: {e}"))?
    }
}
