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

    if let Some(hover) = pcm_sample_path_hover(line, col) {
        return Some(hover);
    }

    if let Some((token, start, _end)) = token_at(line, col) {
        if is_track_token(line, start, token) {
            if let Some(doc) = docs::track_doc(token) {
                return Some(format_hover(token, doc));
            }
        }

        if let Some((_meta_start, meta_end, meta_keyword)) = meta_keyword_bounds(line) {
            if col >= meta_end && start >= meta_end {
                if meta_keyword == "#platform" {
                    if let Some(doc) = docs::platform_value_doc(token) {
                        return Some(format_hover(token, doc));
                    }
                } else if meta_keyword == "#option" {
                    if let Some(doc) = docs::option_value_doc(token) {
                        return Some(format_hover(token, doc));
                    }
                }
                return None;
            }
        }

        if token.starts_with('#') {
            if let Some(doc) = docs::meta_doc(token) {
                return Some(format_hover(token, doc));
            }
            return None;
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
pub(crate) fn two_op_hover_text(text: &str, line_index: u32, col: usize) -> Option<String> {
    let line = text.lines().nth(line_index as usize)?;
    let trimmed = line.trim_start();
    if trimmed.is_empty() || trimmed.starts_with(';') {
        return None;
    }
    if !is_two_op_definition_line(trimmed) {
        return None;
    }

    let parse_end = line.find(';').unwrap_or(line.len());
    if col >= parse_end {
        return None;
    }

    let mut numbers = parse_numbers(line, parse_end);
    numbers = strip_two_op_numbers(line, numbers, parse_end);
    if numbers.len() < 6 {
        return None;
    }

    let idx = number_index_at(&numbers, col)?;
    if idx >= 6 {
        return None;
    }
    let (label, doc) = docs::two_op_param_doc(idx)?;
    Some(format_hover(label, doc))
}

fn is_two_op_definition_line(line: &str) -> bool {
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
    if !rest.starts_with("2op") {
        return false;
    }
    let after = &rest[3..];
    after.is_empty() || after.starts_with(|c: char| c.is_whitespace()) || after.starts_with(';')
}

fn strip_at_number(line: &str, numbers: Vec<(usize, usize, i64)>) -> Vec<(usize, usize, i64)> {
    if let Some((start, _end, _value)) = numbers.first() {
        if *start > 0 && line.as_bytes()[start - 1] == b'@' {
            return numbers.into_iter().skip(1).collect();
        }
    }
    numbers
}

fn strip_two_op_numbers(
    line: &str,
    numbers: Vec<(usize, usize, i64)>,
    parse_end: usize,
) -> Vec<(usize, usize, i64)> {
    let mut filtered = numbers;
    if let Some((start, _end, _value)) = filtered.first() {
        if *start > 0 && line.as_bytes()[start - 1] == b'@' {
            filtered = filtered.into_iter().skip(1).collect();
        }
    }

    if let Some(pos) = line[..parse_end.min(line.len())].find("2op") {
        filtered.retain(|(start, _end, _value)| *start < pos || *start >= pos + 3);
    }

    filtered
}


pub(crate) fn fm_hover_text(text: &str, line_index: u32, col: usize) -> Option<String> {
    let line = text.lines().nth(line_index as usize)?;
    let trimmed = line.trim_start();
    if trimmed.is_empty() || trimmed.starts_with(';') {
        return None;
    }

    let is_anchor_line = is_fm_definition_line(trimmed);
    if trimmed.starts_with('@') && !is_anchor_line {
        return None;
    }

    let parse_end = line.find(';').unwrap_or(line.len());
    if col >= parse_end {
        return None;
    }

    if is_anchor_line {
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
        return Some(format_hover(label, doc));
    }

    let anchor = find_fm_anchor(text, line_index)?;
    if line_index <= anchor {
        return None;
    }

    let numbers = parse_numbers(line, parse_end);
    if numbers.is_empty() {
        return None;
    }

    let (param_keys, op_index) = match numbers.len() {
        2 => (vec!["ALG", "FB"], None),
        10 => (
            vec!["AR", "DR", "SR", "RR", "SL", "TL", "KS", "ML", "DT", "SSG"],
            fm_operator_index(text, anchor, line_index),
        ),
        1 => (vec!["TRS"], None),
        _ => return None,
    };

    let idx = number_index_at(&numbers, col)?;
    let key = param_keys.get(idx)?;
    let (label, doc) = docs::fm_param_doc(key)?;
    let label = if let Some(op) = op_index {
        format!("OP{} {}", op, label)
    } else {
        label.to_string()
    };
    Some(format_hover(&label, doc))
}

fn find_fm_anchor(text: &str, line_index: u32) -> Option<u32> {
    for idx in (0..=line_index).rev() {
        let line = text.lines().nth(idx as usize)?;
        let trimmed = line.trim_start();
        if trimmed.starts_with('@') {
            if is_fm_definition_line(trimmed) {
                return Some(idx);
            }
            return None;
        }
    }
    None
}

fn is_fm_definition_line(line: &str) -> bool {
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
    if !rest.starts_with("fm") {
        return false;
    }
    let after = &rest[2..];
    after.is_empty() || after.starts_with(|c: char| c.is_whitespace()) || after.starts_with(';')
}

fn fm_operator_index(text: &str, anchor: u32, line_index: u32) -> Option<usize> {
    let mut count = 0;
    for idx in (anchor + 1)..=line_index {
        let line = text.lines().nth(idx as usize)?;
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
    if count == 0 { None } else { Some(count) }
}

fn parse_numbers(line: &str, parse_end: usize) -> Vec<(usize, usize, i64)> {
    let mut out = Vec::new();
    let bytes = line.as_bytes();
    let mut idx = 0;
    let end = parse_end.min(line.len());

    while idx < end {
        while idx < end && bytes[idx].is_ascii_whitespace() {
            idx += 1;
        }
        if idx >= end {
            break;
        }

        let start = idx;
        let mut sign = 1i64;
        if bytes[idx] == b'+' || bytes[idx] == b'-' {
            if bytes[idx] == b'-' {
                sign = -1;
            }
            idx += 1;
        }
        if idx >= end || !bytes[idx].is_ascii_digit() {
            idx += 1;
            continue;
        }
        let num_start = idx;
        while idx < end && bytes[idx].is_ascii_digit() {
            idx += 1;
        }
        let num_str = &line[num_start..idx];
        let value = num_str.parse::<i64>().unwrap_or(0) * sign;
        out.push((start, idx, value));
    }

    out
}

fn number_index_at(numbers: &[(usize, usize, i64)], col: usize) -> Option<usize> {
    numbers.iter().enumerate().find_map(|(idx, (start, end, _value))| {
        if col >= *start && col < *end {
            return Some(idx);
        }
        if col == *end && col > *start {
            return Some(idx);
        }
        None
    })
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

fn pcm_sample_path_hover(line: &str, col: usize) -> Option<String> {
    let (start, end) = double_quote_bounds(line, col)?;
    if start + 1 >= end {
        return None;
    }
    let prefix = line.get(..start)?.trim_end();
    let tokens: Vec<&str> = prefix.split_whitespace().collect();
    let pcm_pos = tokens.iter().position(|token| *token == "pcm")?;
    if pcm_pos == 0 {
        return None;
    }
    let at_token = tokens[pcm_pos - 1];
    if !is_at_number(at_token) {
        return None;
    }
    let mut path = line.get(start + 1..end)?.to_string();
    if path.is_empty() {
        return None;
    }
    path = path.replace('`', "\\`");
    Some(format!(
        "**PCM sample path**\n\nRelative to the MML file: `{}`",
        path
    ))
}

fn is_at_number(token: &str) -> bool {
    token.len() > 1 && token.starts_with('@') && token[1..].chars().all(|c| c.is_ascii_digit())
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
        if ch == '_' {
            let start = idx;
            let mut end = idx + 1;
            let mut key = "_";

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
                while end < bytes.len() {
                    let next = bytes[end] as char;
                    if next.is_ascii_digit() {
                        end += 1;
                    } else {
                        break;
                    }
                }
            }

            if offset >= start && offset < end {
                let label = docs::command_completion_label(key).unwrap_or(key);
                return docs::command_doc(key)
                    .or_else(|| docs::platform_command_doc(key))
                    .map(|doc| (label, doc));
            }
            idx = end;
            continue;
        }
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
                let key = command_label(ch)?;
                let label = docs::command_completion_label(key).unwrap_or(key);
                return docs::command_doc(key)
                    .or_else(|| docs::platform_command_doc(key))
                    .map(|doc| (label, doc));
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

fn double_quote_bounds(line: &str, col: usize) -> Option<(usize, usize)> {
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
    ch.is_ascii_alphanumeric() || matches!(ch, '#' | '@' | '_' | '=' | '*' | '-' | '+' | '{' | '}')
}

fn format_hover(label: &str, doc: &str) -> String {
    let link = "[mml_ref.md](https://github.com/superctr/ctrmml/blob/master/mml_ref.md)";
    if doc.is_empty() {
        format!("**{}**\n\n{}", label, link)
    } else {
        format!("**{}**\n\n{}\n\n{}", label, doc, link)
    }
}

fn meta_keyword_bounds(line: &str) -> Option<(usize, usize, &str)> {
    let bytes = line.as_bytes();
    let mut start = 0;
    while start < bytes.len() && bytes[start].is_ascii_whitespace() {
        start += 1;
    }
    if start >= bytes.len() || bytes[start] != b'#' {
        return None;
    }
    let mut end = start + 1;
    while end < bytes.len() && !bytes[end].is_ascii_whitespace() {
        end += 1;
    }
    Some((start, end, &line[start..end]))
}
