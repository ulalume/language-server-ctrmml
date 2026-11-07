//! Filling the current measure with rests.
//!
//! The cursor's compiled tick comes from the compiler (`ctrmml-cmd
//! find-cursor-tick` or its wasm build), so a fill is two steps: decide
//! whether one applies at a position, then build the insertion for the
//! tick. Positions are 0-based lines and UTF-16 characters.

use serde::Serialize;

use crate::beat_fill::{generate_measure_rests, is_after_bar_line};
use crate::block_finder::{find_fm_block_at, find_psg_block_at};
use crate::completion::{utf16_character_to_byte_offset, CoreTextEdit, EditRange, Pos};
use crate::string_model::BorrowedLinesModel;
use crate::timesig::{scan_time_signature, TimeSignature};
use crate::track_selector::{find_enclosing_track_selector, LineReader};

/// A fill that applies at a position.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct FillMeasurePlan {
    pub numerator: u32,
    pub denominator: u32,
    /// The text before the cursor ends with `|`, ignoring whitespace.
    pub after_bar_line: bool,
}

impl FillMeasurePlan {
    pub fn time_signature(&self) -> TimeSignature {
        TimeSignature {
            numerator: self.numerator,
            denominator: self.denominator,
        }
    }
}

/// The insertion that fills the measure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FillMeasureEdit {
    /// The rests inserted; empty when the edit only closes a full measure.
    pub rests: String,
    /// Insertion at the cursor.
    pub edit: CoreTextEdit,
}

/// Whether a fill applies at the position. `None` inside `@N fm` and
/// `@N psg` blocks, outside every track, and under `#timesig no`.
pub fn fill_measure_plan(text: &str, line: u32, character: u32) -> Option<FillMeasurePlan> {
    let model = BorrowedLinesModel::from_text(text);
    let line_number = line.saturating_add(1);
    if line_number > model.get_line_count()
        || find_fm_block_at(&model, line_number).is_some()
        || find_psg_block_at(&model, line_number).is_some()
    {
        return None;
    }
    find_enclosing_track_selector(&model, line_number)?;
    let time_signature = scan_time_signature(text)?;
    let content = model.get_line_content(line_number);
    let column = utf16_character_to_byte_offset(content, character) as u32 + 1;
    Some(FillMeasurePlan {
        numerator: time_signature.numerator,
        denominator: time_signature.denominator,
        after_bar_line: is_after_bar_line(content, column),
    })
}

/// The fill at the position for the cursor's compiled `tick`: rests up to
/// the next bar line then ` |`; a whole measure of rests right after a bar
/// line; ` |` alone when the notes already fill the measure.
pub fn fill_measure_edit(
    text: &str,
    line: u32,
    character: u32,
    tick: u32,
    ppqn: u32,
) -> Option<FillMeasureEdit> {
    let plan = fill_measure_plan(text, line, character)?;
    let time_signature = plan.time_signature();
    if ppqn * time_signature.numerator * 4 / time_signature.denominator == 0 {
        return None;
    }
    let rests = generate_measure_rests(tick, ppqn, Some(time_signature), plan.after_bar_line);
    let new_text = if rests.is_empty() {
        " |".to_string()
    } else {
        format!("{rests} |")
    };
    let position = Pos { line, character };
    Some(FillMeasureEdit {
        rests,
        edit: CoreTextEdit {
            range: EditRange::new(position, position),
            new_text,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const PPQN: u32 = 48;

    fn inserted(text: &str, line: u32, character: u32, tick: u32) -> Option<String> {
        fill_measure_edit(text, line, character, tick, PPQN).map(|fill| fill.edit.new_text)
    }

    #[test]
    fn a_partial_measure_gets_rests_and_a_bar_line() {
        assert_eq!(inserted("A o4 c4 d4\n", 0, 10, 96).as_deref(), Some("r2 |"));
    }

    #[test]
    fn right_after_a_bar_line_a_whole_measure_of_rests_goes_in() {
        assert_eq!(
            inserted("A o4 cdef |\n", 0, 11, 192).as_deref(),
            Some("r1 |")
        );
    }

    #[test]
    fn a_full_measure_without_a_bar_line_is_closed() {
        let fill = fill_measure_edit("A o4 cdef\n", 0, 9, 192, PPQN).expect("fill");
        assert_eq!(fill.rests, "");
        assert_eq!(fill.edit.new_text, " |");
        assert_eq!(
            fill.edit.range.start,
            Pos {
                line: 0,
                character: 9
            }
        );
    }

    #[test]
    fn the_time_signature_sets_the_measure_length() {
        assert_eq!(
            inserted("#timesig 3/4\nA c4\n", 1, 4, 48).as_deref(),
            Some("r2 |")
        );
    }

    #[test]
    fn positions_count_utf16_units() {
        // `あ` is one UTF-16 unit and three bytes; the cursor is after `|`.
        let plan = fill_measure_plan("A 'あ' c4 |\n", 0, 10).expect("plan");
        assert!(plan.after_bar_line);
        assert!(
            !fill_measure_plan("A 'あ' c4 |\n", 0, 9)
                .expect("plan")
                .after_bar_line
        );
    }

    #[test]
    fn fills_stay_out_of_instruments_headers_and_timesig_no() {
        assert_eq!(fill_measure_plan("#title x |\n", 0, 10), None);
        assert_eq!(
            fill_measure_plan("A c\n@1 fm\n 31 0 0 0 0 0 0 0 0 0 |\n", 2, 25),
            None
        );
        assert_eq!(fill_measure_plan("A c\n@1 psg 15 | 0\n", 1, 14), None);
        assert_eq!(fill_measure_plan("#timesig no\nA cdef |\n", 1, 8), None);
        assert_eq!(fill_measure_plan("A c\n", 5, 0), None);
    }

    #[test]
    fn a_zero_ppqn_has_no_fill() {
        assert_eq!(fill_measure_edit("A c\n", 0, 3, 0, 0), None);
    }

    #[test]
    fn plans_carry_the_time_signature() {
        let plan = fill_measure_plan("#timesig 6/8\nA c |\n", 1, 5).expect("plan");
        assert_eq!((plan.numerator, plan.denominator), (6, 8));
        assert!(plan.after_bar_line);
    }
}
