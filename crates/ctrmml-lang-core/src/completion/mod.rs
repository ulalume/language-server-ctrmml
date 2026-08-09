//! Pure, editor-neutral completion cascade.
//!
//! Positions enter and leave this module as UTF-16 code-unit offsets. Context
//! detection converts once to UTF-8 byte offsets and keeps all scans byte-safe.

mod types;

pub use types::{
    ArpeggioPattern, ChordStackMode, CompletionPlan, CompletionSettings, CoreCommand,
    CoreCompletionList, CoreItem, CoreItemKind, CoreTextEdit, CursorTickData, DataPayload,
    DataRequest, EditRange, FmPatchData, InsertFormat, InsertSpec, Pos,
};

use crate::arpeggio::{
    chord_defs_for_arpeggio, render_chord_arpeggio_with_pattern, render_generic_arpeggio,
};
use crate::beat_fill::{generate_measure_rests, is_after_bar_line};
use crate::block_finder::{find_fm_block_at, find_psg_block_at};
use crate::chord::{
    accidental_char, render_chord, render_generic_chord, render_generic_diatonic_dyad,
    render_stacked_chord, render_stacked_generic_chord, render_stacked_generic_diatonic_dyad,
    ChordSize, RootAccidental, CHORDS_3, CHORDS_4, DYADS,
};
use crate::docs::{
    DocEntry, AT_META, AT_META_COMPLETION_LABELS, COMMAND_COMPLETIONS, FM_DEFAULT_TEMPLATE,
    GROUP_VALUES, INSTRUMENT_TYPES, INSTRUMENT_TYPES_TRIGGER_SUGGEST, META_KEYWORDS,
    META_KEYWORDS_TRIGGER_SUGGEST, OPTION_VALUES, PLATFORM_COMMANDS, PLATFORM_VALUES, RATE_OFFSET,
    TIMESIG_VALUES,
};
use crate::key_sig::scan_key_sig_at;
use crate::octave_scan::scan_channel_context_at;
use crate::string_model::BorrowedLinesModel;
use crate::text_scan::{
    is_at_number, is_in_comment, is_in_key_sig, tokenize_outside_double_quotes,
};
use crate::timesig::scan_time_signature;
use crate::track_selector::{find_enclosing_track_selector, parse_leading_track_selector};
use types::ProviderOutcome;

/// Convert a UTF-16 character offset to a byte offset within one line.
///
/// Out-of-range offsets clamp to the line end. An offset in the middle of a
/// surrogate pair clamps to the start of that scalar value.
pub fn utf16_character_to_byte_offset(line: &str, character: u32) -> usize {
    let target = character as usize;
    let mut utf16 = 0usize;
    for (byte, ch) in line.char_indices() {
        let next = utf16 + ch.len_utf16();
        if target < next {
            return byte;
        }
        if target == next {
            return byte + ch.len_utf8();
        }
        utf16 = next;
    }
    line.len()
}

/// Convert a byte offset within one line to a UTF-16 character offset.
///
/// Out-of-range offsets clamp to the line end. An offset in the middle of a
/// UTF-8 scalar value clamps to that scalar's start.
pub fn byte_offset_to_utf16_character(line: &str, byte_offset: usize) -> u32 {
    let target = byte_offset.min(line.len());
    line.char_indices()
        .take_while(|(byte, ch)| *byte + ch.len_utf8() <= target)
        .map(|(_, ch)| ch.len_utf16() as u32)
        .sum()
}

/// Return the `[A-Za-z0-9_]` run immediately before `pos` as an explicit range.
pub fn word_range_before_cursor(line: &str, pos: Pos) -> EditRange {
    let cursor = utf16_character_to_byte_offset(line, pos.character);
    word_range_from_byte(line, pos.line, cursor)
}

/// Run the completion cascade, requesting host data when a provider needs it.
pub fn completion_plan(
    text: &str,
    pos: Pos,
    _trigger: Option<char>,
    settings: &CompletionSettings,
) -> CompletionPlan {
    let Some(line) = line_at(text, pos.line) else {
        return done_empty();
    };
    let cursor = utf16_character_to_byte_offset(line, pos.character);
    let cursor_pos = Pos {
        line: pos.line,
        character: byte_offset_to_utf16_character(line, cursor),
    };
    let prefix = &line[..cursor];

    // D3 / §3.1-8: global exclusive guards.
    if is_in_comment(line, cursor) {
        return done_empty();
    }
    if is_in_key_sig(line, cursor) {
        return done_empty();
    }
    if parse_leading_track_selector(line).is_some_and(|selector| cursor <= selector.end) {
        return done_empty();
    }

    // D4: measure fill follows the global guards and owns the `|` trigger.
    if measure_fill_slot(prefix) {
        return CompletionPlan::NeedsData(DataRequest::CursorTick);
    }

    // D5: quoted platform commands.
    if let ProviderOutcome::Exclusive(list) = platform_command_provider(line, cursor, pos.line) {
        return CompletionPlan::Done(list);
    }

    // PCM path completion inside an open quote after a `pcm` token.
    if pcm_path_context(prefix).is_some() {
        return CompletionPlan::NeedsData(DataRequest::PcmPaths);
    }
    // D7: meta values.
    if let ProviderOutcome::Exclusive(list) = meta_values_slot(line, cursor, pos.line) {
        return CompletionPlan::Done(list);
    }
    // D6: meta keywords.
    if let ProviderOutcome::Exclusive(list) = meta_keywords_slot(line, cursor, pos.line) {
        return CompletionPlan::Done(list);
    }
    if prefix.trim_start().starts_with('#') {
        return done_empty();
    }

    // D8 / §3.1-8: this must stay before the FM and PCM file providers.
    // A completed PCM path also satisfies the PCM-instrument predicate, so
    // moving this lower silently steals rate/offset completion.
    if let ProviderOutcome::Exclusive(list) = rate_offset_slot(line, cursor, pos.line) {
        return CompletionPlan::Done(list);
    }
    // D11/D12: FM patches, optionally in the hierarchy's fragment step.
    if let Some(context) = fm_patch_context(prefix, settings.fm_picker_hierarchy) {
        return CompletionPlan::NeedsData(DataRequest::FmPatches {
            fragment: context.fragment,
        });
    }
    // D14: PCM files at `@N pcm` (with either a boundary or whitespace).
    if pcm_instrument_context(prefix).is_some() {
        return CompletionPlan::NeedsData(DataRequest::PcmFiles);
    }
    // D9: instrument types.
    if let ProviderOutcome::Exclusive(list) = instrument_type_slot(line, cursor, pos.line) {
        return CompletionPlan::Done(list);
    }
    // D10: @meta definitions.
    if let ProviderOutcome::Exclusive(list) = at_meta_slot(line, cursor, pos.line) {
        return CompletionPlan::Done(list);
    }
    if prefix.trim_start().starts_with('@') {
        return done_empty();
    }

    // D15: chord completion owns `{<letter>[accidental]` contexts.
    if let ProviderOutcome::Exclusive(list) = chord_slot(text, line, cursor, pos.line, settings) {
        return CompletionPlan::Done(list);
    }
    // D16: opt-in patterned arpeggios for a just-typed bare note.
    if let ProviderOutcome::Exclusive(list) = arpeggio_slot(text, line, cursor, pos.line, settings)
    {
        return CompletionPlan::Done(list);
    }

    // D3 / §3.1-8: suppression deliberately follows chord and arpeggio.
    if suppress_note_or_rest(line, cursor) {
        return done_empty();
    }

    CompletionPlan::Done(command_fallback(line, cursor_pos))
}

/// Resolve a data-bearing completion request.
///
/// Detection is deliberately re-run so the API remains stateless. If the
/// payload kind does not match the newly detected request, the host supplied
/// stale or incorrect data; that host error is an exclusive empty result.
pub fn completion_resolve(
    text: &str,
    pos: Pos,
    trigger: Option<char>,
    settings: &CompletionSettings,
    data: DataPayload,
) -> CoreCompletionList {
    match completion_plan(text, pos, trigger, settings) {
        CompletionPlan::Done(list) => list,
        CompletionPlan::NeedsData(DataRequest::PcmPaths) => match data {
            DataPayload::PcmPaths(paths) => resolve_pcm_paths(text, pos, paths),
            _ => CoreCompletionList::empty(),
        },
        CompletionPlan::NeedsData(DataRequest::PcmFiles) => match data {
            DataPayload::PcmFiles(paths) => resolve_pcm_instruments(text, pos, paths),
            _ => CoreCompletionList::empty(),
        },
        CompletionPlan::NeedsData(DataRequest::FmPatches { fragment }) => match data {
            DataPayload::FmPatches(patches) => {
                resolve_fm_patches(text, pos, settings, fragment, &patches)
            }
            _ => CoreCompletionList::empty(),
        },
        CompletionPlan::NeedsData(DataRequest::CursorTick) => match data {
            DataPayload::CursorTick(timing) => resolve_measure_fill(text, pos, timing),
            _ => CoreCompletionList::empty(),
        },
    }
}

fn line_at(text: &str, line: u32) -> Option<&str> {
    text.split('\n')
        .nth(line as usize)
        .map(|value| value.strip_suffix('\r').unwrap_or(value))
}

fn done_empty() -> CompletionPlan {
    CompletionPlan::Done(CoreCompletionList::empty())
}

fn word_range_from_byte(line: &str, line_index: u32, cursor: usize) -> EditRange {
    let bytes = line.as_bytes();
    let mut start = cursor.min(bytes.len());
    while start > 0 && is_word_byte(bytes[start - 1]) {
        start -= 1;
    }
    EditRange::new(
        Pos {
            line: line_index,
            character: byte_offset_to_utf16_character(line, start),
        },
        Pos {
            line: line_index,
            character: byte_offset_to_utf16_character(line, cursor),
        },
    )
}

fn is_word_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

fn platform_command_provider(line: &str, cursor: usize, line_index: u32) -> ProviderOutcome {
    let prefix = &line[..cursor];
    if !is_platform_command_context(prefix) {
        return ProviderOutcome::NotApplicable;
    }

    let start = platform_command_start_byte(prefix);
    let range = EditRange::new(
        Pos {
            line: line_index,
            character: byte_offset_to_utf16_character(line, start),
        },
        Pos {
            line: line_index,
            character: byte_offset_to_utf16_character(line, cursor),
        },
    );
    let entries = PLATFORM_COMMANDS
        .iter()
        .filter(|entry| !matches!(entry.key, "_" | "__" | "_{"));
    ProviderOutcome::Exclusive(CoreCompletionList {
        items: render_table(entries, CoreItemKind::Function, range),
        is_incomplete: false,
    })
}

