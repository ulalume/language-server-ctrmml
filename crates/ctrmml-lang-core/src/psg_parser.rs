//! PSG volume envelope parser/serializer — ported from
//! `web-ctrmml/src/mml/psg-parser.ts`.
//!
//! Format: `@N psg value[>target][:length] ... [/ ...] [| ...]`
//!
//! Token rules (mirroring ctrmml's `add_ins_psg`):
//!  - `value`              — single volume, played for `default_length` frames
//!  - `value:length`       — volume held for `length` frames
//!  - `value>target`       — slide; default length = `|target - value| + 1`
//!  - `value>target:length`— slide over `length` frames (explicit override)
//!  - `/`                  — sustain marker (key-on holds here; release continues after)
//!  - `|`                  — loop marker (envelope loops back here)
//!  - `l:N`                — set default frame length for subsequent nodes
//!
//! Upstream is permissive about trailing garbage on numeric tokens because
//! tags split only on whitespace/commas; `15/` parses as `15` (the `/`
//! ignored), `|0` parses as `|` (the `0` ignored). This port preserves
//! that quirk so visualizers match the runtime exactly.

/// One node in the envelope sequence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PsgNode {
    /// Starting volume `0..=15` (15 = loudest).
    pub value: u8,
    /// Slide target volume `0..=15`. `None` means "hold at `value`".
    pub target: Option<u8>,
    /// Explicit frame count. `None` means "use the envelope's effective
    /// default" — see [`node_effective_length`].
    pub length: Option<u32>,
}

/// Parsed PSG envelope. `sustain_pos` and `loop_pos` are node indices, not
/// frame offsets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PsgEnvelope {
    /// Instrument number from the `@N` header.
    pub instrument_number: u32,
    /// Ordered envelope nodes.
    pub nodes: Vec<PsgNode>,
    /// Index of the first *release* node. `None` means no sustain.
    pub sustain_pos: Option<usize>,
    /// Index the envelope jumps back to after the last node. `None` means
    /// "play once then end".
    pub loop_pos: Option<usize>,
    /// Default frame count for nodes with no explicit length and no slide
    /// distance.
    pub default_length: u32,
}

// ---------------------------------------------------------------------------
// Effective length
// ---------------------------------------------------------------------------

/// Resolve a node's duration in frames, replicating ctrmml's rules:
///
///  1. If `node.length` is set, use it.
///  2. Else if a slide is in effect and `target != value`, use
///     `|target - value| + 1`.
///  3. Else use `default_length`.
pub fn node_effective_length(node: &PsgNode, default_length: u32) -> u32 {
    if let Some(len) = node.length {
        return len;
    }
    if let Some(target) = node.target {
        if target != node.value {
            return (target as i32 - node.value as i32).unsigned_abs() + 1;
        }
    }
    default_length
}

// ---------------------------------------------------------------------------
// Parser
// ---------------------------------------------------------------------------

#[inline]
fn clamp_volume(v: u32) -> u8 {
    v.min(15) as u8
}

/// Read a leading run of ASCII digits as a `u32`. Returns
/// `(value, byte_length)` or `None` when the slice doesn't start with a
/// digit. Mirrors the `readDigits` helper in the TS source.
fn read_leading_digits(bytes: &[u8]) -> Option<(u32, usize)> {
    let mut end = 0;
    let mut value: u32 = 0;
    while end < bytes.len() && bytes[end].is_ascii_digit() {
        value = value
            .checked_mul(10)?
            .checked_add((bytes[end] - b'0') as u32)?;
        end += 1;
    }
    if end == 0 {
        None
    } else {
        Some((value, end))
    }
}

