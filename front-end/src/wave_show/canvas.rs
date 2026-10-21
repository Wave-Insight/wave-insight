//! The wave body: one `<canvas>`, only the rows in view, only the changes in
//! view.
//!
//! Everything here is a pure function of [`Frame`]: the component measures the
//! DOM, decides which rows are visible and hands them over. No allocations per
//! row, no per-row DOM nodes, and a redraw costs one `stroke()` per row plus the
//! labels that actually fit.

use wasm_bindgen::{JsCast, JsValue};
use web_sys::{CanvasRenderingContext2d, HtmlCanvasElement};

use wave_insight_lib::data_struct::ShowType;
use wave_insight_lib::waveform::{BitValue, WaveValue};

use crate::wave_show::geometry::{ROW_HEIGHT, Time, View};
use crate::wave_show::util::signal_things::{RowKind, SignalItem};
use crate::wave_show::Settings;

/// Palette, mirrored from `:root` in `styles.css`.
///
/// The canvas cannot use css variables, so the handful of colours it needs live
/// here; when a token in `styles.css` changes, change it here too. Only the row
/// colour comes from the row's own [`Settings`].
mod palette {
    pub const WAVE_BG: &str = "#0d1218";
    pub const GRID: &str = "#18212c";
    pub const SEPARATOR: &str = "#18202b";
    pub const ZEBRA: &str = "rgba(255, 255, 255, 0.012)";
    pub const CHOSEN: &str = "rgba(76, 194, 255, 0.10)";
    pub const CURSOR_A: &str = "#ffd166";
    pub const CURSOR_B: &str = "#dbe6f5";
    pub const TEXT: &str = "#9aa8bb";
    pub const MUTED: &str = "#2a3444";
    pub const DRAG: &str = "rgba(76, 194, 255, 0.14)";
}

/// The monospace stack used for value labels, matching `--font-mono`.
const FONT: &str =
    "10.5px \"Cascadia Code\", \"JetBrains Mono\", \"SF Mono\", Menlo, Consolas, monospace";
/// Stroke width of a waveform, in css pixels.
const WAVE_LINE: f64 = 1.6;
/// Bus rails are drawn slightly thinner than the digital step line.
const BUS_LINE: f64 = 1.2;
/// Width of the diagonal that joins two bus values, in css pixels. A bus
/// segment is a hexagon whose ends are cut at this angle, so consecutive values
/// meet in a point on the row's middle line instead of a vertical edge; that is
/// the usual "bus band" look (and what the user asked for: no straight up and
/// down edges on multi bit signals).
const BUS_SLANT: f64 = 6.0;
/// Vertical padding inside a 30 px row.
const ROW_PAD: f64 = 6.0;

/// One redraw worth of geometry and state.
pub struct Frame<'a> {
    /// Zoom/pan state.
    pub view: View,
    /// Surface size in css pixels (the canvas covers the wave area only).
    pub width: f64,
    pub height: f64,
    /// Pixels the row list is scrolled up.
    pub scroll_top: f64,
    /// Visible rows, top to bottom: `(row index, row)`.
    pub rows: &'a [(usize, &'a SignalItem)],
    /// Time axis ticks that carry a label, so the grid lines up with the ruler.
    pub grid: &'a [(f64, i64)],
    pub cursor1: Option<i32>,
    pub cursor2: Option<i32>,
    /// Drag-to-zoom preview band, in surface pixels.
    pub drag: Option<(f64, f64)>,
    /// `window.devicePixelRatio`.
    pub dpr: f64,
}

/// What one redraw did, for the debug hook and for the QA harness.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PaintStats {
    /// Rows that were drawn.
    pub rows: usize,
    /// Value changes that were visited after the per pixel decimation.
    pub changes: usize,
    /// Bus segments drawn as bands (diagonal joins) instead of step edges.
    pub bands: usize,
    /// Rows drawn as an analog polyline instead of a digital one.
    pub analog: usize,
}

