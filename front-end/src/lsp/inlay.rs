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
//! values come from the same waveform the canvas uses, through the library's
//! **memory-flat** [`Waveform::stream_value_before`]: a hint lookup never
//! materialises the random-access body (FST time table + packed decodes), which
//! used to cost hundreds of MB — or GB — on a large dump the first time the
//! editor asked for a value. The per-pass lookup budget ([`LOOKUP_BUDGET`]) and
//! the small (signal, time) cache ([`HintValueCache`]) bound the work: a scroll
//! cannot run dozens of scans.
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

use wave_insight_lib::waveform::{ShowType, WaveValue, Waveform};

use crate::code_reader::monaco_glue::model_uri;
use crate::nav::{value_text, ValueFormat};
use crate::state::{select, DumpHandle, Store};

use super::bridge::file_of_uri;

/// Never answer more than this many hints for one request; the visible range is
/// small, this is only a guard against a pathological model.
const MAX_HINTS: usize = 300;
/// Never scan more lines than this, whatever range monaco asks for.
const MAX_LINES: u32 = 400;
/// Value lookups one provider call may do.
///
/// Each miss streams the source once (memory-flat but not free: an FST probe or
/// a VCD prefix scan), so a hint pass is bounded instead of turning a scroll
/// into dozens of scans. The provider is re-run for the visible range, and the
/// (signal, time) cache makes the next passes cheap; a lookup over the budget
/// simply leaves the label out.
const LOOKUP_BUDGET: usize = 16;
/// Time one provider call may spend on value lookups, in milliseconds.
///
/// The FST lookup is usually quick (streaming windows), but a VCD lookup is one
/// forward scan from the start of the trace: a cursor deep inside a large VCD can
/// cost seconds *per signal*. The provider is synchronous on monaco's UI thread,
/// so a scroll must not freeze the editor: after this wall-clock budget no new
/// lookup starts (the remaining hints simply have no label). The count budget
/// (`LOOKUP_BUDGET`) bounds the FST case, this bounds the VCD case.
const LOOKUP_TIME_BUDGET_MS: f64 = 250.0;
/// Entries of the (signal, time) -> value cache. The library keeps its own
/// 64-entry LRU of the same answers; this one also saves the formatting work
/// and makes the budget meaningful across provider calls.
const LOOKUP_CACHE_ENTRIES: usize = 64;

/// Page-wide state the provider reads.
struct Context {
    store: Store,
    waveform: Option<DumpHandle>,
    /// Cursor 1, in wave time units; `None` means no hints at all.
    cursor: Option<i64>,
    /// Values by (signal, time): the inlay hints' bounded cache.
    values: RefCell<HintValueCache>,
    emitter: Emitter,
}

thread_local! {
    static CONTEXT: RefCell<Option<Context>> = const { RefCell::new(None) };
}

