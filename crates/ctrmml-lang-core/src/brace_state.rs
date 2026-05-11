//! Shared `{...}` brace / branch / per-channel octave state machine.
//!
//! The MML conditional block `{a/b/c}` introduces per-branch state
//! tracking: octave changes inside one branch (`{c/e/>g}`) affect only
//! that branch's channel, but `>` or `oN` outside a `{...}` shifts every
//! channel together. Three different scanners in this crate need
//! identical bookkeeping to resolve that — transpose's Lift phase,
//! transpose's pre-selection forward scan, and octave-scan's
//! cursor-context walker.
//!
//! [`BraceState`] is the shared abstraction. Each scanner contributes
//! its own outer loop (with its own rules for skipping comments and
//! quoted strings), then calls the [`BraceState`] methods on the bytes
//! it cares about: `{`, `/`, `}`, `oN`, `>`, `<`.

/// Per-channel octave plus branch-tracking state inside `{...}` blocks.
///
/// The `-1` sentinel for "outside any brace" is preserved in the
/// internal `cur_channel` representation to match the TS callers'
/// `< 0` mental model, but external callers go through
/// [`BraceState::active_channel`] which returns an idiomatic
/// `Option<usize>`.
#[derive(Debug, Clone)]
pub struct BraceState {
    brace_depth: u32,
    branch_idx: usize,
    /// `-1` means "outside any `{...}`"; otherwise a 0-based branch index.
    cur_channel: i32,
    shared_octave: i32,
    channel_octave: Vec<i32>,
    num_channels: usize,
}

impl BraceState {
    /// New state with every channel at `initial_octave` and the cursor
    /// outside any brace. `num_channels` is clamped to `>= 1`.
    pub fn new(num_channels: usize, initial_octave: i32) -> Self {
        let width = num_channels.max(1);
        Self {
            brace_depth: 0,
            branch_idx: 0,
            cur_channel: -1,
            shared_octave: initial_octave,
            channel_octave: vec![initial_octave; width],
            num_channels: width,
        }
    }

    #[inline]
    pub fn brace_depth(&self) -> u32 {
        self.brace_depth
    }

    #[inline]
    pub fn branch_idx(&self) -> usize {
        self.branch_idx
    }

    /// `Some(idx)` while inside a `{...}` whose branch maps to a real
    /// channel, otherwise `None`.
    #[inline]
    pub fn active_channel(&self) -> Option<usize> {
        if self.cur_channel < 0 {
            return None;
        }
        let c = self.cur_channel as usize;
        if c >= self.channel_octave.len() {
            None
        } else {
            Some(c)
        }
    }

    #[inline]
    pub fn shared_octave(&self) -> i32 {
        self.shared_octave
    }

    #[inline]
    pub fn channel_octave(&self) -> &[i32] {
        &self.channel_octave
    }

    #[inline]
    pub fn num_channels(&self) -> usize {
        self.num_channels
    }

    /// The effective octave at the cursor: channel-local while inside a
    /// `{...}` branch, shared otherwise.
    #[inline]
    pub fn current_octave(&self) -> i32 {
        match self.active_channel() {
            Some(c) => self.channel_octave[c],
            None => self.shared_octave,
        }
    }

    /// Snapshot of `(channel_index, channel_octave[channel_index])` for
    /// the currently-active branch. Returns `None` outside any brace.
    /// Used by transpose's Lift phase to record per-branch state right
    /// before a `/` or `}` advances away from it.
    #[inline]
    pub fn active_snapshot(&self) -> Option<(usize, i32)> {
        let c = self.active_channel()?;
        Some((c, self.channel_octave[c]))
    }

    /// Open a `{` conditional. The `prev_byte` is the immediately
    /// preceding source byte; passing `b'_'` is a no-op because `_{` is
    /// a key-sig block opener, not a conditional.
    pub fn on_open_brace(&mut self, prev_byte: u8) {
        if prev_byte == b'_' {
            return;
        }
        self.brace_depth += 1;
        self.branch_idx = 0;
        self.cur_channel = if self.num_channels > 1 { 0 } else { -1 };
    }