/// Parse a ctrmml PSG definition. The input may contain the full
/// `@N psg ...` header or just the data tokens (multi-line blocks are
/// supported). Returns `None` when no `psg` keyword is found.
pub fn parse_psg_mml(text: &str) -> Option<PsgEnvelope> {
    let mut env = PsgEnvelope {
        instrument_number: 1,
        nodes: Vec::new(),
        sustain_pos: None,
        loop_pos: None,
        default_length: 1,
    };

    let mut found_psg = false;
    let mut instrument_num: u32 = 1;

    for raw_line in text.split('\n') {
        let line = match raw_line.find(';') {
            Some(p) => &raw_line[..p],
            None => raw_line,
        };
        for token in line.split_whitespace() {
            if !found_psg {
                // `@N` header.
                if let Some(rest) = token.strip_prefix('@') {
                    if let Some((n, _)) = read_leading_digits(rest.as_bytes()) {
                        instrument_num = n;
                    }
                    continue;
                }
                // `psg` keyword (case-insensitive).
                if token.eq_ignore_ascii_case("psg") {
                    found_psg = true;
                    env.instrument_number = instrument_num;
                }
                continue;
            }

            // --- data tokens -------------------------------------------------

            let bytes = token.as_bytes();
            let first = bytes[0]; // safe: split_whitespace yields non-empty tokens.

            if first == b'/' {
                // Sustain marker. ctrmml inserts the boundary AFTER the
                // previous node; record the current node count.
                env.sustain_pos = Some(env.nodes.len());
                continue;
            }
            if first == b'|' {
                // Loop marker — same indexing rule as sustain.
                env.loop_pos = Some(env.nodes.len());
                continue;
            }
            // `l:N` — set default length.
            if (first == b'l' || first == b'L') && bytes.get(1) == Some(&b':') {
                if let Some((len, _)) = read_leading_digits(&bytes[2..]) {
                    if len > 0 {
                        env.default_length = len;
                    }
                }
                continue;
            }
            if !first.is_ascii_digit() {
                // Unknown token — skip silently to match TS behavior.
                continue;
            }

            // value[>target][:length] with permissive trailing garbage.
            let mut i = 0usize;
            let (initial_raw, consumed) = match read_leading_digits(&bytes[i..]) {
                Some(r) => r,
                None => continue,
            };
            i += consumed;
            let mut node = PsgNode {
                value: clamp_volume(initial_raw),
                target: None,
                length: None,
            };

            if bytes.get(i) == Some(&b'>') {
                i += 1;
                if let Some((target_raw, c)) = read_leading_digits(&bytes[i..]) {
                    node.target = Some(clamp_volume(target_raw));
                    i += c;
                }
            }
            if bytes.get(i) == Some(&b':') {
                i += 1;
                if let Some((len, _)) = read_leading_digits(&bytes[i..]) {
                    if len > 0 {
                        node.length = Some(len);
                    }
                }
            }
            env.nodes.push(node);
        }
    }

    if !found_psg {
        return None;
    }
    Some(env)
}

// ---------------------------------------------------------------------------
// Serializer
// ---------------------------------------------------------------------------

/// Serialize a [`PsgEnvelope`] back to a single-line ctrmml MML
/// definition (e.g. `"@5 psg 15>11:5 / 10>0:20"`).
pub fn serialize_psg_mml(env: &PsgEnvelope, instrument_number: u32) -> String {
    let mut parts: Vec<String> = Vec::with_capacity(env.nodes.len() + 4);

    // Markers placed before the i-th node when their position == i; the
    // i == nodes.len() pass handles end-of-list markers.
    for i in 0..=env.nodes.len() {
        if env.loop_pos == Some(i) {
            parts.push("|".to_string());
        }
        if env.sustain_pos == Some(i) {
            parts.push("/".to_string());
        }
        if i < env.nodes.len() {
            let node = &env.nodes[i];
            let mut tok = node.value.to_string();
            if let Some(t) = node.target {
                tok.push('>');
                tok.push_str(&t.to_string());
            }
            if let Some(l) = node.length {
                tok.push(':');
                tok.push_str(&l.to_string());
            }
            parts.push(tok);
        }
    }

    let body = parts.join(" ");
    if body.is_empty() {
        format!("@{instrument_number} psg")
    } else {
        format!("@{instrument_number} psg {body}")
    }
}

// ---------------------------------------------------------------------------
// Timeline for visualization
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimePoint {
    /// Frame offset from envelope start.
    pub frame: u32,
    /// Volume level `0..=15`.
    pub volume: u8,
}

/// Convert the envelope into a `(frame, volume)` sequence suitable for
/// drawing the envelope graph. Each node contributes two points: the
/// start and end of its segment.
pub fn compute_timeline(env: &PsgEnvelope) -> Vec<TimePoint> {
    let mut pts: Vec<TimePoint> = Vec::with_capacity(env.nodes.len() * 2);
    let mut frame: u32 = 0;
    for node in &env.nodes {
        let len = node_effective_length(node, env.default_length);
        let v0 = node.value;
        let v1 = node.target.unwrap_or(node.value);
        pts.push(TimePoint { frame, volume: v0 });
        pts.push(TimePoint {
            frame: frame + len,
            volume: v1,
        });
        frame += len;
    }
    pts
}

