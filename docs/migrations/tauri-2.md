# Tauri 1.x -> 2.x migration plan

Status: **done (Wave 7, tree at 0.10.0).** Both phases landed: the Rust shell
and config on tauri 2, the front-end IPC shims on `__TAURI__.core` and
`onDragDropEvent`. What remains is runtime-only verification (window, IPC,
drag-and-drop, bundles) on the tag workflow, because this environment has no
desktop runtime. Written as a plan in Wave 6 against the tree at 0.8.0; the
"Phase 1 result"/"Phase 2 result" sections at the bottom record what actually
landed and where reality differed.
Was: `tauri 1.8.3` / `tauri-build 1.5.6`, `tauri.conf.json` in the v1 schema
(`"tauri"` key, `allowlist`, `distDir`).
Now: `tauri 2.12.5` / `tauri-build 2.7.4` (`tauri-utils 2.10.2`, `wry 0.57.1`,
`tao 0.37.2`), v2 config schema and `src-tauri/capabilities/default.json`.

Why this is worth doing even though the app works today:

- v1 is maintenance-only, and it is the reason for two CI/workflow compromises:
  the `ubuntu-22.04` pin (v1 links **webkit2gtk-4.0**, which Ubuntu 24.04 no
  longer ships) and the three `deny.toml` advisory ignores that all come from
  the tauri-1.x dependency stack (gtk3-macros -> proc-macro-error, tao -> instant,
  kuchikiki -> fxhash).
- v2 uses webkit2gtk-4.1, so the `ubuntu-latest` (24.04) runner becomes usable.

## What this app actually uses

`src-tauri/src/main.rs`: `tauri::Builder` + `generate_handler!` + `generate_context!`
(`:138-144`), the `windows_subsystem` attribute (`:1-4`), and **5 custom commands**
that are plain `std::fs` reads (`get_file_list` `:69`, `choose_vcd` `:104`,
`choose_vcd_absolute` `:111`, `choose_verilog` `:117`, `choose_verilog_absolute` `:124`).
No `tauri::api::*`, no plugins, no tray/menu/updater, no Rust-emitted events.

Front end (the only code that talks to Tauri): `front-end/src/file_load/filelist.rs:13-18`
(raw `wasm_bindgen` externs for `window.__TAURI__.tauri.invoke` and
`window.__TAURI__.event.listen`); `front-end/src/app.rs:207-230` (`#[cfg(feature = "tauri")]`
listener for `"tauri://file-drop"` decoding the v1 envelope
`{event, windowLabel, payload: Vec<String>, id}`); `file_load/file.rs:38,203-205`
and `app.rs:541` are the other tauri blocks and only call the same commands.

## Config mapping (v1 -> v2)

From the upstream `from-tauri-1` guide (`tauri-docs`, v2 branch), against
`src-tauri/tauri.conf.json`:

| v1 (current) | v2 |
|---|---|
| `package.productName` / `package.version` (`:9-12`) | top-level `productName` / `version` |
| (binary named after the package) | top-level `mainBinaryName` — set it to `"app"` so `target/release/app(.exe)` keeps working for `tauri.yml` staging |
| `tauri` key (`:13`) | `app` |
| `tauri.allowlist` (`:14-16`) | removed; replaced by `src-tauri/capabilities/*.json` |
| `build.distDir` (`:6`) | `build.frontendDist` |
| `build.devPath` (`:5`) | `build.devUrl` |
| `build.withGlobalTauri` (`:7`) | `app.withGlobalTauri` (we keep it: the front end uses the global API) |
| `tauri.bundle` (`:17-49`) | top-level `bundle` |
| `tauri.bundle.identifier` (`:32`) | top-level `identifier` |
| `tauri.bundle.deb` (`:21-23`) | `bundle.linux.deb` (drop `depends: []`) |
| `tauri.bundle.macOS` / `.windows` (`:34-40`, `:44-48`) | `bundle.macOS` / `bundle.windows` (same keys) |
| `tauri.security.csp` (`:51`) | `app.security.csp` (still `null` today; a real CSP is a separate hardening item) |
| `tauri.updater` (`:53-55`) | removed; if we ever enable updates: `plugins.updater` + `bundle.createUpdaterArtifacts` |
| `tauri.windows` (`:56-65`) | `app.windows`; add `"label": "main"` (capabilities reference it) |
| `build.beforeBuildCommand` / `beforeDevCommand` (`:3-4`) | unchanged |

