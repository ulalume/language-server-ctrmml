//! Hover documentation lookup.
//!
//! Ported from `web-ctrmml/src/editor/mml-hover.ts` (kept as the
//! behavioral source of truth). Returns the matched span on the line
//! along with the markdown body so callers (LSP / WASM) can attach a
//! range to the hover bubble — this keeps multi-character constructs
//! (`@1`, `V-8`, `_{+fc}`, `'mode 1'`) as a single hover region instead
//! of being split by the host editor's word boundaries.

use crate::docs;
use crate::text_scan::is_in_key_sig;
use crate::track_selector::parse_leading_track_selector;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HoverInfo {
    pub markdown: String,
    /// 0-based line index that contains the hover. Always the same as the
    /// input line; included to keep call sites symmetric with range types.
    pub line: u32,
    /// 0-based column range (UTF-8 byte offsets; ctrmml source is ASCII so
    /// this matches both char- and UTF-16 columns).
    pub start: u32,
    pub end: u32,
}

#[derive(Debug, Clone)]
struct Hit {
    markdown: String,
    start: usize,
    end: usize,
}

impl Hit {
    fn into_info(self, line: u32) -> HoverInfo {
        HoverInfo {
            markdown: self.markdown,
            line,
            start: self.start as u32,
            end: self.end as u32,
        }
    }
}

// ---------------------------------------------------------------------------
// Public entry points
// ---------------------------------------------------------------------------

