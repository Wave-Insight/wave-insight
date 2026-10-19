//! The four monaco language providers, backed by the browser LSP client.
//!
//! Installation happens once per page, on the `systemverilog` language, through
//! [`MonacoEditor::on_ready`]: monaco is loaded lazily from a CDN, so the
//! registration waits for the `monaco` namespace instead of assuming it.
//!
//! Every provider body is a Rust closure: it turns monaco's `(model, position)`
//! into a design-source position, asks [`LspBridge`], and hands monaco a
//! resolved `Promise` of the plain LSP shape. Nothing is thrown back into JS —
//! a query that cannot be answered yields `null` (or an empty array), which is
//! exactly what monaco shows as "no definition / no references".
//!
//! Because the source pane shows one model per file, a provider result that
//! points into a file whose model was never created asks the glue for that
//! model first; monaco can then open the range itself. The tab strip is kept in
//! sync by [`LspBridge::jump`], so jumping to a definition in another file
//! switches the visible tab instead of leaving monaco and the UI disagreeing.

use std::cell::Cell;

use js_sys::{Array, Function, Object, Promise, Reflect};
use wasm_bindgen::closure::Closure;
use wasm_bindgen::{JsCast, JsValue};

use crate::code_reader::monaco_glue::{ensure_model, model_uri, parse_uri, MonacoEditor};

use super::bridge::{
    document_uri, file_of_uri, parse_range, Location, LspBridge, Position, Range, LANGUAGE_ID,
};

// Providers live as long as the page; registering twice would answer twice.
thread_local! {
    static INSTALLED: Cell<bool> = const { Cell::new(false) };
}

/// Register definition / references / hover / documentSymbol for
/// `systemverilog`, once the editor (and therefore monaco) is alive.
pub fn install(editor: &MonacoEditor, bridge: LspBridge) {
    editor.on_ready(Box::new(move |monaco, _editor| {
        let first = INSTALLED.with(|installed| !installed.replace(true));
        if !first {
            return;
        }
        register(monaco, bridge.clone());
    }));
}

fn register(monaco: &JsValue, bridge: LspBridge) {
    let Ok(languages) = Reflect::get(monaco, &JsValue::from_str("languages")) else {
        return;
    };
    if !languages.is_object() {
        return;
    }

    let definition = Object::new();
    let definition_provider = {
        let bridge = bridge.clone();
        let monaco = monaco.clone();
        Closure::wrap(Box::new(
            move |model: JsValue, position: JsValue, _token: JsValue| -> JsValue {
                Promise::resolve(&provide_definition(&bridge, &monaco, &model, &position)).into()
            },
        ) as Box<dyn FnMut(JsValue, JsValue, JsValue) -> JsValue>)
    };
    set(
        &definition,
        "provideDefinition",
        definition_provider.as_ref().unchecked_ref::<Function>().into(),
    );
    definition_provider.forget();

    let references = Object::new();
    let references_provider = {
        let bridge = bridge.clone();
        let monaco = monaco.clone();
        Closure::wrap(Box::new(
            move |model: JsValue,
                  position: JsValue,
                  _context: JsValue,
                  _token: JsValue|
                  -> JsValue {
                Promise::resolve(&provide_references(&bridge, &monaco, &model, &position)).into()
            },
        ) as Box<dyn FnMut(JsValue, JsValue, JsValue, JsValue) -> JsValue>)
    };
    set(
        &references,
        "provideReferences",
        references_provider.as_ref().unchecked_ref::<Function>().into(),
    );
    references_provider.forget();

    let hover = Object::new();
    let hover_provider = {
        let bridge = bridge.clone();
        Closure::wrap(Box::new(
            move |model: JsValue, position: JsValue, _token: JsValue| -> JsValue {
                Promise::resolve(&provide_hover(&bridge, &model, &position)).into()
            },
        ) as Box<dyn FnMut(JsValue, JsValue, JsValue) -> JsValue>)
    };
    set(
        &hover,
        "provideHover",
        hover_provider.as_ref().unchecked_ref::<Function>().into(),
    );
    hover_provider.forget();

    let symbols = Object::new();
    let symbols_provider = {
        let bridge = bridge.clone();
        Closure::wrap(Box::new(move |model: JsValue, _token: JsValue| -> JsValue {
            Promise::resolve(&provide_document_symbols(&bridge, &model)).into()
        }) as Box<dyn FnMut(JsValue, JsValue) -> JsValue>)
    };
    set(
        &symbols,
        "provideDocumentSymbols",
        symbols_provider.as_ref().unchecked_ref::<Function>().into(),
    );
    symbols_provider.forget();

    attach(&languages, "registerDefinitionProvider", &definition);
    attach(&languages, "registerReferenceProvider", &references);
    attach(&languages, "registerHoverProvider", &hover);
    attach(&languages, "registerDocumentSymbolProvider", &symbols);
    publish(definition, references, hover, symbols);
}

