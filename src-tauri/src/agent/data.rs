//! The data half of the AI integration: querying dumps without the webview.
//!
//! The MCP tools read waveforms directly through this module instead of
//! asking the front end for windows, which is what makes the feature usable on
//! the dumps it matters for: the registry opens files natively (mmap when the
//! platform allows it), so a 500 MiB FST never enters wasm memory just because
//! an assistant asked about one signal.
//!
//! Two rules keep that honest:
//!
//! * every query here is a *bounded* one - a capped window, a value at a time,
//!   a streaming search - never "decode the whole history and hand it back";
//! * the agent's registry is separate from the UI's, so an AI question can
//!   never close or evict the dump the user is looking at.
//!
//! Answers are the shared DTOs in `wave_insight_lib::agent`, the same shapes
//! the front end deserializes.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use wave_insight_lib::agent::{
    AgentChange, AgentDumpInfo, AgentSignalChanges, AgentSignalMatch, AgentSignalSummary,
    AgentTimeRange, AgentValue, GetChangesResult, GetValuesResult, SearchValueResult,
    SummarizeSignalsResult,
};
use wave_insight_lib::waveform::{ShowType, SignalInfo, ValueQuery};

use crate::dumps::{DumpId, DumpRegistry};

/// How many files the agent keeps open at once.
///
/// The UI's registry is per user tab; this one caches the files a conversation
/// touched. Four is enough for "compare this run with the last one" and small
/// enough that a stray tool loop cannot pin a dozen mappings.
pub const AGENT_OPEN_DUMPS: usize = 4;
/// Upper bound for one `find_signals` answer.
pub const MAX_SIGNAL_MATCHES: usize = 500;
/// Upper bound for the signal list of a value/change query.
pub const MAX_SIGNALS_PER_QUERY: usize = 32;
/// Upper bound for the time list of one `get_values` call.
pub const MAX_TIMES_PER_QUERY: usize = 256;
/// Upper bound for the per-signal cap of `get_changes`.
pub const MAX_CHANGES_PER_SIGNAL: usize = 4096;
/// Upper bound for the signal list of an extraction.
pub const MAX_EXTRACT_SIGNALS: usize = 64;

/// The agent's own view of the open waveform files.
pub struct AgentData {
    registry: Arc<DumpRegistry>,
    cache: Mutex<AgentCache>,
}

/// LRU-ish cache of the files the conversation has queried.
#[derive(Default)]
struct AgentCache {
    /// Canonical path -> open dump, most recently used last.
    entries: Vec<(PathBuf, DumpId)>,
}

impl AgentCache {
    fn touch(&mut self, path: &Path, id: DumpId) {
        self.entries.retain(|(cached, _)| cached != path);
        self.entries.push((path.to_path_buf(), id));
    }

    /// The entry to evict, when the cache is over its cap.
    fn evicted(&mut self) -> Option<DumpId> {
        if self.entries.len() <= AGENT_OPEN_DUMPS {
            return None;
        }
        let (_, id) = self.entries.remove(0);
        Some(id)
    }
}

impl AgentData {
    pub fn new() -> Self {
        Self {
            registry: Arc::new(DumpRegistry::new()),
            cache: Mutex::new(AgentCache::default()),
        }
    }

    fn lock(&self) -> MutexGuard<'_, AgentCache> {
        self.cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Open `path` (canonically) and return its dump id and metadata.
    ///
    /// The path is canonicalized first so two spellings of the same file share
    /// one mapping, and a non-file is rejected before anything is opened. On a
    /// cache miss the oldest entry over the cap is closed, which releases its
    /// mapping (and file handle on Windows).
    pub fn open_path(&self, path: &str) -> Result<(DumpId, AgentDumpInfo), String> {
        if path.trim().is_empty() {
            return Err("the path is empty".to_string());
        }
        let canonical = std::fs::canonicalize(path).map_err(|error| format!("{path}: {error}"))?;
        if !canonical.is_file() {
            return Err(format!("{}: not a file", canonical.display()));
        }
        if let Some(id) = self.cached_id(&canonical) {
            if let Ok(info) = self.info(id, &canonical.display().to_string()) {
                return Ok((id, info));
            }
            // the dump was closed behind our back (the registry is also used by
            // tests): fall through and reopen
            self.lock()
                .entries
                .retain(|(cached, _)| cached != &canonical);
        }
        let summary = self.registry.open(&canonical.display().to_string())?;
        let id = summary.id;
        let evicted = {
            let mut cache = self.lock();
            cache.touch(&canonical, id);
            cache.evicted()
        };
        if let Some(old) = evicted {
            // a failure here only means the mapping outlives the cache entry
            let _ = self.registry.close(old);
        }
        let info = self.info(id, &canonical.display().to_string())?;
        Ok((id, info))
    }