/// Install the page-wide context. Called once from `App::create`.
pub fn init(store: Store) {
    CONTEXT.with(|context| {
        let mut context = context.borrow_mut();
        if context.is_none() {
            *context = Some(Context {
                store,
                waveform: None,
                cursor: None,
                values: RefCell::new(HintValueCache::new(LOOKUP_CACHE_ENTRIES)),
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
            // signal indices are per dump: a different dump invalidates the cache
            context.values.borrow_mut().clear();
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
    /// memory-flat lookups attempted against the library
    lookups: usize,
    /// lookups answered by the (signal, time) cache
    cache_hits: usize,
    /// hints left without a label because the budget was gone
    budget_skips: usize,
    /// lookups the library refused (a bad signal index or a decode error)
    lookup_errors: usize,
    /// entries in the cache when the provider call ended
    cache_entries: usize,
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
    publish_debug(
        has_context,
        cursor,
        lines,
        &counters,
        hints.length() as usize,
    );

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
    let Some((cursor, waveform, store)) = borrow_parts() else {
        return (false, None, 0);
    };
    let Some(cursor) = cursor else {
        return (true, None, last - first + 1);
    };
    let Some(waveform) = waveform else {
        return (true, Some(cursor), last - first + 1);
    };

    // one budget for the whole provider call: a scroll cannot run dozens of
    // streaming scans
    let mut budget =
        LookupBudget::with_time_limit(LOOKUP_BUDGET, js_sys::Date::now(), LOOKUP_TIME_BUDGET_MS);
    // The row formats (base format + fixed-point scale) and the row paths (used
    // to disambiguate a shared module body) are read once per request, never per
    // identifier.
    let rows = store.select(select::rows);
    let mut formats: HashMap<&str, ValueFormat> = HashMap::new();
    for row in rows.items.iter() {
        formats.insert(row.path.as_str(), ValueFormat::from_setting(&row.setting));
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
                &store,
                waveform.waveform(),
                &formats,
                &file,
                line,
                start,
                end,
                &word,
                cursor,
                counters,
                &mut budget,
            ) {
                hints.push(&hint);
            }
        }
    }
    counters.cache_entries = CONTEXT.with(|context| {
        context
            .borrow()
            .as_ref()
            .map(|context| context.values.borrow().len())
            .unwrap_or(0)
    });
    (true, Some(cursor), last - first + 1)
}

/// One hint for one identifier, if it names exactly one signal the dump holds and
/// the cursor is inside its history.
#[allow(clippy::too_many_arguments)]
fn hint_at(
    store: &Store,
    waveform: &Waveform,
    formats: &HashMap<&str, ValueFormat>,
    file: &str,
    line: u32,
    start: u32,
    end: u32,
    word: &str,
    cursor: i64,
    counters: &mut Counters,
    budget: &mut LookupBudget,
) -> Option<JsValue> {
    if word.is_empty() || cursor < 0 {
        return None;
    }
    let candidates = store.select(|state| {
        let exact = state.design.signals_at(file, line, start);
        if exact.is_empty() {
            state.design.signals_matching(word)
        } else {
            exact
        }
    });
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
    // memory-flat: the library streams the source instead of materialising the
    // random-access body; the budget + (signal, time) cache bound the passes
    let value = lookup_value(waveform, info.index(), cursor, counters, budget)?;
    counters.values += 1;
    let format = formats
        .get(dump_path.as_str())
        .cloned()
        .unwrap_or_else(|| ValueFormat::base(ShowType::Hex));
    let text = value_text(&value, width, &format)?;

    let position = Object::new();
    set_f64(&position, "lineNumber", f64::from(line));
    set_f64(&position, "column", f64::from(end.max(start + 1)));
    let hint = Object::new();
    set(&hint, "position", position.into());
    // A colon, not `=`: an inlay hint must never read as Verilog source, where
    // `=` is the assignment/equality operator.
    set(&hint, "label", JsValue::from_str(&format!(": {text}")));
    // monaco's InlayHintKind: Type = 1 (a value annotation)
    set_f64(&hint, "kind", 1.0);
    set(&hint, "paddingLeft", JsValue::TRUE);
    Some(hint.into())
}

/// The next value of one signal at one cursor time, or `None` when the lookup
/// budget is exhausted (the hint then has no label).
fn lookup_value(
    waveform: &Waveform,
    signal: usize,
    cursor: i64,
    counters: &mut Counters,
    budget: &mut LookupBudget,
) -> Option<WaveValue> {
    let key = (signal, cursor);
    let cached = CONTEXT.with(|context| {
        context
            .borrow()
            .as_ref()
            .and_then(|context| context.values.borrow().get(key))
    });
    if let Some(cached) = cached {
        counters.cache_hits += 1;
        return cached;
    }
    if !budget.take() {
        counters.budget_skips += 1;
        return None;
    }
    counters.lookups += 1;
    let answer = match waveform.stream_value_before(signal, cursor.max(0) as u64) {
        Ok(found) => found.map(|(_time, value)| value),
        Err(_error) => {
            counters.lookup_errors += 1;
            None
        }
    };
    CONTEXT.with(|context| {
        if let Some(context) = context.borrow().as_ref() {
            context.values.borrow_mut().insert(key, answer.clone());
        }
    });
    answer
}

/// One provider call's value lookup allowance.
struct LookupBudget {
    limit: usize,
    used: usize,
    /// Wall-clock instant (ms, Date::now) after which no lookup may start.
    deadline_ms: f64,
}

impl LookupBudget {
    /// The production budget: at most `limit` lookups and at most `budget_ms` of
    /// wall clock for this provider call.
    fn with_time_limit(limit: usize, now_ms: f64, budget_ms: f64) -> Self {
        Self {
            limit,
            used: 0,
            deadline_ms: now_ms + budget_ms,
        }
    }

    /// Take one lookup; `false` once the count or the time allowance is gone.
    fn take_at(&mut self, now_ms: f64) -> bool {
        if self.used >= self.limit || now_ms > self.deadline_ms {
            return false;
        }
        self.used += 1;
        true
    }

    fn take(&mut self) -> bool {
        self.take_at(js_sys::Date::now())
    }

    #[cfg(test)]
    fn used(&self) -> usize {
        self.used
    }

    #[cfg(test)]
    fn limit(&self) -> usize {
        self.limit
    }
}

/// A tiny (signal, time) -> value cache with insertion-order eviction.
///
/// The library memoises the streaming answers too, but this cache keeps the
/// formatting work cheap across provider calls and is what makes the per-pass
/// budget usable on the next pass (a scroll re-asks for the same hints).
struct HintValueCache {
    entries: HashMap<(usize, i64), Option<WaveValue>>,
    order: std::collections::VecDeque<(usize, i64)>,
    cap: usize,
}

impl HintValueCache {
    fn new(cap: usize) -> Self {
        Self {
            entries: HashMap::new(),
            order: std::collections::VecDeque::new(),
            cap: cap.max(1),
        }
    }

    fn get(&self, key: (usize, i64)) -> Option<Option<WaveValue>> {
        self.entries.get(&key).cloned()
    }

    fn insert(&mut self, key: (usize, i64), value: Option<WaveValue>) {
        if self.entries.insert(key, value).is_none() {
            self.order.push_back(key);
        }
        while self.entries.len() > self.cap {
            let Some(oldest) = self.order.pop_front() else {
                break;
            };
            self.entries.remove(&oldest);
        }
    }

    fn len(&self) -> usize {
        self.entries.len()
    }

    fn clear(&mut self) {
        self.entries.clear();
        self.order.clear();
    }
}

/// Which candidate a source position means.
///
/// One candidate is the answer. Several candidates come from a module body
/// instantiated more than once; a row the user already put in the wave view
/// disambiguates it, and if that is still ambiguous nothing is shown — the hint
/// must never show the value of a random instance.
fn pick_candidate(
    waveform: &Waveform,
    formats: &HashMap<&str, ValueFormat>,
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
    set_f64(&debug, "lookups", counters.lookups as f64);
    set_f64(&debug, "cacheHits", counters.cache_hits as f64);
    set_f64(&debug, "budgetSkips", counters.budget_skips as f64);
    set_f64(&debug, "lookupErrors", counters.lookup_errors as f64);
    set_f64(&debug, "cacheEntries", counters.cache_entries as f64);
    set_f64(&debug, "hints", hints as f64);
    let _ = Reflect::set(&window, &JsValue::from_str("__waveInlayDebug"), &debug);
}

/// Clone the parts the provider needs out of the thread local: no borrow is
/// held while JS or the design database is used.
fn borrow_parts() -> Option<(Option<i64>, Option<DumpHandle>, Store)> {
    CONTEXT.with(|context| {
        let context = context.borrow();
        let context = context.as_ref()?;
        Some((
            context.cursor,
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
    use super::{identifiers, HintValueCache, LookupBudget};

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

    #[test]
    fn the_lookup_budget_allows_exactly_its_limit_per_pass() {
        // no time limit for the count test (INFINITY deadline)
        let mut budget = LookupBudget::with_time_limit(16, 0.0, f64::INFINITY);
        assert_eq!(budget.limit(), 16);
        for _ in 0..16 {
            assert!(budget.take_at(0.0), "the allowance is per provider call");
        }
        assert!(!budget.take_at(0.0), "the 17th lookup must be refused");
        assert!(!budget.take_at(0.0));
        assert_eq!(budget.used(), 16);
        // a budget of zero never permits a scan
        let mut none = LookupBudget::with_time_limit(0, 0.0, f64::INFINITY);
        assert!(!none.take_at(0.0));
    }

    #[test]
    fn the_lookup_budget_stops_starting_lookups_after_the_time_budget() {
        // 16 lookups allowed, but only within the first 250 ms of the call
        let mut budget = LookupBudget::with_time_limit(16, 1_000.0, 250.0);
        assert!(budget.take_at(1_010.0));
        assert!(budget.take_at(1_100.0));
        assert!(budget.take_at(1_250.0));
        // the deadline is exclusive: a lookup starting after it is refused
        assert!(!budget.take_at(1_250.1));
        assert_eq!(budget.used(), 3);
        // a slow first lookup (a VCD prefix scan) can use the whole allowance
        let mut slow = LookupBudget::with_time_limit(16, 0.0, 250.0);
        assert!(slow.take_at(0.0));
        assert!(!slow.take_at(3_000.0), "a VCD scan must not allow 15 more");
    }

    #[test]
    fn the_hint_value_cache_is_bounded_and_keyed_by_signal_and_time() {
        let mut cache = HintValueCache::new(4);
        assert_eq!(cache.get((1, 10)), None);
        cache.insert((1, 10), None);
        // a miss is cached too: "no value yet" must not be re-streamed
        assert_eq!(cache.get((1, 10)), Some(None));
        for index in 0..10 {
            cache.insert((index, 10), None);
        }
        assert_eq!(cache.len(), 4, "the cache is capped");
        // the oldest keys went first
        assert_eq!(cache.get((0, 10)), None);
        cache.clear();
        assert_eq!(cache.len(), 0);
    }
}
