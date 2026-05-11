//! Time signature parsing — ported from `web-ctrmml/src/mml/timesig.ts`.
//!
//! Handles the `#timesig` meta command: `#timesig 4/4`, `#timesig 3/8`,
//! or `#timesig no` (explicitly disables measure lines).

/// A `numerator / denominator` time signature.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimeSignature {
    pub numerator: u32,
    pub denominator: u32,
}

/// 4/4 — the default when no `#timesig` is present.
pub const DEFAULT_TIME_SIGNATURE: TimeSignature = TimeSignature {
    numerator: 4,
    denominator: 4,
};

/// Parse a `"N/D"` string into a [`TimeSignature`], returning `None` for
/// any malformed input. Mirrors `parseTimeSignature` in the TS source.
///
/// Whitespace around the `/` is tolerated. Both components must be
/// positive integers.
pub fn parse_time_signature(value: &str) -> Option<TimeSignature> {
    let trimmed = value.trim();
    let (n_part, d_part) = trimmed.split_once('/')?;
    let numerator: u32 = n_part.trim().parse().ok()?;
    let denominator: u32 = d_part.trim().parse().ok()?;
    if numerator == 0 || denominator == 0 {
        return None;
    }
    Some(TimeSignature {
        numerator,
        denominator,
    })
}

/// Scan MML text for the first `#timesig` line and return its
/// [`TimeSignature`].
///
/// - `Some(sig)` — explicit `#timesig N/D` (or malformed → defaults).
/// - `None`      — `#timesig no` (measure lines disabled).
/// - `Some(DEFAULT_TIME_SIGNATURE)` — no `#timesig` line at all, or the
///   line has a malformed value.
///
/// The two `None`-vs-default cases distinguish "user disabled measures"
/// from "user just didn't specify".
pub fn scan_time_signature(text: &str) -> Option<TimeSignature> {
    for raw_line in text.split('\n') {
        // TS uses `\r?\n` for split; emulate by stripping a trailing \r.
        let raw_line = raw_line.strip_suffix('\r').unwrap_or(raw_line);
        let line = raw_line.trim_start();
        let after = match line.strip_prefix("#timesig") {
            Some(rest) => rest,
            None => continue,
        };
        // Require whitespace or end-of-line directly after `#timesig`,
        // otherwise it's a different keyword (e.g. `#timesignature`).
        if let Some(c) = after.chars().next() {
            if !c.is_whitespace() {
                continue;
            }
        }
        let segment = match after.find(';') {
            Some(p) => &after[..p],
            None => after,
        };
        let value = segment.trim();
        if value.eq_ignore_ascii_case("no") {
            return None;
        }
        return Some(parse_time_signature(value).unwrap_or(DEFAULT_TIME_SIGNATURE));
    }
    Some(DEFAULT_TIME_SIGNATURE)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- parse_time_signature ------------------------------------------------

    #[test]
    fn parses_4_4() {
        assert_eq!(
            parse_time_signature("4/4"),
            Some(TimeSignature {
                numerator: 4,
                denominator: 4
            })
        );
    }

    #[test]
    fn parses_with_whitespace() {
        assert_eq!(
            parse_time_signature("  3 / 8 "),
            Some(TimeSignature {
                numerator: 3,
                denominator: 8
            })
        );
    }

    #[test]
    fn rejects_zero_components() {
        assert!(parse_time_signature("0/4").is_none());
        assert!(parse_time_signature("4/0").is_none());
    }

    #[test]
    fn rejects_non_integer() {
        assert!(parse_time_signature("4.5/4").is_none());
        assert!(parse_time_signature("foo/bar").is_none());
        assert!(parse_time_signature("").is_none());
    }

    #[test]
    fn rejects_missing_slash() {
        assert!(parse_time_signature("44").is_none());
    }

    // ---- scan_time_signature -------------------------------------------------

    #[test]
    fn returns_default_when_no_timesig_line() {
        assert_eq!(scan_time_signature("A o4 cdefg"), Some(DEFAULT_TIME_SIGNATURE));
    }

    #[test]
    fn returns_explicit_value() {
        let text = "#title \"x\"\n#timesig 3/4\nA o4 cdefg";
        assert_eq!(
            scan_time_signature(text),
            Some(TimeSignature {
                numerator: 3,
                denominator: 4
            })
        );
    }

    #[test]
    fn returns_none_for_explicit_no() {
        assert_eq!(scan_time_signature("#timesig no\nA c"), None);
        assert_eq!(scan_time_signature("#timesig NO\nA c"), None);
    }

    #[test]
    fn falls_back_to_default_on_malformed_value() {
        // `#timesig garbage` — value present but unparseable.
        assert_eq!(
            scan_time_signature("#timesig garbage"),
            Some(DEFAULT_TIME_SIGNATURE)
        );
    }

    #[test]
    fn picks_first_timesig_only() {
        // Subsequent `#timesig` lines are ignored.
        let text = "#timesig 3/4\n#timesig 6/8";
        assert_eq!(
            scan_time_signature(text),
            Some(TimeSignature {
                numerator: 3,
                denominator: 4
            })
        );
    }

    #[test]
    fn ignores_keyword_without_separator() {
        // `#timesignature` is NOT `#timesig` — should fall through to default.
        let text = "#timesignature 3/4";
        assert_eq!(scan_time_signature(text), Some(DEFAULT_TIME_SIGNATURE));
    }

    #[test]
    fn strips_inline_comment() {
        let text = "#timesig 3/4 ; my preferred meter";
        assert_eq!(
            scan_time_signature(text),
            Some(TimeSignature {
                numerator: 3,
                denominator: 4
            })
        );
    }

    #[test]
    fn handles_crlf_line_endings() {
        let text = "#title \"x\"\r\n#timesig 3/4\r\nA c";
        assert_eq!(
            scan_time_signature(text),
            Some(TimeSignature {
                numerator: 3,
                denominator: 4
            })
        );
    }

    #[test]
    fn tolerates_leading_whitespace_on_line() {
        let text = "   #timesig 5/8";
        assert_eq!(
            scan_time_signature(text),
            Some(TimeSignature {
                numerator: 5,
                denominator: 8
            })
        );
    }
}
