# Yew 0.19 -> 0.21 migration plan

Status: **planned (Wave 6)**. Written against the tree at version 0.8.0, 2026-11-02.
Current: `yew 0.19.3` (`front-end/Cargo.toml:9`, `yew = "0.19"`).
Target: **`yew 0.21.0`**. 0.22/0.23 are a later, separate step: 0.22 raises the
MSRV to 1.84 and removes `class=(...)`/`ToHtml`/renames `#[function_component]`,
and none of that is needed to get off 0.19. This plan covers exactly the two
upstream guides (0.19->0.20 and 0.20->0.21).

## What this codebase actually uses

- **12 class components** (`impl Component for`): `front-end/src/app.rs:201`,
  `code_reader/code.rs:41`, `code_reader/code_block.rs:47`, `file_load/file.rs:266`,
  `file_load/filelist.rs:106`, `module_struct/module.rs:89`, `nav/inspector.rs:34`,
  `nav/picker.rs:32`, `top_bar/bar.rs:29`, `wave_show/ctrl.rs:126`,
  `wave_show/time.rs:92`, `wave_show/wave.rs:234`.
- **74 `html!` sites**, props + `Callback` + `use yew::prelude::*`; one
  `yew::html::Scope` (`app.rs`).
- **4 `changed()` impls**: `code.rs:102`, `module.rs:110`, `time.rs:219`,
  `wave.rs:315`.
- **0 hooks, 0 function components, 0 `yew::services`/agents.**
- **0 component `ref=` props.** The NodeRefs are DOM refs (`app.rs:356,380`,
  `code_block.rs:117`, `module.rs:243`); `NodeRef` passed as a *property*
  (`code_block.rs:31`, `host: NodeRef`) is unaffected.
- Entry point: `yew::start_app::<App>()` (`front-end/src/main.rs:20`).

## Breaking changes that touch this code

From the upstream guides (yew repo, `website/migration-guides/yew/`):

1. **`start_app*` removed, `yew::Renderer` added (0.20).** `Renderer` needs the
   `csr` feature, and yew 0.21's features are `default = []`, `csr = []`,
   `hydration = ["csr", ...]` — so `csr` must be enabled explicitly or nothing
   renders. The `*_as_body` variants are gone (not used here).
2. **`Component::changed` gained `old_props` (0.20)**: the signature is now
   `fn changed(&mut self, ctx: &Context<Self>, old_props: &Self::Properties) -> bool`.
   Exactly the four impls above; bodies keep reading `ctx.props()` for the new
   value (they compare against fields today).
3. **Automatic message batching (0.20).** Messages to *different* components are
   no longer ordered relative to each other; per-component FIFO is preserved and
   one render per batch. This is the risky change for this app: the whole tree
   is a `Store` + `Callback` design (`docs/state-store.md` §2.6 already assumes a
   callback only *queues* a message), but the browser QA matrix asserts DOM state
   immediately after clicks/keys, so scenario timing is the real acceptance test.
4. **`ref` prop on components removed (0.20).** No usage (see above), no change.
5. **Hooks/function-component API rework (0.20) and hook-dependency argument
   order (0.21).** No hooks here, no change.
6. `yew::start_app`'s automatic panic hook stays via `Renderer`; no separate
   `console_error_panic_hook` wiring exists in this crate.

Dependency side (verified against the current lockfile): `wasm-bindgen 0.2.130`,
`web-sys/js-sys 0.3.107` — both new enough for yew 0.21. yew 0.21 pulls
`gloo 0.10`, `indexmap 2`, `prokio` (lockfile-only churn; the front end's own
`gloo-file 0.2` is a separate crate and unaffected). trunk 0.22.1 downloads the
wasm-bindgen CLI matching the lockfile, which CI already relies on.

## Steps (one commit each, 0.19 stays green until step 2 lands)

