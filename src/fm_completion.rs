use std::collections::HashSet;
use std::path::PathBuf;
use std::time::SystemTime;

use ctrmml_lang_core::completion::FmPatchData;
use walkdir::WalkDir;

use crate::backend::Backend;
use crate::completion::{completion_base_dir, completion_search_roots, relative_completion_path};
use crate::utils::is_fm_instrument;

const CACHE_TTL_SECS: u64 = 60;

pub(crate) struct FmInstrumentCache {
    entries: Vec<CachedPatch>,
    last_scan: SystemTime,
    roots: Vec<PathBuf>,
    library_version: String,
}

#[derive(Clone)]
struct CachedPatch {
    file: String,
    name: String,
    has_macros: bool,
    mml: String,
}

impl FmInstrumentCache {
    fn is_valid(&self, roots: &[PathBuf], library_version: &str) -> bool {
        self.library_version == library_version
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
            if !entry.path().is_file() || !is_fm_instrument(entry.path()) {
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

fn parse_instruments(files: &[PathBuf]) -> Vec<CachedPatch> {
    let mut patches = Vec::new();
    for file in files {
        let Ok(data) = std::fs::read(file) else {
            continue;
        };
        // The file stem is the fallback patch name for formats that carry none.
        let stem = file
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_default();
        let extension = file
            .extension()
            .map(|extension| extension.to_string_lossy().into_owned())
            .unwrap_or_default();
        let Ok(parsed) = ym2612_format::parse(&data, &stem, Some(&extension)) else {
            continue;
        };
        let path = file.to_string_lossy().into_owned();
        for patch in parsed.patches {
            let Some(mml) = patch.mml else {
                continue;
            };
            let body = patch_mml_body(&mml);
            if body.trim().is_empty() {
                continue;
            }
            patches.push(CachedPatch {
                file: path.clone(),
                name: patch.name,
                has_macros: patch.has_macros,
                mml: body,
            });
        }
    }
    patches
}

/// Drop the `@N fm` header: everything after the first `fm`, minus one
/// leading space. Text without `fm` is returned unchanged.
fn patch_mml_body(mml: &str) -> String {
    let Some(position) = mml.find("fm") else {
        return mml.to_string();
    };
    let body = &mml[position + 2..];
    body.strip_prefix(' ').unwrap_or(body).to_string()
}

impl Backend {
    pub(crate) async fn fetch_fm_patches(&self, uri: &str, roots: &[PathBuf]) -> Vec<FmPatchData> {
        if completion_base_dir(uri, roots).is_none() {
            return Vec::new();
        }
        let library_version = ym2612_format::version();
        let scan_roots = completion_search_roots(uri, roots);

        {
            let cache = self.fm_instrument_cache.lock().await;
            if let Some(cached) = cache.as_ref() {
                if cached.is_valid(&scan_roots, library_version) {
                    return patches_for_core(&cached.entries, uri, roots);
                }
            }
        }

        let scan_uri = uri.to_string();
        let scan_root_paths = roots.to_vec();
        let patches = tokio::task::spawn_blocking(move || {
            let files = scan_instrument_files(&scan_uri, &scan_root_paths);
            parse_instruments(&files)
        })
        .await
        .unwrap_or_default();
        let result = patches_for_core(&patches, uri, roots);

        let mut cache = self.fm_instrument_cache.lock().await;
        *cache = Some(FmInstrumentCache {
            entries: patches,
            last_scan: SystemTime::now(),
            roots: scan_roots,
            library_version: library_version.to_string(),
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

    const SAMPLE_MML: &str = "@1 fm ; sample\n\
; ALG  FB\n\
    0   5\n\
;  AR  DR  SR  RR  SL  TL  KS  ML  DT SSG\n\
   25   8   6   3   5  32   0   7   1   0 ; OP1\n\
   30  16   8   4   3  28   0   5   2   0 ; OP2\n\
   29   6   7   4   4  34   0   3   5   0 ; OP3\n\
   30   8   5   3   7   7   0   1   0   0 ; OP4\n";

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

    fn cache(
        roots: Vec<PathBuf>,
        library_version: &str,
        last_scan: SystemTime,
    ) -> FmInstrumentCache {
        FmInstrumentCache {
            entries: Vec::new(),
            last_scan,
            roots,
            library_version: library_version.to_string(),
        }
    }

    #[test]
    fn cache_valid_with_matching_roots_library_and_fresh_scan() {
        let cache = cache(vec![PathBuf::from("/project")], "0.3.0", SystemTime::now());
        assert!(cache.is_valid(&[PathBuf::from("/project")], "0.3.0"));
    }

    #[test]
    fn cache_invalid_when_library_version_differs() {
        let cache = cache(vec![PathBuf::from("/project")], "0.3.0", SystemTime::now());
        assert!(!cache.is_valid(&[PathBuf::from("/project")], "0.4.0"));
    }

    #[test]
    fn cache_invalid_when_scan_roots_differ() {
        let cache = cache(vec![PathBuf::from("/project")], "0.3.0", SystemTime::now());
        assert!(!cache.is_valid(&[PathBuf::from("/other")], "0.3.0"));
    }

    #[test]
    fn cache_invalid_when_scan_is_stale() {
        let cache = cache(
            vec![PathBuf::from("/project")],
            "0.3.0",
            SystemTime::UNIX_EPOCH,
        );
        assert!(!cache.is_valid(&[PathBuf::from("/project")], "0.3.0"));
    }

    #[test]
    fn patch_mml_body_drops_the_instrument_header() {
        assert_eq!(
            patch_mml_body("@1 fm ; name\n    0   5\n"),
            "; name\n    0   5\n"
        );
        assert_eq!(patch_mml_body("@1 fm\n    0   5\n"), "\n    0   5\n");
        assert_eq!(
            patch_mml_body("no instrument header"),
            "no instrument header"
        );
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

    #[cfg(unix)]
    #[test]
    fn scan_instrument_files_follows_supported_file_symlinks() {
        use std::os::unix::fs::symlink;

        let root = temp_test_dir("fm-symlink");
        std::fs::create_dir_all(root.join("nested")).expect("create test directory");
        std::fs::write(root.join("patch-data"), b"patch").expect("write target");
        symlink(root.join("patch-data"), root.join("nested/lead.dmp")).expect("create symlink");

        let files = scan_instrument_files("not-a-uri", std::slice::from_ref(&root));
        assert_eq!(files, vec![root.join("nested/lead.dmp")]);

        std::fs::remove_dir_all(root).expect("remove test directory");
    }

    #[test]
    fn scanned_instrument_files_parse_into_core_patches() {
        let root = temp_test_dir("fm-parse");
        std::fs::create_dir_all(&root).expect("create test directory");
        let dmp = ym2612_format::convert(SAMPLE_MML.as_bytes(), "input.mml", Some("mml"), 0, "dmp")
            .expect("convert mml to dmp");
        std::fs::write(root.join("piano.dmp"), &dmp).expect("write dmp");

        let files = scan_instrument_files("not-a-uri", std::slice::from_ref(&root));
        let patches = parse_instruments(&files);
        let core = patches_for_core(&patches, "not-a-uri", std::slice::from_ref(&root));

        assert_eq!(core.len(), 1);
        assert_eq!(core[0].rel_path, "piano.dmp");
        assert_eq!(core[0].name.as_deref(), Some("piano"));
        assert!(!core[0].mml.starts_with("@1"));
        assert!(core[0].mml.contains("OP4"));
        assert!(!core[0].has_macros);

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