/// Paint `frame` into `canvas`.
///
/// The backing store is sized in device pixels (device pixel ratio aware) while
/// all coordinates below are css pixels, so lines stay 1 device pixel wide on a
/// hidpi screen.
pub fn paint(canvas: &HtmlCanvasElement, frame: &Frame<'_>) -> PaintStats {
    let mut stats = PaintStats::default();
    let dpr = if frame.dpr.is_finite() && frame.dpr > 0.0 {
        frame.dpr
    } else {
        1.0
    };
    let width = if frame.width.is_finite() {
        frame.width.max(0.0)
    } else {
        0.0
    };
    let height = if frame.height.is_finite() {
        frame.height.max(0.0)
    } else {
        0.0
    };
    let pixel_width = (width * dpr).round() as u32;
    let pixel_height = (height * dpr).round() as u32;
    if pixel_width == 0 || pixel_height == 0 {
        return stats;
    }
    //resizing clears the canvas, so only do it when the size really changed
    if canvas.width() != pixel_width {
        canvas.set_width(pixel_width);
    }
    if canvas.height() != pixel_height {
        canvas.set_height(pixel_height);
    }
    let Some(context) = context_of(canvas) else {
        return stats;
    };
    //all following coordinates are css pixels
    let _ = context.set_transform(dpr, 0.0, 0.0, dpr, 0.0, 0.0);

    context.set_fill_style_str(palette::WAVE_BG);
    context.fill_rect(0.0, 0.0, width, height);

    draw_grid(&context, frame, dpr, height);
    draw_rows(&context, frame, dpr, width, height, &mut stats);
    if let Some((from, to)) = frame.drag {
        draw_band(&context, from, to, width, height);
    }
    draw_cursors(&context, frame, dpr, width, height);

    stats
}

fn context_of(canvas: &HtmlCanvasElement) -> Option<CanvasRenderingContext2d> {
    canvas
        .get_context("2d")
        .ok()
        .flatten()
        .and_then(|context| context.dyn_into::<CanvasRenderingContext2d>().ok())
}

/// Snap a coordinate to a device pixel so a 1 px line is exactly 1 px wide.
fn crisp(value: f64, dpr: f64) -> f64 {
    ((value * dpr).round() + 0.5) / dpr
}

fn draw_grid(context: &CanvasRenderingContext2d, frame: &Frame<'_>, dpr: f64, height: f64) {
    context.set_stroke_style_str(palette::GRID);
    context.set_line_width(1.0);
    context.begin_path();
    for &(x, _) in frame.grid {
        let x = crisp(x, dpr);
        if x < -1.0 || x > frame.width + 1.0 {
            continue;
        }
        context.move_to(x, 0.0);
        context.line_to(x, height);
    }
    context.stroke();
}

fn draw_rows(
    context: &CanvasRenderingContext2d,
    frame: &Frame<'_>,
    dpr: f64,
    width: f64,
    height: f64,
    stats: &mut PaintStats,
) {
    for &(row_index, item) in frame.rows {
        let top = row_index as f64 * ROW_HEIGHT - frame.scroll_top;
        if top > height || top + ROW_HEIGHT < 0.0 {
            continue;
        }
        stats.rows += 1;

        if row_index % 2 == 1 {
            context.set_fill_style_str(palette::ZEBRA);
            context.fill_rect(0.0, top, width, ROW_HEIGHT);
        }
        if item.choose {
            context.set_fill_style_str(palette::CHOSEN);
            context.fill_rect(0.0, top, width, ROW_HEIGHT);
        }

        let separator = crisp(top + ROW_HEIGHT - 1.0, dpr);
        context.set_stroke_style_str(palette::SEPARATOR);
        context.set_line_width(1.0);
        context.begin_path();
        context.move_to(0.0, separator);
        context.line_to(width, separator);
        context.stroke();

        match item.values.kind() {
            RowKind::Empty => draw_no_data(context, top, width),
            // analog mode replaces both the digital step and the bus band
            _ if item.setting.analog_active => {
                draw_analog_row(context, frame, item, top, width, stats)
            }
            RowKind::Bool => draw_bool_row(context, frame, item, top, width, stats),
            RowKind::Bits => draw_bus_row(context, frame, item, top, width, stats),
        }
    }
}

