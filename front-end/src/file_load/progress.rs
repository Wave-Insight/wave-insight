//! Pure bookkeeping for opening a file: sizes, read percentages, the status
//! lines the top bar shows and the size guard.
//!
//! The chunked read itself (super::file) needs a browser; everything in this
//! module is plain Rust on purpose, so the *decisions* it makes are unit tested
//! on the host target (cargo test -p front-end): how far along a read is,
//! whether a dump is too large to load at all, what the status line says.
//!
//! Contract kept stable for the QA matrix: a normal load still reports
//! "waveform loaded" -- the timings are appended after that prefix.

use std::fmt::Write as _;

/// How much of the file is read per Blob::slice, in bytes. Small enough that
/// the browser can paint the progress line between chunks, large enough that a
/// 100 MB dump is a dozen reads.
pub const CHUNK_BYTES: u64 = 8 * 1024 * 1024;

/// From here on a dump is readable but may take seconds to parse: say so
/// before the parse instead of leaving the user staring at a frozen tab.
pub const LARGE_BYTES: u64 = 100 * 1024 * 1024;

/// Above this the load is refused before a byte is read: the bytes and the
/// parsed dump both live in wasm linear memory, which a browser tab cannot
/// stretch that far.
pub const REFUSE_BYTES: u64 = 512 * 1024 * 1024;

const KB: f64 = 1024.0;
const MB: f64 = 1024.0 * 1024.0;
const GB: f64 = 1024.0 * 1024.0 * 1024.0;

/// What the size guard wants to happen for a file of this many bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SizeVerdict {
    /// Read it straight away.
    Ok,
    /// Read it, but tell the user why it may take a while.
    Large,
    /// Refuse politely rather than freezing (or crashing) the tab.
    TooLarge,
}

/// A human readable size, 1024 based: 532 B, 4.0 KB, 105 MB, 1.50 GB.
pub fn format_size(bytes: u64) -> String {
    let size = bytes as f64;
    if size < KB {
        format!("{bytes} B")
    } else if size < MB {
        let kb = size / KB;
        if kb < 10.0 {
            format!("{kb:.1} KB")
        } else {
            format!("{kb:.0} KB")
        }
    } else if size < GB {
        let mb = size / MB;
        if mb < 10.0 {
            format!("{mb:.1} MB")
        } else {
            format!("{mb:.0} MB")
        }
    } else {
        let gb = size / GB;
        if gb < 10.0 {
            format!("{gb:.2} GB")
        } else {
            format!("{gb:.1} GB")
        }
    }
}

/// How far a read is, 0-100. An empty file is *complete*, not "0 %", so a
/// total of zero reads as 100.
pub fn percent(done: u64, total: u64) -> u32 {
    if total == 0 {
        return 100;
    }
    let done = u128::from(done);
    let total = u128::from(total);
    (done * 100 / total).min(100) as u32
}

/// One progress line per chunk: "reading test3.vcd: 48% (52 MB of 105 MB)".
pub fn reading_status(name: &str, done: u64, total: u64) -> String {
    format!(
        "reading {name}: {}% ({} of {})",
        percent(done, total),
        format_size(done),
        format_size(total)
    )
}

/// Short human count of signals for the "parsing ..." line: 23k signals.
pub fn format_signal_count(signals: u64) -> String {
    if signals == 1 {
        return "1 signal".to_string();
    }
    if signals < 1_000 {
        return format!("{signals} signals");
    }
    if signals < 10_000 {
        return format!("{:.1}k signals", signals as f64 / 1_000.0);
    }
    if signals < 1_000_000 {
        return format!("{:.0}k signals", signals as f64 / 1_000.0);
    }
    if signals < 10_000_000 {
        return format!("{:.2}M signals", signals as f64 / 1_000_000.0);
    }
    format!("{:.1}M signals", signals as f64 / 1_000_000.0)
}

/// "parsing test3.vcd (105 MB, 23k signals)...".
///
/// The signal count is the cheap $var header estimate collected while the bytes
/// were read (SignalEstimate); FST/GHW pass None because their signal table can
/// only be read by the parser itself.
pub fn parsing_status(name: &str, bytes: u64, signals: Option<u64>) -> String {
    match signals {
        Some(signals) if signals > 0 => format!(
            "parsing {name} ({}, {})…",
            format_size(bytes),
            format_signal_count(signals)
        ),
        _ => format!("parsing {name} ({})…", format_size(bytes)),
    }
}

