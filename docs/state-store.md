# R2 — one state store

Design for the R2 step of [`rebuild-plan.md`](rebuild-plan.md): replace the
props-and-`Rc<RefCell<_>>` plumbing with one store, and use the same move to fix
the pane layout.

Status: **proposal, no code yet.** R1 (canvas renderer) and the self-hosted
editor are in; R3 is deleting `data_struct::Module`/`ModuleValue` in the
library, `file_load/`, `nav/` and `wave_show/` — so this document deliberately
touches only `app.rs`, the component boundaries and a new `state/` module.

---

## 1. Why

`App` today is the whole application: 21 fields and 21 message variants (a few
`#[cfg]`-gated), and every other component is a prop-forwarding shell around it.

| Where state lives now | What it is | Who else keeps a copy |
|---|---|---|
| `App.waveform: Option<SharedWaveform>` | opened dump handle | `WaveShow.waveform` |
| `App.waveform_module` / `design_module` / `module: Rc<Module>` | sidebar tree, rebuilt by hand | `ModuleStruct.root`, `ModuleStruct.show_module`, `ModuleComponent.space/name` |
| `App.design: SharedDesign` (`nav/`) | `DesignDb` + elaboration behind `Rc<RefCell<_>>` | `LspBridge`, inspector |
| `App.verilog_source: Vec<(String, String)>` | open documents + their text | `LspBridge.documents` (its own `HashMap` + version), monaco models |
| `App.signal_add` + `signal_add_seq` | "the user added a row" as a tuple plus a token | `WaveShow.added_seq` |
| `App.selected` | selected signal for the inspector | `WaveShow.signal_things[*].choose` |
| `App.goto` + `goto_token` | jump request | `CodeReader.shown_token`, `CodeBlock.shown_token`, `LspBridge::jump` |
| `App.sidebar_width` + `splitter_start` | layout | — |
| `App.status: Option<String>` | status line | — |
| `wave_show::WaveShow` | rows (`SignalThings`), viewport (`View`), cursors, menu, drag, scroll | `Time` component keeps its own copy of `x_axis/size/cursors` |
| `App.signal_value: Rc<RefCell<ModuleValue>>` | pre-expanded values for the tauri/server transports | `WaveShow.signal_value`, `SignalItem.values` |

Most of those copies have already produced a bug:

- `ModuleComponent`/`ModuleStruct` copies → *the tree could not be expanded* and
  *the label went stale when another dump was opened* (`1572244`, fixed by
  reading props, but the copies are still there);
- `signal_add` + `signal_add_seq` → *deleting a row resurrected it on the next
  re-render*;
- `LspBridge.documents` vs `verilog_source` → closing a tab has to remember to
  call `lsp.close()` by hand, and a re-read of a file has to remember to call
  `lsp.open()`; miss one and hover answers about text that is not on screen;
- `Time`'s copy of `x_axis/size` → the time bar and the canvas can disagree for
  one frame after a zoom.

The second problem is re-render scope. `App::view` builds the sidebar, the
editor, the inspector and the wave pane on every `Msg`. `Msg::SplitterMove`
fires on every mouse move, so dragging the sidebar splitter re-renders the whole
tree twice per frame; the drag-to-zoom preview does the same through
`WaveShow`. That is why both feel heavy on a large dump even after R1.

R2's goals, in order:

1. one place that owns state, one typed way to change it;
2. a component re-renders only when the slice it reads changes — and pointer
   movement re-renders nothing at all;
3. no component keeps a copy of shared state (also the acceptance criterion in
   task-10);
4. make the *next* feature cheap: split view, multiple dumps, persistence.

---

## 2. The store

New module `front-end/src/state/`:

```
state/
  mod.rs        re-exports
  state.rs      State + the slice structs (plain data, no behaviour)
  action.rs     Action (what can happen) and Effect (what the outside must do)
  reduce.rs     fn reduce(&mut State, Action) -> Vec<Effect>   (pure, unit-tested natively)
  store.rs      Store, Subscription, dispatch loop, select/subscribe
  select.rs     the named selectors components use
  types.rs      Goto, SourceClick, Picker, Status — pure data moved out of nav/
```

