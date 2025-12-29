use std::process::Stdio;
use tokio::io::AsyncWriteExt;
use tokio::process::Command as TokioCommand;
use tower_lsp::lsp_types::{Diagnostic, DiagnosticSeverity, Position, Range};

use crate::backend::Backend;
use crate::diagnostics::diagnostic_for_check;
use crate::utils::{is_mml_uri, read_file_text, uri_to_path};

impl Backend {

    pub(crate) async fn run_check(&self, uri: String) -> std::result::Result<(), String> {
        if !is_mml_uri(&uri) {
            return Ok(());
        }

        let cmd_path = self.command_path().await?;
        let text = self
            .docs
            .read()
            .await
            .get(&uri)
            .cloned()
            .or_else(|| read_file_text(&uri));

        let output = if let Some(text) = text {
            let path = uri_to_path(&uri).ok_or_else(|| "invalid file uri".to_string())?;
            let mut child = TokioCommand::new(cmd_path)
                .arg("check")
                .arg("--stdin")
                .arg("--path")
                .arg(&path)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .map_err(|e| format!("failed to run ctrmml-cmd check: {e}"))?;

            if let Some(mut stdin) = child.stdin.take() {
                stdin
                    .write_all(text.as_bytes())
                    .await
                    .map_err(|e| format!("failed to write ctrmml-cmd stdin: {e}"))?;
            }

            child
                .wait_with_output()
                .await
                .map_err(|e| format!("failed to run ctrmml-cmd check: {e}"))?
        } else {
            let file_path = uri_to_path(&uri).ok_or_else(|| "invalid file uri".to_string())?;
            TokioCommand::new(cmd_path)
                .arg("check")
                .arg(&file_path)
                .output()
                .await
                .map_err(|e| format!("failed to run ctrmml-cmd check: {e}"))?
        };

        let mut diagnostics: Vec<Diagnostic> = Vec::new();
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();
            let stdout = String::from_utf8_lossy(&output.stdout).to_string();
            let message = if !stderr.trim().is_empty() { stderr } else { stdout };
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