/// A row that has no values: one muted line, so the row keeps its place.
fn draw_no_data(context: &CanvasRenderingContext2d, top: f64, width: f64) {
    context.set_stroke_style_str(palette::MUTED);
    context.set_line_width(1.0);
    let y = crisp(top + ROW_HEIGHT / 2.0, 1.0);
    context.begin_path();
    context.move_to(0.0, y);
    context.line_to(width, y);
    context.stroke();
}

fn bool_level(value: &WaveValue, top: f64) -> f64 {
    let high = top + ROW_PAD;
    let low = top + ROW_HEIGHT - ROW_PAD;
    let mid = top + ROW_HEIGHT / 2.0;
    match value.as_bits().and_then(|bits| bits.single_bit()) {
        Some(BitValue::Zero) | Some(BitValue::L) => low,
        Some(BitValue::One) | Some(BitValue::H) => high,
        //`x`/`z`/`u`/`w`/`-` sit on the middle line, like the old svg renderer
        _ => mid,
    }
}

fn draw_bool_row(
    context: &CanvasRenderingContext2d,
    frame: &Frame<'_>,
    item: &SignalItem,
    top: f64,
    width: f64,
    stats: &mut PaintStats,
) {
    let (start, end) = frame.view.window(width);
    let color = color_string(&item.setting);
    context.set_stroke_style_str(&color);
    context.set_line_width(WAVE_LINE);
    context.set_line_join("round");

    let low = top + ROW_HEIGHT - ROW_PAD;
    let mut level = match item.values.value_at(start) {
        Some(value) => bool_level(value, top),
        None => f64::NAN,
    };
    let mut started = !level.is_nan();

    context.begin_path();
    if started {
        context.move_to(0.0, level);
    }
    item.values
        .for_each_change(start, end, &frame.view, width, |x, value| {
            stats.changes += 1;
            let next = bool_level(value, top);
            if started {
                context.line_to(x, level);
                context.line_to(x, next);
            } else {
                //no value before the window: this change defines the first level
                context.move_to(0.0, next);
                started = true;
            }
            level = next;
        });
    if started {
        context.line_to(width, level);
        context.stroke();
    }
    let _ = low;
}

fn draw_bus_row(
    context: &CanvasRenderingContext2d,
    frame: &Frame<'_>,
    item: &SignalItem,
    top: f64,
    width: f64,
    stats: &mut PaintStats,
) {
    let (start, end) = frame.view.window(width);
    let color = color_string(&item.setting);
    let band = Band {
        upper: top + ROW_PAD,
        lower: top + ROW_HEIGHT - ROW_PAD,
        mid: top + ROW_HEIGHT / 2.0,
    };
    let format_width = item.format_width();

    context.set_stroke_style_str(&color);
    context.set_line_width(BUS_LINE);
    // the diagonal joins look wrong with the default miter of a thin line
    context.set_line_join("round");

    // The value in effect at the left edge; `None` before the history starts.
    let mut current = item.values.value_at(start);
    // Where the band of `current` begins, in surface pixels.
    let mut segment_start = 0.0f64;

    context.begin_path();
    item.values
        .for_each_change(start, end, &frame.view, width, |x, value| {
            stats.changes += 1;
            let Some(previous) = current else {
                // the history starts inside the window: this is the first band
                current = Some(value);
                segment_start = x;
                return;
            };
            if *previous == *value {
                // a delta cycle or an explicit re-assignment: one band, no
                // transition edge
                return;
            }
            band.trace(context, segment_start, x);
            stats.bands += 1;
            label_segment(
                context,
                previous,
                &item.setting,
                format_width,
                segment_start,
                x,
                band.upper,
                band.lower,
            );
            current = Some(value);
            segment_start = x;
        });
    if let Some(value) = current {
        band.trace(context, segment_start, width);
        stats.bands += 1;
        label_segment(
            context,
            value,
            &item.setting,
            format_width,
            segment_start,
            width,
            band.upper,
            band.lower,
        );
    }
    context.stroke();
}

