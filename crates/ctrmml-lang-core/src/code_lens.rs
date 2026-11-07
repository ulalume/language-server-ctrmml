//! Code lens enumeration.
//!
//! Emits two kinds of code lenses:
//!
//! 1. **Instrument preview buttons** — `Load` / `Save` / `Play <channel>`
//!    lenses anchored to `@N fm` / `@N psg` / `@N pcm` definitions. Command
//!    IDs match what `web-ctrmml` already wires up (`mml.previewPatch`,
//!    `mml.loadPatch`, `mml.savePatch`); other consumers (vscode-ctrmml,
//!    zed-ctrmml) choose the icon syntax and whether file actions are useful.
//! 2. **Track header labels** — for lines that start with a track selector
//!    (`A`, `AB`, `*32`, …), an informational `FM1, FM2` style label so the
//!    reader can tell which channels a section drives at a glance.
//!
//! Ported from `web-ctrmml/src/editor/mml-codelens.ts` (source of truth).

use serde::Serialize;

use crate::instrument_header::{instrument_header_at, InstrumentType};
use crate::string_model::BorrowedLinesModel;
use crate::track_selector::{parse_leading_track_selector, LineReader};

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

/// Icon syntax to use in command titles.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum IconStyle {
    /// VS Code/Monaco codicon markers such as `$(play)`.
    #[default]
    VsCode,
    /// Plain text for clients that do not understand codicon markers.
    ///
    /// Actionable icons become a text glyph so the lens still reads as a
    /// button (`▷ FM`) instead of a bare noun. `U+25B7 WHITE RIGHT-POINTING
    /// TRIANGLE` is used rather than `U+25B6`: it has no emoji presentation
    /// sequence, so clients render it as text in the editor font.
    None,
}

impl IconStyle {
    fn title(self, icon: &str, label: &str) -> String {
        match self {
            Self::VsCode => format!("$({icon}) {label}"),
            Self::None => match icon {
                "play" => format!("▷ {label}"),
                _ => label.to_string(),
            },
        }
    }
}

/// Consumer-specific code-lens rendering and command options.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CodeLensConfig {
    pub icon_style: IconStyle,
    /// Emit Load/Save actions, which require client-side file UI.
    pub include_file_actions: bool,
}

impl Default for CodeLensConfig {
    fn default() -> Self {
        Self {
            icon_style: IconStyle::VsCode,
            include_file_actions: true,
        }
    }
}

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
    code_lens_with_config(text, CodeLensConfig::default())
}

/// Enumerate code lenses for `text` using consumer-specific options.
pub fn code_lens_with_config(text: &str, config: CodeLensConfig) -> Vec<CodeLens> {
    let model = BorrowedLinesModel::from_text(text);
    let mut out = Vec::new();

    for line_number in 1..=model.get_line_count() {
        let line_idx = line_number - 1;

        if let Some(header) = instrument_header_at(&model, line_number) {
            if let Some(ty) = header.instrument_type() {
                emit_instrument_variants(line_idx, ty, header.number, config, &mut out);
            }
            continue;
        }

        let line = model.get_line_content(line_number);
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

fn emit_instrument_variants(
    line: u32,
    ty: InstrumentType,
    instrument_number: u32,
    config: CodeLensConfig,
    out: &mut Vec<CodeLens>,
) {
    let n = instrument_number.to_string();
    let preview = "mml.previewPatch";
    let load = "mml.loadPatch";
    let save = "mml.savePatch";
    let line_str = line.to_string();

    match ty {
        InstrumentType::Fm => {
            if config.include_file_actions {
                out.push(CodeLens::cmd(
                    line,
                    config.icon_style.title("folder-opened", "Load"),
                    load,
                    vec![line_str.clone(), ty.keyword().to_string()],
                ));
                out.push(CodeLens::cmd(
                    line,
                    config.icon_style.title("save", "Save"),
                    save,
                    vec![line_str.clone(), ty.keyword().to_string()],
                ));
            }
            out.push(CodeLens::cmd(
                line,
                config.icon_style.title("play", "FM"),
                preview,
                vec![line_str, ty.keyword().to_string(), "A".to_string(), n],
            ));
        }
        InstrumentType::Pcm => {
            if config.include_file_actions {
                out.push(CodeLens::cmd(
                    line,
                    config.icon_style.title("folder-opened", "Load"),
                    load,
                    vec![line_str.clone(), ty.keyword().to_string()],
                ));
            }
            out.push(CodeLens::cmd(
                line,
                config.icon_style.title("play", "pcm"),
                preview,
                vec![line_str, ty.keyword().to_string(), "F".to_string(), n],
            ));
        }
        InstrumentType::Psg => {
            for (label, channel) in [
                ("Square", "G"),
                ("Noise mode=0", "J"),
                ("Noise mode=1", "J 'mode 1'"),
                ("Noise mode=2", "J 'mode 2'"),
            ] {
                out.push(CodeLens::cmd(
                    line,
                    config.icon_style.title("play", label),
                    preview,
                    vec![
                        line_str.clone(),
                        ty.keyword().to_string(),
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
    fn plain_titles_have_no_codicon_markers() {
        let lenses = code_lens_with_config(
            "@1 fm\n@2 psg 15 14 13\n",
            CodeLensConfig {
                icon_style: IconStyle::None,
                ..CodeLensConfig::default()
            },
        );
        assert!(lenses.iter().all(|lens| !lens.title.contains("$(")));
        // Play actions keep a text glyph so the lens still reads as a button.
        let titles: Vec<&str> = lenses.iter().map(|lens| lens.title.as_str()).collect();
        assert_eq!(
            titles,
            [
                "Load",
                "Save",
                "▷ FM",
                "▷ Square",
                "▷ Noise mode=0",
                "▷ Noise mode=1",
                "▷ Noise mode=2",
            ]
        );
    }

    #[test]
    fn file_actions_can_be_omitted_without_removing_preview() {
        let lenses = code_lens_with_config(
            "@1 fm\n@2 pcm \"path.wav\"\n",
            CodeLensConfig {
                include_file_actions: false,
                ..CodeLensConfig::default()
            },
        );
        assert_eq!(lenses.len(), 2);
        assert!(lenses
            .iter()
            .all(|lens| lens.command_id.as_deref() == Some("mml.previewPatch")));
        let titles: Vec<&str> = lenses.iter().map(|lens| lens.title.as_str()).collect();
        assert_eq!(titles, ["$(play) FM", "$(play) pcm"]);
    }

    #[test]
    fn header_rules_match_the_block_finder() {
        // `@1fm` is not a header; `@1 FM` is.
        assert!(code_lens("@1fm\n").is_empty());
        let titles: Vec<String> = code_lens("@1 FM\n").into_iter().map(|l| l.title).collect();
        assert_eq!(
            titles,
            ["$(folder-opened) Load", "$(save) Save", "$(play) FM"]
        );
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
