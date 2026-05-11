//! Test-only helpers shared across the crate's `#[cfg(test)]` modules.
//!
//! Kept behind `#[cfg(test)]` at the lib root so this module never ships in
//! production builds.

use crate::track_selector::LineReader;

/// A `LineReader` backed by an in-memory `Vec<String>`, addressed 1-based
/// to match Monaco / `getLineContent` conventions.
pub struct LinesModel(pub Vec<String>);

impl LinesModel {
    /// Convenience constructor from an iterator of `&str`-like values.
    pub fn new<I, S>(lines: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        LinesModel(lines.into_iter().map(Into::into).collect())
    }
}

impl LineReader for LinesModel {
    fn get_line_content(&self, line_number: u32) -> &str {
        self.0
            .get((line_number as usize).saturating_sub(1))
            .map(String::as_str)
            .unwrap_or("")
    }
}
