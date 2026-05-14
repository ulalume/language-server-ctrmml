//! MML / platform-command documentation shared by the LSP binary and
//! the web editor (via `ctrmml-lang-wasm::docs_json`).

use serde::Serialize;

#[derive(Serialize, Clone, Copy)]
pub struct DocEntry {
    /// Lookup key. For commands this is the `filterText`-equivalent
    /// (`"o"`, `"V"`, `"_{"`, `"notes"`); for keyword/value entries
    /// it's the literal token (`"#title"`, `"megadrive"`).
    pub key: &'static str,
    /// Display label shown in completion lists.
    pub label: &'static str,
    /// Snippet body to insert. Empty for label-only entries. A `$` in
    /// the body marks it as a snippet on the editor side.
    pub insert: &'static str,
    /// One-line summary (corresponds to web's `detail`).
    pub detail: &'static str,
    /// Long-form documentation (Markdown).
    pub doc: &'static str,
}

pub type CommandCompletion = DocEntry;
pub type PlatformCommand = DocEntry;
pub type FmParamDoc = DocEntry;
pub type TwoOpParamDoc = DocEntry;

const fn entry(
    key: &'static str,
    label: &'static str,
    insert: &'static str,
    detail: &'static str,
    doc: &'static str,
) -> DocEntry {
    DocEntry {
        key,
        label,
        insert,
        detail,
        doc,
    }
}

// ---------------------------------------------------------------------------
// Meta keywords (#title, #platform, ...)
// ---------------------------------------------------------------------------

pub const META_KEYWORDS: &[DocEntry] = &[
    entry(
        "#title",
        "#title",
        "#title ",
        "Song title.",
        "Set the song title. Embedded in the compiled VGM/ROM header and shown in the license editor.\n\nExample:\n  #title Overture",
    ),
    entry(
        "#composer",
        "#composer",
        "#composer ",
        "Original composer.",
        "Set the original composer name. For original works this is the author of both the music and the MML program. For transcriptions/arrangements, this is the original music composer (not the MML programmer).\n\nExample:\n  #composer J.S. Bach",
    ),
    entry(
        "#programmer",
        "#programmer",
        "#programmer ",
        "MML programmer.",
        "Set the name of the person who wrote the MML program. Use this when the MML programmer differs from the original composer (e.g. transcriptions and arrangements). When #programmer is present, the license editor treats the work as a transcription.\n\nExample:\n  #programmer yourname",
    ),
    entry("#author", "#author", "#author ", "Song metadata.", ""),
    entry("#date", "#date", "#date ", "Song metadata.", ""),
    entry("#comment", "#comment", "#comment ", "Song metadata.", ""),
    entry(
        "#platform",
        "#platform",
        "#platform ",
        "Choose the Mega Drive playback mode.",
        "Choose the Mega Drive playback mode.\n\n`megadrive` uses raw VGM DAC stream playback, keeps VGM smaller, and does not support PCM mixing or PCM volume.\n\n`mdsdrv` simulates the MDSDRV PCM driver, adding PCM mixing and PCM volume, but using coarse PCM pitch steps.",
    ),
    entry(
        "#option",
        "#option",
        "#option ",
        "Sets platform options.",
        "Set platform compatibility options. `noextpitch` disables extended pitch envelopes for older MML files.",
    ),
    entry(
        "#group",
        "#group",
        "#group ",
        "MDSDRV song group.",
        "Set the MDSDRV/ROM song group. Use `bgm` for music and `se` for sound effects.",
    ),
    entry("#game", "#game", "#game ", "Song metadata.", ""),
    entry("#composerj", "#composerj", "#composerj ", "Song metadata.", ""),
    entry(
        "#license",
        "#license",
        "#license ",
        "License information.",
        "Specify the license for this work.\n\nExamples:\n  #license CC BY-SA\n  #license CC BY-NC-SA (program only)\n  #license All Rights Reserved\n\nWhen the composer and programmer differ, add `(program only)` to indicate the license covers only the MML program.",
    ),
    entry(
        "#timesig",
        "#timesig",
        "#timesig ",
        "Time signature for piano roll.",
        "Set the time signature for piano-roll measure and beat lines.\nUse `no` to hide measure lines and disable bar-line fill.\nInvalid values fall back to 4/4.\n\nExamples:\n  #timesig 3/4\n  #timesig 6/8\n  #timesig no",
    ),
];

// Editor-overlay hints: which labels should re-trigger the completion
// popup after being inserted (e.g. `#platform ` opens the value picker)
// or be suggested as instrument-definition completions.
pub const META_KEYWORDS_TRIGGER_SUGGEST: &[&str] = &["#platform", "#option", "#group", "#timesig"];
// Instrument keywords that should re-trigger completion after insertion
// so the user immediately gets to pick an instrument file (or fall back
// to the default template). `2op` is intentionally absent — there's no
// per-file pool to choose from, so its snippet template fills in
// directly.
pub const INSTRUMENT_TYPES_TRIGGER_SUGGEST: &[&str] = &["fm", "pcm"];
pub const AT_META_COMPLETION_LABELS: &[&str] = &["@<num>", "@M<num>"];

// ---------------------------------------------------------------------------
// Meta values
// ---------------------------------------------------------------------------

