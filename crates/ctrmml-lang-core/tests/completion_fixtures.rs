//! Canonical completion fixture suite — schema validator and C1 executor.
//!
//! `tests/completion_fixtures/*.json` is the executable specification for
//! the unified completion engine described in `COMPLETION_CORE_PLAN.md`
//! (§2 item model, §3 decisions D1–D20). This harness:
//!   (a) loads every fixture file,
//!   (b) validates it against the schema below (structs + strict
//!       unknown-key rejection, all defined INSIDE this test file so
//!       nothing is added to `src/`),
//!   (c) asserts suite-level invariants: unique case names, positions
//!       within document bounds, at least one assertion per case,
//!       `decisions` entries matching `^D([1-9]|1[0-9]|20)$`, plan/data
//!       agreement, spot indices inside `count`,
//!   (d) executes all 78 cases, resolving data-bearing plans in a second
//!       call, and checks every asserted item field.
//!
//! NOTE: `std::fs` here is fine — the no-I/O rule in `Cargo.toml` applies
//! to the library (which must build for wasm32), not to integration tests.

// The schema structs carry every field the fixtures can assert; only a
// subset is read by today's validator, the rest become assertions at C1.
#![allow(dead_code)]

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use ctrmml_lang_core::{
    completion_plan, completion_resolve, render_chord, ArpeggioPattern, ChordStackMode,
    CompletionPlan, CompletionSettings, CoreCommand, CoreCompletionList, CoreItem, CoreItemKind,
    CursorTickData, DataPayload, DataRequest, FmPatchData, InsertFormat, KeySig, Pos as CorePos,
    RootAccidental, CHORDS_3, CHORDS_4,
};

const EXECUTED_FILES: &[&str] = &[
    "arpeggio.json",
    "chord.json",
    "commands_fallback.json",
    "encoding.json",
    "guards.json",
    "instrument_def.json",
    "fm_patches.json",
    "measure_fill.json",
    "meta.json",
    "pcm.json",
    "platform_commands.json",
];

const PENDING_FILES: &[(&str, &str)] = &[];

// LOUD AND INTENTIONAL: fixture/implementation disagreements belong here;
// quarantined cases must continue to mismatch, so a silent behavior change
// cannot accidentally turn them green. Never add a case merely to force CI
// green: record its compact expected/actual diff in the change handoff.
const KNOWN_DISCREPANCIES: &[&str] = &[];

// ---------------------------------------------------------------------------
// Minimal JSON value + parser
//
// `serde_json` is not a dependency of this crate (and this fixture task
// must not touch `Cargo.toml`), so the loader ships its own reader. The
// schema structs below mirror what `#[derive(Deserialize)]` would
// produce, field for field, so swapping in serde_json at C1 is a
// drop-in change.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
enum Json {
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    Arr(Vec<Json>),
    Obj(Vec<(String, Json)>),
}

impl Json {
    fn type_name(&self) -> &'static str {
        match self {
            Json::Null => "null",
            Json::Bool(_) => "bool",
            Json::Num(_) => "number",
            Json::Str(_) => "string",
            Json::Arr(_) => "array",
            Json::Obj(_) => "object",
        }
    }
}

struct JsonParser<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> JsonParser<'a> {
    fn new(text: &'a str) -> Self {
        JsonParser {
            bytes: text.as_bytes(),
            pos: 0,
        }
    }

    fn parse_document(mut self) -> Result<Json, String> {
        self.skip_ws();
        let value = self.parse_value()?;
        self.skip_ws();
        if self.pos != self.bytes.len() {
            return Err(self.err("trailing content after top-level value"));
        }
        Ok(value)
    }

    fn err(&self, msg: &str) -> String {
        // Report a line/column so a broken fixture is easy to find.
        let mut line = 1usize;
        let mut col = 1usize;
        for &b in &self.bytes[..self.pos.min(self.bytes.len())] {
            if b == b'\n' {
                line += 1;
                col = 1;
            } else {
                col += 1;
            }
        }
        format!("{msg} (at line {line}, column {col})")
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    fn skip_ws(&mut self) {
        while matches!(
            self.peek(),
            Some(b' ') | Some(b'\t') | Some(b'\n') | Some(b'\r')
        ) {
            self.pos += 1;
        }
    }

    fn expect_byte(&mut self, b: u8) -> Result<(), String> {
        if self.peek() == Some(b) {
            self.pos += 1;
            Ok(())
        } else {
            Err(self.err(&format!("expected `{}`", b as char)))
        }
    }

    fn parse_value(&mut self) -> Result<Json, String> {
        match self.peek() {
            Some(b'{') => self.parse_object(),
            Some(b'[') => self.parse_array(),
            Some(b'"') => Ok(Json::Str(self.parse_string()?)),
            Some(b't') => self.parse_literal("true", Json::Bool(true)),
            Some(b'f') => self.parse_literal("false", Json::Bool(false)),
            Some(b'n') => self.parse_literal("null", Json::Null),
            Some(_) => self.parse_number(),
            None => Err(self.err("unexpected end of input")),
        }
    }

    fn parse_literal(&mut self, word: &str, value: Json) -> Result<Json, String> {
        if self.bytes[self.pos..].starts_with(word.as_bytes()) {
            self.pos += word.len();
            Ok(value)
        } else {
            Err(self.err(&format!("expected `{word}`")))
        }
    }

    fn parse_number(&mut self) -> Result<Json, String> {
        let start = self.pos;
        if self.peek() == Some(b'-') {
            self.pos += 1;
        }
        while matches!(
            self.peek(),
            Some(b'0'..=b'9') | Some(b'.') | Some(b'e') | Some(b'E') | Some(b'+') | Some(b'-')
        ) {
            self.pos += 1;
        }
        let text = std::str::from_utf8(&self.bytes[start..self.pos])
            .map_err(|_| self.err("invalid utf-8 in number"))?;
        text.parse::<f64>()
            .map(Json::Num)
            .map_err(|_| self.err(&format!("invalid number `{text}`")))
    }

    fn parse_string(&mut self) -> Result<String, String> {
        self.expect_byte(b'"')?;
        let mut out = String::new();
        loop {
            let b = self.peek().ok_or_else(|| self.err("unterminated string"))?;
            match b {
                b'"' => {
                    self.pos += 1;
                    return Ok(out);
                }
                b'\\' => {
                    self.pos += 1;
                    let esc = self.peek().ok_or_else(|| self.err("unterminated escape"))?;
                    self.pos += 1;
                    match esc {
                        b'"' => out.push('"'),
                        b'\\' => out.push('\\'),
                        b'/' => out.push('/'),
                        b'b' => out.push('\u{8}'),
                        b'f' => out.push('\u{c}'),
                        b'n' => out.push('\n'),
                        b'r' => out.push('\r'),
                        b't' => out.push('\t'),
                        b'u' => {
                            let hi = self.parse_hex4()?;
                            let ch = if (0xD800..0xDC00).contains(&hi) {
                                // Surrogate pair.
                                if self.peek() != Some(b'\\') {
                                    return Err(self.err("lone high surrogate"));
                                }
                                self.pos += 1;
                                if self.peek() != Some(b'u') {
                                    return Err(self.err("lone high surrogate"));
                                }
                                self.pos += 1;
                                let lo = self.parse_hex4()?;
                                let combined =
                                    0x10000 + ((hi as u32 - 0xD800) << 10) + (lo as u32 - 0xDC00);
                                char::from_u32(combined)
                                    .ok_or_else(|| self.err("invalid surrogate pair"))?
                            } else {
                                char::from_u32(hi as u32)
                                    .ok_or_else(|| self.err("invalid \\u escape"))?
                            };
                            out.push(ch);
                        }
                        other => {
                            return Err(self.err(&format!("invalid escape `\\{}`", other as char)))
                        }
                    }
                }
                _ => {
                    // Copy one UTF-8 code point verbatim.
                    let len = utf8_len(b);
                    let slice = self
                        .bytes
                        .get(self.pos..self.pos + len)
                        .ok_or_else(|| self.err("truncated utf-8 sequence"))?;
                    let text = std::str::from_utf8(slice).map_err(|_| self.err("invalid utf-8"))?;
                    out.push_str(text);
                    self.pos += len;
                }
            }
        }
    }

    fn parse_hex4(&mut self) -> Result<u16, String> {
        let slice = self
            .bytes
            .get(self.pos..self.pos + 4)
            .ok_or_else(|| self.err("truncated \\u escape"))?;
        let text = std::str::from_utf8(slice).map_err(|_| self.err("invalid \\u escape"))?;
        let value = u16::from_str_radix(text, 16).map_err(|_| self.err("invalid \\u escape"))?;
        self.pos += 4;
        Ok(value)
    }

    fn parse_array(&mut self) -> Result<Json, String> {
        self.expect_byte(b'[')?;
        let mut items = Vec::new();
        self.skip_ws();
        if self.peek() == Some(b']') {
            self.pos += 1;
            return Ok(Json::Arr(items));
        }
        loop {
            self.skip_ws();
            items.push(self.parse_value()?);
            self.skip_ws();
            match self.peek() {
                Some(b',') => self.pos += 1,
                Some(b']') => {
                    self.pos += 1;
                    return Ok(Json::Arr(items));
                }
                _ => return Err(self.err("expected `,` or `]`")),
            }
        }
    }

    fn parse_object(&mut self) -> Result<Json, String> {
        self.expect_byte(b'{')?;
        let mut entries: Vec<(String, Json)> = Vec::new();
        self.skip_ws();
        if self.peek() == Some(b'}') {
            self.pos += 1;
            return Ok(Json::Obj(entries));
        }
        loop {
            self.skip_ws();
            let key = self.parse_string()?;
            if entries.iter().any(|(k, _)| *k == key) {
                return Err(self.err(&format!("duplicate key `{key}`")));
            }
            self.skip_ws();
            self.expect_byte(b':')?;
            self.skip_ws();
            let value = self.parse_value()?;
            entries.push((key, value));
            self.skip_ws();
            match self.peek() {
                Some(b',') => self.pos += 1,
                Some(b'}') => {
                    self.pos += 1;
                    return Ok(Json::Obj(entries));
                }
                _ => return Err(self.err("expected `,` or `}`")),
            }
        }
    }
}

