//! Hover documentation lookup.
//!
//! Ported from `web-ctrmml/src/editor/mml-hover.ts` (kept as the
//! behavioral source of truth). Returns the matched span on the line
//! along with the markdown body so callers (LSP / WASM) can attach a
//! range to the hover bubble — this keeps multi-character constructs
//! (`@1`, `V-8`, `_{+fc}`, `'mode 1'`) as a single hover region instead
//! of being split by the host editor's word boundaries.

use serde::Serialize;

use crate::completion::{byte_offset_to_utf16_character, utf16_character_to_byte_offset};
use crate::docs::{self, DocEntry};
use crate::instrument_header::{
    instrument_header_at, instrument_header_for_keyword_line, InstrumentHeader,
};
use crate::string_model::BorrowedLinesModel;
use crate::text_scan::{
    double_quote_bounds, is_at_number, is_in_comment, is_in_key_sig, is_ws, single_quote_bounds,
    whitespace_token_at,
};
use crate::track_selector::{parse_leading_track_selector, LineReader};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct HoverInfo {
    pub markdown: String,
    /// 0-based line index that contains the hover. Always the same as the
    /// input line; included to keep call sites symmetric with range types.
    pub line: u32,
    /// 0-based column range in UTF-8 bytes within the line.
    pub start: u32,
    pub end: u32,
    /// The `docs_json` table of the shown entry (`"commands"`,
    /// `"meta_keywords"`, `"platform_commands"`, …), or `"tracks"` for track
    /// selectors. `None` for hovers on document data: PCM paths and PSG
    /// envelopes.
    pub table: Option<&'static str>,
    /// The entry's `key` in `table`; `"letters"` or `"numeric"` for tracks.
    pub key: Option<&'static str>,
}

#[derive(Debug, Clone)]
struct Hit {
    markdown: String,
    start: usize,
    end: usize,
    source: Option<(&'static str, &'static str)>,
}

impl Hit {
    fn new(label: &str, doc: &str, start: usize, end: usize) -> Self {
        Self {
            markdown: format_hover(label, doc),
            start,
            end,
            source: None,
        }
    }

    fn entry(table: &'static str, entry: &'static DocEntry, start: usize, end: usize) -> Self {
        Self::new(entry.label, entry.doc, start, end).from(table, entry.key)
    }

    fn from(mut self, table: &'static str, key: &'static str) -> Self {
        self.source = Some((table, key));
        self
    }

    fn into_info(self, line: u32) -> HoverInfo {
        HoverInfo {
            markdown: self.markdown,
            line,
            start: self.start as u32,
            end: self.end as u32,
            table: self.source.map(|(table, _)| table),
            key: self.source.map(|(_, key)| key),
        }
    }
}