pub const PLATFORM_VALUES: &[DocEntry] = &[
    entry(
        "megadrive",
        "megadrive",
        "megadrive",
        "Mega Drive / Genesis",
        "Use raw VGM DAC stream playback for PCM. VGM stays smaller and works well for VGM/XGM-style workflows, but PCM mixing and PCM volume control are not available here.",
    ),
    entry(
        "mdsdrv",
        "mdsdrv",
        "mdsdrv",
        "MDSDRV sound driver",
        "Simulate the MDSDRV PCM driver. PCM software mixing and PCM volume control are available, but PCM pitch is quantized to coarse driver steps.",
    ),
];

pub const OPTION_VALUES: &[DocEntry] = &[
    entry(
        "noextpitch",
        "noextpitch",
        "noextpitch",
        "Disable extended pitch envelopes",
        "Disable extended pitch envelopes for compatibility with older MML files.",
    ),
];

pub const TIMESIG_VALUES: &[DocEntry] = &[
    entry(
        "3/4",
        "3/4",
        "3/4",
        "Three beats per measure.",
        "Show piano-roll measure lines in 3/4.",
    ),
    entry(
        "4/4",
        "4/4",
        "4/4",
        "Four beats per measure (default).",
        "Show piano-roll measure lines in 4/4.",
    ),
    entry(
        "5/4",
        "5/4",
        "5/4",
        "Five beats per measure.",
        "Show piano-roll measure lines in 5/4.",
    ),
    entry(
        "6/8",
        "6/8",
        "6/8",
        "Six eighth-note beats per measure.",
        "Show piano-roll measure lines in 6/8.",
    ),
    entry(
        "no",
        "no",
        "no",
        "No measure lines.",
        "Hide piano-roll measure lines (beat grid remains). Also disables the `|` bar-line rest fill.",
    ),
];

pub const GROUP_VALUES: &[DocEntry] = &[
    entry(
        "bgm",
        "bgm",
        "bgm",
        "Background music group.",
        "Marks this song as BGM for MDSDRV/ROM grouping.",
    ),
    entry(
        "se",
        "se",
        "se",
        "Sound effect group.",
        "Marks this song as SE for MDSDRV/ROM grouping.",
    ),
];

// ---------------------------------------------------------------------------
// Instrument types & PCM modifiers
// ---------------------------------------------------------------------------

pub const INSTRUMENT_TYPES: &[DocEntry] = &[
    entry(
        "pcm",
        "pcm",
        "pcm ",
        "PCM sample instrument",
        "PCM samples are defined as instruments. The first parameter is the path to the sample, relative to the MML file. The sample rate from the WAV file is used. If the WAV file has more than one channel, only the first (left) channel is used.\n\n    @30 pcm \"path/to/sample.wav\"\n\nPCM samples can be played on channels `F`, `K`, and `L`. PCM uses the panning setting from FM channel 6 (`F`), and FM output on that channel is muted while PCM is playing.",
    ),
    entry(
        "fm",
        "fm",
        // Accepting `fm` inserts just the keyword + space and re-triggers
        // completion (see `INSTRUMENT_TYPES_TRIGGER_SUGGEST`). The user
        // then picks an instrument file from the workspace, or falls
        // back to the default-template entry the FM completer appends.
        "fm ",
        "FM synthesis instrument",
        "FM instruments are defined with ALG (algorithm), FB (feedback), and four operators (OP1-OP4). Each operator has: AR (attack rate), DR (decay rate), SR (sustain rate), RR (release rate), SL (sustain level), TL (total level), KS (key scale), ML (multiplier), DT (detune), and SSG (SSG-EG).\n\n    @1 fm\n    ; ALG FB\n        3   0\n    ;  AR  DR  SR  RR  SL  TL  KS  ML  DT SSG\n       31   0  19   5   0  23   0   0   0   0 ; OP1\n       31   6   0   4   3  19   0   0   0   0 ; OP2\n       31  15   0   5   4  38   0   4   0   0 ; OP3\n       31  27   0  11   1   0   0   1   0   0 ; OP4\n\nTo enable AM for an operator, add 100 to the SSG-EG value.",
    ),
    entry(
        "psg",
        "psg",
        "psg ",
        "PSG envelope instrument",
        "PSG instruments (envelopes) are defined as a sequence of values. 15 is the maximum volume and 0 is silence.\n\n    @10 psg 15>10\n\nUse `>` to slide from one value to another (no space around `>`). Use `:` to set the length of each value in frames. Use `/` to set the sustain position or `|` to set the loop position.\n\n    @13 psg 15 14 / 13>0:7\n    @14 psg 0>14:7 | 15 10 5 0 5 10\n\nNote: there must be no space between values and the `>` or `:` commands.",
    ),
    entry(
        "2op",
        "2op",
        "2op   ${1:2}   ${2:5}   ${3:5}   ${4:4}   ${5:4}   ${6:0}",
        "2-operator FM instrument",
        "Create a derived FM instrument from an existing FM patch by changing OP1-OP4 multiply ratios (ML) and adding a transpose. The result is still a normal FM instrument, so the audible operator pairs depend on the base patch's algorithm and levels.\n\nUse this with base patches designed as two 2-operator stacks.\n\n    ;         @ ML1 ML2 ML3 ML4 TRS\n    @24 2op   2   5   5   4   4   0 ; n+4\n    @25 2op   2   4   4   3   3   5 ; n+5",
    ),
];

pub const AT_META: &[DocEntry] = &[
    entry(
        "@<num>",
        "@<num>",
        "${1:num}",
        "Instrument definition",
        "Define an instrument table. Use this for FM, PSG, PCM, or 2op instruments. The body depends on the selected instrument type.",
    ),
    entry(
        "@M<num>",
        "@M<num>",
        "M${1:num}",
        "Pitch envelope",
        "Define a pitch envelope table. Use `M<num>` in a track to apply it; `M0` disables it.",
    ),
];

