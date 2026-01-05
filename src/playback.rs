use std::process::Stdio;

use tokio::io::{AsyncBufReadExt, BufReader};
use tower_lsp::lsp_types::Diagnostic;

use crate::backend::Backend;
use crate::ctrmml_cmd::spawn_ctrmml_cmd;
use crate::diagnostics::{diagnostics_for_positions, HighlightMessage};
use crate::utils::{read_file_text, uri_to_path};

pub(crate) struct Playback {
    pub(crate) uri: String,
    pub(crate) child: tokio::process::Child,
}

impl Backend {

    pub(crate) async fn start_playback(
        &self,
        uri: String,
        start: Option<(u32, u32)>,
    ) -> std::result::Result<(), String> {
        self.stop_playback().await;

        let cmd_path = self.command_path().await?;
        let text = self
            .docs
            .read()
            .await
            .get(&uri)
            .cloned()
            .or_else(|| read_file_text(&uri));
        let text = text.ok_or_else(|| "failed to read mml text".to_string())?;
        let path = uri_to_path(&uri).ok_or_else(|| "invalid file uri".to_string())?;
        let mut child = spawn_ctrmml_cmd(&cmd_path, "play", &text, |cmd| {
            cmd.arg("play")
                .arg("--stdin")
                .arg("--path")
                .arg(&path)
                .arg("--follow");
            if let Some((line, col)) = start {
                cmd.arg("--start").arg(format!("{line}:{col}"));
            }
            cmd.stdout(Stdio::piped());
        })
        .await?;

        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "failed to capture ctrmml-cmd stdout".to_string())?;

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

    pub(crate) async fn stop_playback(&self) {
        {
            let mut seq = self.playback_seq.lock().await;
            *seq += 1;
        }
        let mut slot = self.playback.lock().await;
        if let Some(mut playback) = slot.take() {
            let _ = playback.child.kill().await;
            if let Ok(uri) = playback.uri.parse() {
                let _ = self.client.publish_diagnostics(uri, Vec::new(), None).await;
            }
        }
    }
}