/// Attach one provider object to the language.
///
/// The JS closure is `forget`-ed by the caller: monaco keeps the function for
/// the lifetime of the page and never unregisters it.
fn attach(languages: &JsValue, name: &str, provider: &Object) {
    if let Some(function) = js_function(languages, name) {
        let provider: JsValue = provider.clone().into();
        let _ = function.call2(languages, &JsValue::from_str(LANGUAGE_ID), &provider);
    }
}

// ---------------------------------------------------------------------------
// Provider bodies
// ---------------------------------------------------------------------------

fn provide_definition(
    bridge: &LspBridge,
    monaco: &JsValue,
    model: &JsValue,
    position: &JsValue,
) -> JsValue {
    let Some((file, line, column)) = editor_position(model, position) else {
        return JsValue::NULL;
    };
    let Some(location) = bridge.definition(&file, line, column) else {
        return JsValue::NULL;
    };
    // The tab strip follows the jump; monaco then reveals the range itself. A
    // target whose buffer is gone (a closed tab) is not handed to monaco at
    // all: it has no model to open, while the status line still explains it.
    bridge.jump(&location);
    if ensure_target_model(monaco, bridge, &location.uri).is_none() {
        return JsValue::NULL;
    }
    location_js(monaco, &location)
}

fn provide_references(
    bridge: &LspBridge,
    monaco: &JsValue,
    model: &JsValue,
    position: &JsValue,
) -> JsValue {
    let Some((file, line, column)) = editor_position(model, position) else {
        return Array::new().into();
    };
    let locations = bridge.references(&file, line, column);
    let array = Array::new();
    for location in &locations {
        // Peek references opens every hit in a second editor, which needs a
        // model for that document even if its tab was never shown.
        if ensure_target_model(monaco, bridge, &location.uri).is_none() {
            continue;
        }
        array.push(&location_js(monaco, location));
    }
    array.into()
}

fn provide_hover(bridge: &LspBridge, model: &JsValue, position: &JsValue) -> JsValue {
    let Some((file, line, column)) = editor_position(model, position) else {
        return JsValue::NULL;
    };
    let Some((text, range)) = bridge.hover(&file, line, column) else {
        return JsValue::NULL;
    };
    let contents = Object::new();
    set(&contents, "value", JsValue::from_str(&text));
    let result = Object::new();
    set(&result, "contents", Array::of1(&contents.into()).into());
    if let Some(range) = range {
        set(&result, "range", range_js(&range));
    }
    result.into()
}

fn provide_document_symbols(bridge: &LspBridge, model: &JsValue) -> JsValue {
    let Some(uri) = model_uri(model) else {
        return JsValue::NULL;
    };
    let Some(file) = file_of_uri(&uri) else {
        return JsValue::NULL;
    };
    let symbols = bridge.symbols(&file);
    let array = Array::new();
    for symbol in &symbols {
        array.push(&symbol_js(symbol));
    }
    array.into()
}

// ---------------------------------------------------------------------------
// monaco ↔ LSP conversion
// ---------------------------------------------------------------------------

/// `(design source, 1-based line, 1-based column)` of a provider call.
///
/// The model URI is the source of truth: the source pane keeps one model per
/// file, all created by [`crate::code_reader::monaco_glue`] with
/// [`document_uri`], so a model that has no design source behind it (monaco's
/// own `inmemory://model/1`) simply answers "nothing here".
fn editor_position(model: &JsValue, position: &JsValue) -> Option<(String, u32, u32)> {
    let uri = model_uri(model)?;
    let file = file_of_uri(&uri)?;
    let line = number(position, "lineNumber")?;
    let column = number(position, "column")?;
    Some((file, line.max(1), column.max(1)))
}

fn number(object: &JsValue, key: &str) -> Option<u32> {
    Reflect::get(object, &JsValue::from_str(key))
        .ok()?
        .as_f64()
        .map(|value| value.max(1.0) as u32)
}

fn ensure_target_model(monaco: &JsValue, bridge: &LspBridge, uri: &str) -> Option<JsValue> {
    let file = file_of_uri(uri)?;
    if !bridge.is_open(&file) {
        // No buffer behind the source (a closed tab), so nothing to show.
        return None;
    }
    let text = bridge.document_text(&file)?;
    ensure_model(monaco, uri, LANGUAGE_ID, &text)
}

