use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::SystemTime;

use tower_lsp::lsp_types::{
    Command, CompletionItem, CompletionItemKind, CompletionItemLabelDetails, CompletionTextEdit,
    Documentation, InsertTextFormat, InsertTextMode, Position, Range, TextEdit,
};

use ctrmml_lang_core::completion::FmPatchData;
use ctrmml_lang_core::docs::FM_DEFAULT_TEMPLATE;
use walkdir::WalkDir;

use crate::backend::Backend;
use crate::completion::{completion_base_dir, completion_search_roots, FmCompletionKind};
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
        if self.cmd_path != cmd_path {
            return false;
        }
        if self.roots != roots {
            return false;
        }
        self.last_scan
            .elapsed()
            .map(|elapsed| elapsed.as_secs() < CACHE_TTL_SECS)
            .unwrap_or(false)
    }
}

fn scan_instrument_files(uri: &str, roots: &[PathBuf]) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut seen_files = HashSet::new();
    for root in completion_search_roots(uri, roots) {
        for entry in WalkDir::new(&root).into_iter().filter_map(|e| e.ok()) {
            if !entry.file_type().is_file() {
                continue;
            }
            let path = entry.path();
            if !is_fm_instrument(path) {
                continue;
            }
            let canonical = path.to_path_buf();
            if seen_files.insert(canonical.clone()) {
                files.push(canonical);
            }
        }
    }
    files
}

async fn parse_instruments(cmd_path: &str, files: &[PathBuf]) -> Vec<CachedPatch> {
    if files.is_empty() {
        return Vec::new();
    }

    // Partition files: unique basenames can be batched, duplicates run per-file
    let mut basename_groups: HashMap<String, Vec<&PathBuf>> = HashMap::new();
    for f in files {
        let bn = path_basename(f);
        basename_groups.entry(bn).or_default().push(f);
    }

    let mut unique_files = Vec::new();
    let mut unique_path_map: HashMap<String, String> = HashMap::new();
    let mut duplicate_files = Vec::new();
    for (bn, group) in &basename_groups {
        if group.len() == 1 {
            unique_files.push(group[0].clone());
            unique_path_map.insert(bn.clone(), group[0].to_string_lossy().to_string());
        } else {
            duplicate_files.extend(group.iter().map(|f| (*f).clone()));
        }
    }

    let mut all_patches = Vec::new();

    // Batch unique basenames
    if !unique_files.is_empty() {
        if let Some(response) = run_info(cmd_path, &unique_files).await {
            for p in response.patches {
                let bn = p.file.clone().unwrap_or_default();
                let full_path = unique_path_map.get(&bn).cloned().unwrap_or(bn);
                all_patches.push(CachedPatch {
                    file: full_path,
                    name: p.name,
                    has_macros: p.has_macros,
                    mml: p.mml,
                });
            }
        }
    }

    // Per-file for duplicate basenames
    for f in &duplicate_files {
        let full_path = f.to_string_lossy().to_string();
        if let Some(response) = run_info(cmd_path, std::slice::from_ref(f)).await {
            for p in response.patches {
                all_patches.push(CachedPatch {
                    file: full_path.clone(),
                    name: p.name,
                    has_macros: p.has_macros,
                    mml: p.mml,
                });
            }
        }
    }

    all_patches
}

fn path_basename(p: &std::path::Path) -> String {
    file_basename(&p.to_string_lossy()).to_string()
}

async fn run_info(cmd_path: &str, files: &[PathBuf]) -> Option<InfoResponse> {
    let output = run_ym2612_convert(cmd_path, "info --json", |cmd| {
        cmd.arg("info").arg("--json");
        for f in files {
            cmd.arg(f);
        }
    })
    .await
    .ok()?;

    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    serde_json::from_str(&stdout).ok()
}

