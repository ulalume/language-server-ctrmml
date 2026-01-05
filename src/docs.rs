

pub(crate) struct CommandCompletion {
    pub(crate) key: &'static str,
    pub(crate) label: &'static str,
    pub(crate) insert: &'static str,
}

pub(crate) const META_KEYWORDS: &[&str] = &[
    "#title",
    "#composer",
    "#author",
    "#date",
    "#comment",
    "#platform",
    "#option",
    "#game",
    "#composerj",
    "#programmer",
];

pub(crate) const PLATFORM_VALUES: &[&str] = &["megadrive", "mdsdrv"];
pub(crate) const OPTION_VALUES: &[&str] = &["noextpitch"];
pub(crate) const INSTRUMENT_TYPES: &[&str] = &["pcm", "fm", "psg", "2op"];

pub(crate) const COMMAND_COMPLETIONS: &[CommandCompletion] = &[
    CommandCompletion {
        key: "o",
        label: "o<0..7>",
        insert: "o${1:0..7}",
    },
    CommandCompletion {
        key: "l",
        label: "l<duration>",
        insert: "l${1:duration}",
    },
    CommandCompletion {
        key: "Q",
        label: "Q<1..8>",
        insert: "Q${1:1..8}",
    },
    CommandCompletion {
        key: "q",
        label: "q<1..8>",
        insert: "q${1:1..8}",
    },
    CommandCompletion {
        key: "s",
        label: "s<ticks>",
        insert: "s${1:ticks}",
    },
    CommandCompletion {
        key: "C",
        label: "C<ticks>",
        insert: "C${1:ticks}",
    },
    CommandCompletion {
        key: "R",
        label: "R<duration>",
        insert: "R${1:duration}",
    },
    CommandCompletion {
        key: "t",
        label: "t<bpm>",
        insert: "t${1:bpm}",
    },
    CommandCompletion {
        key: "T",
        label: "T<value>",
        insert: "T${1:value}",
    },
    CommandCompletion {
        key: "v",
        label: "v<0..15>",
        insert: "v${1:0..15}",
    },
    CommandCompletion {
        key: "V",
        label: "V<0..255>",
        insert: "V${1:0..255}",
    },
    CommandCompletion {
        key: "V",
        label: "V<-128..127>",
        insert: "V${1:-128..127}",
    },
    CommandCompletion {
        key: "p",
        label: "p<-128..127>",
        insert: "p${1:-128..127}",
    },
    CommandCompletion {
        key: "k",
        label: "k<-128..127>",
        insert: "k${1:-128..127}",
    },
    CommandCompletion {
        key: "K",
        label: "K<-128..127>",
        insert: "K${1:-128..127}",
    },
    CommandCompletion {
        key: "E",
        label: "E<0..255>",
        insert: "E${1:0..255}",
    },
    CommandCompletion {
        key: "M",
        label: "M<0..255>",
        insert: "M${1:0..255}",
    },
    CommandCompletion {
        key: "P",
        label: "P<0..255>",
        insert: "P${1:0..255}",
    },
    CommandCompletion {
        key: "G",
        label: "G<0..255>",
        insert: "G${1:0..255}",
    },
    CommandCompletion {
        key: "D",
        label: "D<0..255>",
        insert: "D${1:0..255}",
    },
    CommandCompletion {
        key: "r",
        label: "r<duration>",
        insert: "r${1:duration}",
    },
    CommandCompletion {
        key: "L",
        label: "L",
        insert: "L",
    },
    CommandCompletion {
        key: "^",
        label: "^",
        insert: "^",
    },
    CommandCompletion {
        key: "&",
        label: "&",
        insert: "&",
    },
    CommandCompletion {
        key: "_",
        label: "_<-128..127>",
        insert: "_${1:-128..127}",
    },
    CommandCompletion {
        key: "__",
        label: "__<-128..127>",
        insert: "__${1:-128..127}",
    },
    CommandCompletion {
        key: "_{",
        label: "_{<data>}",
        insert: "_{${1:data}}",
    },
];
pub(crate) struct PlatformCommand {
    pub(crate) key: &'static str,
    pub(crate) label: &'static str,
    pub(crate) insert: &'static str,
    pub(crate) doc: &'static str,
}