    fn cached_id(&self, canonical: &Path) -> Option<DumpId> {
        self.lock()
            .entries
            .iter()
            .find(|(path, _)| path == canonical)
            .map(|(_, id)| *id)
    }

    /// The metadata every data answer repeats.
    pub fn info(&self, id: DumpId, path: &str) -> Result<AgentDumpInfo, String> {
        self.registry.with_waveform(id, |waveform| AgentDumpInfo {
            name: file_name(path),
            path: Some(path.to_string()),
            format: waveform.file_format().name().to_string(),
            timescale: waveform.timescale().map(|timescale| timescale.to_string()),
            time_range: waveform
                .time_range_hint()
                .ok()
                .flatten()
                .map(|range| AgentTimeRange {
                    start: range.start,
                    end: range.end,
                }),
            signal_count: waveform.signal_count(),
        })
    }

    /// Search the hierarchy by local name (optionally inside a scope).
    ///
    /// `mode` is `contains` (the default), `prefix` or `exact`, and the match is
    /// case-insensitive. Only the signals that match have their full path
    /// built: on a 200k-signal dump that is the difference between a cheap
    /// query and 200k string allocations.
    pub fn find_signals(
        &self,
        id: DumpId,
        query: &str,
        mode: &str,
        scope: Option<&str>,
        limit: usize,
    ) -> Result<(usize, Vec<AgentSignalMatch>), String> {
        let needle = query.trim().to_ascii_lowercase();
        if needle.is_empty() {
            return Err("the query is empty: give a signal name (or part of one)".to_string());
        }
        let mode = match mode.trim().to_ascii_lowercase().as_str() {
            "" | "contains" => MatchMode::Contains,
            "prefix" => MatchMode::Prefix,
            "exact" => MatchMode::Exact,
            other => {
                return Err(format!(
                    "unknown match mode '{other}' (expected contains, prefix or exact)"
                ))
            }
        };
        let scope = scope.map(|scope| scope.trim_matches('.').to_ascii_lowercase());
        let limit = limit.clamp(1, MAX_SIGNAL_MATCHES);
        self.registry.with_waveform(id, |waveform| {
            let mut matches = Vec::new();
            let mut total = 0usize;
            for info in waveform.signals() {
                if !mode.matches(&info.name().to_ascii_lowercase(), &needle) {
                    continue;
                }
                if let Some(scope) = &scope {
                    let path = info.scope().join(".").to_ascii_lowercase();
                    if !path.starts_with(scope) {
                        continue;
                    }
                }
                total += 1;
                if matches.len() < limit {
                    matches.push(signal_match(info));
                }
            }
            (total, matches)
        })
    }

    /// Look up one path, or explain the miss with the closest names.
    pub fn resolve(&self, id: DumpId, path: &str) -> Result<(usize, AgentSignalMatch), String> {
        let found = self
            .registry
            .with_waveform(id, |waveform| waveform.signal_by_path(path).cloned())?;
        match found {
            Some(info) => Ok((info.index(), signal_match(&info))),
            None => {
                let leaf = path.rsplit('.').next().unwrap_or(path);
                let mut similar = self.find_signals(id, leaf, "contains", None, 5)?.1;
                if similar.is_empty() {
                    // a typo can be shorter than the name it meant: its first
                    // character is still a useful hint ("px" -> "pc")
                    let prefix: String = leaf.chars().take(1).collect();
                    similar = self.find_signals(id, &prefix, "contains", None, 5)?.1;
                }
                if similar.is_empty() {
                    // nothing resembles it: name a few signals so the model can
                    // see the dump's naming instead of guessing again
                    similar = self.find_signals(id, "a", "contains", None, 5)?.1;
                }
                let hint = if similar.is_empty() {
                    String::new()
                } else {
                    let names: Vec<String> =
                        similar.iter().map(|signal| signal.path.clone()).collect();
                    format!(" Similar: {}.", names.join(", "))
                };
                Err(format!("signal '{path}' is not in this dump.{hint}"))
            }
        }
    }

