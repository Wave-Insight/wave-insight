//! Cursor values as monaco inlay hints.
//!
//! After every identifier that resolves to exactly one elaborated signal, the
//! provider puts the signal's value at cursor 1 (e.g. `= 0x2a`) right behind the
//! name — the Verdi inlay hint the user asked for.
//!
//! Everything here is page state, not component state: the hints are answered
//! from a monaco callback (which has no Yew context) and must not re-render
//! anything when the cursor moves. `App` updates the [`Context`] when the dump or
//! the cursor changes, and the provider fires `onDidChangeInlayHints`, which makes
//! monaco ask again for the visible lines (no round trip per keystroke; the
//! values come from the same decoded waveform the canvas uses).
//!
//! # Which signal does a word mean?
//!
//! A source position can belong to a module body instantiated several times
//! (`u_cpu.pc_reg` and `u_cpu16.pc_reg` share `cpu.sv`), so the position alone
//! does not name a signal. If the user already added one of those instances to
//! the wave view, that instance wins — the hint then shows the value of the row
//! they are looking at. Otherwise several candidates are *not* guessed: the
//! identifier simply gets no hint.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use js_sys::{Array, Function, Object, Reflect};
use wasm_bindgen::closure::Closure;
use wasm_bindgen::{JsCast, JsValue};

use wave_insight_lib::data_struct::ShowType;
use wave_insight_lib::waveform::Waveform;

use crate::code_reader::monaco_glue::model_uri;
use crate::nav::{value_text, SharedDesign};
use crate::state::{select, DumpHandle, Store};

use super::bridge::file_of_uri;

/// Never answer more than this many hints for one request; the visible range is
/// small, this is only a guard against a pathological model.
const MAX_HINTS: usize = 300;
/// Never scan more lines than this, whatever range monaco asks for.
const MAX_LINES: u32 = 400;

/// Page-wide state the provider reads.
struct Context {
    design: SharedDesign,
    store: Store,
    waveform: Option<DumpHandle>,
    /// Cursor 1, in wave time units; `None` means no hints at all.
    cursor: Option<i64>,
    emitter: Emitter,
}

thread_local! {
    static CONTEXT: RefCell<Option<Context>> = const { RefCell::new(None) };
}

/// Install the page-wide context. Called once from `App::create`.
pub fn init(design: SharedDesign, store: Store) {
    CONTEXT.with(|context| {
        let mut context = context.borrow_mut();
        if context.is_none() {
            *context = Some(Context {
                design,
                store,
                waveform: None,
                cursor: None,
                emitter: Emitter::new(),
            });
        }
    });
}

/// The opened dump changed (fired on `GetWaveform`).
pub fn set_waveform(waveform: Option<DumpHandle>) {
    CONTEXT.with(|context| {
        if let Some(context) = context.borrow_mut().as_mut() {
            context.waveform = waveform;
            context.emitter.fire();
        }
    });
}

/// Cursor 1 moved (or was cleared): refresh the visible hints.
pub fn set_cursor(cursor: Option<i64>) {
    CONTEXT.with(|context| {
        if let Some(context) = context.borrow_mut().as_mut() {
            if context.cursor != cursor {
                context.cursor = cursor;
                context.emitter.fire();
            }
        }
    });
}

/// Sources were parsed: names that resolved to nothing may resolve now.
pub fn refresh() {
    CONTEXT.with(|context| {
        if let Some(context) = context.borrow().as_ref() {
            context.emitter.fire();
        }
    });
}

/// The `IEvent<void>` monaco's provider API expects for
/// `onDidChangeInlayHints`, registered once by the provider installer.
pub fn change_event() -> Option<Function> {
    CONTEXT.with(|context| {
        context
            .borrow()
            .as_ref()
            .map(|context| context.emitter.event())
    })
}

/// How far one provider call got, for the QA seam and for debugging.
#[derive(Default)]
struct Counters {
    identifiers: usize,
    /// identifiers that the design resolved to at least one candidate
    candidates: usize,
    /// identifiers that resolved to exactly one signal (or one visible row)
    chosen: usize,
    /// chosen signals that the dump also holds
    dumps: usize,
    /// chosen signals with a value at the cursor
    values: usize,
}