pub(crate) fn command_completion_label(key: &str) -> Option<&'static str> {
    COMMAND_COMPLETIONS
        .iter()
        .find(|entry| entry.key == key)
        .map(|entry| entry.label)
}

pub(crate) const PLATFORM_COMMANDS: &[PlatformCommand] = &[
    PlatformCommand {
        key: "_",
        label: "_<-128..127>",
        insert: "_0",
        doc: "Set transpose.",
    },
    PlatformCommand {
        key: "__",
        label: "__<-128..127>",
        insert: "__0",
        doc: "Set relative transpose.",
    },
    PlatformCommand {
        key: "_{",
        label: "_{<data>}",
        insert: "_{c}",
        doc: "Set key signature.",
    },
    PlatformCommand {
        key: "fm3",
        label: "fm3 <mask>",
        insert: "fm3 0000",
        doc: "Enables FM3 special mode. Mask selects affected operators (e.g. 0011). Use 1111 to disable. Can be used on PSG I or dummy KLMNOP to temporarily use this track for FM3.",
    },
    PlatformCommand {
        key: "lfo",
        label: "lfo <0..3> <0..7>",
        insert: "lfo 0 0",
        doc: "Set hardware LFO AM sensitivity (first) and PM sensitivity (second).",
    },
    PlatformCommand {
        key: "lforate",
        label: "lforate <0..9>",
        insert: "lforate 0",
        doc: "Set hardware LFO rate. 0 disables; 1..9 increase speed (last two are much faster).",
    },
    PlatformCommand {
        key: "mode",
        label: "mode <0..1>",
        insert: "mode 0",
        doc: "For PSG noise channel J, enable using tone channel I as noise frequency source. Controlling both channels can conflict.",
    },
    PlatformCommand {
        key: "pcmmode",
        label: "pcmmode <2..3>",
        insert: "pcmmode 2",
        doc: "(mdsdrv only) 2: 2ch PCM up to 17.5 kHz. 3: 3ch PCM up to 13 kHz.",
    },
    PlatformCommand {
        key: "pcmrate",
        label: "pcmrate <1..8>",
        insert: "pcmrate 1",
        doc: "Change PCM pitch in ~2.2 kHz steps. Temporary until next instrument change.",
    },
    PlatformCommand {
        key: "write",
        label: "write <register> <data>",
        insert: "write 00 00",
        doc: "Write FM registers directly. Aliases: dtml*, ksar*, amdr*, sr*, slrr*, ssg*, fbal (use operator number for *). Temporary until next instrument change.",
    },
    PlatformCommand {
        key: "tl1",
        label: "tl1 <value>",
        insert: "tl1 0",
        doc: "Set base operator total level for OP1. Use +/-. Temporary until next instrument change.",
    },
    PlatformCommand {
        key: "tl2",
        label: "tl2 <value>",
        insert: "tl2 0",
        doc: "Set base operator total level for OP2. Use +/-. Temporary until next instrument change.",
    },
    PlatformCommand {
        key: "tl3",
        label: "tl3 <value>",
        insert: "tl3 0",
        doc: "Set base operator total level for OP3. Use +/-. Temporary until next instrument change.",
    },
    PlatformCommand {
        key: "tl4",
        label: "tl4 <value>",
        insert: "tl4 0",
        doc: "Set base operator total level for OP4. Use +/-. Temporary until next instrument change.",
    },
];

pub(crate) fn meta_doc(label: &str) -> Option<&'static str> {
    match label {
        "#title" | "#composer" | "#author" | "#date" | "#comment" | "#game" | "#composerj"
        | "#programmer" => Some("Song metadata."),
        "#platform" => Some("Sets the MML target platform."),
        "#option" => Some("Sets platform options."),
        _ => None,
    }
}

pub(crate) fn platform_value_doc(label: &str) -> Option<&'static str> {
    match label {
        "megadrive" => Some("Use VGM datablocks and DAC stream commands to play back samples."),
        "mdsdrv" => Some(
            "Simulate MDSDRV's PCM driver (2-3 channel mixing). Sample rate is fixed to ~2 kHz steps.",
        ),
        _ => None,
    }
}

