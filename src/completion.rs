use std::collections::HashSet;
use std::path::PathBuf;

use ctrmml_lang_core::is_at_number;
use pathdiff::diff_paths;
use tower_lsp::lsp_types::{
    Command, CompletionItem, CompletionItemKind, Documentation, InsertTextFormat, Position, Range,
};
use walkdir::WalkDir;

use crate::docs;
use crate::utils::{is_wav, uri_to_dir};

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

    docs::META_KEYWORDS
        .iter()
        .map(|kw| {
            let label = kw.label;
            let insert = label.strip_prefix('#').unwrap_or(label);
            let mut item = meta_item(label);
            let edit = tower_lsp::lsp_types::TextEdit {
                range,
                new_text: insert.to_string(),
            };
            item.text_edit = Some(tower_lsp::lsp_types::CompletionTextEdit::Edit(edit));
            item.insert_text = Some(insert.to_string());
            item.filter_text = Some(label.to_string());
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
        at_meta_item(
            "@<num>",
            docs::at_meta_doc("@<num>").unwrap_or(""),
            "${1:num}",
            range,
        ),
        at_meta_item(
            "@E<num>",
            docs::at_meta_doc("@E<num>").unwrap_or(""),
            "E${1:num}",
            range,
        ),
        at_meta_item(
            "@M<num>",
            docs::at_meta_doc("@M<num>").unwrap_or(""),
            "M${1:num}",
            range,
        ),
        at_meta_item(
            "@P<num>",
            docs::at_meta_doc("@P<num>").unwrap_or(""),
            "P${1:num}",
            range,
        ),
    ]
}

pub(crate) fn platform_items() -> Vec<CompletionItem> {
    docs::PLATFORM_VALUES
        .iter()
        .map(|value| platform_item(value.label))
        .collect()
}

pub(crate) fn option_items() -> Vec<CompletionItem> {
    docs::OPTION_VALUES
        .iter()
        .map(|value| option_item(value.label))
        .collect()
}

pub(crate) fn instrument_items() -> Vec<CompletionItem> {
    docs::INSTRUMENT_TYPES.iter().map(instrument_item).collect()
}

pub(crate) fn rate_offset_items() -> Vec<CompletionItem> {
    ["rate=", "offset="]
        .iter()
        .map(|kw| rate_offset_item(kw))
        .collect()
}

