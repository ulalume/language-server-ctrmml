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

/// Largest char boundary `<= idx` in `s` (clamped to `s.len()`). Keeps
/// `&s[..idx]` / `&s[idx..]` slicing panic-safe when a caller hands us a
/// byte offset that lands inside a multi-byte codepoint — e.g. a UTF-16
/// editor column mis-used as a UTF-8 byte offset against a non-ASCII line
/// (`#title あ…`, comments, sample paths). `str::floor_char_boundary` is
/// still unstable, so we roll our own.
pub(crate) fn floor_char_boundary(s: &str, idx: usize) -> usize {
    if idx >= s.len() {
        return s.len();
    }
    let mut i = idx;
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// Returns `true` when the 0-based column `col` falls inside a `"..."` or
/// `'...'` string literal on `line`. The delimiting quote bytes themselves
/// also report `true` so callers can blanket-skip the whole string region.
///
/// Single quotes carry ctrmml's platform-exclusive commands
/// (e.g. `'fm3 0001'`, `'pcmmode 2'`) and double quotes wrap PCM sample
/// paths in `@N pcm "..."`. In both cases any embedded letters must not be
/// interpreted as MML notes/commands.
pub fn is_in_string(line: &str, col: usize) -> bool {
    let bytes = line.as_bytes();
    if col >= bytes.len() {
        return false;
    }
    let mut in_double = false;
    let mut in_single = false;
    for (i, &ch) in bytes.iter().enumerate() {
        match ch {
            b'"' if !in_single => {
                in_double = !in_double;
                if i == col {
                    return true;
                }
            }
            b'\'' if !in_double => {
                in_single = !in_single;
                if i == col {
                    return true;
                }
            }
            b';' if !in_double && !in_single => return false,
            _ => {
                if i == col {
                    return in_double || in_single;
                }
            }
        }
    }
    false
}

/// Returns the token touching column `col` on `line`, along with its
/// `[start, end)` byte-offset span. Ported verbatim from the root LSP
/// crate's `mml::token_at` (formerly `src/mml/mod.rs`) — walks left from
/// a whitespace-adjusted `col` to the nearest non-whitespace byte, then
/// expands to the full run of [`is_token_char`] bytes around it. `^` and
/// `&` are always treated as single-character tokens (tie/slur
/// commands), regardless of their neighbors.
///
/// Note: this is a distinct implementation from the private,
/// whitespace-delimited `token_at` in `hover.rs` — the two were not
/// unified when this helper was absorbed into core; see that module for
/// details.
pub fn token_at(line: &str, col: usize) -> Option<(&str, usize, usize)> {
    let bytes = line.as_bytes();
    if bytes.is_empty() {
        return None;
    }
    let mut idx = col.min(line.len().saturating_sub(1));
    while idx > 0 && bytes[idx].is_ascii_whitespace() {
        idx -= 1;
    }

    // The byte immediately left of trailing ASCII whitespace can be a UTF-8
    // continuation byte. Normalize it before inspecting or slicing the token.
    idx = floor_char_boundary(line, idx);

    let ch = line[idx..].chars().next()?;
    if ch == '^' || ch == '&' {
        return Some((&line[idx..idx + 1], idx, idx + 1));
    }

    let mut start = idx;
    while start > 0 && is_token_char(bytes[start - 1] as char) {
        start -= 1;
    }
    let mut end = idx + ch.len_utf8();
    while end < line.len() && is_token_char(bytes[end] as char) {
        end += 1;
    }
    if start == end {
        return None;
    }
    Some((&line[start..end], start, end))
}

/// Splits `input` on whitespace outside double-quoted runs.
///
/// Quote characters remain part of their token, and an unterminated quoted
/// run consumes through the end of the input. This is used for PCM instrument
/// declarations whose sample paths may themselves contain whitespace.
pub fn tokenize_outside_double_quotes(input: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    for ch in input.chars() {
        if ch == '"' {
            current.push(ch);
            in_quotes = !in_quotes;
        } else if ch.is_whitespace() && !in_quotes {
            if !current.is_empty() {
                tokens.push(std::mem::take(&mut current));
            }
        } else {
            current.push(ch);
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

/// Returns `true` when `token` is an `@<digits>` instrument reference
/// (e.g. `@12`). Ported verbatim from the root LSP crate's
/// `mml::is_at_number`.
///
/// Note: `hover.rs` has its own private, differently-implemented
/// `is_at_number` (byte-slice based rather than `Chars`-based); the two
/// were not unified when this helper was absorbed into core.
pub fn is_at_number(token: &str) -> bool {
    let mut chars = token.chars();
    if chars.next() != Some('@') {
        return false;
    }
    let rest: String = chars.collect();
    !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit())
}

/// Returns the `(left, right)` byte offsets of the `'...'` run enclosing
/// `col`, if any — both offsets point at the quote bytes themselves.
/// Ported verbatim from the root LSP crate's `mml::single_quote_bounds`.
///
/// Note: `hover.rs` has its own private, differently-implemented
/// `single_quote_bounds`; the two were not unified when this helper was
/// absorbed into core.
pub fn single_quote_bounds(line: &str, col: usize) -> Option<(usize, usize)> {
    let bytes = line.as_bytes();
    let mut left = col.min(line.len().saturating_sub(1));
    while left > 0 && bytes[left] != b'\'' {
        left = left.saturating_sub(1);
    }
    if bytes[left] != b'\'' {
        return None;
    }

    let mut right = col.min(line.len().saturating_sub(1));
    while right < line.len() && bytes[right] != b'\'' {
        right += 1;
    }
    if right >= line.len() || bytes[right] != b'\'' {
        return None;
    }
    if left >= right {
        return None;
    }
    Some((left, right))
}

/// Returns the `(left, right)` byte offsets of the `"..."` run enclosing
/// `col`, if any — both offsets point at the quote bytes themselves.
/// Ported verbatim from the root LSP crate's `mml::double_quote_bounds`.
///
/// Note: `hover.rs` has its own private, differently-implemented
/// `double_quote_bounds`; the two were not unified when this helper was
/// absorbed into core.
pub fn double_quote_bounds(line: &str, col: usize) -> Option<(usize, usize)> {
    let bytes = line.as_bytes();
    let mut left = col.min(line.len().saturating_sub(1));
    while left > 0 && bytes[left] != b'"' {
        left = left.saturating_sub(1);
    }
    if bytes[left] != b'"' {
        return None;
    }

    let mut right = col.min(line.len().saturating_sub(1));
    while right < line.len() && bytes[right] != b'"' {
        right += 1;
    }
    if right >= line.len() || bytes[right] != b'"' {
        return None;
    }
    if left >= right {
        return None;
    }
    Some((left, right))
}

/// Byte classifier backing [`token_at`]'s run expansion. Ported verbatim
/// from the root LSP crate's `mml` module.
fn is_token_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || matches!(ch, '#' | '@' | '_' | '=' | '*' | '-' | '+' | '{' | '}')
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

    // ---------- is_in_string ---------------------------------------------------

    #[test]
    fn string_single_quote_run() {
        // 'fm3 0011' at cols 0..=8. Both delimiters and interior bytes count.
        let line = "'fm3 0011'";
        for col in 0..=9 {
            assert!(is_in_string(line, col), "col {col} should be in string");
        }
    }

    #[test]
    fn string_outside_single_quote_run() {
        // "A 'fm3' c" — only the `'fm3'` run is inside.
        let line = "A 'fm3' c";
        assert!(!is_in_string(line, 0)); // 'A'
        assert!(!is_in_string(line, 1)); // ' '
        assert!(is_in_string(line, 2)); // opening '
        assert!(is_in_string(line, 3)); // 'f'
        assert!(is_in_string(line, 4)); // 'm'
        assert!(is_in_string(line, 5)); // '3'
        assert!(is_in_string(line, 6)); // closing '
        assert!(!is_in_string(line, 7)); // ' '
        assert!(!is_in_string(line, 8)); // 'c'
    }

    #[test]
    fn string_double_quote_run() {
        let line = "@30 pcm \"a.wav\"";
        // Opening " at col 8, closing " at col 14.
        for col in 0..=7 {
            assert!(!is_in_string(line, col));
        }
        for col in 8..=14 {
            assert!(is_in_string(line, col), "col {col} should be in string");
        }
    }

    #[test]
    fn string_single_inside_double_not_a_quote() {
        // A `'` inside `"..."` does not toggle single-quote state.
        let line = "\"a'b\" c";
        for col in 0..=4 {
            assert!(is_in_string(line, col), "col {col} should be in string");
        }
        assert!(!is_in_string(line, 5));
        assert!(!is_in_string(line, 6));
    }

    #[test]
    fn string_double_inside_single_not_a_quote() {
        let line = "'a\"b' c";
        for col in 0..=4 {
            assert!(is_in_string(line, col), "col {col} should be in string");
        }
        assert!(!is_in_string(line, 5));
        assert!(!is_in_string(line, 6));
    }

    #[test]
    fn string_adjacent_single_quoted_runs() {
        // 'tl1 -2''tl3 -2' — two back-to-back strings; everything from col 0
        // through the closing of the second run is inside-or-on a quote.
        let line = "'tl1 -2''tl3 -2'";
        for col in 0..line.len() {
            assert!(is_in_string(line, col), "col {col} should be in string");
        }
    }

    #[test]
    fn string_unterminated_run_keeps_us_inside() {
        let line = "'fm3 0011 no close";
        for col in 0..line.len() {
            assert!(is_in_string(line, col), "col {col} should be in string");
        }
    }

    #[test]
    fn string_after_semicolon_not_in_string() {
        // A `;` outside any string terminates string-tracking — anything
        // after is comment, not string.
        let line = "; 'fm3'";
        for col in 0..line.len() {
            assert!(
                !is_in_string(line, col),
                "col {col} should not be in string"
            );
        }
    }

    #[test]
    fn string_semicolon_inside_quote_is_not_terminator() {
        let line = "'; still string' x";
        for col in 0..=15 {
            assert!(is_in_string(line, col), "col {col} should be in string");
        }
        assert!(!is_in_string(line, 16));
        assert!(!is_in_string(line, 17));
    }

    #[test]
    fn string_col_past_end_returns_false() {
        assert!(!is_in_string("abc", 999));
        assert!(!is_in_string("'abc'", 999));
    }

    // ---------- is_in_comment regression: root-crate absorption -----------

    #[test]
    fn is_in_comment_regression_semicolon_inside_single_quote_string() {
        // Regression pin for the root-crate `is_in_comment` divergence:
        // `src/lsp.rs`'s `goto_definition` and `completion` handlers used
        // to call a duplicate `is_in_comment` in `src/mml/mod.rs` that
        // only tracked `"..."` strings, so a `;` inside a `'...'`
        // platform-command string (e.g. `'mode 1'`) was misread as the
        // start of a line comment. Both call sites now use this core
        // implementation, which tracks `'...'` too.
        let line = "'; a' ; b";
        // Column 2 sits just after the `;` at index 1, which is inside
        // the `'...'` run — must NOT read as a comment start.
        assert!(!is_in_comment(line, 2));
        // Column 7 sits just after the real `;` at index 6, outside any
        // string — must read as a comment start.
        assert!(is_in_comment(line, 7));
    }

    // ---------- token_at ----------------------------------------------------
    //
    // Ported from the root LSP crate's `mml::token_at`. Distinct from the
    // private, whitespace-delimited `token_at` in `hover.rs` — see the
    // doc comment on `token_at` above.

    #[test]
    fn token_at_expands_over_special_token_chars() {
        // `@12` is one token: `@` and digits are both `is_token_char`.
        let line = "@12 cde";
        assert_eq!(token_at(line, 1), Some(("@12", 0, 3)));
        assert_eq!(token_at(line, 0), Some(("@12", 0, 3)));
    }

    #[test]
    fn token_at_caret_and_ampersand_are_standalone() {
        // `^` (tie) and `&` (slur) are always single-character tokens,
        // even when directly touching alphanumeric neighbors.
        let line = "c^d";
        assert_eq!(token_at(line, 1), Some(("^", 1, 2)));
        let line2 = "c&d";
        assert_eq!(token_at(line2, 1), Some(("&", 1, 2)));
    }

    #[test]
    fn token_at_skips_left_over_whitespace() {
        // Cursor sitting in trailing whitespace resolves to the nearest
        // token to its left, not an empty span.
        let line = "cde  fgh";
        assert_eq!(token_at(line, 4), Some(("cde", 0, 3)));
    }

    #[test]
    fn token_at_trailing_whitespace_is_safe_after_multibyte_scalars() {
        for (line, expected) in [
            ("A あ ", ("あ", 2, 5)),
            ("A 😀 ", ("😀", 2, 6)),
            ("A 𝄞 ", ("𝄞", 2, 6)),
            ("A c4 あ\t", ("あ", 5, 8)),
        ] {
            assert_eq!(token_at(line, line.len()), Some(expected), "{line:?}");
        }
    }

    #[test]
    fn token_at_empty_line_is_none() {
        assert_eq!(token_at("", 0), None);
    }

    // ---------- tokenize_outside_double_quotes -----------------------------

    #[test]
    fn quote_aware_tokenizer_keeps_whitespace_inside_paths() {
        assert_eq!(
            tokenize_outside_double_quotes("@1 pcm \"drums and bass/kick.wav\" rate=8000"),
            ["@1", "pcm", "\"drums and bass/kick.wav\"", "rate=8000"]
        );
    }

    #[test]
    fn quote_aware_tokenizer_accepts_tabs_and_unterminated_quotes() {
        assert_eq!(
            tokenize_outside_double_quotes("@1\tpcm\t\"ドラム kick.wav"),
            ["@1", "pcm", "\"ドラム kick.wav"]
        );
    }

    // ---------- is_at_number -------------------------------------------------
    //
    // Ported from the root LSP crate's `mml::is_at_number`. Distinct from
    // the private `is_at_number` in `hover.rs` — see the doc comment on
    // `is_at_number` above.

    #[test]
    fn is_at_number_accepts_at_followed_by_digits() {
        assert!(is_at_number("@12"));
        assert!(is_at_number("@0"));
    }

    #[test]
    fn is_at_number_rejects_bare_at_and_non_digits() {
        assert!(!is_at_number("@"));
        assert!(!is_at_number("@1a"));
        assert!(!is_at_number("12"));
    }

    // ---------- single_quote_bounds / double_quote_bounds --------------------
    //
    // Ported from the root LSP crate's `mml::single_quote_bounds` /
    // `mml::double_quote_bounds`. Distinct from the private, differently
    // implemented `single_quote_bounds` / `double_quote_bounds` in
    // `hover.rs` — see the doc comments above.

    #[test]
    fn single_quote_bounds_finds_enclosing_run() {
        let line = "'abc'";
        assert_eq!(single_quote_bounds(line, 2), Some((0, 4)));
    }

    #[test]
    fn single_quote_bounds_none_outside_any_run() {
        let line = "a 'bc' d";
        assert_eq!(single_quote_bounds(line, 0), None);
    }

    #[test]
    fn double_quote_bounds_finds_enclosing_run() {
        let line = "@1 pcm \"a.wav\"";
        // `"a.wav"` spans bytes [7, 13].
        assert_eq!(double_quote_bounds(line, 9), Some((7, 13)));
    }

    #[test]
    fn double_quote_bounds_none_outside_any_run() {
        let line = "@1 pcm \"a.wav\"";
        assert_eq!(double_quote_bounds(line, 0), None);
    }
}
