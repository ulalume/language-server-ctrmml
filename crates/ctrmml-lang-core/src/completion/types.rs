use serde::{Deserialize, Serialize};

/// Zero-based document position. `character` is measured in UTF-16 code units.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pos {
    /// Zero-based line number.
    pub line: u32,
    /// Zero-based UTF-16 code-unit offset within the line.
    pub character: u32,
}

/// A completion response independent of LSP or Monaco types.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CoreCompletionList {
    /// Completion items in provider-defined display order.
    pub items: Vec<CoreItem>,
    /// Whether the editor should re-query as the user continues typing.
    pub is_incomplete: bool,
}

impl CoreCompletionList {
    pub(crate) fn empty() -> Self {
        Self::default()
    }
}

/// Editor-neutral completion item.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CoreItem {
    /// Primary text displayed for the item.
    pub label: String,
    /// Optional suffix displayed alongside the primary label.
    pub label_description: Option<String>,
    /// Semantic category used by editors for icons and grouping.
    pub kind: CoreItemKind,
    /// Short explanatory text displayed beside the label.
    pub detail: Option<String>,
    /// Optional Markdown documentation for the item.
    pub documentation: Option<String>,
    /// Text and insertion behavior applied when the item is accepted.
    pub insert: InsertSpec,
    /// Text editors use when filtering the item.
    pub filter_text: Option<String>,
    /// Stable lexical key used to preserve provider ordering.
    pub sort_text: Option<String>,
    /// Whether the editor should initially select this item.
    pub preselect: bool,
    /// Explicit document range replaced by the inserted text.
    pub edit_range: EditRange,
    /// Additional edits applied with the primary insertion.
    pub additional_edits: Vec<CoreTextEdit>,
    /// Optional editor action to invoke after insertion.
    pub command: Option<CoreCommand>,
}

/// Editor-neutral completion item categories.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CoreItemKind {
    /// Callable command or function.
    Function,
    /// Language keyword.
    Keyword,
    /// Literal or enumerated value.
    Value,
    /// Named property or modifier.
    Property,
    /// Instrument or other type-like parameter.
    TypeParameter,
    /// Structured declaration.
    Struct,
    /// File or path.
    File,
    /// Reusable snippet template.
    Snippet,
    /// Unclassified text.
    Text,
}

/// Text and editor insertion options for a completion item.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InsertSpec {
    /// Plain text or snippet source inserted into the document.
    pub text: String,
    /// Whether `text` is plain text or snippet syntax.
    pub format: InsertFormat,
    /// Maps to LSP `InsertTextMode::AS_IS`. The C3 FM provider sets this;
    /// every C1 provider leaves it `false`.
    pub as_is: bool,
}

/// Syntax interpretation for inserted completion text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum InsertFormat {
    /// Insert text without interpreting snippet placeholders.
    #[serde(rename = "plain")]
    PlainText,
    /// Interpret insert text as editor snippet syntax.
    #[serde(rename = "snippet")]
    Snippet,
}

/// Explicit edit range; both positions use UTF-16 character offsets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct EditRange {
    /// Inclusive start position of the replaced range.
    pub start: Pos,
    /// Exclusive end position of the replaced range.
    pub end: Pos,
}

impl EditRange {
    /// Creates an edit range from its inclusive start and exclusive end.
    pub const fn new(start: Pos, end: Pos) -> Self {
        Self { start, end }
    }
}

/// An editor-neutral text edit applied alongside a completion.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CoreTextEdit {
    /// Document range replaced by this edit.
    pub range: EditRange,
    /// Replacement text.
    pub new_text: String,
}

/// Editor actions supported after accepting a completion item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CoreCommand {
    /// Ask the editor to open completion suggestions again.
    TriggerSuggest,
}

/// User-configurable completion behavior.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CompletionSettings {
    /// Whether arpeggio completions are enabled.
    pub arpeggio_enabled: bool,
    /// Pattern used to render arpeggio completions.
    pub arpeggio_pattern: ArpeggioPattern,
    /// Direction strategy used to render stacked chords.
    pub chord_stack_mode: ChordStackMode,
    /// Whether FM patches use the two-step file-to-patch picker.
    pub fm_picker_hierarchy: bool,
}

impl Default for CompletionSettings {
    fn default() -> Self {
        Self {
            arpeggio_enabled: false,
            arpeggio_pattern: ArpeggioPattern::Up,
            chord_stack_mode: ChordStackMode::StackUp,
            fm_picker_hierarchy: false,
        }
    }
}

/// Available arpeggio traversal patterns.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ArpeggioPattern {
    /// Ascend through chord tones.
    Up,
    /// Descend through chord tones.
    Down,
    /// Ascend and then descend.
    UpDown,
    /// Descend and then ascend.
    DownUp,
    /// Alternate low, high, middle, high tones.
    Alberti,
}

/// Rendering modes for chord completions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChordStackMode {
    /// Adjust octaves so chord tones rise monotonically.
    StackUp,
    /// Render chord letters without octave stacking.
    Plain,
}

/// Host data requested by a completion provider.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DataRequest {
    /// Request PCM paths reachable from the current document.
    PcmPaths,
    /// Request PCM files usable in instrument definitions.
    PcmFiles,
    /// Request converted FM patches, optionally beneath a path fragment.
    FmPatches {
        /// Typed relative-path fragment used by the hierarchy picker.
        fragment: Option<String>,
    },
    /// Request the compiled tick and PPQN at the cursor.
    CursorTick,
}

/// Host-supplied data used to resolve a completion request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DataPayload {
    /// Relative PCM paths reachable from the current document.
    PcmPaths(Vec<String>),
    /// Relative PCM files usable in instrument definitions.
    PcmFiles(Vec<String>),
    /// Converted FM patches; an empty vector means conversion was unavailable.
    FmPatches(Vec<FmPatchData>),
    /// Compiled cursor timing, or `None` when compilation failed.
    CursorTick(Option<CursorTickData>),
}

/// One converted FM patch supplied by a host adapter.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FmPatchData {
    /// Slash-separated path relative to a configured instrument root.
    pub rel_path: String,
    /// Patch name within a multi-patch file, when present.
    pub name: Option<String>,
    /// Converted `@N fm` body.
    pub mml: String,
    /// Whether the converted body contains macros.
    pub has_macros: bool,
}

/// Compiled timing information at the completion cursor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CursorTickData {
    /// Absolute tick at the cursor.
    pub tick: u32,
    /// Pulses per quarter note used by the compiled document.
    pub ppqn: u32,
}

/// First-stage completion result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CompletionPlan {
    /// Completion finished without host data.
    #[serde(rename = "done")]
    Done(CoreCompletionList),
    /// Completion requires the host to supply the enclosed data kind.
    #[serde(rename = "needs")]
    NeedsData(DataRequest),
}

/// Provider flow control. An exclusive empty result still terminates the cascade.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ProviderOutcome {
    NotApplicable,
    Exclusive(CoreCompletionList),
}