/// Total envelope duration in frames.
pub fn total_duration(env: &PsgEnvelope) -> u32 {
    env.nodes
        .iter()
        .map(|n| node_effective_length(n, env.default_length))
        .sum()
}

/// Frame offset where node `node_index` starts. `node_index` past the
/// end yields the total duration.
pub fn node_start_frame(env: &PsgEnvelope, node_index: usize) -> u32 {
    let cap = node_index.min(env.nodes.len());
    env.nodes[..cap]
        .iter()
        .map(|n| node_effective_length(n, env.default_length))
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- parse_psg_mml -------------------------------------------------------

    #[test]
    fn parses_simple_envelope() {
        let env = parse_psg_mml("@5 psg 15 10 5 0").unwrap();
        assert_eq!(env.instrument_number, 5);
        assert_eq!(env.nodes.len(), 4);
        assert_eq!(
            env.nodes[0],
            PsgNode {
                value: 15,
                target: None,
                length: None
            }
        );
        assert_eq!(env.sustain_pos, None);
        assert_eq!(env.loop_pos, None);
        assert_eq!(env.default_length, 1);
    }

    #[test]
    fn parses_slide() {
        let env = parse_psg_mml("@1 psg 15>0").unwrap();
        assert_eq!(env.nodes[0].value, 15);
        assert_eq!(env.nodes[0].target, Some(0));
        assert_eq!(env.nodes[0].length, None);
    }

    #[test]
    fn parses_explicit_length() {
        let env = parse_psg_mml("@1 psg 10:5").unwrap();
        assert_eq!(env.nodes[0].length, Some(5));
    }

    #[test]
    fn parses_slide_with_explicit_length() {
        let env = parse_psg_mml("@1 psg 15>0:20").unwrap();
        assert_eq!(env.nodes[0].value, 15);
        assert_eq!(env.nodes[0].target, Some(0));
        assert_eq!(env.nodes[0].length, Some(20));
    }

    #[test]
    fn parses_sustain_marker() {
        let env = parse_psg_mml("@1 psg 15 10 / 5 0").unwrap();
        assert_eq!(env.sustain_pos, Some(2));
        assert_eq!(env.nodes.len(), 4);
    }

    #[test]
    fn parses_loop_marker() {
        let env = parse_psg_mml("@1 psg 15 | 10 0").unwrap();
        assert_eq!(env.loop_pos, Some(1));
    }

    #[test]
    fn parses_default_length_directive() {
        let env = parse_psg_mml("@1 psg l:3 15 10").unwrap();
        assert_eq!(env.default_length, 3);
        // l:3 alone does not insert a node.
        assert_eq!(env.nodes.len(), 2);
    }

    #[test]
    fn clamps_value_to_15() {
        let env = parse_psg_mml("@1 psg 99").unwrap();
        assert_eq!(env.nodes[0].value, 15);
    }

    #[test]
    fn clamps_target_to_15() {
        let env = parse_psg_mml("@1 psg 0>99").unwrap();
        assert_eq!(env.nodes[0].target, Some(15));
    }

    #[test]
    fn returns_none_when_no_psg_keyword() {
        assert!(parse_psg_mml("@1 fm 1 1 1 1").is_none());
        assert!(parse_psg_mml("").is_none());
    }

    #[test]
    fn parses_across_multiple_lines() {
        let env = parse_psg_mml("@1 psg\n15 10 5 0\n").unwrap();
        assert_eq!(env.nodes.len(), 4);
    }

    #[test]
    fn strips_inline_comments() {
        let env = parse_psg_mml("@1 psg 15 10 ; release follows\n5 0").unwrap();
        assert_eq!(env.nodes.len(), 4);
    }

    #[test]
    fn permissive_trailing_garbage_on_value() {
        // `15/` should parse as value=15; the `/` after digits is ignored
        // because the value-token branch consumed everything it understood.
        let env = parse_psg_mml("@1 psg 15/ 10").unwrap();
        assert_eq!(env.nodes.len(), 2);
        assert_eq!(env.nodes[0].value, 15);
    }

    #[test]
    fn skips_unknown_tokens() {
        let env = parse_psg_mml("@1 psg 15 foo 10").unwrap();
        assert_eq!(env.nodes.len(), 2);
    }

    // ---- serialize_psg_mml ---------------------------------------------------

    #[test]
    fn serializes_simple_envelope() {
        let env = PsgEnvelope {
            instrument_number: 5,
            nodes: vec![
                PsgNode {
                    value: 15,
                    target: None,
                    length: None,
                },
                PsgNode {
                    value: 0,
                    target: None,
                    length: None,
                },
            ],
            sustain_pos: None,
            loop_pos: None,
            default_length: 1,
        };
        assert_eq!(serialize_psg_mml(&env, 5), "@5 psg 15 0");
    }

    #[test]
    fn serializes_slide_and_length() {
        let env = PsgEnvelope {
            instrument_number: 5,
            nodes: vec![PsgNode {
                value: 15,
                target: Some(0),
                length: Some(20),
            }],
            sustain_pos: None,
            loop_pos: None,
            default_length: 1,
        };
        assert_eq!(serialize_psg_mml(&env, 5), "@5 psg 15>0:20");
    }

    #[test]
    fn serializes_with_sustain_and_loop() {
        let env = PsgEnvelope {
            instrument_number: 5,
            nodes: vec![
                PsgNode {
                    value: 15,
                    target: None,
                    length: None,
                },
                PsgNode {
                    value: 8,
                    target: None,
                    length: None,
                },
                PsgNode {
                    value: 0,
                    target: None,
                    length: None,
                },
            ],
            sustain_pos: Some(2),
            loop_pos: Some(0),
            default_length: 1,
        };
        // Loop is emitted before sustain when both anchor the same index.
        assert_eq!(serialize_psg_mml(&env, 5), "@5 psg | 15 8 / 0");
    }

    #[test]
    fn parse_serialize_roundtrip() {
        let input = "@7 psg 15>10:5 / 8 8 | 4 0";
        let env = parse_psg_mml(input).unwrap();
        let out = serialize_psg_mml(&env, env.instrument_number);
        // Re-parse and compare structure rather than exact string (the
        // serializer normalizes whitespace).
        let env2 = parse_psg_mml(&out).unwrap();
        assert_eq!(env, env2);
    }

    // ---- node_effective_length ----------------------------------------------

    #[test]
    fn effective_length_explicit_wins() {
        let n = PsgNode {
            value: 5,
            target: Some(10),
            length: Some(20),
        };
        assert_eq!(node_effective_length(&n, 1), 20);
    }

    #[test]
    fn effective_length_slide_distance_plus_one() {
        let n = PsgNode {
            value: 5,
            target: Some(10),
            length: None,
        };
        assert_eq!(node_effective_length(&n, 1), 6);
    }

    #[test]
    fn effective_length_slide_to_same_value_uses_default() {
        let n = PsgNode {
            value: 5,
            target: Some(5),
            length: None,
        };
        assert_eq!(node_effective_length(&n, 3), 3);
    }

    #[test]
    fn effective_length_default_when_hold() {
        let n = PsgNode {
            value: 5,
            target: None,
            length: None,
        };
        assert_eq!(node_effective_length(&n, 7), 7);
    }

    // ---- compute_timeline / total_duration / node_start_frame ---------------

    #[test]
    fn timeline_two_points_per_node() {
        let env = parse_psg_mml("@1 psg 15:2 10:3").unwrap();
        let pts = compute_timeline(&env);
        assert_eq!(pts.len(), 4);
        assert_eq!(pts[0], TimePoint { frame: 0, volume: 15 });
        assert_eq!(pts[1], TimePoint { frame: 2, volume: 15 });
        assert_eq!(pts[2], TimePoint { frame: 2, volume: 10 });
        assert_eq!(pts[3], TimePoint { frame: 5, volume: 10 });
    }

    #[test]
    fn total_duration_sums_segment_lengths() {
        let env = parse_psg_mml("@1 psg 15:2 10:3").unwrap();
        assert_eq!(total_duration(&env), 5);
    }

    #[test]
    fn node_start_frame_zero_for_first_index() {
        let env = parse_psg_mml("@1 psg 15:2 10:3").unwrap();
        assert_eq!(node_start_frame(&env, 0), 0);
        assert_eq!(node_start_frame(&env, 1), 2);
        assert_eq!(node_start_frame(&env, 2), 5);
        // Past end → total duration.
        assert_eq!(node_start_frame(&env, 99), 5);
    }
}
