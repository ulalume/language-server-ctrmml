pub(crate) struct PlatformCommand {
    pub(crate) key: &'static str,
    pub(crate) label: &'static str,
    pub(crate) insert: &'static str,
    pub(crate) doc: &'static str,
}

pub(crate) const PLATFORM_COMMANDS: &[PlatformCommand] = &[
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
        "#title" | "#composer" | "#author" | "#date" | "#comment" => Some("Song metadata."),
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
        "p" => Some("Set panning."),
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

pub(crate) fn track_doc(label: &str) -> Option<&'static str> {
    if label.starts_with('*') && label[1..].chars().all(|c| c.is_ascii_digit()) {
        return Some("Select track by number.");
    }
    if label.chars().all(|c| c.is_ascii_uppercase()) {
        return Some(
            "Select tracks. A span of characters at the beginning of a line selects tracks (e.g. A, ABC).",
        );
    }
    None
}