New file `src-tauri/capabilities/default.json`:

```json
{
  "$schema": "../gen/schemas/desktop-schema.json",
  "identifier": "default",
  "description": "main window: core IPC + events, no plugin APIs",
  "windows": ["main"],
  "permissions": ["core:default"]
}
```

The five custom commands are app commands, not plugin commands, so they need no
capability entry; `core:default` is what the webview needs for IPC/event core.

## Breaking changes that touch this code

1. **JS module rename** `@tauri-apps/api/tauri` -> `@tauri-apps/api/core`: the global
   becomes `window.__TAURI__.core.invoke`, so `filelist.rs:14` must change its
   `js_namespace` from `["window", "__TAURI__", "tauri"]` to `["window", "__TAURI__", "core"]`.
   The `event` module keeps its name (`filelist.rs:16`), but its semantics changed.
2. **`tauri://file-drop` is gone.** v2 replaced the drag-drop event with the
   drag-drop event family exposed as `onDragDropEvent` on the webview window
   (`window.__TAURI__.webviewWindow.getCurrentWebviewWindow().onDragDropEvent(cb)`),
   whose payload is `{type: "enter"|"over"|"drop"|"leave", paths?, position?}`.
   The whole `TauriEvent { event, windowLabel, payload, id }` decode at
   `app.rs:209-230` must be replaced; the app only needs `payload.type === "drop"`
   -> `payload.paths` -> the existing `Msg::FileDrop` callback. This is the one
   real rewrite in the desktop channel.
3. **Window label**: v2 capabilities address windows by label; the single window
   gets `"label": "main"` in `app.windows` and in the capability file.
4. **`mainBinaryName`**: without it v2 does not rename the binary to
   `productName`, and the `tauri.yml` "Stage the plain binary" step reads
   `target/release/app(.exe)`. Setting `mainBinaryName: "app"` keeps that step,
   the asset names and `readme` unchanged.
5. **Rust API**: `Builder`, `#[tauri::command]`, `generate_handler!`,
   `generate_context!`, `windows_subsystem` and `std::fs` all stay. No plugin
   migration is needed (we use none). The `custom-protocol` feature stays in
   `src-tauri/Cargo.toml:27-30`; it is not in the v2 "removed features" list.
6. **MSRV**: v2 needs Rust >= 1.77 (project pins 1.98.1). `tauri-action@v0`
   supports v2 builds (it already builds our v1 releases), so `tauri.yml` keeps
   its shape.

## Steps

1. `src-tauri/Cargo.toml`: `tauri = "2"`, `tauri-build = "2"`; `cargo update`
   (expect wry/tao/webkit2gtk and their GTK stack to move).
2. `src-tauri/tauri.conf.json`: apply the mapping table above (the official
   `cargo tauri migrate` CLI can generate a first draft; review it by hand —
   it rewrites the whole file and we keep comments/metadata by hand today).
3. Add `src-tauri/capabilities/default.json` and the window label.
4. `front-end/src/file_load/filelist.rs:14`: `tauri` -> `core` namespace.
5. `front-end/src/app.rs:207-230`: replace the `listen("tauri://file-drop")`
   block with `onDragDropEvent` (see above), keeping `Msg::FileDrop`.
6. `cargo check -p app` (host) and `cargo check -p front-end --no-default-features
   --features tauri --target wasm32-unknown-unknown` (the wasm side of the
   desktop cfg).
7. `cd front-end && trunk build --no-default-features --features tauri --release`,
   then `cargo build --release -p app` and run the binary by hand: file dialog
   (`get_file_list`/`choose_vcd`), source open (`choose_verilog`), drag a
   `.vcd` onto the window (the rewritten event), launch with a path argument.
