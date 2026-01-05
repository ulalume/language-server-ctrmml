pub(crate) fn token_at(line: &str, col: usize) -> Option<(&str, usize, usize)> {
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

pub(crate) fn is_at_number(token: &str) -> bool {
    let mut chars = token.chars();
    if chars.next() != Some('@') {
        return false;
    }
    let rest: String = chars.collect();
    !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit())
}

pub(crate) fn single_quote_bounds(line: &str, col: usize) -> Option<(usize, usize)> {
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

pub(crate) fn double_quote_bounds(line: &str, col: usize) -> Option<(usize, usize)> {
    let bytes = line.as_bytes();
    let mut left = col.min(line.len().saturating_sub(1));
    while left > 0 && bytes[left] != b'"' {
        left = left.saturating_sub(1);
    }
    if bytes[left] != b'"' {
        return None;
    }

    let mut right = col.min(line.len().saturating_sub(1));
    while right < line.len() && bytes[right] != b'"' {
        right += 1;
    }
    if right >= line.len() || bytes[right] != b'"' {
        return None;
    }
    if left >= right {
        return None;
    }
    Some((left, right))
}

fn is_token_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || matches!(ch, '#' | '@' | '_' | '=' | '*' | '-' | '+' | '{' | '}')
}