/// The first `table` entry for `key`, when it carries documentation.
fn documented(table: &'static [DocEntry], key: &str) -> Option<&'static DocEntry> {
    docs::find_entry(table, key).filter(|entry| !entry.doc.is_empty())
}

/// Hover for the command documented under `key`, covering `start..end`.
fn command_hit(key: &str, start: usize, end: usize) -> Option<Hit> {
    documented(docs::COMMAND_COMPLETIONS, key)
        .map(|entry| Hit::entry("commands", entry, start, end))
}

// ---------------------------------------------------------------------------
// Public entry points
// ---------------------------------------------------------------------------

/// Resolve a hover for `text` at zero-based `(line, col)`, where `col` is a
/// UTF-8 byte offset within the line.
pub fn hover_at(text: &str, line: u32, col: u32) -> Option<HoverInfo> {
    let doc = BorrowedLinesModel::from_text(text);
    if line >= doc.get_line_count() {
        return None;
    }
    let line_idx = line as usize;
    let col_usize = col as usize;
    let line_content = line_at(&doc, line_idx);

    if is_in_comment(line_content, col_usize) {
        return None;
    }

    let hit = two_op_hover(&doc, line_idx, col_usize)
        .or_else(|| fm_hover(&doc, line_idx, col_usize))
        .or_else(|| psg_hover(&doc, line_idx, col_usize))
        .or_else(|| track_hit(line_content, col_usize))
        .or_else(|| hover_hit_line(line_content, col_usize))?;
    Some(hit.into_info(line))
}

/// [`hover_at`] with the column and the returned range in UTF-16 code
/// units, as LSP and Monaco count them.
pub fn hover_at_utf16(text: &str, line: u32, character: u32) -> Option<HoverInfo> {
    let doc = BorrowedLinesModel::from_text(text);
    let line_text = doc.get_line_content(line.saturating_add(1));
    let col = utf16_character_to_byte_offset(line_text, character);
    let mut info = hover_at(text, line, col as u32)?;
    info.start = byte_offset_to_utf16_character(line_text, info.start as usize);
    info.end = byte_offset_to_utf16_character(line_text, info.end as usize);
    Some(info)
}

/// Format `(label, body)` into the same markdown shape used by the web
/// editor: a backticked label header followed by the body.
pub fn format_hover(label: &str, doc: &str) -> String {
    let code = label.replace('`', "\\`");
    if doc.is_empty() {
        format!("`{}`", code)
    } else {
        format!("`{}`\n\n{}", code, doc)
    }
}

// ---------------------------------------------------------------------------
// Single-line dispatch — ports `hoverHit` from web-ctrmml mml-hover.ts.
// ---------------------------------------------------------------------------

fn track_hit(line: &str, col: usize) -> Option<Hit> {
    let (token, start, end) = whitespace_token_at(line, col)?;
    track_token_hit(token, start, end)
}

fn track_token_hit(token: &str, start: usize, end: usize) -> Option<Hit> {
    if !is_track_token(start, token) {
        return None;
    }
    let doc = docs::track_doc(token)?;
    let key = if token.starts_with('*') {
        "numeric"
    } else {
        "letters"
    };
    Some(Hit::new(docs::TRACK_HELP_LABEL, doc, start, end).from("tracks", key))
}

fn hover_hit_line(line: &str, col: usize) -> Option<Hit> {
    if line.is_empty() {
        return None;
    }

    // Platform command — cover the whole quoted form so `'mode 1'`,
    // `'lfo 0 5'` etc. show as a single hover region.
    if let Some((sq_start, sq_end)) = single_quote_bounds(line, col) {
        if let Some(entry) = platform_command_at_inner(line, sq_start, sq_end) {
            return Some(Hit::entry("platform_commands", entry, sq_start, sq_end + 1));
        }
    }

    let tok = whitespace_token_at(line, col);

    if let Some((token, start, end)) = tok {
        if is_at_meta_token(token) {
            if let Some(entry) = at_meta_entry(token) {
                return Some(Hit::entry("at_meta", entry, start, end));
            }
        }
    }

    // PCM sample path (inside double quotes after `@N pcm`).
    if let Some((dq_start, dq_end)) = double_quote_bounds(line, col) {
        if dq_start + 1 < dq_end {
            let prefix = line.get(..dq_start)?.trim_end();
            let tokens: Vec<&str> = prefix.split_whitespace().collect();
            if let Some(pos) = tokens.iter().rposition(|t| t.eq_ignore_ascii_case("pcm")) {
                if pos > 0 && is_at_number(tokens[pos - 1]) {
                    let path = &line[dq_start + 1..dq_end];
                    if !path.is_empty() {
                        let escaped = path.replace('`', "\\`");
                        return Some(Hit {
                            markdown: format!(
                                "**PCM sample path**\n\nRelative to the MML file: `{}`",
                                escaped
                            ),
                            start: dq_start,
                            end: dq_end + 1,
                            source: None,
                        });
                    }
                }
            }
        }
    }

    let meta = meta_keyword_bounds(line);

    // Single-symbol commands, except in meta values such as `4/4`. Skip when
    // the cursor is inside `_{...}` or directly on its opening `{` — those
    // characters belong to the key-signature construct and are handled by
    // `command_at` below.
    let symbol_opt = line.as_bytes().get(col).copied().map(|b| b as char);
    if let Some(symbol) = symbol_opt.filter(|_| meta.is_none()) {
        let in_key_sig = is_in_key_sig(line, col);
        let on_key_sig_open =
            symbol == '{' && col > 0 && line.as_bytes().get(col - 1) == Some(&b'_');
        if "[]()|/{}><".contains(symbol) && !in_key_sig && !on_key_sig_open {
            if let Some(hit) = command_hit(&symbol.to_string(), col, col + 1) {
                return Some(hit);
            }
        }
    }

    let (token, start, end) = tok?;

    if let Some(hit) = track_token_hit(token, start, end) {
        return Some(hit);
    }

    // `#platform <value>` etc. — value-context hover.
    if let Some((_, meta_end, meta_keyword)) = meta {
        if col >= meta_end && start >= meta_end {
            let (name, table) = match meta_keyword {
                "#platform" => ("platform_values", docs::PLATFORM_VALUES),
                "#option" => ("option_values", docs::OPTION_VALUES),
                "#timesig" => ("timesig_values", docs::TIMESIG_VALUES),
                "#group" => ("group_values", docs::GROUP_VALUES),
                _ => return None,
            };
            let entry = documented(table, token)?;
            return Some(Hit::new(token, entry.doc, start, end).from(name, entry.key));
        }
    }

    if token.starts_with('#') {
        let entry = docs::find_entry(docs::META_KEYWORDS, token)?;
        let doc = if entry.doc.is_empty() {
            entry.detail
        } else {
            entry.doc
        };
        return Some(Hit::new(token, doc, start, end).from("meta_keywords", entry.key));
    }

    let offset = col.saturating_sub(start).min(token.len().saturating_sub(1));

    // `@meta` embedded inside a larger token (e.g. `cde@2fg`).
    if token.contains('@') {
        if let Some((entry, at_pos, at_end)) = at_meta_in_token(token, offset) {
            return Some(Hit::entry("at_meta", entry, start + at_pos, start + at_end));
        }
    }

    // `\` and `\=` echo commands.
    let current_char = line.as_bytes().get(col).copied().map(|b| b as char);
    if current_char == Some('\\') {
        let next_char = line.as_bytes().get(col + 1).copied().map(|b| b as char);
        let is_eq = next_char == Some('=');
        let key = if is_eq { "\\=" } else { "\\" };
        if let Some(hit) = command_hit(key, col, col + if is_eq { 2 } else { 1 }) {
            return Some(hit);
        }
    }
    if current_char == Some('=') {
        let hit = if col > 0 && line.as_bytes()[col - 1] == b'\\' {
            command_hit("\\=", col - 1, col + 1)
        } else {
            command_hit("=", col, col + 1)
        };
        if hit.is_some() {
            return hit;
        }
    }

    // Instrument type keyword (`fm`, `psg`, `pcm`, `2op`).
    if let Some(entry) = documented(docs::INSTRUMENT_TYPES, token) {
        return Some(Hit::new(token, entry.doc, start, end).from("instrument_types", entry.key));
    }

    // `rate=` / `offset=` PCM params.
    if let Some(entry) = rate_offset_in_token(token) {
        return Some(Hit::entry("rate_offset", entry, start, end));
    }

    // Regular MML command at the cursor offset.
    command_at(token, offset, start)
}

// ---------------------------------------------------------------------------
// Command parsing within a token
// ---------------------------------------------------------------------------

const COMMAND_CHARS: &str = "olQqCRLstTvVpkKMGDr^&";

/// The command under `offset` within `token`, whose first byte sits at
/// `base` on the line.
fn command_at(token: &str, offset: usize, base: usize) -> Option<Hit> {
    let bytes = token.as_bytes();
    let mut idx = 0;
    while idx < bytes.len() {
        let ch = bytes[idx] as char;

        // Notes: `cdefgabh[+|-|=][duration][.]`
        if matches!(ch, 'a'..='h') {
            let start = idx;
            let mut end = idx + 1;
            let mut accidental_index: Option<usize> = None;

            if end < bytes.len() && matches!(bytes[end] as char, '+' | '-' | '=') {
                accidental_index = Some(end);
                end += 1;
            }
            if end < bytes.len() && bytes[end] == b':' {
                end += 1;
                while end < bytes.len() && (bytes[end] as char).is_ascii_digit() {
                    end += 1;
                }
            } else {
                while end < bytes.len() && (bytes[end] as char).is_ascii_digit() {
                    end += 1;
                }
            }
            while end < bytes.len() && bytes[end] == b'.' {
                end += 1;
            }

            if offset >= start && offset < end {
                if accidental_index == Some(offset) {
                    let acc_key = &token[offset..offset + 1];
                    return command_hit(acc_key, base + offset, base + offset + 1);
                }
                return command_hit("notes", base + start, base + end);
            }
            idx = end;
            continue;
        }

        // `_` / `__` / `_{...}`
        if ch == '_' {
            let start = idx;
            let mut end = idx + 1;
            let mut key: &str = "_";

            if end < bytes.len() && bytes[end] == b'_' {
                key = "__";
                end += 1;
            }

            if end < bytes.len() && bytes[end] == b'{' {
                key = "_{";
                end += 1;
                while end < bytes.len() && bytes[end] != b'}' {
                    end += 1;
                }
                if end < bytes.len() && bytes[end] == b'}' {
                    end += 1;
                }
            } else {
                if end < bytes.len() && (bytes[end] == b'+' || bytes[end] == b'-') {
                    end += 1;
                }
                while end < bytes.len() && (bytes[end] as char).is_ascii_digit() {
                    end += 1;
                }
            }

            if offset >= start && offset < end {
                return command_hit(key, base + start, base + end);
            }
            idx = end;
            continue;
        }

        // `\` / `\=...` echo
        if ch == '\\' {
            let start = idx;
            let mut end = idx + 1;
            let mut key: &str = "\\";

            if end < bytes.len() && bytes[end] == b'=' {
                key = "\\=";
                end += 1;
                if end < bytes.len() && (bytes[end] == b'+' || bytes[end] == b'-') {
                    end += 1;
                }
                while end < bytes.len() && (bytes[end] as char).is_ascii_digit() {
                    end += 1;
                }
                if end < bytes.len() && bytes[end] == b',' {
                    end += 1;
                }
                if end < bytes.len() && (bytes[end] == b'+' || bytes[end] == b'-') {
                    end += 1;
                }
                while end < bytes.len() && (bytes[end] as char).is_ascii_digit() {
                    end += 1;
                }
            } else {
                while end < bytes.len() && (bytes[end] as char).is_ascii_digit() {
                    end += 1;
                }
            }

            if offset >= start && offset < end {
                return command_hit(key, base + start, base + end);
            }
            idx = end;
            continue;
        }

        // Regular single-letter commands, with optional `+/-` prefix.
        if COMMAND_CHARS.contains(ch) {
            let start = idx;
            let mut end = idx + 1;
            let mut signed = false;
            if end < bytes.len() && (bytes[end] == b'+' || bytes[end] == b'-') {
                signed = true;
                end += 1;
            }
            while end < bytes.len() && (bytes[end] as char).is_ascii_digit() {
                end += 1;
            }

            if offset >= start && offset < end {
                let key = &token[start..start + 1];
                let entry = docs::command_entry(key, signed)?;
                return Some(Hit::entry("commands", entry, base + start, base + end));
            }
            idx = end;
            continue;
        }

        idx += 1;
    }
    None
}

// ---------------------------------------------------------------------------
// Text helpers
// ---------------------------------------------------------------------------

fn is_at_meta_token(token: &str) -> bool {
    // Matches `@<digits>`, `@E<digits>`, `@M<digits>`, `@P<digits>`.
    let bytes = token.as_bytes();
    if bytes.first() != Some(&b'@') {
        return false;
    }
    let mut idx = 1;
    if idx < bytes.len() && matches!(bytes[idx], b'E' | b'M' | b'P') {
        idx += 1;
    }
    if idx >= bytes.len() {
        return false;
    }
    bytes[idx..].iter().all(|b| b.is_ascii_digit())
}

fn at_meta_label(after_at: &str) -> &'static str {
    match after_at.as_bytes().first() {
        Some(b'E') => "@E<num>",
        Some(b'M') => "@M<num>",
        Some(b'P') => "@P<num>",
        _ => "@<num>",
    }
}

