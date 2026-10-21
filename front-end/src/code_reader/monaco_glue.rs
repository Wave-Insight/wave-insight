//! Thin, in-repo binding to the `monaco-editor` AMD distribution.
//!
//! The crate used to depend on the `monaco` git crate (siku2/rust-monaco). Its
//! Yew wrapper is what [`crate::code_reader::code_block`] needs, but the repo is
//! unreachable from the build machine and every published `monaco` release
//! requires `PartialEq`/`Clone` from the JS option bindings. Instead of pinning
//! a git revision we drive monaco directly over `js_sys`:
//!
//! * the editor is loaded lazily, exactly once per page, from the pinned build
//!   the app ships: `monaco/vs/loader.js` is injected, the AMD
//!   `require.config({paths: {vs: ...}})` root is derived from the URL the
//!   browser actually resolved, and `require(["vs/editor/editor.main"])`
//!   resolves the real editor — no CDN is contacted (see `monaco/README.md`),
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

use std::cell::RefCell;
use std::rc::Rc;

use js_sys::{Array, Function, Object, Reflect};
use wasm_bindgen::closure::Closure;
use wasm_bindgen::{JsCast, JsValue};
use web_sys::HtmlElement;

/// The pinned monaco-editor release, shipped with the app (see `monaco/README.md`).
///
/// The value is only a marker for the update instructions and the loader log;
/// the files themselves live in `front-end/monaco/vs` and are copied to
/// `dist/monaco/vs` by trunk.
const VS_VERSION: &str = "0.44.0";

/// Where the pinned `vs` directory is served from, relative to `index.html`.
///
/// The loader script is injected with this relative URL and the AMD
/// `paths.vs` root is derived from the *absolute* URL the browser resolved, so
/// the app also works when it is deployed under a sub path.
const VS_ROOT: &str = "./monaco/vs";

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

/// A live monaco editor, or the promise of one while the editor is loading.
///
/// The handle is created asynchronously (the first call has to download the
/// editor), so the methods below are safe to call right after
/// [`MonacoEditor::create`]: changes made before the editor exists are applied
/// when it is finally created. Dropping the value disposes the editor.
///
/// The editor itself is a *view*: every document the source pane shows lives in
/// its own monaco text model, addressed by the URI the LSP client uses (see
/// [`crate::lsp::document_uri`]). Switching tabs therefore re-attaches a model
/// instead of overwriting one buffer, which is what lets monaco's own language
/// features (go to definition, peek references) resolve a range in a file that
/// is not on screen.
pub struct MonacoEditor {
    slot: Rc<RefCell<EditorSlot>>,
}

/// A document queued until monaco is ready.
struct PendingDocument {
    uri: String,
    language: String,
    text: String,
    /// Replace the buffer with `text`; `false` keeps whatever the model holds
    /// (an unsaved edit) and only attaches it.
    replace: bool,
}

struct EditorSlot {
    /// The JS `IStandaloneCodeEditor`, `None` until monaco is ready.
    handle: Option<JsValue>,
    /// The `monaco` namespace, `None` until monaco is ready.
    monaco: Option<JsValue>,
    /// Current buffer contents, replayed when the editor is created late.
    value: String,
    /// `(line, column)` to reveal once the editor exists.
    reveal: Option<(u32, u32)>,
    /// The document that should be attached once the editor exists.
    pending: Option<PendingDocument>,
    /// Set by [`MonacoEditor::dispose`]; a pending creation must then bail out.
    disposed: bool,
    /// Keeps the `onMouseDown` callback alive for as long as the editor.
    listener: Option<Closure<dyn FnMut(JsValue)>>,
    /// Keeps the `onDidChangeModelContent` callback alive.
    change_listener: Option<Closure<dyn FnMut(JsValue)>>,
    /// Callbacks waiting for `(monaco namespace, editor handle)`.
    pending_ready: Vec<Box<dyn Fn(&JsValue, &JsValue)>>,
    /// The change callback, installed once the editor exists.
    pending_change: Option<Box<dyn Fn(String, String)>>,
}

impl MonacoEditor {
    /// Create an editor inside `host` with the default options.
    #[allow(dead_code)] // kept for callers that need no click handling
    pub fn create(host: &HtmlElement, initial_text: &str, language: &str, theme: &str) -> Self {
        Self::build(host, EditorOptions::new(initial_text, language, theme), None)
    }

