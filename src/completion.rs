use std::collections::HashSet;
use std::path::{Path, PathBuf};

use pathdiff::diff_paths;
use walkdir::WalkDir;

use crate::utils::{is_wav, uri_to_dir};

pub(crate) fn completion_search_roots(uri: &str, roots: &[PathBuf]) -> Vec<PathBuf> {
    let mut search_roots = Vec::new();
    if let Some(document_dir) = uri_to_dir(uri) {
        search_roots.push(document_dir);
    }
    search_roots.extend(roots.iter().cloned());

    let mut seen = HashSet::new();
    search_roots.retain(|path| seen.insert(path.clone()));
    search_roots
}

pub(crate) fn completion_base_dir(uri: &str, roots: &[PathBuf]) -> Option<PathBuf> {
    uri_to_dir(uri).or_else(|| roots.first().cloned())
}

pub(crate) fn relative_completion_path(
    path: &Path,
    uri: &str,
    roots: &[PathBuf],
) -> Option<String> {
    let base_dir = completion_base_dir(uri, roots)?;
    diff_paths(path, base_dir).map(|relative| relative.to_string_lossy().replace('\\', "/"))
}

/// List WAV files reachable from the document, keyed relative to its directory.
/// If the URI cannot be resolved, the first workspace root is the relative base.
pub(crate) fn scan_pcm_paths(uri: &str, roots: &[PathBuf]) -> Vec<String> {
    let mut paths = Vec::new();
    let mut seen = HashSet::new();
    for root in completion_search_roots(uri, roots) {
        for entry in WalkDir::new(root).into_iter().filter_map(Result::ok) {
            let path = entry.path();
            if !entry.file_type().is_file() || !is_wav(path) {
                continue;
            }
            let Some(relative) = relative_completion_path(path, uri, roots) else {
                continue;
            };
            if seen.insert(relative.clone()) {
                paths.push(relative);
            }
        }
    }
    paths
}

#[cfg(test)]
mod tests {
    use std::time::SystemTime;

    use super::*;

    fn temp_test_dir(label: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        std::env::temp_dir().join(format!("ctrmml-c5-{label}-{}-{unique}", std::process::id()))
    }

    #[test]
    fn relative_path_prefers_document_directory() {
        let root = PathBuf::from("/workspace");
        let uri = "file:///workspace/songs/demo.mml";
        let path = PathBuf::from("/workspace/assets/kick.wav");
        assert_eq!(
            relative_completion_path(&path, uri, &[root]).as_deref(),
            Some("../assets/kick.wav")
        );
    }

    #[test]
    fn relative_path_falls_back_to_workspace_root() {
        let root = PathBuf::from("/workspace");
        let path = PathBuf::from("/workspace/assets/kick.wav");
        assert_eq!(
            relative_completion_path(&path, "not-a-uri", &[root]).as_deref(),
            Some("assets/kick.wav")
        );
    }

    #[test]
    fn scan_pcm_paths_lists_only_wav_files() {
        let root = temp_test_dir("pcm-scan");
        std::fs::create_dir_all(root.join("samples")).expect("create test directory");
        std::fs::write(root.join("samples/kick.wav"), b"wave").expect("write wav");
        std::fs::write(root.join("samples/ignore.txt"), b"nope").expect("write txt");

        let paths = scan_pcm_paths("not-a-uri", std::slice::from_ref(&root));
        assert_eq!(paths, vec!["samples/kick.wav"]);

        std::fs::remove_dir_all(root).expect("remove test directory");
    }
}
