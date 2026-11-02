//! Memory accounting and the memory budget of the browser build.
//!
//! The browser has no mmap: the loader reads the whole file into wasm memory and
//! hands it to the backend, which (for VCD/GHW) materialises the whole trace in
//! wellen's packed representation. Both blocks live until the dump is dropped,
//! so on a large dump the only levers are *not keeping data nobody looks at*:
//!
//! * decoded histories of rows that scrolled far away ([`unload_signal`]),
//! * decoded histories of dumps that no pane draws ([`unload_all_signals`]),
//! * the parsed body of dumps that no pane draws ([`release_cache`]; switching
//!   to such a tab parses the file again, which is the price of the memory).
//!
//! This module owns that policy in one place:
//!
//! * [`DECODE_BUDGET_BYTES`] is one budget for *all* open dumps (it used to be
//!   64 MB per renderer instance, so N dumps held N x 64 MB),
//! * [`preflight`] decides, before a byte is read, whether the file can fit in
//!   what is left of the tab's wasm memory, and says what to do about it,
//! * [`stats`] is the readout published as [`window.__waveDebug.memory`].
//!
//! The pure parts (estimate, preflight decision, the free plan) are host
//! tested; only [`wasm_bytes`] and [`trim`] touch the browser.

use wave_insight_lib::waveform::WaveFormat;

use crate::state::{select, DumpHandle, Store};

/// One budget for the decoded histories of *every* open dump and of the split
/// pane (before Wave 12 this was a per instance 64 MB).
pub const DECODE_BUDGET_BYTES: usize = 64 * 1024 * 1024;

/// Memory the browser build is allowed to reach before it refuses a load.
///
/// wasm32 can address 4 GiB, but a browser tab is killed (or the wasm
/// allocation fails) long before that: this leaves room for the canvas backing
/// stores, the JS heap, monaco and the extra pages a parse grows into. It is a
/// policy number measured against the large-dump fixtures (see the task report:
/// a 349 MB VCD peaked at ~1.03 GB), not a hard browser limit.
pub const MEMORY_CAP_BYTES: u64 = 1280 * 1024 * 1024;

/// What the module itself occupies before any dump is opened (the app, monaco,
/// the canvas backing stores). Measured: a freshly booted tab sits at ~30 MB.
pub const BASE_OVERHEAD_BYTES: u64 = 48 * 1024 * 1024;

/// How much memory the backend needs per byte of file, past the file itself.
///
/// VCD and GHW decode the whole trace into wellen's packed representation when
/// the body is first read (roughly one byte of packed data per change, and a
/// text VCD spends several bytes per change); FST streams the body and only the
/// header is materialised. Measured on the large-dump fixtures (see the task
/// report): a VCD needs `2.1x its file size in wasm memory, an FST far less.
pub fn estimate_factor(format: Option<WaveFormat>) -> f64 {
    match format {
        Some(WaveFormat::Fst) => 0.6,
        Some(WaveFormat::Ghw) => 2.0,
        // VCD is text: the file is held, wellen materialises the body and the
        // decoder copies through a temporary before the body cache exists.
        // Measured peaks: 34 MB -> 138 MB, 69 MB -> 266 MB, 139 MB -> 522 MB,
        // 349 MB -> 1034 MB of wasm memory.
        Some(WaveFormat::Vcd) | None => 3.2,
    }
}

/// The format a file name announces, for the pre-flight estimate.
pub fn format_of(file_name: &str) -> Option<WaveFormat> {
    let lower = file_name.to_ascii_lowercase();
    if lower.ends_with(".vcd") {
        Some(WaveFormat::Vcd)
    } else if lower.ends_with(".fst") {
        Some(WaveFormat::Fst)
    } else if lower.ends_with(".ghw") {
        Some(WaveFormat::Ghw)
    } else {
        None
    }
}