8. CI: `tauri.yml:39` and `tauri_pre.yml:33` apt line
   `libwebkit2gtk-4.0-dev` -> `libwebkit2gtk-4.1-dev`; move `ubuntu-22.04` ->
   `ubuntu-latest`/`ubuntu-24.04` in `tauri.yml:27`, `tauri_pre.yml:23` **and the
   `clippy` job in `rust.yml:103`** (it compiles `-p app` and has the same apt
   line), and delete the "pinned for webkit2gtk-4.0" comments.
9. `deny.toml`: re-run `cargo deny check` and remove the ignores that disappear
   (RUSTSEC-2024-0384 instant, RUSTSEC-2025-0057 fxhash are tauri-1.x-only;
   RUSTSEC-2024-0370 proc-macro-error depends on whether the new glib-macros
   still uses it — delete only what the run proves is gone).
10. Release: tag `v0.9.0` (minor, not patch — the desktop bundle changes shape),
    check the `tauri.yml` matrix and the plain-binary asset names.

## Files affected

`src-tauri/Cargo.toml`, `src-tauri/tauri.conf.json`, new
`src-tauri/capabilities/default.json`, `front-end/src/file_load/filelist.rs`,
`front-end/src/app.rs`, the three workflows (webkit 4.1 + 24.04), `deny.toml`,
`Cargo.lock`.

## Verification strategy

- `cargo check -p app --locked` and the wasm tauri-feature check above (fast loop).
- `cargo deny check` must go green **without** the tauri-1.x ignores (this is the
  measurable proof the GTK3 advisories left the tree).
- Browser QA matrix (356 assertions) proves the *browser* channel is untouched;
  it cannot prove the desktop channel, so the manual checklist in step 7 plus the
  `tauri.yml` tag build are the desktop gate.
- Rollback: the migration is one commit per step; reverting steps 1-5 restores
  v1 (config + two front-end blocks + two Cargo lines). Nothing in the wasm
  build or the QA contract depends on it.

## Risks

- **Config schema mistakes** are the main trap (silent key fallbacks): compare
  with the v2 reference and let `cargo tauri build` validate the schema.
- **Drag-drop rewrite**: wrong payload shape degrades to "dialog still opens
  files", so the failure is contained.
- **Windows origin**: v2 serves from `http://tauri.localhost` (v1: https), so the
  localStorage-backed layout/settings reset once unless `useHttpsScheme` is set.
- **Not verifiable here**: no desktop runtime/bundle build in this environment;
  the `tauri.yml` tag build on the real runners is the final gate.

## Phase 1 result (landed, Wave 7)