/// The vertical geometry of one bus row. The wave convention draws a multi bit
/// value as a band: two horizontal edges joined by diagonals, with consecutive
/// values meeting in a point on the middle line.
struct Band {
    upper: f64,
    lower: f64,
    mid: f64,
}

impl Band {
    /// Add one value's outline to the current path.
    ///
    /// A normal segment is a hexagon (`>` like ends). A segment too short for
    /// two diagonals degrades to a diamond so neighbouring values still join
    /// without a vertical edge.
    fn trace(&self, context: &CanvasRenderingContext2d, from: f64, to: f64) {
        let length = to - from;
        if !(length > 0.0) {
            return;
        }
        let slant = BUS_SLANT.min(length / 2.0);
        context.move_to(from, self.mid);
        context.line_to(from + slant, self.upper);
        if length > 2.0 * slant {
            context.line_to(to - slant, self.upper);
        }
        context.line_to(to, self.mid);
        context.line_to(to - slant, self.lower);
        if length > 2.0 * slant {
            context.line_to(from + slant, self.lower);
        }
        context.line_to(from, self.mid);
    }
}

/// One row in analog mode: a sampled polyline through the numeric values.
///
/// The samples are the value changes on screen, decimated to one per pixel like
/// the digital renderers. `Settings::analog_initerpolate != 0` (the default)
/// joins the samples with straight segments, i.e. the value is interpolated
/// between two changes; `0` keeps the digital zero-order hold shape but as a
/// polyline. `x`/`z`/unknown samples break the line: the trace holds the last
/// known value up to the gap and resumes at the next known sample.
fn draw_analog_row(
    context: &CanvasRenderingContext2d,
    frame: &Frame<'_>,
    item: &SignalItem,
    top: f64,
    width: f64,
    stats: &mut PaintStats,
) {
    let (start, end) = frame.view.window(width);
    let upper = top + ROW_PAD;
    let lower = top + ROW_HEIGHT - ROW_PAD;

    // The vertical scale is the range of the visible samples, so an analog row
    // uses its whole height; a constant signal sits on the middle line.
    let mut min = f64::INFINITY;
    let mut max = f64::NEG_INFINITY;
    let mut sample = |value: &WaveValue| {
        if let Some(number) = analog_number(value, &item.setting) {
            min = min.min(number);
            max = max.max(number);
        }
    };
    if let Some(value) = item.values.value_at(start) {
        sample(value);
    }
    for (_, value) in item.values.window(start, end) {
        sample(value);
    }
    if !min.is_finite() || !max.is_finite() {
        // nothing numeric on screen (an event row, a string, only x/z): keep
        // the row's place with the muted "no data" line
        draw_no_data(context, top, width);
        return;
    }
    let spread = max - min;
    let y_of = |number: f64| -> f64 {
        if spread > 0.0 {
            lower - (number - min) / spread * (lower - upper)
        } else {
            (upper + lower) / 2.0
        }
    };

    context.set_stroke_style_str(&color_string(&item.setting));
    context.set_line_width(WAVE_LINE);
    context.set_line_join("round");
    context.begin_path();

    let interpolate = item.setting.analog_initerpolate != 0;
    // y of the last known sample; `None` while the trace is inside a gap
    let mut last: Option<f64> = None;
    // seed the left edge with the value in effect at the window start, so the
    // trace is not empty until the first in-window change
    if let Some(value) = item.values.value_at(start) {
        let point = analog_number(value, &item.setting).map(|number| y_of(number));
        analog_point(context, &mut last, 0.0, point, interpolate);
    }
    item.values
        .for_each_change(start, end, &frame.view, width, |x, value| {
            stats.changes += 1;
            let point = analog_number(value, &item.setting).map(|number| y_of(number));
            analog_point(context, &mut last, x, point, interpolate);
        });
    if let Some(previous) = last {
        // hold the last known value to the right edge, like a sample and hold
        context.line_to(width, previous);
    }
    context.stroke();
    stats.analog += 1;
}

