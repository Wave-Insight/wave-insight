# Wave Insight for DeepSeek Harness

A DSH bundle that gives the AI waveform tools and shows the real Wave Insight
viewer in a right-sidebar tab. It is the third front end of
[Wave Insight](https://github.com/Wave-Insight/wave-insight), beside the browser
and desktop builds.

## What it does

- Opens VCD, FST, GHW and FSDB dumps through the project's native, memory-mapped
  reader (an FSDB trace is converted once by Verdi's `fsdb2vcd` and cached).
- Reads the Verilog/SystemVerilog the dump came from with the same library the
  source pane uses, so a model and the viewer cannot disagree about where a
  signal is declared.
- Exposes thirteen model-facing tools:

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
  | `find_sources` | The companion RTL next to a dump (`rtl/`, `generated/`, `verilog/`). |
  | `list_modules` | Module definitions, ports and instances of some sources. |
  | `find_declaration` | Where a hierarchical signal is declared, driven and read. |
  | `get_diagnostics` | What the parser and the elaborator could not resolve. |
  | `load_sources` | Open `.v`/`.sv` files in the panel's source pane. |

  The four read-only design tools take `paths` (explicit `.v`/`.sv` files) or
  `dump` (a waveform to find its companion RTL next to); `dump` defaults to the
  file the panel is showing, and they leave the panel untouched. `load_sources`
  is the one that changes it: the opened list is part of the durable view, so
  the panel replays the same sources after a Host restart. The iframe reads each
  file through the loopback backend's `read_source` (the `open_source` command),
  which is why the panel shows exactly the design the model reads.

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
  bridge.js         JSON-lines protocol to the native helper (waveform + design)
  daemon.js         loopback viewer lifecycle (one child per Host, refcounted)
  projection.js     the `waveInsight` session projection
  sessions.js       per-session view cache, restored from the projection
  tools.js          the thirteen tool definitions
  schema.js         dependency-free structural schemas
client.js           Client half: tab type, slot body, view diff, handshake
ui/                 built web-remote viewer (wasm + assets)
bin/                native helper + viewer backend, staged per platform (not in git)
scripts/binaries.mjs  stages a local build into bin/ and gates the package
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

A checkout carries no native binaries — `bin/` is ignored by git — so stage the
pair for this machine first:

```text
cargo build -p dsh-bridge -p wave-insight-web-backend --release
node dsh-plugin/scripts/binaries.mjs stage
```

A released tarball already carries all eight files; skip that step when you
installed one.

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

Every supported target gets both native pieces. The table is the `PLATFORMS`
matrix in [`host/assets.js`](host/assets.js): `package.json` lists exactly those
file names, and `prepublishOnly` ([`scripts/binaries.mjs`](scripts/binaries.mjs))
refuses to publish a package with a row missing.

| Platform | `dsh-bridge` helper | Viewer backend |
|---|---|---|
| Windows x64 | `bin/dsh-bridge-windows-x64.exe` | `bin/wave-insight-web-backend-windows-x64.exe` |
| Linux x64 | `bin/dsh-bridge-linux-x64` | `bin/wave-insight-web-backend-linux-x64` |
| macOS ARM64 | `bin/dsh-bridge-macos-arm64` | `bin/wave-insight-web-backend-macos-arm64` |
| macOS x64 | `bin/dsh-bridge-macos-x64` | `bin/wave-insight-web-backend-macos-x64` |
| anything else | build `dsh-bridge` and set `helperPath` | build the backend and set `backendPath` |

The viewer itself (`ui/`) is platform independent: one wasm bundle.

The binaries are **not** committed. `.github/workflows/dsh-plugin-binaries.yml`
builds the four pairs and the release job unpacks them into the tarball; on a
checkout, `cargo build` plus `node dsh-plugin/scripts/binaries.mjs stage` puts the
pair for this machine where the Host looks. Without one the tools answer
`HELPER_UNAVAILABLE` with the paths they tried, and the panel shows why it has no
backend instead of failing activation. The Linux build targets
`x86_64-unknown-linux-musl`, so the two binaries are static and start on Alpine
as well as on any glibc distribution, instead of tracking which libc generation
the machine happens to carry.

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
- The side panel needs the viewer backend for the machine it runs on. A released
  tarball has every platform; a checkout has only the pair somebody staged.

## Development

```text
node qa/dsh-plugin-smoke.mjs        # Host tools (waveform + design), projection, helper
node qa/dsh-plugin-client.test.mjs  # Client view diff and registrations
node qa/dsh-invoke-shim.test.mjs    # Web shim arguments and origin checks
node qa/web-backend-smoke.mjs       # Loopback backend (needs the release build)
node qa/viewer-build-stamp.mjs      # Fails when ui/ predates front-end/
cargo build -p dsh-bridge -p wave-insight-web-backend --release
node dsh-plugin/scripts/binaries.mjs stage     # puts that build in bin/
node dsh-plugin/scripts/binaries.mjs verify    # what prepublishOnly runs
```

`stage` also takes `--from <dir>` (a directory of already release-named files,
which is how the release job unpacks the workflow artifacts) and `--set
<platform/arch>` for a cross build; `verify --tarball <file>` checks a packed
`.tgz` for all eight files.

`ui/` is a build artifact of `front-end/` (web-remote wasm) plus `dsh-invoke.js`.
After rebuilding it, refresh the stamp so CI keeps the two in step:

```text
cd front-end && trunk build --no-default-features --features web-remote --release
# copy dist's js/wasm/css and index.html into dsh-plugin/ui, keep dsh-invoke.js
node qa/viewer-build-stamp.mjs --write
```

After changing the Host half, disable and re-enable the plugin row (or restart
DSH); after changing the Client half, reload the page.
