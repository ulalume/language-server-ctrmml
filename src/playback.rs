use std::process::Stdio;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{ChildStdin, Command as TokioCommand};
use tokio::sync::watch;
use tower_lsp::lsp_types::Diagnostic;

use crate::backend::Backend;
use crate::ctrmml_cmd::CTRMML_CMD_NAME;
use crate::diagnostics::{
    diagnostic_for_playback_error, diagnostics_for_positions, PlaybackMessage,
};
use crate::utils::{read_file_text, uri_to_path};

/// A running `ctrmml-cmd play` subprocess.
///
/// `hot_reload` distinguishes the main document playback (where
/// `did_change` notifications should be forwarded to the running
/// renderer) from preview playback (a synthesized MML the user can't
/// edit live). When hot-reload is on, `update_tx` is the latest-wins
/// channel feeding a dedicated writer task that owns the child's
/// stdin pipe.
pub(crate) struct Playback {
    pub(crate) uri: String,
    pub(crate) child: tokio::process::Child,
    pub(crate) update_tx: Option<watch::Sender<Option<String>>>,
    pub(crate) hot_reload: bool,
}

impl Backend {
    /// Start playback of the document `uri`, fetching the text from the
    /// LSP doc cache (or file on disk as a fallback). Enables hot-reload
    /// so subsequent `did_change` events can update the renderer
    /// without restarting it.
    pub(crate) async fn start_playback(
        &self,
        uri: String,
        start: Option<(u32, u32)>,
    ) -> std::result::Result<(), String> {
        let text = self
            .docs
            .read()
            .await
            .get(&uri)
            .cloned()
            .or_else(|| read_file_text(&uri))
            .ok_or_else(|| "failed to read mml text".to_string())?;
        self.start_playback_inner(uri, text, start, true).await
    }

    /// Play a caller-supplied MML body (e.g. a synthesized patch
    /// preview). Hot-reload is disabled — the body isn't tied to a
    /// document the user can edit.
    pub(crate) async fn start_playback_with_text(
        &self,
        uri: String,
        text: String,
        start: Option<(u32, u32)>,
    ) -> std::result::Result<(), String> {
        self.start_playback_inner(uri, text, start, false).await
    }

    async fn start_playback_inner(
        &self,
        uri: String,
        text: String,
        start: Option<(u32, u32)>,
        hot_reload: bool,
    ) -> std::result::Result<(), String> {
        self.stop_playback().await;

        let cmd_path = self.command_path().await?;
        let path = uri_to_path(&uri).ok_or_else(|| "invalid file uri".to_string())?;

        let mut cmd = TokioCommand::new(&cmd_path);
        cmd.arg("play")
            .arg("--stdin")
            .arg("--path")
            .arg(&path)
            .arg("--follow");
        if hot_reload {
            cmd.arg("--hot-reload");
        }
        if let Some((line, col)) = start {
            cmd.arg("--start").arg(format!("{line}:{col}"));
        }
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let mut child = cmd
            .spawn()
            .map_err(|e| format!("failed to spawn {CTRMML_CMD_NAME} play: {e}"))?;

        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| format!("failed to capture {CTRMML_CMD_NAME} stdin"))?;
        write_initial(&mut stdin, &text, hot_reload).await?;