/// `provideInlayHints(model, range, token)` → `{ hints, dispose }`.
///
/// monaco 0.44's [`InlayHintsProvider`] returns an `InlayHints` object (the
/// disposable is added to the editor's store), not a bare array; newer versions
/// accept the array directly, which this never has to rely on.
pub fn provide(model: &JsValue, range: &JsValue) -> JsValue {
    let hints = Array::new();
    let mut counters = Counters::default();
    let (has_context, cursor, lines) = fill_hints(model, range, &hints, &mut counters);
    publish_debug(has_context, cursor, lines, &counters, hints.length() as usize);

    let result = Object::new();
    set(&result, "hints", hints.into());
    // the disposable monaco stores next to the hints; nothing to release, the
    // decoded waveform is owned by the dump cache
    let dispose = Closure::once_into_js(|| {});
    set(&result, "dispose", dispose);
    result.into()
}

/// Fill `hints` and return `(has context, cursor, lines scanned)`.
fn fill_hints(
    model: &JsValue,
    range: &JsValue,
    hints: &Array,
    counters: &mut Counters,
) -> (bool, Option<i64>, u32) {
    let Some((file, first, last)) = scan_range(model, range) else {
        return (false, None, 0);
    };
    let Some((cursor, design, waveform, store)) = borrow_parts() else {
        return (false, None, 0);
    };
    let Some(cursor) = cursor else {
        return (true, None, last - first + 1);
    };
    let Some(waveform) = waveform else {
        return (true, Some(cursor), last - first + 1);
    };

    // The row settings (base formats) and the row paths (used to disambiguate a
    // shared module body) are read once per request, never per identifier.
    let rows = store.select(select::rows);
    let mut formats: HashMap<&str, ShowType> = HashMap::new();
    for row in rows.items.iter() {
        formats.insert(row.path.as_str(), row.setting.show_type.clone());
    }

    for line in first..=last {
        let Some(text) = model_line(model, line) else {
            continue;
        };
        let found = identifiers(&text);
        counters.identifiers += found.len();
        for (start, end, word) in found {
            if hints.length() as usize >= MAX_HINTS {
                return (true, Some(cursor), last - first + 1);
            }
            if let Some(hint) = hint_at(
                &design,
                waveform.waveform(),
                &formats,
                &file,
                line,
                start,
                end,
                &word,
                cursor,
                counters,
            ) {
                hints.push(&hint);
            }
        }
    }
    (true, Some(cursor), last - first + 1)
}

/// One hint for one identifier, if it names exactly one signal the dump holds and
/// the cursor is inside its history.
#[allow(clippy::too_many_arguments)]
fn hint_at(
    design: &SharedDesign,
    waveform: &Waveform,
    formats: &HashMap<&str, ShowType>,
    file: &str,
    line: u32,
    start: u32,
    end: u32,
    word: &str,
    cursor: i64,
    counters: &mut Counters,
) -> Option<JsValue> {
    if word.is_empty() || cursor < 0 {
        return None;
    }
    let candidates = {
        let design = design.borrow();
        let exact = design.signals_at(file, line, start);
        if exact.is_empty() {
            design.signals_matching(word)
        } else {
            exact
        }
    };
    if candidates.is_empty() {
        return None;
    }
    counters.candidates += 1;
    let design_path = pick_candidate(waveform, formats, &candidates)?;
    counters.chosen += 1;

    // the dump spells the root scope differently: resolve it like a row does
    let dump_candidates = crate::nav::match_dump_candidates(waveform, &design_path);
    if dump_candidates.len() != 1 {
        return None;
    }
    let dump_path = &dump_candidates[0];
    let info = waveform.signal_by_path(dump_path)?;
    counters.dumps += 1;
    let width = info.width().max(1) as usize;
    let value = waveform
        .value_at(info.index(), cursor as u64)
        .ok()
        .flatten()?;
    counters.values += 1;
    let show = formats
        .get(dump_path.as_str())
        .cloned()
        .unwrap_or(ShowType::Hex);
    let text = value_text(&value, width, &show)?;

    let position = Object::new();
    set_f64(&position, "lineNumber", f64::from(line));
    set_f64(&position, "column", f64::from(end.max(start + 1)));
    let hint = Object::new();
    set(&hint, "position", position.into());
    set(&hint, "label", JsValue::from_str(&format!("= {text}")));
    // monaco's InlayHintKind: Type = 1 (a value annotation)
    set_f64(&hint, "kind", 1.0);
    set(&hint, "paddingLeft", JsValue::TRUE);
    Some(hint.into())
}

