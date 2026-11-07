use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use ctrmml_lang_core::completion::FmPatchData;

use crate::backend::Backend;
use crate::completion::{completion_base_dir, completion_search_roots, relative_completion_path};
use crate::workspace_files::InstrumentFile;

/// Patches parsed from each instrument file, reused while the file keeps
/// its size and modification time.
#[derive(Default)]
pub(crate) struct FmPatchCache {
    files: Mutex<HashMap<PathBuf, ParsedFile>>,
}

struct ParsedFile {
    modified: Option<SystemTime>,
    len: u64,
    patches: Arc<[CachedPatch]>,
}

struct CachedPatch {
    name: String,
    has_macros: bool,
    mml: Arc<str>,
}

impl FmPatchCache {
    /// The patches of each file, parsing only new or changed files.
    fn patches(&self, files: &[InstrumentFile]) -> Vec<(PathBuf, Arc<[CachedPatch]>)> {
        files
            .iter()
            .map(|file| {
                let cached = self
                    .files
                    .lock()
                    .expect("patch cache lock")
                    .get(&file.path)
                    .filter(|parsed| parsed.modified == file.modified && parsed.len == file.len)
                    .map(|parsed| Arc::clone(&parsed.patches));
                let patches = cached.unwrap_or_else(|| {
                    let patches: Arc<[CachedPatch]> = parse_instrument_file(&file.path).into();
                    self.files.lock().expect("patch cache lock").insert(
                        file.path.clone(),
                        ParsedFile {
                            modified: file.modified,
                            len: file.len,
                            patches: Arc::clone(&patches),
                        },
                    );
                    patches
                });
                (file.path.clone(), patches)
            })
            .collect()
    }
}

fn parse_instrument_file(file: &Path) -> Vec<CachedPatch> {
    let Ok(data) = std::fs::read(file) else {
        return Vec::new();
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
        return Vec::new();
    };
    parsed
        .patches
        .into_iter()
        .filter_map(|patch| {
            let body = patch_mml_body(&patch.mml?);
            (!body.trim().is_empty()).then(|| CachedPatch {
                name: patch.name,
                has_macros: patch.has_macros,
                mml: body.into(),
            })
        })
        .collect()
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
    pub(crate) async fn fetch_fm_patches(&self, uri: &str) -> Vec<FmPatchData> {
        let roots = &self.session().roots;
        if completion_base_dir(uri, roots).is_none() {
            return Vec::new();
        }
        let mut files = Vec::new();
        let mut seen = HashSet::new();
        for root in completion_search_roots(uri, roots) {
            let listing = self.workspace_files.under(&root).await;
            files.extend(
                listing
                    .instruments
                    .iter()
                    .filter(|file| seen.insert(file.path.clone()))
                    .cloned(),
            );
        }
        let cache = Arc::clone(&self.fm_patches);
        let parsed = tokio::task::spawn_blocking(move || cache.patches(&files))
            .await
            .unwrap_or_default();
        patches_for_core(&parsed, uri, roots)
    }
}

fn patches_for_core(
    files: &[(PathBuf, Arc<[CachedPatch]>)],
    uri: &str,
    roots: &[PathBuf],
) -> Vec<FmPatchData> {
    let mut out = Vec::new();
    for (path, patches) in files {
        let Some(rel_path) = relative_completion_path(path, uri, roots) else {
            continue;
        };
        if rel_path.is_empty() {
            continue;
        }
        out.extend(patches.iter().map(|patch| FmPatchData {
            rel_path: rel_path.clone(),
            name: (!patch.name.trim().is_empty()).then(|| patch.name.clone()),
            mml: Arc::clone(&patch.mml),
            has_macros: patch.has_macros,
        }));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace_files::scan;

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

    fn patch(name: &str, has_macros: bool, mml: &str) -> CachedPatch {
        CachedPatch {
            name: name.to_string(),
            has_macros,
            mml: mml.into(),
        }
    }

    fn sample_dmp() -> Vec<u8> {
        ym2612_format::convert(SAMPLE_MML.as_bytes(), "input.mml", Some("mml"), 0, "dmp")
            .expect("convert mml to dmp")
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
    fn scanned_instrument_files_parse_into_core_patches() {
        let root = temp_test_dir("fm-parse");
        std::fs::create_dir_all(&root).expect("create test directory");
        std::fs::write(root.join("piano.dmp"), sample_dmp()).expect("write dmp");

        let files = scan(&root).instruments;
        let parsed = FmPatchCache::default().patches(&files);
        let core = patches_for_core(&parsed, "not-a-uri", std::slice::from_ref(&root));

        assert_eq!(core.len(), 1);
        assert_eq!(core[0].rel_path, "piano.dmp");
        assert_eq!(core[0].name.as_deref(), Some("piano"));
        assert!(!core[0].mml.starts_with("@1"));
        assert!(core[0].mml.contains("OP4"));
        assert!(!core[0].has_macros);

        std::fs::remove_dir_all(root).expect("remove test directory");
    }

    #[test]
    fn unchanged_files_reuse_their_parsed_patches() {
        let root = temp_test_dir("fm-reuse");
        std::fs::create_dir_all(&root).expect("create test directory");
        std::fs::write(root.join("piano.dmp"), sample_dmp()).expect("write dmp");
        let cache = FmPatchCache::default();

        let files = scan(&root).instruments;
        let first = cache.patches(&files);
        let second = cache.patches(&files);
        assert!(Arc::ptr_eq(&first[0].1, &second[0].1));

        let mut changed = files.clone();
        changed[0].len += 1;
        let third = cache.patches(&changed);
        assert!(!Arc::ptr_eq(&first[0].1, &third[0].1));

        std::fs::remove_dir_all(root).expect("remove test directory");
    }

    #[test]
    fn lfo_comment_carries_no_rate_when_the_patch_has_none() {
        let mut dmp = sample_dmp();
        dmp[3] = 2; // FMS
        dmp[6] = 1; // AMS
        let parsed = ym2612_format::parse(&dmp, "lfo", Some("dmp")).expect("parse dmp");
        let mml = parsed.patches[0].mml.as_deref().expect("mml text");
        assert!(mml.contains("; 'lfo 1 2' ; LFO"), "{mml}");
        assert!(!mml.contains("lforate"), "{mml}");
    }

    #[test]
    fn cached_patches_convert_to_relative_core_contract() {
        let root = temp_test_dir("fm-data");
        let document_dir = root.join("songs");
        let uri = url::Url::from_file_path(document_dir.join("demo.mml"))
            .expect("file URL")
            .to_string();
        let files: Vec<(PathBuf, Arc<[CachedPatch]>)> = vec![
            (
                document_dir.join("patches/lead.dmp"),
                vec![patch("Lead", true, " 1,2,3 ")].into(),
            ),
            (
                root.join("shared/bass.fui"),
                vec![patch("", false, " 4,5,6 ")].into(),
            ),
        ];

        let core = patches_for_core(&files, &uri, std::slice::from_ref(&root));
        assert_eq!(core.len(), 2);
        assert_eq!(core[0].rel_path, "patches/lead.dmp");
        assert_eq!(core[0].name.as_deref(), Some("Lead"));
        assert_eq!(&*core[0].mml, " 1,2,3 ");
        assert!(core[0].has_macros);
        assert_eq!(core[1].rel_path, "../shared/bass.fui");
        assert_eq!(core[1].name, None);
    }
}