        // Hot-reload: spawn a dedicated writer task fed by a latest-wins
        // `watch` channel. `did_change` only does an O(1) `Sender::send`,
        // never blocks on the pipe, and many fast keystrokes naturally
        // coalesce into one write (the writer sees only the latest body
        // after each turn).
        //
        // Non-hot-reload: the child wants EOF on stdin so it can proceed
        // past its initial-read; let `stdin` drop here.
        let update_tx = if hot_reload {
            let (tx, mut rx) = watch::channel::<Option<String>>(None);
            tokio::spawn(async move {
                while rx.changed().await.is_ok() {
                    let body = rx.borrow_and_update().clone();
                    if let Some(text) = body {
                        if let Err(err) = write_update_frame(&mut stdin, &text).await {
                            eprintln!("ctrmml-lsp: hot-reload write failed: {err}");
                            break;
                        }
                    }
                }
                // Drop stdin on exit so ctrmml-cmd's reader thread sees EOF.
            });
            Some(tx)
        } else {
            drop(stdin);
            None
        };

        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| format!("failed to capture {CTRMML_CMD_NAME} stdout"))?;

        let token = {
            let mut seq = self.playback_seq.lock().await;
            *seq += 1;
            *seq
        };

        {
            let mut slot = self.playback.lock().await;
            *slot = Some(Playback {
                uri: uri.clone(),
                child,
                update_tx,
                hot_reload,
            });
        }

        let client = self.client.clone();
        let docs = self.docs.clone();
        let seq = self.playback_seq.clone();
        let uri_clone = uri.clone();
        tokio::spawn(async move {
            let mut reader = BufReader::new(stdout).lines();
            let mut playback_error_published = false;
            while let Ok(Some(line)) = reader.next_line().await {
                if *seq.lock().await != token {
                    break;
                }
                let msg = match parse_playback_message(&line) {
                    Ok(msg) => msg,
                    Err(_) => continue,
                };
                let diags: Vec<Diagnostic> = match msg {
                    PlaybackMessage::Highlight { positions, .. } => {
                        let text = docs
                            .read()
                            .await
                            .get(&uri_clone)
                            .cloned()
                            .or_else(|| read_file_text(&uri_clone))
                            .unwrap_or_default();
                        diagnostics_for_positions(&text, &positions)
                    }
                    PlaybackMessage::PlaybackError { message } => {
                        playback_error_published = true;
                        vec![diagnostic_for_playback_error(message)]
                    }
                };
                if let Ok(uri) = uri_clone.parse() {
                    let _ = client.publish_diagnostics(uri, diags, None).await;
                }
            }

            if !playback_error_published && *seq.lock().await == token {
                if let Ok(uri) = uri_clone.parse() {
                    let _ = client.publish_diagnostics(uri, Vec::new(), None).await;
                }
            }
        });

        Ok(())
    }

    /// Push an updated MML body to a running playback. O(1) — replaces
    /// any pending body that hasn't reached the pipe yet so fast typing
    /// doesn't queue stale frames. No-op for non-hot-reload sessions
    /// (e.g. preview) or when the URI doesn't match.
    pub(crate) async fn push_playback_update(&self, uri: &str, text: &str) {
        let slot = self.playback.lock().await;
        let Some(playback) = slot.as_ref() else {
            return;
        };
        if !playback.hot_reload || playback.uri != uri {
            return;
        }
        if let Some(tx) = playback.update_tx.as_ref() {
            // A send error means the writer task exited (write failure
            // earlier); subsequent edits silently no-op until Stop+Play.
            let _ = tx.send(Some(text.to_string()));
        }
    }

    pub(crate) async fn stop_playback(&self) {
        {
            let mut seq = self.playback_seq.lock().await;
            *seq += 1;
        }
        let mut slot = self.playback.lock().await;
        if let Some(mut playback) = slot.take() {
            // Drop the watch sender first so the writer task exits and
            // releases stdin; ctrmml-cmd's reader thread sees EOF before
            // we send SIGKILL.
            drop(playback.update_tx.take());
            let _ = playback.child.kill().await;
            if let Ok(uri) = playback.uri.parse() {
                let _ = self.client.publish_diagnostics(uri, Vec::new(), None).await;
            }
        }
    }
}

async fn write_initial(
    stdin: &mut ChildStdin,
    text: &str,
    hot_reload: bool,
) -> std::result::Result<(), String> {
    let result = if hot_reload {
        write_update_frame(stdin, text).await
    } else {
        stdin.write_all(text.as_bytes()).await
    };
    result.map_err(|e| format!("failed to write {CTRMML_CMD_NAME} stdin: {e}"))
}

async fn write_update_frame(
    stdin: &mut ChildStdin,
    text: &str,
) -> std::result::Result<(), std::io::Error> {
    let header = format!("UPDATE {}\n", text.as_bytes().len());
    stdin.write_all(header.as_bytes()).await?;
    stdin.write_all(text.as_bytes()).await?;
    stdin.write_all(b"\n").await?;
    stdin.flush().await
}

fn parse_playback_message(line: &str) -> serde_json::Result<PlaybackMessage> {
    serde_json::from_str(line)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_playback_error_message() {
        let message = parse_playback_message(
            r#"{"type":"playback_error","message":"PCM mixing is unsupported"}"#,
        )
        .expect("playback_error JSON should parse");

        let PlaybackMessage::PlaybackError { message } = message else {
            panic!("expected playback_error message");
        };
        assert_eq!(message, "PCM mixing is unsupported");
    }
}