/// Which candidate a source position means.
///
/// One candidate is the answer. Several candidates come from a module body
/// instantiated more than once; a row the user already put in the wave view
/// disambiguates it, and if that is still ambiguous nothing is shown — the hint
/// must never show the value of a random instance.
fn pick_candidate(
    waveform: &Waveform,
    formats: &HashMap<&str, ShowType>,
    candidates: &[String],
) -> Option<String> {
    if candidates.len() == 1 {
        return Some(candidates[0].clone());
    }
    let mut chosen: Option<String> = None;
    for candidate in candidates {
        let dumps = crate::nav::match_dump_candidates(waveform, candidate);
        if dumps.len() != 1 || !formats.contains_key(dumps[0].as_str()) {
            continue;
        }
        if chosen.is_some() {
            // two instances of the same body are both on screen: ambiguous
            return None;
        }
        chosen = Some(candidate.clone());
    }
    chosen
}

/// Test seam: how far the last provider call got
/// (`window.__waveInlayDebug`), used by the headless browser driver.
fn publish_debug(
    has_context: bool,
    cursor: Option<i64>,
    lines: u32,
    counters: &Counters,
    hints: usize,
) {
    let Some(window) = web_sys::window() else {
        return;
    };
    let debug = Object::new();
    set(&debug, "hasContext", JsValue::from_bool(has_context));
    set(
        &debug,
        "cursor",
        cursor.map_or(JsValue::NULL, |cursor| JsValue::from_f64(cursor as f64)),
    );
    set_f64(&debug, "lines", f64::from(lines));
    set_f64(&debug, "identifiers", counters.identifiers as f64);
    set_f64(&debug, "candidates", counters.candidates as f64);
    set_f64(&debug, "chosen", counters.chosen as f64);
    set_f64(&debug, "dumps", counters.dumps as f64);
    set_f64(&debug, "values", counters.values as f64);
    set_f64(&debug, "hints", hints as f64);
    let _ = Reflect::set(&window, &JsValue::from_str("__waveInlayDebug"), &debug);
}

/// Clone the parts the provider needs out of the thread local: no borrow is
/// held while JS or the design database is used.
fn borrow_parts() -> Option<(Option<i64>, SharedDesign, Option<DumpHandle>, Store)> {
    CONTEXT.with(|context| {
        let context = context.borrow();
        let context = context.as_ref()?;
        Some((
            context.cursor,
            context.design.clone(),
            context.waveform.clone(),
            context.store.clone(),
        ))
    })
}

/// `(file, first line, last line)` of a provider call, clamped to the model.
fn scan_range(model: &JsValue, range: &JsValue) -> Option<(String, u32, u32)> {
    let uri = model_uri(model)?;
    // a model that is not a design source (monaco's own scratch model) has no
    // hints: `file_of_uri` refuses anything that is not a `wave-insight://source`
    // document
    let file = file_of_uri(&uri)?;
    if file.is_empty() {
        return None;
    }
    let line_count = js_function(model, "getLineCount")
        .and_then(|function| function.call0(model).ok())
        .and_then(|value| value.as_f64())
        .map(|value| value.max(1.0) as u32)
        .unwrap_or(1);
    let start = Reflect::get(range, &JsValue::from_str("startLineNumber"))
        .ok()
        .and_then(|value| value.as_f64())
        .map(|value| (value.max(1.0) as u32).min(line_count))
        .unwrap_or(1);
    let mut end = Reflect::get(range, &JsValue::from_str("endLineNumber"))
        .ok()
        .and_then(|value| value.as_f64())
        .map(|value| (value.max(1.0) as u32).min(line_count))
        .unwrap_or(line_count);
    end = end.max(start).min(start.saturating_add(MAX_LINES - 1));
    Some((file, start, end))
}

fn model_line(model: &JsValue, line: u32) -> Option<String> {
    let get_line = js_function(model, "getLineContent")?;
    get_line
        .call1(model, &JsValue::from_f64(f64::from(line)))
        .ok()?
        .as_string()
}

