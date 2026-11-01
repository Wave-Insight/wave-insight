//! The sidebar's debug seam for the virtualized lists.
//!
//! The module tree and the signal list only draw the rows inside their scroll
//! window now, so the QA matrix cannot count `.tree-row` / `.sig-row` nodes to
//! learn what the sidebar shows. This seam carries the *logical* lists instead
//! (exactly the rows the old, unvirtualized DOM held), plus the size of the
//! rendered window, so an assertion can keep checking "the list is complete"
//! and at the same time prove that the DOM stays bounded:
//!
//! ```js
//! window.__waveInsightSidebar = {
//!   modules: ['top', 'u_cpu', ...],   // expanded tree rows, display order
//!   modules_total: 121,               // scopes in the whole tree
//!   modules_dom: 33,                  // .tree-row nodes in the DOM (the window)
//!   modules_windowed: true,
//!   signals: ['clk', 'pc[7:0]', ...], // filtered list of the selected scope
//!   signals_total: 2100,              // signals declared in that scope
//!   signals_dom: 21,                  // .sig-row nodes in the DOM
//!   signals_windowed: true,
//! }
//! ```
//!
//! The arrays are only rewritten when the logical list changes (a scope is
//! expanded, the selection moves, the filter changes); the window sizes are
//! refreshed after every render. Rebuilding a 20k-name array on every scroll
//! frame would defeat the point of the virtualization.

use wasm_bindgen::{JsCast, JsValue};

/// Name of the seam object on `window`.
const KEY: &str = "__waveInsightSidebar";

/// The seam object, created on first use.
fn sidebar() -> Option<js_sys::Object> {
    let window = web_sys::window()?;
    let key = JsValue::from_str(KEY);
    if let Ok(existing) = js_sys::Reflect::get(&window, &key) {
        if existing.is_object() {
            return Some(existing.unchecked_into());
        }
    }
    let object = js_sys::Object::new();
    js_sys::Reflect::set(&window, &key, &object).ok()?;
    Some(object)
}

fn set(object: &js_sys::Object, key: &str, value: &JsValue) {
    let _ = js_sys::Reflect::set(object, &JsValue::from_str(key), value);
}

fn set_number(object: &js_sys::Object, key: &str, value: usize) {
    set(object, key, &JsValue::from_f64(value as f64));
}

fn set_bool(object: &js_sys::Object, key: &str, value: bool) {
    set(object, key, &JsValue::from_bool(value));
}

/// Publish the whole module tree list plus its window size.
///
/// Called only when the list changed: an expand, a collapse (the tree only
/// grows, like before) or a new tree.
pub fn publish_modules<'a>(
    names: impl IntoIterator<Item = &'a str>,
    total_scopes: usize,
    dom_rows: usize,
    windowed: bool,
) {
    let Some(sidebar) = sidebar() else {
        return;
    };
    let array = js_sys::Array::new();
    for name in names {
        array.push(&JsValue::from_str(name));
    }
    set(&sidebar, "modules", &array.into());
    set_number(&sidebar, "modules_total", total_scopes);
    set_number(&sidebar, "modules_dom", dom_rows);
    set_bool(&sidebar, "modules_windowed", windowed);
}

/// Refresh the module window size after a render (no array rebuild).
pub fn publish_module_window(dom_rows: usize, windowed: bool) {
    let Some(sidebar) = sidebar() else {
        return;
    };
    set_number(&sidebar, "modules_dom", dom_rows);
    set_bool(&sidebar, "modules_windowed", windowed);
}

/// Publish the whole signal list (already filtered by the search bar) plus its
/// window size.
pub fn publish_signals<'a>(
    names: impl IntoIterator<Item = &'a str>,
    total_signals: usize,
    dom_rows: usize,
    windowed: bool,
) {
    let Some(sidebar) = sidebar() else {
        return;
    };
    let array = js_sys::Array::new();
    for name in names {
        array.push(&JsValue::from_str(name));
    }
    set(&sidebar, "signals", &array.into());
    set_number(&sidebar, "signals_total", total_signals);
    set_number(&sidebar, "signals_dom", dom_rows);
    set_bool(&sidebar, "signals_windowed", windowed);
}

/// Refresh the signal window size after a render (no array rebuild).
pub fn publish_signal_window(dom_rows: usize, windowed: bool) {
    let Some(sidebar) = sidebar() else {
        return;
    };
    set_number(&sidebar, "signals_dom", dom_rows);
    set_bool(&sidebar, "signals_windowed", windowed);
}

/// Clear both lists: the sidebar is unmounted (the drawer was closed) or has
/// nothing to show, so the seam must not keep describing a stale sidebar.
pub fn clear() {
    publish_modules(std::iter::empty(), 0, 0, false);
    publish_signals(std::iter::empty(), 0, 0, false);
}
