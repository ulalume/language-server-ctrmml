//! Tick → MML rest decomposition + measure-remainder utilities.
//!
//! Ported from `web-ctrmml/src/mml/beat-fill.ts`.
//!
//! Used by the "fill measure with rests" code action: given a cursor
//! tick position and the song's time signature, generate the MML rest
//! sequence that fills to the next bar line.

use crate::timesig::{TimeSignature, DEFAULT_TIME_SIGNATURE};

/// Standard note denominator values (whole → 64th).
const DENOMINATORS: [u32; 7] = [1, 2, 4, 8, 16, 32, 64];

/// One row of the duration decomposition table.
#[derive(Debug, Clone)]
struct DurationEntry {
    ticks: u32,
    label: String,
}

/// Build a sorted table of standard durations (plain, dotted,
/// double-dotted) for greedy decomposition.
///
/// The TS version memoizes this by `ppqn`. We don't: the table is tiny
/// (≤21 entries) and the function is called once per fill — caching it
/// would require a `Mutex<HashMap>` or `OnceCell` for cross-thread
/// access and isn't worth the complexity.
fn build_duration_table(ppqn: u32) -> Vec<DurationEntry> {
    let whole = ppqn * 4;
    let mut entries: Vec<DurationEntry> = Vec::with_capacity(DENOMINATORS.len() * 3);

    for &d in &DENOMINATORS {
        if whole % d != 0 {
            continue;
        }
        let base = whole / d;
        if base < 1 {
            continue;
        }
        entries.push(DurationEntry {
            ticks: base,
            label: d.to_string(),
        });

        // Dotted: base + base/2. Only emit when base/2 is an integer.
        if base % 2 == 0 {
            let dotted = base + base / 2;
            entries.push(DurationEntry {
                ticks: dotted,
                label: format!("{d}."),
            });
        }

        // Double-dotted: base + base/2 + base/4.
        if base % 4 == 0 {
            let ddot = base + base / 2 + base / 4;
            entries.push(DurationEntry {
                ticks: ddot,
                label: format!("{d}.."),
            });
        }
    }

    // Greedy decomposition needs longest-first; ties broken by
    // shorter-label-first (so `2` wins over `4.` at the same tick count).
    entries.sort_by(|a, b| {
        b.ticks
            .cmp(&a.ticks)
            .then_with(|| a.label.len().cmp(&b.label.len()))
    });

    // Dedupe by tick value (keep the first occurrence — already the
    // shortest-label one thanks to the secondary sort).
    let mut seen: Vec<u32> = Vec::with_capacity(entries.len());
    entries.retain(|e| {
        if seen.contains(&e.ticks) {
            false
        } else {
            seen.push(e.ticks);
            true
        }
    });
    entries
}

/// Convert a tick count into MML rest notation via greedy decomposition.
/// Falls back to `r:<ticks>` for any non-standard remainder.
///
/// Returns an empty string for `ticks == 0`.
pub fn ticks_to_mml_rest(ticks: u32, ppqn: u32) -> String {
    if ticks == 0 {
        return String::new();
    }
    let table = build_duration_table(ppqn);
    let mut out = String::new();
    let mut remaining = ticks;
    for entry in &table {
        while remaining >= entry.ticks {
            out.push('r');
            out.push_str(&entry.label);
            remaining -= entry.ticks;
        }
    }
    if remaining > 0 {
        out.push_str(&format!("r:{remaining}"));
    }
    out
}

/// Tick count remaining to reach the next bar line. Returns `0` when the
/// cursor sits exactly on a measure boundary.
///
/// `ticks_per_measure = ppqn * numerator * 4 / denominator`.
pub fn measure_remainder_ticks(
    cursor_tick: u32,
    ppqn: u32,
    time_sig: TimeSignature,
) -> u32 {
    let tpm = ppqn * time_sig.numerator * 4 / time_sig.denominator;
    let pos = cursor_tick % tpm;
    if pos == 0 {
        0
    } else {
        tpm - pos
    }
}

/// Generate rests to fill through the next bar line.
///
/// Returns the rest string (e.g. `"r4r4r4"`) without the trailing
/// `" |"`. Callers append `" |"` themselves when needed.
///
/// When `after_bar_line` is `true` and the cursor sits exactly on a
/// measure boundary, this fills a full measure (the next bar). When
/// `false` on a boundary, returns an empty string.
pub fn generate_measure_rests(
    cursor_tick: u32,
    ppqn: u32,
    time_sig: Option<TimeSignature>,
    after_bar_line: bool,
) -> String {
    let ts = time_sig.unwrap_or(DEFAULT_TIME_SIGNATURE);
    let mut remainder = measure_remainder_ticks(cursor_tick, ppqn, ts);
    if remainder == 0 && after_bar_line {
        remainder = ppqn * ts.numerator * 4 / ts.denominator;
    }
    ticks_to_mml_rest(remainder, ppqn)
}

/// Returns `true` when the text before 1-based `column` on `line_content`
/// ends (ignoring trailing whitespace) with a `|` bar-line marker.
pub fn is_after_bar_line(line_content: &str, column: u32) -> bool {
    let end = (column.saturating_sub(1) as usize).min(line_content.len());
    line_content[..end].trim_end().ends_with('|')
}

#[cfg(test)]
mod tests {
    use super::*;

