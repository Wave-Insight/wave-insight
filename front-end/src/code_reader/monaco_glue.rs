//! Thin, in-repo binding to the `monaco-editor` AMD distribution.
//!
//! The crate used to depend on the `monaco` git crate (siku2/rust-monaco). Its
//! Yew wrapper is what [`crate::code_reader::code_block`] needs, but the repo is
//! unreachable from the build machine and every published `monaco` release
//! requires `PartialEq`/`Clone` from the JS option bindings. Instead of pinning
//! a git revision we drive monaco directly over `js_sys`:
//!
//! * the editor is loaded lazily, exactly once per page, from a CDN:
//!   `vs/loader.js` is injected, `require.config({paths: {vs: ...}})` is called
//!   and `require(["vs/editor/editor.main"])` resolves the real editor,
//! * [`VS_ROOTS`] lists the mirrors that are tried when a host is unreachable,
//! * a [`MonacoEditor`] owns the JS handle so the Yew component can keep it
//!   across renders and dispose it on unmount.
//!
//! No code generation and no extra dependencies are involved: only `js-sys`,
//! `wasm-bindgen` and `web-sys`.
//!
//! # License
//!
//! The lifecycle design (create after render, dispose on destroy, reveal the
//! requested line in the centre) is derived from rust-monaco's Yew wrapper,
//! Copyright (c) 2020 Simon Berger, MIT OR Apache-2.0. The full license texts
//! are kept next to this file in `rust-monaco.LICENSE.txt`.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use js_sys::{Array, Function, Object, Reflect};
use wasm_bindgen::closure::Closure;
use wasm_bindgen::{JsCast, JsValue};
use web_sys::HtmlElement;

/// Where the AMD build of monaco-editor can be fetched from, tried in order.
///
/// Each entry must point at a `min/vs` directory of the same release and is
/// only used as the base URL for `loader.js` and `require.config({paths.vs})`.
/// The Monaco docs use `cdn.jsdelivr.net`, but that host is blocked on the
/// development network (connection reset), so it is not listed; cdnjs is the
/// primary mirror and the other two are fallbacks for restricted networks.
/// Bump the version in every entry together.
const VS_ROOTS: &[&str] = &[
    "https://cdnjs.cloudflare.com/ajax/libs/monaco-editor/0.44.0/min/vs",
    "https://unpkg.com/monaco-editor@0.44.0/min/vs",
    "https://fastly.jsdelivr.net/npm/monaco-editor@0.44.0/min/vs",
];

/// The entry point that pulls in the editor, the languages and the workers.
const EDITOR_MODULE: &str = "vs/editor/editor.main";

/// Construction options for a standalone editor.
///
/// Mirrors the subset of `monaco.editor.IStandaloneEditorConstructionOptions`
/// this project needs; monaco would reject unknown Rust fields, so the JS
/// object is assembled by hand in [`EditorOptions::to_js_value`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EditorOptions {
    /// Initial contents of the buffer.
    pub value: String,
    /// Monaco language id, e.g. `systemverilog` (monaco has no plain `verilog`
    /// id; the SystemVerilog grammar highlights Verilog too).
    pub language: String,
    /// Monaco theme name, e.g. `vs-dark`.
    pub theme: String,
    /// Whether the buffer is editable.
    pub read_only: bool,
    /// Whether the minimap is shown.
    pub minimap: bool,
    /// Font size in CSS pixels.
    pub font_size: u32,
    /// Let monaco watch the host element and re-layout on resize.
    pub automatic_layout: bool,
}

impl Default for EditorOptions {
    fn default() -> Self {
        Self {
            value: String::new(),
            language: "plaintext".to_owned(),
            theme: "vs-dark".to_owned(),
            read_only: false,
            minimap: false,
            font_size: 13,
            automatic_layout: true,
        }
    }
}

impl EditorOptions {
    /// Options with the defaults above and the given text/language/theme.
    pub fn new(value: impl Into<String>, language: impl Into<String>, theme: impl Into<String>) -> Self {
        Self {
            value: value.into(),
            language: language.into(),
            theme: theme.into(),
            ..Self::default()
        }
    }