pub const RATE_OFFSET: &[DocEntry] = &[
    entry(
        "rate=",
        "rate=<num>",
        "rate=${1:<num>}",
        "Override the sample rate.",
        "Override the sample rate of a PCM instrument. By default the sample rate specified in the WAV file is used. In `mdsdrv` mode, the closest possible sample rate in ~2.2 kHz steps will be selected.\n\n    @30 pcm \"path/to/sample.wav\" rate=8000",
    ),
    entry(
        "offset=",
        "offset=<num>",
        "offset=${1:<num>}",
        "Adjust the start position.",
        "Adjust the start position of a PCM instrument, skipping the specified number of samples from the beginning.\n\n    @30 pcm \"path/to/sample.wav\" offset=4000",
    ),
];

// ---------------------------------------------------------------------------
// Track commands
// ---------------------------------------------------------------------------

pub const COMMAND_COMPLETIONS: &[DocEntry] = &[
    entry(
        "notes",
        "cdefgabh",
        "c",
        "Insert notes.",
        "Insert notes. You can optionally add a duration after each note; otherwise the current `l` value is used.\n\nUse `+`, `-`, or `=` after the note letter to add a sharp, flat, or natural. In normal mode, `h` is the same pitch as `b` (B natural).\n\nExamples: `c4`, `f+8`, `b-16`, `h`, `c:12`.\n\nIn drum mode, `a`..`h` no longer mean pitches. They become 0..7 and call drum macro tracks from the current `D` base index.\n\nA trailing `.` after a duration is a dotted note: it multiplies the duration by 1.5. Multiple dots stack (e.g. `c4..` is `c4` × 1.75).",
    ),
    entry(
        "r",
        "r<duration>",
        "r${1:duration}",
        "Insert a rest.",
        "Insert a rest. You can optionally add a duration after `r`; otherwise the current `l` value is used.\n\nExamples: `r4`, `r8.`, `r:12`.\n\nLike notes, numeric durations are based on the current whole-note length set by `C`.",
    ),
    entry(
        "o",
        "o<1..8>",
        "o${1:1..8}",
        "Set octave.",
        "Set octave. In normal melodic tracks, notes below `o1 c` or above `o8 a` may fail ROM export.",
    ),
    entry(
        "l",
        "l<duration>",
        "l${1:duration}",
        "Set default duration.",
        "Set default duration, used if not specified by notes, rests, `R` or `~` commands.",
    ),
    entry(
        "Q",
        "Q<1..8>",
        "Q${1:1..8}",
        "Quantize.",
        "Quantize. Used to set articulation. Note length is `param / 8`. For example, `Q4` makes notes half length, and `Q8` keeps full length.",
    ),
    entry(
        "q",
        "q<1..8>",
        "q${1:1..8}",
        "Set early release.",
        "Set early release. This makes notes release before their full written length, which can make phrases feel more detached or percussive.\n\nUnlike `Q`, which keeps only a fraction of the note, `q` subtracts a small amount from the end of each note. Higher values release earlier. If the early release would be too large, it is clamped so the note still has at least 1 tick of sounding time.\n\nA simple way to think about it: `Q4` makes a note play for about half its length, while `q4` keeps the note mostly full length but lets go a little early.",
    ),
    entry(
        "s",
        "s<ticks>",
        "s${1:ticks}",
        "Set shuffle.",
        "Set shuffle. The specified number of ticks will be added to the the next note, rest or tie, then subtracted from the next.",
    ),
    entry(
        "C",
        "C<ticks>",
        "C${1:ticks}",
        "Set whole-note length in ticks.",
        "Set the whole-note length in ticks. The default is 96.\n\nThis changes how numeric durations are interpreted: `c4`, `l8`, `r16` and similar values are calculated from this base. Example: with `C96`, `c4` = 24 ticks. With `C192`, `c4` = 48 ticks.\n\nUse this only when you need a non-standard timing base. `:ticks` durations are not affected. BPM calculation currently assumes `C96`, so other values may make tempo readouts inaccurate.",
    ),
    entry(
        "R",
        "R<duration>",
        "R${1:duration}",
        "Reverse rest.",
        "Reverse rest. This subtracts the value from the previous note or rest.",
    ),
    entry(
        "t",
        "t<bpm>",
        "t${1:bpm}",
        "Set tempo (BPM).",
        "Set tempo in BPM. This is global to all channels.",
    ),
    entry(
        "T",
        "T<value>",
        "T${1:value}",
        "Set tempo (native).",
        "Set tempo using the platform's native timer values. This is global to all channels.",
    ),
    entry(
        "v",
        "v<0..15>",
        "v${1:0..15}",
        "Set volume.",
        "Set volume.",
    ),
    entry(
        "V",
        "V<0..127>",
        "V${1:0..127}",
        "Set volume (fine).",
        "Set volume (fine), or modify volume (fine) depending on parameter range.\n\nNote: `0` is maximum volume; higher values attenuate (the parameter maps to the YM2612 TL register convention).",
    ),
    entry(
        "V",
        "V<-128..+127>",
        "V${1:-128..+127}",
        "Modify volume (fine).",
        "Set volume (fine), or modify volume (fine) depending on parameter range.\n\nNote: `0` is maximum volume; higher values attenuate (the parameter maps to the YM2612 TL register convention).",
    ),
    entry(
        "p",
        "p<-128..127>",
        "p${1:-128..+127}",
        "Set panning.",
        "Set panning.\n\nPanning using the `p` command is only allowed for FM channels and the accepted range is 0-3. Bit 1 enables the right channel, bit 2 enables the left channel.",
    ),
    entry(
        "k",
        "k<-128..127>",
        "k${1:-128..+127}",
        "Set transpose.",
        "Set transpose. Default behavior is the same as the `_` command.",
    ),
    entry(
        "K",
        "K<-128..127>",
        "K${1:-128..+127}",
        "Set detune.",
        "Set detune.",
    ),
    entry(
        "M",
        "M<0..255>",
        "M${1:0..255}",
        "Set pitch envelope.",
        "Set the pitch envelope number for this track. Use a table defined with `@M<num>`. `M0` disables the pitch envelope.",
    ),
    entry(
        "G",
        "G<0..255>",
        "G${1:0..255}",
        "Set portamento.",
        "Set the portamento table for this track. Use a table defined with `@<num>` for portamento data. `G0` disables portamento.",
    ),
    entry(
        "D",
        "D<0..255>",
        "D${1:0..255}",
        "Set drum mode.",
        "Enable drum mode and set its base index. `D0` disables drum mode. In drum mode, notes `a`..`h` become 0..7 and call macro tracks starting at the base index. The called macro runs up to its first note, and that note uses the duration from the calling track.",
    ),
    entry(
        "L",
        "L",
        "L",
        "Loop point.",
        "Set loop point (segno). If this is present, playback resumes at this point when the end of the track is reached. This is set per channel / track.",
    ),
    entry(
        "(",
        "(",
        "(",
        "Volume down.",
        "Volume down. Decrease coarse volume by 1, or by the following number if provided.",
    ),
    entry(
        ")",
        ")",
        ")",
        "Volume up.",
        "Volume up. Increase coarse volume by 1, or by the following number if provided.",
    ),
    entry(
        "^",
        "^",
        "^",
        "Tie.",
        "Tie. Extends duration of previous note.",
    ),
    entry(
        "&",
        "&",
        "&",
        "Slur.",
        "Slur. Used to connect two notes (legato).",
    ),
    entry(
        "\\=",
        "\\=<delay>,<volume>",
        "\\=${1:delay},${2:volume}",
        "Echo macro setup.",
        "Set echo macro parameters. The first value is how many notes or rests to backtrack, and the second is the volume reduction for the echoed note.",
    ),
    entry(
        "\\",
        "\\<duration>",
        "\\${1:duration}",
        "Echo note.",
        "Insert an echo note using the parameters previously set by `\\=`. The duration follows the same rules as other note lengths.",
    ),
    entry(
        "_",
        "_<-128..127>",
        "_${1:-128..+127}",
        "Set transpose.",
        "Set transpose.",
    ),
    entry(
        "__",
        "__<-128..127>",
        "__${1:-128..+127}",
        "Set relative transpose.",
        "Set relative transpose.",
    ),
    entry(
        "_{",
        "_{<key signature>}",
        "_{${1:key signature}}",
        "Set key signature.",
        "Set key signature. The default is C major / A minor with no sharps or flats.\n\nExamples: `_{D}` sets D major, `_{c}` sets C minor, `_{+cfg}` sharpens C/F/G without clearing the current signature, `_{-b}` flats B, and `_{=f}` makes F natural while keeping the rest of the current signature.",
    ),
    entry(
        ">",
        ">",
        ">",
        "Octave up.",
        "Increase the current octave by 1.",
    ),
    entry(
        "<",
        "<",
        "<",
        "Octave down.",
        "Decrease the current octave by 1.",
    ),
    entry(
        "[",
        "[ ]",
        "[ ]",
        "Repeat block.",
        "Repeat block. `[` starts the block and `]` ends it. Add `/` inside the block to mark the section skipped on the last repetition.",
    ),
    entry(
        "]",
        "[ ... ] end",
        "]",
        "Repeat bracket end.",
        "End of a loop block. The matching `[` starts the repeated section.",
    ),
    entry(
        "/",
        "/",
        "/",
        "Repeat break / conditional separator.",
        "Inside `[ ... ]`, marks the section skipped on the last repetition. Inside `{ ... }`, separates per-track alternatives.",
    ),
    entry(
        "{",
        "{ / }",
        "{",
        "Conditional block.",
        "Conditional block. `{` starts the block and `}` ends it. When multiple tracks are selected, each `/`-separated branch is assigned in channel order.",
    ),
    entry(
        "}",
        "{ ... / ... } end",
        "}",
        "Conditional block end.",
        "End of a conditional block. Use `/` inside the braces to separate the per-track branches.",
    ),
];