fn is_platform_command_context(prefix: &str) -> bool {
    let mut in_double = false;
    let mut in_single = false;
    for ch in prefix.chars() {
        match ch {
            '"' if !in_single => in_double = !in_double,
            '\'' if !in_double => in_single = !in_single,
            _ => {}
        }
    }
    in_single
}

fn platform_command_start_byte(prefix: &str) -> usize {
    let after_quote = prefix.rfind('\'').map_or(0, |byte| byte + 1);
    prefix[after_quote..]
        .char_indices()
        .rev()
        .find(|(_, ch)| ch.is_whitespace())
        .map_or(after_quote, |(byte, ch)| after_quote + byte + ch.len_utf8())
}

fn command_fallback(line: &str, cursor: Pos) -> CoreCompletionList {
    let range = word_range_before_cursor(line, cursor);
    CoreCompletionList {
        items: render_table(COMMAND_COMPLETIONS.iter(), CoreItemKind::Function, range),
        is_incomplete: true,
    }
}

fn render_table<'a>(
    entries: impl Iterator<Item = &'a DocEntry>,
    kind: CoreItemKind,
    range: EditRange,
) -> Vec<CoreItem> {
    entries
        .enumerate()
        .map(|(index, entry)| CoreItem {
            label: entry.label.to_string(),
            label_description: None,
            kind,
            detail: Some(entry.detail.to_string()),
            documentation: (!entry.doc.is_empty()).then(|| entry.doc.to_string()),
            insert: InsertSpec {
                text: entry.insert.to_string(),
                format: if entry.insert.contains('$') {
                    InsertFormat::Snippet
                } else {
                    InsertFormat::PlainText
                },
                as_is: false,
            },
            filter_text: Some(entry.key.to_string()),
            sort_text: Some(format!("{index:03}")),
            preselect: false,
            edit_range: range,
            additional_edits: Vec::new(),
            command: None,
        })
        .collect()
}

fn suppress_note_or_rest(line: &str, cursor: usize) -> bool {
    let prefix = &line[..cursor];
    if let Some(token) = whitespace_token_containing_cursor(line, cursor) {
        if is_note_or_rest_token(token) {
            return true;
        }
    }
    ends_at_note_like(prefix)
}

fn whitespace_token_containing_cursor(line: &str, cursor: usize) -> Option<&str> {
    if cursor > line.len() || !line.is_char_boundary(cursor) {
        return None;
    }
    let start = line[..cursor]
        .char_indices()
        .rev()
        .find(|(_, ch)| ch.is_whitespace())
        .map_or(0, |(byte, ch)| byte + ch.len_utf8());
    let end = line[cursor..]
        .char_indices()
        .find(|(_, ch)| ch.is_whitespace())
        .map_or(line.len(), |(byte, _)| cursor + byte);
    (start < end).then(|| &line[start..end])
}

fn is_note_or_rest_token(token: &str) -> bool {
    let mut body = token;
    let dots = body.len() - body.trim_end_matches('.').len();
    if dots > 3 {
        return false;
    }
    body = body.trim_end_matches('.');

    let digits_start = body
        .char_indices()
        .rev()
        .find(|(_, ch)| !ch.is_ascii_digit())
        .map_or(0, |(byte, ch)| byte + ch.len_utf8());
    body = &body[..digits_start];
    body = body.strip_suffix(':').unwrap_or(body);

    if let Some(rest) = body.strip_prefix('r') {
        return rest.is_empty();
    }
    let mut chars = body.chars();
    let Some(note) = chars.next() else {
        return false;
    };
    if !matches!(note, 'a'..='h') {
        return false;
    }
    matches!(chars.as_str(), "" | "+" | "-" | "=")
}

fn ends_at_note_like(prefix: &str) -> bool {
    let mut body = prefix;
    let dots = body.len() - body.trim_end_matches('.').len();
    if dots > 3 {
        return false;
    }
    body = body.trim_end_matches('.');
    body = body.trim_end_matches(|ch: char| ch.is_ascii_digit());
    body = body.strip_suffix(':').unwrap_or(body);
    if body.ends_with(['+', '-', '=']) {
        body = &body[..body.len() - 1];
    }
    matches!(body.chars().next_back(), Some('a'..='h'))
}