    fn to_js_value(&self) -> JsValue {
        let options = Object::new();
        set(&options, "value", JsValue::from_str(&self.value));
        set(&options, "language", JsValue::from_str(&self.language));
        set(&options, "theme", JsValue::from_str(&self.theme));
        set(&options, "readOnly", JsValue::from_bool(self.read_only));
        set(&options, "fontSize", JsValue::from_f64(f64::from(self.font_size)));
        set(&options, "automaticLayout", JsValue::from_bool(self.automatic_layout));

        let minimap = Object::new();
        set(&minimap, "enabled", JsValue::from_bool(self.minimap));
        set(&options, "minimap", JsValue::from(minimap));

        JsValue::from(options)
    }
}

/// A live monaco editor, or the promise of one while the CDN is loading.
///
/// The handle is created asynchronously (the first call has to download the
/// editor), so the methods below are safe to call right after
/// [`MonacoEditor::create`]: changes made before the editor exists are applied
/// when it is finally created. Dropping the value disposes the editor.
pub struct MonacoEditor {
    slot: Rc<RefCell<EditorSlot>>,
}

struct EditorSlot {
    /// The JS `IStandaloneCodeEditor`, `None` until monaco is ready.
    handle: Option<JsValue>,
    /// Current buffer contents, replayed when the editor is created late.
    value: String,
    /// Line to reveal once the editor exists.
    reveal: Option<u32>,
    /// Set by [`MonacoEditor::dispose`]; a pending creation must then bail out.
    disposed: bool,
}

impl MonacoEditor {
    /// Create an editor inside `host` with the default options.
    pub fn create(host: &HtmlElement, initial_text: &str, language: &str, theme: &str) -> Self {
        Self::create_with_options(host, EditorOptions::new(initial_text, language, theme))
    }

    /// Create an editor inside `host`, overriding any [`EditorOptions`] field.
    pub fn create_with_options(host: &HtmlElement, options: EditorOptions) -> Self {
        let slot = Rc::new(RefCell::new(EditorSlot {
            handle: None,
            value: options.value.clone(),
            reveal: None,
            disposed: false,
        }));

        // The AMD callback is `'static`, keep an owned view of the mount point.
        let host = JsValue::from(host.clone());
        let creator_slot = Rc::clone(&slot);

        ensure_loaded(Box::new(move |monaco| {
            let (value, reveal) = {
                let slot = creator_slot.borrow();
                if slot.disposed {
                    return;
                }
                (slot.value.clone(), slot.reveal)
            };

            let options = EditorOptions { value, ..options };
            match create_editor(monaco, &host, &options) {
                Ok(handle) => {
                    let mut slot = creator_slot.borrow_mut();
                    if slot.disposed {
                        // The component went away while monaco was still loading.
                        drop(slot);
                        dispose_handle(&handle);
                        return;
                    }
                    slot.handle = Some(handle.clone());
                    drop(slot);

                    if let Some(line) = reveal {
                        reveal_line_in_center_js(&handle, line);
                    }
                }
                Err(err) => console_error(&format!(
                    "wave-insight: monaco could not create the editor: {}",
                    describe(&err)
                )),
            }
        }));

        Self { slot }
    }

    /// The raw JS editor handle, for code that wants to talk to monaco directly
    /// (language clients, decorations, ...). `None` until the loader is done.
    #[allow(dead_code)] // reserved for the upcoming language-client integration
    pub fn editor(&self) -> Option<JsValue> {
        self.slot.borrow().handle.clone()
    }

    /// Replace the whole buffer, e.g. when another file tab is activated.
    pub fn set_value(&self, text: &str) {
        let handle = {
            let mut slot = self.slot.borrow_mut();
            slot.value = text.to_owned();
            slot.handle.clone()
        };
        if let Some(handle) = handle {
            set_value_js(&handle, text);
        }
    }

    /// Scroll the cursor line to the middle of the viewport.
    pub fn reveal_line_in_center(&self, line: u32) {
        let handle = {
            let mut slot = self.slot.borrow_mut();
            slot.reveal = Some(line);
            slot.handle.clone()
        };
        if let Some(handle) = handle {
            reveal_line_in_center_js(&handle, line);
        }
    }

    /// Tear the editor down. Safe to call repeatedly and before the loader is
    /// done (the pending creation is then abandoned).
    pub fn dispose(&self) {
        let handle = {
            let mut slot = self.slot.borrow_mut();
            slot.disposed = true;
            slot.handle.take()
        };
        if let Some(handle) = handle {
            dispose_handle(&handle);
        }
    }
}

