# language-server-ctrmml

Language Server for ctrmml.

## Build

```sh
cargo build
```

## Run (stdio)

```sh
cargo run
```

## Playback integration

This server can control playback and exports via `ctrmml-cmd`.

- By default, `ctrmml-cmd` is auto-downloaded from https://github.com/ulalume/ctrmml-cmd (GitHub Releases).
- Auto-downloaded binaries are cached under `~/.cache/ctrmml-cmd` (or OS temp if cache is unavailable).
- Optional overrides:
  - `command_path` in LSP initialization options
  - `CTRMML_CMD_PATH` environment variable
  - `ctrmml-cmd` on PATH
- Use Code Actions in Zed to run: `ctrmml: play`, `ctrmml: play from cursor`, `ctrmml: stop`, `ctrmml: export vgm`, `ctrmml: export wav`, `ctrmml: mdslink file`, `ctrmml: mdslink directory`, `ctrmml: mdslink from mdslink.json`, `ctrmml: quickrom file`, `ctrmml: quickrom directory`, `ctrmml: quickrom from quickrom.json`.
- Playback highlighting is sent as Diagnostics with source `ctrmml-playback`.
