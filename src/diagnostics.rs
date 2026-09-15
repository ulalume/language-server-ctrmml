use std::collections::HashMap;
use std::sync::Arc;

use serde::Deserialize;
use tokio::sync::Mutex;
use tower_lsp::lsp_types::{Diagnostic, DiagnosticSeverity, NumberOrString, Position, Range, Url};
use tower_lsp::Client;

#[derive(Deserialize)]
#[serde(tag = "type")]
pub(crate) enum PlaybackMessage {
    #[serde(rename = "highlight")]
    Highlight {
        #[allow(dead_code)]
        ticks: u32,
        positions: Vec<HighlightPosition>,
    },
    #[serde(rename = "playback_error")]
    PlaybackError { message: String },
}

#[derive(Deserialize)]
pub(crate) struct HighlightPosition {
    pub(crate) line: u32,
    pub(crate) col: u32,
}

#[derive(Deserialize)]
pub(crate) struct CheckReport {
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

pub(crate) fn diagnostic_for_playback_error(message: String) -> Diagnostic {
    Diagnostic {
        range: Range {
            start: Position::new(0, 0),
            end: Position::new(0, 0),
        },
        severity: Some(DiagnosticSeverity::ERROR),
        source: Some("ctrmml-playback".to_string()),
        message,
        ..Diagnostic::default()
    }
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
    let end = if line_len == 0 {
        col
    } else {
        (col + 1).min(line_len)
    };
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
        return Some((
            line_num.saturating_sub(1),
            col_num.saturating_sub(1),
            message,
        ));
    }

    None
}

/// Per-URI diagnostic layers. `publishDiagnostics` replaces the whole set
/// for a URI, so check results and playback markers are kept apart here
/// and published merged.
#[derive(Default)]
pub(crate) struct DiagnosticStore {
    entries: HashMap<String, DiagnosticLayers>,
}

#[derive(Default)]
struct DiagnosticLayers {
    check: Vec<Diagnostic>,
    playback: Vec<Diagnostic>,
    playback_expires_on_check: bool,
}

impl DiagnosticLayers {
    fn merged(&self) -> Vec<Diagnostic> {
        self.check
            .iter()
            .chain(self.playback.iter())
            .cloned()
            .collect()
    }

    fn is_empty(&self) -> bool {
        self.check.is_empty() && self.playback.is_empty()
    }
}

impl DiagnosticStore {
    /// Replace the check layer, returning the set to publish. Drops a
    /// playback layer that a finished playback marked as expiring.
    pub(crate) fn set_check(&mut self, uri: &str, diagnostics: Vec<Diagnostic>) -> Vec<Diagnostic> {
        self.update(uri, |layers| {
            if layers.playback_expires_on_check {
                layers.playback.clear();
                layers.playback_expires_on_check = false;
            }
            layers.check = diagnostics;
        })
    }

    /// Replace the playback layer, returning the set to publish.
    pub(crate) fn set_playback(
        &mut self,
        uri: &str,
        diagnostics: Vec<Diagnostic>,
    ) -> Vec<Diagnostic> {
        self.update(uri, |layers| {
            layers.playback = diagnostics;
            layers.playback_expires_on_check = false;
        })
    }

    /// Drop the playback layer, returning the set to publish.
    pub(crate) fn clear_playback(&mut self, uri: &str) -> Vec<Diagnostic> {
        self.update(uri, |layers| {
            layers.playback.clear();
            layers.playback_expires_on_check = false;
        })
    }

    /// Leave the playback layer standing until the next check replaces it.
    pub(crate) fn expire_playback_on_check(&mut self, uri: &str) {
        if let Some(layers) = self.entries.get_mut(&store_key(uri)) {
            layers.playback_expires_on_check = true;
        }
    }

    fn update(&mut self, uri: &str, apply: impl FnOnce(&mut DiagnosticLayers)) -> Vec<Diagnostic> {
        let key = store_key(uri);
        let layers = self.entries.entry(key.clone()).or_default();
        apply(layers);
        let merged = layers.merged();
        if layers.is_empty() {
            self.entries.remove(&key);
        }
        merged
    }
}

/// Clients spell the same document differently (raw command arguments vs
/// `Url::to_string`), and separate keys would split the layers.
fn store_key(uri: &str) -> String {
    Url::parse(uri)
        .map(|url| url.to_string())
        .unwrap_or_else(|_| uri.to_string())
}

pub(crate) type DiagnosticStoreHandle = Arc<Mutex<DiagnosticStore>>;

pub(crate) async fn publish_check_diagnostics(
    client: &Client,
    store: &DiagnosticStoreHandle,
    uri: &str,
    diagnostics: Vec<Diagnostic>,
) {
    let mut guard = store.lock().await;
    let merged = guard.set_check(uri, diagnostics);
    publish(client, uri, merged).await;
}

pub(crate) async fn publish_playback_diagnostics(
    client: &Client,
    store: &DiagnosticStoreHandle,
    uri: &str,
    diagnostics: Vec<Diagnostic>,
) {
    let mut guard = store.lock().await;
    let merged = guard.set_playback(uri, diagnostics);
    publish(client, uri, merged).await;
}

pub(crate) async fn clear_playback_diagnostics(
    client: &Client,
    store: &DiagnosticStoreHandle,
    uri: &str,
) {
    let mut guard = store.lock().await;
    let merged = guard.clear_playback(uri);
    publish(client, uri, merged).await;
}