impl Backend {
    pub(crate) async fn fetch_fm_patches(&self, uri: &str, roots: &[PathBuf]) -> Vec<FmPatchData> {
        let Some(base_dir) = completion_base_dir(uri, roots) else {
            return Vec::new();
        };
        let cmd_path = match self.ym2612_convert_path().await {
            Ok(path) => path,
            Err(_) => return Vec::new(),
        };
        let scan_roots = completion_search_roots(uri, roots);

        {
            let cache = self.fm_instrument_cache.lock().await;
            if let Some(cached) = cache.as_ref() {
                if cached.is_valid(&scan_roots, &cmd_path) {
                    return patches_for_core(&cached.entries, &base_dir);
                }
            }
        }

        let files = scan_instrument_files(uri, roots);
        let patches = parse_instruments(&cmd_path, &files).await;
        let result = patches_for_core(&patches, &base_dir);

        let mut cache = self.fm_instrument_cache.lock().await;
        *cache = Some(FmInstrumentCache {
            entries: patches,
            last_scan: SystemTime::now(),
            roots: scan_roots,
            cmd_path,
        });
        result
    }

    pub(crate) async fn complete_fm_instruments(
        &self,
        uri: &str,
        roots: &[PathBuf],
        kind: &FmCompletionKind,
        line_num: u32,
        col: u32,
    ) -> std::result::Result<Vec<CompletionItem>, String> {
        let hierarchy = *self.supports_hierarchy.read().await;
        let cmd_path = self.ym2612_convert_path().await.ok();

        // No `ym2612_convert` available → still emit the default template
        // so the user can at least insert a fresh FM body.
        let Some(cmd_path) = cmd_path else {
            return Ok(build_items(&[], kind, line_num, col, hierarchy));
        };

        let scan_roots = completion_search_roots(uri, roots);

        // Check cache
        {
            let cache = self.fm_instrument_cache.lock().await;
            if let Some(cached) = cache.as_ref() {
                if cached.is_valid(&scan_roots, &cmd_path) {
                    return Ok(build_items(&cached.entries, kind, line_num, col, hierarchy));
                }
            }
        }

        // Scan and parse
        let files = scan_instrument_files(uri, roots);
        let patches = parse_instruments(&cmd_path, &files).await;

        let items = build_items(&patches, kind, line_num, col, hierarchy);

        // Update cache
        {
            let mut cache = self.fm_instrument_cache.lock().await;
            *cache = Some(FmInstrumentCache {
                entries: patches,
                last_scan: SystemTime::now(),
                roots: scan_roots,
                cmd_path,
            });
        }

        Ok(items)
    }
}

fn patches_for_core(patches: &[CachedPatch], base_dir: &std::path::Path) -> Vec<FmPatchData> {
    patches
        .iter()
        .filter_map(|patch| {
            let file = std::path::Path::new(&patch.file);
            let rel_path = diff_path_from_base(file, base_dir)?;
            Some(FmPatchData {
                rel_path,
                name: (!patch.name.trim().is_empty()).then(|| patch.name.clone()),
                mml: patch.mml.clone(),
                has_macros: patch.has_macros,
            })
        })
        .collect()
}

fn diff_path_from_base(path: &std::path::Path, base_dir: &std::path::Path) -> Option<String> {
    pathdiff::diff_paths(path, base_dir)
        .map(|relative| relative.to_string_lossy().replace('\\', "/"))
}

