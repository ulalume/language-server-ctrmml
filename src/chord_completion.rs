//! Chord completion inside `{<letter>...}` conditional blocks.
//!
//! Mirrors the gating in `web-ctrmml/src/editor/mml-completions.ts`:
//! must be on a track-content line (enclosing `A`/`B`/... selector),
//! outside any FM/PSG instrument block, and the prefix must end with
//! `{<letter>[+\-=]?` where the `{` is not the key-sig opener `_{`.
//! 3-channel tracks (`ABC`) show only triads, 4-channel (`ABCD`) only
//! sevenths; other counts show both.

use ctrmml_lang_core::{
    chord::{
        render_chord, render_generic_chord, ChordDef, ChordSize, RootAccidental, CHORDS_3,
        CHORDS_4,
    },
    find_enclosing_track_selector, find_fm_block_at, find_psg_block_at, is_in_key_sig,
    scan_key_sig_at, LinesModel,
};
use tower_lsp::lsp_types::{
    CompletionItem, CompletionItemKind, CompletionTextEdit, Documentation, Position, Range,
    TextEdit,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ChordContext {
    root_letter: char,
    root_accidental: Option<RootAccidental>,
    /// 0-based column of the root letter (the character after `{`).
    letter_col: u32,
}

/// Equivalent of the JS regex `(?<!_)\{([a-hA-H])([+\-=]?)$` against the
/// line prefix up to `col` (0-based). The `_{...}` rejection is left to
/// `is_in_key_sig` at the call site so unterminated key-sig blocks
/// (`_{F {c`) are also handled.
fn detect_chord_context(line: &str, col: u32) -> Option<ChordContext> {
    let prefix = line.get(..col as usize)?;
    let bytes = prefix.as_bytes();
    let n = bytes.len();
    if n < 2 {
        return None;
    }

    let (acc_byte, after_letter_idx) = match bytes[n - 1] {
        b'+' | b'-' | b'=' => (Some(bytes[n - 1]), n - 1),
        _ => (None, n),
    };
    if after_letter_idx < 2 {
        return None;
    }

    let letter_idx = after_letter_idx - 1;
    let letter_byte = bytes[letter_idx];
    let letter_lc = match letter_byte {
        b'a'..=b'h' => letter_byte as char,
        b'A'..=b'H' => (letter_byte + 32) as char,
        _ => return None,
    };

    if bytes[letter_idx - 1] != b'{' {
        return None;
    }

    let root_accidental = match acc_byte {
        Some(b'+') => Some(RootAccidental::Sharp),
        Some(b'-') => Some(RootAccidental::Flat),
        Some(b'=') => Some(RootAccidental::Natural),
        _ => None,
    };

    Some(ChordContext {
        root_letter: letter_lc,
        root_accidental,
        letter_col: letter_idx as u32,
    })
}

fn accidental_label(acc: Option<RootAccidental>) -> &'static str {
    match acc {
        Some(RootAccidental::Sharp) => "+",
        Some(RootAccidental::Flat) => "-",
        Some(RootAccidental::Natural) => "=",
        None => "",
    }
}

