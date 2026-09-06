use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::OnceLock;

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

/// Readable binary instrument formats, keyed by lowercase extension.
fn fm_instrument_extensions() -> &'static HashSet<String> {
    static EXTENSIONS: OnceLock<HashSet<String>> = OnceLock::new();
    EXTENSIONS.get_or_init(|| {
        ym2612_format::formats()
            .into_iter()
            .filter(|format| format.can_read && !format.is_text)
            .flat_map(|format| format.extensions)
            .map(|extension| extension.to_ascii_lowercase())
            .collect()
    })
}

pub(crate) fn is_fm_instrument(path: &std::path::Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| fm_instrument_extensions().contains(&ext.to_ascii_lowercase()))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fm_instrument_extensions_come_from_the_format_list() {
        assert!(is_fm_instrument(std::path::Path::new("lead.dmp")));
        assert!(is_fm_instrument(std::path::Path::new("song.vgz")));
        assert!(is_fm_instrument(std::path::Path::new("strings.TFI")));
        assert!(!is_fm_instrument(std::path::Path::new("demo.mml")));
        assert!(!is_fm_instrument(std::path::Path::new("notes.txt")));
    }
}
