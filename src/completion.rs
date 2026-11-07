use std::collections::HashSet;
use std::path::{Path, PathBuf};

use pathdiff::diff_paths;

use crate::backend::Backend;
use crate::utils::uri_to_dir;

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

impl Backend {
    /// WAV files reachable from the document, keyed relative to its
    /// directory. If the URI cannot be resolved, the first workspace root
    /// is the relative base.
    pub(crate) async fn pcm_paths(&self, uri: &str) -> Vec<String> {
        let roots = &self.session().roots;
        let mut paths = Vec::new();
        let mut seen = HashSet::new();
        for root in completion_search_roots(uri, roots) {
            let files = self.workspace_files.under(&root).await;
            for path in &files.wav {
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
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
