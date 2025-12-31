mod backend;
mod check;
mod completion;
mod config;
mod docs;
mod ctrmml_cmd;
mod ctrmml_cmd_ffi;
mod diagnostics;
mod export;
mod hover;
mod lsp;
mod playback;
mod utils;

use serde_json::Value;
use tower::Service;
use tower_lsp::jsonrpc::Request as JsonRpcRequest;
use tower_lsp::{LspService, Server};

use crate::backend::Backend;

struct ShutdownParamFix<S> {
    inner: S,
}

impl<S> ShutdownParamFix<S> {
    fn new(inner: S) -> Self {
        Self { inner }
    }
}

impl<S> Service<JsonRpcRequest> for ShutdownParamFix<S>
where
    S: Service<JsonRpcRequest>,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = S::Future;

    fn poll_ready(
        &mut self,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: JsonRpcRequest) -> Self::Future {
        let req = if req.method() == "shutdown" {
            match req.params() {
                Some(Value::Null) => {
                    let (method, id, _) = req.into_parts();
                    let mut builder = JsonRpcRequest::build(method);
                    if let Some(id) = id {
                        builder = builder.id(id);
                    }
                    builder.finish()
                }
                _ => req,
            }
        } else {
            req
        };
        self.inner.call(req)
    }
}

#[tokio::main]
async fn main() {
    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();

    let (service, socket) = LspService::new(Backend::new);
    let service = ShutdownParamFix::new(service);
    Server::new(stdin, stdout, socket).serve(service).await;
}
