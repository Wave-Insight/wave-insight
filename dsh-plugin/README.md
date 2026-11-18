# Wave Insight for DeepSeek Harness

A DSH bundle that gives the AI waveform tools and a live waveform panel. It is
the third front end of [Wave Insight](https://github.com/Wave-Insight/wave-insight),
beside the browser and desktop builds.

## What it does

- Opens VCD, FST and GHW dumps through the project's native memory-mapped reader.
- Exposes eight model-facing tools: `load_waveform`, `find_signals`, `add_signals`,
  `remove_signals`, `set_time_range`, `set_cursors`, `get_changes`, `list_waveform`.
- Opens the **Wave Insight** tab in DSH's right sidebar when a tool result first
  carries a dump. Later tool calls update the panel through a session projection.
- The tab can also be opened from the right-bar guide after it was closed.

## Use

Ask the AI:

```text
load_waveform with F:/path/to/dump.fst
add_signals TOP.clk and TOP.pc
set_time_range 1000 2000
```

The helper is bundled for the three common desktop platforms, so no external
installation is needed:

| Platform | Binary |
|---|---|
| Windows x64 | `bin/dsh-bridge-windows-x64.exe` |
| Linux x64 | `bin/dsh-bridge-linux-x64` |
| macOS ARM64 | `bin/dsh-bridge-macos-arm64` |

The plugin picks the right one automatically. For any other platform, build
the helper from the repository with `cargo build -p dsh-bridge` and set the
`helperPath` config or the `WAVE_DSH_BRIDGE` environment variable.

## Limits

Up to 40 rows and 256 sampled changes per row are drawn; values are bit strings
(or a debug form for reals and strings). This is a live inspection surface, not
a replacement for the full viewer.