pub const HELP_ONLY_COMMANDS: &[DocEntry] = &[
    entry(
        "+",
        "+",
        "+",
        "Sharp accidental.",
        "Use `+` after a note letter to make that note sharp. This overrides the current key signature for that note.",
    ),
    entry(
        "-",
        "-",
        "-",
        "Flat accidental.",
        "Use `-` after a note letter to make that note flat. This overrides the current key signature for that note.",
    ),
    entry(
        "=",
        "=",
        "=",
        "Natural accidental.",
        "Use `=` after a note letter to force a natural. This overrides the current key signature for that note.",
    ),
];

// ---------------------------------------------------------------------------
// Platform-specific quoted commands
// ---------------------------------------------------------------------------

pub const PLATFORM_COMMANDS: &[DocEntry] = &[
    entry("_", "_<-128..127>", "_0", "Set transpose.", "Set transpose."),
    entry(
        "__",
        "__<-128..127>",
        "__0",
        "Set relative transpose.",
        "Set relative transpose.",
    ),
    entry(
        "_{",
        "_{<key signature>}",
        "_{c}",
        "Set key signature.",
        "Set key signature. The default is C major / A minor with no sharps or flats.\n\nExamples: `_{D}` sets D major, `_{c}` sets C minor, `_{+cfg}` sharpens C/F/G without clearing the current signature, `_{-b}` flats B, and `_{=f}` makes F natural while keeping the rest of the current signature.",
    ),
    entry(
        "fm3",
        "fm3 <mask>",
        "fm3 0000",
        "FM3 special mode.",
        "Mega Drive / MDSDRV command. Enable FM3 special mode for selected operators. Mask order is OP4 OP3 OP2 OP1, so 0001=OP1 and 0011=OP1+OP2. Use 1111 to disable special mode. Each bit assigns that operator to this track's independent pitch control. Usually combine C with helper tracks such as MNOP; I and L can also be used.",
    ),
    entry(
        "lfo",
        "lfo <0..3> <0..7>",
        "lfo 0 0",
        "Hardware LFO.",
        "Set hardware LFO AM sensitivity (first) and PM sensitivity (second). These sensitivity settings are per channel. The hardware LFO rate itself is shared globally.",
    ),
    entry(
        "lforate",
        "lforate <0..9>",
        "lforate 0",
        "Hardware LFO rate.",
        "Set hardware LFO rate. 0 disables; 1..9 increase speed (last two are much faster). This is global to all channels.",
    ),
    entry(
        "carry",
        "carry",
        "carry",
        "Macro carry mode.",
        "Mega Drive / MDSDRV command. Keep macro track position after new notes. Use at the start of a macro track.",
    ),
    entry(
        "mode",
        "mode <0..2>",
        "mode 0",
        "PSG noise mode.",
        "PSG noise mode for channel `J`.\n\n\
`mode 0` uses the chip's normal noise register. Notes do not behave like a normal scale here: only the low 3 bits are used, selecting one of eight noise settings.\n\n\
0 = periodic noise, fixed clock (divider 512)\n\
1 = periodic noise, fixed clock (divider 1024)\n\
2 = periodic noise, fixed clock (divider 2048)\n\
3 = periodic noise, source = tone 3\n\
4 = white noise, fixed clock (divider 512)\n\
5 = white noise, fixed clock (divider 1024)\n\
6 = white noise, fixed clock (divider 2048)\n\
7 = white noise, source = tone 3\n\n\
`mode 1` uses tone channel `I` as the frequency source for white noise, so `J` follows PSG3 pitch. Controlling both channels can conflict.\n\n\
`mode 2` uses tone channel `I` as the frequency source for periodic noise.",
    ),
    entry(
        "pcmmode",
        "pcmmode <2..3>",
        "pcmmode 2",
        "PCM playback mode.",
        "Mega Drive / MDSDRV command. Select PCM mixing mode: 2 = 2-channel PCM up to 17.5 kHz, 3 = 3-channel PCM up to about 13 kHz. In `mdsdrv` this matches the ROM driver. In `megadrive` it changes the simulator / VGM DAC playback mode.",
    ),
    entry(
        "pcmrate",
        "pcmrate <1..8>",
        "pcmrate 1",
        "PCM sample rate.",
        "Mega Drive / MDSDRV command. Change PCM pitch in temporary 1..8 steps until the next instrument change. In `mdsdrv`, these are the driver's coarse PCM pitch steps.",
    ),
    entry(
        "write",
        "write <register> <data>",
        "write 00 00",
        "Direct register write.",
        "Write YM2612 FM registers directly. Aliases: dtml*, ksar*, amdr*, sr*, slrr*, ssg*, fbal (use operator number for *). The effect is temporary until the next instrument change. In `megadrive` / VGM this becomes raw YM2612 register writes. XGM / XGM2 converters generally preserve the usual FM operator/channel register range used by these aliases, but arbitrary unrelated YM writes may not survive conversion.",
    ),
    entry(
        "fbal",
        "fbal <value>",
        "fbal 00",
        "Feedback / Algorithm.",
        "Override feedback and algorithm. Value = (FB << 3) | ALG. Temporary until next instrument change.",
    ),
    entry(
        "tl1",
        "tl1 <value>",
        "tl1 0",
        "TL operator 1.",
        "Set total level. Supports +/- for relative. OP1. Temporary until next instrument change.",
    ),
    entry(
        "tl2",
        "tl2 <value>",
        "tl2 0",
        "TL operator 2.",
        "Set total level. Supports +/- for relative. OP2. Temporary until next instrument change.",
    ),
    entry(
        "tl3",
        "tl3 <value>",
        "tl3 0",
        "TL operator 3.",
        "Set total level. Supports +/- for relative. OP3. Temporary until next instrument change.",
    ),
    entry(
        "tl4",
        "tl4 <value>",
        "tl4 0",
        "TL operator 4.",
        "Set total level. Supports +/- for relative. OP4. Temporary until next instrument change.",
    ),
    entry(
        "dtml1",
        "dtml1 <value>",
        "dtml1 00",
        "DT/MUL operator 1.",
        "Override detune and multiplier. Value = (DT << 4) | MUL. OP1. Temporary until next instrument change.",
    ),
    entry(
        "dtml2",
        "dtml2 <value>",
        "dtml2 00",
        "DT/MUL operator 2.",
        "Override detune and multiplier. Value = (DT << 4) | MUL. OP2. Temporary until next instrument change.",
    ),
    entry(
        "dtml3",
        "dtml3 <value>",
        "dtml3 00",
        "DT/MUL operator 3.",
        "Override detune and multiplier. Value = (DT << 4) | MUL. OP3. Temporary until next instrument change.",
    ),
    entry(
        "dtml4",
        "dtml4 <value>",
        "dtml4 00",
        "DT/MUL operator 4.",
        "Override detune and multiplier. Value = (DT << 4) | MUL. OP4. Temporary until next instrument change.",
    ),
    entry(
        "ksar1",
        "ksar1 <value>",
        "ksar1 31",
        "KS/AR operator 1.",
        "Override key scale and attack rate. Value = (KS << 6) | AR. OP1. Temporary until next instrument change.",
    ),
    entry(
        "ksar2",
        "ksar2 <value>",
        "ksar2 31",
        "KS/AR operator 2.",
        "Override key scale and attack rate. Value = (KS << 6) | AR. OP2. Temporary until next instrument change.",
    ),
    entry(
        "ksar3",
        "ksar3 <value>",
        "ksar3 31",
        "KS/AR operator 3.",
        "Override key scale and attack rate. Value = (KS << 6) | AR. OP3. Temporary until next instrument change.",
    ),
    entry(
        "ksar4",
        "ksar4 <value>",
        "ksar4 31",
        "KS/AR operator 4.",
        "Override key scale and attack rate. Value = (KS << 6) | AR. OP4. Temporary until next instrument change.",
    ),
    entry(
        "amdr1",
        "amdr1 <value>",
        "amdr1 00",
        "AM/DR operator 1.",
        "Override AM enable and decay rate. Value = (AM << 7) | DR. OP1. Temporary until next instrument change.",
    ),
    entry(
        "amdr2",
        "amdr2 <value>",
        "amdr2 00",
        "AM/DR operator 2.",
        "Override AM enable and decay rate. Value = (AM << 7) | DR. OP2. Temporary until next instrument change.",
    ),
    entry(
        "amdr3",
        "amdr3 <value>",
        "amdr3 00",
        "AM/DR operator 3.",
        "Override AM enable and decay rate. Value = (AM << 7) | DR. OP3. Temporary until next instrument change.",
    ),
    entry(
        "amdr4",
        "amdr4 <value>",
        "amdr4 00",
        "AM/DR operator 4.",
        "Override AM enable and decay rate. Value = (AM << 7) | DR. OP4. Temporary until next instrument change.",
    ),
    entry(
        "sr1",
        "sr1 <value>",
        "sr1 0",
        "Sustain rate operator 1.",
        "Override sustain rate (0..31). OP1. Temporary until next instrument change.",
    ),
    entry(
        "sr2",
        "sr2 <value>",
        "sr2 0",
        "Sustain rate operator 2.",
        "Override sustain rate (0..31). OP2. Temporary until next instrument change.",
    ),
    entry(
        "sr3",
        "sr3 <value>",
        "sr3 0",
        "Sustain rate operator 3.",
        "Override sustain rate (0..31). OP3. Temporary until next instrument change.",
    ),
    entry(
        "sr4",
        "sr4 <value>",
        "sr4 0",
        "Sustain rate operator 4.",
        "Override sustain rate (0..31). OP4. Temporary until next instrument change.",
    ),
    entry(
        "slrr1",
        "slrr1 <value>",
        "slrr1 00",
        "SL/RR operator 1.",
        "Override sustain level and release rate. Value = (SL << 4) | RR. OP1. Temporary until next instrument change.",
    ),
    entry(
        "slrr2",
        "slrr2 <value>",
        "slrr2 00",
        "SL/RR operator 2.",
        "Override sustain level and release rate. Value = (SL << 4) | RR. OP2. Temporary until next instrument change.",
    ),
    entry(
        "slrr3",
        "slrr3 <value>",
        "slrr3 00",
        "SL/RR operator 3.",
        "Override sustain level and release rate. Value = (SL << 4) | RR. OP3. Temporary until next instrument change.",
    ),
    entry(
        "slrr4",
        "slrr4 <value>",
        "slrr4 00",
        "SL/RR operator 4.",
        "Override sustain level and release rate. Value = (SL << 4) | RR. OP4. Temporary until next instrument change.",
    ),
    entry(
        "ssg1",
        "ssg1 <value>",
        "ssg1 0",
        "SSG-EG operator 1.",
        "Override SSG-EG mode (0..15). OP1. Temporary until next instrument change.",
    ),
    entry(
        "ssg2",
        "ssg2 <value>",
        "ssg2 0",
        "SSG-EG operator 2.",
        "Override SSG-EG mode (0..15). OP2. Temporary until next instrument change.",
    ),
    entry(
        "ssg3",
        "ssg3 <value>",
        "ssg3 0",
        "SSG-EG operator 3.",
        "Override SSG-EG mode (0..15). OP3. Temporary until next instrument change.",
    ),
    entry(
        "ssg4",
        "ssg4 <value>",
        "ssg4 0",
        "SSG-EG operator 4.",
        "Override SSG-EG mode (0..15). OP4. Temporary until next instrument change.",
    ),
];