`state/` may not import `wave_show/`, `code_reader/` or `module_struct/`: the
store is UI-agnostic and natively testable. `app.rs` and the pane components
import `state/`; `nav/` keeps its components and re-exports the data types it
used to own.

### 2.1 State: only the source of truth

```rust
pub struct State {
    pub sources: Sources,      // open documents + buffer text + active tab
    pub design: Design,        // DesignDb, elaboration cache, top module
    pub dump: Dump,            // Option<Rc<Waveform>>, hierarchy meta, end_time
    pub rows: Rows,            // the wave rows, their order and per-row settings
    pub view: Viewport,        // committed zoom/pan + cursors
    pub layout: Layout,        // sidebar width, editor ratio, drawer
    pub focus: Focus,          // signal selected in the wave view, instance in the sidebar
    pub nav: Navigation,       // pending goto, picker candidates, status line
    pub lsp: LspHandle,        // the client handle (see 2.4)
    #[cfg(feature = "backend")] pub backend: Backend, // legacy transports, deleted in R3
}

pub struct Rows {
    pub items: Rc<Vec<Row>>,   // versioned slice, see 2.2
    pub version: u64,
}
pub struct Row {
    pub path: String,          // hierarchical path: identity, value key and display name
    pub width: usize,          // dump width when present, else declared
    pub is_bool: bool,
    pub index: Option<usize>,  // resolved signal index in the dump (cache, kept for the painter)
    pub setting: Settings,     // ShowType, colour, fixed/analog (moved from wave_show/settings.rs)
    pub chosen: bool,          // multi-selection in the wave list
}
```

Notes that matter:

- **Row identity is `path`.** `SignalItem.signal: Rc<Signal>` disappears with
  R3; the painter only needs `path` (to resolve `index` and to format) plus
  `width/is_bool/setting`. `module_tree()`/`legacy_module()` go away with R3,
  so the sidebar's tree is derived data (2.3), never stored.
- **The dump is a handle, not data.** `State.dump.waveform: Option<Rc<Waveform>>`
  is the one copy; decoded changes live in the library's cache (R3's
  `DecodedSignal`), never in `State`.
- **`sources` is the only copy of the text.** The LSP's document table and the
  monaco models are mirrors, updated by effects (2.5).
- **`view` is committed state** (the time bar draws from it), but a *drag* is
  not (4.1).

### 2.2 Slices and how a component subscribes

Deterministic-ish subscription: a selector plus `PartialEq` plus a callback,
the only approach that works in Yew 0.19 class components without hooks.

```rust
// state/store.rs
pub struct Store(Rc<Inner>);
impl Clone for Store { /* one Rc bump */ }
impl PartialEq for Store { fn eq(&self, other: &Self) -> bool { Rc::ptr_eq(&self.0, &other.0) } }
// so `Store` can be a plain Yew prop: it changes identity only when the page
// is recreated, exactly like the `LspBridge` handle does today.

struct Inner {
    state: RefCell<State>,
    subs: RefCell<Vec<Subscriber>>,
    queue: RefCell<VecDeque<Action>>,
    draining: Cell<bool>,
    effects: RefCell<Vec<Effect>>,
    next_sub: Cell<u64>,
}

impl Store {
    /// Read one slice without subscribing (handlers, `create`).
    pub fn select<S>(&self, select: impl Fn(&State) -> S) -> S;

    /// `callback` receives the selected value, and only when it changed.
    /// `S` must be `Clone + PartialEq + 'static` (see the `Slice` helper).
    pub fn subscribe<S>(&self, select: impl Fn(&State) -> S + 'static, callback: Callback<S>)
        -> Subscription;                    // Drop unsubscribes

    pub fn dispatch(&self, action: Action); // queued, see 2.6
    pub fn take_effects(&self) -> Vec<Effect>;

    pub fn version(&self) -> u64;
}
```

```rust
#[derive(Clone, Debug)]
pub struct Slice<T> { pub version: u64, pub data: Rc<T> }
impl<T> PartialEq for Slice<T> {
    fn eq(&self, other: &Self) -> bool { self.version == other.version }   // O(1)
}
```

Big slices (`rows`, `sources`, `tree`) are handed out as `Slice<T>`: cloning is
one `Rc` bump and comparing is one integer. Small slices (`Layout`, `Viewport`,
`Focus`, `Option<Status>`) are plain `Copy` structs compared by value.

Component boilerplate:

```rust
pub struct Sidebar { tree: Slice<ModuleTree>, focus: Focus, _sub: Subscription }