/// The wasm memory a file of `file_bytes` bytes is estimated to need while it
/// is open (the file itself plus what the backend materialises).
pub fn estimated_need(file_bytes: u64, format: Option<WaveFormat>) -> u64 {
    let factor = estimate_factor(format);
    BASE_OVERHEAD_BYTES + (file_bytes as f64 * factor).ceil() as u64
}

/// What the pre-flight check says about one load.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Preflight {
    /// The file fits with room to spare.
    Ok,
    /// The file fits, but the tab will be close to its budget: warn first.
    Tight(String),
    /// The file cannot fit: refuse before a byte is read.
    Refuse(String),
}

/// Decide, before reading a file, whether the tab can hold it.
///
/// `current_wasm_bytes` is the wasm linear memory the module already owns
/// (including every open dump); the estimate adds what this file needs on top.
/// A refusal is always actionable: it names the numbers and what to do instead
/// of letting the allocator abort in the middle of a read.
pub fn preflight(file_name: &str, file_bytes: u64, current_wasm_bytes: u64) -> Preflight {
    let format = format_of(file_name);
    let need = estimated_need(file_bytes, format);
    let free = MEMORY_CAP_BYTES.saturating_sub(current_wasm_bytes);
    if need > free {
        return Preflight::Refuse(format!(
            "{file_name}: about {} MB of wasm memory are needed to open this {} MB dump, and only {} MB are still free in this tab — close a dump you are not looking at, split the dump, or open it in the desktop build",
            mb(need),
            mb(file_bytes),
            mb(free)
        ));
    }
    let after = current_wasm_bytes + need;
    if after * 4 >= MEMORY_CAP_BYTES * 3 {
        return Preflight::Tight(format!(
            "{file_name}: about {} MB of wasm memory for this {} MB dump; the tab will be at {} of `{} MB — close dumps you are not looking at",
            mb(need),
            mb(file_bytes),
            mb(after),
            mb(MEMORY_CAP_BYTES)
        ));
    }
    Preflight::Ok
}

/// Bytes to whole mebibytes, for messages (never zero: a "0 MB" line is a lie).
pub fn mb(bytes: u64) -> u64 {
    (bytes / (1024 * 1024)).max(1)
}

/// The memory readout published as `window.__waveDebug.memory`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MemoryStats {
    /// Linear memory the wasm module currently owns.
    pub wasm_bytes: u64,
    /// Summed file bytes of the open dumps (from the dump tab slice).
    pub file_bytes: u64,
    /// Decoded histories across every open dump's backend cache.
    pub decoded_bytes: usize,
    /// The budget `decoded_bytes` is trimmed to, across all open dumps.
    pub budget: usize,
    /// Open dumps.
    pub dumps: usize,
    /// Dumps whose decoded history this trim gave back.
    pub unloaded: usize,
    /// Dumps whose parsed body this trim gave back (re-parsed on the next look).
    pub released: usize,
}

impl MemoryStats {
    /// Whether the decoded caches are over the budget (they can exceed it while
    /// the visible rows themselves need more than the budget, which is a soft cap).
    pub fn over_budget(&self) -> bool {
        self.decoded_bytes > self.budget
    }
}

/// One open dump as the budget planner sees it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DumpMemory {
    /// Index in the planner's list (the caller maps it back to its handle).
    pub id: usize,
    /// Decoded history bytes the backend holds for it.
    pub decoded_bytes: usize,
    /// Whether a pane draws it right now: its data must not be given back.
    pub drawn: bool,
}

/// What to give back to reach the budget, cheapest first.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FreePlan {
    /// Dumps whose decoded histories are dropped entirely.
    pub unload_decoded: Vec<usize>,
    /// Dumps whose parsed body is dropped as well (re-parsed when looked at).
    pub release_body: Vec<usize>,
}