/// Resolve a hover for `text` at zero-based `(line, col)`.
pub fn hover_at(text: &str, line: u32, col: u32) -> Option<HoverInfo> {
    let line_idx = line as usize;
    let col_usize = col as usize;
    let line_content = nth_line(text, line_idx)?;

    if crate::text_scan::is_in_comment(line_content, col_usize) {
        return None;
    }

    if let Some(hit) = two_op_hover(text, line_idx, col_usize) {
        return Some(hit.into_info(line));
    }
    if let Some(hit) = fm_hover(text, line_idx, col_usize) {
        return Some(hit.into_info(line));
    }
    if let Some(hit) = psg_hover(text, line_idx, col_usize) {
        return Some(hit.into_info(line));
    }

    if let Some(tok) = token_at(line_content, col_usize) {
        if is_track_token(line_content, tok.start, tok.token) {
            if let Some(doc) = docs::track_doc(tok.token) {
                return Some(
                    Hit {
                        markdown: format_hover(docs::TRACK_HELP_LABEL, doc),
                        start: tok.start,
                        end: tok.end,
                    }
                    .into_info(line),
                );
            }
        }
    }

    hover_hit_line(line_content, col_usize).map(|h| h.into_info(line))
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

fn hover_hit_line(line: &str, col: usize) -> Option<Hit> {
    if line.is_empty() {
        return None;
    }

    // Platform command — cover the whole quoted form so `'mode 1'`,
    // `'lfo 0 5'` etc. show as a single hover region.
    if let Some((sq_start, sq_end)) = single_quote_bounds(line, col) {
        if let Some((cmd_label, cmd_doc)) = platform_command_at_inner(line, sq_start, sq_end) {
            return Some(Hit {
                markdown: format_hover(cmd_label, cmd_doc),
                start: sq_start,
                end: sq_end + 1,
            });
        }
    }

    let tok = token_at(line, col);

    if let Some(tok) = tok.as_ref() {
        if tok.token.starts_with('@') && is_at_meta_token(tok.token) {
            if let Some((label, doc)) = at_meta_lookup(tok.token) {
                return Some(Hit {
                    markdown: format_hover(label, doc),
                    start: tok.start,
                    end: tok.end,
                });
            }
        }
    }

    // PCM sample path (inside double quotes after `@N pcm`).
    if let Some((dq_start, dq_end)) = double_quote_bounds(line, col) {
        if dq_start + 1 < dq_end {
            let prefix = line.get(..dq_start)?.trim_end();
            let tokens: Vec<&str> = prefix.split_whitespace().collect();
            if let Some(pos) = tokens.iter().rposition(|t| *t == "pcm") {
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
                        });
                    }
                }
            }
        }
    }

    // Single-symbol commands. Skip when the cursor is inside `_{...}` or
    // directly on its opening `{` — those characters belong to the
    // key-signature construct and are handled by `command_at` below.
    let symbol_opt = line.as_bytes().get(col).copied().map(|b| b as char);
    if let Some(symbol) = symbol_opt {
        let in_key_sig = is_in_key_sig(line, col);
        let on_key_sig_open =
            symbol == '{' && col > 0 && line.as_bytes().get(col - 1) == Some(&b'_');
        if "[]()|/{}><".contains(symbol) && !in_key_sig && !on_key_sig_open {
            let key = symbol.to_string();
            if let Some(doc) = docs::command_doc(&key) {
                let label = docs::command_completion_label(&key).unwrap_or(&key);
                return Some(Hit {
                    markdown: format_hover(label, doc),
                    start: col,
                    end: col + 1,
                });
            }
        }
    }

    let tok = tok?;
    let TokenSpan { token, start, end } = tok;

    if is_track_token(line, start, token) {
        if let Some(doc) = docs::track_doc(token) {
            return Some(Hit {
                markdown: format_hover(docs::TRACK_HELP_LABEL, doc),
                start,
                end,
            });
        }
    }

    // `#platform <value>` etc. — value-context hover.
    if let Some((_, meta_end, meta_keyword)) = meta_keyword_bounds(line) {
        if col >= meta_end && start >= meta_end {
            let doc = match meta_keyword {
                "#platform" => docs::platform_value_doc(token),
                "#option" => docs::option_value_doc(token),
                "#timesig" => docs::timesig_value_doc(token),
                "#group" => docs::group_value_doc(token),
                _ => None,
            };
            if let Some(doc) = doc {
                return Some(Hit {
                    markdown: format_hover(token, doc),
                    start,
                    end,
                });
            }
            return None;
        }
    }

    if token.starts_with('#') {
        if let Some(doc) = docs::meta_doc(token) {
            return Some(Hit {
                markdown: format_hover(token, doc),
                start,
                end,
            });
        }
        return None;
    }

    let offset = col.saturating_sub(start).min(token.len().saturating_sub(1));

    // `@meta` embedded inside a larger token (e.g. `cde@2fg`).
    if token.contains('@') {
        if let Some((label, doc, at_pos, at_end)) = at_meta_in_token(token, offset) {
            return Some(Hit {
                markdown: format_hover(label, doc),
                start: start + at_pos,
                end: start + at_end,
            });
        }
    }

    // `\` and `\=` echo commands.
    let current_char = line.as_bytes().get(col).copied().map(|b| b as char);
    if current_char == Some('\\') {
        let next_char = line.as_bytes().get(col + 1).copied().map(|b| b as char);
        let is_eq = next_char == Some('=');
        let key = if is_eq { "\\=" } else { "\\" };
        if let Some(doc) = docs::command_doc(key) {
            let label = docs::command_completion_label(key).unwrap_or(key);
            return Some(Hit {
                markdown: format_hover(label, doc),
                start: col,
                end: col + if is_eq { 2 } else { 1 },
            });
        }
    }
    if current_char == Some('=') {
        if col > 0 && line.as_bytes()[col - 1] == b'\\' {
            if let Some(doc) = docs::command_doc("\\=") {
                let label = docs::command_completion_label("\\=").unwrap_or("\\=");
                return Some(Hit {
                    markdown: format_hover(label, doc),
                    start: col - 1,
                    end: col + 1,
                });
            }
        } else if let Some(doc) = docs::command_doc("=") {
            let label = docs::command_completion_label("=").unwrap_or("=");
            return Some(Hit {
                markdown: format_hover(label, doc),
                start: col,
                end: col + 1,
            });
        }
    }

    // Instrument type keyword (`fm`, `psg`, `pcm`, `2op`).
    if let Some(doc) = docs::instrument_doc(token) {
        return Some(Hit {
            markdown: format_hover(token, doc),
            start,
            end,
        });
    }

    // `rate=` / `offset=` PCM params.
    if let Some((label, doc)) = rate_offset_in_token(token) {
        return Some(Hit {
            markdown: format_hover(label, doc),
            start,
            end,
        });
    }

    // Regular MML command at the cursor offset.
    if let Some(cmd) = command_at(token, offset) {
        return Some(Hit {
            markdown: format_hover(cmd.label, cmd.doc),
            start: start + cmd.start,
            end: start + cmd.end,
        });
    }

    None
}

