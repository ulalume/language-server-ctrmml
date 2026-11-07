//! Cached listings of the WAV and FM instrument files under each search
//! root, for path and patch completion.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use walkdir::{DirEntry, WalkDir};

use crate::utils::{is_fm_instrument, is_wav};

/// A listing older than this is refreshed in the background on next use.
const RESCAN_AFTER: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct InstrumentFile {
    pub(crate) path: PathBuf,
    pub(crate) modified: Option<SystemTime>,
    pub(crate) len: u64,
}

#[derive(Debug, Default)]
pub(crate) struct RootFiles {
    pub(crate) wav: Vec<PathBuf>,
    pub(crate) instruments: Vec<InstrumentFile>,
}

struct Listing {
    files: Arc<RootFiles>,
    scanned: Instant,
    refreshing: bool,
}

#[derive(Default)]
pub(crate) struct WorkspaceFiles {
    listings: Mutex<HashMap<PathBuf, Listing>>,
}

impl WorkspaceFiles {
    /// The files under `root`. The first call waits for a scan; later calls
    /// return the last listing at once and refresh it in the background
    /// once it is stale.
    pub(crate) async fn under(self: &Arc<Self>, root: &Path) -> Arc<RootFiles> {
        {
            let mut listings = self.listings.lock().expect("listing lock");
            if let Some(listing) = listings.get_mut(root) {
                if !listing.refreshing && listing.scanned.elapsed() >= RESCAN_AFTER {
                    listing.refreshing = true;
                    let files = Arc::clone(self);
                    let root = root.to_path_buf();
                    tokio::spawn(async move { files.rescan(root).await });
                }
                return Arc::clone(&listing.files);
            }
        }
        self.rescan(root.to_path_buf()).await
    }

    async fn rescan(&self, root: PathBuf) -> Arc<RootFiles> {
        let scan_root = root.clone();
        let files = tokio::task::spawn_blocking(move || scan(&scan_root))
            .await
            .unwrap_or_default();
        let files = Arc::new(files);
        self.listings.lock().expect("listing lock").insert(
            root,
            Listing {
                files: Arc::clone(&files),
                scanned: Instant::now(),
                refreshing: false,
            },
        );
        files
    }
}

/// Walk `root`, skipping hidden entries, `node_modules` and `target`.
/// File symlinks count as the files they point at.
pub(crate) fn scan(root: &Path) -> RootFiles {
    let mut files = RootFiles::default();
    let entries = WalkDir::new(root)
        .into_iter()
        .filter_entry(|entry| entry.depth() == 0 || !is_skipped(entry))
        .filter_map(Result::ok);
    for entry in entries {
        let file_type = entry.file_type();
        let is_file = file_type.is_file() || (file_type.is_symlink() && entry.path().is_file());
        if !is_file {
            continue;
        }
        let path = entry.path();
        if is_wav(path) {
            files.wav.push(path.to_path_buf());
        } else if is_fm_instrument(path) {
            let metadata = std::fs::metadata(path).ok();
            files.instruments.push(InstrumentFile {
                path: path.to_path_buf(),
                modified: metadata.as_ref().and_then(|meta| meta.modified().ok()),
                len: metadata.map_or(0, |meta| meta.len()),
            });
        }
    }
    files
}

fn is_skipped(entry: &DirEntry) -> bool {
    let name = entry.file_name().to_string_lossy();
    name.starts_with('.')
        || (entry.file_type().is_dir() && matches!(name.as_ref(), "node_modules" | "target"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_test_dir(label: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        std::env::temp_dir().join(format!(
            "ctrmml-files-{label}-{}-{unique}",
            std::process::id()
        ))
    }

    #[test]
    fn scan_lists_wav_and_instrument_files() {
        let root = temp_test_dir("scan");
        std::fs::create_dir_all(root.join("samples")).expect("create test directory");
        std::fs::write(root.join("samples/kick.wav"), b"wave").expect("write wav");
        std::fs::write(root.join("samples/lead.dmp"), b"patch").expect("write dmp");
        std::fs::write(root.join("samples/notes.txt"), b"nope").expect("write txt");

        let files = scan(&root);
        assert_eq!(files.wav, vec![root.join("samples/kick.wav")]);
        assert_eq!(files.instruments.len(), 1);
        assert_eq!(files.instruments[0].path, root.join("samples/lead.dmp"));
        assert_eq!(files.instruments[0].len, 5);

        std::fs::remove_dir_all(root).expect("remove test directory");
    }

    #[test]
    fn scan_skips_hidden_entries_and_build_folders() {
        let root = temp_test_dir("skip");
        for dir in [".git", "node_modules/pkg", "target/debug", "songs"] {
            std::fs::create_dir_all(root.join(dir)).expect("create test directory");
        }
        for file in [
            ".git/a.wav",
            "node_modules/pkg/b.wav",
            "target/debug/c.wav",
            "songs/.d.wav",
            "songs/e.wav",
        ] {
            std::fs::write(root.join(file), b"wave").expect("write wav");
        }

        assert_eq!(scan(&root).wav, vec![root.join("songs/e.wav")]);

        std::fs::remove_dir_all(root).expect("remove test directory");
    }

    #[cfg(unix)]
    #[test]
    fn scan_follows_file_symlinks() {
        use std::os::unix::fs::symlink;

        let root = temp_test_dir("symlink");
        std::fs::create_dir_all(root.join("samples")).expect("create test directory");
        std::fs::write(root.join("sample-data"), b"wave").expect("write target");
        symlink(root.join("sample-data"), root.join("samples/kick.wav")).expect("create symlink");
        symlink(root.join("sample-data"), root.join("samples/lead.dmp")).expect("create symlink");

        let files = scan(&root);
        assert_eq!(files.wav, vec![root.join("samples/kick.wav")]);
        assert_eq!(files.instruments[0].len, 4);

        std::fs::remove_dir_all(root).expect("remove test directory");
    }

    #[tokio::test]
    async fn listings_are_reused_until_stale() {
        let root = temp_test_dir("reuse");
        std::fs::create_dir_all(&root).expect("create test directory");
        std::fs::write(root.join("a.wav"), b"wave").expect("write wav");
        let files = Arc::new(WorkspaceFiles::default());

        assert_eq!(files.under(&root).await.wav.len(), 1);
        std::fs::write(root.join("b.wav"), b"wave").expect("write wav");
        assert_eq!(files.under(&root).await.wav.len(), 1);

        std::fs::remove_dir_all(root).expect("remove test directory");
    }
}
