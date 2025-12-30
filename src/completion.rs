use std::collections::HashSet;
use std::path::PathBuf;

use pathdiff::diff_paths;
use tower_lsp::lsp_types::{
    CompletionItem, CompletionItemKind, Documentation, Position, Range,
};
use walkdir::WalkDir;

use crate::docs;
use crate::utils::{is_wav, uri_to_dir};

const META_KEYWORDS: &[&str] = &[
    "#title",
    "#composer",
    "#author",
    "#date",
    "#comment",
    "#platform",
    "#option",
    "#game",
    "#composerj",
    "#programmer",
];

const PLATFORM_VALUES: &[&str] = &["megadrive", "mdsdrv"];
const INSTRUMENT_TYPES: &[&str] = &["pcm", "fm", "psg", "2op"];

pub(crate) fn meta_completion_items(
    line: &str,
    col: usize,
    line_index: u32,
) -> Vec<CompletionItem> {
    let start_col = meta_prefix_start_col(line, col);
    let range = Range {
        start: Position::new(line_index, start_col),
        end: Position::new(line_index, col as u32),
    };

    META_KEYWORDS
        .iter()
        .map(|kw| {
            let insert = kw.strip_prefix('#').unwrap_or(kw);
            let mut item = meta_item(kw);
            let edit = tower_lsp::lsp_types::TextEdit {
                range,
                new_text: insert.to_string(),
            };
            item.text_edit = Some(tower_lsp::lsp_types::CompletionTextEdit::Edit(edit));
            item.insert_text = Some(insert.to_string());
            item.filter_text = Some(kw.to_string());
            item
        })
        .collect()
}

pub(crate) fn at_meta_completion_items(
    line: &str,
    col: usize,
    line_index: u32,
) -> Vec<CompletionItem> {
    let start_col = at_prefix_start_col(line, col);
    let range = Range {
        start: Position::new(line_index, start_col),
        end: Position::new(line_index, col as u32),
    };
    vec![
        at_meta_item("@<num>", docs::at_meta_doc("@<num>").unwrap_or(""), "${1:num}", range),
        at_meta_item("@E<num>", docs::at_meta_doc("@E<num>").unwrap_or(""), "E${1:num}", range),
        at_meta_item("@M<num>", docs::at_meta_doc("@M<num>").unwrap_or(""), "M${1:num}", range),
        at_meta_item("@P<num>", docs::at_meta_doc("@P<num>").unwrap_or(""), "P${1:num}", range),
    ]
}

pub(crate) fn platform_items() -> Vec<CompletionItem> {
    PLATFORM_VALUES.iter().map(|value| platform_item(value)).collect()
}

pub(crate) fn instrument_items() -> Vec<CompletionItem> {
    INSTRUMENT_TYPES
        .iter()
        .map(|value| instrument_item(value))
        .collect()
}

pub(crate) fn rate_offset_items() -> Vec<CompletionItem> {
    ["rate=", "offset="]
        .iter()
        .map(|kw| rate_offset_item(kw))
        .collect()
}

pub(crate) fn command_items() -> Vec<CompletionItem> {
    docs::COMMAND_COMPLETIONS
        .iter()
        .map(command_item)
        .collect()
}

pub(crate) fn platform_command_items(
    line: &str,
    col: usize,
    line_index: u32,
) -> Vec<CompletionItem> {
    let start_col = platform_command_start_col(line, col);
    let range = Range {
        start: Position::new(line_index, start_col),
        end: Position::new(line_index, col as u32),
    };

    docs::PLATFORM_COMMANDS
        .iter()
        .map(|entry| platform_command_item(entry.label, entry.insert, entry.doc, range))
        .collect()
}

