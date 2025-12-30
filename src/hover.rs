use crate::docs;

pub(crate) fn hover_text(line: &str, col: usize) -> Option<String> {
    if line.is_empty() {
        return None;
    }

    if let Some((label, doc)) = platform_command_at(line, col) {
        return Some(format_hover(&label, doc));
    }

    if let Some((label, doc)) = at_meta_at(line, col) {
        return Some(format_hover(&label, doc));
    }

    if let Some((token, start, _end)) = token_at(line, col) {
        if is_track_token(line, start, token) {
            if let Some(doc) = docs::track_doc(token) {
                return Some(format_hover(token, doc));
            }
        }

        let offset = col.saturating_sub(start).min(token.len().saturating_sub(1));
        if let Some((label, doc)) = at_meta_in_token(token, offset) {
            return Some(format_hover(label, doc));
        }
        if let Some((label, doc)) = command_at(token, offset) {
            return Some(format_hover(label, doc));
        }

        if let Some(doc) = docs::meta_doc(token) {
            return Some(format_hover(token, doc));
        }
        if let Some(doc) = docs::platform_value_doc(token) {
            return Some(format_hover(token, doc));
        }
        if let Some(doc) = docs::instrument_doc(token) {
            return Some(format_hover(token, doc));
        }
        if let Some(doc) = docs::rate_offset_doc(token) {
            return Some(format_hover(token, doc));
        }
    }

    None
}

fn at_meta_at(line: &str, col: usize) -> Option<(&'static str, &'static str)> {
    let (token, _start, _end) = token_at(line, col)?;
    if !token.starts_with('@') {
        return None;
    }

    if token.starts_with("@E") {
        return docs::at_meta_doc("@E<num>").map(|doc| ("@E<num>", doc));
    }
    if token.starts_with("@M") {
        return docs::at_meta_doc("@M<num>").map(|doc| ("@M<num>", doc));
    }
    if token.starts_with("@P") {
        return docs::at_meta_doc("@P<num>").map(|doc| ("@P<num>", doc));
    }
    if token.len() > 1 && token[1..].chars().all(|c| c.is_ascii_digit()) {
        return docs::at_meta_doc("@<num>").map(|doc| ("@<num>", doc));
    }
    None
}

fn platform_command_at(line: &str, col: usize) -> Option<(String, &'static str)> {
    let (start, end) = single_quote_bounds(line, col)?;
    if start + 1 >= end {
        return None;
    }
    let inner = line.get(start + 1..end)?.trim_start();
    let cmd = inner.split_whitespace().next()?;
    docs::platform_command_doc(cmd).map(|doc| (cmd.to_string(), doc))
}

fn is_track_token(line: &str, start: usize, token: &str) -> bool {
    if token.starts_with('*') && token.len() > 1 && token[1..].chars().all(|c| c.is_ascii_digit()) {
        return true;
    }
    if !token.chars().all(|c| c.is_ascii_uppercase()) {
        return false;
    }
    let prefix = line.get(..start).unwrap_or("");
    prefix.trim().is_empty()
}

fn at_meta_in_token(token: &str, offset: usize) -> Option<(&'static str, &'static str)> {
    let prefix = token.get(..=offset)?;
    let at_pos = prefix.rfind('@')?;
    let slice = token.get(at_pos..)?;
    if offset < at_pos {
        return None;
    }

    let (label, digits_start) = if slice.starts_with("@E") {
        ("@E<num>", at_pos + 2)
    } else if slice.starts_with("@M") {
        ("@M<num>", at_pos + 2)
    } else if slice.starts_with("@P") {
        ("@P<num>", at_pos + 2)
    } else {
        ("@<num>", at_pos + 1)
    };

    let mut end = digits_start;
    while end < token.len() {
        let ch = token.as_bytes()[end] as char;
        if ch.is_ascii_digit() {
            end += 1;
        } else {
            break;
        }
    }

    if end == digits_start {
        return None;
    }

    if offset >= digits_start && offset < end {
        return docs::at_meta_doc(label).map(|doc| (label, doc));
    }

    None
}

fn command_at(token: &str, offset: usize) -> Option<(&'static str, &'static str)> {
    let bytes = token.as_bytes();
    let mut idx = 0;
    while idx < bytes.len() {
        let ch = bytes[idx] as char;
        if is_command_char(ch) {
            let start = idx;
            let mut end = idx + 1;
            while end < bytes.len() {
                let next = bytes[end] as char;
                if next.is_ascii_digit() {
                    end += 1;
                } else {
                    break;
                }
            }
            if offset >= start && offset < end {
                let label = command_label(ch)?;
                return docs::command_doc(label).map(|doc| (label, doc));
            }
            idx = end;
            continue;
        }
        idx += 1;
    }
    None
}

fn command_label(ch: char) -> Option<&'static str> {
    match ch {
        'o' => Some("o"),
        'l' => Some("l"),
        'Q' => Some("Q"),
        'q' => Some("q"),
        'C' => Some("C"),
        'R' => Some("R"),
        'L' => Some("L"),
        's' => Some("s"),
        't' => Some("t"),
        'T' => Some("T"),
        'v' => Some("v"),
        'V' => Some("V"),
        'p' => Some("p"),
        'k' => Some("k"),
        'K' => Some("K"),
        'E' => Some("E"),
        'M' => Some("M"),
        'P' => Some("P"),
        'G' => Some("G"),
        'D' => Some("D"),
        'r' => Some("r"),
        '^' => Some("^"),
        '&' => Some("&"),
        _ => None,
    }
}

fn is_command_char(ch: char) -> bool {
    matches!(
        ch,
        'o' | 'l' | 'Q' | 'q' | 'C' | 'R' | 'L' | 's' | 't' | 'T' | 'v' | 'V' | 'p' | 'k' | 'K'
            | 'E' | 'M' | 'P' | 'G' | 'D' | 'r' | '^' | '&'
    )
}

fn single_quote_bounds(line: &str, col: usize) -> Option<(usize, usize)> {
    let bytes = line.as_bytes();
    let mut left = col.min(line.len().saturating_sub(1));
    while left > 0 && bytes[left] != b'\'' {
        left = left.saturating_sub(1);
    }
    if bytes[left] != b'\'' {
        return None;
    }

    let mut right = col.min(line.len().saturating_sub(1));
    while right < line.len() && bytes[right] != b'\'' {
        right += 1;
    }
    if right >= line.len() || bytes[right] != b'\'' {
        return None;
    }
    if left >= right {
        return None;
    }
    Some((left, right))
}

fn token_at(line: &str, col: usize) -> Option<(&str, usize, usize)> {
    let bytes = line.as_bytes();
    if bytes.is_empty() {
        return None;
    }
    let mut idx = col.min(line.len().saturating_sub(1));
    while idx > 0 && bytes[idx].is_ascii_whitespace() {
        idx -= 1;
    }

    let ch = bytes[idx] as char;
    if ch == '^' || ch == '&' {
        return Some((&line[idx..idx + 1], idx, idx + 1));
    }

    let mut start = idx;
    while start > 0 && is_token_char(bytes[start - 1] as char) {
        start -= 1;
    }
    let mut end = idx + 1;
    while end < line.len() && is_token_char(bytes[end] as char) {
        end += 1;
    }
    if start == end {
        return None;
    }
    Some((&line[start..end], start, end))
}

fn is_token_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || matches!(ch, '#' | '@' | '_' | '=' | '*')
}

fn format_hover(label: &str, doc: &str) -> String {
    format!("**{}**\n\n{}", label, doc)
}
