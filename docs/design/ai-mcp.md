# AI access: a local MCP server over the native backend

Wave Insight can be driven by an AI assistant. The desktop build runs a
**Model Context Protocol (MCP)** server on loopback; the assistant lives in an
external MCP client (Claude Code, Cursor, any client that speaks Streamable
HTTP), and this app is the tool server. Tools either **read** a waveform (the
data face) or **change what the window shows** (the control face).

This note records why the feature is shaped that way, the tool catalogue, the
wire protocol between the two halves, the security model and what is verified
and what is not. It builds on
[`large-files.md`](large-files.md) (the native mmap backend) and
[`../state-store.md`](../state-store.md) (the reducer, the effects and the
slices).

Status: **Wave 23. The MCP server, the data tools, the control bridge and the
AI / MCP panel are implemented. Desktop only; the browser build is unchanged
and the wasm bundle grows by the shared DTOs alone.**

## 1. Why MCP, and why not an embedded assistant

| Option | Verdict |
|---|---|
| **Local MCP server + external client** | Standard tool discovery and schemas; existing clients; the app only exposes tools; reuses the mmap backend. **Chosen.** |
| A custom websocket/JSON-RPC bridge | Still needs a bespoke client integration, tool loop and schema; only this app can use it. |
| An embedded chat panel | Provider protocol, streaming, tool loop, key storage and a proxy inside the app. Much larger surface, and the interesting question (does the assistant see the *real* dump?) is unchanged. |
| A headless MCP binary | Good for pure extraction, but it cannot change the running window. A later stdio entry point could reuse the same data face. |

MCP is only the tool protocol; the model stays in the client. That boundary is
what keeps this app from becoming an LLM host.

## 2. Architecture

```
MCP client --Streamable HTTP + bearer token--> src-tauri/src/agent/server.rs
                      127.0.0.1:7437/mcp            |-- agent/data.rs    -> mmap dump
                                                    `-- agent/bridge.rs  -> webview store
                                                            |
                                          event "wave-agent-command"
                                                            v
                                        front-end/src/agent/ -> Msg::AgentCommand
                                                            |
                                        store action -> UiResult -> agent_reply
