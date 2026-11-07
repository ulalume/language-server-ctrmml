# language-server-ctrmml

Language Server for ctrmml.

## Build

```sh
cargo build
```

Building requires a C++20 compiler; the `ym2612_format` sources are compiled by the crate's build script.

## Run (stdio)

```sh
cargo run
```

## Options

`initializationOptions` keys, snake_case or camelCase. Keys a client leaves out are guessed from its name (VS Code or other).

| Key | Value | Meaning |
| --- | --- | --- |
| `code_lens_icons` | `"codicon"` \| `"text"` | Icon syntax in lens titles |
| `code_lens_file_actions` | bool | Load and Save lenses |
| `command_code_actions` | bool | Play, stop, export, quickrom and mdslink as code actions |
| `client_commands` | string[] | Commands the client registers itself; not advertised |
| `command_path` | string | The `ctrmml-cmd` to run |
| `arpeggio_enabled` | bool | Arpeggio completions |
| `arpeggio_pattern` | `"up"` \| `"down"` \| `"updown"` \| `"downup"` \| `"alberti"` | Arpeggio order |
| `chord_stack_mode` | `"stack_up"` \| `"plain"` | Chord voicing |
| `fm_picker_hierarchy` | bool | Pick an instrument file, then a patch |

`command_path` and the completion keys are read again from the `ctrmml` configuration section on `workspace/didChangeConfiguration`: pulled with `workspace/configuration` when the client answers with an object, else taken from the notification's settings.

## FM instrument completion

This server provides FM instrument completion via the `ym2612_format` library, linked into the binary.

- Workspace instrument files (.dmp, .dmf, .fui, .fur, .gin, .ginpkg, .rym2612, .tfi, .vgi, .eif, .vgm, .vgz, .spat, .tyi, .y12, .dat, .ins) are auto-scanned and cached.
- Completing after `@N fm` inserts FM parameters as MML.
- The scanned extension set comes from the library's format list.

## Playback integration

This server can control playback and exports via `ctrmml-cmd`.

- By default, the `ctrmml-cmd` release pinned in `src/ctrmml_cmd.rs` (`CTRMML_CMD_VERSION`) is downloaded from https://github.com/ulalume/ctrmml-cmd (GitHub Releases).
- Auto-downloaded binaries are cached under `~/.cache/ctrmml-cmd` (or OS temp if cache is unavailable).
- Optional overrides:
  - `command_path` option
  - `CTRMML_CMD_PATH` environment variable
  - `ctrmml-cmd` on PATH
- Use Code Actions in Zed to run: `ctrmml: play`, `ctrmml: play from cursor`, `ctrmml: stop`, `ctrmml: export vgm`, `ctrmml: export wav`, `ctrmml: mdslink file`, `ctrmml: mdslink directory`, `ctrmml: mdslink from mdslink.json`, `ctrmml: quickrom file`, `ctrmml: quickrom directory`, `ctrmml: quickrom from quickrom.json`.
- Playback highlighting is sent as Diagnostics with source `ctrmml-playback`.
