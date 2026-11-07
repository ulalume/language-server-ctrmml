use std::collections::HashMap;
use std::time::Duration;

use tokio::task::AbortHandle;
use tower_lsp::lsp_types::{Diagnostic, DiagnosticSeverity, MessageType, Position, Range};

use crate::backend::Backend;
use crate::ctrmml_cmd::{output_message, run_ctrmml_cmd};
use crate::diagnostics::{diagnostics_for_check_report, publish, CheckReport};
use crate::utils::{is_mml_uri, uri_to_path};

/// Wait after an edit before checking, so a burst of keystrokes runs one check.
pub(crate) const CHECK_DEBOUNCE: Duration = Duration::from_millis(150);

/// The latest check scheduled for each document.
#[derive(Default)]
pub(crate) struct CheckQueue {
    next: u64,
    latest: HashMap<String, (u64, AbortHandle)>,
}

impl Backend {
    /// Check `uri` after `delay`, replacing any check still pending or
    /// running for it. Only the latest check publishes.
    pub(crate) fn schedule_check(&self, uri: String, delay: Duration) {
        if !is_mml_uri(&uri) {
            return;
        }
        let mut queue = self.checks.lock().expect("check queue lock");
        queue.next += 1;
        let seq = queue.next;
        let backend = self.clone();
        let task_uri = uri.clone();
        let task = tokio::spawn(async move {
            tokio::time::sleep(delay).await;
            if let Err(err) = backend.run_check(&task_uri, seq).await {
                backend
                    .client
                    .log_message(MessageType::WARNING, format!("ctrmml check: {err}"))
                    .await;
            }
        });
        if let Some((_, previous)) = queue.latest.insert(uri, (seq, task.abort_handle())) {
            previous.abort();
        }
    }

    pub(crate) fn cancel_check(&self, uri: &str) {
        let removed = self
            .checks
            .lock()
            .expect("check queue lock")
            .latest
            .remove(uri);
        if let Some((_, task)) = removed {
            task.abort();
        }
    }

    fn is_latest_check(&self, uri: &str, seq: u64) -> bool {
        self.checks
            .lock()
            .expect("check queue lock")
            .latest
            .get(uri)
            .is_some_and(|(latest, _)| *latest == seq)
    }

    async fn run_check(&self, uri: &str, seq: u64) -> std::result::Result<(), String> {
        let cmd_path = self.command_path().await?;
        let path = uri_to_path(uri).ok_or_else(|| "invalid file uri".to_string())?;
        let text = self.document_text(uri).await;

        let output = if let Some(text) = text.as_deref() {
            run_ctrmml_cmd(&cmd_path, "check", Some(text), |cmd| {
                cmd.arg("check")
                    .arg("--json")
                    .arg("--stdin")
                    .arg("--path")
                    .arg(&path);
            })
            .await?
        } else {
            run_ctrmml_cmd(&cmd_path, "check", None, |cmd| {
                cmd.arg("check").arg("--json").arg(&path);
            })
            .await?
        };

        let diagnostics = check_diagnostics(text.as_deref().unwrap_or_default(), &output);

        // Hold the store while deciding, so an older check cannot publish
        // after a newer one.
        let mut store = self.diagnostics.lock().await;
        if !self.is_latest_check(uri, seq) {
            return Ok(());
        }
        let merged = store.set_check(uri, diagnostics);
        publish(&self.client, uri, merged).await;
        Ok(())
    }
}

/// Diagnostics for `ctrmml-cmd check --json` output. Output that is not a
/// report becomes one error at the start of the document.
fn check_diagnostics(text: &str, output: &std::process::Output) -> Vec<Diagnostic> {
    if let Some(report) = parse_check_report(output) {
        return diagnostics_for_check_report(text, &report);
    }
    output_message(output)
        .map(|message| Diagnostic {
            range: Range {
                start: Position::new(0, 0),
                end: Position::new(0, 0),
            },
            severity: Some(DiagnosticSeverity::ERROR),
            source: Some("ctrmml-check".to_string()),
            message,
            ..Diagnostic::default()
        })
        .into_iter()
        .collect()
}

fn parse_check_report(output: &std::process::Output) -> Option<CheckReport> {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let trimmed = stdout.trim();
    if trimmed.is_empty() {
        return None;
    }
    serde_json::from_str(trimmed).ok()
}

#[cfg(test)]
mod tests {
    use std::process::{ExitStatus, Output};

    use super::*;

    fn output(stdout: &str, stderr: &str) -> Output {
        Output {
            status: ExitStatus::default(),
            stdout: stdout.as_bytes().to_vec(),
            stderr: stderr.as_bytes().to_vec(),
        }
    }

    #[test]
    fn report_messages_become_diagnostics() {
        let report =
            r#"{"errors":[{"message":"bad","line":1,"col":3,"length":1,"code":""}],"warnings":[]}"#;
        let diagnostics = check_diagnostics("A cd\n", &output(report, ""));
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].range.start, Position::new(0, 2));
    }

    #[test]
    fn unreadable_output_becomes_one_error_at_the_start() {
        let diagnostics = check_diagnostics("A cd\n", &output("", "song.mml:1:3: bad\n"));
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].message, "song.mml:1:3: bad");
        assert_eq!(diagnostics[0].range.start, Position::new(0, 0));
    }

    #[test]
    fn a_clean_run_without_a_report_publishes_nothing() {
        assert!(check_diagnostics("A cd\n", &output("", "")).is_empty());
    }
}