fn measure_fill_slot(prefix: &str) -> bool {
    prefix.ends_with('|')
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PcmPathContext {
    path_start: usize,
    path_prefix: String,
}

fn pcm_path_context(prefix: &str) -> Option<PcmPathContext> {
    if prefix.bytes().filter(|byte| *byte == b'"').count() % 2 == 0
        || !prefix.split_whitespace().any(|token| token == "pcm")
    {
        return None;
    }
    let quote = prefix.rfind('"')?;
    Some(PcmPathContext {
        path_start: quote + 1,
        path_prefix: prefix[quote + 1..].to_string(),
    })
}

fn meta_values_slot(line: &str, cursor: usize, line_index: u32) -> ProviderOutcome {
    let prefix = &line[..cursor];
    let Some((keyword, entries)) = meta_value_entries(prefix) else {
        return ProviderOutcome::NotApplicable;
    };
    let range = meta_value_range(line, cursor, line_index, keyword);
    ProviderOutcome::Exclusive(CoreCompletionList {
        items: render_table(entries.iter(), CoreItemKind::Value, range),
        is_incomplete: false,
    })
}

fn meta_value_entries(prefix: &str) -> Option<(&'static str, &'static [DocEntry])> {
    [
        ("#platform", PLATFORM_VALUES),
        ("#option", OPTION_VALUES),
        ("#timesig", TIMESIG_VALUES),
        ("#group", GROUP_VALUES),
    ]
    .into_iter()
    .find_map(|(keyword, entries)| {
        is_meta_value_context(prefix, keyword).then_some((keyword, entries))
    })
}

fn meta_value_range(line: &str, cursor: usize, line_index: u32, keyword: &str) -> EditRange {
    let prefix = &line[..cursor];
    let keyword_end = prefix
        .find(keyword)
        .map_or(0, |start| start + keyword.len());
    let start = prefix[keyword_end..]
        .char_indices()
        .rev()
        .take_while(|(_, ch)| !ch.is_whitespace())
        .last()
        .map_or(cursor, |(byte, _)| keyword_end + byte);
    EditRange::new(
        Pos {
            line: line_index,
            character: byte_offset_to_utf16_character(line, start),
        },
        Pos {
            line: line_index,
            character: byte_offset_to_utf16_character(line, cursor),
        },
    )
}

fn is_meta_value_context(prefix: &str, keyword: &str) -> bool {
    let trimmed = prefix.trim_start();
    trimmed
        .strip_prefix(keyword)
        .is_some_and(|after| after.chars().next().is_some_and(char::is_whitespace))
}

fn meta_keywords_slot(line: &str, cursor: usize, line_index: u32) -> ProviderOutcome {
    let prefix = &line[..cursor];
    let Some(hash) = meta_keyword_hash(prefix) else {
        return ProviderOutcome::NotApplicable;
    };
    let range = EditRange::new(
        Pos {
            line: line_index,
            character: byte_offset_to_utf16_character(line, hash + 1),
        },
        Pos {
            line: line_index,
            character: byte_offset_to_utf16_character(line, cursor),
        },
    );
    let mut items = render_table(META_KEYWORDS.iter(), CoreItemKind::Keyword, range);
    for (item, entry) in items.iter_mut().zip(META_KEYWORDS) {
        item.insert.text = entry.key.strip_prefix('#').unwrap_or(entry.key).to_string();
        item.insert.format = if item.insert.text.contains('$') {
            InsertFormat::Snippet
        } else {
            InsertFormat::PlainText
        };
        if META_KEYWORDS_TRIGGER_SUGGEST.contains(&entry.key) {
            item.command = Some(CoreCommand::TriggerSuggest);
        }
    }
    ProviderOutcome::Exclusive(CoreCompletionList {
        items,
        is_incomplete: false,
    })
}

fn meta_keyword_hash(prefix: &str) -> Option<usize> {
    let hash = prefix.rfind('#')?;
    if prefix[..hash].chars().any(|ch| !ch.is_whitespace())
        || prefix[hash + 1..].chars().any(char::is_whitespace)
    {
        return None;
    }
    Some(hash)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FmPatchContext {
    fm_end: usize,
    fragment: Option<String>,
}

fn fm_patch_context(prefix: &str, hierarchy: bool) -> Option<FmPatchContext> {
    let trimmed = prefix.trim_start();
    let leading = prefix.len() - trimmed.len();
    let after_at = trimmed.strip_prefix('@')?;
    let digits = after_at.bytes().take_while(u8::is_ascii_digit).count();
    if digits == 0 {
        return None;
    }
    let after_digits = &after_at[digits..];
    if !after_digits.chars().next().is_some_and(char::is_whitespace) {
        return None;
    }
    let before_fm = after_digits.len() - after_digits.trim_start().len();
    let fm_and_after = &after_digits[before_fm..];
    let after_fm = fm_and_after.strip_prefix("fm")?;
    if after_fm
        .chars()
        .next()
        .is_some_and(|ch| !ch.is_whitespace())
    {
        return None;
    }

    let fragment = hierarchy
        .then(|| {
            let typed = after_fm.trim_start();
            typed.rfind('/').map(|slash| typed[..slash].to_string())
        })
        .flatten();
    Some(FmPatchContext {
        fm_end: leading + 1 + digits + before_fm + 2,
        fragment,
    })
}

fn pcm_instrument_context(prefix: &str) -> Option<usize> {
    let trimmed = prefix.trim_start();
    let leading = prefix.len() - trimmed.len();
    let after_at = trimmed.strip_prefix('@')?;
    let digits = after_at.bytes().take_while(u8::is_ascii_digit).count();
    if digits == 0 {
        return None;
    }
    let after_digits = &after_at[digits..];
    if !after_digits.chars().next().is_some_and(char::is_whitespace) {
        return None;
    }
    let before_pcm = after_digits.len() - after_digits.trim_start().len();
    let pcm_and_after = &after_digits[before_pcm..];
    let after_pcm = pcm_and_after.strip_prefix("pcm")?;
    if after_pcm
        .chars()
        .next()
        .is_some_and(|ch| !ch.is_whitespace())
    {
        return None;
    }
    Some(leading + 1 + digits + before_pcm + 3)
}

fn resolve_pcm_paths(text: &str, pos: Pos, paths: Vec<String>) -> CoreCompletionList {
    let Some(line) = line_at(text, pos.line) else {
        return CoreCompletionList::empty();
    };
    let cursor = utf16_character_to_byte_offset(line, pos.character);
    let Some(context) = pcm_path_context(&line[..cursor]) else {
        return CoreCompletionList::empty();
    };
    let suffix = if line[cursor..].starts_with('"') {
        ""
    } else {
        "\""
    };
    let range = EditRange::new(
        Pos {
            line: pos.line,
            character: byte_offset_to_utf16_character(line, context.path_start),
        },
        Pos {
            line: pos.line,
            character: byte_offset_to_utf16_character(line, cursor),
        },
    );
    let mut seen = Vec::new();
    let items = paths
        .into_iter()
        .filter(|path| path.starts_with(&context.path_prefix))
        .filter(|path| {
            if seen.contains(path) {
                false
            } else {
                seen.push(path.clone());
                true
            }
        })
        .map(|path| CoreItem {
            label: path.clone(),
            label_description: None,
            kind: CoreItemKind::File,
            detail: None,
            documentation: None,
            insert: InsertSpec {
                text: format!("{path}{suffix}"),
                format: InsertFormat::PlainText,
                as_is: false,
            },
            filter_text: Some(path.clone()),
            sort_text: Some(path),
            preselect: false,
            edit_range: range,
            additional_edits: Vec::new(),
            command: None,
        })
        .collect();
    CoreCompletionList {
        items,
        is_incomplete: false,
    }
}

fn resolve_pcm_instruments(text: &str, pos: Pos, paths: Vec<String>) -> CoreCompletionList {
    let Some(line) = line_at(text, pos.line) else {
        return CoreCompletionList::empty();
    };
    let cursor = utf16_character_to_byte_offset(line, pos.character);
    let Some(start) = pcm_instrument_context(&line[..cursor]) else {
        return CoreCompletionList::empty();
    };
    let range = end_of_line_range(line, pos.line, start);
    let mut seen = Vec::new();
    let mut items: Vec<CoreItem> = paths
        .into_iter()
        .filter(|path| {
            if seen.contains(path) {
                false
            } else {
                seen.push(path.clone());
                true
            }
        })
        .map(|path| CoreItem {
            label: path.clone(),
            label_description: None,
            kind: CoreItemKind::File,
            detail: Some(
                path_extension_upper(&path)
                    .map_or_else(|| "sample".to_string(), |ext| format!("{ext} sample")),
            ),
            documentation: None,
            insert: InsertSpec {
                text: format!(" \"{path}\""),
                format: InsertFormat::PlainText,
                as_is: false,
            },
            filter_text: Some(path.clone()),
            sort_text: Some(path),
            preselect: false,
            edit_range: range,
            additional_edits: Vec::new(),
            command: None,
        })
        .collect();

    if items.is_empty() {
        let current_text = line[start..].to_string();
        items.push(CoreItem {
            label: "PCM files (.wav) are suggested when present.".to_string(),
            label_description: None,
            kind: CoreItemKind::Text,
            detail: Some("Import a PCM file to see path suggestions here.".to_string()),
            documentation: None,
            insert: InsertSpec {
                text: current_text.clone(),
                format: InsertFormat::PlainText,
                as_is: false,
            },
            filter_text: Some(if current_text.is_empty() {
                "pcm".to_string()
            } else {
                current_text
            }),
            sort_text: Some("zzzz".to_string()),
            preselect: false,
            edit_range: range,
            additional_edits: Vec::new(),
            command: None,
        });
    }

    CoreCompletionList {
        items,
        is_incomplete: false,
    }
}

fn resolve_fm_patches(
    text: &str,
    pos: Pos,
    settings: &CompletionSettings,
    fragment: Option<String>,
    patches: &[FmPatchData],
) -> CoreCompletionList {
    let Some(line) = line_at(text, pos.line) else {
        return CoreCompletionList::empty();
    };
    let cursor = utf16_character_to_byte_offset(line, pos.character);
    let Some(context) = fm_patch_context(&line[..cursor], settings.fm_picker_hierarchy) else {
        return CoreCompletionList::empty();
    };
    if context.fragment != fragment {
        return CoreCompletionList::empty();
    }
    let range = end_of_line_range(line, pos.line, context.fm_end);
    let additional_edits = collect_instrument_param_edit(text, pos.line);
    let patches: Vec<&FmPatchData> = patches
        .iter()
        .filter(|patch| !patch.rel_path.is_empty() && !patch.mml.trim().is_empty())
        .collect();
    let mut patch_counts = std::collections::HashMap::new();
    for patch in &patches {
        *patch_counts
            .entry(patch.rel_path.as_str())
            .or_insert(0usize) += 1;
    }

    let items = if let Some(fragment) = fragment {
        let mut items = render_fm_fragment_items(&patches, &fragment, range, &additional_edits);
        if items.is_empty() {
            items.push(default_fm_item(range, &additional_edits));
        }
        items
    } else if settings.fm_picker_hierarchy {
        let mut items = render_fm_file_items(&patches, &patch_counts, range, &additional_edits);
        items.push(default_fm_item(range, &additional_edits));
        items
    } else {
        let mut items = render_fm_flat_items(&patches, &patch_counts, range, &additional_edits);
        items.push(default_fm_item(range, &additional_edits));
        items
    };

    CoreCompletionList {
        items,
        is_incomplete: false,
    }
}

fn render_fm_flat_items(
    patches: &[&FmPatchData],
    patch_counts: &std::collections::HashMap<&str, usize>,
    range: EditRange,
    additional_edits: &[CoreTextEdit],
) -> Vec<CoreItem> {
    let mut items = Vec::new();
    for patch in patches {
        let count = patch_counts
            .get(patch.rel_path.as_str())
            .copied()
            .unwrap_or_default();
        if let Some(item) = render_fm_flat_item(patch, count, items.len(), range, additional_edits)
        {
            items.push(item);
        }
    }
    items
}

fn render_fm_flat_item(
    patch: &FmPatchData,
    count: usize,
    sort_index: usize,
    range: EditRange,
    additional_edits: &[CoreTextEdit],
) -> Option<CoreItem> {
    let param_text = fm_param_text(&patch.mml)?;
    let label = match (&patch.name, count > 1) {
        (Some(name), true) => format!("{}: {name}", patch.rel_path),
        (Some(name), false) => format!("{} — {name}", patch.rel_path),
        (None, _) => patch.rel_path.clone(),
    };
    Some(CoreItem {
        label,
        label_description: None,
        kind: CoreItemKind::Snippet,
        detail: Some(patch.name.clone().unwrap_or_else(|| {
            path_extension_upper(&patch.rel_path).map_or_else(
                || "instrument".to_string(),
                |ext| format!("{ext} instrument"),
            )
        })),
        documentation: None,
        insert: InsertSpec {
            text: param_text,
            format: InsertFormat::PlainText,
            as_is: true,
        },
        filter_text: None,
        sort_text: Some(format!("{sort_index:04}")),
        preselect: false,
        edit_range: range,
        additional_edits: additional_edits.to_vec(),
        command: None,
    })
}

fn render_fm_file_items(
    patches: &[&FmPatchData],
    patch_counts: &std::collections::HashMap<&str, usize>,
    range: EditRange,
    additional_edits: &[CoreTextEdit],
) -> Vec<CoreItem> {
    let mut rel_paths = Vec::new();
    patches
        .iter()
        .copied()
        .filter(|patch| {
            if rel_paths.contains(&patch.rel_path.as_str()) {
                false
            } else {
                rel_paths.push(patch.rel_path.as_str());
                true
            }
        })
        .filter_map(|patch| {
            let rel_path = patch.rel_path.as_str();
            let count = patch_counts.get(rel_path).copied().unwrap_or_default();
            if count == 1 {
                let mut item = render_fm_flat_item(patch, count, 0, range, additional_edits)?;
                // §3.1-7 keeps flat-mode content while preserving file-step order.
                item.sort_text = Some(rel_path.to_string());
                return Some(item);
            }
            Some(CoreItem {
                label: rel_path.to_string(),
                label_description: Some(count.to_string()),
                kind: CoreItemKind::File,
                detail: None,
                documentation: None,
                insert: InsertSpec {
                    text: format!(" {rel_path}/"),
                    format: InsertFormat::PlainText,
                    as_is: false,
                },
                filter_text: Some(rel_path.to_string()),
                sort_text: Some(rel_path.to_string()),
                preselect: false,
                edit_range: range,
                additional_edits: Vec::new(),
                command: Some(CoreCommand::TriggerSuggest),
            })
        })
        .collect()
}

fn render_fm_fragment_items(
    patches: &[&FmPatchData],
    fragment: &str,
    range: EditRange,
    additional_edits: &[CoreTextEdit],
) -> Vec<CoreItem> {
    patches
        .iter()
        .filter(|patch| patch.rel_path == fragment)
        .filter_map(|patch| {
            let param_text = fm_param_text(&patch.mml)?;
            let label = patch
                .name
                .clone()
                .unwrap_or_else(|| path_stem(&patch.rel_path).to_string());
            Some(CoreItem {
                label: label.clone(),
                label_description: Some(fragment.to_string()),
                kind: CoreItemKind::Value,
                detail: patch.has_macros.then(|| "[macros]".to_string()),
                documentation: (!patch.rel_path.is_empty()).then(|| patch.rel_path.clone()),
                insert: InsertSpec {
                    text: param_text,
                    format: InsertFormat::PlainText,
                    as_is: true,
                },
                filter_text: Some(format!("{fragment}/{label}")),
                sort_text: Some(label),
                preselect: false,
                edit_range: range,
                additional_edits: additional_edits.to_vec(),
                command: None,
            })
        })
        .collect()
}

fn default_fm_item(range: EditRange, additional_edits: &[CoreTextEdit]) -> CoreItem {
    CoreItem {
        label: "Default FM template".to_string(),
        label_description: None,
        kind: CoreItemKind::Snippet,
        detail: Some("template".to_string()),
        documentation: None,
        insert: InsertSpec {
            text: FM_DEFAULT_TEMPLATE.to_string(),
            format: InsertFormat::Snippet,
            as_is: false,
        },
        filter_text: Some("default fm template".to_string()),
        sort_text: Some("~default".to_string()),
        preselect: false,
        edit_range: range,
        additional_edits: additional_edits.to_vec(),
        command: None,
    }
}

fn fm_param_text(mml: &str) -> Option<String> {
    if mml.is_empty() {
        return None;
    }
    let first_newline = mml.find('\n')?;
    let header = &mml[..first_newline];
    let comment = header.find(';').map(|index| &header[index..]);
    let mut text = String::from(" ");
    if let Some(comment) = comment {
        text.push_str(comment);
        text.push('\n');
    }
    text.push_str(&mml[first_newline + 1..]);
    while text.ends_with('\n') {
        text.pop();
        if text.ends_with('\r') {
            text.pop();
        }
    }
    Some(text)
}

fn collect_instrument_param_edit(text: &str, cursor_line: u32) -> Vec<CoreTextEdit> {
    let lines: Vec<&str> = text
        .split('\n')
        .map(|line| line.strip_suffix('\r').unwrap_or(line))
        .collect();
    let start_line = cursor_line as usize + 1;
    if start_line >= lines.len() {
        return Vec::new();
    }

    let mut has_params = false;
    let mut last_added = None;
    for (index, line) in lines.iter().enumerate().skip(start_line) {
        let trimmed = line.trim_start();
        let is_param_line = is_instrument_param_line(line);
        if line.starts_with('@') || (parse_leading_track_selector(line).is_some() && !is_param_line)
        {
            break;
        }
        if trimmed.is_empty() {
            if has_params {
                break;
            }
            continue;
        }
        if trimmed.starts_with(';') && !trimmed.starts_with("; '") {
            continue;
        }
        if trimmed.starts_with("; '") || is_param_line {
            has_params = true;
            last_added = Some(index);
            continue;
        }
        break;
    }

    let Some(last_added) = last_added else {
        return Vec::new();
    };
    let end = if last_added + 1 >= lines.len() {
        Pos {
            line: last_added as u32,
            character: lines[last_added].encode_utf16().count() as u32,
        }
    } else {
        Pos {
            line: last_added as u32 + 1,
            character: 0,
        }
    };
    vec![CoreTextEdit {
        range: EditRange::new(
            Pos {
                line: start_line as u32,
                character: 0,
            },
            end,
        ),
        new_text: String::new(),
    }]
}

fn is_instrument_param_line(line: &str) -> bool {
    let slice = line.split_once(';').map_or(line, |(before, _)| before);
    slice.chars().all(|ch| {
        ch.is_whitespace()
            || ch.is_ascii_digit()
            || matches!(ch, '+' | '-' | ',' | '>' | ':' | '|' | '/')
    })
}

fn resolve_measure_fill(
    text: &str,
    pos: Pos,
    timing: Option<CursorTickData>,
) -> CoreCompletionList {
    let Some(timing) = timing.filter(|timing| timing.ppqn > 0) else {
        return CoreCompletionList::empty();
    };
    let Some(time_signature) = scan_time_signature(text) else {
        return CoreCompletionList::empty();
    };
    let Some(line) = line_at(text, pos.line) else {
        return CoreCompletionList::empty();
    };
    let cursor = utf16_character_to_byte_offset(line, pos.character);
    if cursor == 0 || line.as_bytes().get(cursor - 1) != Some(&b'|') {
        return CoreCompletionList::empty();
    }
    let after_bar_line = is_after_bar_line(line, cursor as u32);
    let rests = generate_measure_rests(
        timing.tick,
        timing.ppqn,
        Some(time_signature),
        after_bar_line,
    );
    if rests.is_empty() {
        return CoreCompletionList::empty();
    }
    let fill = format!("{rests} |");
    let range = EditRange::new(
        Pos {
            line: pos.line,
            character: byte_offset_to_utf16_character(line, cursor - 1),
        },
        Pos {
            line: pos.line,
            character: byte_offset_to_utf16_character(line, cursor),
        },
    );
    CoreCompletionList {
        items: vec![CoreItem {
            label: format!("Fill measure ({fill})"),
            label_description: None,
            kind: CoreItemKind::Snippet,
            detail: None,
            documentation: None,
            insert: InsertSpec {
                text: fill,
                format: InsertFormat::PlainText,
                as_is: false,
            },
            filter_text: None,
            sort_text: Some("0".to_string()),
            preselect: false,
            edit_range: range,
            additional_edits: Vec::new(),
            command: None,
        }],
        is_incomplete: false,
    }
}

fn end_of_line_range(line: &str, line_index: u32, start: usize) -> EditRange {
    EditRange::new(
        Pos {
            line: line_index,
            character: byte_offset_to_utf16_character(line, start),
        },
        Pos {
            line: line_index,
            character: byte_offset_to_utf16_character(line, line.len()),
        },
    )
}

fn path_extension_upper(path: &str) -> Option<String> {
    path.rsplit('/')
        .next()
        .and_then(|name| name.rsplit_once('.').map(|(_, extension)| extension))
        .filter(|extension| !extension.is_empty())
        .map(str::to_ascii_uppercase)
}

fn path_stem(path: &str) -> &str {
    let name = path.rsplit('/').next().unwrap_or(path);
    name.rsplit_once('.').map_or(name, |(stem, _)| stem)
}

fn instrument_type_slot(line: &str, cursor: usize, line_index: u32) -> ProviderOutcome {
    let prefix = &line[..cursor];
    if !is_instrument_type_context(prefix) {
        return ProviderOutcome::NotApplicable;
    }
    let range = word_range_from_byte(line, line_index, cursor);
    let mut items = render_table(INSTRUMENT_TYPES.iter(), CoreItemKind::TypeParameter, range);
    for (item, entry) in items.iter_mut().zip(INSTRUMENT_TYPES) {
        if INSTRUMENT_TYPES_TRIGGER_SUGGEST.contains(&entry.key) {
            item.command = Some(CoreCommand::TriggerSuggest);
        }
    }
    ProviderOutcome::Exclusive(CoreCompletionList {
        items,
        is_incomplete: true,
    })
}

fn is_instrument_type_context(prefix: &str) -> bool {
    let trimmed = prefix.trim_start();
    let Some(after_at) = trimmed.strip_prefix('@') else {
        return false;
    };
    let digits = after_at.bytes().take_while(u8::is_ascii_digit).count();
    if digits == 0 {
        return false;
    }
    let Some(after_space) = after_at[digits..].strip_prefix(' ') else {
        return false;
    };
    let typed = after_space.trim_start();
    ["fm", "psg", "pcm", "2op"]
        .iter()
        .any(|keyword| keyword.starts_with(typed))
}

fn at_meta_slot(line: &str, cursor: usize, line_index: u32) -> ProviderOutcome {
    let prefix = &line[..cursor];
    let Some(start) = at_meta_edit_start(prefix) else {
        return ProviderOutcome::NotApplicable;
    };
    let range = EditRange::new(
        Pos {
            line: line_index,
            character: byte_offset_to_utf16_character(line, start),
        },
        Pos {
            line: line_index,
            character: byte_offset_to_utf16_character(line, cursor),
        },
    );
    ProviderOutcome::Exclusive(CoreCompletionList {
        items: render_table(at_meta_completion_labels(), CoreItemKind::Struct, range),
        is_incomplete: false,
    })
}

fn at_meta_edit_start(prefix: &str) -> Option<usize> {
    let trimmed = prefix.trim_start();
    if !trimmed.starts_with('@') || trimmed.chars().any(char::is_whitespace) {
        return None;
    }
    if trimmed.chars().skip(1).any(|ch| ch.is_ascii_digit()) {
        return None;
    }
    Some(prefix.rfind('@')? + 1)
}

fn at_meta_completion_labels() -> impl Iterator<Item = &'static DocEntry> {
    AT_META_COMPLETION_LABELS
        .iter()
        .filter_map(|label| AT_META.iter().find(|entry| entry.label == *label))
}

fn rate_offset_slot(line: &str, cursor: usize, line_index: u32) -> ProviderOutcome {
    let prefix = &line[..cursor];
    if !is_rate_offset_context(prefix) {
        return ProviderOutcome::NotApplicable;
    }
    let range = word_range_from_byte(line, line_index, cursor);
    ProviderOutcome::Exclusive(CoreCompletionList {
        items: render_table(RATE_OFFSET.iter(), CoreItemKind::Property, range),
        is_incomplete: false,
    })
}

fn is_rate_offset_context(prefix: &str) -> bool {
    let trimmed = prefix.trim_start();
    if !trimmed.ends_with(char::is_whitespace) {
        return false;
    }
    let tokens = tokenize_outside_double_quotes(trimmed);
    tokens.len() >= 3
        && is_at_number(&tokens[0])
        && tokens[1] == "pcm"
        && tokens[2].starts_with('"')
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct NoteRootContext {
    root_letter: char,
    root_accidental: Option<RootAccidental>,
    letter_byte: usize,
}

fn trailing_root(prefix: &str, allow_h: bool) -> Option<(char, Option<RootAccidental>, usize)> {
    let bytes = prefix.as_bytes();
    let (root_accidental, letter_end) = match bytes.last().copied() {
        Some(b'+') => (Some(RootAccidental::Sharp), bytes.len() - 1),
        Some(b'-') => (Some(RootAccidental::Flat), bytes.len() - 1),
        Some(b'=') => (Some(RootAccidental::Natural), bytes.len() - 1),
        _ => (None, bytes.len()),
    };
    let letter_byte = letter_end.checked_sub(1)?;
    let root_letter = match bytes[letter_byte] {
        byte @ b'a'..=b'g' => byte as char,
        byte @ b'A'..=b'G' => byte.to_ascii_lowercase() as char,
        b'h' if allow_h => 'h',
        b'H' if allow_h => 'h',
        _ => return None,
    };
    Some((root_letter, root_accidental, letter_byte))
}

fn chord_context(prefix: &str) -> Option<NoteRootContext> {
    let (root_letter, root_accidental, letter_byte) = trailing_root(prefix, true)?;
    let open_brace = letter_byte.checked_sub(1)?;
    if prefix.as_bytes()[open_brace] != b'{'
        || (open_brace > 0 && prefix.as_bytes()[open_brace - 1] == b'_')
    {
        return None;
    }
    Some(NoteRootContext {
        root_letter,
        root_accidental,
        letter_byte,
    })
}

fn chord_slot(
    text: &str,
    line: &str,
    cursor: usize,
    line_index: u32,
    settings: &CompletionSettings,
) -> ProviderOutcome {
    let Some(context) = chord_context(&line[..cursor]) else {
        return ProviderOutcome::NotApplicable;
    };
    if is_in_key_sig(line, context.letter_byte) {
        return ProviderOutcome::Exclusive(CoreCompletionList::empty());
    }

    let line_number = line_index + 1;
    let model = BorrowedLinesModel::from_text(text);
    if find_fm_block_at(&model, line_number).is_some()
        || find_psg_block_at(&model, line_number).is_some()
    {
        return ProviderOutcome::Exclusive(CoreCompletionList::empty());
    }

    let selector = find_enclosing_track_selector(&model, line_number);
    let channel_count = selector.as_ref().map_or(0, |value| value.spans.len());
    let allow_two = channel_count != 3 && channel_count != 4;
    let allow_three = channel_count != 2 && channel_count != 4;
    let allow_four = channel_count != 2 && channel_count != 3;
    let key_sig = scan_key_sig_at(&model, line_number, cursor as u32 + 1);
    let stack_up = settings.chord_stack_mode == ChordStackMode::StackUp;
    let channel_octaves = if stack_up {
        scan_channel_context_at(
            &model,
            line_number,
            context.letter_byte as u32,
            channel_count.max(3),
            None,
        )
        .octaves
    } else {
        Vec::new()
    };

    let has_close = line.as_bytes().get(cursor) == Some(&b'}');
    let close_suffix = if has_close { "" } else { "}" };
    let content_after_chord = if has_close {
        &line[cursor + 1..]
    } else {
        &line[cursor..]
    };
    let trimmed_after = content_after_chord.trim_start();
    let compensate = stack_up && !trimmed_after.is_empty() && !trimmed_after.starts_with(';');
    let range = EditRange::new(
        Pos {
            line: line_index,
            character: byte_offset_to_utf16_character(line, context.letter_byte),
        },
        Pos {
            line: line_index,
            character: byte_offset_to_utf16_character(line, cursor),
        },
    );
    let root_upper = context.root_letter.to_ascii_uppercase();
    let filter_prefix = format!("{root_upper}{}", accidental_char(context.root_accidental));
    let mut items = Vec::new();

    if allow_two {
        for dyad in DYADS {
            let body = if stack_up {
                render_stacked_generic_diatonic_dyad(
                    context.root_letter,
                    context.root_accidental,
                    dyad.step,
                    &key_sig,
                    &channel_octaves,
                    compensate,
                )
            } else {
                render_generic_diatonic_dyad(
                    context.root_letter,
                    context.root_accidental,
                    dyad.step,
                )
            };
            if let Some(body) = body {
                let label = format!("{root_upper}{}", dyad.name);
                let detail = format!("2-note dyad · {}", dyad.detail);
                let filter_text = format!("{filter_prefix}{}", dyad.name);
                let preselect = channel_count == 2 && dyad.name == "5";
                items.push(chord_item(
                    label,
                    body,
                    detail,
                    filter_text,
                    preselect,
                    range,
                    close_suffix,
                    items.len(),
                ));
            }
        }
    }

    for (allowed, size, size_number) in [
        (allow_three, ChordSize::Triad, 3usize),
        (allow_four, ChordSize::Seventh, 4usize),
    ] {
        if !allowed {
            continue;
        }
        let body = if stack_up {
            render_stacked_generic_chord(
                context.root_letter,
                context.root_accidental,
                size,
                &key_sig,
                &channel_octaves,
                compensate,
            )
        } else {
            render_generic_chord(context.root_letter, context.root_accidental, size)
        };
        if let Some(body) = body {
            items.push(chord_item(
                format!("Chord ({size_number} notes)"),
                body.clone(),
                format!("{filter_prefix}: {body}"),
                filter_prefix.clone(),
                true,
                range,
                close_suffix,
                items.len(),
            ));
        }
    }

    for (allowed, defs, size_number) in [
        (allow_three, CHORDS_3, 3usize),
        (allow_four, CHORDS_4, 4usize),
    ] {
        if !allowed {
            continue;
        }
        for def in defs {
            if def.suffix.is_empty() {
                continue;
            }
            let body = if stack_up {
                render_stacked_chord(
                    context.root_letter,
                    context.root_accidental,
                    def,
                    &key_sig,
                    &channel_octaves,
                    compensate,
                )
            } else {
                render_chord(context.root_letter, context.root_accidental, def, &key_sig)
            };
            if let Some(body) = body {
                items.push(chord_item(
                    format!("{root_upper}{}", def.suffix),
                    body,
                    format!("{size_number}-note chord · {}", def.detail),
                    format!("{filter_prefix}{}", def.suffix),
                    false,
                    range,
                    close_suffix,
                    items.len(),
                ));
            }
        }
    }

    ProviderOutcome::Exclusive(CoreCompletionList {
        items,
        is_incomplete: false,
    })
}

#[allow(clippy::too_many_arguments)]
fn chord_item(
    label: String,
    body: String,
    detail: String,
    filter_text: String,
    preselect: bool,
    edit_range: EditRange,
    close_suffix: &str,
    sort_index: usize,
) -> CoreItem {
    CoreItem {
        label,
        label_description: None,
        kind: CoreItemKind::Snippet,
        detail: Some(detail),
        documentation: Some(format!(
            "Inserts the ctrmml branch expression `{body}` for this chord."
        )),
        insert: InsertSpec {
            text: format!("{body}{close_suffix}"),
            format: InsertFormat::PlainText,
            as_is: false,
        },
        filter_text: Some(filter_text),
        sort_text: Some(format!("{sort_index:03}")),
        preselect,
        edit_range,
        additional_edits: Vec::new(),
        command: None,
    }
}

fn is_inside_open_brace(prefix: &str) -> bool {
    let mut depth = 0usize;
    for byte in prefix.bytes().rev() {
        match byte {
            b'}' => depth += 1,
            b'{' if depth == 0 => return true,
            b'{' => depth -= 1,
            _ => {}
        }
    }
    false
}

fn arpeggio_context(prefix: &str) -> Option<NoteRootContext> {
    let (root_letter, root_accidental, letter_byte) = trailing_root(prefix, false)?;
    let mut match_start = letter_byte;
    while match_start > 0 && matches!(prefix.as_bytes()[match_start - 1], b'<' | b'>') {
        match_start -= 1;
    }
    if (match_start > 0 && prefix.as_bytes()[match_start - 1] == b'{')
        || is_inside_open_brace(prefix)
    {
        return None;
    }
    Some(NoteRootContext {
        root_letter,
        root_accidental,
        letter_byte,
    })
}

fn arpeggio_slot(
    text: &str,
    line: &str,
    cursor: usize,
    line_index: u32,
    settings: &CompletionSettings,
) -> ProviderOutcome {
    if !settings.arpeggio_enabled {
        return ProviderOutcome::NotApplicable;
    }
    let Some(context) = arpeggio_context(&line[..cursor]) else {
        return ProviderOutcome::NotApplicable;
    };

    let line_number = line_index + 1;
    let model = BorrowedLinesModel::from_text(text);
    let key_sig = scan_key_sig_at(&model, line_number, cursor as u32 + 1);
    // §3.1-12/§3.1-22: arpeggio output is invariant under a uniform starting
    // octave shift, so avoid the channel-context scan and use a constant base.
    const STARTING_OCTAVE: i32 = 4;
    let range = EditRange::new(
        Pos {
            line: line_index,
            character: byte_offset_to_utf16_character(line, context.letter_byte),
        },
        Pos {
            line: line_index,
            character: byte_offset_to_utf16_character(line, cursor),
        },
    );
    let root_upper = context.root_letter.to_ascii_uppercase();
    let filter_prefix = format!("{root_upper}{}", accidental_char(context.root_accidental));
    let pattern = settings.arpeggio_pattern;
    let mut items = Vec::new();

    for size in [3usize, 4] {
        if let Some(body) = render_generic_arpeggio(
            context.root_letter,
            context.root_accidental,
            size,
            &key_sig,
            STARTING_OCTAVE,
            pattern,
        ) {
            items.push(arpeggio_item(
                format!("Arpeggio ({size} notes)"),
                body.clone(),
                format!("{filter_prefix}: {body} · {}", pattern.name()),
                filter_prefix.clone(),
                size == 3,
                range,
                items.len(),
            ));
        }
    }

    for (def, size) in chord_defs_for_arpeggio() {
        if let Some(body) = render_chord_arpeggio_with_pattern(
            context.root_letter,
            context.root_accidental,
            def,
            &key_sig,
            STARTING_OCTAVE,
            pattern,
        ) {
            items.push(arpeggio_item(
                format!("{root_upper}{}", def.suffix),
                body,
                format!("{size}-note arpeggio · {} · {}", def.detail, pattern.name()),
                format!("{filter_prefix}{}", def.suffix),
                false,
                range,
                items.len(),
            ));
        }
    }

    ProviderOutcome::Exclusive(CoreCompletionList {
        items,
        is_incomplete: false,
    })
}

#[allow(clippy::too_many_arguments)]
fn arpeggio_item(
    label: String,
    body: String,
    detail: String,
    filter_text: String,
    preselect: bool,
    edit_range: EditRange,
    sort_index: usize,
) -> CoreItem {
    CoreItem {
        label,
        label_description: None,
        kind: CoreItemKind::Snippet,
        detail: Some(detail),
        documentation: Some(format!(
            "Inserts the ctrmml note sequence `{body}` for this arpeggio."
        )),
        insert: InsertSpec {
            text: body,
            format: InsertFormat::PlainText,
            as_is: false,
        },
        filter_text: Some(filter_text),
        sort_text: Some(format!("{sort_index:03}")),
        preselect,
        edit_range,
        additional_edits: Vec::new(),
        command: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn done(text: &str, character: u32) -> CoreCompletionList {
        match completion_plan(
            text,
            Pos { line: 0, character },
            None,
            &CompletionSettings::default(),
        ) {
            CompletionPlan::Done(list) => list,
            CompletionPlan::NeedsData(request) => panic!("unexpected data request: {request:?}"),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn assert_item(
        item: &CoreItem,
        label: &str,
        kind: CoreItemKind,
        detail: &str,
        insert: &str,
        format: InsertFormat,
        filter_text: &str,
        sort_text: &str,
        start: u32,
        end: u32,
        command: Option<CoreCommand>,
    ) {
        assert_eq!(item.label, label);
        assert_eq!(item.label_description, None);
        assert_eq!(item.kind, kind);
        assert_eq!(item.detail.as_deref(), Some(detail));
        assert_eq!(item.insert.text, insert);
        assert_eq!(item.insert.format, format);
        assert!(!item.insert.as_is);
        assert_eq!(item.filter_text.as_deref(), Some(filter_text));
        assert_eq!(item.sort_text.as_deref(), Some(sort_text));
        assert!(!item.preselect);
        assert_eq!(
            item.edit_range.start,
            Pos {
                line: 0,
                character: start
            }
        );
        assert_eq!(
            item.edit_range.end,
            Pos {
                line: 0,
                character: end
            }
        );
        assert!(item.additional_edits.is_empty());
        assert_eq!(item.command, command);
    }

    #[test]
    fn settings_defaults_match_plan() {
        let settings = CompletionSettings::default();
        assert!(!settings.arpeggio_enabled);
        assert_eq!(settings.arpeggio_pattern, ArpeggioPattern::Up);
        assert_eq!(settings.chord_stack_mode, ChordStackMode::StackUp);
        assert!(!settings.fm_picker_hierarchy);
    }

    #[test]
    fn data_requests_use_c3_predicates_and_last_slash_fragment() {
        let hierarchy = CompletionSettings {
            fm_picker_hierarchy: true,
            ..CompletionSettings::default()
        };
        let fm_text = "@12 fm banks/sub/kit.dmp/Pa";
        assert_eq!(
            completion_plan(
                fm_text,
                Pos {
                    line: 0,
                    character: fm_text.len() as u32,
                },
                None,
                &hierarchy,
            ),
            CompletionPlan::NeedsData(DataRequest::FmPatches {
                fragment: Some("banks/sub/kit.dmp".to_string()),
            })
        );
        assert!(matches!(
            completion_plan(
                "@1 pcm",
                Pos {
                    line: 0,
                    character: 6,
                },
                None,
                &CompletionSettings::default(),
            ),
            CompletionPlan::NeedsData(DataRequest::PcmFiles)
        ));
        assert!(matches!(
            completion_plan(
                "@1 pcm \"drums/",
                Pos {
                    line: 0,
                    character: 14,
                },
                Some('/'),
                &CompletionSettings::default(),
            ),
            CompletionPlan::NeedsData(DataRequest::PcmPaths)
        ));
        assert!(matches!(
            completion_plan(
                "A c |",
                Pos {
                    line: 0,
                    character: 5,
                },
                Some('|'),
                &CompletionSettings::default(),
            ),
            CompletionPlan::NeedsData(DataRequest::CursorTick)
        ));

        assert!(!matches!(
            completion_plan(
                "@1 fmx",
                Pos {
                    line: 0,
                    character: 6,
                },
                None,
                &CompletionSettings::default(),
            ),
            CompletionPlan::NeedsData(_)
        ));
    }

    #[test]
    fn measure_fill_ctrl_space_requests_cursor_tick() {
        assert_eq!(
            completion_plan(
                "A o4 c4 |",
                Pos {
                    line: 0,
                    character: 9,
                },
                None,
                &CompletionSettings::default(),
            ),
            CompletionPlan::NeedsData(DataRequest::CursorTick)
        );
    }

    #[test]
    fn resolve_rejects_mismatched_payload() {
        let empty = completion_resolve(
            "@1 fm",
            Pos {
                line: 0,
                character: 5,
            },
            None,
            &CompletionSettings::default(),
            DataPayload::PcmFiles(Vec::new()),
        );
        assert!(empty.items.is_empty());
        assert!(!empty.is_incomplete);
    }

    #[test]
    fn hierarchy_single_patch_has_flat_content_and_path_sort() {
        let text = "@1 fm \n; ALG  FB\n    3   4\n\nA c";
        let position = Pos {
            line: 0,
            character: 6,
        };
        let patch = FmPatchData {
            rel_path: "inst/bass.dmp".to_string(),
            name: Some("Bass".to_string()),
            mml: "@1 fm ; Bass\n; ALG  FB\n    4   7\n".to_string(),
            has_macros: false,
        };
        let flat = completion_resolve(
            text,
            position,
            Some(' '),
            &CompletionSettings::default(),
            DataPayload::FmPatches(vec![patch.clone()]),
        );
        let hierarchy = CompletionSettings {
            fm_picker_hierarchy: true,
            ..CompletionSettings::default()
        };
        let collapsed = completion_resolve(
            text,
            position,
            Some(' '),
            &hierarchy,
            DataPayload::FmPatches(vec![patch]),
        );

        let flat_item = &flat.items[0];
        let collapsed_item = &collapsed.items[0];
        let mut expected_item = flat_item.clone();
        expected_item.sort_text = Some("inst/bass.dmp".to_string());
        assert_eq!(collapsed_item, &expected_item);
        assert!(!collapsed_item.additional_edits.is_empty());
    }

    #[test]
    fn hierarchy_fragment_zero_match_returns_template_escape() {
        let hierarchy = CompletionSettings {
            fm_picker_hierarchy: true,
            ..CompletionSettings::default()
        };
        let patch = FmPatchData {
            rel_path: "banks/kit.dmp".to_string(),
            name: Some("Kick".to_string()),
            mml: "@1 fm ; Kick\n  4 7\n".to_string(),
            has_macros: false,
        };

        let stale_text = "@1 fm stale/path/";
        let stale = completion_resolve(
            stale_text,
            Pos {
                line: 0,
                character: stale_text.len() as u32,
            },
            None,
            &hierarchy,
            DataPayload::FmPatches(vec![patch.clone()]),
        );
        assert_eq!(stale.items.len(), 1);
        assert_eq!(stale.items[0].label, "Default FM template");
    }

    #[test]
    fn hierarchy_fragment_nonempty_match_stays_template_free() {
        let hierarchy = CompletionSettings {
            fm_picker_hierarchy: true,
            ..CompletionSettings::default()
        };
        let patch = FmPatchData {
            rel_path: "banks/kit.dmp".to_string(),
            name: Some("Kick".to_string()),
            mml: "@1 fm ; Kick\n  4 7\n".to_string(),
            has_macros: false,
        };

        let matching_text = "@1 fm banks/kit.dmp/";
        let matching = completion_resolve(
            matching_text,
            Pos {
                line: 0,
                character: matching_text.len() as u32,
            },
            None,
            &hierarchy,
            DataPayload::FmPatches(vec![patch]),
        );
        assert_eq!(matching.items.len(), 1);
        assert_eq!(matching.items[0].label, "Kick");
        assert!(matching
            .items
            .iter()
            .all(|item| item.label != "Default FM template"));
    }

    #[test]
    fn fm_resolver_skips_only_degenerate_payload_entries() {
        let patch = |rel_path: &str, mml: &str| FmPatchData {
            rel_path: rel_path.to_string(),
            name: None,
            mml: mml.to_string(),
            has_macros: false,
        };
        let list = completion_resolve(
            "@1 fm ",
            Pos {
                line: 0,
                character: 6,
            },
            Some(' '),
            &CompletionSettings::default(),
            DataPayload::FmPatches(vec![
                patch("", "@1 fm\n  1\n"),
                patch("a/", "@1 fm\n  2\n"),
                patch("lead", "@1 fm\n  3\n"),
                patch("blank.dmp", " \n\t "),
                patch("empty.dmp", ""),
            ]),
        );

        assert_eq!(
            list.items
                .iter()
                .map(|item| item.label.as_str())
                .collect::<Vec<_>>(),
            vec!["a/", "lead", "Default FM template"]
        );
        assert_eq!(list.items[0].detail.as_deref(), Some("instrument"));
        assert_eq!(list.items[1].detail.as_deref(), Some("instrument"));
        assert_eq!(list.items[0].sort_text.as_deref(), Some("0000"));
        assert_eq!(list.items[1].sort_text.as_deref(), Some("0001"));
    }

    #[test]
    fn hierarchy_two_patch_file_drills_down() {
        let hierarchy = CompletionSettings {
            fm_picker_hierarchy: true,
            ..CompletionSettings::default()
        };
        let list = completion_resolve(
            "@1 fm ",
            Pos {
                line: 0,
                character: 6,
            },
            Some(' '),
            &hierarchy,
            DataPayload::FmPatches(vec![
                FmPatchData {
                    rel_path: "banks/kit.dmp".to_string(),
                    name: Some("Kick".to_string()),
                    mml: "@1 fm ; Kick\n  4 7\n".to_string(),
                    has_macros: false,
                },
                FmPatchData {
                    rel_path: "banks/kit.dmp".to_string(),
                    name: Some("Snare".to_string()),
                    mml: "@1 fm ; Snare\n  3 2\n".to_string(),
                    has_macros: false,
                },
            ]),
        );

        assert_eq!(list.items.len(), 2);
        let item = &list.items[0];
        assert_eq!(item.label, "banks/kit.dmp");
        assert_eq!(item.label_description.as_deref(), Some("2"));
        assert_eq!(item.kind, CoreItemKind::File);
        assert_eq!(item.insert.text, " banks/kit.dmp/");
        assert_eq!(item.command, Some(CoreCommand::TriggerSuggest));
    }

    #[test]
    fn hierarchy_mixed_payload_matches_fixture_order() {
        let hierarchy = CompletionSettings {
            fm_picker_hierarchy: true,
            ..CompletionSettings::default()
        };
        let list = completion_resolve(
            "@1 fm ",
            Pos {
                line: 0,
                character: 6,
            },
            Some(' '),
            &hierarchy,
            DataPayload::FmPatches(vec![
                FmPatchData {
                    rel_path: "inst/bass.dmp".to_string(),
                    name: Some("Bass".to_string()),
                    mml: "@1 fm ; Bass\n; ALG  FB\n    4   7\n".to_string(),
                    has_macros: false,
                },
                FmPatchData {
                    rel_path: "inst/multi.dmp".to_string(),
                    name: Some("Pad".to_string()),
                    mml: "@1 fm ; Pad\n; ALG  FB\n    2   0\n".to_string(),
                    has_macros: false,
                },
                FmPatchData {
                    rel_path: "inst/multi.dmp".to_string(),
                    name: Some("Brass".to_string()),
                    mml: "@1 fm ; Brass\n; ALG  FB\n    6   1\n".to_string(),
                    has_macros: true,
                },
            ]),
        );

        assert_eq!(
            list.items
                .iter()
                .map(|item| (item.label.as_str(), item.kind))
                .collect::<Vec<_>>(),
            vec![
                ("inst/bass.dmp — Bass", CoreItemKind::Snippet),
                ("inst/multi.dmp", CoreItemKind::File),
                ("Default FM template", CoreItemKind::Snippet),
            ]
        );
        assert_eq!(list.items[0].sort_text.as_deref(), Some("inst/bass.dmp"));
        assert_eq!(list.items[1].sort_text.as_deref(), Some("inst/multi.dmp"));
    }

    #[test]
    fn instrument_param_collection_preserves_ts_stop_and_eof_rules() {
        let edit = collect_instrument_param_edit(
            "@1 fm\n\n; header\n; 'platform'\n  3, 4 ; params\n\nA c",
            0,
        );
        assert_eq!(
            edit,
            vec![CoreTextEdit {
                range: EditRange::new(
                    Pos {
                        line: 1,
                        character: 0,
                    },
                    Pos {
                        line: 5,
                        character: 0,
                    },
                ),
                new_text: String::new(),
            }]
        );

        let eof = collect_instrument_param_edit("@1 fm\n  3, 4", 0);
        assert_eq!(
            eof[0].range.end,
            Pos {
                line: 1,
                character: 6
            }
        );
        assert!(collect_instrument_param_edit("@1 fm\n; header\nA c", 0).is_empty());
    }

    #[test]
    fn instrument_param_collection_accepts_unindented_digit_rows() {
        let edit = collect_instrument_param_edit("@1 fm\n; ALG  FB\n3 4\n31 0 0 5\n\nA c", 0);
        assert_eq!(
            edit,
            vec![CoreTextEdit {
                range: EditRange::new(
                    Pos {
                        line: 1,
                        character: 0,
                    },
                    Pos {
                        line: 4,
                        character: 0,
                    },
                ),
                new_text: String::new(),
            }]
        );
    }

    #[test]
    fn instrument_param_collection_stops_at_digit_track_with_note_letters() {
        let stopped = collect_instrument_param_edit("@1 fm\n3 4\n4 c4 d e\n5 6", 0);
        assert_eq!(
            stopped[0].range.end,
            Pos {
                line: 2,
                character: 0,
            }
        );
    }

    #[test]
    fn partial_settings_deserialization_uses_rust_defaults() {
        use serde::de::value::{BoolDeserializer, Error, MapDeserializer, StringDeserializer};
        use serde::Deserialize as _;

        let fields = [(
            StringDeserializer::<Error>::new("arpeggio_enabled".to_string()),
            BoolDeserializer::<Error>::new(true),
        )];
        let settings = CompletionSettings::deserialize(MapDeserializer::new(fields.into_iter()))
            .expect("partial settings should deserialize");

        assert!(settings.arpeggio_enabled);
        assert_eq!(settings.arpeggio_pattern, ArpeggioPattern::Up);
        assert_eq!(settings.chord_stack_mode, ChordStackMode::StackUp);
        assert!(!settings.fm_picker_hierarchy);
    }

    #[test]
    fn utf16_and_byte_offsets_round_trip_at_every_scalar_boundary() {
        let line = "A 日本語 😀 ドラム";
        let mut utf16 = 0u32;
        assert_eq!(utf16_character_to_byte_offset(line, utf16), 0);
        for (byte, ch) in line.char_indices() {
            assert_eq!(byte_offset_to_utf16_character(line, byte), utf16);
            assert_eq!(utf16_character_to_byte_offset(line, utf16), byte);
            utf16 += ch.len_utf16() as u32;
        }
        assert_eq!(utf16_character_to_byte_offset(line, utf16), line.len());
        assert_eq!(byte_offset_to_utf16_character(line, line.len()), utf16);
    }

    #[test]
    fn utf16_helpers_clamp_invalid_and_mid_scalar_offsets() {
        let line = "あ😀い";
        assert_eq!(utf16_character_to_byte_offset(line, 2), 3);
        assert_eq!(byte_offset_to_utf16_character(line, 2), 0);
        assert_eq!(utf16_character_to_byte_offset(line, 999), line.len());
        assert_eq!(byte_offset_to_utf16_character(line, 999), 4);
    }

    #[test]
    fn word_range_is_ascii_run_before_cursor() {
        let range = word_range_before_cursor(
            "日本 foo_12",
            Pos {
                line: 4,
                character: 9,
            },
        );
        assert_eq!(
            range.start,
            Pos {
                line: 4,
                character: 3
            }
        );
        assert_eq!(
            range.end,
            Pos {
                line: 4,
                character: 9
            }
        );
    }

    #[test]
    fn platform_commands_match_fixture_shape() {
        let list = done("A '", 3);
        assert!(!list.is_incomplete);
        assert_eq!(list.items.len(), 37);
        let first = &list.items[0];
        assert_eq!(first.label, "fm3 <mask>");
        assert_eq!(first.kind, CoreItemKind::Function);
        assert_eq!(first.detail.as_deref(), Some("FM3 special mode."));
        assert!(first
            .documentation
            .as_deref()
            .unwrap()
            .contains("Enable FM3 special mode for selected operators"));
        assert_eq!(first.insert.text, "fm3 0000");
        assert_eq!(first.insert.format, InsertFormat::PlainText);
        assert_eq!(first.filter_text.as_deref(), Some("fm3"));
        assert_eq!(first.sort_text.as_deref(), Some("000"));
        assert_eq!(first.edit_range.start.character, 3);
        assert_eq!(first.edit_range.end.character, 3);

        assert_eq!(list.items[7].label, "write <register> <data>");
        assert_eq!(list.items[7].sort_text.as_deref(), Some("007"));
        assert_eq!(list.items[36].label, "ssg4 <value>");
        assert_eq!(list.items[36].sort_text.as_deref(), Some("036"));
    }

    #[test]
    fn platform_command_range_uses_last_inner_whitespace() {
        let item = &done("A 'fm3 000", 10).items[0];
        assert_eq!(item.edit_range.start.character, 7);
        assert_eq!(item.edit_range.end.character, 10);
    }

    #[test]
    fn platform_command_range_is_utf16_with_multibyte_prefix() {
        let item = &done("A 'あ fm", 7).items[0];
        assert_eq!(item.edit_range.start.character, 5);
        assert_eq!(item.edit_range.end.character, 7);
    }

    #[test]
    fn meta_keywords_match_fixture_exactly() {
        let list = done("#ti", 3);
        assert!(!list.is_incomplete);
        let expected = [
            ("#title", "Song title.", "title", false),
            ("#composer", "Original composer.", "composer", false),
            ("#programmer", "MML programmer.", "programmer", false),
            ("#author", "Song metadata.", "author", false),
            ("#date", "Song metadata.", "date", false),
            ("#comment", "Song metadata.", "comment", false),
            (
                "#platform",
                "Choose the Mega Drive playback mode.",
                "platform",
                true,
            ),
            ("#option", "Sets platform options.", "option", true),
            ("#group", "MDSDRV song group.", "group", true),
            ("#game", "Song metadata.", "game", false),
            ("#composerj", "Song metadata.", "composerj", false),
            ("#license", "License information.", "license", false),
            (
                "#timesig",
                "Time signature for piano roll.",
                "timesig",
                true,
            ),
        ];
        assert_eq!(list.items.len(), expected.len());
        for (index, (item, (label, detail, insert, triggers))) in
            list.items.iter().zip(expected).enumerate()
        {
            assert_item(
                item,
                label,
                CoreItemKind::Keyword,
                detail,
                insert,
                InsertFormat::PlainText,
                label,
                &format!("{index:03}"),
                1,
                3,
                triggers.then_some(CoreCommand::TriggerSuggest),
            );
        }
        assert!(list.items[0]
            .documentation
            .as_deref()
            .unwrap()
            .contains("Set the song title."));
        assert_eq!(list.items[3].documentation, None);

        let fallback = done("x #ti", 5);
        assert!(fallback.is_incomplete);
        assert_eq!(fallback.items.len(), 37);
        assert_eq!(fallback.items[0].edit_range.start.character, 3);
        assert_eq!(fallback.items[0].edit_range.end.character, 5);
    }

    #[test]
    fn meta_keyword_predicate_rejects_nonleading_hash_and_whitespace() {
        assert_eq!(meta_keyword_hash("  #plat"), Some(2));
        assert_eq!(meta_keyword_hash("x #plat"), None);
        assert_eq!(meta_keyword_hash("#platform "), None);
        assert_eq!(meta_keyword_hash("platform"), None);
        assert_eq!(meta_keyword_hash("#ti#tle"), None);
        assert_eq!(meta_keyword_hash("##"), None);

        for (text, character) in [("#ti#tle", 7), ("##", 2)] {
            let list = done(text, character);
            assert!(!list.is_incomplete, "{text}");
            assert!(list.items.is_empty(), "{text}");
        }
    }

    #[test]
    fn meta_values_match_all_fixture_tables_and_live_word_range() {
        let cases: [(&str, u32, &[(&str, &str)]); 4] = [
            (
                "#platform ",
                10,
                &[
                    ("megadrive", "Mega Drive / Genesis"),
                    ("mdsdrv", "MDSDRV sound driver"),
                ],
            ),
            (
                "#option ",
                8,
                &[("noextpitch", "Disable extended pitch envelopes")],
            ),
            (
                "#timesig ",
                9,
                &[
                    ("3/4", "Three beats per measure."),
                    ("4/4", "Four beats per measure (default)."),
                    ("5/4", "Five beats per measure."),
                    ("6/8", "Six eighth-note beats per measure."),
                    ("no", "No measure lines."),
                ],
            ),
            (
                "#group ",
                7,
                &[
                    ("bgm", "Background music group."),
                    ("se", "Sound effect group."),
                ],
            ),
        ];
        for (text, character, expected) in cases {
            let list = done(text, character);
            assert!(!list.is_incomplete, "{text}");
            assert_eq!(list.items.len(), expected.len(), "{text}");
            for (index, (item, (label, detail))) in list.items.iter().zip(expected).enumerate() {
                assert_item(
                    item,
                    label,
                    CoreItemKind::Value,
                    detail,
                    label,
                    InsertFormat::PlainText,
                    label,
                    &format!("{index:03}"),
                    character,
                    character,
                    None,
                );
            }
        }
        assert!(done("#platform ", 10).items[0]
            .documentation
            .as_deref()
            .unwrap()
            .contains("Use raw VGM DAC stream playback for PCM"));
        assert!(done("#timesig ", 9).items[4]
            .documentation
            .as_deref()
            .unwrap()
            .contains("disables the `|` bar-line rest fill"));

        let live = done("#platform me", 12);
        assert_eq!(live.items.len(), 2);
        assert!(live.items.iter().all(|item| item.edit_range
            == EditRange::new(
                Pos {
                    line: 0,
                    character: 10
                },
                Pos {
                    line: 0,
                    character: 12
                },
            )));

        let timesig = done("#timesig 3/", 11);
        assert_eq!(timesig.items.len(), 5);
        assert!(timesig.items.iter().all(|item| item.edit_range
            == EditRange::new(
                Pos {
                    line: 0,
                    character: 9
                },
                Pos {
                    line: 0,
                    character: 11
                },
            )));

        let platform = done("#platform mega-", 15);
        assert_eq!(platform.items.len(), 2);
        assert!(platform.items.iter().all(|item| item.edit_range
            == EditRange::new(
                Pos {
                    line: 0,
                    character: 10
                },
                Pos {
                    line: 0,
                    character: 15
                },
            )));
    }

    #[test]
    fn meta_value_predicate_requires_exact_keyword_then_whitespace() {
        assert!(is_meta_value_context("  #platform me", "#platform"));
        assert!(!is_meta_value_context("#platform", "#platform"));
        assert!(!is_meta_value_context("#platformx ", "#platform"));
        assert!(!is_meta_value_context("#option value", "#platform"));
    }

    #[test]
    fn at_meta_matches_fixture_exactly_and_has_negative_cases() {
        let list = done("@M", 2);
        assert!(!list.is_incomplete);
        assert_eq!(list.items.len(), 2);
        assert_item(
            &list.items[0],
            "@<num>",
            CoreItemKind::Struct,
            "Instrument definition",
            "${1:num}",
            InsertFormat::Snippet,
            "@<num>",
            "000",
            1,
            2,
            None,
        );
        assert_item(
            &list.items[1],
            "@M<num>",
            CoreItemKind::Struct,
            "Pitch envelope",
            "M${1:num}",
            InsertFormat::Snippet,
            "@M<num>",
            "001",
            1,
            2,
            None,
        );
        assert!(list.items[0]
            .documentation
            .as_deref()
            .unwrap()
            .contains("Define an instrument table."));
        assert!(list.items[1]
            .documentation
            .as_deref()
            .unwrap()
            .contains("Define a pitch envelope table."));
        assert_eq!(at_meta_edit_start("  @M"), Some(3));
        assert_eq!(at_meta_edit_start("x @M"), None);
        assert_eq!(at_meta_edit_start("@M1"), None);
        assert_eq!(at_meta_edit_start("@M "), None);

        for (text, cursor, start) in [("@", 1, 1), ("@M", 2, 1), ("@E", 2, 1), ("@@", 2, 2)] {
            let list = done(text, cursor);
            assert_eq!(list.items.len(), 2, "{text}");
            assert!(
                list.items.iter().all(|item| item.edit_range
                    == EditRange::new(
                        Pos {
                            line: 0,
                            character: start
                        },
                        Pos {
                            line: 0,
                            character: cursor
                        },
                    )),
                "{text}"
            );
        }
    }

    #[test]
    fn instrument_types_match_fixture_exactly() {
        let list = done("@1 ", 3);
        assert!(list.is_incomplete);
        let expected = [
            (
                "pcm",
                "PCM sample instrument",
                "pcm ",
                InsertFormat::PlainText,
                true,
            ),
            (
                "fm",
                "FM synthesis instrument",
                "fm ",
                InsertFormat::PlainText,
                true,
            ),
            (
                "psg",
                "PSG envelope instrument",
                "psg ",
                InsertFormat::PlainText,
                false,
            ),
            (
                "2op",
                "2-operator FM instrument",
                "2op   ${1:2}   ${2:5}   ${3:5}   ${4:4}   ${5:4}   ${6:0}",
                InsertFormat::Snippet,
                false,
            ),
        ];
        assert_eq!(list.items.len(), expected.len());
        for (index, (item, (label, detail, insert, format, triggers))) in
            list.items.iter().zip(expected).enumerate()
        {
            assert_item(
                item,
                label,
                CoreItemKind::TypeParameter,
                detail,
                insert,
                format,
                label,
                &format!("{index:03}"),
                3,
                3,
                triggers.then_some(CoreCommand::TriggerSuggest),
            );
        }
        assert!(list.items[0]
            .documentation
            .as_deref()
            .unwrap()
            .contains("PCM samples are defined as instruments."));
        assert!(list.items[1]
            .documentation
            .as_deref()
            .unwrap()
            .contains("FM instruments are defined with ALG"));
        assert!(list.items[2]
            .documentation
            .as_deref()
            .unwrap()
            .contains("PSG instruments (envelopes) are defined"));
        assert!(list.items[3]
            .documentation
            .as_deref()
            .unwrap()
            .contains("Create a derived FM instrument"));
    }

    #[test]
    fn instrument_type_predicate_stays_live_but_rejects_fmx() {
        let live = done("@1 f", 4);
        assert!(live.is_incomplete);
        assert_eq!(live.items.len(), 4);
        assert_eq!(live.items[1].label, "fm");
        assert_eq!(live.items[1].edit_range.start.character, 3);
        assert_eq!(live.items[1].edit_range.end.character, 4);
        assert_eq!(live.items[1].command, Some(CoreCommand::TriggerSuggest));

        let fmx = done("@1 fmx", 6);
        assert!(!fmx.is_incomplete);
        assert!(fmx.items.is_empty());
        assert!(!is_instrument_type_context("@1 fmx"));
        assert!(!is_rate_offset_context("@1 fmx"));

        assert!(!is_instrument_type_context("@ "));
        assert!(!is_instrument_type_context("@1"));
        assert!(!is_instrument_type_context("@1\t"));
        assert!(!is_instrument_type_context("@1 xyz"));
    }

    #[test]
    fn rate_offset_matches_both_fixtures_and_quote_aware_shape() {
        for (text, character) in [
            ("@1 pcm \"drums/kick.wav\" ", 24),
            ("@1 pcm \"drums/kick.wav\" rate=8000 ", 34),
        ] {
            let list = done(text, character);
            assert!(!list.is_incomplete);
            assert_eq!(list.items.len(), 2);
            assert_item(
                &list.items[0],
                "rate=<num>",
                CoreItemKind::Property,
                "Override the sample rate.",
                "rate=${1:<num>}",
                InsertFormat::Snippet,
                "rate=",
                "000",
                character,
                character,
                None,
            );
            assert_item(
                &list.items[1],
                "offset=<num>",
                CoreItemKind::Property,
                "Adjust the start position.",
                "offset=${1:<num>}",
                InsertFormat::Snippet,
                "offset=",
                "001",
                character,
                character,
                None,
            );
            assert!(list.items[0]
                .documentation
                .as_deref()
                .unwrap()
                .contains("Override the sample rate of a PCM instrument."));
            assert!(list.items[1]
                .documentation
                .as_deref()
                .unwrap()
                .contains("Adjust the start position of a PCM instrument"));
        }
        assert!(is_rate_offset_context("@1 pcm \"a b.wav\" "));
    }

    #[test]
    fn rate_offset_predicate_rejects_wrong_shape() {
        assert!(!is_rate_offset_context("@1 pcm \"a.wav\""));
        assert!(!is_rate_offset_context("@1 pcm "));
        assert!(!is_rate_offset_context("1 pcm \"a.wav\" "));
        assert!(!is_rate_offset_context("@1 fm \"a.wav\" "));
        assert!(!is_rate_offset_context("@1 pcm a.wav "));
    }

    #[test]
    fn command_fallback_matches_fixture_shape_and_word_range() {
        let empty_word_list = done("A ", 2);
        assert_eq!(empty_word_list.items[0].edit_range.start.character, 2);
        assert_eq!(empty_word_list.items[0].edit_range.end.character, 2);

        let list = done("A o", 3);
        assert!(list.is_incomplete);
        assert_eq!(list.items.len(), 37);
        assert_eq!(list.items[0].label, "cdefgabh");
        assert_eq!(list.items[0].kind, CoreItemKind::Function);
        assert_eq!(list.items[0].insert.format, InsertFormat::PlainText);
        assert_eq!(list.items[0].filter_text.as_deref(), Some("notes"));
        assert_eq!(list.items[0].sort_text.as_deref(), Some("000"));
        assert_eq!(list.items[0].edit_range.start.character, 2);
        assert_eq!(list.items[0].edit_range.end.character, 3);
        assert_eq!(list.items[1].insert.text, "r${1:duration}");
        assert_eq!(list.items[1].insert.format, InsertFormat::Snippet);
        assert_eq!(list.items[36].insert.format, InsertFormat::PlainText);
        assert_eq!(list.items[36].sort_text.as_deref(), Some("036"));
    }

    #[test]
    fn command_fallback_is_safe_after_multibyte_track_text() {
        let list = done("A ドラム ", 6);
        assert!(list.is_incomplete);
        assert_eq!(list.items.len(), 37);
    }

    #[test]
    fn out_of_range_line_is_an_empty_exclusive_result() {
        match completion_plan(
            "A c4",
            Pos {
                line: 9999,
                character: 0,
            },
            None,
            &CompletionSettings::default(),
        ) {
            CompletionPlan::Done(list) => {
                assert!(!list.is_incomplete);
                assert!(list.items.is_empty());
            }
            CompletionPlan::NeedsData(request) => {
                panic!("unexpected data request: {request:?}")
            }
        }
    }

    #[test]
    fn comment_guard_is_quote_aware_and_utf16_safe() {
        assert!(done("A c4 ; 日本語の注釈", 13).items.is_empty());
        let list = done("A c 'sr1 0;0' ", 14);
        assert_eq!(list.items.len(), 37);
        assert!(list.is_incomplete);
    }

    #[test]
    fn key_signature_guard_is_exclusive() {
        assert!(done("A _{F", 5).items.is_empty());
    }

    #[test]
    fn chord_provider_claims_fm_block_rejection_with_empty_exclusive_outcome() {
        let text = "@1 fm\n{c";
        let outcome = chord_slot(text, "{c", 2, 1, &CompletionSettings::default());
        assert!(matches!(
            outcome,
            ProviderOutcome::Exclusive(list) if list.items.is_empty() && !list.is_incomplete
        ));
    }

    #[test]
    fn leading_track_selector_guard_is_exclusive() {
        assert!(done("AB c4", 2).items.is_empty());
    }

    #[test]
    fn note_and_rest_suppression_matches_ts_suffix_semantics() {
        for (text, character) in [
            ("A c", 3),
            ("A c8.", 5),
            ("A c:12", 6),
            ("A >c", 4),
            ("A r4", 4),
            ("A r:12", 6),
        ] {
            assert!(done(text, character).items.is_empty(), "{text}");
        }

        for (text, character) in [("A r4x", 4), ("A rx", 3)] {
            let list = done(text, character);
            assert!(list.is_incomplete, "{text}");
            assert_eq!(list.items.len(), 37, "{text}");
        }
    }

    #[test]
    fn hash_and_at_prefix_bails_are_exclusive() {
        assert!(done("#title My Song", 14).items.is_empty());
        assert!(done("@1 xyz", 6).items.is_empty());
    }

    #[test]
    fn resolve_reruns_stage_one_cascade_for_any_payload() {
        let settings = CompletionSettings::default();
        let list = completion_resolve(
            "A ",
            Pos {
                line: 0,
                character: 2,
            },
            None,
            &settings,
            DataPayload::PcmPaths(vec!["kick.wav".to_string()]),
        );
        assert_eq!(list.items.len(), 37);
        assert!(list.is_incomplete);
    }
}
