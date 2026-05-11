//! Chord completion — Phase 3.2.1.
//!
//! Offers `CompletionItem`s for diatonic chord shapes (triads and
//! sevenths) on a track-content line. The user types a note letter
//! (`c`, `d`, ..., `b`) and gets a menu of `cm`, `c7`, `cM7`, `cdim`,
//! `csus2`, etc., each inserting the rendered ctrmml branch
//! expression (e.g. `c/e-/g` for C minor).
//!
//! All semantics come from `ctrmml-lang-core`: chord rendering, key
//! signature scanning, brace-context-aware channel octaves.

use ctrmml_lang_core::{
    chord::{
        render_chord, render_generic_chord, ChordSize, CHORDS_3, CHORDS_4,
    },
    find_enclosing_track_selector, find_fm_block_at, find_psg_block_at, scan_key_sig_at,
    LinesModel,
};
use tower_lsp::lsp_types::{CompletionItem, CompletionItemKind, Documentation};

/// Build chord-completion items for the cursor position.
///
/// Returns an empty vec when chord completion is inappropriate at
/// `(line, col)` — outside any track, inside an FM/PSG instrument
/// block, or with no enclosing track selector.
/// `line_zero_based` / `character` are 0-based (LSP convention);
/// ctrmml-lang-core takes 1-based positions so the function rebases
/// once at entry.
pub(crate) fn chord_completion_items(
    doc_text: &str,
    line_zero_based: u32,
    character: u32,
) -> Vec<CompletionItem> {
    let line = line_zero_based + 1;
    let col = character + 1;
    let model = LinesModel::from_text(doc_text);

    // Suppress inside instrument-definition blocks — chord shapes
    // make no sense as FM operator data or PSG envelope tokens.
    if find_fm_block_at(&model, line).is_some() {
        return Vec::new();
    }
    if find_psg_block_at(&model, line).is_some() {
        return Vec::new();
    }
    // Require an enclosing track selector. Without it we're either in
    // the file header (`#title`, `@1 fm`, ...) or in an unstructured
    // region where chord completion would be noise.
    if find_enclosing_track_selector(&model, line).is_none() {
        return Vec::new();
    }

    let key_sig = scan_key_sig_at(&model, line, col);

    let mut items: Vec<CompletionItem> = Vec::with_capacity(7 * (1 + 1 + CHORDS_3.len() + CHORDS_4.len()));
    for &root in &['c', 'd', 'e', 'f', 'g', 'a', 'b'] {
        // Generic diatonic triad (no chord suffix). Useful as a quick
        // shortcut for "stack the next two letters in the key sig".
        if let Some(text) = render_generic_chord(root, None, ChordSize::Triad) {
            items.push(make_item(
                format!("{root}"),
                text,
                "Diatonic triad (key-sig aware)",
            ));
        }
        if let Some(text) = render_generic_chord(root, None, ChordSize::Seventh) {
            items.push(make_item(
                format!("{root}7th"),
                text,
                "Diatonic seventh (key-sig aware)",
            ));
        }

        // Named chord variants (Cm, CM7, Cdim7, Csus2, ...).
        for def in CHORDS_3.iter().chain(CHORDS_4.iter()) {
            if def.suffix.is_empty() {
                // Bare-letter major triad is already covered by the
                // generic-chord branch above; skip the dup.
                continue;
            }
            if let Some(text) = render_chord(root, None, def, &key_sig) {
                items.push(make_item(
                    format!("{root}{}", def.suffix),
                    text,
                    def.detail,
                ));
            }
        }
    }
    items
}

fn make_item(label: String, insert_text: String, detail: impl Into<String>) -> CompletionItem {
    CompletionItem {
        label: label.clone(),
        // Editor clients filter by `filter_text`/`label` against what
        // the user typed. Setting both explicitly keeps the behaviour
        // consistent across Zed and VSCode.
        filter_text: Some(label),
        insert_text: Some(insert_text.clone()),
        kind: Some(CompletionItemKind::SNIPPET),
        detail: Some(detail.into()),
        documentation: Some(Documentation::String(format!(
            "Inserts the ctrmml branch expression `{insert_text}` for this chord."
        ))),
        ..CompletionItem::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_outside_any_track() {
        // Header lines have no enclosing track selector.
        let doc = "#title \"x\"\n#timesig 4/4\n";
        let items = chord_completion_items(doc, 1, 0);
        assert!(items.is_empty(), "got {} items", items.len());
    }

    #[test]
    fn empty_inside_fm_block() {
        // Cursor inside the `@1 fm` data should not see chord items.
        let doc = "A cdefg\n@1 fm\n\t31,0,12,7,0,28,0,0,5,0\n";
        let items = chord_completion_items(doc, 2, 0);
        assert!(items.is_empty(), "got {} items", items.len());
    }

    #[test]
    fn offers_named_chord_on_track_line() {
        // Track A line, cursor mid-line. Expect e.g. "cm" → "c/e-/g".
        let doc = "A o4 cdefg\n";
        let items = chord_completion_items(doc, 0, 10);
        let cm = items
            .iter()
            .find(|it| it.label == "cm")
            .expect("missing cm chord item");
        assert_eq!(cm.insert_text.as_deref(), Some("c/e-/g"));
    }

    #[test]
    fn respects_key_sig_at_cursor() {
        // After `_{F}` (F major, b flat) the diatonic-7th on D should
        // use the flat b rather than emitting an explicit accidental.
        let doc = "A _{F} o4 d\n";
        let items = chord_completion_items(doc, 0, 11);
        let d7 = items
            .iter()
            .find(|it| it.label == "d7th")
            .expect("missing d7th item");
        // F major flats b; the diatonic 7th from d is d/f/a/c (no
        // accidentals on any of the four tones — the key sig already
        // makes b flat, which doesn't appear in this chord).
        assert_eq!(d7.insert_text.as_deref(), Some("d/f/a/c"));
    }

    #[test]
    fn populates_every_root_letter() {
        let doc = "A cdefg\n";
        let items = chord_completion_items(doc, 0, 5);
        let roots = ['c', 'd', 'e', 'f', 'g', 'a', 'b'];
        for r in roots {
            assert!(
                items.iter().any(|it| it.label == format!("{r}m")),
                "missing {r}m item",
            );
        }
    }
}