fn create(ctx: &Context<Self>) -> Self {
    let store = ctx.props().store.clone();
    let sub = store.subscribe(select::tree, ctx.link().callback(Msg::Tree));
    Self { tree: store.select(select::tree), focus: store.select(select::focus), _sub: sub }
}
fn update(&mut self, ctx: &Context<Self>, msg: Msg) -> bool {
    match msg {
        Msg::Tree(tree) => { self.tree = tree; true }      // re-render, one slice
        Msg::Focus(focus) => { self.focus = focus; true }
        Msg::Redraw => { self.draw(); false }              // no virtual DOM work at all
    }
}
```

`Msg::Redraw` is the important trick: a subscription whose callback returns
`false` from `update` lets a component react to state (a canvas repaint, a
scroll offset, a focus ring) **without diffing any nodes**.

What a component subscribes to is its own business, and that is the answer to
"how do I avoid re-rendering on unrelated changes": nothing is passed by
propagation, so an unrelated dispatch simply never reaches it.

| Component | Subscribes to | Re-renders on |
|---|---|---|
| `App` | `layout` | sidebar/editor ratio, drawer |
| `TopBar` | `nav.status` | status line |
| `Sidebar` (`ModuleStruct`) | `tree`, `focus.instance` | dump/design change, instance click |
| `SignalList` (today `SignalStruct`) | `tree`, `sources` | module selection, filter list |
| `CodeReader` | `sources`, `nav.goto` | open/close tab, jump |
| `CodeBlock` | `nav.goto` (+ `sources` version for a re-read) | jump, file reload |
| `Inspector` | `focus.signal`, `design.version` | selection, new source parsed |
| `PathPicker` | `nav.picker` | candidates appear/disappear |
| `WaveShow` shell | `rows`, `dump`, `focus.signal` | row added/removed/reordered/selected |
| `WaveSurface` (canvas) | `rows`, `dump` → `Msg::Redraw`; reads `view` via `select` | structural changes only |
| `TimeBar` | `view`, `view.cursors` | zoom/pan/cursor |
| `Ctrl` (settings dialog) | `nav.dialog` | dialog opened/closed/edited |

`App` subscribes to **only `layout`**. Everything else is a leaf subscription,
so a zoom no longer walks the sidebar or the editor.

**Debug seams for the render counts.** `App` owns a `Cell<u32>` that is bumped at
the end of every `update` that returns `true`, and `rendered()` copies it into
`__waveDebug.render_count` (via the page-level `__waveRenderCount` that
`canvas::publish_debug` carries over on every redraw). It measures *shell*
re-renders only: a leaf that re-renders on its own subscription does not bump it,
and reading it never triggers a render. The splitter assertion is written against
exactly this ("how many times the shell re-rendered"), so do not turn it into a
per-component counter — that would make the assertion meaningless.

`WaveShow` has the same pair for its own renders (`pane_render_count`, carried
from `__wavePaneRenderCount`). It exists to prove the tier-A rule for gestures
that never touch the shell: during a cursor drag or a range-zoom the pane count
must not move at all (the canvas repaints from its own frame), and on mouseup the
commit is one dispatch. Two counters, two questions — shell re-renders and pane
re-renders — and neither replaces the other.

### 2.3 Derived vs stored

Stored (source of truth, survives everything):

| Slice | Contents |
|---|---|
| `sources` | file name, buffer text, language id, active tab, version |
| `design` | `DesignDb` (only copy), `top` module name, `db_version` |
| `dump` | `Rc<Waveform>`, hierarchy meta (`format`, `end_time`), identity |
| `rows` | ordered `Vec<Row>`: path, width, is_bool, settings, chosen |
| `view` | committed `View { x_axis, size }` + `cursor1/cursor2` |
| `layout` | `sidebar_w`, `editor_ratio`, `drawer_open` |
| `focus` | `signal: Option<String>` (inspector), `instance: Option<String>` (sidebar) |
| `nav` | `goto: Option<Goto>`, `picker`, `dialog: Option<Dialog>` (settings target), `status` |

Derived (recomputed or cached with a version key, never serialized):

| Derived value | From | Where |
|---|---|---|
| elaborated design | `design.db_version`, `design.top` | `State.design.elaborated: Option<ElaboratedDesign>`, rebuilt by `reduce` when either input changes; `select` only reads it |
| module tree (dump hierarchy, else design instances) | `dump`, `design` | `State.tree: Slice<ModuleTree>`, rebuilt by `reduce` on the same edge; `select::tree` only reads it |
| signal details (declaration/drivers/loads) | `design`, `focus.signal` | queried in `Inspector::view`; cheap, no cache |
| row → dump signal index/width | `dump`, `row.path` | stored on `Row` as a convenience, recomputed on `Action::DumpOpened` |
| decoded values | `dump` | **the library cache**, not the store |
| monaco models, LSP document table | `sources` | mirrored by effects (2.5) |
| editor collapsed? | `sources.is_empty()` | **derived**, see §3 |
| inspector visible? | `design.is_empty() && focus.signal.is_none()` | derived |
| `end_clock` | `dump.end_time()` | derived (R3 deletes the legacy `Module.end_clock`) |

Rule of thumb: if the user can undo it, or another pane must agree with it, it
is stored; if two slices determine it, derive it. Derived values that need more
than a field lookup are **computed in `reduce` when their inputs change** and
stored as a versioned cache, not recomputed in `select`: `select` takes
`&State`, so a selector that would have to build a tree or elaborate a design
belongs in the reducer (or in a `build_*` helper it calls), and the selector
stays a cheap read.

### 2.4 Where the LSP and the design live

`LspBridge` today owns three things: the server handle, the document table, and
the design fallback. After R2:

- the **server handle** stays in the bridge (`state.lsp.handle`);
- the **document table moves into the store** as `sources` (name + text +
  version); `didOpen/didChange/didClose` are *effects* emitted by the reducer.
  The bridge keeps only `uri(file)` mapping and a request/answer path; it no
  longer decides when a document exists;
- the **fallback** reads `State.design` instead of holding a `SharedDesign`.

`SharedDesign` and `SharedWaveform` (identity-comparing `Rc` wrappers) are
deleted: `Store` is the identity and it compares by pointer, and `State` hands
out `Rc<Waveform>` directly.

### 2.5 Actions, purity and effects

```rust
pub enum Action {
    SourceOpened { name: String, text: String },
    SourceReloaded { name: String, text: String },
    SourceClosed { name: String },
    TabActivated { index: usize },
    DumpOpened { waveform: Rc<Waveform> },
    DumpFailed { message: String },
    RowAdded { path: String },
    RowsRemoved { indices: Vec<usize> },
    RowSelected { index: usize, ctrl: bool, shift: bool },
    RowsReordered { from: usize, to: usize },
    RowSettingChanged { indices: Vec<usize>, setting: Settings },
    ViewCommitted { view: View },
    CursorsChanged { cursor1: Option<i32>, cursor2: Option<i32> },
    ScrollCommitted { scroll_top: f64 },
    LayoutChanged { layout: Layout },
    FocusChanged { focus: Focus },
    JumpRequested { file: String, line: u32, col: u32 },
    JumpHandled { token: u32 },
    PickerOpened { title: String, paths: Vec<String> },
    PickerChosen { index: usize },
    PickerCancelled,
    Status(Status),
    TopChanged { top: Option<String> },
}

