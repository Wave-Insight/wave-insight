//! Cheap time range for a VCD file, without parsing the body.
//!
//! `wellen` only reports the time range after it has read the whole body, which
//! is exactly the cost the viewer must not pay when it opens a dump. For VCD the
//! information is available in the text: the first timestamp token starts the
//! trace and the last one ends it. This module finds the first one with a
//! bounded forward scan over the header and the last one by reading only the
//! tail of the file, so the cost does not grow with the body (growing the tail
//! window and, ultimately, the exhaustive whole-file scan handle layouts the
//! bounded scans cannot decide). No value is decoded and nothing is allocated
//! per line/signal, and `$comment ... $end` blocks are skipped because they may
//! legally contain anything, including something that looks like a timestamp.
//!
//! The result is a *hint*: a truncated body ends at its last timestamp (which is
//! correct), and a window that starts inside an unterminated trailing comment
//! could still be misread (the exhaustive scan is the fallback whenever the
//! bounded scan notices the ambiguity). [`super::Waveform::end_time`] stays the
//! exact but expensive answer.

use std::io::{self, BufRead, Seek, SeekFrom};

use super::TimeRange;

/// Largest timestamp we accept; a stray huge number would otherwise blow up the
/// viewer's zoom. `i32::MAX` is the legacy limit and a sane bound for a viewer.
const MAX_TIME: u64 = i32::MAX as u64;

/// Budget of the forward scan that looks for the first time stamp.
///
/// The header (scope and var declarations) is small compared to the body, so a
/// few KiB are normally enough.  A dump whose first time stamp is further in
/// than this falls back to the exhaustive scan instead of guessing.
const START_SCAN_LIMIT: u64 = 1 << 20;

/// First tail block of the backward search for the last time stamp.
const TAIL_BLOCK: u64 = 16 << 10;

/// Largest tail read before giving up to the exhaustive scan.
const TAIL_SCAN_LIMIT: u64 = 4 << 20;

/// Cheap first/last time stamp for a VCD body.
///
/// The first time stamp is found with a bounded forward scan (the header is a
/// few KiB in practice) and the last one by reading only the tail of the file,
/// so opening a dump costs O(header + tail) instead of O(file).  Anything the
/// bounded scans cannot decide - no time stamp in the search window, an
/// unterminated command block, a tail window that starts inside a command - is
/// answered by the exhaustive [scan_vcd_time_range_full], so the hint can never
/// become less accurate than the old whole-file scan.
///
/// A "#" is accepted after any whitespace, not only at the start of a line: VCD
/// is a token format and the timestamp may share its line with a value (the old
/// scanner missed those layouts).
pub fn scan_vcd_time_range(input: &mut (impl BufRead + Seek)) -> io::Result<Option<TimeRange>> {
    let length = input.seek(SeekFrom::End(0))?;
    if length == 0 {
        return Ok(None);
    }
    input.seek(SeekFrom::Start(0))?;
    let start = scan_start_timestamp(input, length)?;
    let end = match start {
        Some(_) => scan_tail_timestamp(input, length)?,
        None => None,
    };
    match (start, end) {
        (Some(start), Some(end)) => Ok(Some(TimeRange { start, end })),
        _ => scan_vcd_time_range_full(input),
    }
}

/// Forward scan for the first time stamp, bounded to
/// min(length, START_SCAN_LIMIT) bytes.
///
/// Stops at the first "#digits" token outside a command block.  If the budget
/// runs out first (a huge header, or the file has no time stamp) the caller
/// falls back to the exhaustive scan.
fn scan_start_timestamp(input: &mut impl BufRead, length: u64) -> io::Result<Option<u64>> {
    let budget = length.min(START_SCAN_LIMIT);
    let mut read = 0u64;
    let mut buffer = [0u8; 16 * 1024];
    let mut scanner = TokenScan::new();
    while read < budget {
        let want = (budget - read).min(buffer.len() as u64) as usize;
        let bytes = input.read(&mut buffer[..want])?;
        if bytes == 0 {
            break;
        }
        read += bytes as u64;
        scanner.feed(&buffer[..bytes]);
        if let Some(first) = scanner.first {
            return Ok(Some(first));
        }
        if scanner.in_command {
            // the first time stamp may be inside this command block: let the
            // exhaustive scan answer rather than look behind it
            return Ok(None);
        }
    }
    Ok(None)
}

