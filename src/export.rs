use tokio::process::Command as TokioCommand;

use crate::backend::Backend;
use crate::utils::uri_to_path;

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
        let (file_path, temp_path) = self.materialize_mml(&uri).await?;
        let out_path = match format {
            ExportFormat::Vgm => original_path.with_extension("vgm"),
            ExportFormat::Wav => original_path.with_extension("wav"),
        };

        let cmd_path = self.command_path().await?;
        let mut cmd = TokioCommand::new(cmd_path);
        cmd.arg("export").arg(file_path);
        match format {
            ExportFormat::Vgm => cmd.arg("--vgm"),
            ExportFormat::Wav => cmd.arg("--wav"),
        };
        cmd.arg("--out").arg(out_path);

        let status = cmd
            .status()
            .await
            .map_err(|e| format!("failed to run ctrmml-cmd: {e}"))?;
        if let Some(path) = temp_path {
            let _ = std::fs::remove_file(path);
        }
        if !status.success() {
            return Err("ctrmml-cmd export failed".to_string());
        }
        Ok(())
    }
}