    /// Advance to the next branch (only meaningful inside `{...}`).
    /// Returns the snapshot of the branch we just left so callers that
    /// need it (e.g. transpose's per-branch compensation) don't have to
    /// peek before calling.
    pub fn on_slash(&mut self) -> Option<(usize, i32)> {
        let closing = self.active_snapshot();
        self.branch_idx += 1;
        if self.num_channels > 1 && self.branch_idx < self.num_channels {
            self.cur_channel = self.branch_idx as i32;
        }
        closing
    }

    /// Close a `}` (no-op if we weren't inside a brace). Returns the
    /// snapshot of the branch we just left.
    pub fn on_close_brace(&mut self) -> Option<(usize, i32)> {
        if self.brace_depth == 0 {
            return None;
        }
        let closing = self.active_snapshot();
        self.brace_depth -= 1;
        if self.brace_depth == 0 {
            self.cur_channel = -1;
        }
        closing
    }

    /// `oN` — explicit octave set. Affects all channels when outside a
    /// brace, channel-local while inside.
    pub fn on_octave_set(&mut self, oct: i32) {
        match self.active_channel() {
            Some(c) => self.channel_octave[c] = oct,
            None => {
                self.shared_octave = oct;
                for v in self.channel_octave.iter_mut() {
                    *v = oct;
                }
            }
        }
    }

    /// `>` (`delta = +1`) or `<` (`delta = -1`).
    pub fn on_octave_shift(&mut self, delta: i32) {
        match self.active_channel() {
            Some(c) => self.channel_octave[c] += delta,
            None => {
                self.shared_octave += delta;
                for v in self.channel_octave.iter_mut() {
                    *v += delta;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_state_is_outside_brace() {
        let s = BraceState::new(3, 4);
        assert_eq!(s.brace_depth(), 0);
        assert_eq!(s.active_channel(), None);
        assert_eq!(s.current_octave(), 4);
        assert_eq!(s.channel_octave(), &[4, 4, 4]);
    }

    #[test]
    fn open_brace_makes_branch_zero_active() {
        let mut s = BraceState::new(3, 4);
        s.on_open_brace(b' ');
        assert_eq!(s.active_channel(), Some(0));
        assert_eq!(s.brace_depth(), 1);
    }

    #[test]
    fn open_brace_after_underscore_is_noop() {
        let mut s = BraceState::new(3, 4);
        s.on_open_brace(b'_');
        assert_eq!(s.brace_depth(), 0);
        assert_eq!(s.active_channel(), None);
    }

    #[test]
    fn slash_advances_branch() {
        let mut s = BraceState::new(3, 4);
        s.on_open_brace(b' ');
        let snap = s.on_slash();
        assert_eq!(snap, Some((0, 4)));
        assert_eq!(s.active_channel(), Some(1));
    }

    #[test]
    fn close_brace_returns_to_outside() {
        let mut s = BraceState::new(3, 4);
        s.on_open_brace(b' ');
        s.on_close_brace();
        assert_eq!(s.brace_depth(), 0);
        assert_eq!(s.active_channel(), None);
    }

    #[test]
    fn octave_shift_outside_brace_affects_all_channels() {
        let mut s = BraceState::new(3, 4);
        s.on_octave_shift(1);
        assert_eq!(s.channel_octave(), &[5, 5, 5]);
        assert_eq!(s.shared_octave(), 5);
    }

    #[test]
    fn octave_shift_inside_branch_is_channel_local() {
        let mut s = BraceState::new(3, 4);
        s.on_open_brace(b' '); // branch 0
        s.on_slash(); // branch 1
        s.on_octave_shift(1); // affects ch 1 only
        assert_eq!(s.channel_octave(), &[4, 5, 4]);
    }

    #[test]
    fn slash_past_num_channels_does_not_advance_cur_channel() {
        let mut s = BraceState::new(2, 4);
        s.on_open_brace(b' '); // branch 0, ch 0
        s.on_slash(); // branch 1, ch 1
        s.on_slash(); // branch 2; ch stays at 1 (we ran out of channels)
        assert_eq!(s.active_channel(), Some(1));
    }

    #[test]
    fn single_channel_brace_has_no_active_channel() {
        // num_channels = 1 means there's no per-branch routing; even
        // inside `{...}` we report None.
        let mut s = BraceState::new(1, 4);
        s.on_open_brace(b' ');
        assert_eq!(s.active_channel(), None);
        s.on_octave_shift(1);
        // The shift is treated as shared.
        assert_eq!(s.shared_octave(), 5);
        assert_eq!(s.channel_octave(), &[5]);
    }
}