// ---------------------------------------------------------------------------
// FM parameter / 2op parameter hover labels
// ---------------------------------------------------------------------------

pub const FM_PARAM_DOCS: &[DocEntry] = &[
    entry("ALG", "Algorithm (0..7)", "", "", ""),
    entry("FB", "Feedback (0..7)", "", "", ""),
    entry("AR", "Attack Rate (0..31)", "", "", ""),
    entry("DR", "Decay Rate (0..31)", "", "", ""),
    entry("SR", "Sustain Rate (0..31)", "", "", ""),
    entry("RR", "Release Rate (0..15)", "", "", ""),
    entry("SL", "Sustain Level (0..15)", "", "", ""),
    entry("TL", "Total Level (0..127)", "", "", ""),
    entry("KS", "Key Scale (0..3)", "", "", ""),
    entry(
        "ML",
        "Multiple (0..15)",
        "",
        "",
        "1 is normal, 2 is 2x, 3 is 3x, 0 is 0.5x.",
    ),
    entry("DT", "Detune (0..7)", "", "", ""),
    entry(
        "SSG",
        "SSG-EG",
        "",
        "",
        "SSG-EG type: 0..7, enabled: +8, AM enabled: +100.",
    ),
    entry("TRS", "Transpose (-24..24)", "", "", ""),
];