fn at_meta_entry(token: &str) -> Option<&'static DocEntry> {
    let label = at_meta_label(&token[1..]);
    docs::find_entry_by_label(docs::AT_META, label).filter(|entry| !entry.doc.is_empty())
}

fn at_meta_in_token(token: &str, offset: usize) -> Option<(&'static DocEntry, usize, usize)> {
    let prefix = token.get(..=offset)?;
    let at_pos = prefix.rfind('@')?;
    let rest = &token[at_pos..];

    let label = at_meta_label(&rest[1..]);
    let mut idx = if label == "@<num>" { 1 } else { 2 };
    let bytes = rest.as_bytes();
    let digit_start = idx;
    while idx < bytes.len() && (bytes[idx] as char).is_ascii_digit() {
        idx += 1;
    }
    if idx == digit_start {
        return None;
    }
    let at_end = at_pos + idx;
    if offset >= at_pos && offset < at_end {
        let entry = at_meta_entry(rest)?;
        return Some((entry, at_pos, at_end));
    }
    None
}

fn rate_offset_in_token(token: &str) -> Option<&'static DocEntry> {
    ["rate=", "offset="]
        .into_iter()
        .find(|key| token.starts_with(key))
        .and_then(|key| documented(docs::RATE_OFFSET, key))
}

