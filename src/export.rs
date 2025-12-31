use crate::backend::Backend;
use crate::ctrmml_cmd_ffi;
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

        let text = self
            .docs
            .read()
            .await
            .get(&uri)
            .cloned()
            .or_else(|| read_file_text(&uri));

        if let Some(text) = text.as_deref() {
            let base_dir = original_path
                .parent()
                .ok_or_else(|| "invalid file uri".to_string())?;
            let display_name = original_path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or(".mml");
            match format {
                ExportFormat::Vgm => ctrmml_cmd_ffi::export_vgm_text(
                    text,
                    &base_dir.to_string_lossy(),
                    display_name,
                    &out_path.to_string_lossy(),
                ),
                ExportFormat::Wav => ctrmml_cmd_ffi::export_wav_text(
                    text,
                    &base_dir.to_string_lossy(),
                    display_name,
                    &out_path.to_string_lossy(),
                ),
            }
        } else {
            match format {
                ExportFormat::Vgm => ctrmml_cmd_ffi::export_vgm_file(
                    &original_path.to_string_lossy(),
                    &out_path.to_string_lossy(),
                ),
                ExportFormat::Wav => ctrmml_cmd_ffi::export_wav_file(
                    &original_path.to_string_lossy(),
                    &out_path.to_string_lossy(),
                ),
            }
        }
    }
}