pub(crate) fn instrument_doc(label: &str) -> Option<&'static str> {
    match label {
        "fm" => Some("FM instruments are defined as below."),
        "2op" => Some(
            "Instrument type `2op` is used to duplicate FM instruments, modifying the operators' multiply ratios and setting a transpose.",
        ),
        "psg" => Some("PSG instruments (envelopes) are defined as a sequence of values."),
        "pcm" => Some(
            "PCM samples are defined as instruments. The first parameter is the path to the sample (relative to that of the MML file).",
        ),
        _ => None,
    }
}


pub(crate) fn option_value_doc(label: &str) -> Option<&'static str> {
    match label {
        "noextpitch" => Some("Disable extended pitch envelopes for compatibility."),
        _ => None,
    }
}

pub(crate) fn rate_offset_doc(label: &str) -> Option<&'static str> {
    match label {
        "rate=" => Some("Override the sample rate."),
        "offset=" => Some("Adjust the start position."),
        _ => None,
    }
}

pub(crate) fn command_doc(label: &str) -> Option<&'static str> {
    match label {
        "o" => Some("Set octave."),
        "l" => Some("Set default duration, used if not specified by notes, rests, `R` or `~` commands."),
        "Q" => Some("Quantize. Used to set articulation. Note length is param/8."),
        "q" => Some("Set early release. Used to set articulation."),
        "C" => Some("Set the length of a measure (or a whole note) in ticks."),
        "R" => Some("Reverse rest. This subtracts the value from the previous note or rest."),
        "L" => Some("Set loop point (segno). If this is present, playback resumes at this point when the end of the track is reached."),
        "s" => Some("Set shuffle. The specified number of ticks will be added to the the next note, rest or tie, then subtracted from the next."),
        "t" => Some("Set tempo in BPM."),
        "T" => Some("Set tempo using the platform's native timer values."),
        "v" => Some("Set volume."),
        "V" => Some("Set volume (fine), or modify volume (fine) depending on parameter range."),
        "p" => Some("Set panning.\n\n#### Limitations\nPanning using the `p` command is only allowed for FM channels and the accepted range is 0-3. Bit 1 enables the right channel, bit 2 enables the left channel."),
        "k" => Some("Set transpose. Default behavior is the same as the `_` command."),
        "K" => Some("Set detune."),
        "E" => Some("Set envelope. 0 to disable."),
        "M" => Some("Set pitch envelope. 0 to disable."),
        "P" => Some("Set pan envelope or macro track. 0 to disable."),
        "G" => Some("Set portamento. 0 to disable."),
        "D" => Some("Set drum mode. 0 disables drum mode."),
        "r" => Some("Rest. Optionally set duration after the rest."),
        "^" => Some("Tie. Extends duration of previous note."),
        "&" => Some("Slur. Used to connect two notes (legato)."),
        _ => None,
    }
}

pub(crate) fn at_meta_doc(label: &str) -> Option<&'static str> {
    match label {
        "@<num>" => Some("Defines an instrument. Parameters are platform-specific."),
        "@E<num>" => Some("Defines an envelope."),
        "@M<num>" => Some("Defines a pitch envelope."),
        "@P<num>" => Some("Defines a pan envelope."),
        _ => None,
    }
}

pub(crate) fn platform_command_doc(key: &str) -> Option<&'static str> {
    PLATFORM_COMMANDS
        .iter()
        .find(|cmd| cmd.key == key)
        .map(|cmd| cmd.doc)
}

pub(crate) struct FmParamDoc {
    pub(crate) key: &'static str,
    pub(crate) label: &'static str,
    pub(crate) doc: &'static str,
}