pub const TWO_OP_PARAM_DOCS: &[DocEntry] = &[
    entry(
        "",
        "FM Instrument Number",
        "",
        "",
        "FM instrument number to duplicate.",
    ),
    entry(
        "",
        "OP1 Multiple (0..15)",
        "",
        "",
        "1 is normal, 2 is 2x, 3 is 3x, 0 is 0.5x.",
    ),
    entry(
        "",
        "OP2 Multiple (0..15)",
        "",
        "",
        "1 is normal, 2 is 2x, 3 is 3x, 0 is 0.5x.",
    ),
    entry(
        "",
        "OP3 Multiple (0..15)",
        "",
        "",
        "1 is normal, 2 is 2x, 3 is 3x, 0 is 0.5x.",
    ),
    entry(
        "",
        "OP4 Multiple (0..15)",
        "",
        "",
        "1 is normal, 2 is 2x, 3 is 3x, 0 is 0.5x.",
    ),
    entry(
        "",
        "Transpose (-24..24)",
        "",
        "",
        "Transpose in semitones (-24..24).",
    ),
];

// ---------------------------------------------------------------------------
// Lookup helpers
// ---------------------------------------------------------------------------

fn nonempty(s: &'static str) -> Option<&'static str> {
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

fn lookup_doc(table: &'static [DocEntry], key: &str) -> Option<&'static str> {
    table
        .iter()
        .find(|e| e.key == key)
        .and_then(|e| nonempty(e.doc))
}

fn lookup_doc_by_label(table: &'static [DocEntry], label: &str) -> Option<&'static str> {
    table
        .iter()
        .find(|e| e.label == label)
        .and_then(|e| nonempty(e.doc))
}

pub fn command_completion_label(key: &str) -> Option<&'static str> {
    COMMAND_COMPLETIONS
        .iter()
        .find(|entry| entry.key == key)
        .map(|entry| entry.label)
}

pub fn meta_doc(label: &str) -> Option<&'static str> {
    META_KEYWORDS
        .iter()
        .find(|e| e.key == label)
        .map(|e| if e.doc.is_empty() { e.detail } else { e.doc })
}

pub fn platform_value_doc(label: &str) -> Option<&'static str> {
    lookup_doc(PLATFORM_VALUES, label)
}

pub fn option_value_doc(label: &str) -> Option<&'static str> {
    lookup_doc(OPTION_VALUES, label)
}