pub enum Effect {
    DecodeRows { paths: Vec<String> },     // drop the painter's stale cache after a dump switch
    LspOpen { name: String, text: String },
    LspChange { name: String, text: String },
    LspClose { name: String },
    LspTop { top: Option<String> },
    MonacoShow { name: String },
}
```

`reduce` is pure: no `Waveform` calls, no DOM, no LSP. It may *ask* for outside
work through `Effect`s, which `App::update` drains after the dispatch:

```rust
fn update(&mut self, _ctx: &Context<Self>, msg: Msg) -> bool {
    match msg {
        // a dispatch is re-entrancy safe (2.6); it returns before any pane
        // re-renders, because subscribers only enqueue Yew messages
        Msg::Action(action) => {
            self.store.dispatch(action);
            for effect in self.store.take_effects() { self.executor.run(effect); }
            false
        }
        // the one slice App itself reads: the CSS variables of the shell
        Msg::Layout(layout) => { self.layout = layout; true }
    }
}
```

Two consequences worth having:

- the reducer is testable with `cargo test` on the host target
  (`reduce(&mut state, Action::RowAdded { .. })`), which is the first real unit
  test surface the front end gets;
- `added_seq` disappears. `Action::RowAdded` is applied once by the reducer, so
  "a deleted row comes back" cannot happen by construction, and adding the same
  signal twice is a dedupe *in the reducer*, not in a component.

### 2.6 Dispatch loop, borrows, re-entrancy

`Action` handlers frequently cascade (`RowSettingChanged` → status message,
`JumpRequested` → `Status` + `FocusChanged`). The rules:

1. `dispatch` pushes onto `queue` and, unless `draining` is set, drains:
   pop → `reduce` under a `RefCell` borrow that ends before anything else →
   bump `version` → **drop the borrow** → notify subscribers.
2. A subscriber callback may `dispatch` again; it is appended to the same queue
   and handled by the same drain loop (depth is bounded by the queue, not by
   the stack).
3. A notification only calls `Callback::emit`, and a Yew `Callback` from
   `ctx.link()` *queues* a message; it never runs another component's `update`
   synchronously. So dispatching from inside `update` (the effect executor does
   exactly that) cannot recurse into a render, and a pane can never observe a
   half-applied action.
4. A subscriber whose callback panics (a bug in a pane) is skipped and logged;
   the store does not poison.

That is the whole discipline: *mutate under a borrow, notify outside it, and
let Yew schedule the renders.*

---

## 3. Layout

Today `.code-pane { flex: 0 0 50% }` and `.wave-pane { flex: 1 }` in
`styles.css`: the editor is always half of the main pane, even with the tab
strip empty and no design loaded — 388 px of monaco showing nothing, while the
wave pane is short.

### 3.1 The rule

1. The source pane occupies `--editor-h`; the wave pane takes the rest.
2. With **no sources open**, the source pane collapses to the tab strip: the
   pane is the tab strip and the editor content height is **0** (the wave pane
   gets the 388 px back and shows a "drop a .vcd/.fst or a .sv to start" hint
   where the empty editor was).
3. As soon as one source opens, the pane expands to its remembered ratio
   (`layout.editor_ratio`, default `0.42`) — automatic, no click. The expanded
   editor content is **at least 120 px**, whatever the ratio says. The ratio is
   applied to the space the inspector dock leaves, not to the whole pane: the
   dock lives in the same column, so counting it as editor share pushed the wave
   pane down to its four-row minimum as soon as the sources opened.
4. `.wave-splitter` (5 px, `cursor: row-resize`) drags the boundary. Double
   click restores the default ratio. The bounds are locked, see §3.4.
5. The inspector (drivers/loads) is a fixed dock **between** the editor and the
   wave pane, not inside the collapsing region: it stays visible when the
   editor collapses, and hides itself when it has nothing to say
   (`design.is_empty() && focus.signal.is_none()`).

### 3.2 How it is expressed

One stored field, two derived booleans:

```rust
pub struct Layout {
    pub sidebar_w: f64,      // 280.0 default, clamped as today
    pub editor_ratio: f32,   // 0.42 default, clamped to [0.15, 0.75] on every write
    pub drawer_open: bool,
}
// derived, never stored:
fn collapsed(sources: &Sources) -> bool { sources.items.is_empty() }   // same versioned slice as Rows
fn inspector_visible(state: &State) -> bool { !state.design.is_empty() || state.focus.signal.is_some() }
```

`App` measures the main pane once per layout change (`rendered()` plus the
existing window `resize` listener; `height` of `.main-pane`) and renders exactly
one inline style from `layout`:

```rust
// App::view — the shell owns the two variables every pane is laid out from.
// The inspector docks in the same column, so it is subtracted before the ratio
// is applied; the four-row wave minimum still wins over the ratio.
let dock = if inspector_visible { INSPECTOR_MAX } else { 0.0 };
let share = (main_h - dock).max(TABSTRIP_PX + EDITOR_CONTENT_MIN);
<div class="app" style={format!(
    "--sidebar-w:{:.0}px; --editor-h:{}px",
    layout.sidebar_w,
    if collapsed { TABSTRIP_PX } else { (share * layout.editor_ratio as f64).clamp(min, max) },
)}>
```

CSS changes (`styles.css`, not R2's state work but required by it):

```css
.main-pane { display: flex; flex-direction: column; }
.code-pane { flex: 0 0 var(--editor-h); min-height: var(--tabs-h); }
.inspector { flex: 0 0 auto; max-height: 190px; }
.wave-pane { flex: 1 1 auto; min-height: 0; }
.wave-splitter { flex: 0 0 5px; cursor: row-resize; }   /* same drag-shield trick as the column splitter */
```

The rule to keep: **the ratio is the stored value and the pixel height is
derived from the measurement** — no code ever hard-codes "388 px", and a window
resize recomputes `--editor-h` from the same ratio.

### 3.3 Dragging without re-rendering

Both splitters follow the Tier A rule of §4.1: on `mousedown` the splitter
component records `(start_x/start_y, start_value)` and adds a `.dragging` class;
on `mousemove` it writes the CSS variable directly
(`document.documentElement.style.setProperty("--sidebar-w", …)`) and lets CSS
reflow; on `mouseup` it dispatches **one** `Action::LayoutChanged`. No Yew
render happens during the drag, which is the actual fix for the "heavy
splitter", not a faster render path.

### 3.4 Locked constants

Do not invent other numbers; these are the contract the QA gate asserts.

| Quantity | Value |
|---|---|
| collapsed editor | tab strip only, **content height 0** |
| expanded editor content, minimum | **120 px** |
| `layout.editor_ratio` | clamped to **[0.15, 0.75]** on every write |
| wave pane minimum | **4 rows** = `4 * --row-h` |

**Precedence.** The wave minimum wins over the ratio: if the ratio would push
the wave pane below four rows, the *ratio is trimmed*, never the rows. The same
rule applies while dragging — the drag stops at the 4-row floor and at the
120 px editor floor, it does not "remember" an out-of-range value (so the next
window resize cannot leave the panes inverted).

---

## 4. Avoiding re-renders (the rules that make it fast)

Three tiers. A feature is assigned to a tier by *event frequency*, not by
importance.

### 4.1 Tier A — nothing enters Yew

Continuous pointer input: splitter drag, drag-to-zoom band, hover, canvas
scroll, the inspector's hover highlight, the tab strip's drag-over state.

- the component keeps the provisional value in a plain field;
- it mutates the DOM directly (`style.setProperty`, `classList`, canvas
  `fillRect`) or repaints the canvas from its own copy of the view;
- on gesture end it dispatches one commit action.

The canvas is the natural home for this: `WaveShow` already owns a
`CanvasRenderingContext2d` and a `draw()`; a drag adds a band to `Frame.drag`
and calls `draw()` again. Yew is not involved, so a 60 Hz drag costs one canvas
repaint, no diffing.

### 4.2 Tier B — one slice, one leaf

Discrete user actions: a wheel tick, a cursor click, a format change, adding a
row, a tab switch, a jump.

- dispatch a committed action;
- subscribers of the touched slice react: `TimeBar` re-renders (it is ~30 DOM
  nodes), the canvas repaints through `Msg::Redraw` (returns `false`), the
  inspector re-queries if `focus.signal` changed;
- `App` does not re-render unless `layout` changed.

### 4.3 Tier C — structural

Opening a dump, parsing sources, changing the top module. Several slices
change; the panes that read them re-render once per dispatch.

### 4.4 What we do *not* store

`scroll_top`, the drag band, hover row, the open/closed state of a tree node
and the cursor of a text editor are component-local. They are reconstructed on
mount (from `focus.instance`, `view`, `scroll_committed`) and never notified.

---

## 5. Migration order

Each step ends with: `cargo check -p front-end --no-default-features --features
wasm --target wasm32-unknown-unknown` clean, one commit, and the browser matrix
green (`& "$env:TEMP\wi-qa\run-qa.ps1"`). "Rewire" below means "adjust the
assertion to the store-shaped DOM/behaviour"; assertions not mentioned keep
working unchanged.

### Step 1 — the store exists, layout and status move in (no visual change)

- add `state/{mod,state,action,reduce,store,select,types}.rs`; move `Goto`,
  `SourceClick`, `Picker`, `Status` out of `nav/` (re-exported there);
- `App` owns `Store` and the small executor; `App::update` becomes
  `Msg::Action(_)` + the transport messages, which now *dispatch*;
- `layout` (sidebar width, drawer) and `nav.status` move into `State`;
  `TopBar`/`App` subscribe;
- the sidebar splitter keeps its current behaviour but starts writing
  `--sidebar-w` directly during the drag (Tier A); commit on mouseup;
- `Msg::SplitterDown/Move/Up` disappear.

Rewire: none. Watch: `layout: splitter resizes the sidebar`,
`layout: canvas follows the splitter`, `empty: app renders`.
New assertions: `layout: splitter drag re-renders nothing` (count draws via
`window.__waveDebug.rows`/a `renderCount` seam), `layout: double click restores
the default width`.

### Step 2 — the wave pane becomes a subscriber (rows, viewport, cursors)

- `Rows`, `View`, `Row` and per-row `Settings` move into `State`;
  `SignalThings` is reduced to `Rc<Vec<Row>>` plus a version;
- `WaveShowProps` shrinks to `{ store }`; `added_seq`, `signaladd`,
  `signal_value`, `waveform`, `end_clock` go away (dump and end time come from
  `State.dump`);
- the canvas reads `view` with `select` inside `draw()` and subscribes to
  `rows`/`dump` with `Msg::Redraw`; `TimeBar` subscribes to `view`;
- wheel zoom and cursor clicks dispatch committed actions; drag-zoom stays
  local until mouseup.

Rewire (canvas-side selectors): `zoom: *`, `pan: *`, `cursor1/2: *`,
`formats: *`, `colour: *`, `delete: *`, `deep: unknown values`,
`deep: clicking a name does not reorder rows`, `deep: dragging a name reorders
rows`, `deep: settings open for the row whose button was clicked`,
`extras: fst bus shows a value at the cursor` — these already go through
`__waveDebug.x_axis/size/cursor1/2` and `__qa2.fullHash()/bandInk()`, so most
only need their row-count reads switched from `.wave-name-cell` to
`window.__waveDebug.total_rows`.
Delete (obsolete by construction): the `added_seq` note inside
`delete: row stays deleted across re-renders`.

### Step 3 — documents, design and LSP effects

- `sources` (name, text, language, version, active tab) is the only document
  list; `App.verilog_source`, `LspBridge.documents` and `SharedDesign` are
  removed;
- `Design` holds `DesignDb` + the elaboration cache + `top`; `select::tree`
  derives the module tree (dump hierarchy preferred, design fallback — moving
  `refresh_module()` into a selector);
- `ModuleStruct`/`ModuleComponent` lose `root`, `show_module`, `space`, `name`;
  they subscribe to `tree` and `focus.instance`; expansion stays local, the
  *selected instance* becomes `focus.instance`;
- `CodeReader`/`CodeBlock` subscribe to `sources` + `nav.goto`; the per-file
  `shown` map stays (it is monaco state) but the *jump* token comes from the
  store.

Rewire: `sources: *`, `combo: *`, `clickToAdd: *`, `waveJump: *`, `inspector: *`,
`lsp: *`, `close: lsp document closed | closed source still addable`,
`deep: switching tabs switches the buffer | one model per opened file | an
unsaved edit survives a tab switch | the filter narrows the signal list |
reloading a source keeps one tab | reloading a source keeps one document`,
`add: *`.
New assertion: `design: adding a source re-parses exactly once` (version seam).

### Step 4 — layout and the legacy teardown

- `--editor-h`, `.wave-splitter`, collapse rule and the inspector dock (§3);
- delete `Rc<RefCell<ModuleValue>>`, `Rc<Module>`, `signal_value`, the
  `end_clock` plumbing and the `#[cfg(feature = "backend")]` values path (R3
  removes the transports it served);