pub(crate) async fn expire_playback_diagnostics_on_check(store: &DiagnosticStoreHandle, uri: &str) {
    store.lock().await.expire_playback_on_check(uri);
}

/// Callers hold the store lock across this await so publishes reach the
/// client in the order their layers were computed.
async fn publish(client: &Client, uri: &str, diagnostics: Vec<Diagnostic>) {
    if let Ok(parsed) = uri.parse() {
        client.publish_diagnostics(parsed, diagnostics, None).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check_error(message: &str) -> Diagnostic {
        Diagnostic {
            severity: Some(DiagnosticSeverity::ERROR),
            source: Some("ctrmml-check".to_string()),
            message: message.to_string(),
            ..Diagnostic::default()
        }
    }

    fn playback_hint() -> Diagnostic {
        Diagnostic {
            severity: Some(DiagnosticSeverity::HINT),
            source: Some("ctrmml-playback".to_string()),
            message: "playback".to_string(),
            ..Diagnostic::default()
        }
    }

    #[test]
    fn playback_layer_is_published_on_top_of_check_layer() {
        let mut store = DiagnosticStore::default();
        store.set_check("file:///a.mml", vec![check_error("missing sample")]);

        let published = store.set_playback("file:///a.mml", vec![playback_hint()]);

        let sources: Vec<_> = published
            .iter()
            .map(|diag| diag.source.as_deref().unwrap_or_default())
            .collect();
        assert_eq!(sources, ["ctrmml-check", "ctrmml-playback"]);
    }

    #[test]
    fn clearing_playback_keeps_check_diagnostics() {
        let mut store = DiagnosticStore::default();
        store.set_check("file:///a.mml", vec![check_error("missing sample")]);
        store.set_playback("file:///a.mml", vec![playback_hint()]);

        let published = store.clear_playback("file:///a.mml");

        assert_eq!(published.len(), 1);
        assert_eq!(published[0].message, "missing sample");
    }

    #[test]
    fn playback_error_keeps_check_diagnostics() {
        let mut store = DiagnosticStore::default();
        store.set_check("file:///a.mml", vec![check_error("missing sample")]);

        let published = store.set_playback(
            "file:///a.mml",
            vec![diagnostic_for_playback_error("Playback error".to_string())],
        );

        assert_eq!(published.len(), 2);
        assert_eq!(published[0].message, "missing sample");
        assert_eq!(published[1].message, "Playback error");
    }

    #[test]
    fn check_update_keeps_running_playback_markers() {
        let mut store = DiagnosticStore::default();
        store.set_playback("file:///a.mml", vec![playback_hint()]);

        let published = store.set_check("file:///a.mml", vec![check_error("missing sample")]);

        assert_eq!(published.len(), 2);
        assert_eq!(published[1].source.as_deref(), Some("ctrmml-playback"));
    }

    #[test]
    fn standing_playback_error_expires_on_next_check() {
        let mut store = DiagnosticStore::default();
        store.set_playback(
            "file:///a.mml",
            vec![diagnostic_for_playback_error("Playback error".to_string())],
        );
        store.expire_playback_on_check("file:///a.mml");

        let published = store.set_check("file:///a.mml", vec![check_error("missing sample")]);

        assert_eq!(published.len(), 1);
        assert_eq!(published[0].message, "missing sample");
    }

    #[test]
    fn a_new_playback_layer_is_not_expired_by_the_next_check() {
        let mut store = DiagnosticStore::default();
        store.set_playback(
            "file:///a.mml",
            vec![diagnostic_for_playback_error("Playback error".to_string())],
        );
        store.expire_playback_on_check("file:///a.mml");
        store.set_playback("file:///a.mml", vec![playback_hint()]);

        let published = store.set_check("file:///a.mml", vec![check_error("missing sample")]);

        assert_eq!(published.len(), 2);
        assert_eq!(published[1].source.as_deref(), Some("ctrmml-playback"));
    }

    #[test]
    fn layers_share_one_entry_across_uri_spellings() {
        let mut store = DiagnosticStore::default();
        store.set_check("file:///tmp/a b.mml", vec![check_error("missing sample")]);

        let published = store.set_playback("file:///tmp/a%20b.mml", vec![playback_hint()]);

        assert_eq!(published.len(), 2);
        assert_eq!(published[0].source.as_deref(), Some("ctrmml-check"));
        assert_eq!(store.entries.len(), 1);
    }

    #[test]
    fn empty_layers_drop_the_uri_entry() {
        let mut store = DiagnosticStore::default();
        store.set_check("file:///a.mml", vec![check_error("missing sample")]);
        store.set_playback("file:///a.mml", vec![playback_hint()]);

        store.set_check("file:///a.mml", Vec::new());
        let published = store.clear_playback("file:///a.mml");

        assert!(published.is_empty());
        assert!(store.entries.is_empty());
    }

    #[test]
    fn playback_error_diagnostic_uses_document_start_and_playback_source() {
        let diagnostic = diagnostic_for_playback_error("unsupported playback mode".to_string());

        assert_eq!(diagnostic.range.start, Position::new(0, 0));
        assert_eq!(diagnostic.range.end, Position::new(0, 0));
        assert_eq!(diagnostic.severity, Some(DiagnosticSeverity::ERROR));
        assert_eq!(diagnostic.source.as_deref(), Some("ctrmml-playback"));
        assert_eq!(diagnostic.message, "unsupported playback mode");
    }
}