pub fn timesig_value_doc(label: &str) -> Option<&'static str> {
    lookup_doc(TIMESIG_VALUES, label)
}

pub fn group_value_doc(label: &str) -> Option<&'static str> {
    lookup_doc(GROUP_VALUES, label)
}

/// Snippet body for the "Default FM template" item the FM completer
/// appends at the bottom of its file-suggestion list. Body starts with
/// a leading space so it concatenates cleanly with the preceding `fm`
/// keyword (`@N fm` + this snippet = `@N fm ; default …`).
pub const FM_DEFAULT_TEMPLATE: &str =
    " ; ${1:default}\n; ALG  FB\n    ${2:3}   ${3:4}\n;  AR  DR  SR  RR  SL  TL  KS  ML  DT SSG\n   ${4:31}   ${5:0}   ${6:0}   ${7:5}   ${8:0}   ${9:48}   ${10:0}   ${11:1}   ${12:3}   ${13:0} ; OP1\n   ${14:31}   ${15:0}   ${16:0}   ${17:5}   ${18:0}   ${19:36}   ${20:0}   ${21:1}   ${22:3}   ${23:0} ; OP2\n   ${24:31}   ${25:0}   ${26:0}   ${27:5}   ${28:0}   ${29:24}   ${30:0}   ${31:1}   ${32:3}   ${33:0} ; OP3\n   ${34:31}   ${35:0}   ${36:0}   ${37:5}   ${38:0}   ${39:12}   ${40:0}   ${41:1}   ${42:4}   ${43:0} ; OP4\n";

pub fn instrument_doc(label: &str) -> Option<&'static str> {
    lookup_doc(INSTRUMENT_TYPES, label)
}

pub fn rate_offset_doc(label: &str) -> Option<&'static str> {
    lookup_doc(RATE_OFFSET, label)
}

pub fn rate_offset_label(label: &str) -> Option<&'static str> {
    RATE_OFFSET
        .iter()
        .find(|e| e.key == label)
        .map(|e| e.label)
}

pub fn command_doc(label: &str) -> Option<&'static str> {
    lookup_doc(COMMAND_COMPLETIONS, label)
}