    /// Values of several signals at several times, one batch pass per time.
    ///
    /// `values_before` is the memory-flat path: it never materialises the body,
    /// and one call covers all the signals for one time (on a 20M-step FST that
    /// was measured 6-7x faster than per-signal lookups).
    pub fn values(
        &self,
        id: DumpId,
        paths: &[String],
        times: &[u64],
        format_name: &str,
        info: AgentDumpInfo,
    ) -> Result<GetValuesResult, String> {
        if paths.is_empty() || times.is_empty() {
            return Err("give at least one signal and one time".to_string());
        }
        if paths.len() > MAX_SIGNALS_PER_QUERY {
            return Err(format!(
                "too many signals in one call: {} (limit {MAX_SIGNALS_PER_QUERY})",
                paths.len()
            ));
        }
        if times.len() > MAX_TIMES_PER_QUERY {
            return Err(format!(
                "too many times in one call: {} (limit {MAX_TIMES_PER_QUERY})",
                times.len()
            ));
        }
        let mut resolved = Vec::with_capacity(paths.len());
        for path in paths {
            let (index, _) = self.resolve(id, path)?;
            resolved.push((path.clone(), index));
        }
        let indices: Vec<usize> = resolved.iter().map(|(_, index)| *index).collect();
        let mut values = Vec::with_capacity(paths.len() * times.len());
        for &time in times {
            let answers = self
                .registry
                .values_at(id, &indices, time, Some(format_name))?;
            for (slot, (path, _)) in resolved.iter().enumerate() {
                let answer = answers.get(slot).and_then(|answer| answer.as_ref());
                values.push(AgentValue {
                    signal: path.clone(),
                    requested_time: time,
                    value_time: answer.map(|answer| answer.time),
                    value: answer.map(|answer| answer.value.clone()),
                    found: answer.is_some(),
                });
            }
        }
        Ok(GetValuesResult {
            dump: info,
            format: format_name.to_string(),
            values,
        })
    }

    /// The changes of several signals inside a range, capped per signal.
    ///
    /// `capped` on a signal means the per-signal cap stopped the collection;
    /// `next_start` is where a continuation should begin, so an assistant can
    /// page through a dense signal instead of asking for its whole history.
    #[allow(clippy::too_many_arguments)]
    pub fn changes(
        &self,
        id: DumpId,
        paths: &[String],
        start: u64,
        end: Option<u64>,
        max_per_signal: usize,
        format_name: &str,
        info: AgentDumpInfo,
    ) -> Result<GetChangesResult, String> {
        if paths.is_empty() {
            return Err("give at least one signal".to_string());
        }
        if paths.len() > MAX_SIGNALS_PER_QUERY {
            return Err(format!(
                "too many signals in one call: {} (limit {MAX_SIGNALS_PER_QUERY})",
                paths.len()
            ));
        }
        let max_per_signal = max_per_signal.clamp(1, MAX_CHANGES_PER_SIGNAL);
        let mut resolved = Vec::with_capacity(paths.len());
        for path in paths {
            let (index, signal) = self.resolve(id, path)?;
            resolved.push((path.clone(), index, signal.width));
        }
        let indices: Vec<usize> = resolved.iter().map(|(_, index, _)| *index).collect();
        let windows = self.registry.window(
            id,
            &indices,
            start,
            end,
            Some(max_per_signal),
            Some(format_name),
        )?;
        let mut truncated = false;
        let mut signals = Vec::with_capacity(windows.len());
        for ((path, _, width), window) in resolved.iter().zip(windows) {
            if window.capped {
                truncated = true;
            }
            let next_start = window
                .changes
                .last()
                .map(|change| change.time.saturating_add(1));
            signals.push(AgentSignalChanges {
                signal: path.clone(),
                width: *width,
                before: window.before,
                capped: window.capped,
                next_start: window.capped.then_some(next_start).flatten(),
                changes: window
                    .changes
                    .into_iter()
                    .map(|change| AgentChange {
                        time: change.time,
                        value: change.value,
                    })
                    .collect(),
            });
        }
        let range_end = info.time_range.map(|range| range.end).unwrap_or(start);
        Ok(GetChangesResult {
            dump: info,
            start,
            end: end.unwrap_or(range_end),
            format: format_name.to_string(),
            truncated,
            signals,
        })
    }