/// Plan the free operations for `budget`.
///
/// Rules, in order: dumps no pane draws give back their decoded histories first
/// (largest first, so the fewest dumps are disturbed); drawing dumps are never
/// touched here (the pane trims its own off-viewport rows). Only when the
/// decodeds of the undrawn dumps still do not fit are their parsed bodies
/// released too, which is the expensive one (a later tab switch re-parses).
pub fn plan_free(dumps: &[DumpMemory], budget: usize) -> FreePlan {
    let total: usize = dumps.iter().map(|dump| dump.decoded_bytes).sum();
    if total <= budget {
        return FreePlan::default();
    }
    let mut inactive: Vec<&DumpMemory> = dumps.iter().filter(|dump| !dump.drawn).collect();
    inactive.sort_by(|left, right| {
        right
            .decoded_bytes
            .cmp(&left.decoded_bytes)
            .then(left.id.cmp(&right.id))
    });
    let mut plan = FreePlan::default();
    let mut freed = 0usize;
    let target = total - budget;
    for dump in &inactive {
        if freed >= target {
            break;
        }
        if dump.decoded_bytes == 0 {
            continue;
        }
        plan.unload_decoded.push(dump.id);
        freed += dump.decoded_bytes;
    }
    if freed < target {
        // the decoded histories alone could not make room: give back the parsed
        // bodies of the undrawn dumps (their biggest block)
        for dump in &inactive {
            plan.release_body.push(dump.id);
        }
    }
    plan
}

/// The wasm linear memory the module owns right now.
#[cfg(target_arch = "wasm32")]
pub fn wasm_bytes() -> u64 {
    let memory = wasm_bindgen::memory();
    let Ok(buffer) = js_sys::Reflect::get(&memory, &wasm_bindgen::JsValue::from_str("buffer"))
    else {
        return 0;
    };
    let Ok(length) = js_sys::Reflect::get(&buffer, &wasm_bindgen::JsValue::from_str("byteLength"))
    else {
        return 0;
    };
    length.as_f64().map(|bytes| bytes as u64).unwrap_or(0)
}

/// The host target (the unit tests) has no wasm linear memory to report.
#[cfg(not(target_arch = "wasm32"))]
pub fn wasm_bytes() -> u64 {
    0
}

/// The decoded bytes one dump's backend cache holds.
pub fn decoded_of(handle: &DumpHandle) -> usize {
    handle.waveform().cache_stats().decoded_bytes()
}

/// The open dumps in tab order.
pub fn dump_handles(store: &Store) -> Vec<DumpHandle> {
    let tabs = store.select(select::dump_tabs);
    (0..tabs.items.len())
        .filter_map(|index| store.select(move |state| select::dump_waveform_of(state, index)))
        .collect()
}

/// Indexes (in [`dump_handles`]) of the dumps a pane draws right now: the active
/// one and the split's right pane. Those are the only caches worth keeping.
pub fn drawn_dumps(store: &Store) -> Vec<usize> {
    let tabs = store.select(select::dump_tabs);
    let mut drawn = vec![tabs.active];
    if let Some(split) = store.select(select::split) {
        if split.right != tabs.active && split.right < tabs.items.len() {
            drawn.push(split.right);
        }
    }
    drawn.retain(|index| *index < tabs.items.len());
    drawn
}

/// Whether a pane draws this dump right now (its decoded cache is not freeable).
pub fn is_drawn(store: &Store, handle: &DumpHandle) -> bool {
    let drawn = drawn_dumps(store);
    dump_handles(store)
        .iter()
        .enumerate()
        .any(|(index, candidate)| drawn.contains(&index) && candidate == handle)
}

/// The decoded budget one dump may use when several are open: the single budget
/// is split evenly over the open dumps (the split pane's dump counts too).
pub fn per_dump_share(dumps: usize) -> usize {
    DECODE_BUDGET_BYTES / dumps.max(1)
}