1. **Cargo**: `front-end/Cargo.toml:9` -> `yew = { version = "0.21", features = ["csr"] }`;
   `cargo update -p yew` (lockfile only). `cargo metadata --no-deps` sanity check.
2. **Entry point**: `front-end/src/main.rs:20` ->
   `yew::Renderer::<App>::new().render();`. While there, re-check the crate-level
   `#![allow(clippy::unnecessary_operation)]` (`main.rs:4`): it exists because
   0.19's `html!` expansion trips the lint; if 0.21 no longer does, delete it
   (otherwise keep it and the comment).
3. **`changed()` x4**: add `_old_props: &Self::Properties` to
   `code.rs:102`, `module.rs:110`, `time.rs:219`, `wave.rs:315`.
4. **Compile**: `cargo check -p front-end --target wasm32-unknown-unknown --locked`
   (this is the fast loop; expect `html!` prop-type fallout, not architecture).
5. **Host tests**: `cargo test -p front-end --bins --locked` — 151 tests today
   (`state/reduce.rs`, `state/store.rs`, `wave_show/geometry.rs`,
   `signal_things.rs`, `nav/*`). Batching must not move any of them.
6. **Bundle**: `cd front-end && trunk build --no-default-features --features wasm --release`;
   check the wasm size budget (`rust.yml` threshold 10 MiB, current build ~9.6 MB).
7. **Browser QA**: `node qa/run.mjs --scenarios empty,vcd,layout,layoutRules` first,
   then the full 324-assertion matrix. The batching-sensitive scenarios are
   `tierA`, `layout`, `layoutRules`, `clickToAdd`, `tabClose`, `deepWave`,
   `loadDrive`, `inlayHints` — a green matrix is the only proof the store-driven
   panes still see messages in the order they assume.
8. **Gates**: `cargo clippy --workspace --all-targets --locked -- -D warnings`,
   `cargo fmt --all -- --check`, `cargo deny check`.

## Files affected

| File | Change |
|---|---|
| `front-end/Cargo.toml` | yew version + `csr` feature |
| `Cargo.lock` | yew/gloo/indexmap/prokio bump |
| `front-end/src/main.rs` | `Renderer` |
| `front-end/src/code_reader/code.rs` | `changed()` signature |
| `front-end/src/module_struct/module.rs` | `changed()` signature |
| `front-end/src/wave_show/time.rs` | `changed()` signature |
| `front-end/src/wave_show/wave.rs` | `changed()` signature |
| (possible) several `html!` sites | macro strictness fixes |

## Verification strategy

- Host: `cargo test -p front-end --bins` (151 tests) — the reducer/store tests are
  the regression oracle for the batching change.
- Compile: `cargo check -p front-end --target wasm32-unknown-unknown` and
  `cargo check -p front-end --no-default-features --features tauri --target wasm32-unknown-unknown`
  (the tauri cfg of the front end compiles with the same yew).
- Browser: the full QA matrix (324 assertions) plus `--offline` (`node qa/run.mjs --offline`)
  because the editor glue and the event handlers are exercised there.
- Desktop: `trunk build --features tauri` locally; the real proof is the
  `tauri.yml` tag build (same yew code in both channels).

## Risks and rollback

- **Message batching** is the one behavioural risk: panes that used to observe a
  sibling's render in the same tick may now see it one tick later. If QA goes red
  in a way that looks like a timing race, check `Store::dispatch`/`subscribe`
  (`front-end/src/state/store.rs`) and the `Msg::Action` drain in `app.rs` before
  touching any component.
- **`html!` strictness**: 0.20/0.21 reject some prop expressions 0.19 accepted;
  these are mechanical fixes at the reported lines.
- **MSRV**: yew 0.21 needs 1.64; the project pins 1.98.1, no change.
- **Rollback**: revert `main.rs` + the four `changed()` signatures +
  `Cargo.toml`/`Cargo.lock` (a single commit); no data formats, no DOM contract,
  no store semantics are involved, so the QA matrix keeps passing on the old tree.
