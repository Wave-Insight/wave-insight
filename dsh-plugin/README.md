# Wave Insight for DeepSeek Harness

A DSH bundle that gives the AI waveform tools and shows the real Wave Insight
viewer in a right-sidebar tab. It is the third front end of
[Wave Insight](https://github.com/Wave-Insight/wave-insight), beside the browser
and desktop builds.

## What it does

- Opens VCD, FST and GHW dumps through the project's native, memory-mapped reader.
- Exposes eight model-facing tools:

  | Tool | Purpose |
  |---|---|
  | `load_waveform` | Open a dump (replaces any previous one). |
  | `find_signals` | Search hierarchical signal paths. |
  | `add_signals` | Validate and add rows to the wave. |
  | `remove_signals` | Remove rows. |
  | `set_time_range` | Set the visible time window (clamped to the dump). |
  | `set_cursors` | Place or clear the two time cursors. |
  | `get_changes` | Return bounded `time=value` pairs to the model and show the signals. |
  | `list_waveform` | Report the current dump, rows, window and cursors. |

- Opens the **Wave Insight** tab in DSH's right sidebar when a tool result first
  carries a dump. The tab renders the real Wave Insight viewer: the Host starts a
  loopback `wave-insight-web-backend` that serves the built UI and answers
  waveform queries from the native reader. Later tool calls forward view changes
  to that UI (one-way).
- Reopens from the right-bar guide after the tab was closed.

## How it is put together

```
index.js            Host entry: re-exports apply / Config / inject
host/
  plugin.js         wiring: assets, viewer, projection, tools, prompt section
  assets.js         bundled binary lookup (config -> env -> bin/)
  bridge.js         JSON-lines protocol to the native helper
  daemon.js         loopback viewer lifecycle (one child per Host, refcounted)
  projection.js     the `waveInsight` session projection
  sessions.js       per-session view cache, restored from the projection
  tools.js          the eight tool definitions
  schema.js         dependency-free structural schemas
client.js           Client half: tab type, slot body, view diff, handshake
ui/                 built web-remote viewer (wasm + assets)
bin/                native helper and viewer backend binaries
```

Every tool result carries the whole view in its presentation meta
(`{ waveInsight: { version, state } }`); the projection folds the last non-failed
one and the panel replays the difference as `wave-dsh` commands. The viewer URL is
**not** durable: the projection strips it from the log and stamps the running
Host's URL, so a resumed session never iframes a port from a previous run. The
panel also probes `/health` before showing the iframe.

## Install

```text
Plugins > Add plugin > local directory: <repo>/dsh-plugin
```

The row is inserted by `cordis.patch.yml`; no configuration is required. Both
optional paths are declared in the plugin `Config`, so a row may override them:

```yaml
- id: wave-insight-dsh
  name: '@wave-insight/dsh-plugin'
  config:
    helperPath: 'C:/path/to/dsh-bridge.exe'
    backendPath: 'C:/path/to/wave-insight-web-backend.exe'
```

`WAVE_DSH_BRIDGE` and `WAVE_WEB_BACKEND` are the environment equivalents. A
missing viewer never disables the tools: the panel shows the reason instead.

## Platform support

| Platform | `dsh-bridge` helper | Viewer backend |
|---|---|---|
| Windows x64 | bundled | bundled |
| Linux x64 | bundled | build it: `cargo build -p wave-insight-web-backend --release`, then point `backendPath` at it (or place it as `bin/wave-insight-web-backend-linux-x64`) |
| macOS ARM64 | bundled | same, `bin/wave-insight-web-backend-macos-arm64` |
| anything else | build `dsh-bridge` and set `helperPath` | build the backend and set `backendPath` |

The viewer itself (`ui/`) is platform independent: one wasm bundle.

The committed Linux and macOS helper binaries predate the current `dsh-bridge`
source. They still work — the Host falls back to the older per-name validation
when a helper has no `validate` command — but rebuild them to get the fast path,
the GHW time-range fallback and the plain-text value rendering:
`cargo build -p dsh-bridge --release`, or take them from the
`dsh-plugin-binaries` workflow.

## Security model

- The viewer backend binds `127.0.0.1` on an OS-assigned port and prints a fresh
  random token; `/invoke` requires that token, a loopback `Host` header and this
  server's own `Origin`. That stops DNS-rebinding pages from reaching
  `open_dump`, which reads arbitrary local files by design.
- `/health` is token-free so the panel can probe a stale URL; it exposes nothing.
- The token travels in the iframe URL and in the shim's `/invoke` header, not in
  the session log.
- The backend is meant for a local DSH process. Serving the DSH page over TLS or
  from another machine cannot reach it: the iframe URL points at the viewer's own
  machine.

## Limits

- The UI-to-tool direction is not wired: zooming, dragging cursors or adding
  signals inside the embedded viewer does not write back to the DSH session.
- `get_changes` renders at most 24 value pairs per signal to the model, even when
  more were decoded.
- A GHW dump whose library reports no cheap time range falls back to the first and
  last time step; a dump with no changes at all reports no range.
- Time steps travel as JSON numbers, so a dump whose timestamps exceed 2^53 loses
  precision in the panel and in tool arguments.
- The Linux/macOS viewer backends are not bundled in this repository, so the
  panel is Windows-only out of the box.

## Development

```text
node qa/dsh-plugin-smoke.mjs        # Host tools, projection and helper
node qa/dsh-plugin-client.test.mjs  # Client view diff and registrations
node qa/web-backend-smoke.mjs       # Loopback backend (needs the release build)
cargo build -p dsh-bridge -p wave-insight-web-backend --release
```

After changing the Host half, disable and re-enable the plugin row (or restart
DSH); after changing the Client half, reload the page.