```

Two deliberate properties:

* **The data tools never need the webview.** They open files themselves through
  the registry (`Waveform::open_path_mmap` with the read fallback), so an
  assistant can analyse a dump that is not open, and a query can never block or
  evict the dump the user is looking at.
* **The control tools never read files.** They send a command to the store and
  return its answer, which is what keeps the UI the single source of truth for
  the view.

## 3. The tool catalogue

Times are the dump's own ticks (the answer carries `timescale`) and every range
is inclusive on both ends.

### Data (read-only)

| Tool | Answer |
|---|---|
| `list_dumps` | The displayed dump, every open tab, format/timescale/time range, the visible range, cursors and the signal paths already on the wave. The models start here. |
| `find_signals` | Case-insensitive name search (`contains`/`prefix`/`exact`, optional scope), returns the full hierarchical paths the other tools take. |
| `get_values` | Batch values at up to 256 times for up to 32 signals, with `value_time` and `found`. |
| `get_changes` | Changes in a range, capped per signal; `capped` + `next_start` page a dense signal instead of materialising its history. |
| `search_value` | Walk one signal for a rendered value (text or `0x`/`0b`/`0o`). Worst case is a full scan, documented. |
| `summarize_signals` | One streaming pass: change count and endpoints per signal. |
| `extract_changes` | Stream a range into a CSV (long format `time,signal,value`) with a sidecar `.meta.json`, row/byte budgets and a preview. |

### Control (changes the displayed dump)

| Tool | Effect |
|---|---|
| `add_signals` | One rows batch through the same path as the sidebar's bulk add. |
| `remove_signals` | Removes the rows whose path is listed. |
| `set_time_range` | Clamped to the dump; the answer reports what was applied. |
| `set_cursors` | `{"time": t}` or `{"clear": true}` per cursor. |

All data tools take an optional `path`; absent, the displayed dump is used. The
control tools address the displayed dump only - switching tabs stays a user
gesture, so no tool can silently retarget the view.

## 4. The wire protocol between the halves

The command and result types live in `wave_insight_lib::agent` and are used by
both sides, so the JSON shape has one definition and one round-trip test.

```rust
pub enum UiCommand {
    Snapshot,
    AddSignals { signals: Vec<String> },
    RemoveSignals { signals: Vec<String> },
    SetTimeRange { start: u64, end: u64 },
    SetCursors { one: Option<CursorMove>, two: Option<CursorMove> },
}
```

`CursorMove` is `Set { time }` or `Clear`; "leave this cursor alone" is the
*absent* field. `Option<Option<i32>>` looked tempting and is wrong: JSON `null`
reads back as "absent" either way, so the two actions would collapse. The test
`the_command_tag_is_the_kind_field` pins the tagged shape.

The backend emits `wave-agent-command` with `{ id, command }` and waits for
`agent_reply { id, result | error }`. A request times out after 5 s; an answer
for an unknown id (a late reply to a timed-out request) is dropped. A reloaded
webview therefore produces an error message the model can act on, never a hung
tool call.

## 5. View state the server cannot see

A `View` covers a time range only together with the surface width (`x_axis` and
pixels-per-tick). The pane is the only place that measures it, so the store now
keeps `Nav.wave_width` (`Action::WaveSurfaceWidth`, ignored when it moves by
less than half a pixel). `View::from_time_range(start, end, width)` is the
inverse of `View::window`, with tests for the degenerate cases.

One real interaction bug came out of this: the wave pane applies its
first-paint fit in `redraw`, and a commit that arrived before that fit would be
overwritten by it. `Msg::Viewport` now marks the pane as fitted when the
committed view is not the one it holds, so an externally set range survives.

## 6. Security model

* The listener binds `127.0.0.1` only, on 7437..=7446 (the panel shows the port
  that actually bound).
* Every request needs `Authorization: Bearer <token>`. The token is 32 random
  bytes hex-encoded, stored in the app config directory (`mcp-token`, `0600` on
  Unix); a failure to store it warns in the panel and keeps a session-only
  token.
* `Host` must be loopback (`127.0.0.1`, `localhost`, `[::1]`); rmcp's own
  Origin validation stays on, which is the DNS-rebinding defence.
* **The token is the permission.** It grants reading the files the tools are
  pointed at and changing the current view; the panel says so next to the copy
  button. This was a deliberate product decision: requiring a confirmation per
  edit would interrupt the conversation, and the server is already unreachable
  from other machines.
* The only writes are extractions into the system temp directory under
  `wave-insight-agent/`; no tool takes a write path or executes anything.

## 7. Verification

Automated (host tests; the app crate runs in the `test-app` CI job):

* `wave-insight-lib`: the shared `UiCommand`/`UiResult`/`UiReply`/`AgentSnapshot`
  JSON round-trips, the tagged command shape, the omitted reply field.
* `front-end --bins`: `View::from_time_range` (exact inverse, degenerate range,
  unmeasured surface, huge ticks), `Action::WaveSurfaceWidth` noise handling, and
  the snapshot builder (visible window, unmeasured fallback, native dump as the
  active entry, row dedupe, tab path making a dump queryable, empty state).
* `app`: the data face on a temporary VCD (open/cache, name modes and scopes,
  miss suggestions, batch values, capped changes with `next_start`, search,
  summary), the extraction (CSV contents, metadata, range, row budget,
  empty/reversed range, CSV quoting), the token (generation, file round trip,
  constant-time compare), Host validation, the bridge (reply routing, listener
  error, unreachable window), and **the real HTTP endpoint**: 401 without a
  token, 401 with a wrong one, 403 for a non-loopback Host, and a full
  `initialize` → `tools/list` → `tools/call` exchange through the router the app
  serves.
* `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets` and
  `cargo deny check` stay green; `rmcp`, `axum` and `tokio` are desktop-only and
  never enter the wasm graph.

Not verified here (no desktop GUI in this environment) - the desktop checklist:

1. `cargo tauri dev`, open a normal VCD and a >64 MiB FST (native mode).
2. Open the `AI / MCP` tab; confirm the status, URL and token; copy the config.
3. Register the server in an MCP client, then ask it to list the dumps, add two
   signals, show a range, read a value, list changes, and export a CSV; compare
   the rows with the UI.
4. Repeat the reads against the >64 MiB FST and confirm `__waveDebug.memory`
   does not grow with the queries (the reads are streamed).
5. Restart the app: the token must persist; delete `mcp-token` and restart: a
   new token must be generated and the panel must say the config changed.
6. A request without the token, and one with a foreign `Origin`, must be
   refused.

## 8. Risks and limits

* **Client compatibility.** Streamable HTTP with a custom header is required;
  this build creates a session for clients that use the `initialize` handshake
  and serves newer revisions statelessly. A stdio-only client would need a shim
  (not in this wave).
* **Port and instances.** Two app instances take two ports; the panel shows the
  real one, so a stale client config fails to connect rather than reaching the
  wrong window.
* **A second mapping.** The data face maps files on its own (4-file cache); on
  Windows that is one more handle until the cache evicts it. The UI's native
  dump keeps its own mapping.
* **Format costs.** VCD pays its validation pass at open, GHW cannot stream; the
  extraction answer records the format so the cost is attributable.
* **Cursors are `i32`.** A time past `i32::MAX` cannot be a cursor;
  `set_cursors` says so and points at `set_time_range`.
* **Long searches cannot be cancelled.** `search_value` may scan the trace, and
  an extraction stops only at its budget. The service restricts the data work
  that can run at once.
* **Webview reload.** Pending UI commands fail with a timeout; the tool call can
  be retried.

## 9. Non-goals

* An embedded model, key management or token streaming.
* AI in the browser build.
* Writing outside the temp directory, editing RTL, running simulations or
  arbitrary scripts.
* Targeting a specific dump other than the displayed one, or the split pane.
* Row formats, row selection and bit expansion through tools (the store owns
  them; the tools only add/remove whole rows).
* OAuth, remote (non-loopback) access, a stdio transport.
