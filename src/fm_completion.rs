use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::SystemTime;

use ctrmml_lang_core::completion::FmPatchData;
use walkdir::WalkDir;

use crate::backend::Backend;
use crate::completion::{completion_base_dir, completion_search_roots, relative_completion_path};
use crate::utils::is_fm_instrument;
use crate::ym2612_convert::{run_ym2612_convert, InfoResponse};

const CACHE_TTL_SECS: u64 = 60;

pub(crate) struct FmInstrumentCache {
    entries: Vec<CachedPatch>,
    last_scan: SystemTime,
    roots: Vec<PathBuf>,
    cmd_path: String,
}

#[derive(Clone)]
struct CachedPatch {
    file: String,
    name: String,
    has_macros: bool,
    mml: String,
}

impl FmInstrumentCache {
    fn is_valid(&self, roots: &[PathBuf], cmd_path: &str) -> bool {
        self.cmd_path == cmd_path
            && self.roots == roots
            && self
                .last_scan
                .elapsed()
                .map(|elapsed| elapsed.as_secs() < CACHE_TTL_SECS)
                .unwrap_or(false)
    }
}

fn scan_instrument_files(uri: &str, roots: &[PathBuf]) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut seen_files = HashSet::new();
    for root in completion_search_roots(uri, roots) {
        for entry in WalkDir::new(root).into_iter().filter_map(Result::ok) {
            if !entry.file_type().is_file() || !is_fm_instrument(entry.path()) {
                continue;
            }
            let path = entry.path().to_path_buf();
            if seen_files.insert(path.clone()) {
                files.push(path);
            }
        }
    }
    files
}

async fn parse_instruments(cmd_path: &str, files: &[PathBuf]) -> Vec<CachedPatch> {
    if files.is_empty() {
        return Vec::new();
    }

    // The converter identifies patches by basename. Batch unique basenames,
    // but run duplicate basenames one file at a time so paths stay unambiguous.
    let mut basename_groups: HashMap<String, Vec<&PathBuf>> = HashMap::new();
    for file in files {
        basename_groups
            .entry(path_basename(file))
            .or_default()
            .push(file);
    }

    let mut unique_files = Vec::new();
    let mut unique_path_map = HashMap::new();
    let mut duplicate_files = Vec::new();
    for (basename, group) in &basename_groups {
        if group.len() == 1 {
            unique_files.push(group[0].clone());
            unique_path_map.insert(basename.clone(), group[0].to_string_lossy().to_string());
        } else {
            duplicate_files.extend(group.iter().map(|file| (*file).clone()));
        }
    }

    let mut patches = Vec::new();
    if !unique_files.is_empty() {
        if let Some(response) = run_info(cmd_path, &unique_files).await {
            for patch in response.patches {
                let basename = patch.file.unwrap_or_default();
                let file = unique_path_map.get(&basename).cloned().unwrap_or(basename);
                patches.push(CachedPatch {
                    file,
                    name: patch.name,
                    has_macros: patch.has_macros,
                    mml: patch.mml,
                });
            }
        }
    }

    for file in duplicate_files {
        let full_path = file.to_string_lossy().to_string();
        if let Some(response) = run_info(cmd_path, std::slice::from_ref(&file)).await {
            for patch in response.patches {
                patches.push(CachedPatch {
                    file: full_path.clone(),
                    name: patch.name,
                    has_macros: patch.has_macros,
                    mml: patch.mml,
                });
            }
        }
    }
    patches
}

fn path_basename(path: &std::path::Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .map(str::to_string)
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
}

async fn run_info(cmd_path: &str, files: &[PathBuf]) -> Option<InfoResponse> {
    let output = run_ym2612_convert(cmd_path, "info --json", |command| {
        command.arg("info").arg("--json");
        for file in files {
            command.arg(file);
        }
    })
    .await
    .ok()?;
    if !output.status.success() {
        return None;
    }
    serde_json::from_slice(&output.stdout).ok()
}

impl Backend {
    pub(crate) async fn fetch_fm_patches(&self, uri: &str, roots: &[PathBuf]) -> Vec<FmPatchData> {
        if completion_base_dir(uri, roots).is_none() {
            return Vec::new();
        }
        let cmd_path = match self.ym2612_convert_path().await {
            Ok(path) => path,
            Err(_) => return Vec::new(),
        };
        let scan_roots = completion_search_roots(uri, roots);

        {
            let cache = self.fm_instrument_cache.lock().await;
            if let Some(cached) = cache.as_ref() {
                if cached.is_valid(&scan_roots, &cmd_path) {
                    return patches_for_core(&cached.entries, uri, roots);
                }
            }
        }

        let files = scan_instrument_files(uri, roots);
        let patches = parse_instruments(&cmd_path, &files).await;
        let result = patches_for_core(&patches, uri, roots);

        let mut cache = self.fm_instrument_cache.lock().await;
        *cache = Some(FmInstrumentCache {
            entries: patches,
            last_scan: SystemTime::now(),
            roots: scan_roots,
            cmd_path,
        });
        result
    }
}