/// Read the tail of the file in growing blocks and return the last time stamp
/// found outside a command block.
fn scan_tail_timestamp(input: &mut (impl BufRead + Seek), length: u64) -> io::Result<Option<u64>> {
    let mut window = length.min(TAIL_BLOCK);
    loop {
        input.seek(SeekFrom::Start(length - window))?;
        let mut tail = vec![0u8; window as usize];
        input.read_exact(&mut tail)?;
        let mut scanner = TokenScan::new();
        scanner.feed(&tail);
        if !scanner.unmatched_end && scanner.last.is_some() {
            return Ok(scanner.last);
        }
        if window >= length || window >= TAIL_SCAN_LIMIT {
            return Ok(None);
        }
        window = window.saturating_mul(2).min(length);
    }
}

/// Exhaustive scan of the whole body: the old behaviour, used as a fallback.
///
/// Returns None if the body has no timestamp at all (an empty or malformed
/// dump), which the caller treats as "unknown".
pub fn scan_vcd_time_range_full(
    input: &mut (impl BufRead + Seek),
) -> io::Result<Option<TimeRange>> {
    input.seek(SeekFrom::Start(0))?;
    let mut scanner = Scanner::default();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = input.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        scanner.feed(&buffer[..read]);
    }
    Ok(scanner.finish())
}

/// Token scanner used by both bounded scans.
///
/// Unlike [Scanner] it takes the first *and* last time stamp it sees (the caller
/// decides whether to stop) and accepts a "#" after whitespace, so mid-line
/// timestamps are found.  Command blocks ($comment etc.) are skipped the same
/// way the exhaustive scanner skips them.
#[derive(Default)]
struct TokenScan {
    /// Previous byte in the stream; None at the start of a window.
    prev: Option<u8>,
    at_line_start: bool,
    in_command: bool,
    pending_end: bool,
    window: [u8; 8],
    window_len: usize,
    reading_time: bool,
    digits: usize,
    current: u64,
    /// Name of the command block being skipped, to recognise a window that
    /// starts with the closing "$end" of a block opened before the window.
    command_name: [u8; 8],
    command_name_len: usize,
    command_name_done: bool,
    /// "$end" was seen while the command name was still being read: either the
    /// window starts inside a command (ambiguous) or the name is a longer
    /// command such as "$enddefinitions" (fine); the next bytes decide.
    candidate_closing_end: bool,
    /// First time stamp of the scanned bytes.
    first: Option<u64>,
    /// Largest time stamp of the scanned bytes (VCD time stamps are monotonic,
    /// so this is the last one; taking the max keeps the exhaustive scanner's
    /// "best effort on a truncated dump" behaviour).
    last: Option<u64>,
    /// A "$end" token was seen outside a command block, i.e. the window started
    /// inside a command: the window cannot be trusted.
    unmatched_end: bool,
}

impl TokenScan {
    fn new() -> Self {
        Self {
            at_line_start: true,
            ..Self::default()
        }
    }