- `nav/` keeps only components; `state/` has no UI imports.

Rewire: `layout: *` (all four), `empty: *`, `vcd: module tree has top | top
signals listed`, `fst: *`, `both: *`, `extras: the wave canvas stays usable in
a tiny window`.
New assertions: `layout: the editor starts collapsed with no sources`,
`layout: opening a source expands the editor`,
`layout: the inspector is hidden with no design and no selection`.

---

## 6. Acceptance

The task-10 wording, made checkable:

- `grep -R "RefCell<ModuleValue>" front-end/src` → nothing;
- `grep -R "Rc<Module>" front-end/src` → nothing;
- `SharedDesign`, `SharedWaveform`, `signal_add`, `signal_add_seq`, `added_seq`,
  `goto_token`, `splitter_start` → nothing;
- each pane component has a `Subscription` field, and no `struct` field that
  duplicates a `State` field (the table in §2.2 is the checklist);
- the layout constants of §3.4 hold, and `window.__waveDebug.render_count` does
  not move during a splitter drag (it counts shell re-renders only);
- `cargo test -p front-end --bins` covers `reduce` (row add/remove/reorder,
  document open/close, dump switch, jump, picker) — no DOM needed;
- the 140-assertion browser matrix passes, with the rewires listed above.

---

## 7. Rejected alternatives