Versions actually resolved (Cargo.lock): `tauri 2.12.5`, `tauri-build 2.7.4`,
`tauri-utils 2.10.2`, `wry 0.57.1`, `tao 0.37.2`; `tauri` keeps the
`custom-protocol` feature (it still exists in v2, confirmed against the v2
crate's `Cargo.toml`).

Files changed in phase 1:

- `src-tauri/Cargo.toml` — `tauri-build = "2"`, `tauri = "2"`; the
  `custom-protocol` feature stays (v2 still has it).
- `src-tauri/tauri.conf.json` — v2 schema, written from the schema at
  `https://schema.tauri.app/config/2` and validated by `tauri-build`:
  top-level `productName`/`version`/`identifier`/`mainBinaryName: "app"`,
  `build.frontendDist`/`devUrl`, `app.windows` (label `main`),
  `app.withGlobalTauri`, `app.security.csp`, top-level `bundle` with
  `bundle.linux.deb` (the old `deb`), `bundle.macOS`/`bundle.windows`; the
  inactive `updater` block and the v1 `allowlist` are gone.
- `src-tauri/capabilities/default.json` (new) — `windows: ["main"]`,
  `permissions: ["core:default"]`. The five app commands are not ACL-gated;
  tauri-build compiled this file and generated `gen/schemas/capabilities.json`
  with it, which is the compile-time proof it parses.
- `src-tauri/.gitignore` — `/gen/` (tauri-build v2 writes `gen/schemas/*`).
- `.github/workflows/{tauri,tauri_pre,rust}.yml` — `ubuntu-22.04` ->
  `ubuntu-24.04`, `libwebkit2gtk-4.0-dev` -> `libwebkit2gtk-4.1-dev`, plus
  `libxdo-dev`/`libssl-dev` from the v2 prerequisite list; the `rust.yml`
  clippy job (it builds `-p app`) moved with them.
- `deny.toml` — RUSTSEC-2024-0384 (instant) and RUSTSEC-2025-0057 (fxhash) are
  gone from the tree and their ignores were removed; RUSTSEC-2024-0370
  (proc-macro-error) **remains** because tauri 2 still links GTK3 on Linux,
  so `glib-macros/gtk3-macros` is still in the build tree.
- `Cargo.lock` — the tauri-1.x stack replaced by the v2 stack (~1300 lines).

Verified here (Windows, no desktop runtime):

- `cargo check -p app --locked` -> exit 0 (tauri-build parsed the v2 config and
  the capability, generated `gen/schemas/`, and the shell compiled).
- `cargo deny check` -> advisories ok, bans ok, licenses ok, sources ok; the
  remaining duplicate-version warnings are informational (bans is `warn`).
- `src-tauri/gen/schemas/capabilities.json` contains the `default` capability,
  so the ACL file is valid.

Not verified here (needs a desktop runtime / real runners): the window opens
and the webview loads the bundled frontend; the five `#[tauri::command]`s
answer through v2 IPC; drag-and-drop (phase 2); `cargo tauri build` bundles
(deb/rpm/AppImage/dmg/msi) and the `tauri.yml` tag workflow, including
`beforeBuildCommand`'s working directory and the tauri-action CLI version
selection for v2.

## Phase 2 result (landed, Wave 7)

Front-end tauri-gated code only (the browser build and the QA contract are
untouched; `trunk build --features wasm` and the 171 front-end tests stay
green):

- `front-end/src/file_load/filelist.rs` — the IPC shim now declares
  `js_namespace = ["window", "__TAURI__", "core"]` for `invoke` (v2 renamed
  the JS module from `tauri` to `core`), and the v1 `listen` extern was
  replaced by `current_webview()`:
  `js_namespace = ["window", "__TAURI__", "webview"], js_name = getCurrentWebview`.
- `front-end/src/file_load/mod.rs` — exports `current_webview` instead of
  `listen`.
- `front-end/src/app.rs` (the `#[cfg(feature = "tauri")]` block in
  `App::create`) — the removed `tauri://file-drop` listener is gone. It now
  calls `current_webview()` and registers `onDragDropEvent` on it via
  `Reflect` (the v2 webview method), reads `event.payload.type` and only on
  `"drop"` maps `event.payload.paths` (absolute paths) into the existing
  `Msg::FileDrop(Vec<String>)` path. enter/over/leave are ignored, so the
  rest of the file-drop handling is unchanged.

Verified in phase 2:

- `cargo check -p front-end --no-default-features --features tauri
  --target wasm32-unknown-unknown --locked` -> exit 0.
- `cargo check -p front-end --target wasm32-unknown-unknown --locked`
  (default/browser features) -> exit 0.
- `cargo check -p app --locked` -> exit 0.
- `cargo test -p front-end --bins --locked` -> 171 passed, 0 failed.
- `trunk build --no-default-features --features tauri --release` -> exit 0
  (93 s); wasm-bindgen processed the bundle, which validates the new
  `js_namespace`/`js_name` bindings. The dist was then rebuilt with the wasm
  feature so the tree is back to the browser bundle.

Still runtime-only (cannot be checked here): the window opens and loads the
frontend, the five commands answer over v2 IPC, a real drag-and-drop delivers
absolute paths through `onDragDropEvent`, and `cargo tauri build` produces
the deb/rpm/AppImage/dmg/msi bundles on the tag workflow.