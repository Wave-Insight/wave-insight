//! The wave body: one `<canvas>`, only the rows in view, only the changes in
//! view.
//!
//! Everything here is a pure function of [`Frame`]: the component measures the
//! DOM, decides which rows are visible and hands them over. No allocations per
//! row, no per-row DOM nodes, and a redraw costs one `stroke()` per row plus the
//! labels that actually fit.

use wasm_bindgen::{JsCast, JsValue};
use web_sys::{CanvasRenderingContext2d, HtmlCanvasElement};

use wave_insight_lib::data_struct::{BitsData, BoolData};
use wave_insight_lib::waveform::WaveValue;

use crate::wave_show::geometry::{ROW_HEIGHT, Time, View};
use crate::wave_show::util::signal_things::{RowKind, RowValue, SignalItem};
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
            RowKind::Bool => draw_bool_row(context, frame, item, top, width, stats),
            RowKind::Bits => draw_bus_row(context, frame, item, top, width, stats),
        }

        draw_cursor_value(context, frame, item, top, width, stats);
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

fn bool_level(value: RowValue<'_>, top: f64) -> f64 {
    let high = top + ROW_PAD;
    let low = top + ROW_HEIGHT - ROW_PAD;
    let mid = top + ROW_HEIGHT / 2.0;
    match value {
        RowValue::Backend(value) => match value.to_bool_data() {
            Some(BoolData::Zero) => low,
            Some(BoolData::One) => high,
            //`x` and `z` sit on the middle line, like the old svg renderer
            _ => mid,
        },
        RowValue::Bool(flag) => match flag {
            BoolData::Zero => low,
            BoolData::One => high,
            _ => mid,
        },
        RowValue::Bits(_) => mid,
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
    let upper = top + ROW_PAD;
    let lower = top + ROW_HEIGHT - ROW_PAD;
    let format_width = item.format_width();

    context.set_stroke_style_str(&color);
    context.set_line_width(BUS_LINE);

    let mut current = item.values.value_at(start);
    let mut last_x = 0.0f64;
    context.begin_path();
    item.values
        .for_each_change(start, end, &frame.view, width, |x, value| {
            stats.changes += 1;
            if let Some(previous) = current {
                context.move_to(last_x, upper);
                context.line_to(x, upper);
                context.move_to(last_x, lower);
                context.line_to(x, lower);
                label_segment(context, previous, &item.setting, format_width, last_x, x, upper, lower);
            }
            //the transition edge
            context.move_to(x, upper);
            context.line_to(x, lower);
            last_x = x;
            current = Some(value);
        });
    if let Some(previous) = current {
        context.move_to(last_x, upper);
        context.line_to(width, upper);
        context.move_to(last_x, lower);
        context.line_to(width, lower);
        label_segment(
            context,
            previous,
            &item.setting,
            format_width,
            last_x,
            width,
            upper,
            lower,
        );
    }
    context.stroke();
}

/// Draw the value of one bus segment if the text fits between `from` and `to`.
fn label_segment(
    context: &CanvasRenderingContext2d,
    value: RowValue<'_>,
    setting: &Settings,
    format_width: usize,
    from: f64,
    to: f64,
    upper: f64,
    lower: f64,
) {
    let text = format_row_value(value, setting, format_width);
    let Some(text) = text else {
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
    let _ = context.fill_text(&text, from + 3.0, y);
}

/// The value of this row at the first cursor, next to the cursor line.
fn draw_cursor_value(
    context: &CanvasRenderingContext2d,
    frame: &Frame<'_>,
    item: &SignalItem,
    top: f64,
    width: f64,
    _stats: &mut PaintStats,
) {
    let Some(cursor) = frame.cursor1 else {
        return;
    };
    if cursor < 0 {
        return;
    }
    let cursor = cursor as Time;
    let x = frame.view.x_of(cursor);
    if x < 0.0 || x > width {
        return;
    }
    let Some(value) = item.values.value_at(cursor) else {
        return;
    };
    let Some(text) = format_row_value(value, &item.setting, item.format_width()) else {
        return;
    };
    context.set_font(FONT);
    let Ok(metrics) = context.measure_text(&text) else {
        return;
    };
    let mut text_x = x + 4.0;
    if text_x + metrics.width() > width {
        text_x = (width - metrics.width() - 2.0).max(0.0);
    }
    context.set_fill_style_str(palette::TEXT);
    let _ = context.set_text_baseline("middle");
    let _ = context.fill_text(&text, text_x, top + ROW_HEIGHT / 2.0);
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

/// Format one value the way the row's [`Settings::show_type`] asks for.
///
/// The bit count is clamped to what the value really holds: a dump can be
/// narrower than the design (a truncated vector), and the legacy formatters
/// panic when asked for more bits than they have.
fn format_row_value(value: RowValue<'_>, setting: &Settings, width: usize) -> Option<String> {
    match value {
        RowValue::Backend(value) => format_wave_value(value, setting, width),
        RowValue::Bool(flag) => Some(bool_text(flag)),
        RowValue::Bits(data) => Some(format_bits_data(data, setting, width)),
    }
}

fn bool_text(flag: &BoolData) -> String {
    match flag {
        BoolData::Zero => "0",
        BoolData::One => "1",
        BoolData::X => "x",
        BoolData::Z => "z",
    }
    .to_string()
}

fn format_wave_value(value: &WaveValue, setting: &Settings, width: usize) -> Option<String> {
    if let Some(flag) = value.to_bool_data() {
        return Some(bool_text(&flag));
    }
    let actual = value.as_bits().map(|bits| bits.width()).unwrap_or(width);
    let bitcount = width.max(1).min(actual.max(1));
    value
        .to_bits_data()
        .map(|bits| bits.to_string(bitcount, &setting.show_type))
}

fn format_bits_data(data: &BitsData, setting: &Settings, width: usize) -> String {
    //the legacy transports do not report the real width, so `width` is all we
    //have here; the backend path above clamps against the value itself
    data.to_string(width.max(1), &setting.show_type)
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
    set("at", JsValue::from_f64(js_sys::Date::now()));
    let _ = js_sys::Reflect::set(&window, &JsValue::from_str("__waveDebug"), &object);
}