impl Drop for MonacoEditor {
    fn drop(&mut self) {
        self.dispose();
    }
}

// ---------------------------------------------------------------------------
// AMD loader
// ---------------------------------------------------------------------------

type ReadyCallback = Box<dyn FnOnce(&JsValue)>;

/// The AMD loader installs itself on the window, so the loading progress is a
/// page-wide singleton rather than a per-editor state.
enum LoaderState {
    Idle,
    /// `loader.js` / `editor.main` are downloading, callbacks wait here.
    Loading(Vec<ReadyCallback>),
    /// The `monaco` namespace is available.
    Ready(JsValue),
    /// The CDN could not be used; editors will never be created.
    Failed,
}

thread_local! {
    static LOADER: RefCell<LoaderState> = RefCell::new(LoaderState::Idle);
    /// Index into [`VS_ROOTS`] of the mirror currently being tried.
    static CDN_INDEX: Cell<usize> = const { Cell::new(0) };
}

/// Run `callback` with the `monaco` namespace, loading it first if needed.
fn ensure_loaded(callback: ReadyCallback) {
    let mut callback = Some(callback);
    let mut ready: Option<JsValue> = None;
    let mut start_loading = false;

    LOADER.with(|state| {
        let mut state = state.borrow_mut();
        let mut next: Option<LoaderState> = None;
        match &mut *state {
            LoaderState::Ready(monaco) => ready = Some(monaco.clone()),
            LoaderState::Loading(pending) => {
                if let Some(callback) = callback.take() {
                    pending.push(callback);
                }
            }
            LoaderState::Idle => {
                if let Some(callback) = callback.take() {
                    next = Some(LoaderState::Loading(vec![callback]));
                }
            }
            LoaderState::Failed => {}
        }
        if let Some(next) = next {
            *state = next;
            start_loading = true;
        }
    });

    if let Some(monaco) = ready {
        if let Some(callback) = callback.take() {
            callback(&monaco);
        }
        return;
    }

    if start_loading {
        start_load();
    }
}

/// Try the current [`VS_ROOTS`] entry, falling through to the next mirror when
/// the loader script itself cannot be fetched.
fn start_load() {
    let index = CDN_INDEX.with(Cell::get);
    let Some(root) = VS_ROOTS.get(index) else {
        mark_failed();
        return;
    };
    if let Err(err) = inject_loader(root) {
        console_error(&format!(
            "wave-insight: could not inject the monaco loader from {root}: {}",
            describe(&err)
        ));
        try_next_cdn();
    }
}

fn try_next_cdn() {
    let next = CDN_INDEX.with(|index| {
        let next = index.get() + 1;
        index.set(next);
        next
    });
    if next < VS_ROOTS.len() {
        start_load();
    } else {
        console_error("wave-insight: no monaco-editor CDN could be reached");
        mark_failed();
    }
}

/// Inject `<script src="<root>/loader.js">` and wire the AMD bootstrap.
fn inject_loader(root: &str) -> Result<(), JsValue> {
    let window = web_sys::window().ok_or_else(|| JsValue::from_str("no window"))?;
    let document = window
        .document()
        .ok_or_else(|| JsValue::from_str("no document"))?;

    let script: web_sys::HtmlScriptElement = document.create_element("script")?.dyn_into()?;
    let loader_url = format!("{root}/loader.js");

    let on_load = Closure::once_into_js(move || {
        if let Err(err) = configure_require() {
            console_error(&format!(
                "wave-insight: could not configure the monaco AMD loader: {}",
                describe(&err)
            ));
            mark_failed();
        }
    });
    let failed_url = loader_url.clone();
    let on_error = Closure::once_into_js(move |err: JsValue| {
        console_error(&format!(
            "wave-insight: could not download {failed_url}: {}",
            describe(&err)
        ));
        try_next_cdn();
    });

    script.add_event_listener_with_callback("load", on_load.unchecked_ref())?;
    script.add_event_listener_with_callback("error", on_error.unchecked_ref())?;
    script.set_src(&loader_url);
    script.set_async(true);

    let head = document
        .head()
        .ok_or_else(|| JsValue::from_str("document has no head"))?;
    head.append_child(&script)?;
    Ok(())
}