/// Add one analog sample (or a gap) to the current path.
fn analog_point(
    context: &CanvasRenderingContext2d,
    last: &mut Option<f64>,
    x: f64,
    point: Option<f64>,
    interpolate: bool,
) {
    match point {
        Some(y) => {
            match *last {
                Some(previous) if !interpolate => {
                    // zero order hold: stay on the old level until this change
                    context.line_to(x, previous);
                    context.line_to(x, y);
                }
                Some(_) => context.line_to(x, y),
                None => context.move_to(x, y),
            }
            *last = Some(y);
        }
        None => {
            // the value became unknown: hold up to the gap and break the line
            if let Some(previous) = *last {
                context.line_to(x, previous);
            }
            *last = None;
        }
    }
}

/// Numeric value of one sample for analog drawing.
///
/// `None` when the value has no number: any `x`/`z` bit, a string, an event, a
/// NaN real, or a vector wider than what the formatter can represent.
fn analog_number(value: &WaveValue, setting: &Settings) -> Option<f64> {
    match value {
        WaveValue::Real(number) => number.is_finite().then_some(*number),
        other => {
            let bits = other.as_bits()?;
            let raw = bits.as_u64()?;
            let width = bits.width();
            if setting.show_type == ShowType::SInt && (1..=63).contains(&width) {
                // two's complement, so a signed bus is plotted around zero
                let modulus = 1i64 << width;
                let signed = if raw & (1u64 << (width - 1)) != 0 {
                    raw as i64 - modulus
                } else {
                    raw as i64
                };
                return Some(signed as f64);
            }
            Some(raw as f64)
        }
    }
}

/// The label of one value: bit vectors follow the row's `ShowType`, reals and
/// strings are printed as they are, events have no text.
pub(crate) fn value_text(value: &WaveValue, width: usize, setting: &Settings) -> Option<String> {
    match value {
        WaveValue::Real(number) => Some(format!("{number}")),
        WaveValue::String(text) => Some(text.clone()),
        _ => value.render(width, &setting.show_type),
    }
}

/// Draw the value of one bus segment in the middle of its band, if the text fits
/// between `from` and `to`.
fn label_segment(
    context: &CanvasRenderingContext2d,
    value: &WaveValue,
    setting: &Settings,
    format_width: usize,
    from: f64,
    to: f64,
    upper: f64,
    lower: f64,
) {
    let Some(text) = value_text(value, format_width, setting) else {
        return;
    };
    let segment = to - from;
    if segment < 12.0 {
        return;
    }
    context.set_font(FONT);
    let Ok(metrics) = context.measure_text(&text) else {
        return;
    };
    let text_width = metrics.width();
    //keep a little air on both sides, and never draw over the next transition
    if text_width + 6.0 > segment {
        return;
    }
    context.set_fill_style_str(palette::TEXT);
    let y = (upper + lower) / 2.0;
    let _ = context.set_text_baseline("middle");
    //centred in the band, like every waveform viewer draws a bus value
    let _ = context.fill_text(&text, from + (segment - text_width) / 2.0, y);
}

fn draw_band(context: &CanvasRenderingContext2d, from: f64, to: f64, width: f64, height: f64) {
    let left = from.min(to).max(0.0);
    let right = from.max(to).min(width);
    if right <= left {
        return;
    }
    context.set_fill_style_str(palette::DRAG);
    context.fill_rect(left, 0.0, right - left, height);
}

fn draw_cursors(
    context: &CanvasRenderingContext2d,
    frame: &Frame<'_>,
    dpr: f64,
    width: f64,
    height: f64,
) {
    context.set_line_width(1.0);
    for (cursor, color) in [
        (frame.cursor1, palette::CURSOR_A),
        (frame.cursor2, palette::CURSOR_B),
    ] {
        let Some(cursor) = cursor else {
            continue;
        };
        let x = crisp(frame.view.x_of(cursor.max(0) as Time), dpr);
        if x < -1.0 || x > width + 1.0 {
            continue;
        }
        context.set_stroke_style_str(color);
        context.begin_path();
        context.move_to(x, 0.0);
        context.line_to(x, height);
        context.stroke();
    }
}