fn platform_command_at_inner(
    line: &str,
    sq_start: usize,
    sq_end: usize,
) -> Option<&'static DocEntry> {
    if sq_start + 1 >= sq_end {
        return None;
    }
    let inner = line.get(sq_start + 1..sq_end)?.trim_start();
    let cmd = inner.split_whitespace().next()?;
    documented(docs::PLATFORM_COMMANDS, cmd)
}

fn is_track_token(start: usize, token: &str) -> bool {
    // `*N` is a macro/subroutine reference and is valid anywhere on a line.
    if let Some(rest) = token.strip_prefix('*') {
        if !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit()) {
            return true;
        }
    }
    if start != 0 {
        return false;
    }
    !token.is_empty() && token.bytes().all(|b| b.is_ascii_uppercase())
}

fn meta_keyword_bounds(line: &str) -> Option<(usize, usize, &str)> {
    let bytes = line.as_bytes();
    let mut start = 0;
    while start < bytes.len() && is_ws(bytes[start]) {
        start += 1;
    }
    if start >= bytes.len() || bytes[start] != b'#' {
        return None;
    }
    let mut end = start + 1;
    while end < bytes.len() && !is_ws(bytes[end]) {
        end += 1;
    }
    Some((start, end, &line[start..end]))
}

// ---------------------------------------------------------------------------
// FM / 2op / PSG parameter hover (context-sensitive — needs full text)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy)]
struct NumberSpan {
    start: usize,
    end: usize,
}

fn parse_numbers(line: &str, parse_end: usize) -> Vec<NumberSpan> {
    let mut out = Vec::new();
    let bytes = line.as_bytes();
    let end_lim = parse_end.min(line.len());
    let mut idx = 0;
    while idx < end_lim {
        while idx < end_lim && is_ws(bytes[idx]) {
            idx += 1;
        }
        if idx >= end_lim {
            break;
        }
        let start = idx;
        if bytes[idx] == b'+' || bytes[idx] == b'-' {
            idx += 1;
        }
        if idx >= end_lim || !(bytes[idx] as char).is_ascii_digit() {
            idx += 1;
            continue;
        }
        while idx < end_lim && (bytes[idx] as char).is_ascii_digit() {
            idx += 1;
        }
        out.push(NumberSpan { start, end: idx });
    }
    out
}

