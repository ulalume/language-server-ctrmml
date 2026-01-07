use serde::Deserialize;
use tower_lsp::lsp_types::{Diagnostic, DiagnosticSeverity, NumberOrString, Position, Range};

#[derive(Deserialize)]
pub(crate) struct HighlightMessage {
    #[serde(rename = "type")]
    pub(crate) kind: String,
    #[allow(dead_code)]
    pub(crate) ticks: u32,
    pub(crate) positions: Vec<HighlightPosition>,
}

#[derive(Deserialize)]
pub(crate) struct HighlightPosition {
    pub(crate) line: u32,
    pub(crate) col: u32,
}

#[derive(Deserialize)]
pub(crate) struct CheckReport {
    #[serde(default)]
    pub(crate) ok: bool,
    #[serde(default)]
    pub(crate) errors: Vec<CheckMessage>,
    #[serde(default)]
    pub(crate) warnings: Vec<CheckMessage>,
}

#[derive(Deserialize)]
pub(crate) struct CheckMessage {
    #[serde(default)]
    pub(crate) message: String,
    #[serde(default)]
    pub(crate) path: String,
    #[serde(default)]
    pub(crate) line: u32,
    #[serde(default)]
    pub(crate) col: u32,
    #[serde(default)]
    pub(crate) length: u32,
    #[serde(default)]
    pub(crate) code: String,
}

pub(crate) fn diagnostics_for_positions(
    text: &str,
    positions: &[HighlightPosition],
) -> Vec<Diagnostic> {
    let lines: Vec<&str> = text.lines().collect();
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();

    for pos in positions {
        let line = pos.line as usize;
        if line >= lines.len() {
            continue;
        }
        let line_len = lines[line].len() as u32;
        let mut col = pos.col;
        if col > line_len {
            col = line_len;
        }
        let end = (col + 1).min(line_len);
        let key = (pos.line as u64) << 32 | pos.col as u64;
        if !seen.insert(key) {
            continue;
        }
        out.push(Diagnostic {
            range: Range {
                start: Position::new(pos.line, col),
                end: Position::new(pos.line, end),
            },
            severity: Some(DiagnosticSeverity::HINT),
            source: Some("ctrmml-playback".to_string()),
            message: "playback".to_string(),
            ..Diagnostic::default()
        });
    }

    out
}

pub(crate) fn diagnostics_for_check_report(text: &str, report: &CheckReport) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    for msg in &report.errors {
        if let Some(diag) = diagnostic_for_check_message(text, msg, DiagnosticSeverity::ERROR) {
            out.push(diag);
        }
    }
    for msg in &report.warnings {
        if let Some(diag) = diagnostic_for_check_message(text, msg, DiagnosticSeverity::WARNING) {
            out.push(diag);
        }
    }
    out
}

fn diagnostic_for_check_message(
    text: &str,
    message: &CheckMessage,
    severity: DiagnosticSeverity,
) -> Option<Diagnostic> {
    if message.message.trim().is_empty() {
        return None;
    }
    let lines: Vec<&str> = text.lines().collect();
    let line_idx = message.line.saturating_sub(1);
    let mut col = message.col.saturating_sub(1);
    let line_len = lines
        .get(line_idx as usize)
        .map(|line| line.len() as u32)
        .unwrap_or(0);
    if col > line_len {
        col = line_len;
    }
    let length = message.length;
    let end = if line_len == 0 {
        col
    } else if length > 0 {
        (col + length).min(line_len)
    } else {
        (col + 1).min(line_len)
    };
    let code = if message.code.trim().is_empty() {
        None
    } else {
        Some(NumberOrString::String(message.code.clone()))
    };
    Some(Diagnostic {
        range: Range {
            start: Position::new(line_idx, col),
            end: Position::new(line_idx, end),
        },
        severity: Some(severity),
        source: Some("ctrmml-check".to_string()),
        message: message.message.clone(),
        code,
        ..Diagnostic::default()
    })
}


pub(crate) fn diagnostic_for_check(text: &str, output: &str) -> Option<Diagnostic> {
    let (line_idx, col_idx, message) = output
        .lines()
        .filter_map(|line| {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                return None;
            }
            parse_error_line(trimmed)
        })
        .next()?;
    let lines: Vec<&str> = text.lines().collect();
    let mut col = col_idx;
    let line_len = lines
        .get(line_idx as usize)
        .map(|line| line.len() as u32)
        .unwrap_or(0);
    if col > line_len {
        col = line_len;
    }
    let end = if line_len == 0 { col } else { (col + 1).min(line_len) };
    Some(Diagnostic {
        range: Range {
            start: Position::new(line_idx, col),
            end: Position::new(line_idx, end),
        },
        severity: Some(DiagnosticSeverity::ERROR),
        source: Some("ctrmml-check".to_string()),
        message,
        ..Diagnostic::default()
    })
}

fn parse_error_line(line: &str) -> Option<(u32, u32, String)> {
    let line = line.strip_prefix("Playback error: ").unwrap_or(line).trim();
    if let Some(rest) = line.strip_prefix("line ") {
        let mut parts = rest.splitn(2, ':');
        let line_str = parts.next()?.trim();
        let message = parts.next()?.trim_start();
        let line_num: u32 = line_str.parse().ok()?;
        return Some((line_num.saturating_sub(1), 0, message.to_string()));
    }

    let parts: Vec<&str> = line.split(':').collect();
    if parts.len() < 3 {
        return None;
    }

    for idx in (1..parts.len() - 1).rev() {
        let col_str = parts[idx].trim();
        if col_str.is_empty() || !col_str.chars().all(|c| c.is_ascii_digit()) {
            continue;
        }
        let line_str = parts[idx - 1].trim();
        if line_str.is_empty() || !line_str.chars().all(|c| c.is_ascii_digit()) {
            continue;
        }
        let line_num: u32 = line_str.parse().ok()?;
        let col_num: u32 = col_str.parse().ok()?;
        let mut message = parts[idx + 1..].join(":").trim_start().to_string();
        if let Some(stripped) = message.strip_suffix("(ctrmml-check)") {
            message = stripped.trim_end().to_string();
        }
        return Some((line_num.saturating_sub(1), col_num.saturating_sub(1), message));
    }

    None
}
