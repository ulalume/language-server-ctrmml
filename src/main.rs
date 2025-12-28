mod backend;
mod completion;
mod config;
mod ctrmml_cmd;
mod diagnostics;
mod export;
mod lsp;
mod playback;
mod utils;

use tower_lsp::{LspService, Server};

use crate::backend::Backend;

#[tokio::main]
async fn main() {
    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();

    let (service, socket) = LspService::new(Backend::new);
    Server::new(stdin, stdout, socket).serve(service).await;
}