fn number_index_at(numbers: &[NumberSpan], col: usize) -> Option<usize> {
    for (i, n) in numbers.iter().enumerate() {
        if (col >= n.start && col < n.end) || (col == n.end && col > n.start) {
            return Some(i);
        }
    }
    None
}

fn is_numeric_param_line(line: &str, parse_end: usize) -> bool {
    let slice = &line[..parse_end.min(line.len())];
    for ch in slice.chars() {
        if ch.is_ascii_whitespace() || ch.is_ascii_digit() || ch == '+' || ch == '-' || ch == ',' {
            continue;
        }
        return false;
    }
    true
}

fn strip_at_number(line: &str, numbers: Vec<NumberSpan>) -> Vec<NumberSpan> {
    if let Some(first) = numbers.first() {
        if first.start > 0 && line.as_bytes()[first.start - 1] == b'@' {
            return numbers.into_iter().skip(1).collect();
        }
    }
    numbers
}

fn line_at<'a>(doc: &'a BorrowedLinesModel, line_idx: usize) -> &'a str {
    doc.get_line_content(line_idx as u32 + 1)
}

/// The `@N <keyword>` header written on the single line `line_idx`.
fn definition_header<'a>(
    doc: &'a BorrowedLinesModel,
    line_idx: usize,
    keyword: &str,
) -> Option<InstrumentHeader<'a>> {
    let line_number = line_idx as u32 + 1;
    instrument_header_at(doc, line_number)
        .filter(|header| header.keyword_line == line_number && header.is(keyword))
}

/// The header whose `keyword` stands alone on `line_idx`, below a bare `@N`.
fn keyword_line_header<'a>(
    doc: &'a BorrowedLinesModel,
    line_idx: usize,
    keyword: &str,
) -> Option<InstrumentHeader<'a>> {
    instrument_header_for_keyword_line(doc, line_idx as u32 + 1).filter(|header| header.is(keyword))
}

/// Scan upward from `line_idx` for the `@N` line of a `<keyword>` header.
fn find_instrument_anchor(
    doc: &BorrowedLinesModel,
    line_idx: usize,
    keyword: &str,
) -> Option<usize> {
    for idx in (0..=line_idx).rev() {
        let line = line_at(doc, idx);
        let trimmed = line.trim_start();
        if trimmed.starts_with('@') {
            return instrument_header_at(doc, idx as u32 + 1)
                .filter(|header| header.is(keyword))
                .map(|_| idx);
        }
        if trimmed.starts_with('#') || parse_leading_track_selector(line).is_some() {
            return None;
        }
    }
    None
}

fn fm_operator_index(doc: &BorrowedLinesModel, anchor: usize, line_idx: usize) -> Option<usize> {
    let mut count = 0usize;
    for idx in (anchor + 1)..=line_idx {
        let line = line_at(doc, idx);
        let trimmed = line.trim_start();
        if trimmed.starts_with('@') {
            break;
        }
        if trimmed.is_empty() || trimmed.starts_with(';') {
            continue;
        }
        let parse_end = line.find(';').unwrap_or(line.len());
        let numbers = parse_numbers(line, parse_end);
        if numbers.len() == 10 {
            count += 1;
        }
    }
    if count == 0 {
        None
    } else {
        Some(count)
    }
}

const FM_10_KEYS: [&str; 10] = ["AR", "DR", "SR", "RR", "SL", "TL", "KS", "ML", "DT", "SSG"];

fn fm_hover(doc: &BorrowedLinesModel, line_idx: usize, col: usize) -> Option<Hit> {
    let line = line_at(doc, line_idx);
    let trimmed = line.trim_start();
    if trimmed.is_empty() || trimmed.starts_with(';') {
        return None;
    }

    let is_anchor = definition_header(doc, line_idx, "fm").is_some();
    let is_ml_keyword = !is_anchor && keyword_line_header(doc, line_idx, "fm").is_some();
    if trimmed.starts_with('@') && !is_anchor {
        return None;
    }

    let parse_end = line.find(';').unwrap_or(line.len());
    if col >= parse_end {
        return None;
    }

    if is_anchor || is_ml_keyword {
        let mut numbers = parse_numbers(line, parse_end);
        numbers = strip_at_number(line, numbers);
        if numbers.len() < 2 {
            return None;
        }
        let idx = number_index_at(&numbers, col)?;
        if idx >= 2 {
            return None;
        }
        let key = if idx == 0 { "ALG" } else { "FB" };
        let entry = docs::find_entry(docs::FM_PARAM_DOCS, key)?;
        let n = numbers[idx];
        return Some(Hit::entry("fm_params", entry, n.start, n.end));
    }

    if !is_numeric_param_line(line, parse_end) {
        return None;
    }
    let anchor = find_instrument_anchor(doc, line_idx.saturating_sub(1), "fm")?;
    if line_idx <= anchor {
        return None;
    }
    let numbers = parse_numbers(line, parse_end);
    if numbers.is_empty() {
        return None;
    }

    let (param_keys, op_index): (Vec<&'static str>, Option<usize>) = match numbers.len() {
        2 => (vec!["ALG", "FB"], None),
        10 => (
            FM_10_KEYS.to_vec(),
            fm_operator_index(doc, anchor, line_idx),
        ),
        1 => (vec!["TRS"], None),
        _ => return None,
    };

    let idx = number_index_at(&numbers, col)?;
    let key = *param_keys.get(idx)?;
    let entry = docs::find_entry(docs::FM_PARAM_DOCS, key)?;
    let n = numbers[idx];
    let label = match op_index {
        Some(op) => format!("OP{} {}", op, entry.label),
        None => entry.label.to_string(),
    };
    Some(Hit::new(&label, entry.doc, n.start, n.end).from("fm_params", entry.key))
}