// ---------------------------------------------------------------------------
// Command parsing within a token
// ---------------------------------------------------------------------------

const COMMAND_CHARS: &str = "olQqCRLstTvVpkKMGDr^&";

struct CommandHit {
    label: &'static str,
    doc: &'static str,
    start: usize,
    end: usize,
}

fn command_at(token: &str, offset: usize) -> Option<CommandHit> {
    let bytes = token.as_bytes();
    let mut idx = 0;
    while idx < bytes.len() {
        let ch = bytes[idx] as char;

        // Notes: `cdefgabh[+|-|=][duration][.]`
        if matches!(ch, 'a'..='h') {
            let start = idx;
            let mut end = idx + 1;
            let mut accidental_index: Option<usize> = None;

            if end < bytes.len()
                && matches!(bytes[end] as char, '+' | '-' | '=')
            {
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
                if let Some(acc_idx) = accidental_index {
                    if offset == acc_idx {
                        let acc_key: &'static str = match token.as_bytes()[acc_idx] {
                            b'+' => "+",
                            b'-' => "-",
                            b'=' => "=",
                            _ => return None,
                        };
                        if let Some(doc) = docs::command_doc(acc_key) {
                            let label = docs::command_completion_label(acc_key).unwrap_or(acc_key);
                            return Some(CommandHit {
                                label,
                                doc,
                                start: acc_idx,
                                end: acc_idx + 1,
                            });
                        }
                        return None;
                    }
                }
                if let Some(doc) = docs::command_doc("notes") {
                    let label = docs::command_completion_label("notes").unwrap_or("notes");
                    return Some(CommandHit {
                        label,
                        doc,
                        start,
                        end,
                    });
                }
                return None;
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
                if let Some(doc) = docs::command_doc(key) {
                    let label = docs::command_completion_label(key).unwrap_or(key);
                    return Some(CommandHit {
                        label,
                        doc,
                        start,
                        end,
                    });
                }
                return None;
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
                if let Some(doc) = docs::command_doc(key) {
                    let label = docs::command_completion_label(key).unwrap_or(key);
                    return Some(CommandHit {
                        label,
                        doc,
                        start,
                        end,
                    });
                }
                return None;
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
                if let Some((label, doc)) = docs::command_entry(key, signed) {
                    return Some(CommandHit {
                        label,
                        doc,
                        start,
                        end,
                    });
                }
                return None;
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

#[derive(Debug, Clone, Copy)]
struct TokenSpan<'a> {
    token: &'a str,
    start: usize,
    end: usize,
}

fn token_at(line: &str, col: usize) -> Option<TokenSpan<'_>> {
    if col > line.len() {
        return None;
    }
    let bytes = line.as_bytes();
    let mut start = col;
    while start > 0 && !is_ws(bytes[start - 1]) {
        start -= 1;
    }
    let mut end = col;
    while end < line.len() && !is_ws(bytes[end]) {
        end += 1;
    }
    if start >= end {
        return None;
    }
    Some(TokenSpan {
        token: &line[start..end],
        start,
        end,
    })
}

fn is_ws(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t' | b'\r' | b'\n' | 0x0b | 0x0c)
}

fn single_quote_bounds(line: &str, col: usize) -> Option<(usize, usize)> {
    quote_bounds(line, col, b'\'')
}

fn double_quote_bounds(line: &str, col: usize) -> Option<(usize, usize)> {
    quote_bounds(line, col, b'"')
}

fn quote_bounds(line: &str, col: usize, quote: u8) -> Option<(usize, usize)> {
    let bytes = line.as_bytes();
    let mut last: Option<usize> = None;
    for (i, &b) in bytes.iter().enumerate() {
        if b == quote {
            match last {
                Some(open) => {
                    if col >= open && col <= i {
                        return Some((open, i));
                    }
                    last = None;
                }
                None => last = Some(i),
            }
        }
    }
    None
}

fn is_at_number(token: &str) -> bool {
    let bytes = token.as_bytes();
    bytes.first() == Some(&b'@')
        && bytes.len() > 1
        && bytes[1..].iter().all(|b| b.is_ascii_digit())
}

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

fn at_meta_lookup(token: &str) -> Option<(&'static str, &'static str)> {
    let label = if token.starts_with("@E") {
        "@E<num>"
    } else if token.starts_with("@M") {
        "@M<num>"
    } else if token.starts_with("@P") {
        "@P<num>"
    } else {
        "@<num>"
    };
    docs::at_meta_doc(label).map(|doc| (label, doc))
}

fn at_meta_in_token(token: &str, offset: usize) -> Option<(&'static str, &'static str, usize, usize)> {
    let prefix = token.get(..=offset)?;
    let at_pos = prefix.rfind('@')?;
    let rest = &token[at_pos..];

    let mut idx = 1;
    let label = if rest.len() > 1 && matches!(&rest[1..2], "E" | "M" | "P") {
        idx = 2;
        match &rest[1..2] {
            "E" => "@E<num>",
            "M" => "@M<num>",
            "P" => "@P<num>",
            _ => unreachable!(),
        }
    } else {
        "@<num>"
    };
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
        let doc = docs::at_meta_doc(label)?;
        return Some((label, doc, at_pos, at_end));
    }
    None
}