    fn feed(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.feed_byte(byte);
        }
    }

    fn at_token_start(&self) -> bool {
        self.prev
            .is_none_or(|previous| previous.is_ascii_whitespace())
    }

    fn feed_byte(&mut self, byte: u8) {
        if self.reading_time {
            if byte.is_ascii_digit() {
                // saturating: a corrupt file must not wrap the number
                self.current = self
                    .current
                    .saturating_mul(10)
                    .saturating_add(u64::from(byte - b'0'))
                    .min(MAX_TIME);
                self.digits += 1;
                self.prev = Some(byte);
                return;
            }
            self.finish_time();
            // fall through: this byte is handled as a normal one below
        }

        if self.in_command {
            self.feed_command_byte(byte);
            if byte == b'\n' {
                self.at_line_start = true;
            } else if !byte.is_ascii_whitespace() {
                self.at_line_start = false;
            }
            self.prev = Some(byte);
            return;
        }

        // A "$end" outside a command means the window started inside one.
        if self.window_ends_with_end(byte) {
            self.unmatched_end = true;
        }

        if byte == b'#' && self.at_token_start() {
            self.reading_time = true;
            self.digits = 0;
            self.current = 0;
        } else if byte == b'$' && self.at_line_start {
            self.in_command = true;
            self.pending_end = false;
            self.window = [0u8; 8];
            self.window_len = 1;
            self.window[0] = b'$';
            self.command_name = [0u8; 8];
            self.command_name[0] = b'$';
            self.command_name_len = 1;
            self.command_name_done = false;
        }

        if byte == b'\n' {
            self.at_line_start = true;
        } else if !byte.is_ascii_whitespace() {
            self.at_line_start = false;
        }
        self.prev = Some(byte);
    }

    /// Rolling "$end" match outside a command block (a window that starts inside
    /// a command).  Only used for the ambiguity flag, never to parse.
    fn window_ends_with_end(&mut self, byte: u8) -> bool {
        if self.window_len == self.window.len() {
            self.window.copy_within(1.., 0);
            self.window_len -= 1;
        }
        self.window[self.window_len] = byte;
        self.window_len += 1;
        self.window[..self.window_len].ends_with(b"$end")
    }

    fn feed_command_byte(&mut self, byte: u8) {
        // A window that starts inside a command sees its closing "$end" as the
        // first token of a command.  That is not a command start: remember the
        // window as untrustworthy instead of silently eating the real content.
        if !self.command_name_done && !self.pending_end {
            if byte.is_ascii_whitespace() {
                self.command_name_done = true;
                if self.candidate_closing_end {
                    self.unmatched_end = true;
                }
                self.candidate_closing_end = false;
            } else if self.command_name_len < self.command_name.len() {
                self.command_name[self.command_name_len] = byte;
                self.command_name_len += 1;
                self.candidate_closing_end = false;
            }
        }
        if self.pending_end {
            // "$end" as the first token of a command means the window started
            // inside a command; a longer command name such as "$enddefinitions"
            // clears the flag on the next byte.
            if !self.command_name_done && &self.command_name[..self.command_name_len] == b"$end" {
                self.candidate_closing_end = true;
            }
            self.pending_end = false;
            if byte.is_ascii_whitespace() {
                if self.candidate_closing_end {
                    self.unmatched_end = true;
                }
                self.candidate_closing_end = false;
                self.in_command = false;
                self.window_len = 0;
                return;
            }
            self.candidate_closing_end = false;
        }
        if self.window_len == self.window.len() {
            self.window.copy_within(1.., 0);
            self.window_len -= 1;
        }
        self.window[self.window_len] = byte;
        self.window_len += 1;
        if self.window[..self.window_len].ends_with(b"$end") {
            self.pending_end = true;
        }
    }

    fn finish_time(&mut self) {
        self.reading_time = false;
        if self.digits > 0 {
            let value = self.current;
            if self.first.is_none() {
                self.first = Some(value);
            }
            self.last = Some(self.last.map_or(value, |last| last.max(value)));
        }
        self.current = 0;
        self.digits = 0;
    }
}

/// Byte level state machine: line start, `$command ... $end` blocks and
/// `#<digits>` tokens.
///
/// A `#` only counts at the start of a line, and only outside a command block:
/// `$date`, `$version` and `$comment` may legally contain anything, including a
/// line that looks like a timestamp.
#[derive(Default)]
struct Scanner {
    /// At the beginning of a line (no non-whitespace byte seen yet).
    at_line_start: bool,
    /// Inside a `$comment`/`$date`/`$version`/... block that has not seen `$end`.
    in_command: bool,
    /// We are reading the digits of a `#<number>` token at a line start.
    reading_time: bool,
    current: u64,
    first: Option<u64>,
    last: Option<u64>,
    /// Rolling window while inside a command, to find the closing `$end`.
    window: [u8; 8],
    window_len: usize,
    /// We just saw `$end`; the next byte decides whether it is the real token.
    pending_end: bool,
    initialized: bool,
}

impl Scanner {
    fn feed(&mut self, bytes: &[u8]) {
        if !self.initialized {
            self.at_line_start = true;
            self.initialized = true;
        }
        for &byte in bytes {
            if self.reading_time {
                if byte.is_ascii_digit() {
                    //saturating: a corrupt file must not wrap the number
                    self.current = self
                        .current
                        .saturating_mul(10)
                        .saturating_add((byte - b'0') as u64)
                        .min(MAX_TIME);
                    continue;
                }
                self.finish_time();
                //fall through: this byte is handled as a normal one below
            }

            if self.in_command {
                self.feed_command_byte(byte);
                if byte == b'\n' {
                    self.at_line_start = true;
                } else if !byte.is_ascii_whitespace() {
                    self.at_line_start = false;
                }
                continue;
            }

            match byte {
                b'\n' => {
                    self.at_line_start = true;
                }
                b' ' | b'\t' | b'\r' => {}
                _ => {
                    if self.at_line_start {
                        match byte {
                            b'#' => {
                                self.at_line_start = false;
                                self.reading_time = true;
                                self.current = 0;
                            }
                            b'$' => {
                                //a command block: `$date`/`$version`/`$comment`
                                //may contain anything, including fake timestamps
                                self.at_line_start = false;
                                self.in_command = true;
                                self.pending_end = false;
                                self.window = [0; 8];
                                self.window_len = 1;
                                self.window[0] = b'$';
                            }
                            _ => self.at_line_start = false,
                        }
                    }
                }
            }
        }
    }