fn location_js(monaco: &JsValue, location: &Location) -> JsValue {
    let object = Object::new();
    let uri = parse_uri(monaco, &location.uri)
        .unwrap_or_else(|| JsValue::from_str(location.uri.as_str()));
    set(&object, "uri", uri);
    set(&object, "range", range_js(&location.range));
    object.into()
}

fn range_js(range: &Range) -> JsValue {
    let object = Object::new();
    set(&object, "startLineNumber", line_number(range.start));
    set(&object, "startColumn", column_number(range.start));
    set(&object, "endLineNumber", line_number(range.end));
    set(&object, "endColumn", column_number(range.end));
    object.into()
}

fn line_number(position: Position) -> JsValue {
    JsValue::from_f64(f64::from(position.line.saturating_add(1)))
}

fn column_number(position: Position) -> JsValue {
    JsValue::from_f64(f64::from(position.character.saturating_add(1)))
}

/// One LSP `DocumentSymbol` → the object monaco's outline consumes.
///
/// The two protocols number `SymbolKind` differently (LSP starts at `Text = 1`,
/// monaco at `File = 0`), so the kinds this server emits are translated instead
/// of being passed through.
fn symbol_js(symbol: &serde_json::Value) -> JsValue {
    let object = Object::new();
    let name = symbol
        .get("name")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    set(&object, "name", JsValue::from_str(name));
    if let Some(detail) = symbol.get("detail").and_then(serde_json::Value::as_str) {
        set(&object, "detail", JsValue::from_str(detail));
    }
    let kind = symbol
        .get("kind")
        .and_then(serde_json::Value::as_i64)
        .unwrap_or(LSP_VARIABLE);
    set(&object, "kind", JsValue::from_f64(symbol_kind(kind)));
    if let Some(range) = symbol.get("range").and_then(parse_range) {
        set(&object, "range", range_js(&range));
    }
    if let Some(range) = symbol.get("selectionRange").and_then(parse_range) {
        set(&object, "selectionRange", range_js(&range));
    }
    if let Some(children) = symbol.get("children").and_then(serde_json::Value::as_array) {
        let array = Array::new();
        for child in children {
            array.push(&symbol_js(child));
        }
        set(&object, "children", array.into());
    }
    object.into()
}

// LSP `SymbolKind` values this server sends (see `lsp::server`).
const LSP_MODULE: i64 = 2;
const LSP_FIELD: i64 = 8;
const LSP_VARIABLE: i64 = 13;
const LSP_CONSTANT: i64 = 14;
const LSP_OBJECT: i64 = 19;

/// LSP `SymbolKind` → `monaco.languages.SymbolKind`.
fn symbol_kind(kind: i64) -> f64 {
    match kind {
        LSP_MODULE => 1.0,   // Module
        LSP_FIELD => 7.0,    // Field
        LSP_VARIABLE => 12.0, // Variable
        LSP_CONSTANT => 13.0, // Constant
        LSP_OBJECT => 18.0,  // Object
        other => other as f64,
    }
}

// ---------------------------------------------------------------------------
// Test seam
// ---------------------------------------------------------------------------

/// Publish the registered providers as `window.__waveInsightProviders`.
///
/// monaco has no outline or reference list of its own outside VS Code, so the
/// headless driver calls these very functions (the ones monaco dispatches to)
/// to inspect definition, hover, references and documentSymbol results
/// directly.
fn publish(
    definition: Object,
    references: Object,
    hover: Object,
    symbols: Object,
) {
    let Some(window) = web_sys::window() else {
        return;
    };
    let hook = Object::new();
    set(&hook, "definition", definition.into());
    set(&hook, "references", references.into());
    set(&hook, "hover", hover.into());
    set(&hook, "documentSymbol", symbols.into());

    let to_uri = Closure::<dyn Fn(String) -> String>::new(|file: String| document_uri(&file));
    set(&hook, "documentUri", to_uri.as_ref().unchecked_ref::<Function>().into());
    to_uri.forget();

    let to_file = Closure::<dyn Fn(String) -> String>::new(|uri: String| {
        file_of_uri(&uri).unwrap_or_default()
    });
    set(&hook, "fileOf", to_file.as_ref().unchecked_ref::<Function>().into());
    to_file.forget();

    let _ = Reflect::set(&window, &JsValue::from_str("__waveInsightProviders"), &hook);
}

fn js_function(target: &JsValue, name: &str) -> Option<Function> {
    Reflect::get(target, &JsValue::from_str(name))
        .ok()?
        .dyn_into()
        .ok()
}

fn set(object: &JsValue, key: &str, value: JsValue) {
    let _ = Reflect::set(object, &JsValue::from_str(key), &value);
}
