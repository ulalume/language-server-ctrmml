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
//! Instrument lens arguments, after the 0-based header line and the type
//! keyword (all strings):
//!
//! - Load: `number, keyword_line, keyword_end`
//! - Save: `number, keyword_line, keyword_end, name`
//! - Preview: `channel, number, keyword_line, keyword_end`
//!
//! `keyword_line` is the 0-based line of the type keyword and `keyword_end`
//! the UTF-16 character just past it. `name` is the comment on the `@N`
//! line (the patch name), empty when there is none.

use serde::{Deserialize, Serialize};

use crate::completion::byte_offset_to_utf16_character;
use crate::docs::CHANNEL_DESCRIPTIONS;
use crate::instrument_header::{
    instrument_header_at, instrument_name, InstrumentHeader, InstrumentType,
};
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
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
pub enum IconStyle {
    /// VS Code/Monaco codicon markers such as `$(play)`.
    #[default]
    #[serde(rename = "codicon")]
    VsCode,
    /// Plain text for clients that do not understand codicon markers.
    ///
    /// Actionable icons become a text glyph so the lens still reads as a
    /// button (`▷ FM`) instead of a bare noun. `U+25B7 WHITE RIGHT-POINTING
    /// TRIANGLE` is used rather than `U+25B6`: it has no emoji presentation
    /// sequence, so clients render it as text in the editor font.
    #[serde(rename = "text")]
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

/// Consumer-specific code-lens rendering and command options. As JSON:
/// `{ "code_lens_icons": "codicon" | "text", "code_lens_file_actions":
/// bool }`, both optional, camelCase accepted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct CodeLensConfig {
    #[serde(rename = "code_lens_icons", alias = "codeLensIcons")]
    pub icon_style: IconStyle,
    /// Emit Load/Save actions, which require client-side file UI.
    #[serde(rename = "code_lens_file_actions", alias = "codeLensFileActions")]
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
                let target = LensTarget {
                    line: line_idx.to_string(),
                    ty,
                    number: header.number.to_string(),
                    keyword: keyword_position(&model, &header),
                    name: instrument_name(model.get_line_content(line_number))
                        .unwrap_or("")
                        .to_string(),
                };
                emit_instrument_variants(&target, config, &mut out);
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

/// What an instrument lens acts on, as lens arguments.
struct LensTarget {
    line: String,
    ty: InstrumentType,
    number: String,
    /// The keyword's 0-based line and the UTF-16 character past its end.
    keyword: [String; 2],
    name: String,
}

impl LensTarget {
    fn args(&self, between: &[&str], after: &[&str]) -> Vec<String> {
        let mut args = vec![self.line.clone(), self.ty.keyword().to_string()];
        args.extend(between.iter().map(|arg| arg.to_string()));
        args.push(self.number.clone());
        args.extend(self.keyword.iter().cloned());
        args.extend(after.iter().map(|arg| arg.to_string()));
        args
    }
}

fn keyword_position(model: &BorrowedLinesModel, header: &InstrumentHeader) -> [String; 2] {
    let line = model.get_line_content(header.keyword_line);
    let end = byte_offset_to_utf16_character(line, header.keyword_end());
    [(header.keyword_line - 1).to_string(), end.to_string()]
}

fn emit_instrument_variants(target: &LensTarget, config: CodeLensConfig, out: &mut Vec<CodeLens>) {
    let line: u32 = target.line.parse().unwrap_or(0);
    let preview = "mml.previewPatch";
    let load = "mml.loadPatch";
    let save = "mml.savePatch";
    let icons = config.icon_style;

    match target.ty {
        InstrumentType::Fm => {
            if config.include_file_actions {
                out.push(CodeLens::cmd(
                    line,
                    icons.title("folder-opened", "Load"),
                    load,
                    target.args(&[], &[]),
                ));
                out.push(CodeLens::cmd(
                    line,
                    icons.title("save", "Save"),
                    save,
                    target.args(&[], &[&target.name]),
                ));
            }
            out.push(CodeLens::cmd(
                line,
                icons.title("play", "FM"),
                preview,
                target.args(&["A"], &[]),
            ));
        }
        InstrumentType::Pcm => {
            if config.include_file_actions {
                out.push(CodeLens::cmd(
                    line,
                    icons.title("folder-opened", "Load"),
                    load,
                    target.args(&[], &[]),
                ));
            }
            out.push(CodeLens::cmd(
                line,
                icons.title("play", "pcm"),
                preview,
                target.args(&["F"], &[]),
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
                    icons.title("play", label),
                    preview,
                    target.args(&[channel], &[]),
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
        // The keyword sits on the next line.
        assert_eq!(lenses[0].arguments, ["0", "fm", "3", "1", "4"]);
    }

    #[test]
    fn lens_arguments_carry_number_keyword_position_and_name() {
        // `あ` is one UTF-16 unit and three bytes.
        let text = "@7 FM ; Bass あ\n@8\t pcm \"a.wav\"\n";
        let lenses = code_lens(text);
        let args: Vec<(&str, Vec<&str>)> = lenses
            .iter()
            .map(|lens| {
                (
                    lens.command_id.as_deref().unwrap_or(""),
                    lens.arguments.iter().map(String::as_str).collect(),
                )
            })
            .collect();
        assert_eq!(
            args,
            [
                ("mml.loadPatch", vec!["0", "fm", "7", "0", "5"]),
                ("mml.savePatch", vec!["0", "fm", "7", "0", "5", "Bass あ"]),
                ("mml.previewPatch", vec!["0", "fm", "A", "7", "0", "5"]),
                ("mml.loadPatch", vec!["1", "pcm", "8", "1", "7"]),
                ("mml.previewPatch", vec!["1", "pcm", "F", "8", "1", "7"]),
            ]
        );
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
    fn psg_track_j_labeled_as_noise() {
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