fn two_op_hover(doc: &BorrowedLinesModel, line_idx: usize, col: usize) -> Option<Hit> {
    let line = line_at(doc, line_idx);
    let trimmed = line.trim_start();
    if trimmed.is_empty() || trimmed.starts_with(';') {
        return None;
    }
    let header = definition_header(doc, line_idx, "2op")
        .or_else(|| keyword_line_header(doc, line_idx, "2op"))?;

    let parse_end = line.find(';').unwrap_or(line.len());
    if col >= parse_end {
        return None;
    }

    // `2op` itself reads as a number; drop it along with the `@N` digits.
    let numbers: Vec<NumberSpan> = strip_at_number(line, parse_numbers(line, parse_end))
        .into_iter()
        .filter(|n| n.start < header.keyword_start || n.start >= header.keyword_end())
        .collect();
    if numbers.len() < 6 {
        return None;
    }
    let idx = number_index_at(&numbers, col)?;
    if idx >= 6 {
        return None;
    }
    let entry = docs::TWO_OP_PARAM_DOCS.get(idx)?;
    let n = numbers[idx];
    Some(Hit::entry("two_op_params", entry, n.start, n.end))
}

fn psg_hover(doc: &BorrowedLinesModel, line_idx: usize, col: usize) -> Option<Hit> {
    let line = line_at(doc, line_idx);
    let trimmed = line.trim_start();
    if trimmed.is_empty() || trimmed.starts_with(';') {
        return None;
    }

    let mut in_psg_block = false;
    let mut data_start_col: usize = 0;

    if trimmed.starts_with('@') {
        let header = definition_header(doc, line_idx, "psg")?;
        in_psg_block = true;
        data_start_col = header.keyword_end();
    } else if let Some(header) = keyword_line_header(doc, line_idx, "psg") {
        in_psg_block = true;
        data_start_col = header.keyword_end();
    } else {
        if parse_leading_track_selector(line).is_some() {
            return None;
        }
        if find_instrument_anchor(doc, line_idx.saturating_sub(1), "psg").is_some() {
            in_psg_block = true;
        }
    }

    if !in_psg_block {
        return None;
    }
    let parse_end = line.find(';').unwrap_or(line.len());
    if col >= parse_end || col < data_start_col {
        return None;
    }

    let (token, start, end) = whitespace_token_at(line, col)?;
    let mk = |label: &str, body: &str| Hit::new(label, body, start, end);

    if token == "|" {
        return Some(mk(
            "PSG envelope loop position",
            "Set the loop point inside this PSG envelope. Playback jumps back here when the envelope reaches the end.",
        ));
    }
    if token == "/" {
        return Some(mk(
            "PSG envelope sustain position",
            "Set the sustain point inside this PSG envelope. Sustain keeps the envelope around this point while the note is held.",
        ));
    }
    if is_l_colon_number(token) {
        return Some(mk(
            "PSG envelope default step length",
            "Set the default length in frames for the following PSG envelope values on this definition line.",
        ));
    }
    if is_slide_token(token) {
        return Some(mk(
            "PSG envelope slide",
            "Slide from one PSG volume value to another. `value>target:length` slides over the given number of frames.",
        ));
    }
    if is_hold_token(token) {
        return Some(mk(
            "PSG envelope hold",
            "Keep this PSG volume value for the specified number of frames.",
        ));
    }
    if token.chars().all(|c| c.is_ascii_digit()) && !token.is_empty() {
        return Some(mk(
            "PSG envelope volume",
            "PSG instruments are sequences of volume values. `15` is loudest, `0` is silence. Use `>` for slides, `:` for frame length, `|` for loop, and `/` for sustain.",
        ));
    }

    Some(mk(
        "PSG envelope",
        "PSG instruments are sequences of volume values. `15` is loudest, `0` is silence.\nUse `>` for slides, `:` for frame length, `|` for loop, and `/` for sustain.",
    ))
}

fn is_l_colon_number(token: &str) -> bool {
    let bytes = token.as_bytes();
    bytes.len() >= 3
        && bytes[0] == b'l'
        && bytes[1] == b':'
        && bytes[2..].iter().all(|b| b.is_ascii_digit())
}