/// `rgb(r,g,b)` for a row's colour.
fn color_string(setting: &Settings) -> String {
    format!(
        "rgb({},{},{})",
        setting.color.0, setting.color.1, setting.color.2
    )
}

/// Numbers from one redraw, for the QA harness and for profiling.
#[derive(Clone, Copy, Debug, Default)]
pub struct DebugInfo {
    /// Wall time of the whole redraw (decode + paint), in milliseconds.
    pub redraw_ms: f64,
    /// Wall time spent decoding newly visible rows, in milliseconds.
    pub decode_ms: f64,
    /// Rows drawn / changes visited by the paint pass.
    pub stats: PaintStats,
    /// Rows in the viewport.
    pub visible: usize,
    /// Rows decoded by this redraw (0 means every visible row was cached).
    pub decoded: usize,
    /// Bytes the backend holds in its decode cache.
    pub decoded_bytes: usize,
    /// Rows in the model.
    pub total_rows: usize,
    /// Scroll offset of the row list.
    pub scroll_top: f64,
    /// Index of the first drawn row.
    pub first_row: usize,
    /// First cursor, in wave time.
    pub cursor1: Option<i32>,
    /// Second cursor, in wave time.
    pub cursor2: Option<i32>,
    /// Zoom/pan state, so a test can see that a gesture changed the view.
    pub view: View,
}

/// Expose one redraw's numbers on `window.__waveDebug` for the QA harness.
pub fn publish_debug(info: &DebugInfo) {
    let Some(window) = web_sys::window() else {
        return;
    };
    let object = js_sys::Object::new();
    let set = |key: &str, value: JsValue| {
        let _ = js_sys::Reflect::set(&object, &JsValue::from_str(key), &value);
    };
    set("rows", JsValue::from_f64(info.stats.rows as f64));
    set("changes", JsValue::from_f64(info.stats.changes as f64));
    set("bands", JsValue::from_f64(info.stats.bands as f64));
    set("analog", JsValue::from_f64(info.stats.analog as f64));
    set("visible", JsValue::from_f64(info.visible as f64));
    set("decoded", JsValue::from_f64(info.decoded as f64));
    set(
        "decoded_bytes",
        JsValue::from_f64(info.decoded_bytes as f64),
    );
    set("redraw_ms", JsValue::from_f64(info.redraw_ms));
    set("decode_ms", JsValue::from_f64(info.decode_ms));
    set("total_rows", JsValue::from_f64(info.total_rows as f64));
    set("scroll_top", JsValue::from_f64(info.scroll_top));
    set("first_row", JsValue::from_f64(info.first_row as f64));
    set("x_axis", JsValue::from_f64(info.view.x_axis));
    set("size", JsValue::from_f64(info.view.size));
    set(
        "cursor1",
        info.cursor1.map_or(JsValue::NULL, |cursor| JsValue::from_f64(cursor as f64)),
    );
    set(
        "cursor2",
        info.cursor2.map_or(JsValue::NULL, |cursor| JsValue::from_f64(cursor as f64)),
    );
    // R2: the shell's re-render counter, written by `App::rendered` into
    // `__waveRenderCount`. Carried over here because this function replaces the
    // whole debug object on every redraw; it is *not* a per-component counter.
    if let Ok(count) = js_sys::Reflect::get(&window, &JsValue::from_str("__waveRenderCount")) {
        if count.as_f64().is_some() {
            set("render_count", count);
        }
    }
    // the wave pane's own re-render count (same carry-over for the same reason)
    if let Ok(count) = js_sys::Reflect::get(&window, &JsValue::from_str("__wavePaneRenderCount")) {
        if count.as_f64().is_some() {
            set("pane_render_count", count);
        }
    }
    set("at", JsValue::from_f64(js_sys::Date::now()));
    let _ = js_sys::Reflect::set(&window, &JsValue::from_str("__waveDebug"), &object);
}
