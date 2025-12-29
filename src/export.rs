use std::process::Stdio;

use tokio::io::AsyncWriteExt;
use tokio::process::Command as TokioCommand;

use crate::backend::Backend;
use crate::utils::{read_file_text, uri_to_path};

pub(crate) enum ExportFormat {
    Vgm,
    Wav,
}

impl Backend {
    pub(crate) async fn run_export(
        &self,
        uri: String,
        format: ExportFormat,
    ) -> std::result::Result<(), String> {
        let original_path = uri_to_path(&uri).ok_or_else(|| "invalid file uri".to_string())?;
        let out_path = match format {
            ExportFormat::Vgm => original_path.with_extension("vgm"),
            ExportFormat::Wav => original_path.with_extension("wav"),
        };

        let cmd_path = self.command_path().await?;
        let text = self
            .docs
            .read()
            .await
            .get(&uri)
            .cloned()
            .or_else(|| read_file_text(&uri))
            .ok_or_else(|| "failed to read mml text".to_string())?;
        let mut cmd = TokioCommand::new(cmd_path);
        cmd.arg("export")
            .arg("--stdin")
            .arg("--path")
            .arg(&original_path);
        match format {
            ExportFormat::Vgm => cmd.arg("--vgm"),
            ExportFormat::Wav => cmd.arg("--wav"),
        };
        cmd.arg("--out").arg(out_path);
        cmd.stdin(Stdio::piped());

        let mut child = cmd
            .spawn()
            .map_err(|e| format!("failed to run ctrmml-cmd: {e}"))?;
        if let Some(mut stdin) = child.stdin.take() {
            stdin
                .write_all(text.as_bytes())
                .await
                .map_err(|e| format!("failed to write ctrmml-cmd stdin: {e}"))?;
        }

        let output = child
            .wait_with_output()
            .await
            .map_err(|e| format!("failed to run ctrmml-cmd: {e}"))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();
            let stdout = String::from_utf8_lossy(&output.stdout).to_string();
            let message = if !stderr.trim().is_empty() { stderr } else { stdout };
            let message = message.trim();
            if message.is_empty() {
                return Err("ctrmml-cmd export failed".to_string());
            }
            return Err(message.to_string());
        }
        Ok(())
    }
}