fn utf8_len(first: u8) -> usize {
    match first {
        0x00..=0x7F => 1,
        0xC0..=0xDF => 2,
        0xE0..=0xEF => 3,
        _ => 4,
    }
}

// ---------------------------------------------------------------------------
// Strict object reader (the `deny_unknown_fields` equivalent)
// ---------------------------------------------------------------------------

struct Obj<'a> {
    path: String,
    entries: &'a [(String, Json)],
    used: BTreeSet<String>,
}

impl<'a> Obj<'a> {
    fn new(path: &str, value: &'a Json) -> Result<Self, String> {
        match value {
            Json::Obj(entries) => Ok(Obj {
                path: path.to_string(),
                entries,
                used: BTreeSet::new(),
            }),
            other => Err(format!(
                "{path}: expected object, found {}",
                other.type_name()
            )),
        }
    }

    fn opt(&mut self, key: &str) -> Option<&'a Json> {
        self.used.insert(key.to_string());
        self.entries.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    fn req(&mut self, key: &str) -> Result<&'a Json, String> {
        self.opt(key)
            .ok_or_else(|| format!("{}: missing required field `{key}`", self.path))
    }

    fn finish(self) -> Result<(), String> {
        for (k, _) in self.entries {
            if !self.used.contains(k) {
                return Err(format!("{}: unknown field `{k}`", self.path));
            }
        }
        Ok(())
    }
}

fn as_str(path: &str, value: &Json) -> Result<String, String> {
    match value {
        Json::Str(s) => Ok(s.clone()),
        other => Err(format!(
            "{path}: expected string, found {}",
            other.type_name()
        )),
    }
}

fn as_bool(path: &str, value: &Json) -> Result<bool, String> {
    match value {
        Json::Bool(b) => Ok(*b),
        other => Err(format!(
            "{path}: expected bool, found {}",
            other.type_name()
        )),
    }
}

fn as_u32(path: &str, value: &Json) -> Result<u32, String> {
    match value {
        Json::Num(n) if *n >= 0.0 && n.fract() == 0.0 && *n <= u32::MAX as f64 => Ok(*n as u32),
        other => Err(format!(
            "{path}: expected non-negative integer, found {}",
            other.type_name()
        )),
    }
}

fn as_usize(path: &str, value: &Json) -> Result<usize, String> {
    as_u32(path, value).map(|v| v as usize)
}

fn as_arr<'a>(path: &str, value: &'a Json) -> Result<&'a [Json], String> {
    match value {
        Json::Arr(items) => Ok(items),
        other => Err(format!(
            "{path}: expected array, found {}",
            other.type_name()
        )),
    }
}

fn one_of(path: &str, value: &str, allowed: &[&str]) -> Result<(), String> {
    if allowed.contains(&value) {
        Ok(())
    } else {
        Err(format!(
            "{path}: `{value}` is not one of [{}]",
            allowed.join(", ")
        ))
    }
}

// ---------------------------------------------------------------------------
// Schema (mirrors COMPLETION_CORE_PLAN.md §2 / the fixture README)
// ---------------------------------------------------------------------------