fn patches_for_core(patches: &[CachedPatch], uri: &str, roots: &[PathBuf]) -> Vec<FmPatchData> {
    patches
        .iter()
        .filter(|patch| !patch.mml.trim().is_empty())
        .filter_map(|patch| {
            let rel_path = relative_completion_path(std::path::Path::new(&patch.file), uri, roots)?;
            if rel_path.is_empty() {
                return None;
            }
            Some(FmPatchData {
                rel_path,
                name: (!patch.name.trim().is_empty()).then(|| patch.name.clone()),
                mml: patch.mml.clone(),
                has_macros: patch.has_macros,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_test_dir(label: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        std::env::temp_dir().join(format!("ctrmml-c5-{label}-{}-{unique}", std::process::id()))
    }

    fn patch(file: &str, name: &str, has_macros: bool, mml: &str) -> CachedPatch {
        CachedPatch {
            file: file.to_string(),
            name: name.to_string(),
            has_macros,
            mml: mml.to_string(),
        }
    }

    fn cache(roots: Vec<PathBuf>, cmd_path: &str, last_scan: SystemTime) -> FmInstrumentCache {
        FmInstrumentCache {
            entries: Vec::new(),
            last_scan,
            roots,
            cmd_path: cmd_path.to_string(),
        }
    }

    #[test]
    fn cache_valid_with_matching_roots_command_and_fresh_scan() {
        let cache = cache(
            vec![PathBuf::from("/project")],
            "/bin/ym2612_convert",
            SystemTime::now(),
        );
        assert!(cache.is_valid(&[PathBuf::from("/project")], "/bin/ym2612_convert"));
    }

    #[test]
    fn cache_invalid_when_command_differs() {
        let cache = cache(
            vec![PathBuf::from("/project")],
            "/bin/ym2612_convert",
            SystemTime::now(),
        );
        assert!(!cache.is_valid(&[PathBuf::from("/project")], "/other/converter"));
    }

    #[test]
    fn cache_invalid_when_scan_roots_differ() {
        let cache = cache(
            vec![PathBuf::from("/project")],
            "/bin/ym2612_convert",
            SystemTime::now(),
        );
        assert!(!cache.is_valid(&[PathBuf::from("/other")], "/bin/ym2612_convert"));
    }

    #[test]
    fn cache_invalid_when_scan_is_stale() {
        let cache = cache(
            vec![PathBuf::from("/project")],
            "/bin/ym2612_convert",
            SystemTime::UNIX_EPOCH,
        );
        assert!(!cache.is_valid(&[PathBuf::from("/project")], "/bin/ym2612_convert"));
    }

    #[test]
    fn scan_instrument_files_keeps_supported_extensions() {
        let root = temp_test_dir("fm-scan");
        std::fs::create_dir_all(root.join("nested")).expect("create test directory");
        std::fs::write(root.join("nested/lead.dmp"), b"patch").expect("write dmp");
        std::fs::write(root.join("nested/ignore.txt"), b"nope").expect("write txt");

        let files = scan_instrument_files("not-a-uri", std::slice::from_ref(&root));
        assert_eq!(files, vec![root.join("nested/lead.dmp")]);

        std::fs::remove_dir_all(root).expect("remove test directory");
    }

    #[test]
    fn cached_patches_convert_to_relative_core_contract() {
        let root = temp_test_dir("fm-data");
        let document_dir = root.join("songs");
        let uri = url::Url::from_file_path(document_dir.join("demo.mml"))
            .expect("file URL")
            .to_string();
        let patches = vec![
            patch(
                &document_dir.join("patches/lead.dmp").to_string_lossy(),
                "Lead",
                true,
                " 1,2,3 ",
            ),
            patch(
                &root.join("shared/bass.fui").to_string_lossy(),
                "",
                false,
                " 4,5,6 ",
            ),
            patch(
                &root.join("shared/empty.dmp").to_string_lossy(),
                "Empty",
                false,
                "   ",
            ),
        ];

        let core = patches_for_core(&patches, &uri, std::slice::from_ref(&root));
        assert_eq!(core.len(), 2);
        assert_eq!(core[0].rel_path, "patches/lead.dmp");
        assert_eq!(core[0].name.as_deref(), Some("Lead"));
        assert_eq!(core[0].mml, " 1,2,3 ");
        assert!(core[0].has_macros);
        assert_eq!(core[1].rel_path, "../shared/bass.fui");
        assert_eq!(core[1].name, None);
    }
}
