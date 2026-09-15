use std::process::Stdio;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{ChildStdin, Command as TokioCommand};
use tokio::sync::watch;
use tower_lsp::lsp_types::{Diagnostic, MessageType};

use crate::backend::Backend;
use crate::ctrmml_cmd::CTRMML_CMD_NAME;
use crate::diagnostics::{
    clear_playback_diagnostics, diagnostic_for_playback_error, diagnostics_for_positions,
    expire_playback_diagnostics_on_check, publish_playback_diagnostics, PlaybackMessage,
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
    mode: PlaybackMode,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum PlaybackMode {
    Document,
    // Preview positions refer to synthesized MML, never to `uri`'s document.
    Preview,
}

impl PlaybackMode {
    fn hot_reload(self) -> bool {
        self == Self::Document
    }

    /// Only document playback maps onto the document's own text, so
    /// only it may publish diagnostics for the document URI. Preview
    /// playback reports through the client message path instead.
    fn owns_document_diagnostics(self) -> bool {
        self == Self::Document
    }
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
        self.start_playback_inner(uri, text, start, PlaybackMode::Document)
            .await
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
        self.start_playback_inner(uri, text, start, PlaybackMode::Preview)
            .await
    }

    async fn start_playback_inner(
        &self,
        uri: String,
        text: String,
        start: Option<(u32, u32)>,
        mode: PlaybackMode,
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
        if mode.hot_reload() {
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
        write_initial(&mut stdin, &text, mode.hot_reload()).await?;

        // Hot-reload: spawn a dedicated writer task fed by a latest-wins
        // `watch` channel. `did_change` only does an O(1) `Sender::send`,
        // never blocks on the pipe, and many fast keystrokes naturally
        // coalesce into one write (the writer sees only the latest body
        // after each turn).
        //
        // Non-hot-reload: the child wants EOF on stdin so it can proceed
        // past its initial-read; let `stdin` drop here.
        let update_tx = if mode.hot_reload() {
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
                mode,
            });
        }

        let client = self.client.clone();
        let docs = self.docs.clone();
        let store = self.diagnostics.clone();
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
                let text = match &msg {
                    PlaybackMessage::Highlight { .. } if mode.owns_document_diagnostics() => docs
                        .read()
                        .await
                        .get(&uri_clone)
                        .cloned()
                        .or_else(|| read_file_text(&uri_clone))
                        .unwrap_or_default(),
                    _ => String::new(),
                };
                match playback_outcome(mode, &text, &msg) {
                    PlaybackOutcome::Diagnostics(diags) => {
                        if matches!(msg, PlaybackMessage::PlaybackError { .. }) {
                            playback_error_published = true;
                        }
                        publish_playback_diagnostics(&client, &store, &uri_clone, diags).await;
                    }
                    PlaybackOutcome::Message(message) => {
                        client.show_message(MessageType::ERROR, message).await;
                    }
                    PlaybackOutcome::Ignore => {}
                }
            }

            if *seq.lock().await == token {
                match playback_exit(mode, playback_error_published) {
                    PlaybackExit::Clear => {
                        clear_playback_diagnostics(&client, &store, &uri_clone).await;
                    }
                    PlaybackExit::ExpireOnNextCheck => {
                        expire_playback_diagnostics_on_check(&store, &uri_clone).await;
                    }
                    PlaybackExit::Leave => {}
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
        if !playback.mode.hot_reload() || playback.uri != uri {
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
            if playback.mode.owns_document_diagnostics() {
                clear_playback_diagnostics(&self.client, &self.diagnostics, &playback.uri).await;
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

/// What a `ctrmml-cmd play` message does to the document's diagnostics.
#[derive(Debug, PartialEq, Eq)]
enum PlaybackOutcome {
    /// Replace the playback layer of the document's diagnostics.
    Diagnostics(Vec<Diagnostic>),
    /// Report to the user without touching the document's diagnostics.
    Message(String),
    Ignore,
}

fn playback_outcome(mode: PlaybackMode, text: &str, message: &PlaybackMessage) -> PlaybackOutcome {
    match message {
        PlaybackMessage::Highlight { positions, .. } if mode.owns_document_diagnostics() => {
            PlaybackOutcome::Diagnostics(diagnostics_for_positions(text, positions))
        }
        PlaybackMessage::Highlight { .. } => PlaybackOutcome::Ignore,
        PlaybackMessage::PlaybackError { message } if mode.owns_document_diagnostics() => {
            PlaybackOutcome::Diagnostics(vec![diagnostic_for_playback_error(message.clone())])
        }
        PlaybackMessage::PlaybackError { message } => PlaybackOutcome::Message(message.clone()),
    }
}

/// What happens to the playback layer once the child exits.
#[derive(Debug, PartialEq, Eq)]
enum PlaybackExit {
    Clear,
    /// A playback error stays visible until the next check replaces it.
    ExpireOnNextCheck,
    Leave,
}

fn playback_exit(mode: PlaybackMode, playback_error_published: bool) -> PlaybackExit {
    if !mode.owns_document_diagnostics() {
        PlaybackExit::Leave
    } else if playback_error_published {
        PlaybackExit::ExpireOnNextCheck
    } else {
        PlaybackExit::Clear
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn highlight_message() -> PlaybackMessage {
        parse_playback_message(
            r#"{"type":"highlight","ticks":24,"positions":[{"line":0,"col":2}]}"#,
        )
        .expect("highlight JSON should parse")
    }

    fn playback_error_message() -> PlaybackMessage {
        parse_playback_message(r#"{"type":"playback_error","message":"Playback error: pcm"}"#)
            .expect("playback_error JSON should parse")
    }

    #[test]
    fn preview_playback_produces_no_document_highlight_diagnostics() {
        let outcome = playback_outcome(PlaybackMode::Preview, "A cdef", &highlight_message());

        assert_eq!(outcome, PlaybackOutcome::Ignore);
    }

    #[test]
    fn document_playback_still_produces_highlight_diagnostics() {
        let PlaybackOutcome::Diagnostics(diagnostics) =
            playback_outcome(PlaybackMode::Document, "A cdef", &highlight_message())
        else {
            panic!("document playback should publish highlight diagnostics");
        };

        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].source.as_deref(), Some("ctrmml-playback"));
        assert_eq!(diagnostics[0].range.start.line, 0);
        assert_eq!(diagnostics[0].range.start.character, 2);
    }

    #[test]
    fn preview_playback_error_is_reported_as_a_message() {
        let outcome = playback_outcome(PlaybackMode::Preview, "", &playback_error_message());

        assert_eq!(
            outcome,
            PlaybackOutcome::Message("Playback error: pcm".to_string())
        );
    }

    #[test]
    fn document_playback_error_becomes_a_diagnostic() {
        let PlaybackOutcome::Diagnostics(diagnostics) =
            playback_outcome(PlaybackMode::Document, "", &playback_error_message())
        else {
            panic!("document playback error should publish a diagnostic");
        };

        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].message, "Playback error: pcm");
    }

    #[test]
    fn preview_playback_never_touches_document_diagnostics_on_exit() {
        assert_eq!(
            playback_exit(PlaybackMode::Preview, false),
            PlaybackExit::Leave
        );
        assert_eq!(
            playback_exit(PlaybackMode::Preview, true),
            PlaybackExit::Leave
        );
    }

    #[test]
    fn document_playback_clears_its_own_markers_on_exit() {
        assert_eq!(
            playback_exit(PlaybackMode::Document, false),
            PlaybackExit::Clear
        );
    }

    #[test]
    fn document_playback_error_outlives_the_child_until_the_next_check() {
        assert_eq!(
            playback_exit(PlaybackMode::Document, true),
            PlaybackExit::ExpireOnNextCheck
        );
    }

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
