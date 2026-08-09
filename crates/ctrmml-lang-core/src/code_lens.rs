//! Code lens enumeration.
//!
//! Emits two kinds of code lenses:
//!
//! 1. **Instrument preview buttons** — `Load` / `Save` / `Play <channel>`
//!    lenses anchored to `@N fm` / `@N psg` / `@N pcm` definitions. Command
//!    IDs match what `web-ctrmml` already wires up (`mml.previewPatch`,
//!    `mml.loadPatch`, `mml.savePatch`); other consumers (vscode-ctrmml,
//!    zed-ctrmml) can either register matching handlers or display the
//!    titles informationally.
//! 2. **Track header labels** — for lines that start with a track selector
//!    (`A`, `AB`, `*32`, …), an informational `FM1, FM2` style label so the
//!    reader can tell which channels a section drives at a glance.
//!
//! Ported from `web-ctrmml/src/editor/mml-codelens.ts` (source of truth).

use serde::Serialize;

use crate::track_selector::parse_leading_track_selector;

/// One emitted lens.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CodeLens {
    /// 0-based line index where the lens should anchor.
    pub line: u32,
    /// Title shown in the editor.
    pub title: String,
    /// Command ID, or `None` for informational labels.
    pub command_id: Option<String>,
    /// Stringly-typed argument list. Callers usually prepend the document
    /// URI before sending the command to a client.
    pub arguments: Vec<String>,
}

impl CodeLens {
    fn info(line: u32, title: impl Into<String>) -> Self {
        Self {
            line,
            title: title.into(),
            command_id: None,
            arguments: Vec::new(),
        }
    }