    /// Create an editor inside `host`, overriding any [`EditorOptions`] field.
    #[allow(dead_code)] // kept for callers that need no click handling
    pub fn create_with_options(host: &HtmlElement, options: EditorOptions) -> Self {
        Self::build(host, options, None)
    }

    /// Create an editor that reports clicks on identifiers.
    ///
    /// `on_click(line, column, word)` is called for a plain left click on an
    /// identifier; clicks with a modifier held and non text targets are ignored,
    /// so text selection, multi cursor and drag and drop keep working. `column`
    /// is the first column of the identifier, i.e. what a "what is here" query
    /// wants.
    pub fn create_interactive(
        host: &HtmlElement,
        options: EditorOptions,
        on_click: Box<dyn Fn(u32, u32, String) + 'static>,
    ) -> Self {
        Self::build(host, options, Some(on_click))
    }

    fn build(
        host: &HtmlElement,
        options: EditorOptions,
        on_click: Option<Box<dyn Fn(u32, u32, String) + 'static>>,
    ) -> Self {
        let slot = Rc::new(RefCell::new(EditorSlot {
            handle: None,
            monaco: None,
            value: options.value.clone(),
            reveal: None,
            pending: None,
            disposed: false,
            listener: None,
            change_listener: None,
            pending_ready: Vec::new(),
            pending_change: None,
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
                    let (ready, change) = {
                        let mut slot = creator_slot.borrow_mut();
                        if slot.disposed {
                            // The component went away while monaco was still loading.
                            drop(slot);
                            dispose_handle(&handle);
                            return;
                        }
                        slot.handle = Some(handle.clone());
                        slot.monaco = Some(monaco.clone());
                        slot.reveal = reveal;
                        (
                            std::mem::take(&mut slot.pending_ready),
                            slot.pending_change.take(),
                        )
                    };

                    if let Some(on_click) = on_click {
                        let listener = install_click_listener(&handle, on_click);
                        creator_slot.borrow_mut().listener = Some(listener);
                    }
                    if let Some(change) = change {
                        // installed before the first document is attached, so
                        // replacing a buffer is reported like any other edit
                        let listener = install_change_listener(&handle, change);
                        creator_slot.borrow_mut().change_listener = Some(listener);
                    }
                    for callback in ready {
                        callback(monaco, &handle);
                    }
                    // attach the queued document and reveal the queued position
                    flush(&mut creator_slot.borrow_mut());
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
    /// (decorations, commands, ...). `None` until the loader is done.
    #[allow(dead_code)] // reserved for direct monaco calls outside the glue
    pub fn editor(&self) -> Option<JsValue> {
        self.slot.borrow().handle.clone()
    }

    /// Run `callback` with the `monaco` namespace and the live editor once both
    /// exist. This is how the language providers are registered without
    /// assuming the editor finished loading.
    pub fn on_ready(&self, callback: Box<dyn Fn(&JsValue, &JsValue)>) {
        let mut callback = Some(callback);
        let ready = {
            let mut slot = self.slot.borrow_mut();
            match (slot.monaco.clone(), slot.handle.clone()) {
                (Some(monaco), Some(handle)) => Some((monaco, handle)),
                _ => {
                    if let Some(callback) = callback.take() {
                        slot.pending_ready.push(callback);
                    }
                    None
                }
            }
        };
        if let (Some((monaco, handle)), Some(callback)) = (ready, callback) {
            callback(&monaco, &handle);
        }
    }

    /// Show the model for `uri`, creating it from `text` the first time.
    ///
    /// Calling this again with a different `text` (the app reloaded the file,
    /// or another tab was activated) replaces the buffer contents; the queued
    /// request is replayed when monaco is still loading.
    pub fn show_document(&self, uri: &str, language: &str, text: &str) {
        self.queue_document(uri, language, text, true);
    }

    /// Attach the model for `uri` without touching its contents.
    ///
    /// Used when the source pane switches back to a file: the model may hold an
    /// edit the app's copy does not have, and switching tabs must not throw it
    /// away. A model that does not exist yet is created from `text`.
    pub fn attach_document(&self, uri: &str, language: &str, text: &str) {
        self.queue_document(uri, language, text, false);
    }

    fn queue_document(&self, uri: &str, language: &str, text: &str, replace: bool) {
        let mut slot = self.slot.borrow_mut();
        slot.pending = Some(PendingDocument {
            uri: uri.to_owned(),
            language: language.to_owned(),
            text: text.to_owned(),
            replace,
        });
        flush(&mut slot);
    }

    /// `editor.onDidChangeModelContent`: `callback(model uri, buffer text)`.
    ///
    /// Replacing the whole buffer (a tab switch, a re-read of the file) fires
    /// the same event, so the language server sees every change the user or the
    /// app makes.
    pub fn on_content_change(&self, callback: Box<dyn Fn(String, String)>) {
        let mut callback = Some(callback);
        let handle = {
            let mut slot = self.slot.borrow_mut();
            match slot.handle.clone() {
                Some(handle) => Some(handle),
                None => {
                    if let Some(callback) = callback.take() {
                        slot.pending_change = Some(callback);
                    }
                    None
                }
            }
        };
        if let (Some(handle), Some(callback)) = (handle, callback) {
            let listener = install_change_listener(&handle, callback);
            self.slot.borrow_mut().change_listener = Some(listener);
        }
    }

    /// Scroll the cursor line to the middle of the viewport.
    #[allow(dead_code)] // the column aware variant below is what the UI uses
    pub fn reveal_line_in_center(&self, line: u32) {
        self.reveal_position_in_center(line, 0);
    }

    /// Move the cursor to `line:col` and scroll that position into the middle.
    ///
    /// `col == 0` means "no column": the line is revealed and the cursor is put
    /// at its first column.
    pub fn reveal_position_in_center(&self, line: u32, col: u32) {
        let mut slot = self.slot.borrow_mut();
        slot.reveal = Some((line, col));
        flush(&mut slot);
    }

    /// Put `line:col` in the middle **and select the identifier there**.
    ///
    /// Used after a jump from the waveform: scrolling alone leaves the eye to
    /// find the name, selecting it makes the target obvious.
    pub fn select_word_at(&self, line: u32, col: u32) {
        self.on_ready(Box::new(move |monaco, editor| {
            let run = || -> Option<()> {
            let call = |target: &JsValue, name: &str, args: &js_sys::Array| -> Option<JsValue> {
                let function = js_sys::Reflect::get(target, &JsValue::from_str(name)).ok()?;
                let function = function.dyn_into::<js_sys::Function>().ok()?;
                function.apply(target, args).ok()
            };

            let model = call(editor, "getModel", &js_sys::Array::new())?;

            let position = js_sys::Object::new();
            js_sys::Reflect::set(&position, &JsValue::from_str("lineNumber"), &JsValue::from_f64(line as f64)).ok()?;
            js_sys::Reflect::set(&position, &JsValue::from_str("column"), &JsValue::from_f64(col.max(1) as f64)).ok()?;

            // select the identifier under the cursor when there is one
            let word = {
                let args = js_sys::Array::new();
                args.push(&position);
                call(&model, "getWordAtPosition", &args)
            };
            let range = word
                .filter(|word| !word.is_null() && !word.is_undefined())
                .and_then(|word| {
                    let start = js_sys::Reflect::get(&word, &JsValue::from_str("startColumn")).ok()?.as_f64()?;
                    let end = js_sys::Reflect::get(&word, &JsValue::from_str("endColumn")).ok()?.as_f64()?;
                    let ctor = js_sys::Reflect::get(monaco, &JsValue::from_str("Range")).ok()?;
                    let ctor = ctor.dyn_into::<js_sys::Function>().ok()?;
                    let args = js_sys::Array::new();
                    args.push(&JsValue::from_f64(line as f64));
                    args.push(&JsValue::from_f64(start));
                    args.push(&JsValue::from_f64(line as f64));
                    args.push(&JsValue::from_f64(end));
                    js_sys::Reflect::construct(&ctor, &args).ok()
                });

            if let Some(range) = range {
                let args = js_sys::Array::new();
                args.push(&range);
                let _ = call(editor, "setSelection", &args);
            }

            let args = js_sys::Array::new();
            args.push(&position);
            let _ = call(editor, "revealPositionInCenter", &args);
            Some(())
            };
            let _ = run();
        }));
    }

    /// Tear the editor down. Safe to call repeatedly and before the loader is
    /// done (the pending creation is then abandoned).
    pub fn dispose(&self) {
        let (handle, listener, change_listener) = {
            let mut slot = self.slot.borrow_mut();
            slot.disposed = true;
            (
                slot.handle.take(),
                slot.listener.take(),
                slot.change_listener.take(),
            )
        };
        if let Some(handle) = handle {
            dispose_handle(&handle);
        }
        // the JS editor no longer exists, so the callbacks can go too
        drop(listener);
        drop(change_listener);
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
    /// The shipped editor could not be loaded; editors will never be created.
    Failed,
}

thread_local! {
    static LOADER: RefCell<LoaderState> = RefCell::new(LoaderState::Idle);
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

/// Load the pinned, self-hosted editor.
///
/// There is exactly one place to load from (the copy trunk ships next to the
/// page), so a failure is final: the app keeps working without an editor pane
/// and the console says which file is missing.
fn start_load() {
    let loader_url = format!("{VS_ROOT}/loader.js");
    if let Err(err) = inject_loader(&loader_url) {
        console_error(&format!(
            "wave-insight: could not inject the monaco loader from {loader_url}: {}",
            describe(&err)
        ));
        mark_failed();
    }
}

/// Inject `<script src="<root>/loader.js">` and wire the AMD bootstrap.
fn inject_loader(loader_url: &str) -> Result<(), JsValue> {
    let window = web_sys::window().ok_or_else(|| JsValue::from_str("no window"))?;
    let document = window
        .document()
        .ok_or_else(|| JsValue::from_str("no document"))?;

    let script: web_sys::HtmlScriptElement = document.create_element("script")?.dyn_into()?;
    script.set_src(loader_url);

    // `script.src` is the absolute URL the browser resolved, which is exactly
    // the AMD root: it stays correct when the app is served under a sub path.
    let absolute = script.src();
    let root = absolute
        .strip_suffix("/loader.js")
        .unwrap_or(absolute.as_str())
        .to_owned();

    let on_load = Closure::once_into_js(move || {
        if let Err(err) = configure_require(&root) {
            console_error(&format!(
                "wave-insight: could not configure the monaco AMD loader: {}",
                describe(&err)
            ));
            mark_failed();
        }
    });
    let failed_url = loader_url.to_owned();
    let on_error = Closure::once_into_js(move |err: JsValue| {
        console_error(&format!(
            "wave-insight: could not load {failed_url} (monaco {VS_VERSION} is shipped in \
             front-end/monaco/vs, see monaco/README.md): {}",
            describe(&err)
        ));
        mark_failed();
    });

    script.add_event_listener_with_callback("load", on_load.unchecked_ref())?;
    script.add_event_listener_with_callback("error", on_error.unchecked_ref())?;
    script.set_async(true);

    let head = document
        .head()
        .ok_or_else(|| JsValue::from_str("document has no head"))?;
    head.append_child(&script)?;
    Ok(())
}

/// `require.config({paths: {vs: <root>}})`, then `require(["vs/editor/editor.main"])`.
fn configure_require(root: &str) -> Result<(), JsValue> {
    let window = web_sys::window().ok_or_else(|| JsValue::from_str("no window"))?;
    let require = Reflect::get(&window, &JsValue::from_str("require"))?;
    if !require.is_function() {
        return Err(JsValue::from_str(
            "loader.js did not define a global require()",
        ));
    }

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

/// The `monaco.Uri` for an LSP document URI.
pub fn parse_uri(monaco: &JsValue, uri: &str) -> Option<JsValue> {
    let namespace = Reflect::get(monaco, &JsValue::from_str("Uri")).ok()?;
    let parse = js_function(&namespace, "parse")?;
    parse
        .call1(&namespace, &JsValue::from_str(uri))
        .ok()
        .filter(JsValue::is_object)
}

/// The text model for `uri`, created from `text` when it does not exist yet.
///
/// This is what gives every design source its own monaco buffer, so a language
/// provider can point monaco at a range in a file that is not on screen.
pub fn ensure_model(monaco: &JsValue, uri: &str, language: &str, text: &str) -> Option<JsValue> {
    let uri_value = parse_uri(monaco, uri)?;
    let editor = Reflect::get(monaco, &JsValue::from_str("editor")).ok()?;
    let get_model = js_function(&editor, "getModel")?;
    if let Ok(existing) = get_model.call1(&editor, &uri_value) {
        if existing.is_object() {
            return Some(existing);
        }
    }
    let create_model = js_function(&editor, "createModel")?;
    match create_model.call3(
        &editor,
        &JsValue::from_str(text),
        &JsValue::from_str(language),
        &uri_value,
    ) {
        Ok(model) if model.is_object() => Some(model),
        // another caller created the same URI first
        _ => get_model
            .call1(&editor, &uri_value)
            .ok()
            .filter(JsValue::is_object),
    }
}

/// `model.uri.toString()`, i.e. the LSP document URI.
pub fn model_uri(model: &JsValue) -> Option<String> {
    let uri = Reflect::get(model, &JsValue::from_str("uri")).ok()?;
    let to_string = js_function(&uri, "toString")?;
    to_string.call0(&uri).ok()?.as_string()
}

/// `model.getValue()`.
pub fn model_text(model: &JsValue) -> Option<String> {
    let get_value = js_function(model, "getValue")?;
    get_value.call0(model).ok()?.as_string()
}

fn set_model_value(model: &JsValue, text: &str) {
    if let Some(set_value) = js_function(model, "setValue") {
        let _ = set_value.call1(model, &JsValue::from_str(text));
    }
}

fn editor_model_uri(editor: &JsValue) -> Option<String> {
    let get_model = js_function(editor, "getModel")?;
    let model = get_model.call0(editor).ok()?;
    if !model.is_object() {
        return None;
    }
    model_uri(&model)
}

fn set_editor_model(editor: &JsValue, model: &JsValue) {
    if let Some(set_model) = js_function(editor, "setModel") {
        let _ = set_model.call1(editor, model);
    }
}

/// Attach the queued document and reveal the queued position.
///
/// Does nothing while monaco is still loading: [`EditorSlot::pending`] and
/// [`EditorSlot::reveal`] are replayed by the creation callback.
fn flush(slot: &mut EditorSlot) {
    let (Some(handle), Some(monaco)) = (slot.handle.clone(), slot.monaco.clone()) else {
        return;
    };
    if let Some(document) = slot.pending.take() {
        if let Some(model) = ensure_model(&monaco, &document.uri, &document.language, &document.text)
        {
            // A re-read of the same file is brought up to date before it is
            // shown; merely switching tabs attaches the model as it is, so an
            // unsaved edit survives.
            if document.replace && model_text(&model).as_deref() != Some(document.text.as_str()) {
                set_model_value(&model, &document.text);
            }
            if editor_model_uri(&handle).as_deref() != Some(document.uri.as_str()) {
                set_editor_model(&handle, &model);
            }
        }
    }
    if let Some((line, column)) = slot.reveal.take() {
        reveal_position_js(&handle, line, column);
    }
}

/// `editor.onMouseDown(...)`, filtered down to "plain click on an identifier".
///
/// The returned closure must be kept alive; it is stored next to the editor
/// handle and dropped when the editor is disposed.
fn install_click_listener(
    editor: &JsValue,
    on_click: Box<dyn Fn(u32, u32, String) + 'static>,
) -> Closure<dyn FnMut(JsValue)> {
    let editor_for_click = editor.clone();
    let listener = Closure::wrap(Box::new(move |event: JsValue| {
        let Some((line, column)) = click_position(&event) else {
            return;
        };
        // the identifier may start before the clicked column, and a "what is
        // here" query wants the start
        let Some((start, word)) = word_at(&editor_for_click, line, column) else {
            return;
        };
        on_click(line, start, word);
    }) as Box<dyn FnMut(JsValue)>);

    if let Some(on_mouse_down) = js_function(editor, "onMouseDown") {
        let _ = on_mouse_down.call1(editor, listener.as_ref().unchecked_ref());
    }
    listener
}

/// `editor.onDidChangeModelContent(...)`, reported as `(model URI, text)`.
///
/// The returned closure must be kept alive; it is stored next to the editor
/// handle and dropped when the editor is disposed.
fn install_change_listener(
    editor: &JsValue,
    callback: Box<dyn Fn(String, String)>,
) -> Closure<dyn FnMut(JsValue)> {
    let editor_for_change = editor.clone();
    let listener = Closure::wrap(Box::new(move |_event: JsValue| {
        let Some(get_model) = js_function(&editor_for_change, "getModel") else {
            return;
        };
        let Ok(model) = get_model.call0(&editor_for_change) else {
            return;
        };
        if !model.is_object() {
            return;
        }
        if let (Some(uri), Some(text)) = (model_uri(&model), model_text(&model)) {
            callback(uri, text);
        }
    }) as Box<dyn FnMut(JsValue)>);

    if let Some(on_change) = js_function(editor, "onDidChangeModelContent") {
        let _ = on_change.call1(editor, listener.as_ref().unchecked_ref());
    }
    listener
}

/// Line and column of an `IEditorMouseEvent`, `None` for anything that is not a
/// plain left click on text.
fn click_position(event: &JsValue) -> Option<(u32, u32)> {
    let mouse = Reflect::get(event, &JsValue::from_str("event")).ok()?;
    if mouse.is_object() {
        // monaco's `IMouseEvent` reports the buttons as booleans
        // (`leftButton`/`rightButton`/`middleButton`) and carries the modifier
        // state itself; the DOM `button` number is not part of it. Requiring
        // `leftButton` is what keeps a right click (which opens monaco's
        // context menu) from also being taken for an "open this name" gesture.
        let flag = |key: &str| {
            Reflect::get(&mouse, &JsValue::from_str(key))
                .ok()
                .and_then(|value| value.as_bool())
        };
        let right_or_middle = flag("rightButton") == Some(true) || flag("middleButton") == Some(true);
        if right_or_middle || flag("leftButton") == Some(false) {
            return None;
        }
        for modifier in ["ctrlKey", "altKey", "metaKey", "shiftKey"] {
            if flag(modifier) == Some(true) {
                // keep multi cursor, column selection and drag and drop usable
                return None;
            }
        }
    }

    let target = Reflect::get(event, &JsValue::from_str("target")).ok()?;
    let position = Reflect::get(&target, &JsValue::from_str("position")).ok()?;
    let field = |key: &str| {
        Reflect::get(&position, &JsValue::from_str(key))
            .ok()
            .and_then(|value| value.as_f64())
    };
    let line = field("lineNumber")? as u32;
    let column = field("column")? as u32;
    Some((line.max(1), column.max(1)))
}

/// `(start column, word)` of the identifier at `line:column`.
fn word_at(editor: &JsValue, line: u32, column: u32) -> Option<(u32, String)> {
    let model = js_function(editor, "getModel")?.call0(editor).ok()?;
    if !model.is_object() {
        return None;
    }
    let get_word = js_function(&model, "getWordAtPosition")?;
    let position = Object::new();
    set(&position, "lineNumber", JsValue::from_f64(f64::from(line)));
    set(&position, "column", JsValue::from_f64(f64::from(column)));

    let word = get_word.call1(&model, &position).ok()?;
    if !word.is_object() {
        return None;
    }
    let text = Reflect::get(&word, &JsValue::from_str("word"))
        .ok()?
        .as_string()?;
    let start = Reflect::get(&word, &JsValue::from_str("startColumn"))
        .ok()
        .and_then(|value| value.as_f64())
        .map(|value| value as u32)
        .filter(|value| *value > 0)
        .unwrap_or(column);
    Some((start, text))
}

/// Move the cursor to `line:col` and scroll there.
fn reveal_position_js(editor: &JsValue, line: u32, column: u32) {
    let line = line.max(1);
    let position = Object::new();
    set(&position, "lineNumber", JsValue::from_f64(f64::from(line)));
    set(
        &position,
        "column",
        JsValue::from_f64(f64::from(column.max(1))),
    );

    if let Some(set_position) = js_function(editor, "setPosition") {
        let _ = set_position.call1(editor, &position);
    }
    // a plain line request keeps the horizontal scroll where the user left it
    let reveal = if column > 1 {
        js_function(editor, "revealPositionInCenter")
    } else {
        js_function(editor, "revealLineInCenter")
    };
    if let Some(reveal) = reveal {
        if column > 1 {
            let _ = reveal.call1(editor, &position);
        } else {
            let _ = reveal.call1(editor, &JsValue::from_f64(f64::from(line)));
        }
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
