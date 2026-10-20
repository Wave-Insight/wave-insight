//! Cheap time range for a VCD file, without parsing the body.
//!
//! `wellen` only reports the time range after it has read the whole body, which
//! is exactly the cost the viewer must not pay when it opens a dump. For VCD the
//! information is trivially available in the text: the last `#<number>` line is
//! the end of the trace and the first one is its start. This module scans the
//! bytes for those lines (no value decoding, no allocation per line) and ignores
//! `$comment ... $end` blocks, which may legally contain anything, including
//! something that looks like a timestamp.
//!
//! The result is a *hint*: a truncated body ends at its last timestamp (which is
//! correct), and a file with a `#` line inside an `$end`-less comment block could
//! be misread. [`super::Waveform::end_time`] stays the exact but expensive
//! answer.

use std::io::{self, BufRead};

use super::TimeRange;

/// Largest timestamp we accept; a stray huge number would otherwise blow up the
/// viewer's zoom. `i32::MAX` is the legacy limit and a sane bound for a viewer.
const MAX_TIME: u64 = i32::MAX as u64;

/// Scan a VCD body for its first and last timestamp.
///
/// Returns `None` if the body has no timestamp at all (an empty or malformed
/// dump), which the caller treats as "unknown".
pub fn scan_vcd_time_range(input: &mut impl BufRead) -> io::Result<Option<TimeRange>> {
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
        assert_eq!(scanner.finish(), Some(TimeRange { start: 1, end: 1_234_567 }));
    }

    #[test]
    fn a_command_token_must_be_whole() {
        //`$endx` inside a command does not close it
        assert_eq!(scan("$comment keep going $endx\n#777\n$end\n#3\n0!\n"), Some(TimeRange { start: 3, end: 3 }));
        //`$end` followed by a newline does
        assert_eq!(scan("$comment a b c $end\n#4\n0!\n"), Some(TimeRange { start: 4, end: 4 }));
    }
}
