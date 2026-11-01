# Yew 0.19 -> 0.21 migration plan

Status: **done (Wave 9, 2026-11-02)**. Plan written against the tree at 0.8.0 in
Wave 6; executed at 0.12.0. See [Result](#result-wave-9) at the end for what
actually happened (it was almost exactly this plan).
Before: `yew 0.19.3` (`front-end/Cargo.toml`, `yew = "0.19"`).
Now: **`yew 0.21.0`**, `features = ["csr"]`. 0.22/0.23 are a later, separate step: 0.22 raises the
MSRV to 1.84 and removes `class=(...)`/`ToHtml`/renames `#[function_component]`,
and none of that is needed to get off 0.19. This plan covers exactly the two
upstream guides (0.19->0.20 and 0.20->0.21).

## What this codebase actually uses

- **15 class components** (`impl Component for`) at migration time — the plan
  counted 12; `nav/picker.rs`, `wave_show/signal/signal_name.rs` and the split
  `module_struct/signal_struct/*` added more. `impl Component for` is still the
  only component style.
- **`html!` sites** everywhere, props + `Callback` + `use yew::prelude::*`; one
  `yew::html::Scope` (`app.rs`).
- **5 `changed()` impls** (the plan counted 4; `signal_struct/signal.rs` was
  added in a later wave): `code_reader/code.rs`, `module_struct/module.rs`,
  `module_struct/signal_struct/signal.rs`,
  `module_struct/signal_struct/signal_component.rs`, `wave_show/time.rs`,
  `wave_show/wave.rs`.
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

## Result (Wave 9)

Executed 2026-11-02 against the tree at 0.12.0. **It went almost exactly as
planned**; the plan's risk list was right and the one real trap was a build-flag
detail, not the framework.

### What landed

| File | Change |
|---|---|
| `front-end/Cargo.toml` | `yew = { version = "0.21", features = ["csr"] }` |
| `Cargo.lock` | yew/yew-macro 0.21.0 + the dependency churn below |
| `front-end/src/main.rs` | `yew::Renderer::<App>::new().render()`; **deleted** the crate-level `#![allow(clippy::unnecessary_operation)]` |
| `front-end/src/wave_show/wave.rs`, `wave_show/time.rs`, `module_struct/module.rs`, `module_struct/signal_struct/signal.rs`, `module_struct/signal_struct/signal_component.rs` | `changed(&mut self, ctx, _old_props: &Self::Properties)` |

No other source change was needed: no `html!` fallout, no `Callback`/props/
`NodeRef` change, no store or DOM change. The tier-A render-count gate and the
whole browser matrix stayed green **without touching the app for batching**.

### Verification (commands actually run, in order)

| Command | Result |
|---|---|
| `cargo check -p front-end --target wasm32-unknown-unknown` | clean (first pass after the 5 signatures) |
| `cargo check -p front-end --no-default-features --features tauri --target wasm32-unknown-unknown` | clean (desktop cfg compiles) |
| `cargo test -p front-end --bins` | **179 passed, 0 failed** (the plan said 151; tests were added in Waves 5-8) |
| `cargo clippy -p front-end --all-targets -- -D warnings` | **clean with the `unnecessary_operation` allow removed** |
| `cargo fmt -p front-end -- --check` | clean |
| `cd front-end && trunk build --release` | success — but **12.36 MiB**, because this does not reproduce CI's flags (see below) |
| `CARGO_PROFILE_RELEASE_OPT_LEVEL=s trunk build --no-default-features --features wasm --release` | success, **9,953,799 bytes = 9.49 MiB**, budget 10,485,760 — within budget |
| `node qa/run.mjs --layout` (51 scenarios) | **`=== summary: 414 passed, 0 failed, 0 pending (R2 layout, required), 0 deferred (R2 step 4), 2 retired (canvas rewrite) ===`**, exit 0 |

Size delta against the 9,604,086 bytes the budget was introduced at:
**+349,713 bytes (+3.6%)** for the yew 0.21 runtime + its dependency tree.

### Where this plan was wrong or incomplete

1. **The size command in step 6 is misleading.** `trunk build ... --release`
   alone builds with the default `opt-level = 3` and yields a 12.36 MiB wasm
   that *looks* like a budget regression. CI's `CARGO_PROFILE_RELEASE_OPT_LEVEL: s`
   job env var is what produces the 9.49 MiB number. Anyone checking the budget
   locally must set it (or run the CI step) — this is the one thing that cost
   time and would have caused a false alarm.
2. **`changed()` count**: 5, not 4 — `signal_struct/signal.rs` (and
   `signal_component.rs`) arrived after Wave 6. The compiler names every one, so
   this is self-correcting.
3. **Test count**: 179, not 151.
4. **Assertion count**: the matrix is now 414 passing assertions in 51 scenarios
   (plus 2 retired), not the 324/407 quoted in earlier waves.
5. **No `html!` strictness fallout at all.** The plan budgeted "several sites";
   there were zero. Everything the macro accepted on 0.19 it accepts on 0.21.
6. **The `unnecessary_operation` allow is no longer needed** — it existed for
   0.19's `html!` expansion and was deleted; `-D warnings` stays clean.
7. **Dependency churn is bigger than the plan listed.** `cargo update -p yew
   --precise 0.21.0` also added `gloo 0.8.1` **and** `gloo 0.10.0` (two majors
   coexist), `gloo-file/console/dialogs/events/history/net/render/storage/timers/
   utils/worker` at 0.2/0.3, `implicit-clone`, `prokio`, `pinned`,
   `tokio-stream`, `serde_urlencoded`, `http`, `futures`, `anymap2`,
   `bincode`, `getrandom`, `num_cpus`, `pin-project`, `prettyplease`,
   `ryu`, `tracing-attributes`; and removed `gloo 0.4.2`, `lazy_static`,
   `scoped-tls-hkt`. `Cargo.lock` is shared, so that churn is part of the same
   commit — expect a noisy lock diff.
8. **`cargo update -p yew` needs `--precise 0.21.0`** to land exactly 0.21.0
   (otherwise it resolves the newest 0.21.x). After the lock is updated,
   `--locked` works as the plan assumes.
9. **Message batching: no app fix was needed.** The store design already assumed
   a callback only queues a message (`docs/state-store.md` §2.6), and the
   shell/pane render counters are published from `rendered`, so the tier-A gate
   is unaffected. The plan's "if QA looks like a timing race, check the store
   first" advice was not needed this time.