const ITEM_KINDS: &[&str] = &[
    "function",
    "keyword",
    "value",
    "property",
    "type_parameter",
    "struct",
    "file",
    "snippet",
    "text",
];
const INSERT_FORMATS: &[&str] = &["plain", "snippet"];
const ARPEGGIO_PATTERNS: &[&str] = &["up", "down", "updown", "downup", "alberti"];
const CHORD_STACK_MODES: &[&str] = &["stack_up", "plain"];
const DATA_KINDS: &[&str] = &["fm_patches", "pcm_files", "pcm_paths", "cursor_tick"];
const NEEDS_KINDS: &[&str] = &["fm_patches", "pcm_files", "pcm_paths", "cursor_tick"];
const ASSERT_ABSENT_FIELDS: &[&str] = &[
    "command",
    "documentation",
    "label_description",
    "detail",
    "sort_text",
    "filter_text",
];
/// D1: trigger characters = union of both implementations.
const TRIGGER_CHARS: &[char] = &['#', '@', '"', ' ', '\'', '/', '.', '|', '{', '+', '-', '='];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Pos {
    line: u32,
    character: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Settings {
    arpeggio_enabled: bool,
    arpeggio_pattern: String,
    chord_stack_mode: String,
    fm_picker_hierarchy: bool,
}

impl Default for Settings {
    /// Suite defaults = megamml's current defaults, see the fixture README.
    fn default() -> Self {
        Settings {
            arpeggio_enabled: false,
            arpeggio_pattern: "up".to_string(),
            chord_stack_mode: "stack_up".to_string(),
            fm_picker_hierarchy: false,
        }
    }
}

#[derive(Debug, Clone)]
struct FmPatch {
    rel_path: String,
    name: Option<String>,
    mml: String,
    has_macros: bool,
}

#[derive(Debug, Clone)]
enum Data {
    FmPatches(Vec<FmPatch>),
    PcmFiles(Vec<String>),
    PcmPaths(Vec<String>),
    CursorTick {
        tick: Option<u32>,
        ppqn: Option<u32>,
    },
}

impl Data {
    fn kind(&self) -> &'static str {
        match self {
            Data::FmPatches(_) => "fm_patches",
            Data::PcmFiles(_) => "pcm_files",
            Data::PcmPaths(_) => "pcm_paths",
            Data::CursorTick { .. } => "cursor_tick",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Plan {
    Done,
    Needs(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Range {
    start_line: Option<u32>,
    start_character: u32,
    end_line: Option<u32>,
    end_character: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Edit {
    start_line: u32,
    start_character: u32,
    end_line: u32,
    end_character: u32,
    new_text: String,
}

#[derive(Debug, Clone, Default)]
struct Item {
    label: String,
    kind: Option<String>,
    detail: Option<String>,
    documentation_contains: Option<String>,
    insert: Option<String>,
    insert_format: Option<String>,
    filter_text: Option<String>,
    sort_text: Option<String>,
    preselect: Option<bool>,
    range: Option<Range>,
    additional_edits: Option<Vec<Edit>>,
    command: Option<String>,
    assert_absent: Option<Vec<String>>,
}

#[derive(Debug, Clone)]
struct SpotItem {
    index: usize,
    item: Item,
}

#[derive(Debug, Clone)]
struct NamedBody {
    root: String,
    suffix: String,
    size: usize,
    key_sig: String,
    body: String,
}

#[derive(Debug, Clone, Default)]
struct Expect {
    plan: Option<Plan>,
    is_incomplete: Option<bool>,
    items: Option<Vec<Item>>,
    count: Option<usize>,
    items_spot: Option<Vec<SpotItem>>,
    named_body: Option<NamedBody>,
}

#[derive(Debug, Clone)]
struct Case {
    name: String,
    notes: String,
    decisions: Vec<String>,
    doc: String,
    pos: Pos,
    trigger: Option<String>,
    settings: Settings,
    data: Option<Data>,
    /// `None` == the fixture is an OPEN question (plan ambiguity).
    expect: Option<Expect>,
}

// ---------------------------------------------------------------------------
// Parsing the schema out of Json
// ---------------------------------------------------------------------------

fn parse_case(path: &str, value: &Json) -> Result<Case, String> {
    let mut obj = Obj::new(path, value)?;
    let name = as_str(&format!("{path}.name"), obj.req("name")?)?;
    let cpath = format!("{path}[{name}]");
    let notes = as_str(&format!("{cpath}.notes"), obj.req("notes")?)?;

    let mut decisions = Vec::new();
    for (i, d) in as_arr(&format!("{cpath}.decisions"), obj.req("decisions")?)?
        .iter()
        .enumerate()
    {
        decisions.push(as_str(&format!("{cpath}.decisions[{i}]"), d)?);
    }

    let doc = as_str(&format!("{cpath}.doc"), obj.req("doc")?)?;
    let pos = parse_pos(&format!("{cpath}.pos"), obj.req("pos")?)?;

    let trigger = match obj.opt("trigger") {
        None | Some(Json::Null) => None,
        Some(v) => Some(as_str(&format!("{cpath}.trigger"), v)?),
    };

    let settings = match obj.opt("settings") {
        None | Some(Json::Null) => Settings::default(),
        Some(v) => parse_settings(&format!("{cpath}.settings"), v)?,
    };

    let data = match obj.opt("data") {
        None | Some(Json::Null) => None,
        Some(v) => Some(parse_data(&format!("{cpath}.data"), v)?),
    };

    let expect = match obj.req("expect")? {
        Json::Null => None,
        v => Some(parse_expect(&format!("{cpath}.expect"), v)?),
    };

    obj.finish()?;

    Ok(Case {
        name,
        notes,
        decisions,
        doc,
        pos,
        trigger,
        settings,
        data,
        expect,
    })
}

fn parse_pos(path: &str, value: &Json) -> Result<Pos, String> {
    let mut obj = Obj::new(path, value)?;
    let line = as_u32(&format!("{path}.line"), obj.req("line")?)?;
    let character = as_u32(&format!("{path}.character"), obj.req("character")?)?;
    obj.finish()?;
    Ok(Pos { line, character })
}

fn parse_settings(path: &str, value: &Json) -> Result<Settings, String> {
    let mut obj = Obj::new(path, value)?;
    let arpeggio_enabled = as_bool(
        &format!("{path}.arpeggio_enabled"),
        obj.req("arpeggio_enabled")?,
    )?;
    let arpeggio_pattern = as_str(
        &format!("{path}.arpeggio_pattern"),
        obj.req("arpeggio_pattern")?,
    )?;
    one_of(
        &format!("{path}.arpeggio_pattern"),
        &arpeggio_pattern,
        ARPEGGIO_PATTERNS,
    )?;
    let chord_stack_mode = as_str(
        &format!("{path}.chord_stack_mode"),
        obj.req("chord_stack_mode")?,
    )?;
    one_of(
        &format!("{path}.chord_stack_mode"),
        &chord_stack_mode,
        CHORD_STACK_MODES,
    )?;
    let fm_picker_hierarchy = as_bool(
        &format!("{path}.fm_picker_hierarchy"),
        obj.req("fm_picker_hierarchy")?,
    )?;
    obj.finish()?;
    Ok(Settings {
        arpeggio_enabled,
        arpeggio_pattern,
        chord_stack_mode,
        fm_picker_hierarchy,
    })
}

fn parse_data(path: &str, value: &Json) -> Result<Data, String> {
    let mut obj = Obj::new(path, value)?;
    let kind = as_str(&format!("{path}.kind"), obj.req("kind")?)?;
    one_of(&format!("{path}.kind"), &kind, DATA_KINDS)?;
    let data = match kind.as_str() {
        "fm_patches" => {
            let mut patches = Vec::new();
            for (i, p) in as_arr(&format!("{path}.patches"), obj.req("patches")?)?
                .iter()
                .enumerate()
            {
                let ppath = format!("{path}.patches[{i}]");
                let mut pobj = Obj::new(&ppath, p)?;
                let rel_path = as_str(&format!("{ppath}.rel_path"), pobj.req("rel_path")?)?;
                let name = match pobj.req("name")? {
                    Json::Null => None,
                    v => Some(as_str(&format!("{ppath}.name"), v)?),
                };
                let mml = as_str(&format!("{ppath}.mml"), pobj.req("mml")?)?;
                let has_macros = as_bool(&format!("{ppath}.has_macros"), pobj.req("has_macros")?)?;
                pobj.finish()?;
                patches.push(FmPatch {
                    rel_path,
                    name,
                    mml,
                    has_macros,
                });
            }
            Data::FmPatches(patches)
        }
        "pcm_files" | "pcm_paths" => {
            let mut paths = Vec::new();
            for (i, p) in as_arr(&format!("{path}.paths"), obj.req("paths")?)?
                .iter()
                .enumerate()
            {
                paths.push(as_str(&format!("{path}.paths[{i}]"), p)?);
            }
            if kind == "pcm_files" {
                Data::PcmFiles(paths)
            } else {
                Data::PcmPaths(paths)
            }
        }
        "cursor_tick" => {
            let tick = match obj.req("tick")? {
                Json::Null => None,
                v => Some(as_u32(&format!("{path}.tick"), v)?),
            };
            let ppqn = match obj.opt("ppqn") {
                None | Some(Json::Null) => None,
                Some(v) => Some(as_u32(&format!("{path}.ppqn"), v)?),
            };
            if tick.is_some() && ppqn.is_none() {
                return Err(format!("{path}: `ppqn` is required when `tick` is set"));
            }
            Data::CursorTick { tick, ppqn }
        }
        _ => unreachable!("kind already validated"),
    };
    obj.finish()?;
    Ok(data)
}

fn parse_expect(path: &str, value: &Json) -> Result<Expect, String> {
    let mut obj = Obj::new(path, value)?;
    let plan = match obj.opt("plan") {
        None | Some(Json::Null) => None,
        Some(Json::Str(s)) if s == "done" => Some(Plan::Done),
        Some(Json::Str(s)) => {
            return Err(format!(
                "{path}.plan: expected \"done\" or {{needs}}, found `{s}`"
            ))
        }
        Some(v) => {
            let ppath = format!("{path}.plan");
            let mut pobj = Obj::new(&ppath, v)?;
            let needs = as_str(&format!("{ppath}.needs"), pobj.req("needs")?)?;
            one_of(&format!("{ppath}.needs"), &needs, NEEDS_KINDS)?;
            pobj.finish()?;
            Some(Plan::Needs(needs))
        }
    };
    let is_incomplete = match obj.opt("is_incomplete") {
        None | Some(Json::Null) => None,
        Some(v) => Some(as_bool(&format!("{path}.is_incomplete"), v)?),
    };
    let items = match obj.opt("items") {
        None | Some(Json::Null) => None,
        Some(v) => {
            let mut out = Vec::new();
            for (i, it) in as_arr(&format!("{path}.items"), v)?.iter().enumerate() {
                out.push(parse_item(&format!("{path}.items[{i}]"), it)?);
            }
            Some(out)
        }
    };
    let count = match obj.opt("count") {
        None | Some(Json::Null) => None,
        Some(v) => Some(as_usize(&format!("{path}.count"), v)?),
    };
    let items_spot = match obj.opt("items_spot") {
        None | Some(Json::Null) => None,
        Some(v) => {
            let mut out = Vec::new();
            for (i, it) in as_arr(&format!("{path}.items_spot"), v)?.iter().enumerate() {
                let spath = format!("{path}.items_spot[{i}]");
                let mut sobj = Obj::new(&spath, it)?;
                let index = as_usize(&format!("{spath}.index"), sobj.req("index")?)?;
                // `index` is consumed here; the rest of the object is the item.
                sobj.finish_ignoring_item_fields();
                let item = parse_item_allow_index(&spath, it)?;
                out.push(SpotItem { index, item });
            }
            Some(out)
        }
    };
    let named_body = match obj.opt("named_body") {
        None | Some(Json::Null) => None,
        Some(v) => {
            let npath = format!("{path}.named_body");
            let mut nobj = Obj::new(&npath, v)?;
            let root = as_str(&format!("{npath}.root"), nobj.req("root")?)?;
            let suffix = as_str(&format!("{npath}.suffix"), nobj.req("suffix")?)?;
            let size = as_usize(&format!("{npath}.size"), nobj.req("size")?)?;
            if !matches!(size, 3 | 4) {
                return Err(format!("{npath}.size: expected 3 or 4, found {size}"));
            }
            let key_sig = as_str(&format!("{npath}.key_sig"), nobj.req("key_sig")?)?;
            one_of(&format!("{npath}.key_sig"), &key_sig, &["none"])?;
            let body = as_str(&format!("{npath}.body"), nobj.req("body")?)?;
            nobj.finish()?;
            Some(NamedBody {
                root,
                suffix,
                size,
                key_sig,
                body,
            })
        }
    };
    obj.finish()?;
    Ok(Expect {
        plan,
        is_incomplete,
        items,
        count,
        items_spot,
        named_body,
    })
}

impl<'a> Obj<'a> {
    /// Spot entries are `{index, ...item fields}`; the item fields are
    /// validated by `parse_item_allow_index`, so unknown-key checking is
    /// deferred to that pass.
    fn finish_ignoring_item_fields(self) {}
}

fn parse_item(path: &str, value: &Json) -> Result<Item, String> {
    parse_item_inner(path, value, false)
}

fn parse_item_allow_index(path: &str, value: &Json) -> Result<Item, String> {
    parse_item_inner(path, value, true)
}

fn parse_item_inner(path: &str, value: &Json, allow_index: bool) -> Result<Item, String> {
    let mut obj = Obj::new(path, value)?;
    if allow_index {
        let _ = obj.opt("index");
    }
    let label = as_str(&format!("{path}.label"), obj.req("label")?)?;
    let kind = match obj.opt("kind") {
        None | Some(Json::Null) => None,
        Some(v) => {
            let k = as_str(&format!("{path}.kind"), v)?;
            one_of(&format!("{path}.kind"), &k, ITEM_KINDS)?;
            Some(k)
        }
    };
    let detail = match obj.opt("detail") {
        None | Some(Json::Null) => None,
        Some(v) => Some(as_str(&format!("{path}.detail"), v)?),
    };
    let documentation_contains = match obj.opt("documentation_contains") {
        None | Some(Json::Null) => None,
        Some(v) => Some(as_str(&format!("{path}.documentation_contains"), v)?),
    };
    let insert = match obj.opt("insert") {
        None | Some(Json::Null) => None,
        Some(v) => Some(as_str(&format!("{path}.insert"), v)?),
    };
    let insert_format = match obj.opt("insert_format") {
        None | Some(Json::Null) => None,
        Some(v) => {
            let f = as_str(&format!("{path}.insert_format"), v)?;
            one_of(&format!("{path}.insert_format"), &f, INSERT_FORMATS)?;
            Some(f)
        }
    };
    let filter_text = match obj.opt("filter_text") {
        None | Some(Json::Null) => None,
        Some(v) => Some(as_str(&format!("{path}.filter_text"), v)?),
    };
    let sort_text = match obj.opt("sort_text") {
        None | Some(Json::Null) => None,
        Some(v) => Some(as_str(&format!("{path}.sort_text"), v)?),
    };
    let preselect = match obj.opt("preselect") {
        None | Some(Json::Null) => None,
        Some(v) => Some(as_bool(&format!("{path}.preselect"), v)?),
    };
    let range = match obj.opt("range") {
        None | Some(Json::Null) => None,
        Some(v) => {
            let rpath = format!("{path}.range");
            let mut robj = Obj::new(&rpath, v)?;
            let start_line = match robj.opt("start_line") {
                None | Some(Json::Null) => None,
                Some(v) => Some(as_u32(&format!("{rpath}.start_line"), v)?),
            };
            let start_character = as_u32(
                &format!("{rpath}.start_character"),
                robj.req("start_character")?,
            )?;
            let end_line = match robj.opt("end_line") {
                None | Some(Json::Null) => None,
                Some(v) => Some(as_u32(&format!("{rpath}.end_line"), v)?),
            };
            let end_character = as_u32(
                &format!("{rpath}.end_character"),
                robj.req("end_character")?,
            )?;
            robj.finish()?;
            Some(Range {
                start_line,
                start_character,
                end_line,
                end_character,
            })
        }
    };
    let additional_edits = match obj.opt("additional_edits") {
        None | Some(Json::Null) => None,
        Some(v) => {
            let mut edits = Vec::new();
            for (i, e) in as_arr(&format!("{path}.additional_edits"), v)?
                .iter()
                .enumerate()
            {
                let epath = format!("{path}.additional_edits[{i}]");
                let mut eobj = Obj::new(&epath, e)?;
                let start_line = as_u32(&format!("{epath}.start_line"), eobj.req("start_line")?)?;
                let start_character = as_u32(
                    &format!("{epath}.start_character"),
                    eobj.req("start_character")?,
                )?;
                let end_line = as_u32(&format!("{epath}.end_line"), eobj.req("end_line")?)?;
                let end_character = as_u32(
                    &format!("{epath}.end_character"),
                    eobj.req("end_character")?,
                )?;
                let new_text = as_str(&format!("{epath}.new_text"), eobj.req("new_text")?)?;
                eobj.finish()?;
                edits.push(Edit {
                    start_line,
                    start_character,
                    end_line,
                    end_character,
                    new_text,
                });
            }
            Some(edits)
        }
    };
    let command = match obj.opt("command") {
        None | Some(Json::Null) => None,
        Some(v) => {
            let c = as_str(&format!("{path}.command"), v)?;
            one_of(&format!("{path}.command"), &c, &["trigger_suggest"])?;
            Some(c)
        }
    };
    let assert_absent = match obj.opt("assert_absent") {
        None | Some(Json::Null) => None,
        Some(v) => {
            let mut fields = Vec::new();
            for (i, field) in as_arr(&format!("{path}.assert_absent"), v)?
                .iter()
                .enumerate()
            {
                let field = as_str(&format!("{path}.assert_absent[{i}]"), field)?;
                one_of(
                    &format!("{path}.assert_absent[{i}]"),
                    &field,
                    ASSERT_ABSENT_FIELDS,
                )?;
                if fields.contains(&field) {
                    return Err(format!("{path}.assert_absent: duplicate field `{field}`"));
                }
                fields.push(field);
            }
            Some(fields)
        }
    };
    obj.finish()?;
    Ok(Item {
        label,
        kind,
        detail,
        documentation_contains,
        insert,
        insert_format,
        filter_text,
        sort_text,
        preselect,
        range,
        additional_edits,
        command,
        assert_absent,
    })
}

// ---------------------------------------------------------------------------
// Suite invariants
// ---------------------------------------------------------------------------

fn utf16_line_lengths(doc: &str) -> Vec<usize> {
    doc.split('\n')
        .map(|line| {
            line.strip_suffix('\r')
                .unwrap_or(line)
                .encode_utf16()
                .count()
        })
        .collect()
}

fn is_decision_ref(text: &str) -> bool {
    // ^D([1-9]|1[0-9]|20)$
    let Some(rest) = text.strip_prefix('D') else {
        return false;
    };
    match rest.parse::<u32>() {
        Ok(n) => (1..=20).contains(&n) && rest == n.to_string(),
        Err(_) => false,
    }
}

fn is_snake_case(text: &str) -> bool {
    !text.is_empty()
        && text
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
        && !text.starts_with('_')
        && !text.ends_with('_')
}

fn validate_case(file: &str, case: &Case) -> Result<(), String> {
    let id = format!("{file}[{}]", case.name);

    if !is_snake_case(&case.name) {
        return Err(format!("{id}: case name must be snake_case"));
    }
    if case.notes.trim().is_empty() {
        return Err(format!("{id}: `notes` must explain the case"));
    }
    for d in &case.decisions {
        if !is_decision_ref(d) {
            return Err(format!(
                "{id}: decision `{d}` does not match ^D([1-9]|1[0-9]|20)$"
            ));
        }
    }
    if let Some(trigger) = &case.trigger {
        let mut chars = trigger.chars();
        let (Some(c), None) = (chars.next(), chars.next()) else {
            return Err(format!("{id}: `trigger` must be a single character"));
        };
        if !TRIGGER_CHARS.contains(&c) {
            return Err(format!(
                "{id}: `{c}` is not one of the D1 trigger characters"
            ));
        }
    }

    // Position within document bounds (UTF-16 units, plan §2).
    let lens = utf16_line_lengths(&case.doc);
    let line = case.pos.line as usize;
    if line >= lens.len() {
        return Err(format!(
            "{id}: pos.line {} is past the last line ({} lines)",
            case.pos.line,
            lens.len()
        ));
    }
    if case.pos.character as usize > lens[line] {
        return Err(format!(
            "{id}: pos.character {} is past end of line {} (length {})",
            case.pos.character, case.pos.line, lens[line]
        ));
    }

    let Some(expect) = &case.expect else {
        // OPEN case: nothing else to check; `notes` must say so.
        if !case.notes.starts_with("OPEN:") {
            return Err(format!(
                "{id}: `expect: null` requires `notes` to start with \"OPEN:\""
            ));
        }
        return Ok(());
    };

    // At least one assertion.
    let assertions = expect.plan.is_some() as u32
        + expect.is_incomplete.is_some() as u32
        + expect.items.is_some() as u32
        + expect.count.is_some() as u32
        + expect.items_spot.is_some() as u32;
    if assertions == 0 {
        return Err(format!("{id}: `expect` asserts nothing"));
    }

    if expect.items.is_some() && expect.count.is_some() {
        return Err(format!(
            "{id}: use either the exact `items` list or `count` + `items_spot`, not both"
        ));
    }
    if expect.items_spot.is_some() && expect.count.is_none() {
        return Err(format!("{id}: `items_spot` requires `count`"));
    }

    if let (Some(count), Some(spots)) = (expect.count, &expect.items_spot) {
        let mut seen = BTreeSet::new();
        for spot in spots {
            if spot.index >= count {
                return Err(format!(
                    "{id}: items_spot index {} is outside count {count}",
                    spot.index
                ));
            }
            if !seen.insert(spot.index) {
                return Err(format!("{id}: duplicate items_spot index {}", spot.index));
            }
        }
    }

    // Ranges must be well formed.
    let mut all_items: Vec<&Item> = Vec::new();
    if let Some(items) = &expect.items {
        all_items.extend(items.iter());
    }
    if let Some(spots) = &expect.items_spot {
        all_items.extend(spots.iter().map(|s| &s.item));
    }
    for item in all_items {
        if let Some(range) = &item.range {
            let start_line = range.start_line.unwrap_or(case.pos.line);
            let end_line = range.end_line.unwrap_or(start_line);
            if end_line < start_line
                || (end_line == start_line && range.end_character < range.start_character)
            {
                return Err(format!("{id}: item `{}` has an inverted range", item.label));
            }
        }
        if let Some(edits) = &item.additional_edits {
            for edit in edits {
                if edit.end_line < edit.start_line
                    || (edit.end_line == edit.start_line
                        && edit.end_character < edit.start_character)
                {
                    return Err(format!(
                        "{id}: item `{}` has an inverted additional edit",
                        item.label
                    ));
                }
                if edit.end_line as usize >= lens.len() {
                    return Err(format!(
                        "{id}: item `{}` edits line {} but the document has {} lines",
                        item.label,
                        edit.end_line,
                        lens.len()
                    ));
                }
            }
        }
    }

    // Plan / data agreement.
    match (&expect.plan, &case.data) {
        (Some(Plan::Done), Some(data)) => {
            return Err(format!(
                "{id}: plan is Done but the case supplies `{}` data",
                data.kind()
            ))
        }
        (Some(Plan::Needs(needs)), Some(data)) if needs != data.kind() => {
            return Err(format!(
                "{id}: plan needs `{needs}` but the case supplies `{}` data",
                data.kind()
            ))
        }
        (None, Some(_)) => {
            return Err(format!(
                "{id}: a case with resolve data must state the expected plan"
            ))
        }
        _ => {}
    }
    if case.data.is_none() && (expect.items.is_some() || expect.count.is_some()) {
        if let Some(Plan::Needs(needs)) = &expect.plan {
            return Err(format!(
                "{id}: plan needs `{needs}` but no `data` payload is supplied for the resolve phase"
            ));
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Loader + entry point
// ---------------------------------------------------------------------------

fn fixture_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("completion_fixtures")
}

fn load_all() -> Result<Vec<(String, Vec<Case>)>, String> {
    let dir = fixture_dir();
    let mut files: Vec<PathBuf> = fs::read_dir(&dir)
        .map_err(|e| format!("cannot read {}: {e}", dir.display()))?
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|p| p.extension().map(|e| e == "json").unwrap_or(false))
        .collect();
    files.sort();

    let mut out = Vec::new();
    for path in files {
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("<unknown>")
            .to_string();
        let text = fs::read_to_string(&path).map_err(|e| format!("{name}: {e}"))?;
        let value = JsonParser::new(&text)
            .parse_document()
            .map_err(|e| format!("{name}: {e}"))?;
        let entries = as_arr(&name, &value)?;
        let mut cases = Vec::new();
        for (i, entry) in entries.iter().enumerate() {
            cases.push(parse_case(&format!("{name}[{i}]"), entry)?);
        }
        out.push((name, cases));
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Completion execution assertions
// ---------------------------------------------------------------------------

fn core_settings(settings: &Settings) -> CompletionSettings {
    CompletionSettings {
        arpeggio_enabled: settings.arpeggio_enabled,
        arpeggio_pattern: match settings.arpeggio_pattern.as_str() {
            "up" => ArpeggioPattern::Up,
            "down" => ArpeggioPattern::Down,
            "updown" => ArpeggioPattern::UpDown,
            "downup" => ArpeggioPattern::DownUp,
            "alberti" => ArpeggioPattern::Alberti,
            other => unreachable!("schema rejected arpeggio pattern `{other}`"),
        },
        chord_stack_mode: match settings.chord_stack_mode.as_str() {
            "stack_up" => ChordStackMode::StackUp,
            "plain" => ChordStackMode::Plain,
            other => unreachable!("schema rejected chord stack mode `{other}`"),
        },
        fm_picker_hierarchy: settings.fm_picker_hierarchy,
    }
}

fn core_data(data: &Data) -> DataPayload {
    match data {
        Data::FmPatches(patches) => DataPayload::FmPatches(
            patches
                .iter()
                .map(|patch| FmPatchData {
                    rel_path: patch.rel_path.clone(),
                    name: patch.name.clone(),
                    mml: patch.mml.clone(),
                    has_macros: patch.has_macros,
                })
                .collect(),
        ),
        Data::PcmFiles(paths) => DataPayload::PcmFiles(paths.clone()),
        Data::PcmPaths(paths) => DataPayload::PcmPaths(paths.clone()),
        Data::CursorTick { tick, ppqn } => {
            DataPayload::CursorTick(tick.map(|tick| CursorTickData {
                tick,
                ppqn: ppqn.expect("schema requires ppqn with a cursor tick"),
            }))
        }
    }
}

fn request_kind(request: &DataRequest) -> &'static str {
    match request {
        DataRequest::PcmPaths => "pcm_paths",
        DataRequest::PcmFiles => "pcm_files",
        DataRequest::FmPatches { .. } => "fm_patches",
        DataRequest::CursorTick => "cursor_tick",
    }
}

fn item_kind(kind: CoreItemKind) -> &'static str {
    match kind {
        CoreItemKind::Function => "function",
        CoreItemKind::Keyword => "keyword",
        CoreItemKind::Value => "value",
        CoreItemKind::Property => "property",
        CoreItemKind::TypeParameter => "type_parameter",
        CoreItemKind::Struct => "struct",
        CoreItemKind::File => "file",
        CoreItemKind::Snippet => "snippet",
        CoreItemKind::Text => "text",
    }
}

fn insert_format(format: InsertFormat) -> &'static str {
    match format {
        InsertFormat::PlainText => "plain",
        InsertFormat::Snippet => "snippet",
    }
}

fn command_name(command: Option<CoreCommand>) -> Option<&'static str> {
    command.map(|command| match command {
        CoreCommand::TriggerSuggest => "trigger_suggest",
    })
}

fn push_mismatch(
    mismatches: &mut Vec<String>,
    id: &str,
    field: &str,
    expected: impl std::fmt::Debug,
    actual: impl std::fmt::Debug,
) {
    mismatches.push(format!(
        "{id}: {field}: expected {expected:?}, actual {actual:?}"
    ));
}

fn compare_item(
    id: &str,
    index: usize,
    expected: &Item,
    actual: &CoreItem,
    cursor_line: u32,
    mismatches: &mut Vec<String>,
) {
    let field = |name: &str| format!("item[{index}].{name}");

    if expected.label != actual.label {
        push_mismatch(
            mismatches,
            id,
            &field("label"),
            &expected.label,
            &actual.label,
        );
    }
    if let Some(expected) = &expected.kind {
        let actual = item_kind(actual.kind);
        if expected != actual {
            push_mismatch(mismatches, id, &field("kind"), expected, actual);
        }
    }
    if let Some(expected) = &expected.detail {
        if actual.detail.as_ref() != Some(expected) {
            push_mismatch(mismatches, id, &field("detail"), expected, &actual.detail);
        }
    }
    if let Some(expected) = &expected.documentation_contains {
        let matches = actual
            .documentation
            .as_deref()
            .is_some_and(|documentation| documentation.contains(expected));
        if !matches {
            push_mismatch(
                mismatches,
                id,
                &field("documentation_contains"),
                expected,
                &actual.documentation,
            );
        }
    }
    if let Some(expected) = &expected.insert {
        if expected != &actual.insert.text {
            push_mismatch(
                mismatches,
                id,
                &field("insert"),
                expected,
                &actual.insert.text,
            );
        }
    }
    if let Some(expected) = &expected.insert_format {
        let actual = insert_format(actual.insert.format);
        if expected != actual {
            push_mismatch(mismatches, id, &field("insert_format"), expected, actual);
        }
    }
    if let Some(expected) = &expected.filter_text {
        if actual.filter_text.as_ref() != Some(expected) {
            push_mismatch(
                mismatches,
                id,
                &field("filter_text"),
                expected,
                &actual.filter_text,
            );
        }
    }
    if let Some(expected) = &expected.sort_text {
        if actual.sort_text.as_ref() != Some(expected) {
            push_mismatch(
                mismatches,
                id,
                &field("sort_text"),
                expected,
                &actual.sort_text,
            );
        }
    }
    if let Some(expected) = expected.preselect {
        if expected != actual.preselect {
            push_mismatch(
                mismatches,
                id,
                &field("preselect"),
                expected,
                actual.preselect,
            );
        }
    }
    if let Some(expected) = &expected.range {
        let expected = (
            expected.start_line.unwrap_or(cursor_line),
            expected.start_character,
            expected.end_line.unwrap_or(cursor_line),
            expected.end_character,
        );
        let actual = (
            actual.edit_range.start.line,
            actual.edit_range.start.character,
            actual.edit_range.end.line,
            actual.edit_range.end.character,
        );
        if expected != actual {
            push_mismatch(mismatches, id, &field("range"), expected, actual);
        }
    }
    if let Some(expected) = &expected.additional_edits {
        let actual: Vec<Edit> = actual
            .additional_edits
            .iter()
            .map(|edit| Edit {
                start_line: edit.range.start.line,
                start_character: edit.range.start.character,
                end_line: edit.range.end.line,
                end_character: edit.range.end.character,
                new_text: edit.new_text.clone(),
            })
            .collect();
        if expected != &actual {
            push_mismatch(mismatches, id, &field("additional_edits"), expected, actual);
        }
    }
    if let Some(expected) = &expected.command {
        let actual = command_name(actual.command);
        if actual != Some(expected.as_str()) {
            push_mismatch(mismatches, id, &field("command"), expected, actual);
        }
    }
    if let Some(fields) = &expected.assert_absent {
        for absent in fields {
            let present = match absent.as_str() {
                "command" => actual.command.is_some(),
                "documentation" => actual.documentation.is_some(),
                "label_description" => actual.label_description.is_some(),
                "detail" => actual.detail.is_some(),
                "sort_text" => actual.sort_text.is_some(),
                "filter_text" => actual.filter_text.is_some(),
                other => unreachable!("schema rejected assert_absent field `{other}`"),
            };
            if present {
                push_mismatch(
                    mismatches,
                    id,
                    &field(&format!("assert_absent.{absent}")),
                    "absent",
                    "present",
                );
            }
        }
    }
}

fn compare_list(
    id: &str,
    case: &Case,
    expected: &Expect,
    actual: &CoreCompletionList,
    mismatches: &mut Vec<String>,
) {
    if let Some(expected) = expected.is_incomplete {
        if expected != actual.is_incomplete {
            push_mismatch(
                mismatches,
                id,
                "is_incomplete",
                expected,
                actual.is_incomplete,
            );
        }
    }

    if let Some(expected_items) = &expected.items {
        if expected_items.len() != actual.items.len() {
            push_mismatch(
                mismatches,
                id,
                "items.length",
                expected_items.len(),
                actual.items.len(),
            );
        }
        for (index, (expected_item, actual_item)) in
            expected_items.iter().zip(&actual.items).enumerate()
        {
            compare_item(
                id,
                index,
                expected_item,
                actual_item,
                case.pos.line,
                mismatches,
            );
        }
    }

    if let Some(expected_count) = expected.count {
        if expected_count != actual.items.len() {
            push_mismatch(
                mismatches,
                id,
                "items.length",
                expected_count,
                actual.items.len(),
            );
        }
        if let Some(spots) = &expected.items_spot {
            for spot in spots {
                if let Some(actual_item) = actual.items.get(spot.index) {
                    compare_item(
                        id,
                        spot.index,
                        &spot.item,
                        actual_item,
                        case.pos.line,
                        mismatches,
                    );
                } else {
                    push_mismatch(
                        mismatches,
                        id,
                        &format!("item[{}]", spot.index),
                        &spot.item,
                        "<missing>",
                    );
                }
            }
        }
    }
}

fn execute_case(file: &str, case: &Case) -> Vec<String> {
    let id = format!("{file}[{}]", case.name);
    let mut mismatches = Vec::new();

    let trigger = case.trigger.as_ref().and_then(|value| value.chars().next());
    let pos = CorePos {
        line: case.pos.line,
        character: case.pos.character,
    };
    let settings = core_settings(&case.settings);
    let actual_plan = completion_plan(&case.doc, pos, trigger, &settings);

    let Some(expected) = &case.expect else {
        push_mismatch(
            &mut mismatches,
            &id,
            "expect",
            "non-null for an executed fixture",
            "null",
        );
        return mismatches;
    };

    match (&expected.plan, &actual_plan) {
        (Some(Plan::Done), CompletionPlan::NeedsData(request)) => push_mismatch(
            &mut mismatches,
            &id,
            "plan",
            "done",
            format!("needs {}", request_kind(request)),
        ),
        (Some(Plan::Needs(expected_kind)), CompletionPlan::Done(_)) => push_mismatch(
            &mut mismatches,
            &id,
            "plan",
            format!("needs {expected_kind}"),
            "done",
        ),
        (Some(Plan::Needs(expected_kind)), CompletionPlan::NeedsData(request)) => {
            let actual_kind = request_kind(request);
            if expected_kind != actual_kind {
                push_mismatch(
                    &mut mismatches,
                    &id,
                    "plan.needs",
                    expected_kind,
                    actual_kind,
                );
            }
        }
        _ => {}
    }

    match (&actual_plan, &case.data) {
        (CompletionPlan::Done(list), _) => {
            compare_list(&id, case, expected, list, &mut mismatches);
        }
        (CompletionPlan::NeedsData(_), Some(data)) => {
            let list = completion_resolve(&case.doc, pos, trigger, &settings, core_data(data));
            compare_list(&id, case, expected, &list, &mut mismatches);
        }
        (CompletionPlan::NeedsData(_), None)
            if expected.is_incomplete.is_some()
                || expected.items.is_some()
                || expected.count.is_some()
                || expected.items_spot.is_some() =>
        {
            push_mismatch(
                &mut mismatches,
                &id,
                "completion_list",
                "available",
                "unavailable because the fixture supplies no resolve data",
            );
        }
        _ => {}
    }

    if let Some(named) = &expected.named_body {
        let mut root_chars = named.root.chars();
        let root = root_chars.next();
        let accidental = match root_chars.as_str() {
            "" => Some(None),
            "+" => Some(Some(RootAccidental::Sharp)),
            "-" => Some(Some(RootAccidental::Flat)),
            "=" => Some(Some(RootAccidental::Natural)),
            _ => None,
        };
        let defs = match named.size {
            3 => Some(CHORDS_3),
            4 => Some(CHORDS_4),
            _ => None,
        };
        let actual = root
            .zip(accidental)
            .zip(defs)
            .and_then(|((root, accidental), defs)| {
                defs.iter()
                    .find(|def| def.suffix == named.suffix)
                    .and_then(|def| {
                        let key_sig = match named.key_sig.as_str() {
                            "none" => KeySig::new(),
                            _ => return None,
                        };
                        render_chord(root, accidental, def, &key_sig)
                    })
            });
        if actual.as_deref() != Some(named.body.as_str()) {
            push_mismatch(
                &mut mismatches,
                &id,
                "named_body.body",
                &named.body,
                actual.as_deref().unwrap_or("<unrenderable>"),
            );
        }
    }

    mismatches
}

#[test]
fn completion_fixture_suite_is_well_formed() {
    let files = match load_all() {
        Ok(files) => files,
        Err(e) => panic!("fixture load failed: {e}"),
    };
    assert!(!files.is_empty(), "no fixture files found");

    let mut errors: Vec<String> = Vec::new();
    let mut names: BTreeMap<String, String> = BTreeMap::new();
    let mut open: Vec<(String, String, String)> = Vec::new();
    let mut decisions_seen: BTreeSet<String> = BTreeSet::new();
    let mut total = 0usize;

    let mut summary = String::new();
    let _ = writeln!(summary, "\ncompletion fixture suite:");

    for (file, cases) in &files {
        let _ = writeln!(summary, "  {file}: {} cases", cases.len());
        for case in cases {
            total += 1;
            if let Some(previous) = names.insert(case.name.clone(), file.clone()) {
                errors.push(format!(
                    "duplicate case name `{}` in {file} (also in {previous})",
                    case.name
                ));
            }
            for d in &case.decisions {
                decisions_seen.insert(d.clone());
            }
            if case.expect.is_none() {
                open.push((file.clone(), case.name.clone(), case.notes.clone()));
            }
            if let Err(e) = validate_case(file, case) {
                errors.push(e);
            }
        }
    }

    let _ = writeln!(summary, "  total: {total} cases, {} OPEN", open.len());
    let mut covered: Vec<&str> = decisions_seen.iter().map(|s| s.as_str()).collect();
    covered.sort_by_key(|d| d[1..].parse::<u32>().unwrap_or(0));
    let _ = writeln!(summary, "  decisions referenced: {}", covered.join(" "));
    if !open.is_empty() {
        let _ = writeln!(summary, "  OPEN cases (no canon chosen yet):");
        for (file, name, notes) in &open {
            let _ = writeln!(summary, "    - {file}[{name}]: {notes}");
        }
    }
    println!("{summary}");

    assert_eq!(total, 78, "fixture suite case count changed");
    assert!(
        errors.is_empty(),
        "fixture schema/invariant violations:\n  - {}",
        errors.join("\n  - ")
    );
}

#[test]
fn all_completion_fixtures_execute() {
    let files = load_all().unwrap_or_else(|e| panic!("fixture load failed: {e}"));
    let known: BTreeSet<&str> = KNOWN_DISCREPANCIES.iter().copied().collect();
    assert_eq!(
        known.len(),
        KNOWN_DISCREPANCIES.len(),
        "KNOWN_DISCREPANCIES contains duplicate case names"
    );

    let mut errors = Vec::new();
    let mut quarantined_seen = BTreeSet::new();
    let mut total = 0usize;
    let mut summary = String::from("\nexecuted completion fixtures:\n");

    for (file, cases) in &files {
        if !EXECUTED_FILES.contains(&file.as_str()) {
            continue;
        }

        let mut passed = 0usize;
        let mut quarantined = 0usize;
        for case in cases {
            total += 1;
            let mismatches = execute_case(file, case);
            if known.contains(case.name.as_str()) {
                quarantined_seen.insert(case.name.as_str());
                if mismatches.is_empty() {
                    errors.push(format!(
                        "{file}[{}]: KNOWN DISCREPANCY unexpectedly passed; remove it from the quarantine",
                        case.name
                    ));
                } else {
                    quarantined += 1;
                    let _ = writeln!(
                        summary,
                        "  QUARANTINED {file}[{}]:\n    {}",
                        case.name,
                        mismatches.join("\n    ")
                    );
                }
            } else if mismatches.is_empty() {
                passed += 1;
            } else {
                errors.extend(mismatches);
            }
        }
        let _ = writeln!(
            summary,
            "  {file}: {passed}/{} passed, {quarantined} quarantined",
            cases.len()
        );
    }

    assert_eq!(total, 78, "executed completion fixture case count changed");
    for name in known.difference(&quarantined_seen) {
        errors.push(format!(
            "KNOWN_DISCREPANCIES entry `{name}` does not name an executed case"
        ));
    }
    let _ = writeln!(summary, "  total: {total} executed");
    println!("{summary}");

    assert!(
        errors.is_empty(),
        "completion fixture mismatches (all mismatches per case):\n  - {}",
        errors.join("\n  - ")
    );
}

#[test]
fn completion_fixture_pending_list_is_empty() {
    let files = load_all().unwrap_or_else(|e| panic!("fixture load failed: {e}"));
    let actual_files: BTreeSet<&str> = files.iter().map(|(file, _)| file.as_str()).collect();
    let classified_files: BTreeSet<&str> = EXECUTED_FILES
        .iter()
        .copied()
        .chain(PENDING_FILES.iter().map(|(file, _)| *file))
        .collect();
    assert_eq!(
        actual_files, classified_files,
        "every fixture file must be explicitly executed or pending"
    );

    let mut by_phase: BTreeMap<&str, Vec<String>> = BTreeMap::new();
    let mut total = 0usize;
    for (file, phase) in PENDING_FILES {
        let cases = files
            .iter()
            .find(|(candidate, _)| candidate == file)
            .unwrap_or_else(|| panic!("pending fixture `{file}` is missing"))
            .1
            .len();
        total += cases;
        by_phase
            .entry(phase)
            .or_default()
            .push(format!("{file} ({cases})"));
    }
    assert_eq!(total, 0, "completion fixtures remain pending");
    assert!(
        PENDING_FILES.is_empty(),
        "PENDING_FILES must stay empty after C3"
    );

    let mut summary = String::from("\npending completion fixtures (schema-validated):\n");
    for (phase, entries) in by_phase {
        let _ = writeln!(summary, "  pending {phase}: {}", entries.join(", "));
    }
    println!("{summary}");
}