/// Milliseconds as the user should read them: 412 ms, 2.0 s.
pub fn format_duration_ms(ms: f64) -> String {
    let ms = if ms.is_finite() { ms.max(0.0) } else { 0.0 };
    if ms < 1000.0 {
        format!("{ms:.0} ms")
    } else {
        format!("{:.1} s", ms / 1000.0)
    }
}

/// The cost of the parse, appended to whatever line already says what opened:
/// "opened in 412 ms, 105 MB, wasm memory 320 MB".
pub fn stats_suffix(parse_ms: f64, bytes: u64, memory_mb: Option<f64>) -> String {
    let mut text = format!(
        "opened in {}, {}",
        format_duration_ms(parse_ms),
        format_size(bytes)
    );
    if let Some(memory) = memory_mb {
        if memory.is_finite() {
            let _ = write!(text, ", wasm memory {memory:.0} MB");
        }
    }
    text
}

/// The success line: the "waveform loaded" every earlier build printed (the QA
/// matrix asserts it) extended with what the parse cost.
pub fn loaded_status(parse_ms: f64, bytes: u64, memory_mb: Option<f64>) -> String {
    format!(
        "waveform loaded — {}",
        stats_suffix(parse_ms, bytes, memory_mb)
    )
}

/// The cancel line: the file is *not* opened and nothing is left half-loaded.
pub fn cancelled_status(name: &str) -> String {
    format!("load cancelled — {name} was not opened")
}

/// The guard decision for a file (or a blob of bytes) of this size.
pub fn size_verdict(bytes: u64) -> SizeVerdict {
    if bytes > REFUSE_BYTES {
        SizeVerdict::TooLarge
    } else if bytes >= LARGE_BYTES {
        SizeVerdict::Large
    } else {
        SizeVerdict::Ok
    }
}

/// What to tell the user about the size guard, if anything.
///
/// The refusal is the actionable one: the user can still open a trimmed dump,
/// they only cannot open *this* one, so the message says what to change instead
/// of only "too large".
pub fn guard_message(name: &str, bytes: u64) -> Option<String> {
    match size_verdict(bytes) {
        SizeVerdict::Ok => None,
        SizeVerdict::Large => Some(format!(
            "{name}: large dump ({}) — parsing may take a few seconds and several hundred MB of memory",
            format_size(bytes)
        )),
        SizeVerdict::TooLarge => Some(format!(
            "{name}: {} is more than the {} a browser tab can hold in wasm memory — trim the dump (a shorter time range or fewer signals) and load it again",
            format_size(bytes),
            format_size(REFUSE_BYTES)
        )),
    }
}

/// Incremental counter for the $var declarations of a VCD header.
///
/// The "parsing ..." line is shown *before* the parse, so its signal count can
/// only be an estimate. The browser reader feeds every chunk through here while
/// the bytes are already in hand (no second pass), and counting stops at
/// $enddefinitions -- a VCD declares all of its signals in the header, so the
/// body (which is where a large dump spends its bytes) is never scanned.
#[derive(Debug, Default)]
pub struct SignalEstimate {
    count: u64,
    /// The tail of the previous chunk: a token can straddle the boundary.
    tail: Vec<u8>,
    finished: bool,
}

const VAR_TOKEN: &[u8] = b"$var";
const END_DEFINITIONS_TOKEN: &[u8] = b"$enddefinitions";
/// Bytes of the previous chunk kept so a token split across two chunks is
/// still found: one byte short of the longest token.
const KEEP: usize = END_DEFINITIONS_TOKEN.len() - 1;