fn file_basename(path: &str) -> &str {
    if path.is_empty() {
        return "(unknown)";
    }
    std::path::Path::new(path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(path)
}

fn file_stem(path: &str) -> &str {
    let basename = file_basename(path);
    std::path::Path::new(basename)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(basename)
}

fn unique_file_count(patches: &[CachedPatch]) -> usize {
    let mut seen = HashSet::new();
    for p in patches {
        seen.insert(&p.file);
    }
    seen.len()
}

fn build_items(
    patches: &[CachedPatch],
    kind: &FmCompletionKind,
    line_num: u32,
    col: u32,
    hierarchy: bool,
) -> Vec<CompletionItem> {
    match kind {
        FmCompletionKind::SelectFile { fm_end_col } => {
            let mut items = if !hierarchy || unique_file_count(patches) <= 1 {
                build_flat_items(patches, line_num, *fm_end_col, col)
            } else {
                build_file_items(patches, line_num, *fm_end_col, col)
            };
            items.push(default_template_item(line_num, *fm_end_col, col));
            items
        }
        FmCompletionKind::SelectPatch {
            file_key,
            fm_end_col,
        } => build_patch_items(patches, file_key, line_num, *fm_end_col, col),
    }
}

/// Bottom-of-list fallback when none of the workspace's instrument
/// files fit — inserts the same FM template the `fm` keyword used to
/// insert directly before we switched the keyword to re-trigger
/// completion. `sort_text` is `"~default"` so it lands after every
/// file-derived item (`~` sorts after any alphanumeric).
fn default_template_item(line_num: u32, fm_end_col: u32, col: u32) -> CompletionItem {
    let range = Range {
        start: Position::new(line_num, fm_end_col),
        end: Position::new(line_num, col),
    };
    CompletionItem {
        label: "Default FM template".to_string(),
        kind: Some(CompletionItemKind::SNIPPET),
        detail: Some("template".to_string()),
        text_edit: Some(CompletionTextEdit::Edit(TextEdit {
            range,
            new_text: FM_DEFAULT_TEMPLATE.to_string(),
        })),
        insert_text_format: Some(InsertTextFormat::SNIPPET),
        sort_text: Some("~default".to_string()),
        filter_text: Some("default fm template".to_string()),
        ..CompletionItem::default()
    }
}

fn build_flat_items(
    patches: &[CachedPatch],
    line_num: u32,
    fm_end_col: u32,
    col: u32,
) -> Vec<CompletionItem> {
    let range = Range {
        start: Position::new(line_num, fm_end_col),
        end: Position::new(line_num, col),
    };

    patches
        .iter()
        .map(|p| {
            let basename = file_basename(&p.file);

            CompletionItem {
                label: p.name.clone(),
                label_details: if p.file.is_empty() || p.name == file_stem(&p.file) {
                    None
                } else {
                    Some(CompletionItemLabelDetails {
                        detail: None,
                        description: Some(basename.to_string()),
                    })
                },
                kind: Some(CompletionItemKind::VALUE),
                detail: if p.has_macros {
                    Some("[macros]".to_string())
                } else {
                    None
                },
                documentation: if p.file.is_empty() {
                    None
                } else {
                    Some(Documentation::String(basename.to_string()))
                },
                text_edit: Some(CompletionTextEdit::Edit(TextEdit {
                    range,
                    new_text: format!(" {}", p.mml),
                })),
                filter_text: Some(format!("{} {}", p.name, basename)),
                sort_text: Some(p.name.clone()),
                insert_text_mode: Some(InsertTextMode::AS_IS),
                ..CompletionItem::default()
            }
        })
        .collect()
}

fn build_file_items(
    patches: &[CachedPatch],
    line_num: u32,
    fm_end_col: u32,
    col: u32,
) -> Vec<CompletionItem> {
    let range = Range {
        start: Position::new(line_num, fm_end_col),
        end: Position::new(line_num, col),
    };

    // Group patches by file path for O(1) lookup
    let mut file_groups: HashMap<&str, Vec<&CachedPatch>> = HashMap::new();
    for p in patches {
        file_groups.entry(&p.file).or_default().push(p);
    }

    let mut seen = HashSet::new();
    let mut items = Vec::new();

    for p in patches {
        let file_path = &p.file;
        if !seen.insert(file_path.clone()) {
            continue;
        }
        let basename = file_basename(file_path);
        let file_patches = &file_groups[file_path.as_str()];
        let count = file_patches.len();

        if count == 1 {
            // Single patch in file — insert MML directly, no second step
            let single = file_patches[0];
            items.push(CompletionItem {
                label: basename.to_string(),
                kind: Some(CompletionItemKind::VALUE),
                detail: if single.has_macros {
                    Some("[macros]".to_string())
                } else {
                    None
                },
                documentation: if single.file.is_empty() {
                    None
                } else {
                    Some(Documentation::String(basename.to_string()))
                },
                text_edit: Some(CompletionTextEdit::Edit(TextEdit {
                    range,
                    new_text: format!(" {}", single.mml),
                })),
                filter_text: Some(format!("{} {}", single.name, basename)),
                sort_text: Some(basename.to_string()),
                insert_text_mode: Some(InsertTextMode::AS_IS),
                ..CompletionItem::default()
            });
        } else {
            items.push(CompletionItem {
                label: basename.to_string(),
                label_details: Some(CompletionItemLabelDetails {
                    detail: None,
                    description: Some(count.to_string()),
                }),
                kind: Some(CompletionItemKind::FILE),
                text_edit: Some(CompletionTextEdit::Edit(TextEdit {
                    range,
                    new_text: format!(" {basename}/"),
                })),
                filter_text: Some(basename.to_string()),
                sort_text: Some(basename.to_string()),
                command: Some(Command {
                    title: "Trigger suggest".to_string(),
                    command: "editor.action.triggerSuggest".to_string(),
                    arguments: None,
                }),
                ..CompletionItem::default()
            });
        }
    }

    items
}

fn build_patch_items(
    patches: &[CachedPatch],
    file_key: &str,
    line_num: u32,
    fm_end_col: u32,
    col: u32,
) -> Vec<CompletionItem> {
    let range = Range {
        start: Position::new(line_num, fm_end_col),
        end: Position::new(line_num, col),
    };

    patches
        .iter()
        .filter(|p| file_basename(&p.file) == file_key)
        .map(|p| CompletionItem {
            label: p.name.clone(),
            label_details: Some(CompletionItemLabelDetails {
                detail: None,
                description: Some(file_key.to_string()),
            }),
            kind: Some(CompletionItemKind::VALUE),
            detail: if p.has_macros {
                Some("[macros]".to_string())
            } else {
                None
            },
            documentation: if p.file.is_empty() {
                None
            } else {
                Some(Documentation::String(file_basename(&p.file).to_string()))
            },
            text_edit: Some(CompletionTextEdit::Edit(TextEdit {
                range,
                new_text: format!(" {}", p.mml),
            })),
            filter_text: Some(format!("{}/{}", file_key, p.name)),
            sort_text: Some(p.name.clone()),
            insert_text_mode: Some(InsertTextMode::AS_IS),
            ..CompletionItem::default()
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Characterization tests — pin TODAY's behavior ahead of the
// COMPLETION_CORE_PLAN.md rewrite. These exercise only the pure,
// already-parsed-data half of this module (item construction, basename
// grouping/labeling). `scan_instrument_files` (real WalkDir over the
// filesystem) and `parse_instruments`/`run_info`/`Backend::complete_fm_instruments`
// (spawn `ym2612_convert`, or require a live `tower_lsp::Client` for
// `Backend::new`) are not exercised here — see the task report for why.
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

    fn range_text_edit(item: &CompletionItem) -> &TextEdit {
        match item.text_edit.as_ref().expect("expected a text_edit") {
            CompletionTextEdit::Edit(edit) => edit,
            other => panic!("expected CompletionTextEdit::Edit, got {other:?}"),
        }
    }

    // ---------- file_basename / file_stem / path_basename ------------------

    #[test]
    fn file_basename_strips_directory() {
        assert_eq!(file_basename("patches/lead/Lead.dmp"), "Lead.dmp");
    }

    #[test]
    fn file_basename_no_directory_is_identity() {
        assert_eq!(file_basename("Lead.dmp"), "Lead.dmp");
    }

    #[test]
    fn file_basename_empty_is_unknown_placeholder() {
        assert_eq!(file_basename(""), "(unknown)");
    }

    #[test]
    fn file_stem_strips_extension_and_directory() {
        assert_eq!(file_stem("patches/lead/Lead.dmp"), "Lead");
    }

    #[test]
    fn file_stem_no_extension_is_basename() {
        assert_eq!(file_stem("patches/Lead"), "Lead");
    }

    #[test]
    fn path_basename_matches_file_basename() {
        let p = PathBuf::from("patches/lead/Lead.dmp");
        assert_eq!(path_basename(&p), "Lead.dmp");
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
        let base = PathBuf::from("/project/song");
        let patches = vec![
            patch("/project/song/patches/lead.dmp", "Lead", true, " 1,2,3 "),
            patch("/project/shared/bass.fui", "", false, " 4,5,6 "),
        ];

        let core = patches_for_core(&patches, &base);
        assert_eq!(core.len(), 2);
        assert_eq!(core[0].rel_path, "patches/lead.dmp");
        assert_eq!(core[0].name.as_deref(), Some("Lead"));
        assert_eq!(core[0].mml, " 1,2,3 ");
        assert!(core[0].has_macros);
        assert_eq!(core[1].rel_path, "../shared/bass.fui");
        assert_eq!(core[1].name, None);
    }

    // ---------- unique_file_count -------------------------------------------

    #[test]
    fn unique_file_count_dedupes_same_file() {
        let patches = vec![
            patch("a.dmp", "Patch 1", false, "@1 fm"),
            patch("a.dmp", "Patch 2", false, "@1 fm"),
            patch("b.dmp", "Patch 3", false, "@1 fm"),
        ];
        assert_eq!(unique_file_count(&patches), 2);
    }

    #[test]
    fn unique_file_count_empty_is_zero() {
        assert_eq!(unique_file_count(&[]), 0);
    }

    // ---------- FmInstrumentCache::is_valid ---------------------------------

    #[test]
    fn cache_valid_with_matching_roots_and_cmd_path_and_fresh_scan() {
        let cache = FmInstrumentCache {
            entries: Vec::new(),
            last_scan: SystemTime::now(),
            roots: vec![PathBuf::from("/proj")],
            cmd_path: "/usr/bin/ym2612_convert".to_string(),
        };
        assert!(cache.is_valid(&[PathBuf::from("/proj")], "/usr/bin/ym2612_convert"));
    }

    #[test]
    fn cache_invalid_when_cmd_path_differs() {
        let cache = FmInstrumentCache {
            entries: Vec::new(),
            last_scan: SystemTime::now(),
            roots: vec![PathBuf::from("/proj")],
            cmd_path: "/usr/bin/ym2612_convert".to_string(),
        };
        assert!(!cache.is_valid(&[PathBuf::from("/proj")], "/other/ym2612_convert"));
    }

    #[test]
    fn cache_invalid_when_roots_differ() {
        let cache = FmInstrumentCache {
            entries: Vec::new(),
            last_scan: SystemTime::now(),
            roots: vec![PathBuf::from("/proj")],
            cmd_path: "/usr/bin/ym2612_convert".to_string(),
        };
        assert!(!cache.is_valid(&[PathBuf::from("/other")], "/usr/bin/ym2612_convert"));
    }

    #[test]
    fn cache_invalid_when_scan_older_than_ttl() {
        let cache = FmInstrumentCache {
            entries: Vec::new(),
            last_scan: SystemTime::UNIX_EPOCH,
            roots: vec![PathBuf::from("/proj")],
            cmd_path: "/usr/bin/ym2612_convert".to_string(),
        };
        assert!(!cache.is_valid(&[PathBuf::from("/proj")], "/usr/bin/ym2612_convert"));
    }

    // ---------- default_template_item ---------------------------------------

    #[test]
    fn default_template_item_shape() {
        let item = default_template_item(3, 10, 12);
        assert_eq!(item.label, "Default FM template");
        assert_eq!(item.kind, Some(CompletionItemKind::SNIPPET));
        assert_eq!(item.detail.as_deref(), Some("template"));
        assert_eq!(item.insert_text_format, Some(InsertTextFormat::SNIPPET));
        // `~` sorts after alphanumerics, so this always lands last among
        // file-derived items.
        assert_eq!(item.sort_text.as_deref(), Some("~default"));
        assert_eq!(item.filter_text.as_deref(), Some("default fm template"));
        let edit = range_text_edit(&item);
        assert_eq!(edit.range.start, Position::new(3, 10));
        assert_eq!(edit.range.end, Position::new(3, 12));
        assert_eq!(edit.new_text, FM_DEFAULT_TEMPLATE);
    }

    // ---------- build_flat_items ---------------------------------------------

    #[test]
    fn flat_item_shows_basename_when_name_differs_from_stem() {
        let patches = vec![patch("patches/lead.dmp", "Lead Synth", false, "@1 fm ...")];
        let items = build_flat_items(&patches, 5, 4, 8);
        assert_eq!(items.len(), 1);
        let item = &items[0];
        assert_eq!(item.label, "Lead Synth");
        assert_eq!(
            item.label_details
                .as_ref()
                .and_then(|d| d.description.clone()),
            Some("lead.dmp".to_string())
        );
        assert_eq!(
            item.documentation,
            Some(Documentation::String("lead.dmp".to_string()))
        );
        assert_eq!(item.kind, Some(CompletionItemKind::VALUE));
        assert_eq!(item.detail, None);
        assert_eq!(item.filter_text.as_deref(), Some("Lead Synth lead.dmp"));
        assert_eq!(item.sort_text.as_deref(), Some("Lead Synth"));
        assert_eq!(item.insert_text_mode, Some(InsertTextMode::AS_IS));
        let edit = range_text_edit(item);
        assert_eq!(edit.range.start, Position::new(5, 4));
        assert_eq!(edit.range.end, Position::new(5, 8));
        // The inserted MML always gets a leading space prepended, even if
        // the user already typed one after `fm` themselves.
        assert_eq!(edit.new_text, " @1 fm ...");
    }

    #[test]
    fn flat_item_suppresses_label_details_when_name_matches_stem() {
        // Name equal to the file stem is considered redundant — no
        // `label_details` — but `documentation` is unaffected by that
        // check (it only cares whether `file` is empty), so it still
        // shows the basename. This asymmetry is today's behavior.
        let patches = vec![patch("patches/lead.dmp", "lead", false, "@1 fm ...")];
        let items = build_flat_items(&patches, 0, 0, 0);
        assert_eq!(items[0].label_details, None);
        assert_eq!(
            items[0].documentation,
            Some(Documentation::String("lead.dmp".to_string()))
        );
    }

    #[test]
    fn flat_item_empty_file_suppresses_details_and_documentation() {
        let patches = vec![patch("", "Default", false, "@1 fm ...")];
        let items = build_flat_items(&patches, 0, 0, 0);
        assert_eq!(items[0].label_details, None);
        assert_eq!(items[0].documentation, None);
    }

    #[test]
    fn flat_item_has_macros_sets_detail_tag() {
        let patches = vec![patch("a.dmp", "Patch", true, "@1 fm ...")];
        let items = build_flat_items(&patches, 0, 0, 0);
        assert_eq!(items[0].detail.as_deref(), Some("[macros]"));
    }

    #[test]
    fn flat_items_preserve_input_order() {
        let patches = vec![
            patch("b.dmp", "B", false, "mb"),
            patch("a.dmp", "A", false, "ma"),
        ];
        let items = build_flat_items(&patches, 0, 0, 0);
        assert_eq!(
            items.iter().map(|i| i.label.as_str()).collect::<Vec<_>>(),
            vec!["B", "A"]
        );
    }

    // ---------- build_file_items (hierarchy mode) -----------------------------

    #[test]
    fn file_item_single_patch_inserts_mml_directly() {
        let patches = vec![patch("patches/lead.dmp", "Lead", false, "@1 fm ...")];
        let items = build_file_items(&patches, 2, 4, 6);
        assert_eq!(items.len(), 1);
        let item = &items[0];
        // Single-patch files skip the two-step picker: label is the
        // basename (not the patch name), and the edit inserts the MML
        // body immediately, just like a flat item.
        assert_eq!(item.label, "lead.dmp");
        assert_eq!(item.kind, Some(CompletionItemKind::VALUE));
        assert_eq!(item.sort_text.as_deref(), Some("lead.dmp"));
        assert_eq!(item.insert_text_mode, Some(InsertTextMode::AS_IS));
        assert_eq!(item.command, None);
        let edit = range_text_edit(item);
        assert_eq!(edit.new_text, " @1 fm ...");
    }

    #[test]
    fn file_item_multi_patch_inserts_basename_slash_and_retriggers() {
        let patches = vec![
            patch("kit.dmp", "Kick", false, "mk"),
            patch("kit.dmp", "Snare", false, "ms"),
        ];
        let items = build_file_items(&patches, 1, 4, 6);
        assert_eq!(items.len(), 1, "one item per file, not per patch");
        let item = &items[0];
        assert_eq!(item.label, "kit.dmp");
        assert_eq!(item.kind, Some(CompletionItemKind::FILE));
        assert_eq!(
            item.label_details
                .as_ref()
                .and_then(|d| d.description.clone()),
            Some("2".to_string())
        );
        let edit = range_text_edit(item);
        // The hierarchy file-item insert: leading space, basename,
        // trailing slash, so the picker's second step reads `fm kit.dmp/`.
        assert_eq!(edit.new_text, " kit.dmp/");
        let cmd = item
            .command
            .as_ref()
            .expect("expected trigger-suggest command");
        assert_eq!(cmd.command, "editor.action.triggerSuggest");
    }

    #[test]
    fn file_items_one_per_unique_file_mixed_counts() {
        let patches = vec![
            patch("solo.dmp", "Solo", false, "ms"),
            patch("kit.dmp", "Kick", false, "mk"),
            patch("kit.dmp", "Snare", false, "msn"),
        ];
        let items = build_file_items(&patches, 0, 0, 0);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].label, "solo.dmp");
        assert_eq!(items[0].kind, Some(CompletionItemKind::VALUE));
        assert_eq!(items[1].label, "kit.dmp");
        assert_eq!(items[1].kind, Some(CompletionItemKind::FILE));
    }

    // ---------- build_patch_items (hierarchy step 2) ---------------------------

    #[test]
    fn patch_items_filter_by_basename_key() {
        let patches = vec![
            patch("kit.dmp", "Kick", false, "mk"),
            patch("kit.dmp", "Snare", true, "msn"),
            patch("other.dmp", "Lead", false, "ml"),
        ];
        let items = build_patch_items(&patches, "kit.dmp", 0, 4, 6);
        assert_eq!(items.len(), 2);
        assert!(items
            .iter()
            .all(|i| i.label == "Kick" || i.label == "Snare"));
        let snare = items.iter().find(|i| i.label == "Snare").unwrap();
        assert_eq!(snare.detail.as_deref(), Some("[macros]"));
        assert_eq!(snare.filter_text.as_deref(), Some("kit.dmp/Snare"));
        assert_eq!(snare.sort_text.as_deref(), Some("Snare"));
        // Unlike `build_flat_items`, `label_details` here is unconditional
        // on the file key — it does not check name-vs-stem equality.
        assert_eq!(
            snare
                .label_details
                .as_ref()
                .and_then(|d| d.description.clone()),
            Some("kit.dmp".to_string())
        );
    }

    // ---------- build_items dispatcher -----------------------------------------

    #[test]
    fn dispatch_select_file_flat_mode_appends_default_template() {
        let patches = vec![patch("a.dmp", "A", false, "ma")];
        let kind = FmCompletionKind::SelectFile { fm_end_col: 4 };
        let items = build_items(&patches, &kind, 0, 8, /* hierarchy */ false);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].label, "A");
        assert_eq!(items[1].label, "Default FM template");
    }

    #[test]
    fn dispatch_select_file_hierarchy_falls_back_to_flat_when_one_file() {
        // Hierarchy mode is requested, but only one unique file exists —
        // `build_items` falls back to the flat rendering (a two-step
        // picker would be pointless for a single file).
        let patches = vec![
            patch("a.dmp", "Patch 1", false, "m1"),
            patch("a.dmp", "Patch 2", false, "m2"),
        ];
        let kind = FmCompletionKind::SelectFile { fm_end_col: 4 };
        let items = build_items(&patches, &kind, 0, 8, /* hierarchy */ true);
        // 2 flat patch items + 1 default template, not a single grouped
        // file item.
        assert_eq!(items.len(), 3);
        assert!(items.iter().any(|i| i.label == "Patch 1"));
        assert!(items.iter().any(|i| i.label == "Patch 2"));
        assert!(items.iter().any(|i| i.label == "Default FM template"));
    }

    #[test]
    fn dispatch_select_file_hierarchy_groups_when_multiple_files() {
        // Two files, each with more than one patch, so both collapse to
        // a FILE-kind grouped item (not a per-patch VALUE item).
        let patches = vec![
            patch("a.dmp", "Patch 1a", false, "m1a"),
            patch("a.dmp", "Patch 1b", false, "m1b"),
            patch("b.dmp", "Patch 2a", false, "m2a"),
            patch("b.dmp", "Patch 2b", false, "m2b"),
        ];
        let kind = FmCompletionKind::SelectFile { fm_end_col: 4 };
        let items = build_items(&patches, &kind, 0, 8, /* hierarchy */ true);
        assert_eq!(items.len(), 3);
        assert_eq!(items[0].kind, Some(CompletionItemKind::FILE));
        assert_eq!(items[1].kind, Some(CompletionItemKind::FILE));
        assert_eq!(items[2].label, "Default FM template");
    }

    #[test]
    fn dispatch_select_patch_does_not_append_default_template() {
        // Only the SelectFile step ever appends the default-template
        // fallback; the patch-selection step (inside a chosen file) does
        // not repeat it.
        let patches = vec![
            patch("kit.dmp", "Kick", false, "mk"),
            patch("kit.dmp", "Snare", false, "msn"),
        ];
        let kind = FmCompletionKind::SelectPatch {
            file_key: "kit.dmp".to_string(),
            fm_end_col: 4,
        };
        let items = build_items(&patches, &kind, 0, 8, true);
        assert_eq!(items.len(), 2);
        assert!(!items.iter().any(|i| i.label == "Default FM template"));
    }
}
