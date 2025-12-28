use serde::Deserialize;
use tower_lsp::lsp_types::{Diagnostic, DiagnosticSeverity, Position, Range};

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
