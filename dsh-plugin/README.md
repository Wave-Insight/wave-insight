# Wave Insight for DeepSeek Harness

A DSH bundle that gives the AI waveform tools and a live waveform panel. It is
the third front end of [Wave Insight](https://github.com/Wave-Insight/wave-insight),
beside the browser and desktop builds.

## What it does

- Opens VCD, FST and GHW dumps through the project's native memory-mapped reader.
- Exposes eight model-facing tools: `load_waveform`, `find_signals`, `add_signals`,
  `remove_signals`, `set_time_range`, `set_cursors`, `get_changes`, `list_waveform`.
  `get_changes` returns bounded time/value pairs to the model, not just counts.
- Opens the **Wave Insight** tab in DSH's right sidebar when a tool result first
  carries a dump. The tab renders the **real Wave Insight viewer**, not a
  simplified sketch: the Host starts a loopback `wave-insight-web-backend` that
  hosts the built UI and answers waveform queries from the native, memory-mapped
  reader. Later tool calls forward view changes to that UI (one-way).
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

The embedded UI needs the matching `wave-insight-web-backend` binary (same
platform list) and the built viewer in `ui/`. Override the backend with the
`backendPath` config or `WAVE_WEB_BACKEND` environment variable. Both
`helperPath` and `backendPath` are declared in the plugin's `Config`, so they
can be set in the bundle's `cordis.patch.yml` row. If either the backend or the
UI is unavailable, the Host tools keep working and the tab shows the reason
instead of a blank panel.

## Limits

- The UI-to-tool direction is not wired yet: zooming, dragging cursors or adding
  signals inside the embedded viewer does not write back to the DSH session.
- The backend is loopback-only and has no authentication; it is meant for a
  local DSH process, not for exposure on a network.
- A few desktop-only viewer features (file dialogs, session reopen, MCP panel)
  are not present in the Web build of the viewer.