/// The current memory readout (no side effects).
pub fn stats(store: &Store) -> MemoryStats {
    let tabs = store.select(select::dump_tabs);
    let handles = dump_handles(store);
    MemoryStats {
        wasm_bytes: wasm_bytes(),
        file_bytes: tabs.total_bytes,
        decoded_bytes: handles.iter().map(decoded_of).sum(),
        budget: DECODE_BUDGET_BYTES,
        dumps: handles.len(),
        unloaded: 0,
        released: 0,
    }
}

/// Enforce the budget: give back what no pane draws, then report.
///
/// Called once per redraw by the wave pane and the split pane; it is cheap when
/// everything fits (a few `cache_stats` calls). It never touches a drawn dump's
/// cache: the pane itself trims its own off-viewport rows first.
pub fn trim(store: &Store) -> MemoryStats {
    let tabs = store.select(select::dump_tabs);
    let handles = dump_handles(store);
    let drawn = drawn_dumps(store);
    let dumps: Vec<DumpMemory> = handles
        .iter()
        .enumerate()
        .map(|(id, handle)| DumpMemory {
            id,
            decoded_bytes: decoded_of(handle),
            drawn: drawn.contains(&id),
        })
        .collect();
    let plan = plan_free(&dumps, DECODE_BUDGET_BYTES);
    for &id in &plan.unload_decoded {
        handles[id].waveform().unload_all_signals();
    }
    for &id in &plan.release_body {
        handles[id].waveform().release_cache();
    }
    MemoryStats {
        wasm_bytes: wasm_bytes(),
        file_bytes: tabs.total_bytes,
        decoded_bytes: handles.iter().map(decoded_of).sum(),
        budget: DECODE_BUDGET_BYTES,
        dumps: handles.len(),
        unloaded: plan.unload_decoded.len(),
        released: plan.release_body.len(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_detection_is_case_insensitive() {
        assert_eq!(format_of("big.VCD"), Some(WaveFormat::Vcd));
        assert_eq!(format_of("x.fst"), Some(WaveFormat::Fst));
        assert_eq!(format_of("x.GHW"), Some(WaveFormat::Ghw));
        assert_eq!(format_of("top.sv"), None);
        assert_eq!(format_of("no-extension"), None);
    }

    #[test]
    fn the_estimate_is_file_bytes_times_the_format_factor() {
        let vcd = estimated_need(100 * 1024 * 1024, Some(WaveFormat::Vcd));
        assert!(vcd > 100 * 1024 * 1024);
        assert!(vcd >= BASE_OVERHEAD_BYTES + 3 * 100 * 1024 * 1024);
        assert!(vcd <= BASE_OVERHEAD_BYTES + 4 * 100 * 1024 * 1024);
        // FST streams: much less than a VCD of the same size (only the header
        // is materialised, so the estimate is the base + a fraction of the file)
        let fst = estimated_need(100 * 1024 * 1024, Some(WaveFormat::Fst));
        assert!(fst < BASE_OVERHEAD_BYTES + 100 * 1024 * 1024);
        assert!(fst < vcd / 2);
        // an unknown format is estimated like the text format (worst case)
        assert_eq!(
            estimated_need(1024, None),
            estimated_need(1024, Some(WaveFormat::Vcd))
        );
    }

    #[test]
    fn preflight_allows_small_loads_on_an_empty_tab() {
        assert_eq!(
            preflight("small.vcd", 30 * 1024 * 1024, 64 * 1024 * 1024),
            Preflight::Ok
        );
    }

    #[test]
    fn preflight_refuses_when_the_estimate_does_not_fit() {
        // a 512 MB VCD needs `1 GB, the tab has `1.5 GB and 900 MB is already used
        let decision = preflight("huge.vcd", 512 * 1024 * 1024, 900 * 1024 * 1024);
        match decision {
            Preflight::Refuse(message) => {
                assert!(message.contains("huge.vcd"));
                assert!(message.contains("wasm memory"));
                // actionable: what to do about it
                assert!(message.contains("close a dump"));
                assert!(message.contains("desktop build"));
            }
            other => panic!("expected a refusal, got {other:?}"),
        }
    }

    #[test]
    fn preflight_warns_before_the_tab_is_full_instead_of_refusing() {
        // everything fits, but the estimate takes the tab past 75 % of the cap
        let decision = preflight("big.vcd", 300 * 1024 * 1024, 200 * 1024 * 1024);
        match decision {
            Preflight::Tight(message) => {
                assert!(message.contains("big.vcd"));
                assert!(message.contains("close dumps"));
            }
            other => panic!("expected a warning, got {other:?}"),
        }
    }

    #[test]
    fn the_free_plan_does_nothing_while_under_budget() {
        let dumps = [
            DumpMemory {
                id: 0,
                decoded_bytes: 10,
                drawn: true,
            },
            DumpMemory {
                id: 1,
                decoded_bytes: 20,
                drawn: false,
            },
        ];
        assert_eq!(plan_free(&dumps, 100), FreePlan::default());
    }

    #[test]
    fn the_free_plan_frees_undrawn_decoded_histories_first() {
        let dumps = [
            DumpMemory {
                id: 0,
                decoded_bytes: 50,
                drawn: true,
            },
            DumpMemory {
                id: 1,
                decoded_bytes: 100,
                drawn: false,
            },
            DumpMemory {
                id: 2,
                decoded_bytes: 30,
                drawn: false,
            },
        ];
        let plan = plan_free(&dumps, 120);
        // 180 bytes for a 120 budget: free the 50 bytes target from the biggest
        // undrawn cache first, leaving the drawn dump alone
        assert_eq!(plan.unload_decoded, vec![1]);
        assert!(plan.release_body.is_empty());
    }

    #[test]
    fn the_free_plan_never_touches_a_drawn_dump() {
        let dumps = [DumpMemory {
            id: 0,
            decoded_bytes: 500,
            drawn: true,
        }];
        let plan = plan_free(&dumps, 64);
        assert_eq!(plan, FreePlan::default());
    }

    #[test]
    fn the_free_plan_releases_bodies_only_when_the_histories_are_not_enough() {
        // one undrawn dump, 40 bytes decoded, but the target is 100
        let dumps = [
            DumpMemory {
                id: 0,
                decoded_bytes: 40,
                drawn: true,
            },
            DumpMemory {
                id: 1,
                decoded_bytes: 60,
                drawn: false,
            },
        ];
        let plan = plan_free(&dumps, 50);
        assert_eq!(plan.unload_decoded, vec![1]);
        // 60 freed vs 50 needed: no body needs to be thrown away
        assert!(plan.release_body.is_empty());

        let dumps = [
            DumpMemory {
                id: 0,
                decoded_bytes: 400,
                drawn: true,
            },
            DumpMemory {
                id: 1,
                decoded_bytes: 10,
                drawn: false,
            },
        ];
        let plan = plan_free(&dumps, 64);
        assert_eq!(plan.unload_decoded, vec![1]);
        // only 10 bytes of histories to give back: the parsed body goes too
        assert_eq!(plan.release_body, vec![1]);
    }

    #[test]
    fn the_budget_is_shared_between_open_dumps() {
        assert_eq!(per_dump_share(1), DECODE_BUDGET_BYTES);
        assert_eq!(per_dump_share(2), DECODE_BUDGET_BYTES / 2);
        assert_eq!(per_dump_share(4), DECODE_BUDGET_BYTES / 4);
        // no dump open: the whole budget, never a division by zero
        assert_eq!(per_dump_share(0), DECODE_BUDGET_BYTES);
    }

    #[test]
    fn the_readout_never_divides_by_zero() {
        assert_eq!(mb(0), 1);
        assert_eq!(mb(1024 * 1024 - 1), 1);
        assert_eq!(mb(1024 * 1024), 1);
    }
}