pub(crate) fn complete_pcm_paths(
    line: &str,
    col: usize,
    uri: &str,
    roots: &[PathBuf],
    line_index: u32,
) -> Option<Vec<CompletionItem>> {
    let (prefix, start_col) = string_prefix(line, col)?;
    if !has_pcm_token_before(line, col) {
        return None;
    }

    let base_dir = uri_to_dir(uri)?;
    let mut search_roots = Vec::new();
    search_roots.push(base_dir.clone());
    search_roots.extend(roots.iter().cloned());
    let mut seen = HashSet::new();
    search_roots.retain(|path| seen.insert(path.clone()));
    let mut items = Vec::new();
    let mut seen_items = HashSet::new();

    for root in search_roots {
        for entry in WalkDir::new(&root).into_iter().filter_map(|e| e.ok()) {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            if !is_wav(path) {
                continue;
            }

            if let Some(rel) = diff_paths(path, &base_dir) {
                let rel_str = rel.to_string_lossy().replace('\\', "/");
                if !prefix.is_empty() && !rel_str.starts_with(&prefix) {
                    continue;
                }

                let suffix = match line.get(col..).and_then(|s| s.chars().next()) {
                    Some('"') => "",
                    _ => "\"",
                };
                let insert_text = format!("{rel_str}{suffix}");
                let edit = tower_lsp::lsp_types::TextEdit {
                    range: Range {
                        start: Position::new(line_index, start_col as u32),
                        end: Position::new(line_index, col as u32),
                    },
                    new_text: insert_text.clone(),
                };

                if seen_items.insert(rel_str.clone()) {
                    items.push(CompletionItem {
                        label: rel_str.clone(),
                        kind: Some(CompletionItemKind::FILE),
                        insert_text: Some(insert_text),
                        filter_text: Some(rel_str.clone()),
                        text_edit: Some(tower_lsp::lsp_types::CompletionTextEdit::Edit(edit)),
                        ..CompletionItem::default()
                    });
                }
            }
        }
    }

    Some(items)
}

pub(crate) fn is_in_comment(line: &str, col: usize) -> bool {
    let prefix = match line.get(..col) {
        Some(text) => text,
        None => return false,
    };
    let mut in_string = false;
    for ch in prefix.chars() {
        if ch == '"' {
            in_string = !in_string;
            continue;
        }
        if ch == ';' && !in_string {
            return true;
        }
    }
    false
}

pub(crate) fn is_rate_offset_context(line: &str, col: usize) -> bool {
    let prefix = match line.get(..col) {
        Some(text) => text,
        None => return false,
    };

    if !prefix.ends_with(' ') {
        return false;
    }

    let tokens = tokenize_outside_quotes(prefix);
    if tokens.len() != 3 {
        return false;
    }

    if !is_at_number(&tokens[0]) {
        return false;
    }
    if tokens[1] != "pcm" {
        return false;
    }
    is_quoted(&tokens[2])
}

pub(crate) fn is_instrument_definition_context(line: &str, col: usize) -> bool {
    let prefix = match line.get(..col) {
        Some(text) => text,
        None => return false,
    };
    let trimmed = prefix.trim_start();
    if !trimmed.starts_with('@') {
        return false;
    }
    let rest = &trimmed[1..];
    let digits_len = rest
        .chars()
        .take_while(|ch| ch.is_ascii_digit())
        .count();
    if digits_len == 0 {
        return false;
    }
    let after_digits = &rest[digits_len..];
    after_digits == " "
}

pub(crate) fn is_at_meta_context(line: &str, col: usize) -> bool {
    let prefix = match line.get(..col) {
        Some(text) => text,
        None => return false,
    };
    let trimmed = prefix.trim_start();
    if !trimmed.starts_with('@') {
        return false;
    }
    if trimmed.chars().any(|ch| ch.is_whitespace()) {
        return false;
    }
    !trimmed.chars().skip(1).any(|ch| ch.is_ascii_digit())
}

pub(crate) fn is_platform_command_context(line: &str, col: usize) -> bool {
    let prefix = match line.get(..col) {
        Some(text) => text,
        None => return false,
    };

    let mut in_double = false;
    let mut in_single = false;
    for ch in prefix.chars() {
        if ch == '"' && !in_single {
            in_double = !in_double;
            continue;
        }
        if ch == '\'' && !in_double {
            in_single = !in_single;
        }
    }
    in_single
}


fn meta_prefix_start_col(line: &str, col: usize) -> u32 {
    let prefix = match line.get(..col) {
        Some(text) => text,
        None => return col as u32,
    };
    prefix
        .rfind('#')
        .map(|idx| (idx + 1) as u32)
        .unwrap_or(col as u32)
}

fn at_prefix_start_col(line: &str, col: usize) -> u32 {
    let prefix = match line.get(..col) {
        Some(text) => text,
        None => return col as u32,
    };
    prefix
        .rfind('@')
        .map(|idx| (idx + 1) as u32)
        .unwrap_or(col as u32)
}

fn platform_command_start_col(line: &str, col: usize) -> u32 {
    let prefix = match line.get(..col) {
        Some(text) => text,
        None => return col as u32,
    };
    let last_quote = prefix.rfind('\'').map(|idx| idx + 1).unwrap_or(0);
    let after_quote = &prefix[last_quote..];
    let last_ws = after_quote.rfind(|ch: char| ch.is_whitespace());
    let start = match last_ws {
        Some(idx) => last_quote + idx + 1,
        None => last_quote,
    };
    start as u32
}


