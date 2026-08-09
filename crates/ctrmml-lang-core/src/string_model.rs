//! In-memory `LineReader` implementations.
//!
//! Provides a simple `Vec<String>`-backed model that both the LSP
//! integration and the crate's own test suite use to feed text into
//! scanners. Addressed 1-based to match Monaco / `getLineContent`
//! conventions.

use crate::track_selector::LineReader;

/// A `LineReader` backed by a `Vec<String>`. The public field lets tests
/// construct one with `LinesModel(vec![...])` ergonomics; production
/// callers usually use [`LinesModel::from_text`] (split a single string
/// on newlines, stripping any `\r`) or [`LinesModel::new`] (build from
/// an iterator of line-like values).
pub struct LinesModel(pub Vec<String>);

/// A borrowing line model for read-only scans that should not copy the
/// document text.
pub struct BorrowedLinesModel<'a>(Vec<&'a str>);

impl<'a> BorrowedLinesModel<'a> {
    /// Index the document's lines while retaining borrowed slices.
    pub fn from_text(text: &'a str) -> Self {
        Self(
            text.split('\n')
                .map(|line| line.strip_suffix('\r').unwrap_or(line))
                .collect(),
        )
    }
}

impl LinesModel {
    /// Build a model from an iterator of `&str`-like values, one per
    /// line. None of the inputs should contain `\n`.
    pub fn new<I, S>(lines: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        LinesModel(lines.into_iter().map(Into::into).collect())
    }

    /// Build a model from a single document string. Splits on `\n` and
    /// strips a trailing `\r` from each line so CRLF input is treated
    /// the same as LF.
    pub fn from_text(text: &str) -> Self {
        let lines = text
            .split('\n')
            .map(|l| l.strip_suffix('\r').unwrap_or(l).to_string())
            .collect();
        Self(lines)
    }
}

impl LineReader for LinesModel {
    fn get_line_content(&self, line_number: u32) -> &str {
        self.0
            .get((line_number as usize).saturating_sub(1))
            .map(String::as_str)
            .unwrap_or("")
    }

    fn get_line_count(&self) -> u32 {
        self.0.len() as u32
    }
}

impl LineReader for BorrowedLinesModel<'_> {
    fn get_line_content(&self, line_number: u32) -> &str {
        self.0
            .get((line_number as usize).saturating_sub(1))
            .copied()
            .unwrap_or("")
    }

    fn get_line_count(&self) -> u32 {
        self.0.len() as u32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_text_splits_on_lf() {
        let m = LinesModel::from_text("a\nb\nc");
        assert_eq!(m.get_line_count(), 3);
        assert_eq!(m.get_line_content(2), "b");
    }

    #[test]
    fn from_text_strips_cr() {
        let m = LinesModel::from_text("a\r\nb\r\nc");
        assert_eq!(m.get_line_content(1), "a");
        assert_eq!(m.get_line_content(2), "b");
    }

    #[test]
    fn out_of_range_line_returns_empty() {
        let m = LinesModel::new(["a", "b"]);
        assert_eq!(m.get_line_content(99), "");
    }
}