    fn cmd(
        line: u32,
        title: impl Into<String>,
        command_id: impl Into<String>,
        arguments: Vec<String>,
    ) -> Self {
        Self {
            line,
            title: title.into(),
            command_id: Some(command_id.into()),
            arguments,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PreviewType {
    Fm,
    Psg,
    Pcm,
}

impl PreviewType {
    const fn keyword(self) -> &'static str {
        match self {
            PreviewType::Fm => "fm",
            PreviewType::Psg => "psg",
            PreviewType::Pcm => "pcm",
        }
    }
    const fn arg(self) -> &'static str {
        self.keyword()
    }
}

const PREVIEW_TYPES: [PreviewType; 3] = [PreviewType::Fm, PreviewType::Psg, PreviewType::Pcm];

// Channel descriptions for Mega Drive / MDSDRV. Mirrors
// `web-ctrmml/src/ui/piano-roll-colors.ts CHANNEL_DESCRIPTIONS`. Platform-
// independent emission is future work — at the moment ctrmml only targets
// MD/MDSDRV so a fixed table is fine.
const CHANNEL_DESCRIPTIONS: &[&str] = &[
    "FM1", "FM2", "FM3", "FM4", "FM5", "FM6", // A..F
    "PSG1", "PSG2", "PSG3",  // G..I
    "Noise", // J
    "PCM2", "PCM3", // K..L
    "Dummy", "Dummy", "Dummy", "Dummy", // M..P
];

/// Enumerate code lenses for `text`.
pub fn code_lens(text: &str) -> Vec<CodeLens> {
    let lines: Vec<&str> = text.lines().collect();
    let mut out = Vec::new();

    for (idx, line) in lines.iter().enumerate() {
        let line_idx = idx as u32;

        if let Some(def) = find_definition(&lines, idx) {
            emit_instrument_variants(line_idx, def.preview_type, def.instrument_number, &mut out);
            continue;
        }

        if let Some(selector) = parse_leading_track_selector(line) {
            let descs: Vec<&'static str> = selector
                .spans
                .iter()
                .filter_map(|span| CHANNEL_DESCRIPTIONS.get(span.track_id as usize).copied())
                .filter(|d| *d != "Dummy")
                .collect();
            if !descs.is_empty() {
                out.push(CodeLens::info(line_idx, descs.join(", ")));
            }
        }
    }
    out
}

struct DefinitionMatch {
    preview_type: PreviewType,
    instrument_number: u32,
}

fn find_definition(lines: &[&str], idx: usize) -> Option<DefinitionMatch> {
    let trimmed = lines.get(idx)?.trim_start();
    let (number, after_number) = take_at_number(trimmed)?;
    let after_number_trimmed = after_number.trim_start();

    // Single-line: `@N type` on the same line.
    for &ty in &PREVIEW_TYPES {
        if let Some(rest) = after_number_trimmed.strip_prefix(ty.keyword()) {
            if rest.is_empty() || rest.starts_with(|c: char| c.is_whitespace() || c == ';') {
                return Some(DefinitionMatch {
                    preview_type: ty,
                    instrument_number: number,
                });
            }
        }
    }

    // Multi-line: `@N` alone, type keyword on the next non-blank, non-comment line.
    let after_only_comment = matches!(after_number_trimmed.chars().next(), None | Some(';'));
    if !after_only_comment {
        return None;
    }
    let lookahead_end = (idx + 3).min(lines.len().saturating_sub(1));
    for j in (idx + 1)..=lookahead_end {
        let next = lines.get(j)?.trim();
        if next.is_empty() || next.starts_with(';') {
            continue;
        }
        for &ty in &PREVIEW_TYPES {
            if let Some(rest) = next.strip_prefix(ty.keyword()) {
                if rest.is_empty() || rest.starts_with(|c: char| c.is_whitespace() || c == ';') {
                    return Some(DefinitionMatch {
                        preview_type: ty,
                        instrument_number: number,
                    });
                }
            }
        }
        break;
    }
    None
}

fn take_at_number(s: &str) -> Option<(u32, &str)> {
    let bytes = s.as_bytes();
    if bytes.first() != Some(&b'@') {
        return None;
    }
    let mut idx = 1;
    while idx < bytes.len() && (bytes[idx] as char).is_ascii_digit() {
        idx += 1;
    }
    if idx == 1 {
        return None;
    }
    let number: u32 = s[1..idx].parse().ok()?;
    Some((number, &s[idx..]))
}

fn emit_instrument_variants(
    line: u32,
    ty: PreviewType,
    instrument_number: u32,
    out: &mut Vec<CodeLens>,
) {
    let n = instrument_number.to_string();
    let preview = "mml.previewPatch";
    let load = "mml.loadPatch";
    let save = "mml.savePatch";
    let line_str = line.to_string();

    match ty {
        PreviewType::Fm => {
            out.push(CodeLens::cmd(
                line,
                "$(folder-opened) Load",
                load,
                vec![line_str.clone(), ty.arg().to_string()],
            ));
            out.push(CodeLens::cmd(
                line,
                "$(save) Save",
                save,
                vec![line_str.clone(), ty.arg().to_string()],
            ));
            out.push(CodeLens::cmd(
                line,
                "$(play) FM",
                preview,
                vec![line_str, ty.arg().to_string(), "A".to_string(), n],
            ));
        }
        PreviewType::Pcm => {
            out.push(CodeLens::cmd(
                line,
                "$(folder-opened) Load",
                load,
                vec![line_str.clone(), ty.arg().to_string()],
            ));
            out.push(CodeLens::cmd(
                line,
                "$(play) pcm",
                preview,
                vec![line_str, ty.arg().to_string(), "F".to_string(), n],
            ));
        }
        PreviewType::Psg => {
            for (title, channel) in [
                ("$(play) Square", "G"),
                ("$(play) Noise mode=0", "J"),
                ("$(play) Noise mode=1", "J 'mode 1'"),
                ("$(play) Noise mode=2", "J 'mode 2'"),
            ] {
                out.push(CodeLens::cmd(
                    line,
                    title,
                    preview,
                    vec![
                        line_str.clone(),
                        ty.arg().to_string(),
                        channel.to_string(),
                        n.clone(),
                    ],
                ));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fm_definition_emits_three_variants() {
        let text = "@1 fm\n3 4\n";
        let lenses = code_lens(text);
        assert_eq!(lenses.len(), 3);
        assert!(lenses.iter().all(|l| l.line == 0));
        let titles: Vec<&str> = lenses.iter().map(|l| l.title.as_str()).collect();
        assert_eq!(
            titles,
            ["$(folder-opened) Load", "$(save) Save", "$(play) FM"]
        );
    }

    #[test]
    fn psg_definition_emits_four_play_variants() {
        let text = "@2 psg 15 14 13\n";
        let lenses = code_lens(text);
        assert_eq!(lenses.len(), 4);
        assert!(lenses
            .iter()
            .all(|l| l.command_id.as_deref() == Some("mml.previewPatch")));
        let titles: Vec<&str> = lenses.iter().map(|l| l.title.as_str()).collect();
        assert_eq!(
            titles,
            [
                "$(play) Square",
                "$(play) Noise mode=0",
                "$(play) Noise mode=1",
                "$(play) Noise mode=2",
            ]
        );
    }

    #[test]
    fn pcm_definition_emits_load_and_play() {
        let text = "@1 pcm \"path.wav\"\n";
        let lenses = code_lens(text);
        let titles: Vec<&str> = lenses.iter().map(|l| l.title.as_str()).collect();
        assert_eq!(titles, ["$(folder-opened) Load", "$(play) pcm"]);
    }

    #[test]
    fn multi_line_header_resolves() {
        let text = "@3\n  fm\n3 4\n";
        let lenses = code_lens(text);
        assert!(!lenses.is_empty());
        assert!(lenses.iter().all(|l| l.line == 0));
    }

    #[test]
    fn track_header_emits_channel_label() {
        let text = "AB t120\n  c d e f\n";
        let lenses = code_lens(text);
        let header_labels: Vec<&str> = lenses
            .iter()
            .filter(|l| l.command_id.is_none())
            .map(|l| l.title.as_str())
            .collect();
        assert_eq!(header_labels, ["FM1, FM2"]);
    }

    #[test]
    fn psg_track_J_labeled_as_noise() {
        let text = "J 'mode 0' c8 d8\n";
        let lenses = code_lens(text);
        let labels: Vec<&str> = lenses
            .iter()
            .filter(|l| l.command_id.is_none())
            .map(|l| l.title.as_str())
            .collect();
        assert_eq!(labels, ["Noise"]);
    }

    #[test]
    fn macro_track_star_32_is_unlabeled() {
        // Dummy / subroutine tracks are filtered out (no channel name).
        let text = "*32 c4 d4\n";
        let lenses = code_lens(text);
        let labels: Vec<&str> = lenses
            .iter()
            .filter(|l| l.command_id.is_none())
            .map(|l| l.title.as_str())
            .collect();
        assert!(labels.is_empty());
    }
}