fn rate_offset_in_token(token: &str) -> Option<(&'static str, &'static str)> {
    if token.starts_with("rate=") {
        let label = docs::rate_offset_label("rate=").unwrap_or("rate=");
        return docs::rate_offset_doc("rate=").map(|doc| (label, doc));
    }
    if token.starts_with("offset=") {
        let label = docs::rate_offset_label("offset=").unwrap_or("offset=");
        return docs::rate_offset_doc("offset=").map(|doc| (label, doc));
    }
    None
}

fn platform_command_at_inner(
    line: &str,
    sq_start: usize,
    sq_end: usize,
) -> Option<(&'static str, &'static str)> {
    if sq_start + 1 >= sq_end {
        return None;
    }
    let inner = line.get(sq_start + 1..sq_end)?.trim_start();
    let cmd = inner.split_whitespace().next()?;
    let doc = docs::platform_command_doc(cmd)?;
    // Resolve to the canonical `'static` label from the docs table. Any
    // command that has a `doc` always has a `label`, so the unwrap is safe;
    // we use `?` to keep the type discipline tight without panicking.
    let label = docs::platform_command_label(cmd)?;
    Some((label, doc))
}

fn is_track_token(_line: &str, start: usize, token: &str) -> bool {
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

fn is_definition_line(line: &str, keyword: &str) -> bool {
    let trimmed = line.trim_start();
    if !trimmed.starts_with('@') {
        return false;
    }
    let bytes = trimmed.as_bytes();
    let mut idx = 1;
    while idx < bytes.len() && (bytes[idx] as char).is_ascii_digit() {
        idx += 1;
    }
    if idx == 1 {
        return false;
    }
    let rest = trimmed[idx..].trim_start();
    if !rest.starts_with(keyword) {
        return false;
    }
    let after = &rest[keyword.len()..];
    after.is_empty()
        || after.starts_with(|c: char| c.is_whitespace())
        || after.starts_with(';')
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

fn strip_two_op_numbers(line: &str, numbers: Vec<NumberSpan>, parse_end: usize) -> Vec<NumberSpan> {
    let mut filtered = strip_at_number(line, numbers);
    if let Some(pos) = line[..parse_end.min(line.len())].find("2op") {
        filtered.retain(|n| n.start < pos || n.start >= pos + 3);
    }
    filtered
}

/// Scan upward from `line_idx` for an `@N <keyword>` anchor. Handles both
/// single-line (`@1 fm`) and multi-line headers (`@1` alone, `fm` on the
/// next non-blank line).
fn find_instrument_anchor(text: &str, line_idx: usize, keyword: &str) -> Option<usize> {
    let lines: Vec<&str> = text.lines().collect();
    for idx in (0..=line_idx).rev() {
        let line = *lines.get(idx)?;
        let trimmed = line.trim_start();
        if trimmed.starts_with('@') {
            if is_definition_line(line, keyword) {
                return Some(idx);
            }
            if is_bare_at_number(trimmed) {
                let lookahead = (idx + 1).min(idx + 3);
                for j in (idx + 1)..=lookahead.min(lines.len().saturating_sub(1)) {
                    let next = lines.get(j)?.trim();
                    if next.is_empty() || next.starts_with(';') {
                        continue;
                    }
                    if let Some(after) = next.strip_prefix(keyword) {
                        if after.is_empty() || after.starts_with(|c: char| c.is_whitespace() || c == ';') {
                            return Some(idx);
                        }
                    }
                    break;
                }
            }
            return None;
        }
        if trimmed.starts_with('#') {
            return None;
        }
        if parse_leading_track_selector(line).is_some() {
            return None;
        }
    }
    None
}

fn is_bare_at_number(trimmed: &str) -> bool {
    if !trimmed.starts_with('@') {
        return false;
    }
    let bytes = trimmed.as_bytes();
    let mut idx = 1;
    while idx < bytes.len() && (bytes[idx] as char).is_ascii_digit() {
        idx += 1;
    }
    if idx == 1 {
        return false;
    }
    let rest = trimmed[idx..].trim_start();
    rest.is_empty() || rest.starts_with(';')
}

fn is_multiline_keyword_line(text: &str, line_idx: usize, keyword: &str) -> bool {
    let lines: Vec<&str> = text.lines().collect();
    let Some(line) = lines.get(line_idx) else {
        return false;
    };
    let trimmed = line.trim_start();
    if !trimmed.starts_with(keyword) {
        return false;
    }
    let after = &trimmed[keyword.len()..];
    if !after.is_empty() && !after.starts_with(|c: char| c.is_whitespace() || c == ';') {
        return false;
    }
    let lower_bound = line_idx.saturating_sub(3);
    for j in (lower_bound..line_idx).rev() {
        let prev = lines.get(j).map(|l| l.trim()).unwrap_or("");
        if prev.is_empty() || prev.starts_with(';') {
            continue;
        }
        return is_bare_at_number(prev);
    }
    false
}

fn fm_operator_index(text: &str, anchor: usize, line_idx: usize) -> Option<usize> {
    let lines: Vec<&str> = text.lines().collect();
    let mut count = 0usize;
    for idx in (anchor + 1)..=line_idx {
        let line = *lines.get(idx)?;
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

fn fm_hover(text: &str, line_idx: usize, col: usize) -> Option<Hit> {
    let line = nth_line(text, line_idx)?;
    let trimmed = line.trim_start();
    if trimmed.is_empty() || trimmed.starts_with(';') {
        return None;
    }

    let is_anchor = is_definition_line(line, "fm");
    let is_ml_keyword = !is_anchor && is_multiline_keyword_line(text, line_idx, "fm");
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
        let (label, doc) = docs::fm_param_doc(key)?;
        let n = numbers[idx];
        return Some(Hit {
            markdown: format_hover(label, doc),
            start: n.start,
            end: n.end,
        });
    }

    if !is_numeric_param_line(line, parse_end) {
        return None;
    }
    let anchor = find_instrument_anchor(text, line_idx.saturating_sub(1), "fm")?;
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
            fm_operator_index(text, anchor, line_idx),
        ),
        1 => (vec!["TRS"], None),
        _ => return None,
    };

    let idx = number_index_at(&numbers, col)?;
    let key = *param_keys.get(idx)?;
    let (label, doc) = docs::fm_param_doc(key)?;
    let n = numbers[idx];
    let final_label = if let Some(op) = op_index {
        format!("OP{} {}", op, label)
    } else {
        label.to_string()
    };
    Some(Hit {
        markdown: format_hover(&final_label, doc),
        start: n.start,
        end: n.end,
    })
}

fn two_op_hover(text: &str, line_idx: usize, col: usize) -> Option<Hit> {
    let line = nth_line(text, line_idx)?;
    let trimmed = line.trim_start();
    if trimmed.is_empty() || trimmed.starts_with(';') {
        return None;
    }
    if !is_definition_line(line, "2op") && !is_multiline_keyword_line(text, line_idx, "2op") {
        return None;
    }

    let parse_end = line.find(';').unwrap_or(line.len());
    if col >= parse_end {
        return None;
    }

    let numbers = strip_two_op_numbers(line, parse_numbers(line, parse_end), parse_end);
    if numbers.len() < 6 {
        return None;
    }
    let idx = number_index_at(&numbers, col)?;
    if idx >= 6 {
        return None;
    }
    let (label, doc) = docs::two_op_param_doc(idx)?;
    let n = numbers[idx];
    Some(Hit {
        markdown: format_hover(label, doc),
        start: n.start,
        end: n.end,
    })
}

fn psg_hover(text: &str, line_idx: usize, col: usize) -> Option<Hit> {
    let line = nth_line(text, line_idx)?;
    let trimmed = line.trim_start();
    if trimmed.is_empty() || trimmed.starts_with(';') {
        return None;
    }

    let mut in_psg_block = false;
    let mut data_start_col: usize = 0;

    if trimmed.starts_with('@') {
        if !is_definition_line(line, "psg") {
            return None;
        }
        in_psg_block = true;
        let line_offset = line.len() - trimmed.len();
        let psg_idx = trimmed.find("psg").unwrap_or(0);
        data_start_col = line_offset + psg_idx + 3;
    } else if is_multiline_keyword_line(text, line_idx, "psg") {
        in_psg_block = true;
        let line_offset = line.len() - trimmed.len();
        data_start_col = line_offset + 3;
    } else {
        if parse_leading_track_selector(line).is_some() {
            return None;
        }
        if find_instrument_anchor(text, line_idx.saturating_sub(1), "psg").is_some() {
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

    let tok = token_at(line, col)?;
    let mk = |label: &str, body: &str| Hit {
        markdown: format_hover(label, body),
        start: tok.start,
        end: tok.end,
    };

    if tok.token == "|" {
        return Some(mk(
            "PSG envelope loop position",
            "Set the loop point inside this PSG envelope. Playback jumps back here when the envelope reaches the end.",
        ));
    }
    if tok.token == "/" {
        return Some(mk(
            "PSG envelope sustain position",
            "Set the sustain point inside this PSG envelope. Sustain keeps the envelope around this point while the note is held.",
        ));
    }
    if is_l_colon_number(tok.token) {
        return Some(mk(
            "PSG envelope default step length",
            "Set the default length in frames for the following PSG envelope values on this definition line.",
        ));
    }
    if is_slide_token(tok.token) {
        return Some(mk(
            "PSG envelope slide",
            "Slide from one PSG volume value to another. `value>target:length` slides over the given number of frames.",
        ));
    }
    if is_hold_token(tok.token) {
        return Some(mk(
            "PSG envelope hold",
            "Keep this PSG volume value for the specified number of frames.",
        ));
    }
    if tok.token.chars().all(|c| c.is_ascii_digit()) && !tok.token.is_empty() {
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

fn nth_line(text: &str, idx: usize) -> Option<&str> {
    text.lines().nth(idx)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn span(info: &HoverInfo) -> (u32, u32) {
        (info.start, info.end)
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
    fn e_and_p_no_longer_resolve() {
        // The engine no-ops these; they should produce no hover.
        assert!(hover_at("A E5 c", 0, 2).is_none());
        assert!(hover_at("A P5 c", 0, 2).is_none());
    }
}
