use crate::backend::Backend;
use crate::ctrmml_cmd::{output_message, run_ctrmml_cmd};
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
    ) -> std::result::Result<std::path::PathBuf, String> {
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
        let output = run_ctrmml_cmd(&cmd_path, "export", Some(&text), |cmd| {
            cmd.arg("export")
                .arg("--stdin")
                .arg("--path")
                .arg(&original_path);
            match format {
                ExportFormat::Vgm => {
                    cmd.arg("--vgm");
                }
                ExportFormat::Wav => {
                    cmd.arg("--wav");
                }
            };
            cmd.arg("--out").arg(&out_path);
        })
        .await?;
        if !output.status.success() {
            if let Some(message) = output_message(&output) {
                return Err(message);
            }
            return Err("ctrmml-cmd export failed".to_string());
        }
        Ok(out_path)
    }
}