fn is_slide_token(token: &str) -> bool {
    // `digits > digits` or `digits > digits : digits`
    let bytes = token.as_bytes();
    let mut idx = 0;
    while idx < bytes.len() && (bytes[idx] as char).is_ascii_digit() {
        idx += 1;
    }
    if idx == 0 || idx >= bytes.len() || bytes[idx] != b'>' {
        return false;
    }
    idx += 1;
    let target_start = idx;
    while idx < bytes.len() && (bytes[idx] as char).is_ascii_digit() {
        idx += 1;
    }
    if idx == target_start {
        return false;
    }
    if idx == bytes.len() {
        return true;
    }
    if bytes[idx] != b':' {
        return false;
    }
    idx += 1;
    let len_start = idx;
    while idx < bytes.len() && (bytes[idx] as char).is_ascii_digit() {
        idx += 1;
    }
    idx == bytes.len() && idx > len_start
}

fn is_hold_token(token: &str) -> bool {
    let bytes = token.as_bytes();
    let mut idx = 0;
    while idx < bytes.len() && (bytes[idx] as char).is_ascii_digit() {
        idx += 1;
    }
    if idx == 0 || idx >= bytes.len() || bytes[idx] != b':' {
        return false;
    }
    idx += 1;
    let rest_start = idx;
    while idx < bytes.len() && (bytes[idx] as char).is_ascii_digit() {
        idx += 1;
    }
    idx == bytes.len() && idx > rest_start
}

#[cfg(test)]
mod tests {
    use super::*;

    fn span(info: &HoverInfo) -> (u32, u32) {
        (info.start, info.end)
    }

    fn doc_ref(text: &str, line: u32, col: u32) -> Option<(&'static str, &'static str)> {
        let info = hover_at(text, line, col)?;
        Some((info.table?, info.key?))
    }

    #[test]
    fn hovers_name_the_docs_entry_they_show() {
        let text = concat!(
            "#platform mdsdrv\n",
            "@1 fm\n",
            " 3 4\n",
            "@2 pcm \"kick.wav\" rate=8000\n",
            "@3 psg 15 0\n",
            "AB o4 V-8 c+ 'mode 1' @M1 [c]2 *40\n",
        );
        assert_eq!(doc_ref(text, 0, 2), Some(("meta_keywords", "#platform")));
        assert_eq!(doc_ref(text, 0, 12), Some(("platform_values", "mdsdrv")));
        assert_eq!(doc_ref(text, 1, 0), Some(("at_meta", "@<num>")));
        assert_eq!(doc_ref(text, 1, 3), Some(("instrument_types", "fm")));
        assert_eq!(doc_ref(text, 2, 1), Some(("fm_params", "ALG")));
        assert_eq!(doc_ref(text, 3, 20), Some(("rate_offset", "rate=")));
        assert_eq!(doc_ref(text, 5, 0), Some(("tracks", "letters")));
        assert_eq!(doc_ref(text, 5, 3), Some(("commands", "o")));
        assert_eq!(doc_ref(text, 5, 6), Some(("commands", "V")));
        assert_eq!(doc_ref(text, 5, 10), Some(("commands", "notes")));
        assert_eq!(doc_ref(text, 5, 15), Some(("platform_commands", "mode")));
        assert_eq!(doc_ref(text, 5, 23), Some(("at_meta", "@M<num>")));
        assert_eq!(doc_ref(text, 5, 26), Some(("commands", "[")));
        assert_eq!(doc_ref(text, 5, 31), Some(("tracks", "numeric")));
        assert_eq!(
            doc_ref("@1 2op 1 2 3 4 5 0\n", 0, 9),
            Some(("two_op_params", "ML1"))
        );
        // Document data has no docs entry.
        assert_eq!(doc_ref(text, 3, 9), None);
        assert!(hover_at(text, 3, 9).is_some());
        assert_eq!(doc_ref(text, 4, 7), None);
        assert!(hover_at(text, 4, 7).is_some());
    }

    #[test]
    fn meta_values_are_not_commands() {
        assert_eq!(
            doc_ref("#timesig 4/4", 0, 10),
            Some(("timesig_values", "4/4"))
        );
        assert!(hover_at("#timesig 7/8", 0, 10).is_none());
        assert!(hover_at("#title a/b [c]", 0, 8).is_none());
        assert!(hover_at("#title a/b [c]", 0, 11).is_none());
    }

    #[test]
    fn at_number_full_token_range() {
        // `@1` lives at byte range [2, 4) in "A @1 c". Cursor on the `@` (col
        // 2) and on the `1` (col 3) should produce the same hit.
        let info = hover_at("A @1 c", 0, 2).unwrap();
        assert_eq!(span(&info), (2, 4));
        assert!(info.markdown.starts_with("`@<num>`"));
        let info2 = hover_at("A @1 c", 0, 3).unwrap();
        assert_eq!(span(&info2), (2, 4));
    }

    #[test]
    fn at_m_number_full_token_range() {
        let info = hover_at("A @M1 c", 0, 2).unwrap();
        assert_eq!(span(&info), (2, 5));
        assert!(info.markdown.starts_with("`@M<num>`"));
    }