impl SignalEstimate {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed the next chunk of the file. Empty chunks are ignored; once
    /// $enddefinitions was seen every further chunk is ignored too.
    pub fn feed(&mut self, chunk: &[u8]) {
        if self.finished || chunk.is_empty() {
            return;
        }
        let tail_len = self.tail.len();
        let mut scan = std::mem::take(&mut self.tail);
        scan.extend_from_slice(chunk);

        // matches starting in the retained tail were already counted by the
        // previous feed; only the ones that were incomplete there are new
        let from = tail_len.saturating_sub(VAR_TOKEN.len() - 1);

        if let Some(end) = find(&scan, END_DEFINITIONS_TOKEN) {
            // the header ends inside this chunk: count the variables declared
            // before it and keep nothing (the body never declares a signal)
            self.count += count_token_from(&scan[..end], VAR_TOKEN, from);
            self.finished = true;
            return;
        }

        self.count += count_token_from(&scan, VAR_TOKEN, from);
        self.tail = scan[scan.len().saturating_sub(KEEP)..].to_vec();
    }

    /// The number of $var declarations seen so far.
    pub fn count(&self) -> u64 {
        self.count
    }
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn count_token_from(haystack: &[u8], needle: &[u8], from: usize) -> u64 {
    let mut count = 0;
    let mut index = from;
    while index + needle.len() <= haystack.len() {
        if &haystack[index..index + needle.len()] == needle {
            count += 1;
            index += needle.len();
        } else {
            index += 1;
        }
    }
    count
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn size_formatting_is_readable() {
        assert_eq!(format_size(0), "0 B");
        assert_eq!(format_size(532), "532 B");
        assert_eq!(format_size(4 * 1024), "4.0 KB");
        assert_eq!(format_size(64 * 1024), "64 KB");
        assert_eq!(format_size(105 * 1024 * 1024), "105 MB");
        assert_eq!(format_size(52 * 1024 * 1024), "52 MB");
        assert_eq!(format_size(512 * 1024 * 1024), "512 MB");
        assert_eq!(
            format_size(1024 * 1024 * 1024 + 512 * 1024 * 1024),
            "1.50 GB"
        );
        assert_eq!(format_size(20 * 1024 * 1024 * 1024), "20.0 GB");
    }

    #[test]
    fn percentage_is_clamped_and_empty_is_complete() {
        assert_eq!(percent(0, 100), 0);
        assert_eq!(percent(50, 100), 50);
        assert_eq!(percent(100, 100), 100);
        assert_eq!(percent(0, 0), 100);
        // the last chunk may overshoot a stale total; never show > 100 %
        assert_eq!(percent(120, 100), 100);
        assert_eq!(percent(u64::MAX, u64::MAX), 100);
    }

    #[test]
    fn reading_status_matches_the_documented_shape() {
        let total = 105 * 1024 * 1024;
        let done = 52 * 1024 * 1024;
        assert_eq!(
            reading_status("test3.vcd", done, total),
            "reading test3.vcd: 49% (52 MB of 105 MB)"
        );
        assert!(reading_status("x.vcd", 0, 0).ends_with("100% (0 B of 0 B)"));
    }

    #[test]
    fn guard_thresholds_are_exact() {
        assert_eq!(size_verdict(0), SizeVerdict::Ok);
        assert_eq!(size_verdict(LARGE_BYTES - 1), SizeVerdict::Ok);
        assert_eq!(size_verdict(LARGE_BYTES), SizeVerdict::Large);
        // exactly 512 MB is still allowed (it only warns), one byte more is not
        assert_eq!(size_verdict(REFUSE_BYTES), SizeVerdict::Large);
        assert_eq!(size_verdict(REFUSE_BYTES + 1), SizeVerdict::TooLarge);
        assert!(guard_message("large.vcd", LARGE_BYTES).is_some());
        assert!(guard_message("tiny.vcd", 1024).is_none());
    }

    #[test]
    fn guard_messages_are_actionable() {
        let large = guard_message("test3.vcd", 105 * 1024 * 1024).expect("a large dump warns");
        assert!(large.contains("test3.vcd"));
        assert!(large.contains("large dump"));
        assert!(large.contains("parsing may take a few seconds and several hundred MB of memory"));
        assert!(large.contains("105 MB"));

        let refused =
            guard_message("huge.vcd", REFUSE_BYTES + 1).expect("too large is refused with text");
        assert!(refused.contains("huge.vcd"));
        assert!(refused.contains("512 MB"));
        assert!(refused.contains("wasm memory"));
        // the refusal has to tell the user what to do about it
        assert!(refused.contains("trim the dump"));
    }

    #[test]
    fn parsing_status_shows_the_signal_estimate() {
        let total = 105 * 1024 * 1024;
        assert_eq!(
            parsing_status("test3.vcd", total, Some(23_000)),
            "parsing test3.vcd (105 MB, 23k signals)…"
        );
        assert_eq!(
            parsing_status("dump.fst", total, None),
            "parsing dump.fst (105 MB)…"
        );
        // zero is not shown: "0 signals" would be a lie when the scan saw nothing
        assert_eq!(
            parsing_status("dump.vcd", total, Some(0)),
            "parsing dump.vcd (105 MB)…"
        );
    }

    #[test]
    fn signal_count_formatting() {
        assert_eq!(format_signal_count(1), "1 signal");
        assert_eq!(format_signal_count(2), "2 signals");
        assert_eq!(format_signal_count(999), "999 signals");
        assert_eq!(format_signal_count(2_300), "2.3k signals");
        assert_eq!(format_signal_count(23_000), "23k signals");
        assert_eq!(format_signal_count(1_500_000), "1.50M signals");
    }

    #[test]
    fn success_line_keeps_the_stable_prefix() {
        let line = loaded_status(412.0, 105 * 1024 * 1024, Some(320.4));
        assert!(line.starts_with("waveform loaded"));
        assert!(line.contains("412 ms"));
        assert!(line.contains("105 MB"));
        assert!(line.contains("wasm memory 320 MB"));
        // without a readable wasm memory size the numbers still appear
        let line = loaded_status(2_000.0, 1024, None);
        assert!(line.starts_with("waveform loaded"));
        assert!(line.contains("2.0 s"));
        assert!(!line.contains("wasm memory"));
        assert_eq!(
            stats_suffix(412.0, 1024 * 1024, None),
            "opened in 412 ms, 1.0 MB"
        );
        // a NaN duration (a broken clock) must not print "NaN ms"
        assert!(!loaded_status(f64::NAN, 1, None).contains("NaN"));
    }

    #[test]
    fn cancel_line_says_nothing_was_opened() {
        let line = cancelled_status("test3.vcd");
        assert!(line.starts_with("load cancelled"));
        assert!(line.contains("test3.vcd"));
        assert!(line.contains("was not opened"));
    }

    #[test]
    fn signal_estimate_counts_header_vars_only() {
        let header = "$date today $end\n\
                      $version v $end\n\
                      $timescale 1ns $end\n\
                      $var wire 1 ! a $end\n\
                      $var wire 8 \" bus [7:0] $end\n\
                      $var reg 1 # clk $end\n\
                      $enddefinitions $end\n\
                      #0\n$var 1 ! 0\n";
        let mut estimate = SignalEstimate::new();
        estimate.feed(header.as_bytes());
        assert_eq!(estimate.count(), 3);
        // the body is never counted, even if it contains the token
        estimate.feed(b"$var wire 1 !! bogus $end\n");
        assert_eq!(estimate.count(), 3);
    }

    #[test]
    fn signal_estimate_survives_every_chunk_boundary() {
        let header = "$var wire 1 ! a $end $var wire 2 \" b [1:0] $end \
                      $var reg 4 # c $end $var wire 1 $ d $end $enddefinitions $end #0";
        let expected = 4;
        // one chunk: no boundary at all
        let mut estimate = SignalEstimate::new();
        estimate.feed(header.as_bytes());
        assert_eq!(estimate.count(), expected);

        // the token split at *every* possible point still counts once
        for split in 1..header.len() {
            let mut estimate = SignalEstimate::new();
            estimate.feed(&header.as_bytes()[..split]);
            estimate.feed(&header.as_bytes()[split..]);
            assert_eq!(
                estimate.count(),
                expected,
                "split at {split} counted wrong for {header:?}"
            );
        }

        // one byte at a time (the worst case for the carry logic)
        let mut estimate = SignalEstimate::new();
        for byte in header.as_bytes() {
            estimate.feed(&[*byte]);
        }
        assert_eq!(estimate.count(), expected);
    }

    #[test]
    fn signal_estimate_ignores_empty_chunks() {
        let mut estimate = SignalEstimate::new();
        estimate.feed(b"");
        assert_eq!(estimate.count(), 0);
        estimate.feed(b"$var wire 1 ! a $end ");
        estimate.feed(b"");
        assert_eq!(estimate.count(), 1);
    }
}
