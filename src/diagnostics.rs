use std::collections::HashMap;
use std::sync::Arc;

use ctrmml_lang_core::byte_offset_to_utf16_character;
use serde::Deserialize;
use tokio::sync::Mutex;
use tower_lsp::lsp_types::{Diagnostic, DiagnosticSeverity, NumberOrString, Position, Range, Url};
use tower_lsp::Client;

#[derive(Deserialize)]
#[serde(tag = "type")]
pub(crate) enum PlaybackMessage {
    #[serde(rename = "highlight")]
    Highlight { positions: Vec<HighlightPosition> },
    #[serde(rename = "playback_error")]
    PlaybackError { message: String },
}

#[derive(Clone, PartialEq, Eq, Deserialize)]
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
    let lines = split_lines(text);
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();

    for pos in positions {
        let Some(line) = lines.get(pos.line as usize) else {
            continue;
        };
        if !seen.insert((pos.line, pos.col)) {
            continue;
        }
        let start = floor_char_boundary(line, pos.col as usize);
        let end = ceil_char_boundary(line, start + 1);
        out.push(Diagnostic {
            range: utf16_range(line, pos.line, start, end),
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
    let lines = split_lines(text);
    let errors = report
        .errors
        .iter()
        .map(|msg| (msg, DiagnosticSeverity::ERROR));
    let warnings = report
        .warnings
        .iter()
        .map(|msg| (msg, DiagnosticSeverity::WARNING));
    errors
        .chain(warnings)
        .filter_map(|(msg, severity)| diagnostic_for_check_message(&lines, msg, severity))
        .collect()
}

/// `message` carries a 1-based line and byte column and a byte length.
fn diagnostic_for_check_message(
    lines: &[&str],
    message: &CheckMessage,
    severity: DiagnosticSeverity,
) -> Option<Diagnostic> {
    if message.message.trim().is_empty() {
        return None;
    }
    let line_idx = message.line.saturating_sub(1);
    let line = lines.get(line_idx as usize).copied().unwrap_or("");
    let start = floor_char_boundary(line, message.col.saturating_sub(1) as usize);
    let end = if line.is_empty() {
        start
    } else {
        ceil_char_boundary(line, start + message.length.max(1) as usize)
    };
    let code = if message.code.trim().is_empty() {
        None
    } else {
        Some(NumberOrString::String(message.code.clone()))
    };
    Some(Diagnostic {
        range: utf16_range(line, line_idx, start, end),
        severity: Some(severity),
        source: Some("ctrmml-check".to_string()),
        message: message.message.clone(),
        code,
        ..Diagnostic::default()
    })
}

fn split_lines(text: &str) -> Vec<&str> {
    text.split('\n')
        .map(|line| line.strip_suffix('\r').unwrap_or(line))
        .collect()
}

fn floor_char_boundary(line: &str, index: usize) -> usize {
    let mut index = index.min(line.len());
    while !line.is_char_boundary(index) {
        index -= 1;
    }
    index
}

fn ceil_char_boundary(line: &str, index: usize) -> usize {
    let mut index = index.min(line.len());
    while !line.is_char_boundary(index) {
        index += 1;
    }
    index
}

/// An LSP range for the byte span `start..end` of `line`.
fn utf16_range(line: &str, line_idx: u32, start: usize, end: usize) -> Range {
    Range {
        start: Position::new(line_idx, byte_offset_to_utf16_character(line, start)),
        end: Position::new(line_idx, byte_offset_to_utf16_character(line, end)),
    }
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
pub(crate) async fn publish(client: &Client, uri: &str, diagnostics: Vec<Diagnostic>) {
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

    fn check_message(line: u32, col: u32, length: u32) -> CheckMessage {
        CheckMessage {
            message: "bad".to_string(),
            line,
            col,
            length,
            code: String::new(),
        }
    }

    #[test]
    fn check_ranges_count_utf16_units_after_japanese_text() {
        // `あい` is two UTF-16 units and six bytes; `c` is byte 9, 1-based col 10.
        let report = CheckReport {
            errors: vec![check_message(2, 10, 2)],
            warnings: vec![check_message(1, 1, 0)],
        };
        let diagnostics = diagnostics_for_check_report("; x\nA あい cd e\n", &report);
        assert_eq!(diagnostics.len(), 2);
        assert_eq!(diagnostics[0].range.start, Position::new(1, 5));
        assert_eq!(diagnostics[0].range.end, Position::new(1, 7));
        assert_eq!(diagnostics[1].severity, Some(DiagnosticSeverity::WARNING));
        assert_eq!(diagnostics[1].range.end, Position::new(0, 1));
    }

    #[test]
    fn highlight_ranges_count_utf16_units_after_japanese_text() {
        let positions = [HighlightPosition { line: 0, col: 8 }];
        let diagnostics = diagnostics_for_positions("A 'あ' c\n", &positions);
        assert_eq!(diagnostics[0].range.start, Position::new(0, 6));
        assert_eq!(diagnostics[0].range.end, Position::new(0, 7));
    }

    #[test]
    fn highlights_on_multibyte_text_cover_the_whole_character() {
        let positions = [HighlightPosition { line: 0, col: 2 }];
        let diagnostics = diagnostics_for_positions("A あ\n", &positions);
        assert_eq!(diagnostics[0].range.start, Position::new(0, 2));
        assert_eq!(diagnostics[0].range.end, Position::new(0, 3));
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
