use std::{collections::HashMap, path::PathBuf, sync::Arc};

use serde_json::Value;
use tokio::sync::{Mutex, RwLock};
use tower_lsp::Client;

use ctrmml_lang_core::completion::CompletionSettings;

use crate::config::Config;
use crate::ctrmml_cmd::resolve_command_path;
use crate::fm_completion::FmInstrumentCache;
use crate::playback::Playback;
use crate::ym2612_convert::resolve_ym2612_convert_path;

pub(crate) struct Backend {
    pub(crate) client: Client,
    pub(crate) docs: Arc<RwLock<HashMap<String, String>>>,
    pub(crate) roots: Arc<RwLock<Vec<PathBuf>>>,
    pub(crate) config: Arc<RwLock<Config>>,
    command_path_cache: Arc<Mutex<Option<CommandPathCache>>>,
    ym2612_convert_path_cache: Arc<Mutex<Option<CommandPathCache>>>,
    pub(crate) fm_instrument_cache: Arc<Mutex<Option<FmInstrumentCache>>>,
    pub(crate) playback: Arc<Mutex<Option<Playback>>>,
    pub(crate) playback_seq: Arc<Mutex<u64>>,
    pub(crate) last_doc: Arc<RwLock<Option<String>>>,
    pub(crate) supports_hierarchy: Arc<RwLock<bool>>,
    pub(crate) completion_settings: Arc<RwLock<CompletionSettings>>,
    pub(crate) supports_completion_as_is: Arc<RwLock<bool>>,
}

struct CommandPathCache {
    config_path: Option<String>,
    resolved_path: String,
}

impl Backend {
    pub(crate) fn new(client: Client) -> Self {
        Self {
            client,
            docs: Arc::new(RwLock::new(HashMap::new())),
            roots: Arc::new(RwLock::new(Vec::new())),
            config: Arc::new(RwLock::new(Config::default())),
            command_path_cache: Arc::new(Mutex::new(None)),
            ym2612_convert_path_cache: Arc::new(Mutex::new(None)),
            fm_instrument_cache: Arc::new(Mutex::new(None)),
            playback: Arc::new(Mutex::new(None)),
            playback_seq: Arc::new(Mutex::new(0)),
            last_doc: Arc::new(RwLock::new(None)),
            supports_hierarchy: Arc::new(RwLock::new(false)),
            completion_settings: Arc::new(RwLock::new(CompletionSettings::default())),
            supports_completion_as_is: Arc::new(RwLock::new(false)),
        }
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

    pub(crate) async fn command_path(&self) -> std::result::Result<String, String> {
        let config_path = self.config.read().await.command_path.clone();
        {
            let cache = self.command_path_cache.lock().await;
            if let Some(cached) = cache.as_ref() {
                if cached.config_path == config_path {
                    return Ok(cached.resolved_path.clone());
                }
            }
        }

        let resolved = resolve_command_path(config_path.clone()).await?;
        let mut cache = self.command_path_cache.lock().await;
        *cache = Some(CommandPathCache {
            config_path,
            resolved_path: resolved.clone(),
        });
        Ok(resolved)
    }

    pub(crate) async fn ym2612_convert_path(&self) -> std::result::Result<String, String> {
        let config_path = self.config.read().await.ym2612_convert_path.clone();
        {
            let cache = self.ym2612_convert_path_cache.lock().await;
            if let Some(cached) = cache.as_ref() {
                if cached.config_path == config_path {
                    return Ok(cached.resolved_path.clone());
                }
            }
        }

        let resolved = resolve_ym2612_convert_path(config_path.clone()).await?;
        let mut cache = self.ym2612_convert_path_cache.lock().await;
        *cache = Some(CommandPathCache {
            config_path,
            resolved_path: resolved.clone(),
        });
        Ok(resolved)
    }
}