    /// Feed one byte to the `$end` matcher.
    ///
    /// `$end` only closes the block when it is a whole token: `$endx` does not.
    fn feed_command_byte(&mut self, byte: u8) {
        if self.pending_end {
            self.pending_end = false;
            if byte.is_ascii_whitespace() {
                self.in_command = false;
                self.window_len = 0;
                return;
            }
        }
        if self.window_len == self.window.len() {
            self.window.copy_within(1.., 0);
            self.window_len -= 1;
        }
        self.window[self.window_len] = byte;
        self.window_len += 1;
        if self.window[..self.window_len].ends_with(b"$end") {
            self.pending_end = true;
        }
    }

    fn finish_time(&mut self) {
        self.reading_time = false;
        let value = self.current;
        self.current = 0;
        self.first.get_or_insert(value);
        self.last = Some(self.last.map_or(value, |last| last.max(value)));
    }

    fn finish(mut self) -> Option<TimeRange> {
        if self.reading_time {
            self.finish_time();
        }
        let (start, end) = (self.first?, self.last?);
        Some(TimeRange { start, end })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scan(text: &str) -> Option<TimeRange> {
        scan_vcd_time_range(&mut io::Cursor::new(text.as_bytes().to_vec())).unwrap()
    }

    #[test]
    fn finds_the_first_and_last_timestamp() {
        let vcd = "$timescale 1ns $end\n\
                   $scope module top $end\n\
                   $var wire 1 ! clk $end\n\
                   $upscope $end\n\
                   $enddefinitions $end\n\
                   #0\n0!\n#10\n1!\n#250\n0!\n";
        assert_eq!(scan(vcd), Some(TimeRange { start: 0, end: 250 }));
    }

    #[test]
    fn ignores_hash_characters_that_are_not_timestamps() {
        let vcd = "$scope module top $end\n\
                   $var wire 8 ! data $end\n\
                   $upscope $end\n\
                   $enddefinitions $end\n\
                   b1x#z01 !\n\
                   #7\nr1.5 !\n";
        //the `#` inside the value must not become a timestamp
        assert_eq!(scan(vcd), Some(TimeRange { start: 7, end: 7 }));
    }

    #[test]
    fn skips_comment_blocks() {
        let vcd = "$comment this is a note\n#999999\nmore text $end\n\
                   #0\n0!\n#42\n1!\n";
        assert_eq!(scan(vcd), Some(TimeRange { start: 0, end: 42 }));
    }

    #[test]
    fn skips_date_and_version_blocks() {
        //`$date` and `$version` are free text too, and may contain a line that
        //looks exactly like a timestamp
        let vcd = "$date today\n#12345\n$end\n\
                   $version sim\n#54321\n$end\n\
                   $timescale 1ns $end\n\
                   $scope module top $end\n\
                   $var wire 1 ! clk $end\n\
                   $upscope $end\n\
                   $enddefinitions $end\n\
                   #0\n0!\n#90\n1!\n";
        assert_eq!(scan(vcd), Some(TimeRange { start: 0, end: 90 }));
    }

    #[test]
    fn an_end_token_needs_to_be_whole() {
        //`$endx` does not close a command block
        let vcd = "$comment keep going $endx\n#777\n$end\n#3\n0!\n";
        assert_eq!(scan(vcd), Some(TimeRange { start: 3, end: 3 }));
    }

    #[test]
    fn crlf_line_endings_are_fine() {
        let vcd = "#0\r\n0!\r\n#12\r\n1!\r\n";
        assert_eq!(scan(vcd), Some(TimeRange { start: 0, end: 12 }));
    }

    #[test]
    fn an_unterminated_command_swallows_the_rest() {
        //a broken header is reported as "unknown" instead of a garbage number
        let vcd = "$comment never closed\n#99\n0!\n";
        assert_eq!(scan(vcd), None);
    }

    #[test]
    fn a_truncated_body_ends_at_its_last_timestamp() {
        let vcd = "#5\n0!\n#99\n1!\n#12";
        assert_eq!(scan(vcd), Some(TimeRange { start: 5, end: 99 }));
    }

    #[test]
    fn no_timestamps_is_none() {
        assert_eq!(scan(""), None);
        assert_eq!(scan("$scope module top $end\n$upscope $end\n"), None);
    }

    #[test]
    fn absurd_numbers_are_clamped() {
        let vcd = "#99999999999999999999999999\n0!\n";
        assert_eq!(
            scan(vcd),
            Some(TimeRange {
                start: MAX_TIME,
                end: MAX_TIME
            })
        );
    }

    #[test]
    fn works_across_chunk_boundaries() {
        //the scanner is fed 64k chunks; a `#` at the very end of a chunk must
        //still be seen (feed it byte by byte here)
        let vcd = "#1\n0!\n#1234567\n1!\n";
        let mut scanner = Scanner::default();
        for byte in vcd.as_bytes() {
            scanner.feed(&[*byte]);
        }
        assert_eq!(
            scanner.finish(),
            Some(TimeRange {
                start: 1,
                end: 1_234_567
            })
        );
    }

    #[test]
    fn a_command_token_must_be_whole() {
        //`$endx` inside a command does not close it
        assert_eq!(
            scan("$comment keep going $endx\n#777\n$end\n#3\n0!\n"),
            Some(TimeRange { start: 3, end: 3 })
        );
        //`$end` followed by a newline does
        assert_eq!(
            scan("$comment a b c $end\n#4\n0!\n"),
            Some(TimeRange { start: 4, end: 4 })
        );
    }

    // -- cheap scan corner layouts (Wave 5) --------------------------------

    #[test]
    fn a_timestamp_may_share_its_line_with_a_value() {
        // VCD is a token format: the time stamp does not have to start the line.
        let vcd = "$scope module top $end\n\
                   $var wire 1 ! clk $end\n\
                   $upscope $end\n\
                   $enddefinitions $end\n\
                    0! #7\n\
                    1! #9\n";
        assert_eq!(scan(vcd), Some(TimeRange { start: 7, end: 9 }));
    }

    #[test]
    fn a_comment_after_the_last_timestamp_is_not_the_end() {
        // The tail scan must not take the fake #999 inside the comment for the
        // trace end, even when it is the last hash in the file.
        let vcd = "#0\n0!\n#20\n1!\n$comment trailing note #999 $end\n";
        assert_eq!(scan(vcd), Some(TimeRange { start: 0, end: 20 }));
    }

    #[test]
    fn a_comment_before_the_last_timestamp_is_skipped() {
        let vcd = "#0\n0!\n$comment note #999 $end\n#20\n1!\n";
        assert_eq!(scan(vcd), Some(TimeRange { start: 0, end: 20 }));
    }

    #[test]
    fn a_missing_final_newline_is_fine() {
        let vcd = "#0\n0!\n#20\n1!";
        assert_eq!(scan(vcd), Some(TimeRange { start: 0, end: 20 }));
    }

    #[test]
    fn an_empty_body_has_no_range() {
        let vcd = "$date today $end\n\
                   $timescale 1ns $end\n\
                   $scope module top $end\n\
                   $upscope $end\n\
                   $enddefinitions $end\n";
        assert_eq!(scan(vcd), None);
    }

    #[test]
    fn a_long_header_is_scanned_across_read_chunks() {
        // More than one 16 KiB read chunk before the first time stamp: the
        // forward scan has to keep its command-block state across chunks.
        let mut vcd = String::from("$scope module top $end\n");
        for index in 0..2000 {
            vcd.push_str(&format!(
                "$var wire 1 {} sig{index} $end\n",
                (b'!' + (index % 90) as u8) as char
            ));
        }
        vcd.push_str("$upscope $end\n$enddefinitions $end\n#0\n0!\n#10\n1!\n");
        assert_eq!(scan(&vcd), Some(TimeRange { start: 0, end: 10 }));
    }

    #[test]
    fn a_comment_larger_than_the_tail_block_is_expanded_and_skipped() {
        // The first 16 KiB tail window starts inside the trailing comment, so
        // the "unmatched $end" guard has to expand the window until the
        // $comment start is visible; the fake #999s inside it are ignored.
        let mut vcd = String::from("#0\n0!\n#20\n1!\n$comment big trailing note\n");
        while vcd.len() < 40 * 1024 {
            vcd.push_str("padding #999\n");
        }
        vcd.push_str("$end\n");
        assert_eq!(scan(&vcd), Some(TimeRange { start: 0, end: 20 }));
    }

    #[test]
    fn the_cheap_scan_agrees_with_the_exhaustive_scan_on_regular_layouts() {
        for vcd in [
            "#0\n0!\n#10\n1!\n#250\n0!\n",
            "$comment c $end\n#3\n0!\n#4\n1!\n",
            "#0\r\n0!\r\n#12\r\n1!\r\n",
            "#5\n0!\n#99\n1!\n#12",
            "$date x\n#1\n$end\n$version y\n#2\n$end\n#7\n0!\n",
        ] {
            let fast = scan(vcd);
            let full = scan_vcd_time_range_full(&mut io::Cursor::new(vcd.as_bytes().to_vec()))
                .expect("cursor scan");
            assert_eq!(fast, full, "{vcd:?}");
        }
    }
}