fn string_prefix(line: &str, col: usize) -> Option<(String, usize)> {
    let before = line.get(..col)?;
    let quote_count = before.chars().filter(|c| *c == '"').count();
    if quote_count % 2 == 0 {
        return None;
    }

    let last_quote = before.rfind('"')? + 1;
    let prefix = before.get(last_quote..)?.to_string();
    Some((prefix, last_quote))
}

fn has_pcm_token_before(line: &str, col: usize) -> bool {
    line.get(..col)
        .map(|s| s.split_whitespace().any(|tok| tok == "pcm"))
        .unwrap_or(false)
}

fn tokenize_outside_quotes(input: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut in_string = false;

    for ch in input.chars() {
        if ch == '"' {
            current.push(ch);
            in_string = !in_string;
            continue;
        }

        if ch.is_whitespace() && !in_string {
            if !current.is_empty() {
                tokens.push(current.clone());
                current.clear();
            }
            continue;
        }

        current.push(ch);
    }

    if !current.is_empty() {
        tokens.push(current);
    }

    tokens
}

fn is_at_number(token: &str) -> bool {
    let mut chars = token.chars();
    if chars.next() != Some('@') {
        return false;
    }
    let rest: String = chars.collect();
    !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit())
}

fn is_quoted(token: &str) -> bool {
    token.len() >= 2 && token.starts_with('"') && token.ends_with('"')
}

fn documented_item(
    label: &str,
    kind: CompletionItemKind,
    doc: Option<&'static str>,
) -> CompletionItem {
    CompletionItem {
        label: label.to_string(),
        kind: Some(kind),
        documentation: doc.map(|text| Documentation::String(text.to_string())),
        ..CompletionItem::default()
    }
}

fn meta_item(label: &str) -> CompletionItem {
    documented_item(label, CompletionItemKind::KEYWORD, docs::meta_doc(label))
}

fn platform_item(label: &str) -> CompletionItem {
    documented_item(label, CompletionItemKind::KEYWORD, docs::platform_value_doc(label))
}

fn instrument_item(label: &str) -> CompletionItem {
    documented_item(label, CompletionItemKind::TYPE_PARAMETER, docs::instrument_doc(label))
}

fn rate_offset_item(label: &str) -> CompletionItem {
    documented_item(label, CompletionItemKind::PROPERTY, docs::rate_offset_doc(label))
}

fn command_item(entry: &docs::CommandCompletion) -> CompletionItem {
    CompletionItem {
        label: entry.label.to_string(),
        kind: Some(CompletionItemKind::KEYWORD),
        documentation: docs::command_doc(entry.key)
            .map(|text| Documentation::String(text.to_string())),
        insert_text: Some(entry.insert.to_string()),
        insert_text_format: Some(tower_lsp::lsp_types::InsertTextFormat::SNIPPET),
        filter_text: Some(entry.key.to_string()),
        ..CompletionItem::default()
    }
}

fn at_meta_item(label: &str, doc: &'static str, insert_text: &str, range: Range) -> CompletionItem {
    let edit = tower_lsp::lsp_types::TextEdit {
        range,
        new_text: insert_text.to_string(),
    };
    CompletionItem {
        label: label.to_string(),
        kind: Some(CompletionItemKind::KEYWORD),
        documentation: Some(Documentation::String(doc.to_string())),
        insert_text: Some(insert_text.to_string()),
        insert_text_format: Some(tower_lsp::lsp_types::InsertTextFormat::SNIPPET),
        text_edit: Some(tower_lsp::lsp_types::CompletionTextEdit::Edit(edit)),
        filter_text: Some(label.to_string()),
        ..CompletionItem::default()
    }
}

fn platform_command_item(
    label: &str,
    insert_text: &str,
    doc: &'static str,
    range: Range,
) -> CompletionItem {
    let edit = tower_lsp::lsp_types::TextEdit {
        range,
        new_text: insert_text.to_string(),
    };
    CompletionItem {
        label: label.to_string(),
        kind: Some(CompletionItemKind::KEYWORD),
        documentation: Some(Documentation::String(doc.to_string())),
        insert_text: Some(insert_text.to_string()),
        text_edit: Some(tower_lsp::lsp_types::CompletionTextEdit::Edit(edit)),
        filter_text: Some(label.to_string()),
        ..CompletionItem::default()
    }
}

