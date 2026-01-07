use tower_lsp::lsp_types::{Diagnostic, DiagnosticSeverity, Position, Range};

use crate::backend::Backend;
use crate::ctrmml_cmd::{output_message, run_ctrmml_cmd};
use crate::diagnostics::{diagnostic_for_check, diagnostics_for_check_report, CheckReport};
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

        let output = if let Some(text) = text.as_deref() {
            let path = uri_to_path(&uri).ok_or_else(|| "invalid file uri".to_string())?;
            run_ctrmml_cmd(&cmd_path, "check", Some(text), |cmd| {
                cmd.arg("check")
                    .arg("--json")
                    .arg("--stdin")
                    .arg("--path")
                    .arg(&path);
            })
            .await?
        } else {
            let file_path = uri_to_path(&uri).ok_or_else(|| "invalid file uri".to_string())?;
            run_ctrmml_cmd(&cmd_path, "check", None, |cmd| {
                cmd.arg("check").arg("--json").arg(&file_path);
            })
            .await?
        };

        let text = text
            .or_else(|| read_file_text(&uri))
            .unwrap_or_default();
        let diagnostics = if let Some(report) = parse_check_report(&output) {
            diagnostics_for_check_report(&text, &report)
        } else if !output.status.success() {
            let mut out = Vec::new();
            if let Some(message) = output_message(&output) {
                if let Some(diag) = diagnostic_for_check(&text, &message) {
                    out.push(diag);
                } else {
                    out.push(Diagnostic {
                        range: Range {
                            start: Position::new(0, 0),
                            end: Position::new(0, 0),
                        },
                        severity: Some(DiagnosticSeverity::ERROR),
                        source: Some("ctrmml-check".to_string()),
                        message,
                        ..Diagnostic::default()
                    });
                }
            }
            out
        } else {
            Vec::new()
        };

        if let Ok(uri) = uri.parse() {
            let _ = self.client.publish_diagnostics(uri, diagnostics, None).await;
        }

        Ok(())
    }
}

fn parse_check_report(output: &std::process::Output) -> Option<CheckReport> {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let trimmed = stdout.trim();
    if trimmed.is_empty() {
        return None;
    }
    serde_json::from_str(trimmed).ok()
}