/// `require.config({paths: {vs: ...}})`, then `require(["vs/editor/editor.main"])`.
fn configure_require() -> Result<(), JsValue> {
    let window = web_sys::window().ok_or_else(|| JsValue::from_str("no window"))?;
    let require = Reflect::get(&window, &JsValue::from_str("require"))?;
    if !require.is_function() {
        return Err(JsValue::from_str(
            "loader.js did not define a global require()",
        ));
    }

    let root = CDN_INDEX.with(Cell::get);
    let root = VS_ROOTS
        .get(root)
        .ok_or_else(|| JsValue::from_str("no monaco-editor CDN configured"))?;

    let configure: Function = Reflect::get(&require, &JsValue::from_str("config"))?.dyn_into()?;
    let paths = Object::new();
    set(&paths, "vs", JsValue::from_str(root));
    let config = Object::new();
    set(&config, "paths", JsValue::from(paths));
    configure.call1(&require, &JsValue::from(config))?;

    let dependencies = Array::of1(&JsValue::from_str(EDITOR_MODULE));
    let on_ready = Closure::once_into_js(move |module: JsValue| module_ready(module));
    let on_error = Closure::once_into_js(move |err: JsValue| {
        console_error(&format!(
            "wave-insight: could not load {EDITOR_MODULE}: {}",
            describe(&err)
        ));
        mark_failed();
    });
    require
        .unchecked_ref::<Function>()
        .call3(&require, &dependencies, &on_ready, &on_error)?;
    Ok(())
}

/// The editor module resolved: publish the namespace and wake every waiter.
fn module_ready(module: JsValue) {
    let monaco = web_sys::window()
        .and_then(|window| Reflect::get(&window, &JsValue::from_str("monaco")).ok())
        .filter(JsValue::is_object)
        .unwrap_or(module);

    let pending = LOADER.with(|state| {
        let mut state = state.borrow_mut();
        let previous = std::mem::replace(&mut *state, LoaderState::Ready(monaco.clone()));
        match previous {
            LoaderState::Loading(pending) => pending,
            _ => Vec::new(),
        }
    });

    for callback in pending {
        callback(&monaco);
    }
}

/// Give up permanently; queued editors are dropped instead of half-created.
fn mark_failed() {
    LOADER.with(|state| {
        let mut state = state.borrow_mut();
        let previous = std::mem::replace(&mut *state, LoaderState::Failed);
        drop(previous);
    });
}

// ---------------------------------------------------------------------------
// JS plumbing
// ---------------------------------------------------------------------------

fn create_editor(monaco: &JsValue, host: &JsValue, options: &EditorOptions) -> Result<JsValue, JsValue> {
    let namespace = Reflect::get(monaco, &JsValue::from_str("editor"))?;
    let create: Function = Reflect::get(&namespace, &JsValue::from_str("create"))?.dyn_into()?;
    create.call2(&namespace, host, &options.to_js_value())
}

fn set_value_js(editor: &JsValue, text: &str) {
    if let Some(set_value) = js_function(editor, "setValue") {
        let _ = set_value.call1(editor, &JsValue::from_str(text));
    }
}

fn reveal_line_in_center_js(editor: &JsValue, line: u32) {
    if let Some(reveal) = js_function(editor, "revealLineInCenter") {
        let _ = reveal.call1(editor, &JsValue::from_f64(f64::from(line.max(1))));
    }
}

fn dispose_handle(editor: &JsValue) {
    if let Some(dispose) = js_function(editor, "dispose") {
        let _ = dispose.call0(editor);
    }
}

fn js_function(target: &JsValue, name: &str) -> Option<Function> {
    Reflect::get(target, &JsValue::from_str(name))
        .ok()?
        .dyn_into()
        .ok()
}

fn set(object: &JsValue, key: &str, value: JsValue) {
    // `Reflect::set` only fails for non-object targets, and `object` is a fresh
    // `Object` here, so the error case cannot happen.
    let _ = Reflect::set(object, &JsValue::from_str(key), &value);
}

fn console_error(message: &str) {
    web_sys::console::error_1(&JsValue::from_str(message));
}

fn describe(value: &JsValue) -> String {
    value
        .as_string()
        .unwrap_or_else(|| format!("{value:?}"))
}
