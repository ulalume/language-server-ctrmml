use std::{collections::HashMap, path::PathBuf, sync::Arc};

use serde_json::Value;
use tokio::sync::{Mutex, RwLock};
use tower_lsp::Client;

use crate::config::Config;
use crate::ctrmml_cmd::resolve_command_path;
use crate::playback::Playback;

pub(crate) struct Backend {
    pub(crate) client: Client,
    pub(crate) docs: Arc<RwLock<HashMap<String, String>>>,
    pub(crate) roots: Arc<RwLock<Vec<PathBuf>>>,
    pub(crate) config: Arc<RwLock<Config>>,
    pub(crate) playback: Arc<Mutex<Option<Playback>>>,
    pub(crate) playback_seq: Arc<Mutex<u64>>,
    pub(crate) last_doc: Arc<RwLock<Option<String>>>,
}

impl Backend {
    pub(crate) fn new(client: Client) -> Self {
        Self {
            client,
            docs: Arc::new(RwLock::new(HashMap::new())),
            roots: Arc::new(RwLock::new(Vec::new())),
            config: Arc::new(RwLock::new(Config::default())),
            playback: Arc::new(Mutex::new(None)),
            playback_seq: Arc::new(Mutex::new(0)),
            last_doc: Arc::new(RwLock::new(None)),
        }
    }

    pub(crate) async fn resolve_uri_arg(
        &self,
        args: &[Value],
    ) -> std::result::Result<String, String> {
        if let Some(Value::String(uri)) = args.get(0) {
            return Ok(uri.clone());
        }
        if let Some(uri) = self.last_doc.read().await.clone() {
            return Ok(uri);
        }
        Err("no active document".to_string())
    }

    pub(crate) async fn command_path(&self) -> std::result::Result<String, String> {
        let config_path = self.config.read().await.command_path.clone();
        resolve_command_path(config_path).await
    }
}
