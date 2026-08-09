//! Patch preview helpers.
//!
//! Given an `@N fm|psg|pcm` definition at an anchor line, extract the
//! block text and build a self-contained preview MML that plays it on a
//! caller-chosen channel. Ported from
//! `web-ctrmml/src/editor/mml-codelens.ts` (`extractInstrumentBlock` +
//! `buildPreviewMml`).
//!
//! All line numbers in / out are 0-based to match the rest of
//! `ctrmml-lang-core`. Web callers convert to Monaco's 1-based at the
//! shim boundary.

use serde::{Deserialize, Serialize};

use crate::block_finder::{find_block_at, InstrumentKind};
use crate::string_model::LinesModel;
use crate::track_selector::LineReader;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum InstrumentType {
    Fm,
    Psg,
    Pcm,
}

impl InstrumentType {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "fm" => Some(Self::Fm),
            "psg" => Some(Self::Psg),
            "pcm" => Some(Self::Pcm),
            _ => None,
        }
    }
    fn as_kind(self) -> InstrumentKind {
        match self {
            Self::Fm => InstrumentKind::Fm,
            Self::Psg => InstrumentKind::Psg,
            Self::Pcm => InstrumentKind::Pcm,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtractedBlock {
    pub instrument_number: u32,
    pub instrument_type: InstrumentType,
    pub mml_text: String,
    /// 0-based line index of the `@N` header.
    pub start_line: u32,
    /// 0-based line index of the last content line in the block.
    pub end_line: u32,
}

/// Extract the block at `anchor_line` (0-based). Returns `None` when no
/// `@N <type>` definition covers that line.
pub fn extract_instrument_block(
    text: &str,
    anchor_line: u32,
    ty: InstrumentType,
) -> Option<ExtractedBlock> {
    let model = LinesModel::from_text(text);
    // block_finder uses 1-based line numbers internally.
    let one_based = anchor_line.saturating_add(1);
    let region = find_block_at(&model, one_based, ty.as_kind(), &[])?;

    // Re-extract the MML text from the model's view rather than slicing
    // bytes — this keeps line-ending normalisation aligned with how the
    // model already split the input.
    let mut lines: Vec<String> = Vec::new();
    for i in region.start_line..=region.end_line {
        lines.push(model.get_line_content(i).to_string());
    }
    Some(ExtractedBlock {
        instrument_number: region.instrument_number,
        instrument_type: ty,
        mml_text: lines.join("\n"),
        start_line: region.start_line.saturating_sub(1),
        end_line: region.end_line.saturating_sub(1),
    })
}

/// Build a minimal self-contained MML preview that plays the given
/// instrument block on `channel`. Mirrors `buildPreviewMml` in
/// `web-ctrmml/src/editor/mml-codelens.ts`.
pub fn build_preview_mml(text: &str, block: &ExtractedBlock, channel: &str) -> String {
    let platform = detect_platform(text).unwrap_or_else(|| "megadrive".to_string());
    let preview_line = match block.instrument_type {
        InstrumentType::Pcm => format!(
            "{channel} @{n} o4 l4 c",
            n = block.instrument_number,
            channel = channel,
        ),
        _ => format!(
            "{channel} @{n} o4 l8 c r d r e r f r g r a r b r >c r",
            n = block.instrument_number,
            channel = channel,
        ),
    };
    format!(
        "#platform {platform}\n\n{body}\n\n{preview_line}\n",
        body = block.mml_text,
    )
}

fn detect_platform(text: &str) -> Option<String> {
    for line in text.lines() {
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix("#platform") {
            let value = rest.trim().split_whitespace().next()?;
            return Some(value.to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_simple_fm_block() {
        let text = "@1 fm\n\t31,0,12,7,0,28,0,0,5,0\n\t31,0,2,6,0,0,0,0,2,0\n\nA c\n";
        let block = extract_instrument_block(text, 0, InstrumentType::Fm).unwrap();
        assert_eq!(block.instrument_number, 1);
        assert_eq!(block.start_line, 0);
        assert_eq!(block.end_line, 2);
        assert!(block.mml_text.starts_with("@1 fm"));
        assert!(block.mml_text.contains("31,0,2,6"));
    }

    #[test]
    fn extract_multiline_header_form() {
        let text = "@2\n  fm\n\t31,0,12,7,0,28,0,0,5,0\n\nA c\n";
        let block = extract_instrument_block(text, 0, InstrumentType::Fm).unwrap();
        assert_eq!(block.instrument_number, 2);
        assert_eq!(block.start_line, 0);
        assert!(block.mml_text.contains("\n  fm"));
    }

    #[test]
    fn extract_psg_block() {
        let text = "@3 psg 15 14 13 | 5\n\nA g\n";
        let block = extract_instrument_block(text, 0, InstrumentType::Psg).unwrap();
        assert_eq!(block.instrument_number, 3);
    }

    #[test]
    fn extract_pcm_block() {
        let text = "@4 pcm \"snare.wav\"\n";
        let block = extract_instrument_block(text, 0, InstrumentType::Pcm).unwrap();
        assert_eq!(block.instrument_number, 4);
        assert_eq!(block.instrument_type, InstrumentType::Pcm);
    }

    #[test]
    fn build_preview_for_fm() {
        let text = "#platform megadrive\n\n@1 fm\n\t1 1 1 1\n";
        let block = ExtractedBlock {
            instrument_number: 1,
            instrument_type: InstrumentType::Fm,
            mml_text: "@1 fm\n\t1 1 1 1".to_string(),
            start_line: 2,
            end_line: 3,
        };
        let preview = build_preview_mml(text, &block, "A");
        assert!(preview.starts_with("#platform megadrive\n"));
        assert!(preview.contains("@1 fm\n\t1 1 1 1"));
        assert!(preview
            .trim_end()
            .ends_with("A @1 o4 l8 c r d r e r f r g r a r b r >c r"));
    }

    #[test]
    fn build_preview_for_pcm_uses_short_line() {
        let block = ExtractedBlock {
            instrument_number: 4,
            instrument_type: InstrumentType::Pcm,
            mml_text: "@4 pcm \"snare.wav\"".to_string(),
            start_line: 0,
            end_line: 0,
        };
        let preview = build_preview_mml("@4 pcm \"snare.wav\"\n", &block, "F");
        assert!(preview.contains("F @4 o4 l4 c\n"));
    }

    #[test]
    fn build_preview_defaults_to_megadrive() {
        let block = ExtractedBlock {
            instrument_number: 1,
            instrument_type: InstrumentType::Fm,
            mml_text: "@1 fm".to_string(),
            start_line: 0,
            end_line: 0,
        };
        let preview = build_preview_mml("@1 fm\n", &block, "A");
        assert!(preview.starts_with("#platform megadrive\n"));
    }

    #[test]
    fn build_preview_passes_channel_string_verbatim() {
        // `J 'mode 1'` — used by the PSG noise mode=1 lens.
        let block = ExtractedBlock {
            instrument_number: 2,
            instrument_type: InstrumentType::Psg,
            mml_text: "@2 psg 15".to_string(),
            start_line: 0,
            end_line: 0,
        };
        let preview = build_preview_mml("@2 psg 15\n", &block, "J 'mode 1'");
        assert!(preview.contains("J 'mode 1' @2 o4 l8 c r"));
    }
}