pub(crate) const FM_PARAM_DOCS: &[FmParamDoc] = &[
    FmParamDoc {
        key: "ALG",
        label: "Algorithm (0..7)",
        doc: "",
    },
    FmParamDoc {
        key: "FB",
        label: "Feedback (0..7)",
        doc: "",
    },
    FmParamDoc {
        key: "AR",
        label: "Attack Rate (0..31)",
        doc: "",
    },
    FmParamDoc {
        key: "DR",
        label: "Decay Rate (0..31)",
        doc: "",
    },
    FmParamDoc {
        key: "SR",
        label: "Sustain Rate (0..31)",
        doc: "",
    },
    FmParamDoc {
        key: "RR",
        label: "Release Rate (0..15)",
        doc: "",
    },
    FmParamDoc {
        key: "SL",
        label: "Sustain Level (0..15)",
        doc: "",
    },
    FmParamDoc {
        key: "TL",
        label: "Total Level (0..127)",
        doc: "",
    },
    FmParamDoc {
        key: "KS",
        label: "Key Scale (0..3)",
        doc: "",
    },
    FmParamDoc {
        key: "ML",
        label: "Multiple (0..15)",
        doc: "1 is normal, 2 is 2x, 3 is 3x, 0 is 0.5x.",
    },
    FmParamDoc {
        key: "DT",
        label: "Detune (0..7)",
        doc: "",
    },
    FmParamDoc {
        key: "SSG",
        label: "SSG-EG",
        doc: "SSG-EG type: 0..7, enabled: +8, AM enabled: +100.",
    },
    FmParamDoc {
        key: "TRS",
        label: "Transpose (-24..24)",
        doc: "",
    },
];

pub(crate) fn fm_param_doc(key: &str) -> Option<(&'static str, &'static str)> {
    FM_PARAM_DOCS
        .iter()
        .find(|entry| entry.key == key)
        .map(|entry| (entry.label, entry.doc))
}

pub(crate) struct TwoOpParamDoc {
    pub(crate) label: &'static str,
    pub(crate) doc: &'static str,
}

pub(crate) const TWO_OP_PARAM_DOCS: &[TwoOpParamDoc] = &[
    TwoOpParamDoc {
        label: "FM Instrument Number",
        doc: "FM instrument number to duplicate.",
    },
    TwoOpParamDoc {
        label: "OP1 Multiple (0..15)",
        doc: "Operator 1 multiple (0..15). 1 is normal, 2 is 2x, 3 is 3x, 0 is 0.5x.",
    },
    TwoOpParamDoc {
        label: "OP2 Multiple (0..15)",
        doc: "Operator 2 multiple (0..15). 1 is normal, 2 is 2x, 3 is 3x, 0 is 0.5x.",
    },
    TwoOpParamDoc {
        label: "OP3 Multiple (0..15)",
        doc: "Operator 3 multiple (0..15). 1 is normal, 2 is 2x, 3 is 3x, 0 is 0.5x.",
    },
    TwoOpParamDoc {
        label: "OP4 Multiple (0..15)",
        doc: "Operator 4 multiple (0..15). 1 is normal, 2 is 2x, 3 is 3x, 0 is 0.5x.",
    },
    TwoOpParamDoc {
        label: "Transpose (-24..24)",
        doc: "Transpose in semitones (-24..24).",
    },
];

pub(crate) fn two_op_param_doc(index: usize) -> Option<(&'static str, &'static str)> {
    TWO_OP_PARAM_DOCS
        .get(index)
        .map(|entry| (entry.label, entry.doc))
}

pub(crate) fn track_doc(label: &str) -> Option<&'static str> {
    if label.starts_with('*') && label[1..].chars().all(|c| c.is_ascii_digit()) {
        return Some("Select track by number.");
    }
    if label.chars().all(|c| c.is_ascii_uppercase()) {
        return Some(
            "Select tracks.\n\n\
Channel mapping:\n\
- `ABCDEF` = FM 1-6\n\
- `GHI` = PSG tone 1-3 (`I` may be FM3 special mode)\n\
- `J` = PSG noise\n\
- `KL` = PCM 2-3\n\
- `MNOP` = Dummy (may be FM3 special mode).\n\n\
Channels `F`,`K`,`L` can play PCM instruments; PCM takes priority over FM at channel 6 (`F`). With `#platform mdsdrv`, software mixing and volume control apply to these PCM channels.",
        );
    }
    None
}