    #[test]
    fn signed_v_picks_signed_variant() {
        let info = hover_at("A V-8 c", 0, 4).unwrap();
        assert!(info.markdown.starts_with("`V<-128..+127>`"));
        assert_eq!(span(&info), (2, 5));
    }

    #[test]
    fn unsigned_v_picks_unsigned_variant() {
        let info = hover_at("A V8 c", 0, 2).unwrap();
        assert!(info.markdown.starts_with("`V<0..127>`"));
    }

    #[test]
    fn signed_p_picks_signed_variant() {
        let info = hover_at("A p+5 c", 0, 4).unwrap();
        assert!(info.markdown.starts_with("`p<-128..127>`"));
        assert_eq!(span(&info), (2, 5));
    }

    #[test]
    fn note_with_dot_covers_full_token() {
        let info = hover_at("g4. c", 0, 0).unwrap();
        assert!(info.markdown.starts_with("`cdefgabh`"));
        assert!(info.markdown.contains("dotted note"));
        assert_eq!(span(&info), (0, 3));
        // cursor on the dot still hits the same range
        let info2 = hover_at("g4. c", 0, 2).unwrap();
        assert_eq!(span(&info2), (0, 3));
    }

    #[test]
    fn star_macro_works_mid_line() {
        // "A *32 *33 | c" — the second `*33` is at bytes [6, 9). Title is
        // the canonical `A..Z / *<num>` label (not the literal token) so
        // hover matches the Help Panel's wording.
        let info = hover_at("A *32 *33 | c", 0, 7).unwrap();
        assert_eq!(span(&info), (6, 9));
        assert!(info.markdown.starts_with("`A..Z / *<num>`"));
    }

    #[test]
    fn star_macro_works_at_line_start() {
        let info = hover_at("*33 d f a f", 0, 1).unwrap();
        assert_eq!(span(&info), (0, 3));
    }

    #[test]
    fn platform_command_covers_whole_quote() {
        // "A 'mode 1'": quote bounds are [2, 9]; hit range = [2, 10).
        let info = hover_at("A 'mode 1'", 0, 7).unwrap();
        assert_eq!(span(&info), (2, 10));
        assert!(info.markdown.starts_with("`mode <0..2>`"));
    }

    #[test]
    fn platform_command_multi_arg_quote() {
        let info = hover_at("A 'lfo 0 5'", 0, 8).unwrap();
        assert_eq!(span(&info), (2, 11));
        assert!(info.markdown.starts_with("`lfo <0..3> <0..7>`"));
    }

    #[test]
    fn key_signature_covers_whole_underscore_brace() {
        // "A _{+fc} c" — `_{+fc}` is at bytes [2, 8).
        for col in 2..=7 {
            let info = hover_at("A _{+fc} c", 0, col).unwrap_or_else(|| {
                panic!("missing hover at col {col}");
            });
            assert_eq!(span(&info), (2, 8), "col {col}");
            assert!(info.markdown.starts_with("`_{<key signature>}`"));
        }
    }

    #[test]
    fn chord_conditional_braces_still_resolve() {
        let info = hover_at("{a/b/c}", 0, 0).unwrap();
        assert!(info.markdown.starts_with("`{ / }`"));
    }

    #[test]
    fn repeat_block_open_bracket() {
        let info = hover_at("[cdefg]2", 0, 0).unwrap();
        assert!(info.markdown.starts_with("`[ ]`"));
    }

    #[test]
    fn fm_parameters_resolve_under_a_multi_line_header() {
        let text = "@1\n; comment\nfm\n  0 5\n";
        let info = hover_at(text, 3, 2).expect("ALG hover");
        assert!(info.markdown.starts_with("`Algorithm"), "{}", info.markdown);
    }

    #[test]
    fn fm_parameters_resolve_under_an_upper_case_header() {
        let info = hover_at("@1 FM\n  0 5\n", 1, 4).expect("FB hover");
        assert!(info.markdown.starts_with("`Feedback"), "{}", info.markdown);
    }

    #[test]
    fn psg_data_starts_after_an_upper_case_keyword() {
        let info = hover_at("@2 PSG 15\n", 0, 7).expect("PSG volume hover");
        assert!(info.markdown.starts_with("`PSG envelope volume`"));
        assert_eq!(span(&info), (7, 9));
    }

    #[test]
    fn utf16_hover_counts_columns_after_japanese_text() {
        // `あ` is one UTF-16 unit and three bytes.
        let info = hover_at_utf16("A 'あ' @1 c", 0, 7).expect("hover on @1");
        assert_eq!(span(&info), (6, 8));
        assert!(info.markdown.starts_with("`@<num>`"));
    }

    #[test]
    fn e_and_p_no_longer_resolve() {
        // The engine no-ops these; they should produce no hover.
        assert!(hover_at("A E5 c", 0, 2).is_none());
        assert!(hover_at("A P5 c", 0, 2).is_none());
    }
}