    /// Search one signal for a value (text, or a `0x`/`0b`/`0o` radix form).
    ///
    /// Worst case is a full trace scan; the tool documents that and the caller
    /// is expected to narrow the range first when it matters.
    #[allow(clippy::too_many_arguments)]
    pub fn search(
        &self,
        id: DumpId,
        path: &str,
        query: &str,
        from: u64,
        backwards: bool,
        show: &ShowType,
        info: AgentDumpInfo,
    ) -> Result<SearchValueResult, String> {
        let (index, _) = self.resolve(id, path)?;
        let found = self
            .registry
            .find(id, index, query, from, backwards, show)?;
        Ok(SearchValueResult {
            dump: info,
            signal: path.to_string(),
            query: query.to_string(),
            backwards,
            found: found.is_some(),
            time: found.as_ref().map(|found| found.time),
            value: found.map(|found| found.value),
        })
    }

    /// One streaming pass over the selected signals: counts and endpoints.
    ///
    /// This is the "what is in here?" query: it costs one pass and O(signals)
    /// memory, and it tells an assistant whether extracting a range is worth it
    /// before it asks for the changes themselves.
    #[allow(clippy::too_many_arguments)]
    pub fn summarize(
        &self,
        id: DumpId,
        paths: &[String],
        start: u64,
        end: u64,
        show: &ShowType,
        format_name: &str,
        info: AgentDumpInfo,
    ) -> Result<SummarizeSignalsResult, String> {
        if paths.is_empty() {
            return Err("give at least one signal".to_string());
        }
        if paths.len() > MAX_SIGNALS_PER_QUERY {
            return Err(format!(
                "too many signals in one call: {} (limit {MAX_SIGNALS_PER_QUERY})",
                paths.len()
            ));
        }
        let mut resolved = Vec::with_capacity(paths.len());
        for path in paths {
            let (index, signal) = self.resolve(id, path)?;
            resolved.push((path.clone(), index, signal.width));
        }
        let indices: Vec<usize> = resolved.iter().map(|(_, index, _)| *index).collect();
        let widths: HashMap<usize, u32> = resolved
            .iter()
            .map(|(_, index, width)| (*index, *width))
            .collect();
        let mut rows: HashMap<usize, AgentSignalSummary> = resolved
            .iter()
            .map(|(path, index, width)| {
                (
                    *index,
                    AgentSignalSummary {
                        signal: path.clone(),
                        width: *width,
                        changes: 0,
                        first_time: None,
                        last_time: None,
                        first_value: None,
                        last_value: None,
                    },
                )
            })
            .collect();
        // the value in effect at the range start, per signal, in one batch
        let before = self
            .registry
            .values_at(id, &indices, start, Some(format_name))?;
        let mut first_values: HashMap<usize, String> = HashMap::new();
        for (slot, answer) in before.into_iter().enumerate() {
            if let Some(answer) = answer {
                first_values.insert(indices[slot], answer.value);
            }
        }
        let query = ValueQuery::for_signals(indices.clone()).between(start, Some(end));
        self.registry.with_waveform(id, |waveform| {
            let _ = waveform.stream_changes(&query, |signal, time, value| {
                let Some(summary) = rows.get_mut(&signal) else {
                    return true;
                };
                let width = widths.get(&signal).copied().unwrap_or(0);
                let rendered = crate::dumps::render_value(&value, width, show);
                if summary.first_time.is_none() {
                    summary.first_time = Some(time);
                }
                summary.last_time = Some(time);
                summary.last_value = Some(rendered);
                summary.changes += 1;
                true
            });
        })?;
        let mut signals = Vec::with_capacity(resolved.len());
        for (path, index, _) in &resolved {
            if let Some(mut summary) = rows.remove(index) {
                let _ = path;
                summary.first_value = first_values
                    .get(index)
                    .cloned()
                    .or_else(|| summary.first_value.clone());
                signals.push(summary);
            }
        }
        Ok(SummarizeSignalsResult {
            dump: info,
            start,
            end,
            format: format_name.to_string(),
            signals,
        })
    }

    /// Drop every agent mapping (used on application exit).
    #[allow(dead_code)]
    pub fn close_all(&self) {
        self.lock().entries.clear();
        self.registry.close_all();
    }
}

impl Default for AgentData {
    fn default() -> Self {
        Self::new()
    }
}

/// How `find_signals` compares a name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MatchMode {
    Contains,
    Prefix,
    Exact,
}