/// Resolve a command entry, preferring the signed variant when `signed=true`.
/// Several commands (`V`, `p`, `k`, `K`) ship two entries that share the same
/// `key`: an unsigned form (`V<0..127>`) and a signed one (`V<-128..+127>`).
/// When the call site saw an explicit `+/-` prefix on the operand it wants the
/// signed variant.
pub fn command_entry(key: &str, signed: bool) -> Option<(&'static str, &'static str)> {
    let matches = COMMAND_COMPLETIONS.iter().filter(|e| e.key == key);
    let mut first: Option<&DocEntry> = None;
    let mut signed_match: Option<&DocEntry> = None;
    for entry in matches {
        if first.is_none() {
            first = Some(entry);
        }
        if entry.label.contains("<-") {
            signed_match = Some(entry);
        }
    }
    let entry = if signed { signed_match.or(first) } else { first }?;
    if entry.doc.is_empty() {
        return None;
    }
    Some((entry.label, entry.doc))
}

pub fn at_meta_doc(label: &str) -> Option<&'static str> {
    lookup_doc_by_label(AT_META, label)
}

pub fn platform_command_doc(key: &str) -> Option<&'static str> {
    lookup_doc(PLATFORM_COMMANDS, key)
}

pub fn platform_command_label(key: &str) -> Option<&'static str> {
    PLATFORM_COMMANDS
        .iter()
        .find(|e| e.key == key)
        .map(|e| e.label)
}

pub fn fm_param_doc(key: &str) -> Option<(&'static str, &'static str)> {
    FM_PARAM_DOCS
        .iter()
        .find(|entry| entry.key == key)
        .map(|entry| (entry.label, entry.doc))
}

pub fn two_op_param_doc(index: usize) -> Option<(&'static str, &'static str)> {
    TWO_OP_PARAM_DOCS
        .get(index)
        .map(|entry| (entry.label, entry.doc))
}

pub const TRACK_DOC_NUMERIC: &str = "Select track by number.\n\n\
`A`..`Z` are shorthand for `*0`..`*25`.\n\n\
On Mega Drive, `*0`..`*15` are the channel/dummy tracks.\n\
`*32` and above are typically used as macro/subroutine tracks.";

pub const TRACK_DOC_LETTERS: &str = "Select tracks.\n\n\
`A`..`Z` are shorthand for `*0`..`*25`.\n\
`*32` and above are typically used as macro/subroutine tracks.\n\n\
Channel mapping:\n\
- FM = `ABCDEF`\n\
- PSG tone = `GHI`\n\
- PSG noise = `J`\n\
- PCM = `FKL` (shares FM6)\n\
- FM3 Special = `C` + helper tracks\n\
- Usually `MNOP`; `I` and `L` may also be used";

fn is_numeric_track_label(label: &str) -> bool {
    label.starts_with('*')
        && label.len() > 1
        && label[1..].chars().all(|c| c.is_ascii_digit())
}

pub fn track_doc(label: &str) -> Option<&'static str> {
    if is_numeric_track_label(label) {
        return Some(TRACK_DOC_NUMERIC);
    }
    if !label.is_empty() && label.chars().all(|c| c.is_ascii_uppercase()) {
        return Some(TRACK_DOC_LETTERS);
    }
    None
}

#[derive(Serialize)]
pub struct AllDocs {
    pub meta_keywords: &'static [DocEntry],
    pub meta_keywords_trigger_suggest: &'static [&'static str],
    pub platform_values: &'static [DocEntry],
    pub option_values: &'static [DocEntry],
    pub timesig_values: &'static [DocEntry],
    pub group_values: &'static [DocEntry],
    pub instrument_types: &'static [DocEntry],
    pub instrument_types_trigger_suggest: &'static [&'static str],
    pub at_meta: &'static [DocEntry],
    pub at_meta_completion_labels: &'static [&'static str],
    pub rate_offset: &'static [DocEntry],
    pub commands: &'static [DocEntry],
    pub help_only_commands: &'static [DocEntry],
    pub platform_commands: &'static [DocEntry],
    pub fm_params: &'static [DocEntry],
    pub two_op_params: &'static [DocEntry],
    pub track_doc_numeric: &'static str,
    pub track_doc_letters: &'static str,
    pub fm_default_template: &'static str,
}

pub fn all_docs() -> AllDocs {
    AllDocs {
        meta_keywords: META_KEYWORDS,
        meta_keywords_trigger_suggest: META_KEYWORDS_TRIGGER_SUGGEST,
        platform_values: PLATFORM_VALUES,
        option_values: OPTION_VALUES,
        timesig_values: TIMESIG_VALUES,
        group_values: GROUP_VALUES,
        instrument_types: INSTRUMENT_TYPES,
        instrument_types_trigger_suggest: INSTRUMENT_TYPES_TRIGGER_SUGGEST,
        at_meta: AT_META,
        at_meta_completion_labels: AT_META_COMPLETION_LABELS,
        rate_offset: RATE_OFFSET,
        commands: COMMAND_COMPLETIONS,
        help_only_commands: HELP_ONLY_COMMANDS,
        platform_commands: PLATFORM_COMMANDS,
        fm_params: FM_PARAM_DOCS,
        two_op_params: TWO_OP_PARAM_DOCS,
        track_doc_numeric: TRACK_DOC_NUMERIC,
        track_doc_letters: TRACK_DOC_LETTERS,
        fm_default_template: FM_DEFAULT_TEMPLATE,
    }
}
