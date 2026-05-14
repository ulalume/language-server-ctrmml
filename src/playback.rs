use std::process::Stdio;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{ChildStdin, Command as TokioCommand};
use tower_lsp::lsp_types::Diagnostic;

use crate::backend::Backend;
use crate::ctrmml_cmd::CTRMML_CMD_NAME;
use crate::diagnostics::{diagnostics_for_positions, HighlightMessage};
use crate::utils::{read_file_text, uri_to_path};

/// A running `ctrmml-cmd play` subprocess.
///
/// `hot_reload` distinguishes the main document playback (where
/// `did_change` notifications should be forwarded to the running
/// renderer) from preview playback (a synthesized MML that the user
/// can't edit live). When `hot_reload` is true, `stdin` stays open and
/// holds the writable end of ctrmml-cmd's framing protocol.
pub(crate) struct Playback {
    pub(crate) uri: String,
    pub(crate) child: tokio::process::Child,
    pub(crate) stdin: Option<ChildStdin>,
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
        cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());

        let mut child = cmd
            .spawn()
            .map_err(|e| format!("failed to spawn {CTRMML_CMD_NAME} play: {e}"))?;

        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| format!("failed to capture {CTRMML_CMD_NAME} stdin"))?;
        write_initial(&mut stdin, &text, hot_reload).await?;

        // For non-hot-reload runs the child wants EOF on stdin so it can
        // proceed past the read; for hot-reload runs we hold the pipe
        // open so later did_change events can land more frames.
        let stdin_holder = if hot_reload { Some(stdin) } else { None };

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
                stdin: stdin_holder,
                hot_reload,
            });
        }

        let client = self.client.clone();
        let docs = self.docs.clone();
        let seq = self.playback_seq.clone();
        let uri_clone = uri.clone();
        tokio::spawn(async move {
            let mut reader = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = reader.next_line().await {
                if *seq.lock().await != token {
                    break;
                }
                let msg = match serde_json::from_str::<HighlightMessage>(&line) {
                    Ok(msg) => msg,
                    Err(_) => continue,
                };
                if msg.kind != "highlight" {
                    continue;
                }

                let text = docs
                    .read()
                    .await
                    .get(&uri_clone)
                    .cloned()
                    .or_else(|| read_file_text(&uri_clone))
                    .unwrap_or_default();
                let diags: Vec<Diagnostic> = diagnostics_for_positions(&text, &msg.positions);
                if let Ok(uri) = uri_clone.parse() {
                    let _ = client.publish_diagnostics(uri, diags, None).await;
                }
            }

            if *seq.lock().await == token {
                if let Ok(uri) = uri_clone.parse() {
                    let _ = client.publish_diagnostics(uri, Vec::new(), None).await;
                }
            }
        });

        Ok(())
    }

    /// Push an updated MML body to a running playback. No-op when no
    /// playback is active, when the active session was started without
    /// hot-reload (e.g. preview), or when the URI doesn't match the
    /// one being played. Errors are swallowed so a transient pipe
    /// write failure doesn't cascade into the LSP request handler — the
    /// next did_change will retry, or the user can hit Stop.
    pub(crate) async fn push_playback_update(&self, uri: &str, text: &str) {
        let mut slot = self.playback.lock().await;
        let playback = match slot.as_mut() {
            Some(p) => p,
            None => return,
        };
        if !playback.hot_reload || playback.uri != uri {
            return;
        }
        let stdin = match playback.stdin.as_mut() {
            Some(s) => s,
            None => return,
        };
        if let Err(err) = write_update_frame(stdin, text).await {
            eprintln!("ctrmml-lsp: hot-reload write failed: {err}");
            // Drop the stdin handle so subsequent didChange events stop
            // hammering a dead pipe; ctrmml-cmd will keep playing the
            // last good version until the user hits Stop.
            playback.stdin = None;
        }
    }

    pub(crate) async fn stop_playback(&self) {
        {
            let mut seq = self.playback_seq.lock().await;
            *seq += 1;
        }
        let mut slot = self.playback.lock().await;
        if let Some(mut playback) = slot.take() {
            // Close stdin first — ctrmml-cmd's reader thread will see EOF
            // and clean up before we send SIGKILL.
            drop(playback.stdin.take());
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