impl MatchMode {
    fn matches(self, name: &str, needle: &str) -> bool {
        match self {
            MatchMode::Contains => name.contains(needle),
            MatchMode::Prefix => name.starts_with(needle),
            MatchMode::Exact => name.eq(needle),
        }
    }
}

fn signal_match(info: &SignalInfo) -> AgentSignalMatch {
    AgentSignalMatch {
        path: info.path().to_string(),
        name: info.name().to_string(),
        width: info.width(),
        kind: crate::dumps::signal_kind_label(info.kind()).to_string(),
        range: info.range_string(),
        scope: info.scope().join("."),
    }
}

fn file_name(path: &str) -> String {
    Path::new(path)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// The metadata a call needs when the dump itself is not the subject.
    fn empty_info() -> AgentDumpInfo {
        AgentDumpInfo {
            name: "x".into(),
            path: None,
            format: "VCD".into(),
            timescale: None,
            time_range: None,
            signal_count: 0,
        }
    }

    const SMALL_VCD: &str = "$date test $end\n$version wave-insight agent data test $end\n$timescale 1ns $end\n$scope module top $end\n$var wire 1 ! clk $end\n$var wire 4 \" data [3:0] $end\n$scope module u $end\n$var wire 8 # pc [7:0] $end\n$upscope $end\n$upscope $end\n$enddefinitions $end\n$dumpvars\n0!\nb0000 \"\nb00000000 #\n$end\n#0\n0!\n#10\n1!\nb1010 \"\nb00000011 #\n#20\n0!\nb0101 \"\nb00000100 #\n#30\n1!\n";

    fn write_vcd(name: &str, text: &str) -> PathBuf {
        let dir = std::env::temp_dir()
            .join("wave-insight-agent-data")
            .join(format!("{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(format!("{name}.vcd"));
        let mut file = std::fs::File::create(&path).unwrap();
        file.write_all(text.as_bytes()).unwrap();
        path
    }

    fn open(data: &AgentData, name: &str, text: &str) -> (DumpId, String, AgentDumpInfo) {
        let path = write_vcd(name, text);
        let display = path.display().to_string();
        let (id, _) = data.open_path(&display).unwrap();
        let info = data.info(id, &display).unwrap();
        (id, display, info)
    }

    #[test]
    fn a_path_is_opened_once_and_cached() {
        let data = AgentData::new();
        let path = write_vcd("cache", SMALL_VCD).display().to_string();
        let (first, _) = data.open_path(&path).unwrap();
        let (second, _) = data.open_path(&path).unwrap();
        assert_eq!(first, second);
        // a misspelled path is a clean error, not a panic
        let error = data.open_path("no/such/file.vcd").unwrap_err();
        assert!(error.contains("no/such/file.vcd"), "{error}");
    }

    #[test]
    fn signals_are_found_by_mode_and_scope() {
        let data = AgentData::new();
        let (id, _, _) = open(&data, "find", SMALL_VCD);
        let (total, matches) = data.find_signals(id, "pc", "exact", None, 50).unwrap();
        assert_eq!(total, 1);
        assert_eq!(matches[0].path, "top.u.pc");
        assert_eq!(matches[0].width, 8);
        // contains finds both `data` and nothing else for a prefix that matches one
        let (total, _) = data.find_signals(id, "da", "prefix", None, 50).unwrap();
        assert_eq!(total, 1);
        // the scope narrows a name that exists in several scopes
        let (total, _) = data
            .find_signals(id, "pc", "contains", Some("top.u"), 50)
            .unwrap();
        assert_eq!(total, 1);
        let (total, _) = data
            .find_signals(id, "pc", "contains", Some("top.nope"), 50)
            .unwrap();
        assert_eq!(total, 0);
        // an empty pattern is rejected instead of matching everything
        assert!(data.find_signals(id, "  ", "contains", None, 50).is_err());
        assert!(data.find_signals(id, "pc", "fuzzy", None, 50).is_err());
        // the limit truncates the list but keeps the true total
        let (total, matches) = data.find_signals(id, "c", "contains", None, 1).unwrap();
        assert!(total >= 2, "clk and pc both contain a c");
        assert_eq!(matches.len(), 1);
    }

    #[test]
    fn a_missing_path_suggests_the_closest_names() {
        let data = AgentData::new();
        let (id, _, _) = open(&data, "suggest", SMALL_VCD);
        let error = data.resolve(id, "top.u.px").unwrap_err();
        assert!(error.contains("top.u.pc"), "{error}");
        let (index, signal) = data.resolve(id, "top.u.pc").unwrap();
        assert!(index > 0);
        assert_eq!(signal.path, "top.u.pc");
    }

    #[test]
    fn values_are_read_for_every_signal_and_time() {
        let data = AgentData::new();
        let (id, _, info) = open(&data, "values", SMALL_VCD);
        let paths = vec!["top.clk".to_string(), "top.data".to_string()];
        let result = data.values(id, &paths, &[0, 15, 25], "hex", info).unwrap();
        assert_eq!(result.values.len(), 6);
        // t=15: clk changed at 10 to 1, data changed at 10 to 0xa
        let clk = result
            .values
            .iter()
            .find(|value| value.signal == "top.clk" && value.requested_time == 15)
            .unwrap();
        assert_eq!(clk.value.as_deref(), Some("1"));
        assert_eq!(clk.value_time, Some(10));
        let data_value = result
            .values
            .iter()
            .find(|value| value.signal == "top.data" && value.requested_time == 15)
            .unwrap();
        assert_eq!(data_value.value.as_deref(), Some("a"));
        // t=0 is a real read, not a miss
        let at_zero = result
            .values
            .iter()
            .find(|value| value.requested_time == 0)
            .unwrap();
        assert!(at_zero.found);
        // an unknown signal fails the whole call with the miss named
        assert!(data
            .values(id, &["top.nope".to_string()], &[0], "hex", empty_info())
            .is_err());
    }

    #[test]
    fn changes_are_capped_and_report_where_to_continue() {
        let data = AgentData::new();
        let (id, _, info) = open(&data, "changes", SMALL_VCD);
        let paths = vec!["top.clk".to_string()];
        let result = data
            .changes(id, &paths, 0, Some(30), 1, "hex", info.clone())
            .unwrap();
        assert_eq!(result.signals.len(), 1);
        let signal = &result.signals[0];
        assert!(signal.capped);
        assert_eq!(signal.changes.len(), 1);
        assert_eq!(signal.changes[0].time, 0);
        assert_eq!(signal.next_start, Some(1));
        assert!(result.truncated);
        // a wide cap returns everything and says it is not capped
        let result = data
            .changes(id, &paths, 0, Some(30), 64, "hex", info)
            .unwrap();
        assert!(!result.truncated);
        assert!(!result.signals[0].capped);
        assert_eq!(result.signals[0].next_start, None);
        // clk is declared in dumpvars and again at #0, so it has two changes at
        // t=0 plus two more later: five in a 0..=30 window
        assert_eq!(result.signals[0].changes.len(), 5);
    }

    #[test]
    fn a_value_search_reports_the_match_or_a_miss() {
        let data = AgentData::new();
        let (id, _, info) = open(&data, "search", SMALL_VCD);
        let hit = data
            .search(id, "top.clk", "1", 0, false, &ShowType::Hex, info.clone())
            .unwrap();
        assert!(hit.found);
        assert_eq!(hit.time, Some(10));
        assert_eq!(hit.value.as_deref(), Some("1"));
        let miss = data
            .search(id, "top.clk", "9", 0, false, &ShowType::Hex, info)
            .unwrap();
        assert!(!miss.found);
        assert_eq!(miss.time, None);
    }

    #[test]
    fn a_summary_counts_and_endpoints_the_range() {
        let data = AgentData::new();
        let (id, _, info) = open(&data, "summary", SMALL_VCD);
        let paths = vec!["top.clk".to_string(), "top.u.pc".to_string()];
        let result = data
            .summarize(id, &paths, 0, 30, &ShowType::Hex, "hex", info)
            .unwrap();
        let clk = result
            .signals
            .iter()
            .find(|signal| signal.signal == "top.clk")
            .unwrap();
        // clk: declared in dumpvars (0) and again at #0, then 1@10, 0@20, 1@30
        assert_eq!(clk.changes, 5);
        assert_eq!(clk.first_time, Some(0));
        assert_eq!(clk.last_time, Some(30));
        assert_eq!(clk.first_value.as_deref(), Some("0"));
        assert_eq!(clk.last_value.as_deref(), Some("1"));
        let pc = result
            .signals
            .iter()
            .find(|signal| signal.signal == "top.u.pc")
            .unwrap();
        // pc: 0@0, 3@10, 4@20; hex rendering pads to the 8-bit width
        assert_eq!(pc.changes, 3);
        assert_eq!(pc.last_value.as_deref(), Some("04"));
    }
}
