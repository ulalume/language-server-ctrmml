use tower_lsp::lsp_types::{Diagnostic, DiagnosticSeverity, Position, Range};

use crate::backend::Backend;
use crate::ctrmml_cmd_ffi;
use crate::diagnostics::diagnostic_for_check;
use crate::utils::{is_mml_uri, read_file_text, uri_to_path};

impl Backend {
    pub(crate) async fn run_check(&self, uri: String) -> std::result::Result<(), String> {
        if !is_mml_uri(&uri) {
            return Ok(());
        }

        let text = self
            .docs
            .read()
            .await
            .get(&uri)
            .cloned()
            .or_else(|| read_file_text(&uri));

        let file_path = uri_to_path(&uri).ok_or_else(|| "invalid file uri".to_string())?;

        let check_result = if let Some(text) = text.as_deref() {
            let base_dir = file_path
                .parent()
                .ok_or_else(|| "invalid file uri".to_string())?;
            let display_name = file_path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or(".mml");
            ctrmml_cmd_ffi::check_text(text, &base_dir.to_string_lossy(), display_name)
        } else {
            ctrmml_cmd_ffi::check_file(&file_path.to_string_lossy())
        };

        let mut diagnostics: Vec<Diagnostic> = Vec::new();
        if let Err(message) = check_result {
            let text = self
                .docs
                .read()
                .await
                .get(&uri)
                .cloned()
                .or_else(|| read_file_text(&uri))
                .unwrap_or_default();

            if let Some(diag) = diagnostic_for_check(&text, &message) {
                diagnostics.push(diag);
            } else if !message.trim().is_empty() {
                diagnostics.push(Diagnostic {
                    range: Range {
                        start: Position::new(0, 0),
                        end: Position::new(0, 0),
                    },
                    severity: Some(DiagnosticSeverity::ERROR),
                    source: Some("ctrmml-check".to_string()),
                    message: message.trim().to_string(),
                    ..Diagnostic::default()
                });
            }
        }

        if let Ok(uri) = uri.parse() {
            let _ = self.client.publish_diagnostics(uri, diagnostics, None).await;
        }

        Ok(())
    }
}