/// Build chord-completion items at the cursor.
///
/// Returns `None` when chord completion does not apply — the caller
/// falls through to the normal command completion path. `Some(vec)`
/// when the cursor is in a `{<letter>...` context (the vec may still
/// be empty if rendering produced nothing).
pub(crate) fn chord_completion_items(
    doc_text: &str,
    line_zero_based: u32,
    character: u32,
) -> Option<Vec<CompletionItem>> {
    // Cheap byte walk on the current line first — the common keystroke
    // bails here without touching the rest of the document.
    let line_text = doc_text
        .split('\n')
        .nth(line_zero_based as usize)
        .map(|s| s.strip_suffix('\r').unwrap_or(s))?;
    let ctx = detect_chord_context(line_text, character)?;

    // Reject `_{...}` (key signature block) — covers the simple
    // `_{c` case and unterminated `_{F {c` runs alike.
    if is_in_key_sig(line_text, ctx.letter_col as usize) {
        return None;
    }

    let line_one_based = line_zero_based + 1;
    let model = LinesModel::from_text(doc_text);

    if find_fm_block_at(&model, line_one_based).is_some() {
        return None;
    }
    if find_psg_block_at(&model, line_one_based).is_some() {
        return None;
    }
    let selector = find_enclosing_track_selector(&model, line_one_based)?;
    let key_sig = scan_key_sig_at(&model, line_one_based, character + 1);

    let num_channels = selector.spans.len();
    let allow3 = num_channels != 4;
    let allow4 = num_channels != 3;

    let range = Range {
        start: Position::new(line_zero_based, ctx.letter_col),
        end: Position::new(line_zero_based, character),
    };
    let next_char = line_text
        .get(character as usize..)
        .and_then(|s| s.chars().next());
    let close_suffix = if next_char == Some('}') { "" } else { "}" };

    let root_upper = ctx.root_letter.to_ascii_uppercase();
    let filter_prefix = format!("{root_upper}{}", accidental_label(ctx.root_accidental));

    fn make_item(
        label: String,
        body: String,
        detail: String,
        filter_text: String,
        preselect: bool,
        sort_idx: usize,
        range: Range,
        close_suffix: &str,
    ) -> CompletionItem {
        let insert_text = format!("{body}{close_suffix}");
        CompletionItem {
            label,
            kind: Some(CompletionItemKind::SNIPPET),
            detail: Some(detail),
            filter_text: Some(filter_text),
            preselect: Some(preselect),
            sort_text: Some(format!("{sort_idx:03}")),
            insert_text: Some(insert_text.clone()),
            text_edit: Some(CompletionTextEdit::Edit(TextEdit {
                range,
                new_text: insert_text,
            })),
            documentation: Some(Documentation::String(format!(
                "Inserts the ctrmml branch expression `{body}` for this chord."
            ))),
            ..CompletionItem::default()
        }
    }

    let mut items: Vec<CompletionItem> = Vec::new();

    // Generic items first so the simple `{c` case lands on a key-sig-aware
    // diatonic triad/seventh rather than the first named definition.
    let add_generic = |items: &mut Vec<CompletionItem>, size: ChordSize, size_n: u32| {
        if let Some(body) = render_generic_chord(ctx.root_letter, ctx.root_accidental, size) {
            let item = make_item(
                format!("Chord ({size_n} notes)"),
                body.clone(),
                format!("{filter_prefix}: {body}"),
                filter_prefix.clone(),
                true,
                items.len(),
                range,
                close_suffix,
            );
            items.push(item);
        }
    };
    if allow3 {
        add_generic(&mut items, ChordSize::Triad, 3);
    }
    if allow4 {
        add_generic(&mut items, ChordSize::Seventh, 4);
    }

    let add_named = |items: &mut Vec<CompletionItem>, defs: &[ChordDef], size_n: u32| {
        for def in defs {
            // Bare-letter major triad duplicates the generic triad above.
            if def.suffix.is_empty() {
                continue;
            }
            if let Some(body) = render_chord(ctx.root_letter, ctx.root_accidental, def, &key_sig) {
                let item = make_item(
                    format!("{root_upper}{}", def.suffix),
                    body,
                    format!("{size_n}-note chord · {}", def.detail),
                    format!("{filter_prefix}{}", def.suffix),
                    false,
                    items.len(),
                    range,
                    close_suffix,
                );
                items.push(item);
            }
        }
    };
    if allow3 {
        add_named(&mut items, CHORDS_3, 3);
    }
    if allow4 {
        add_named(&mut items, CHORDS_4, 4);
    }

    Some(items)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn has_detail(items: &[CompletionItem], n: u32, def_detail: &str) -> bool {
        let want = format!("{n}-note chord · {def_detail}");
        items.iter().any(|it| it.detail.as_deref() == Some(&want))
    }

    #[test]
    fn no_chord_outside_brace() {
        let doc = "A a";
        assert!(chord_completion_items(doc, 0, 3).is_none());
    }

    #[test]
    fn fires_inside_brace() {
        let doc = "ABC {d";
        let items = chord_completion_items(doc, 0, 6).expect("chord context");
        assert!(items.iter().any(|it| it.label == "Chord (3 notes)"));
        assert!(items.iter().any(|it| it.label == "Dm"));
        assert!(!items.iter().any(|it| it.label == "DM7"));
        assert!(!items.iter().any(|it| it.label == "Chord (4 notes)"));
    }

    #[test]
    fn three_channels_filters_to_triads() {
        let doc = "ABC {c";
        let items = chord_completion_items(doc, 0, 6).expect("chord context");
        for def in CHORDS_3.iter().filter(|d| !d.suffix.is_empty()) {
            assert!(has_detail(&items, 3, def.detail), "missing {}", def.detail);
        }
        for def in CHORDS_4 {
            assert!(!has_detail(&items, 4, def.detail), "unexpected {}", def.detail);
        }
    }

    #[test]
    fn four_channels_filters_to_sevenths() {
        let doc = "ABCD {c";
        let items = chord_completion_items(doc, 0, 7).expect("chord context");
        for def in CHORDS_4 {
            assert!(has_detail(&items, 4, def.detail), "missing {}", def.detail);
        }
        assert!(!items.iter().any(|it| it.label == "Chord (3 notes)"));
        for def in CHORDS_3 {
            assert!(!has_detail(&items, 3, def.detail), "unexpected {}", def.detail);
        }
    }

    #[test]
    fn other_channel_counts_show_all() {
        let doc = "AB {c";
        let items = chord_completion_items(doc, 0, 5).expect("chord context");
        assert!(items.iter().any(|it| it.label == "Chord (3 notes)"));
        assert!(items.iter().any(|it| it.label == "Chord (4 notes)"));
        assert!(items.iter().any(|it| it.label == "Cm"));
        assert!(items.iter().any(|it| it.label == "CM7"));
    }

    #[test]
    fn ignores_keysig_block() {
        let doc = "A _{F";
        assert!(chord_completion_items(doc, 0, 5).is_none());
    }

    #[test]
    fn ignores_chord_inside_unterminated_keysig() {
        // is_in_key_sig still reports inside, so the chord context is rejected.
        let doc = "A _{F {c";
        assert!(chord_completion_items(doc, 0, 8).is_none());
    }

    #[test]
    fn handles_explicit_accidental() {
        let doc = "ABC {c+";
        let items = chord_completion_items(doc, 0, 7).expect("chord context");
        let cm = items.iter().find(|it| it.label == "Cm").expect("missing Cm");
        assert_eq!(cm.insert_text.as_deref(), Some("c+/e/g+}"));
        assert_eq!(cm.filter_text.as_deref(), Some("C+m"));
    }

    #[test]
    fn appends_close_brace_only_when_missing() {
        let doc = "ABC {c}";
        let items = chord_completion_items(doc, 0, 6).expect("chord context");
        let cm = items.iter().find(|it| it.label == "Cm").expect("missing Cm");
        assert_eq!(cm.insert_text.as_deref(), Some("c/e-/g"));
    }

    #[test]
    fn empty_outside_any_track() {
        let doc = "#title \"x\"\n{c";
        assert!(chord_completion_items(doc, 1, 2).is_none());
    }

    #[test]
    fn empty_inside_fm_block() {
        let doc = "A x\n@1 fm\n\t31,0,12,7,0,28,0,0,5,0\n\t{c";
        assert!(chord_completion_items(doc, 3, 3).is_none());
    }

    #[test]
    fn respects_key_sig_at_cursor() {
        // F major flats b; the diatonic 7th from d is d/f/a/c — no
        // accidentals needed since b doesn't appear in the chord.
        let doc = "ABCD _{F} o4 {d";
        let items = chord_completion_items(doc, 0, 15).expect("chord context");
        let chord4 = items
            .iter()
            .find(|it| it.label == "Chord (4 notes)")
            .expect("missing 4-note generic chord");
        assert_eq!(
            chord4.insert_text.as_deref().unwrap().trim_end_matches('}'),
            "d/f/a/c"
        );
    }
}
