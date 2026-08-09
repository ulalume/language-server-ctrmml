use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::SystemTime;

use tower_lsp::lsp_types::{
    Command, CompletionItem, CompletionItemKind, CompletionItemLabelDetails, CompletionTextEdit,
    Documentation, InsertTextFormat, InsertTextMode, Position, Range, TextEdit,
};

use ctrmml_lang_core::docs::FM_DEFAULT_TEMPLATE;
use walkdir::WalkDir;

use crate::backend::Backend;
use crate::completion::FmCompletionKind;
use crate::utils::{is_fm_instrument, uri_to_dir};
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
    let mut search_roots = Vec::new();
    if let Some(base_dir) = uri_to_dir(uri) {
        search_roots.push(base_dir);
    }
    search_roots.extend(roots.iter().cloned());
    let mut seen = HashSet::new();
    search_roots.retain(|path| seen.insert(path.clone()));

    let mut files = Vec::new();
    let mut seen_files = HashSet::new();
    for root in search_roots {
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

        // Check cache
        {
            let cache = self.fm_instrument_cache.lock().await;
            if let Some(cached) = cache.as_ref() {
                if cached.is_valid(roots, &cmd_path) {
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
                roots: roots.to_vec(),
                cmd_path,
            });
        }

        Ok(items)
    }
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
        .map(|p| {
            CompletionItem {
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
            }
        })
        .collect()
}
