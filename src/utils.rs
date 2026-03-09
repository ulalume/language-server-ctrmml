use std::path::PathBuf;

pub(crate) fn uri_to_path(uri: &str) -> Option<PathBuf> {
    let url = url::Url::parse(uri).ok()?;
    url.to_file_path().ok()
}

pub(crate) fn uri_to_dir(uri: &str) -> Option<PathBuf> {
    let path = uri_to_path(uri)?;
    path.parent().map(|p| p.to_path_buf())
}

pub(crate) fn is_mml_uri(uri: &str) -> bool {
    if let Some(path) = uri_to_path(uri) {
        if let Some(ext) = path.extension().and_then(|ext| ext.to_str()) {
            return ext.eq_ignore_ascii_case("mml");
        }
    }
    false
}

pub(crate) fn read_file_text(uri: &str) -> Option<String> {
    let path = uri_to_path(uri)?;
    std::fs::read_to_string(path).ok()
}

pub(crate) fn line_at(text: &str, line_index: u32) -> Option<String> {
    text.lines().nth(line_index as usize).map(|s| s.to_string())
}

pub(crate) fn is_wav(path: &std::path::Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.eq_ignore_ascii_case("wav"))
        .unwrap_or(false)
}

pub(crate) fn is_fm_instrument(path: &std::path::Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| {
            let lower = ext.to_ascii_lowercase();
            matches!(
                lower.as_str(),
                "dmp" | "fui" | "fur" | "gin" | "rym2612" | "dmf" | "ginpkg"
            )
        })
        .unwrap_or(false)
}