pub(crate) fn command_items() -> Vec<CompletionItem> {
    docs::COMMAND_COMPLETIONS.iter().map(command_item).collect()
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

pub(crate) enum FmCompletionKind {
    SelectFile { fm_end_col: u32 },
    SelectPatch { file_key: String, fm_end_col: u32 },
}

pub(crate) fn fm_instrument_context(line: &str, col: usize) -> Option<FmCompletionKind> {
    let prefix = line.get(..col)?;
    let trimmed = prefix.trim_start();
    if !trimmed.starts_with('@') {
        return None;
    }
    let leading_ws = prefix.len() - trimmed.len();
    let rest = &trimmed[1..];
    let digits_len = rest.chars().take_while(|ch| ch.is_ascii_digit()).count();
    if digits_len == 0 {
        return None;
    }
    let after_digits = &rest[digits_len..];
    let after_trimmed = after_digits.trim_start();
    if !after_trimmed.starts_with("fm") {
        return None;
    }
    let after_fm = &after_trimmed[2..];
    // Word-boundary check: `@1 fmx` shouldn't trigger FM file completion.
    // Accept the cursor sitting immediately after `fm`, or after a
    // whitespace separator; reject anything else.
    if let Some(next) = after_fm.chars().next() {
        if !next.is_whitespace() {
            return None;
        }
    }

    let gap = after_digits.len() - after_trimmed.len();
    // `fm_end_col` always points at the position right after `fm`
    // (before any trailing space). Item builders prepend `" "` to the
    // inserted patch body so the final layout reads `fm ; name`
    // regardless of whether the user already typed the space.
    let fm_end_col = (leading_ws + 1 + digits_len + gap + 2) as u32;
    let after_fm_space = after_fm.trim_start();

    if let Some(slash_pos) = after_fm_space.find('/') {
        let file_key = after_fm_space[..slash_pos].to_string();
        Some(FmCompletionKind::SelectPatch {
            file_key,
            fm_end_col,
        })
    } else {
        Some(FmCompletionKind::SelectFile { fm_end_col })
    }
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
    let digits_len = rest.chars().take_while(|ch| ch.is_ascii_digit()).count();
    if digits_len == 0 {
        return false;
    }
    let after_digits = &rest[digits_len..];
    if !after_digits.starts_with(' ') {
        return false;
    }
    // Accept `@<N> `, `@<N> p`, `@<N> ps`, `@<N> psg`, `@<N> pcm`, …
    // anywhere a known keyword shares the typed prefix. This keeps the
    // dropdown alive while the user types the keyword, so vscode's
    // client-side filter can do its work and an `is_incomplete: true`
    // response can swap to FM-file picker when the user reaches `fm`.
    let after_space = after_digits.trim_start();
    let keywords = ["fm", "psg", "pcm", "2op"];
    keywords.iter().any(|kw| kw.starts_with(after_space))
}

pub(crate) fn is_meta_keyword_context(line: &str, col: usize) -> bool {
    let prefix = match line.get(..col) {
        Some(text) => text,
        None => return false,
    };
    let hash_pos = match prefix.rfind('#') {
        Some(pos) => pos,
        None => return false,
    };
    if prefix[..hash_pos].chars().any(|ch| !ch.is_whitespace()) {
        return false;
    }
    let after = &prefix[hash_pos + 1..];
    !after.chars().any(|ch| ch.is_whitespace())
}

pub(crate) fn is_meta_value_context(line: &str, col: usize, keyword: &str) -> bool {
    let trimmed = line.trim_start();
    if !trimmed.starts_with(keyword) {
        return false;
    }
    let leading_ws = line.len() - trimmed.len();
    let kw_start = leading_ws;
    let kw_end = kw_start + keyword.len();
    if col <= kw_end {
        return false;
    }
    let prefix = match line.get(..col) {
        Some(text) => text,
        None => return false,
    };
    let between = match prefix.get(kw_end..) {
        Some(text) => text,
        None => return false,
    };
    let mut saw_ws = false;
    for ch in between.chars() {
        if !saw_ws {
            if ch.is_whitespace() {
                saw_ws = true;
                continue;
            }
            return false;
        }
    }
    saw_ws
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
    documented_item(
        label,
        CompletionItemKind::KEYWORD,
        docs::platform_value_doc(label),
    )
}

fn option_item(label: &str) -> CompletionItem {
    documented_item(
        label,
        CompletionItemKind::KEYWORD,
        docs::option_value_doc(label),
    )
}

fn instrument_item(entry: &docs::DocEntry) -> CompletionItem {
    let mut item = documented_item(
        entry.label,
        CompletionItemKind::TYPE_PARAMETER,
        docs::instrument_doc(entry.label),
    );
    if !entry.insert.is_empty() {
        item.insert_text = Some(entry.insert.to_string());
        if entry.insert.contains('$') {
            item.insert_text_format = Some(InsertTextFormat::SNIPPET);
        }
    }
    // `fm` / `pcm` re-trigger completion so the user immediately gets
    // the file-picker list without having to type a space first.
    if docs::INSTRUMENT_TYPES_TRIGGER_SUGGEST.contains(&entry.label) {
        item.command = Some(Command {
            title: "Suggest instrument files".to_string(),
            command: "editor.action.triggerSuggest".to_string(),
            arguments: None,
        });
    }
    item
}

fn rate_offset_item(label: &str) -> CompletionItem {
    let display_label = docs::rate_offset_label(label).unwrap_or(label);
    let insert_text = match label {
        "rate=" => "rate=${1:<num>}",
        "offset=" => "offset=${1:<num>}",
        _ => label,
    };
    CompletionItem {
        label: display_label.to_string(),
        kind: Some(CompletionItemKind::PROPERTY),
        documentation: docs::rate_offset_doc(label)
            .map(|text| Documentation::String(text.to_string())),
        insert_text: Some(insert_text.to_string()),
        insert_text_format: Some(tower_lsp::lsp_types::InsertTextFormat::SNIPPET),
        filter_text: Some(label.to_string()),
        ..CompletionItem::default()
    }
}

fn command_item(entry: &docs::CommandCompletion) -> CompletionItem {
    CompletionItem {
        label: entry.label.to_string(),
        kind: Some(CompletionItemKind::KEYWORD),
        documentation: docs::command_doc(entry.key)
            .or_else(|| docs::platform_command_doc(entry.key))
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

// ---------------------------------------------------------------------------
// Characterization tests — pin TODAY's behavior of the native completion
// cascade ahead of the COMPLETION_CORE_PLAN.md rewrite. Where today's
// behavior is intentionally going to change per a lettered decision in
// COMPLETION_CORE_PLAN.md §3, the test is still asserting what happens
// TODAY, annotated with a `// CURRENT behavior; changes at C5 per
// COMPLETION_CORE_PLAN D<n>` comment.
#[cfg(test)]
mod tests {
    use super::*;
    use tower_lsp::lsp_types::{CompletionTextEdit, TextEdit};

    fn edit_of(item: &CompletionItem) -> &TextEdit {
        match item.text_edit.as_ref().expect("expected a text_edit") {
            CompletionTextEdit::Edit(edit) => edit,
            other => panic!("expected CompletionTextEdit::Edit, got {other:?}"),
        }
    }

    // =========================================================================
    // 1. is_platform_command_context / platform_command_start_col
    // =========================================================================

    #[test]
    fn platform_command_context_true_inside_open_single_quote() {
        let line = "A 'fm3 0011";
        assert!(is_platform_command_context(line, line.len()));
    }

    #[test]
    fn platform_command_context_false_outside_any_quote() {
        let line = "A fm3 0011";
        assert!(!is_platform_command_context(line, line.len()));
    }

    #[test]
    fn platform_command_context_false_after_closed_single_quote() {
        let line = "'fm3 0011' ";
        // Right after the closing quote, and after the trailing space.
        assert!(!is_platform_command_context(line, 10));
        assert!(!is_platform_command_context(line, line.len()));
    }

    #[test]
    fn platform_command_context_nested_double_quote_inside_open_single_is_ignored() {
        // Odd/nested case: a `"` encountered while already inside an
        // unterminated `'...'` run does not toggle double-quote state,
        // so it can't accidentally close the single-quoted context.
        let line = "'foo \"bar";
        assert!(is_platform_command_context(line, line.len()));
    }

    #[test]
    fn platform_command_context_open_double_quote_suppresses_single_toggle() {
        // Odd/nested case: an unterminated `"..` run in progress means a
        // later `'` does not toggle single-quote state either — double
        // quote "wins" and this reports false even though a naive
        // unpaired-quote count on `'` alone would suggest otherwise.
        let line = "\"abc'";
        assert!(!is_platform_command_context(line, line.len()));
    }

    #[test]
    fn platform_command_context_reopening_after_close_is_true() {
        let line = "'foo' 'bar";
        assert!(is_platform_command_context(line, line.len()));
    }

    #[test]
    fn platform_command_context_col_past_end_returns_false() {
        let line = "'x";
        assert!(line.get(..100).is_none());
        assert!(!is_platform_command_context(line, 100));
    }

    #[test]
    fn platform_command_start_col_after_quote_and_whitespace() {
        // Cursor mid-way through the last space-separated token after the
        // opening quote: start col is right after the LAST whitespace
        // following the LAST quote, not right after the quote itself.
        let line = "A 'fm3 0011";
        assert_eq!(platform_command_start_col(line, line.len()), 7);
        assert_eq!(&line[7..], "0011");
    }

    #[test]
    fn platform_command_start_col_no_quote_uses_last_whitespace() {
        let line = "abc def";
        assert_eq!(platform_command_start_col(line, line.len()), 4);
        assert_eq!(&line[4..], "def");
    }

    #[test]
    fn platform_command_start_col_no_quote_no_whitespace_is_zero() {
        let line = "abcdef";
        assert_eq!(platform_command_start_col(line, line.len()), 0);
    }

    #[test]
    fn platform_command_start_col_uses_byte_offsets_on_multibyte_line() {
        // "あ" is 3 UTF-8 bytes but 1 character. `platform_command_start_col`
        // walks byte offsets (`str::rfind` on a `&str`), so the returned
        // column is a byte index. Pinning this because LSP columns are
        // UTF-16 code units — a caller forwarding an LSP column straight
        // into this function on a non-ASCII line is not using the same
        // unit the function returns.
        let line = "'あ fm3 0011";
        let col = line.len();
        let got = platform_command_start_col(line, col);
        let expected_byte_idx = line.rfind(' ').unwrap() + 1;
        assert_eq!(got as usize, expected_byte_idx);
        let expected_char_idx = line[..expected_byte_idx].chars().count();
        assert_ne!(
            expected_byte_idx, expected_char_idx,
            "fixture assumption: multibyte prefix makes byte idx != char idx"
        );
    }

    #[test]
    fn platform_command_start_col_non_char_boundary_returns_col_verbatim() {
        // A `col` landing inside a multibyte codepoint makes
        // `line.get(..col)` return `None`; the fallback returns `col`
        // unchanged instead of panicking or snapping to a boundary.
        let line = "'あ";
        assert!(!line.is_char_boundary(2));
        assert_eq!(platform_command_start_col(line, 2), 2);
    }

    // =========================================================================
    // 2. PCM-path context: string_prefix / has_pcm_token_before /
    //    complete_pcm_paths early-return gates.
    // =========================================================================

    #[test]
    fn string_prefix_odd_quote_count_returns_open_run() {
        let line = "@30 pcm \"foo";
        assert_eq!(
            string_prefix(line, line.len()),
            Some(("foo".to_string(), 9))
        );
    }

    #[test]
    fn string_prefix_even_quote_count_is_none() {
        let line = "\"a\" b";
        assert_eq!(string_prefix(line, line.len()), None);
    }

    #[test]
    fn string_prefix_zero_quotes_is_none() {
        let line = "abc";
        assert_eq!(string_prefix(line, line.len()), None);
    }

    #[test]
    fn has_pcm_token_before_finds_whole_word() {
        let line = "@30 pcm ";
        assert!(has_pcm_token_before(line, line.len()));
    }

    #[test]
    fn has_pcm_token_before_false_without_pcm() {
        let line = "@30 fm ";
        assert!(!has_pcm_token_before(line, line.len()));
    }

    #[test]
    fn has_pcm_token_before_requires_whole_token_not_substring() {
        let line = "@30 pcmx ";
        assert!(!has_pcm_token_before(line, line.len()));
    }

    #[test]
    fn complete_pcm_paths_none_when_not_in_open_string() {
        let line = "@30 pcm x";
        assert!(complete_pcm_paths(line, line.len(), "file:///proj/a.mml", &[], 0).is_none());
    }

    #[test]
    fn complete_pcm_paths_none_without_pcm_token_even_in_open_string() {
        let line = "@30 xyz \"foo";
        assert!(complete_pcm_paths(line, line.len(), "file:///proj/a.mml", &[], 0).is_none());
    }

    #[test]
    fn complete_pcm_paths_none_when_uri_unresolvable() {
        // Odd-quote and `pcm`-token gates both pass, but an unparsable
        // URI makes `uri_to_dir` fail — the function bails before ever
        // touching the filesystem (no WalkDir call happens here).
        let line = "@30 pcm \"foo";
        assert!(complete_pcm_paths(line, line.len(), "not a uri", &[], 0).is_none());
    }

    // =========================================================================
    // 3. Meta-value context (#platform / #option values)
    // =========================================================================

    #[test]
    fn meta_value_context_true_right_after_keyword_and_space() {
        let line = "#platform ";
        assert!(is_meta_value_context(line, line.len(), "#platform"));
    }

    #[test]
    fn meta_value_context_false_at_keyword_end_no_space_yet() {
        let line = "#platform";
        assert!(!is_meta_value_context(line, line.len(), "#platform"));
    }

    #[test]
    fn meta_value_context_false_when_keyword_extended_without_space() {
        // "#platformx" is not "#platform" followed by a value gap — the
        // char right after the recognized keyword text is non-whitespace.
        let line = "#platformx";
        assert!(!is_meta_value_context(line, line.len(), "#platform"));
    }

    #[test]
    fn meta_value_context_false_when_keyword_does_not_match() {
        let line = "#option foo";
        assert!(!is_meta_value_context(line, line.len(), "#platform"));
    }

    #[test]
    fn meta_value_context_true_option_keyword() {
        let line = "#option noext";
        assert!(is_meta_value_context(line, line.len(), "#option"));
    }

    #[test]
    fn meta_value_context_respects_leading_line_whitespace() {
        let line = "  #platform g";
        assert!(is_meta_value_context(line, line.len(), "#platform"));
    }

    #[test]
    fn meta_value_context_stays_true_after_typing_into_the_value() {
        // CURRENT behavior; changes at C5 per COMPLETION_CORE_PLAN D7.
        //
        // Once the character immediately after the keyword is
        // whitespace, this predicate never re-inspects the rest of the
        // prefix: it returns `true` regardless of how much of a value
        // the user has already typed past that first space. So it does
        // NOT narrow/stop as typing continues into "#platform mega" —
        // only a totally different first character (no gap at all)
        // makes it false. D7 replaces this with TS's prefix-filtering
        // behavior instead.
        let line = "#platform mega";
        assert!(is_meta_value_context(line, line.len(), "#platform"));
    }

    // =========================================================================
    // 4. Meta-keyword context (#<keyword> itself) + meta_prefix_start_col +
    //    meta_completion_items item shape.
    // =========================================================================

    #[test]
    fn meta_keyword_context_true_hash_is_first_nonws_no_space_yet() {
        let line = "  #plat";
        assert!(is_meta_keyword_context(line, line.len()));
    }

    #[test]
    fn meta_keyword_context_false_once_space_typed_after_hash() {
        let line = "#platform ";
        assert!(!is_meta_keyword_context(line, line.len()));
    }

    #[test]
    fn meta_keyword_context_false_when_hash_not_first_nonws() {
        let line = "a #foo";
        assert!(!is_meta_keyword_context(line, line.len()));
    }

    #[test]
    fn meta_keyword_context_false_without_any_hash() {
        let line = "platform";
        assert!(!is_meta_keyword_context(line, line.len()));
    }

    #[test]
    fn meta_keyword_context_uses_last_hash_and_first_wins_the_strictness_check() {
        // rfind locates the *last* `#`, but the strictness check then
        // looks at everything before THAT `#` — a leading `#` earlier in
        // the line is non-whitespace and fails the check.
        let line = "# #foo";
        assert!(!is_meta_keyword_context(line, line.len()));
    }

    #[test]
    fn meta_prefix_start_col_after_hash() {
        let line = "  #plat";
        assert_eq!(meta_prefix_start_col(line, line.len()), 3);
    }

    #[test]
    fn meta_prefix_start_col_no_hash_falls_back_to_col() {
        let line = "abc";
        assert_eq!(meta_prefix_start_col(line, 3), 3);
    }

    #[test]
    fn meta_completion_items_count_matches_table() {
        let items = meta_completion_items("#plat", 5, 0);
        assert_eq!(items.len(), docs::META_KEYWORDS.len());
        assert!(items
            .iter()
            .all(|it| it.kind == Some(CompletionItemKind::KEYWORD)));
    }

    #[test]
    fn meta_completion_items_insert_strips_hash_and_range_starts_after_hash() {
        let items = meta_completion_items("#plat", 5, 0);
        let item = items
            .iter()
            .find(|it| it.label == "#platform")
            .expect("missing #platform item");
        // Insert text is the label minus its leading `#` — NOT the docs
        // table's own `insert` field ("#platform ", which carries a
        // trailing space and keeps the `#`); the two are unrelated here.
        assert_eq!(item.insert_text.as_deref(), Some("platform"));
        assert_eq!(item.filter_text.as_deref(), Some("#platform"));
        let edit = edit_of(item);
        assert_eq!(edit.new_text, "platform");
        assert_eq!(edit.range.start, Position::new(0, 1));
        assert_eq!(edit.range.end, Position::new(0, 5));
    }

    #[test]
    fn meta_completion_items_doc_falls_back_to_detail_when_doc_field_empty() {
        // `#author`'s docs-table entry has an empty `doc` field; `meta_doc`
        // falls back to `detail` ("Song metadata.") rather than yielding
        // no documentation at all.
        let items = meta_completion_items("#a", 2, 0);
        let item = items
            .iter()
            .find(|it| it.label == "#author")
            .expect("missing #author item");
        assert_eq!(
            item.documentation,
            Some(Documentation::String("Song metadata.".to_string()))
        );
    }

    // =========================================================================
    // 5. rate/offset context: tokenize_outside_quotes / is_quoted /
    //    is_rate_offset_context / rate_offset_items shape.
    // =========================================================================

    #[test]
    fn tokenize_outside_quotes_keeps_quoted_space_as_one_token() {
        let tokens = tokenize_outside_quotes("@1 pcm \"a b\"");
        assert_eq!(tokens, vec!["@1", "pcm", "\"a b\""]);
    }

    #[test]
    fn tokenize_outside_quotes_collapses_runs_of_whitespace() {
        let tokens = tokenize_outside_quotes("a   b  c");
        assert_eq!(tokens, vec!["a", "b", "c"]);
    }

    #[test]
    fn tokenize_outside_quotes_keeps_unterminated_trailing_token() {
        let tokens = tokenize_outside_quotes("@1 pcm \"a");
        assert_eq!(tokens, vec!["@1", "pcm", "\"a"]);
    }

    #[test]
    fn is_quoted_true_for_paired_quotes() {
        assert!(is_quoted("\"x\""));
    }

    #[test]
    fn is_quoted_false_for_lone_quote_char() {
        // Starts-with and ends-with both technically see the same single
        // `"`, but the length-2 minimum rejects it.
        assert!(!is_quoted("\""));
    }

    #[test]
    fn is_quoted_false_without_quotes() {
        assert!(!is_quoted("x"));
    }

    #[test]
    fn is_quoted_false_when_unterminated() {
        assert!(!is_quoted("\"x"));
    }

    #[test]
    fn rate_offset_context_true_for_at_pcm_closed_quote_and_trailing_space() {
        let line = "@1 pcm \"a.wav\" ";
        assert!(is_rate_offset_context(line, line.len()));
    }

    #[test]
    fn rate_offset_context_false_without_trailing_space() {
        let line = "@1 pcm \"a.wav\"";
        assert!(!is_rate_offset_context(line, line.len()));
    }

    #[test]
    fn rate_offset_context_false_for_four_tokens() {
        // CURRENT behavior; changes at C5 per COMPLETION_CORE_PLAN D8.
        //
        // A 4th already-typed token (e.g. a prior `rate=8000`) makes the
        // exactly-3-tokens rule reject the context entirely, even though
        // the tail is a legitimate place to keep adding `rate=`/`offset=`
        // params. D8 relaxes this to `tokens.len() >= 3`.
        let line = "@1 pcm \"a.wav\" rate=8000 ";
        assert!(!is_rate_offset_context(line, line.len()));
    }

    #[test]
    fn rate_offset_context_false_for_two_tokens() {
        let line = "@1 pcm ";
        assert!(!is_rate_offset_context(line, line.len()));
    }

    #[test]
    fn rate_offset_context_false_when_second_token_not_pcm() {
        let line = "@1 xyz \"a.wav\" ";
        assert!(!is_rate_offset_context(line, line.len()));
    }

    #[test]
    fn rate_offset_context_false_when_first_token_not_at_number() {
        let line = "1 pcm \"a.wav\" ";
        assert!(!is_rate_offset_context(line, line.len()));
    }

    #[test]
    fn rate_offset_context_false_when_third_token_quote_unterminated() {
        // Closed-quote requirement: the trailing space lands *inside* the
        // still-open quote (so it doesn't break tokenization into a 4th
        // token), but the resulting 3rd token doesn't end with `"`, so
        // `is_quoted` rejects it.
        let line = "@1 pcm \"a.wav ";
        let tokens = tokenize_outside_quotes(line);
        assert_eq!(tokens.len(), 3, "fixture assumption: still tokenizes to 3");
        assert!(!is_quoted(&tokens[2]));
        assert!(!is_rate_offset_context(line, line.len()));
    }

    #[test]
    fn rate_offset_items_shape() {
        let items = rate_offset_items();
        assert_eq!(items.len(), 2);
        let rate = items
            .iter()
            .find(|it| it.filter_text.as_deref() == Some("rate="))
            .unwrap();
        assert_eq!(rate.label, "rate=<num>");
        assert_eq!(rate.kind, Some(CompletionItemKind::PROPERTY));
        assert_eq!(rate.insert_text.as_deref(), Some("rate=${1:<num>}"));
        assert_eq!(rate.insert_text_format, Some(InsertTextFormat::SNIPPET));
        assert_eq!(
            rate.documentation,
            docs::rate_offset_doc("rate=").map(|d| Documentation::String(d.to_string()))
        );

        let offset = items
            .iter()
            .find(|it| it.filter_text.as_deref() == Some("offset="))
            .unwrap();
        assert_eq!(offset.label, "offset=<num>");
        assert_eq!(offset.insert_text.as_deref(), Some("offset=${1:<num>}"));
        assert_eq!(offset.insert_text_format, Some(InsertTextFormat::SNIPPET));
    }

    // =========================================================================
    // 6. Instrument-definition context (prefix-of-keyword liveness) +
    //    fm_instrument_context (SelectFile/SelectPatch parse) +
    //    instrument_items shape.
    // =========================================================================

    #[test]
    fn instrument_def_context_true_immediately_after_space_no_prefix_yet() {
        let line = "@1 ";
        assert!(is_instrument_definition_context(line, line.len()));
    }

    #[test]
    fn instrument_def_context_true_for_live_prefix_p() {
        let line = "@1 p";
        assert!(is_instrument_definition_context(line, line.len()));
    }

    #[test]
    fn instrument_def_context_true_for_live_prefix_2() {
        let line = "@1 2";
        assert!(is_instrument_definition_context(line, line.len()));
    }

    #[test]
    fn instrument_def_context_false_for_non_matching_prefix() {
        let line = "@1 xyz";
        assert!(!is_instrument_definition_context(line, line.len()));
    }

    #[test]
    fn instrument_def_context_false_without_space_after_digits() {
        // Unlike `fm_instrument_context`, this predicate requires a
        // literal space between the digits and the keyword prefix —
        // "@1fm" (no space) is rejected here even though the FM-specific
        // parser accepts it.
        let line = "@1fm";
        assert!(!is_instrument_definition_context(line, line.len()));
    }

    #[test]
    fn instrument_def_context_false_without_digits() {
        let line = "@ fm";
        assert!(!is_instrument_definition_context(line, line.len()));
    }

    #[test]
    fn instrument_def_context_false_without_at() {
        let line = "1 fm";
        assert!(!is_instrument_definition_context(line, line.len()));
    }

    #[test]
    fn instrument_def_context_respects_leading_whitespace() {
        let line = "  @12 f";
        assert!(is_instrument_definition_context(line, line.len()));
    }

    fn expect_select_file(kind: Option<FmCompletionKind>) -> u32 {
        match kind {
            Some(FmCompletionKind::SelectFile { fm_end_col }) => fm_end_col,
            other => panic!(
                "expected SelectFile, got a different result: {}",
                other.is_some()
            ),
        }
    }

    fn expect_select_patch(kind: Option<FmCompletionKind>) -> (String, u32) {
        match kind {
            Some(FmCompletionKind::SelectPatch {
                file_key,
                fm_end_col,
            }) => (file_key, fm_end_col),
            other => panic!(
                "expected SelectPatch, got a different result: {}",
                other.is_some()
            ),
        }
    }

    #[test]
    fn fm_instrument_context_select_file_with_space() {
        let line = "@1 fm";
        let fm_end_col = expect_select_file(fm_instrument_context(line, line.len()));
        assert_eq!(fm_end_col, 5);
    }

    #[test]
    fn fm_instrument_context_select_file_no_space_between_digits_and_fm() {
        // "@1fm" acceptance: unlike `is_instrument_definition_context`,
        // the gap between the digits and `fm` is optional here (plain
        // `trim_start`, not a `starts_with(' ')` requirement).
        let line = "@1fm";
        let fm_end_col = expect_select_file(fm_instrument_context(line, line.len()));
        assert_eq!(fm_end_col, 4);
    }

    #[test]
    fn fm_instrument_context_rejects_fmx_word_boundary() {
        // "@1 fmx" rejection: `fmx` starts with `fm`, but the character
        // right after `fm` is neither whitespace nor end-of-input, so
        // the word-boundary check bails out.
        let line = "@1 fmx";
        assert!(fm_instrument_context(line, line.len()).is_none());
    }

    #[test]
    fn fm_instrument_context_select_file_trailing_space_after_fm() {
        let line = "@1 fm ";
        let fm_end_col = expect_select_file(fm_instrument_context(line, line.len()));
        assert_eq!(fm_end_col, 5);
    }

    #[test]
    fn fm_instrument_context_select_patch_extracts_first_path_segment() {
        // Fragment extraction from `@1 fm dir/...`: `file_key` is
        // everything before the FIRST `/` only (today's hierarchy keys
        // by a single basename-like segment, not the full relative path
        // — see D12).
        let line = "@1 fm dir/pat";
        let (file_key, fm_end_col) = expect_select_patch(fm_instrument_context(line, line.len()));
        assert_eq!(file_key, "dir");
        assert_eq!(fm_end_col, 5);
    }

    #[test]
    fn fm_instrument_context_select_patch_nested_path_keys_by_first_segment_only() {
        let line = "@1 fm a/b/c";
        let (file_key, _) = expect_select_patch(fm_instrument_context(line, line.len()));
        assert_eq!(file_key, "a");
    }

    #[test]
    fn fm_instrument_context_none_without_at() {
        let line = "1 fm";
        assert!(fm_instrument_context(line, line.len()).is_none());
    }

    #[test]
    fn fm_instrument_context_none_without_digits() {
        let line = "@ fm";
        assert!(fm_instrument_context(line, line.len()).is_none());
    }

    #[test]
    fn fm_instrument_context_none_for_psg_keyword() {
        let line = "@1 psg";
        assert!(fm_instrument_context(line, line.len()).is_none());
    }

    #[test]
    fn fm_instrument_context_respects_leading_whitespace_in_fm_end_col() {
        let line = "  @1 fm";
        let fm_end_col = expect_select_file(fm_instrument_context(line, line.len()));
        assert_eq!(fm_end_col, 7);
    }

    #[test]
    fn instrument_items_count_matches_table() {
        let items = instrument_items();
        assert_eq!(items.len(), docs::INSTRUMENT_TYPES.len());
        assert!(items
            .iter()
            .all(|it| it.kind == Some(CompletionItemKind::TYPE_PARAMETER)));
    }

    #[test]
    fn instrument_items_fm_and_pcm_trigger_suggest_others_do_not() {
        let items = instrument_items();
        let fm = items.iter().find(|it| it.label == "fm").unwrap();
        assert_eq!(fm.insert_text.as_deref(), Some("fm "));
        assert_eq!(
            fm.insert_text_format, None,
            "no `$` in \"fm \" => not marked snippet"
        );
        assert!(fm.command.is_some());

        let pcm = items.iter().find(|it| it.label == "pcm").unwrap();
        assert!(pcm.command.is_some());

        let psg = items.iter().find(|it| it.label == "psg").unwrap();
        assert_eq!(psg.command, None);

        let two_op = items.iter().find(|it| it.label == "2op").unwrap();
        assert_eq!(
            two_op.command, None,
            "2op has no per-file pool to pick from"
        );
        assert_eq!(
            two_op.insert_text_format,
            Some(InsertTextFormat::SNIPPET),
            "2op's insert contains `$` placeholders"
        );
    }

    // =========================================================================
    // 7. At-meta context (@, @E, @M, @P) + at_meta_completion_items +
    //    at_prefix_start_col.
    // =========================================================================

    #[test]
    fn at_meta_context_true_for_bare_at() {
        assert!(is_at_meta_context("@", 1));
    }

    #[test]
    fn at_meta_context_true_for_at_letter_no_digit_yet() {
        assert!(is_at_meta_context("@E", 2));
        assert!(is_at_meta_context("@P", 2));
    }

    #[test]
    fn at_meta_context_false_once_a_digit_appears() {
        assert!(!is_at_meta_context("@1", 2));
        assert!(!is_at_meta_context("@E1", 3));
    }

    #[test]
    fn at_meta_context_false_with_whitespace() {
        assert!(!is_at_meta_context("@ ", 2));
    }

    #[test]
    fn at_meta_context_false_without_at() {
        assert!(!is_at_meta_context("foo", 3));
    }

    #[test]
    fn at_meta_context_respects_leading_whitespace() {
        assert!(is_at_meta_context("  @P", 4));
    }

    #[test]
    fn at_prefix_start_col_after_at() {
        assert_eq!(at_prefix_start_col("  @P", 4), 3);
    }

    #[test]
    fn at_prefix_start_col_no_at_falls_back_to_col() {
        assert_eq!(at_prefix_start_col("abc", 3), 3);
    }

    #[test]
    fn at_meta_completion_items_always_four_hardcoded_items() {
        // CURRENT behavior; changes at C5 per COMPLETION_CORE_PLAN D10.
        //
        // The list is a fixed 4-item literal (not filtered by what's
        // already typed, and not sourced from `docs::AT_META`, which
        // only has 2 entries — `@<num>` and `@M<num>` — nor from
        // `docs::AT_META_COMPLETION_LABELS`). D10 replaces this with the
        // table-driven 2-item version, dropping `@E<num>`/`@P<num>`.
        let items = at_meta_completion_items("@E", 2, 0);
        assert_eq!(
            items.iter().map(|it| it.label.as_str()).collect::<Vec<_>>(),
            vec!["@<num>", "@E<num>", "@M<num>", "@P<num>"]
        );
        assert!(items
            .iter()
            .all(|it| it.kind == Some(CompletionItemKind::KEYWORD)));
        assert!(items
            .iter()
            .all(|it| it.insert_text_format == Some(InsertTextFormat::SNIPPET)));
    }

    #[test]
    fn at_meta_completion_items_insert_text_and_range() {
        let items = at_meta_completion_items("@E", 2, 0);
        let at_num = items.iter().find(|it| it.label == "@<num>").unwrap();
        assert_eq!(at_num.insert_text.as_deref(), Some("${1:num}"));
        let edit = edit_of(at_num);
        assert_eq!(edit.range.start, Position::new(0, 1));
        assert_eq!(edit.range.end, Position::new(0, 2));

        let e_num = items.iter().find(|it| it.label == "@E<num>").unwrap();
        assert_eq!(e_num.insert_text.as_deref(), Some("E${1:num}"));

        let m_num = items.iter().find(|it| it.label == "@M<num>").unwrap();
        assert_eq!(m_num.insert_text.as_deref(), Some("M${1:num}"));

        let p_num = items.iter().find(|it| it.label == "@P<num>").unwrap();
        assert_eq!(p_num.insert_text.as_deref(), Some("P${1:num}"));
    }

    #[test]
    fn at_meta_completion_items_e_and_p_have_empty_documentation() {
        // `@E<num>`/`@P<num>` are not present in `docs::AT_META`, so
        // `docs::at_meta_doc` returns `None` for them and the item
        // builder falls back to `""` — but it still wraps that in
        // `Some(Documentation::String(...))` rather than leaving
        // `documentation` unset. `@<num>`/`@M<num>` get real text.
        let items = at_meta_completion_items("@", 1, 0);
        let e_num = items.iter().find(|it| it.label == "@E<num>").unwrap();
        assert_eq!(
            e_num.documentation,
            Some(Documentation::String(String::new()))
        );
        let p_num = items.iter().find(|it| it.label == "@P<num>").unwrap();
        assert_eq!(
            p_num.documentation,
            Some(Documentation::String(String::new()))
        );

        let at_num = items.iter().find(|it| it.label == "@<num>").unwrap();
        assert_ne!(
            at_num.documentation,
            Some(Documentation::String(String::new()))
        );
        let m_num = items.iter().find(|it| it.label == "@M<num>").unwrap();
        assert_ne!(
            m_num.documentation,
            Some(Documentation::String(String::new()))
        );
    }

    // =========================================================================
    // 8. command_items (fallback command list), platform_items/option_items
    //    (meta values), platform_command_items (single-quote commands).
    // =========================================================================

    #[test]
    fn command_items_count_matches_table() {
        let items = command_items();
        assert_eq!(items.len(), docs::COMMAND_COMPLETIONS.len());
    }

    #[test]
    fn command_items_are_keyword_kind_unconditional_snippet_no_sort_text() {
        // CURRENT behavior; changes at C5 per COMPLETION_CORE_PLAN D18/D19.
        //
        // Every command item is kind KEYWORD (D19 wants Function) with
        // `insert_text_format` unconditionally SNIPPET (D18 wants it
        // conditional on the insert text actually containing `$`) and no
        // `sort_text` at all (D19 adds an index-based one). The `{`
        // entry's insert is a bare literal brace with no placeholder —
        // marking it SNIPPET regardless is exactly the "risks `}`/`\`
        // escaping bugs" concern D18 calls out.
        let items = command_items();
        assert!(items
            .iter()
            .all(|it| it.kind == Some(CompletionItemKind::KEYWORD)));
        assert!(items
            .iter()
            .all(|it| it.insert_text_format == Some(InsertTextFormat::SNIPPET)));
        assert!(items.iter().all(|it| it.sort_text.is_none()));

        let brace = items
            .iter()
            .find(|it| it.insert_text.as_deref() == Some("{"))
            .unwrap();
        assert_eq!(brace.kind, Some(CompletionItemKind::KEYWORD));
        assert_eq!(brace.insert_text_format, Some(InsertTextFormat::SNIPPET));
    }

    #[test]
    fn command_items_notes_entry_shape() {
        let items = command_items();
        let notes = items
            .iter()
            .find(|it| it.filter_text.as_deref() == Some("notes"))
            .unwrap();
        assert_eq!(notes.label, "cdefgabh");
        assert_eq!(notes.insert_text.as_deref(), Some("c"));
    }

    #[test]
    fn platform_items_and_option_items_have_no_insert_text_or_range() {
        // CURRENT behavior; changes at C5 per COMPLETION_CORE_PLAN D17.
        //
        // Unlike the meta-keyword/at-meta/platform-command item
        // builders, `platform_item`/`option_item` never set
        // `insert_text`, `text_edit`, `filter_text`, or `sort_text` —
        // they rely entirely on the LSP client's default word-range
        // replacement using `label`. D17 makes every item carry an
        // explicit range.
        for item in platform_items() {
            assert_eq!(item.insert_text, None, "{}", item.label);
            assert!(item.text_edit.is_none(), "{}", item.label);
            assert_eq!(item.filter_text, None, "{}", item.label);
            assert_eq!(item.kind, Some(CompletionItemKind::KEYWORD));
        }
        for item in option_items() {
            assert_eq!(item.insert_text, None, "{}", item.label);
            assert!(item.text_edit.is_none(), "{}", item.label);
        }
    }

    #[test]
    fn platform_items_count_and_docs_match_table() {
        let items = platform_items();
        assert_eq!(items.len(), docs::PLATFORM_VALUES.len());
        let megadrive = items.iter().find(|it| it.label == "megadrive").unwrap();
        assert_eq!(
            megadrive.documentation,
            docs::platform_value_doc("megadrive").map(|d| Documentation::String(d.to_string()))
        );
    }

    #[test]
    fn option_items_count_and_docs_match_table() {
        let items = option_items();
        assert_eq!(items.len(), docs::OPTION_VALUES.len());
    }

    #[test]
    fn platform_command_items_shape_and_range() {
        let line = "'";
        let items = platform_command_items(line, 1, 0);
        assert_eq!(items.len(), docs::PLATFORM_COMMANDS.len());
        assert!(items
            .iter()
            .all(|it| it.kind == Some(CompletionItemKind::KEYWORD)));
        // Unlike `command_items`, these never set `insert_text_format`
        // at all (not even conditionally) — a third, distinct snippet
        // policy from the other two item builders in this file.
        assert!(items.iter().all(|it| it.insert_text_format.is_none()));

        let underscore = items
            .iter()
            .find(|it| it.filter_text.as_deref() == Some("_<-128..127>"))
            .unwrap();
        assert_eq!(underscore.insert_text.as_deref(), Some("_0"));
        assert_eq!(
            underscore.documentation,
            Some(Documentation::String("Set transpose.".to_string()))
        );
        let edit = edit_of(underscore);
        assert_eq!(edit.range.start, Position::new(0, 1));
        assert_eq!(edit.range.end, Position::new(0, 1));
    }
}