/// `(start column, one-past-end column, word)` of every identifier in a line.
///
/// SystemVerilog identifiers start with a letter or `_` and continue with
/// letters, digits, `_` and `$`; escaped identifiers (`\name `) are skipped, they
/// are rare in the sources this viewer is used on and would need the model's
/// tokenizer to be right.
pub(crate) fn identifiers(line: &str) -> Vec<(u32, u32, String)> {
    let mut out = Vec::new();
    let mut chars = line.char_indices().peekable();
    while let Some((index, character)) = chars.next() {
        if !(character.is_ascii_alphabetic() || character == '_') {
            continue;
        }
        let mut word = String::new();
        word.push(character);
        let mut end = index + character.len_utf8();
        while let Some(&(next_index, next)) = chars.peek() {
            if next.is_ascii_alphanumeric() || next == '_' || next == '$' {
                word.push(next);
                end = next_index + next.len_utf8();
                chars.next();
            } else {
                break;
            }
        }
        let start_col = line[..index].chars().count() as u32 + 1;
        let end_col = line[..end].chars().count() as u32 + 1;
        out.push((start_col, end_col, word));
    }
    out
}

// ---------------------------------------------------------------------------
// onDidChangeInlayHints: a tiny JS event emitter
// ---------------------------------------------------------------------------

/// An `IEvent<void>` monaco can listen to.
///
/// monaco has no Rust event loop to hook into: the provider object needs a
/// function `(listener) => IDisposable` it can subscribe to, and we call every
/// listener when the cursor or the dump changed.
pub struct Emitter {
    listeners: Rc<RefCell<Vec<Function>>>,
}

impl Emitter {
    fn new() -> Self {
        Self {
            listeners: Rc::new(RefCell::new(Vec::new())),
        }
    }

    /// The `onDidChangeInlayHints` function of the provider object.
    fn event(&self) -> Function {
        let listeners = Rc::clone(&self.listeners);
        let subscribe = Closure::wrap(Box::new(move |listener: Function| -> JsValue {
            listeners.borrow_mut().push(listener);
            let disposable = Object::new();
            // monaco may dispose the listener; a page-long editor keeps it
            // alive, so the no-op is enough
            let noop = Closure::once_into_js(|| {});
            set(&disposable, "dispose", noop);
            disposable.into()
        }) as Box<dyn FnMut(Function) -> JsValue>);
        subscribe
            .into_js_value()
            .dyn_into::<Function>()
            .expect("a Closure is a Function")
    }

    /// Ask monaco to request the visible hints again.
    fn fire(&self) {
        let listeners: Vec<Function> = self.listeners.borrow().clone();
        for listener in listeners {
            let _ = listener.call0(&JsValue::NULL);
        }
    }
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

fn set_f64(object: &Object, key: &str, value: f64) {
    set(object, key, JsValue::from_f64(value));
}

#[cfg(test)]
mod tests {
    use super::identifiers;

    #[test]
    fn identifiers_are_found_with_their_columns() {
        let found = identifiers("    assign next_pc = pc_reg + 1'b1;");
        let words: Vec<&str> = found.iter().map(|(_, _, word)| word.as_str()).collect();
        assert_eq!(words, vec!["assign", "next_pc", "pc_reg", "b1"]);
        assert_eq!(found[1], (12, 19, "next_pc".to_string()));
        assert_eq!(found[2], (22, 28, "pc_reg".to_string()));
    }

    #[test]
    fn numbers_and_punctuation_do_not_start_identifiers() {
        let found = identifiers("x = 8'h0a; // done");
        let words: Vec<&str> = found.iter().map(|(_, _, word)| word.as_str()).collect();
        assert_eq!(words, vec!["x", "h0a", "done"]);
    }

    #[test]
    fn identifiers_use_character_columns_not_bytes() {
        // a multi byte character before the identifier must not shift the column
        let found = identifiers("// \u{394} clock: clk");
        let words: Vec<&str> = found.iter().map(|(_, _, word)| word.as_str()).collect();
        assert_eq!(words, vec!["clock", "clk"]);
        let clk = found.last().unwrap();
        assert_eq!(clk.0, 13);
        assert_eq!(clk.1, 16);
    }
}