- **Yew `ContextProvider` + context hooks.** Re-renders every consumer of the
  context, which is exactly what we are removing; hooks (`use_selector`) are
  not usable from the class components that make up the tree after R1.
- **A signals crate (`yewdux`, `leptos_reactive`, …).** New dependency, new
  runtime semantics, and the subscription we need is ~120 lines with exactly
  the slices we want.
- **Keeping the store out of the reducer (`RefCell` mutation from handlers).**
  That is what we have now.
- **Storing the viewport only in the canvas.** The time bar needs it, and it
  must survive a pane re-render; only *uncommitted* drags are local.
- **One global `Rc<RefCell<State>>` read directly by components.** No
  subscription, implicit borrows across components, and no way to know why a
  component re-rendered. `select`/`subscribe` are the only door.

## 8. Risks

- **Borrow panics** if a subscriber is called while `State` is borrowed. The
  drain loop in §2.6 is the mitigation; add a debug assertion that
  `state.try_borrow_mut().is_ok()` when notifying.
- **Effect ordering**: `SourceOpened` → `LspOpen` → a *later* `SourceClosed`
  must not leave a document open. The reducer owns the document list, so
  effects are derived from the diff of `sources` before/after, not enqueued ad
  hoc.
- **R3 overlap**: `state/` must stay UI-free, or the deletion pass will have to
  move it. Keep `Row`/`View` free of `wave_show` types (no `Signal`, no
  `Series`).
- **Yew 0.19 `Callback<S>` ties a subscription to a component scope**: keeping
  the `Subscription` guard in the component struct and dropping it in `destroy`
  is mandatory; forgetting it leaks the callback (and the scope) for the page
  lifetime.
