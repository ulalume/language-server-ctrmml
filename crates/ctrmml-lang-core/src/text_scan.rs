//! Single-line cursor-position classifiers.
//!
//! Ported from `web-ctrmml/src/editor/mml-docs.ts` — these helpers answer
//! "is the cursor inside a comment / key-sig block on this line?" without
//! a parser. They operate on a single line plus a 0-based column.
//!
//! ASCII assumption: ctrmml source is ASCII, so byte indexing matches
//! character indexing. Non-ASCII bytes are treated as opaque payload.

/// Returns `true` when the 0-based column `col` falls inside a `_{...}`
/// key signature block on `line`. Mirrors `isInKeySig` in
/// `web-ctrmml/src/editor/mml-docs.ts`.
pub fn is_in_key_sig(line: &str, col: usize) -> bool {
    let bytes = line.as_bytes();
    if col == 0 {
        return false;
    }
    // Walk left from `col - 1` toward index 1 (inclusive). We need at
    // least two characters to its left to spot the `_{` opener.
    let mut i = (col - 1).min(bytes.len().saturating_sub(1));
    while i >= 1 {
        if bytes[i] == b'}' {
            return false;
        }
        if bytes[i] == b'{' && bytes[i - 1] == b'_' {
            return true;
        }
        i -= 1;
    }
    false
}

/// Returns `true` when the 0-based column `col` falls inside a `;` line
/// comment, respecting `"..."` and `'...'` string contexts. Mirrors
/// `isInComment` in `web-ctrmml/src/editor/mml-docs.ts`.
pub fn is_in_comment(line: &str, col: usize) -> bool {
    let bytes = line.as_bytes();
    let scan_end = col.min(bytes.len());
    let mut in_double = false;
    let mut in_single = false;
    for &ch in &bytes[..scan_end] {
        match ch {
            b'"' if !in_single => in_double = !in_double,
            b'\'' if !in_double => in_single = !in_single,
            b';' if !in_double && !in_single => return true,
            _ => {}
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---------- is_in_key_sig --------------------------------------------------

    #[test]
    fn keysig_inside_block() {
        // Line: "A _{+c} cd" — indices A=0 _=2 {=3 +=4 c=5 }=6.
        // A cursor column lands "between" two characters; the inside-block
        // range is therefore cols 4..=6 (just after `{` through just before
        // the position past `}`).
        let line = "A _{+c} cd";
        // Cursor at `{` itself — still outside, no unmatched `_{` to the left.
        assert!(!is_in_key_sig(line, 3));
        assert!(is_in_key_sig(line, 4));
        assert!(is_in_key_sig(line, 5));
        assert!(is_in_key_sig(line, 6));
        // Cursor just past `}` — the closer is now to our left.
        assert!(!is_in_key_sig(line, 7));
    }

    #[test]
    fn keysig_outside_any_block() {
        let line = "A cdefg";
        for col in 0..=line.len() {
            assert!(!is_in_key_sig(line, col), "col {col}");
        }
    }

    #[test]
    fn keysig_after_closed_block() {
        // After the `}` we are outside again.
        let line = "_{+c} cdefg";
        assert!(!is_in_key_sig(line, 8));
    }

    #[test]
    fn keysig_unterminated_block_keeps_us_inside() {
        // `_{...` with no closing `}` — cursor anywhere past `_{` is inside.
        let line = "_{+c no close";
        assert!(is_in_key_sig(line, 10));
    }

    #[test]
    fn keysig_lone_open_brace_without_underscore_not_inside() {
        // `{` without preceding `_` is a chord-conditional, not a key-sig.
        let line = "{a/b/c}";
        assert!(!is_in_key_sig(line, 3));
    }

    #[test]
    fn keysig_col_zero_is_outside() {
        assert!(!is_in_key_sig("_{+c}", 0));
    }

    // ---------- is_in_comment --------------------------------------------------

    #[test]
    fn comment_after_semicolon() {
        let line = "abc ; comment";
        assert!(!is_in_comment(line, 3));
        // `;` itself is at col 4; TS isInComment(line, col) checks i < col,
        // so col=4 doesn't yet see the `;`. From col=5 onward we're in.
        assert!(!is_in_comment(line, 4));
        assert!(is_in_comment(line, 5));
        assert!(is_in_comment(line, 12));
    }

    #[test]
    fn comment_no_semicolon() {
        let line = "abc def";
        for col in 0..=line.len() {
            assert!(!is_in_comment(line, col));
        }
    }

    #[test]
    fn comment_semicolon_inside_double_quote_is_string() {
        let line = "\"; not comment\" ; real comment";
        // Before the real `;` (col 16), the `;` at col 1 is inside quotes.
        assert!(!is_in_comment(line, 13));
        // After the real `;`.
        assert!(is_in_comment(line, 20));
    }

    #[test]
    fn comment_semicolon_inside_single_quote_is_string() {
        let line = "'; not'; comment";
        assert!(!is_in_comment(line, 5));
        assert!(is_in_comment(line, 10));
    }

    #[test]
    fn comment_double_quote_takes_precedence_over_single() {
        // Inside a "..." run, a `'` does not toggle single-quote state.
        let line = "\"a 'b ; c\" ; real";
        assert!(!is_in_comment(line, 8));
        assert!(is_in_comment(line, 14));
    }

    #[test]
    fn comment_col_zero_never_in_comment() {
        assert!(!is_in_comment("; whole line", 0));
    }

    #[test]
    fn comment_col_past_end_treated_as_end() {
        // No `;` anywhere; even a huge col returns false.
        assert!(!is_in_comment("abc", 999));
        // With a `;`, columns past the line end still see the comment.
        assert!(is_in_comment("a;b", 999));
    }
}
