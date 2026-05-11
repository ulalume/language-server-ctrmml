//! Phase 4.3 spike: build tower-lsp for `wasm32-unknown-unknown`.
//!
//! Drop-in `LanguageServer` impl — the simplest possible — so we can
//! exercise the trait surface and see what the dependency graph
//! actually pulls in.

use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::{
    InitializeParams, InitializeResult, ServerCapabilities,
};
use tower_lsp::{Client, LanguageServer};

pub struct SpikeBackend {
    _client: Client,
}

#[tower_lsp::async_trait]
impl LanguageServer for SpikeBackend {
    async fn initialize(&self, _: InitializeParams) -> Result<InitializeResult> {
        Ok(InitializeResult {
            capabilities: ServerCapabilities::default(),
            server_info: None,
        })
    }

    async fn shutdown(&self) -> Result<()> {
        Ok(())
    }
}