    const PPQN: u32 = 48;

    // ---- ticks_to_mml_rest ---------------------------------------------------

    #[test]
    fn zero_ticks_empty_string() {
        assert_eq!(ticks_to_mml_rest(0, PPQN), "");
    }

    #[test]
    fn whole_note_decomposes_to_r1() {
        // ppqn=48 → whole = 192. Single whole rest.
        assert_eq!(ticks_to_mml_rest(192, PPQN), "r1");
    }

    #[test]
    fn quarter_note_decomposes_to_r4() {
        assert_eq!(ticks_to_mml_rest(PPQN, PPQN), "r4");
    }

    #[test]
    fn dotted_quarter_decomposes_to_r4_dot() {
        // ppqn=48 → dotted quarter = 48 + 24 = 72.
        assert_eq!(ticks_to_mml_rest(72, PPQN), "r4.");
    }

    #[test]
    fn three_quarters_decomposes_to_dotted() {
        // 144 = dotted-half = 96 + 48 → "r2." (preferred over r2r4 or r4r4r4).
        assert_eq!(ticks_to_mml_rest(144, PPQN), "r2.");
    }

    #[test]
    fn non_standard_remainder_uses_colon_form() {
        // 2 ticks — smaller than the smallest table entry (r64 = 3 ticks
        // at PPQN=48), so it has to fall back to `r:2`.
        assert_eq!(ticks_to_mml_rest(2, PPQN), "r:2");
    }

    #[test]
    fn combines_standard_and_smaller_durations() {
        // quarter (48) + 3 ticks → r4 + r64 (3 ticks).
        assert_eq!(ticks_to_mml_rest(PPQN + 3, PPQN), "r4r64");
    }

    #[test]
    fn combines_standard_with_non_standard_remainder() {
        // quarter + 2 ticks → r4 + r:2 (2 is below the smallest entry).
        assert_eq!(ticks_to_mml_rest(PPQN + 2, PPQN), "r4r:2");
    }

    // ---- measure_remainder_ticks --------------------------------------------

    #[test]
    fn remainder_zero_on_bar_boundary() {
        assert_eq!(
            measure_remainder_ticks(0, PPQN, DEFAULT_TIME_SIGNATURE),
            0
        );
        // One full 4/4 measure later.
        let tpm = PPQN * 4;
        assert_eq!(
            measure_remainder_ticks(tpm, PPQN, DEFAULT_TIME_SIGNATURE),
            0
        );
    }

    #[test]
    fn remainder_partway_into_measure() {
        // Cursor on beat 2 of 4/4 → 3 beats left.
        assert_eq!(
            measure_remainder_ticks(PPQN, PPQN, DEFAULT_TIME_SIGNATURE),
            PPQN * 3
        );
    }

    #[test]
    fn remainder_uses_compound_meter_correctly() {
        // 3/8 with ppqn=48: tpm = 48*3*4/8 = 72.
        let ts = TimeSignature {
            numerator: 3,
            denominator: 8,
        };
        assert_eq!(measure_remainder_ticks(0, 48, ts), 0);
        assert_eq!(measure_remainder_ticks(36, 48, ts), 36);
        assert_eq!(measure_remainder_ticks(72, 48, ts), 0);
    }

    // ---- generate_measure_rests ---------------------------------------------

    #[test]
    fn fills_partial_measure() {
        // Cursor on beat 2 of 4/4 → 3 beats (144 ticks) left → "r2."
        // (greedy decomp picks the dotted half over three quarters).
        assert_eq!(generate_measure_rests(PPQN, PPQN, None, false), "r2.");
    }

    #[test]
    fn empty_on_bar_boundary_without_after_flag() {
        assert_eq!(generate_measure_rests(0, PPQN, None, false), "");
    }

    #[test]
    fn fills_full_measure_on_boundary_with_after_flag() {
        // 4/4 full measure (192 ticks at ppqn=48) → "r1" (greedy picks
        // the whole rest).
        assert_eq!(generate_measure_rests(0, PPQN, None, true), "r1");
    }

    #[test]
    fn fills_full_measure_compound_meter() {
        // 3/8 measure at ppqn=48 → tpm = 72 → "r4." (dotted quarter).
        let ts = TimeSignature {
            numerator: 3,
            denominator: 8,
        };
        assert_eq!(generate_measure_rests(0, 48, Some(ts), true), "r4.");
    }

    // ---- is_after_bar_line --------------------------------------------------

    #[test]
    fn detects_bar_line_at_end() {
        // Column 1-based; "abc |" with col 6 sees "abc |".
        assert!(is_after_bar_line("abc |", 6));
    }

    #[test]
    fn detects_bar_line_through_trailing_whitespace() {
        // "abc |  " with col 8 sees "abc |  ", trim_end → "abc |" ends with |.
        assert!(is_after_bar_line("abc |  ", 8));
    }

    #[test]
    fn rejects_when_no_bar_line() {
        assert!(!is_after_bar_line("abc def", 8));
    }

    #[test]
    fn ignores_text_after_cursor() {
        // The | is after the cursor → not detected.
        assert!(!is_after_bar_line("abc |", 4));
    }

    #[test]
    fn column_past_end_treated_as_end() {
        assert!(is_after_bar_line("abc |", 99));
    }
}
