//! An open editor buffer and the coordinate systems it bridges.
//!
//! Two problems live here:
//!
//! 1. **Positions.**  LSP counts lines from 0 and columns in UTF-16 code units
//!    (that is what monaco and `vscode-languageserver` send).  The design
//!    database counts lines and columns from 1 and columns in `char`s (see the
//!    span production in `design::frontend`).  A position is converted against
//!    the real buffer text, so non-ASCII source (comments, string literals)
//!    keeps working instead of drifting by one per wide character.
//! 2. **Edits.**  `didChange` may send a full buffer or a range replacement;
//!    both are applied to the stored text, which is then reparsed by the
//!    design database.
//!
//! [`Span`]: crate::design::Span

use std::path::PathBuf;

use crate::design::Span;

use super::protocol::{Position, Range};

/// One `didOpen` document.
#[derive(Clone, Debug)]
pub struct Document {
    /// The URI the client used, echoed back in every `Location`.
    pub uri: String,
    /// The database key for this buffer (see [`super::uri`]).
    pub path: PathBuf,
    /// The `version` the client last announced (0 when it did not send one).
    pub version: i64,
    /// The buffer text, always the most recent `didChange`.
    pub text: String,
}

impl Document {
    pub fn new(uri: impl Into<String>, path: PathBuf, version: i64, text: impl Into<String>) -> Self {
        Document {
            uri: uri.into(),
            path,
            version,
            text: text.into(),
        }
    }

    /// The text of a 0-based line, without its `\r` (LSP positions never
    /// include the line terminator).
    fn line_text(&self, line: u32) -> Option<&str> {
        self.text
            .split('\n')
            .nth(line as usize)
            .map(|text| text.strip_suffix('\r').unwrap_or(text))
    }

    /// An LSP position → `(line, column)` as the design database counts them
    /// (both 1-based, columns in characters).
    ///
    /// `None` when the position is below the last line; a character offset past
    /// the end of a line is clamped to the line end, which is what a caret in
    /// trailing whitespace means.
    pub fn source_position(&self, position: Position) -> Option<(u32, u32)> {
        let line = self.line_text(position.line)?;
        let mut utf16 = 0u32;
        let mut characters = 0u32;
        for character in line.chars() {
            if utf16 >= position.character {
                break;
            }
            utf16 += character.len_utf16() as u32;
            characters += 1;
        }
        Some((position.line + 1, characters + 1))
    }

    /// A `(line, column)` pair as the design database counts them → an LSP
    /// position.
    pub fn position_of(&self, line: u32, column: u32) -> Position {
        let line_number = line.saturating_sub(1);
        let character = match self.line_text(line_number) {
            Some(text) => text
                .chars()
                .take(column.saturating_sub(1) as usize)
                .map(|character| character.len_utf16() as u32)
                .sum(),
            // The span points at a line this buffer no longer has (for example
            // after a `didChange` that shortened it).  Falling back to the raw
            // column keeps the result usable instead of rejecting the answer.
            None => column.saturating_sub(1),
        };
        Position::new(line_number, character)
    }

    /// A span of *this* document as an LSP range.
    pub fn span_range(&self, span: &Span) -> Range {
        Range::new(
            self.position_of(span.start_line, span.start_col),
            self.position_of(span.end_line, span.end_col),
        )
    }

    /// Byte offset of an LSP position, for splicing a `didChange` edit.
    fn byte_offset_of(&self, position: Position) -> Option<usize> {
        let mut line = 0u32;
        let mut line_start = 0usize;
        if position.line > 0 {
            let mut found = false;
            for (offset, byte) in self.text.bytes().enumerate() {
                if byte == b'\n' {
                    line += 1;
                    line_start = offset + 1;
                    if line == position.line {
                        found = true;
                        break;
                    }
                }
            }
            if !found {
                return None;
            }
        }
        let mut utf16 = 0u32;
        for (offset, character) in self.text[line_start..].char_indices() {
            if matches!(character, '\n' | '\r') || utf16 >= position.character {
                return Some(line_start + offset);
            }
            utf16 += character.len_utf16() as u32;
        }
        Some(self.text.len())
    }

    /// Apply one `contentChanges` entry: a full replacement when `range` is
    /// absent, a splice otherwise.
    pub fn apply_change(&mut self, range: Option<Range>, text: &str) {
        match range {
            None => self.text = text.to_owned(),
            Some(range) => {
                let start = self
                    .byte_offset_of(range.start)
                    .unwrap_or(self.text.len());
                let end = self
                    .byte_offset_of(range.end)
                    .unwrap_or(self.text.len())
                    .max(start);
                self.text.replace_range(start..end, text);
            }
        }
    }
}

/// A range for a span whose file is not open in the editor.
///
/// Without the text there is no way to know where the UTF-16 offsets are, so
/// the 1-based character columns are shifted by one and used as they are.  For
/// the ASCII SystemVerilog this project deals with that is exact; for a file
/// that is both closed and full of non-ASCII identifiers it can be off by the
/// width difference.
pub fn approximate_span_range(span: &Span) -> Range {
    Range::new(
        Position::new(
            span.start_line.saturating_sub(1),
            span.start_col.saturating_sub(1),
        ),
        Position::new(
            span.end_line.saturating_sub(1),
            span.end_col.saturating_sub(1),
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn document(text: &str) -> Document {
        Document::new("file:///tmp/a.sv", PathBuf::from("a.sv"), 1, text)
    }

    #[test]
    fn positions_bridge_utf16_and_character_columns() {
        // The emoji is one `char` but two UTF-16 code units; the `é` is one of
        // each.  A naive (column - 1) conversion would drift on line 0.
        let document = document("// \u{1f600} x\nlogic y;\n");
        let position = document.position_of(1, 6);
        assert_eq!(position, Position::new(0, 6));
        assert_eq!(document.source_position(position), Some((1, 6)));

        let position = document.position_of(2, 7);
        assert_eq!(position, Position::new(1, 6));
        assert_eq!(document.source_position(position), Some((2, 7)));
    }

    #[test]
    fn positions_past_the_end_are_clamped_or_rejected() {
        let document = document("abc\r\ndef\r\n");
        // Line 0 has three characters; asking for column 40 means "end of line".
        assert_eq!(
            document.source_position(Position::new(0, 40)),
            Some((1, 4))
        );
        // The `\r` is not part of the line.
        assert_eq!(document.source_position(Position::new(1, 3)), Some((2, 4)));
        // Below the last line there is nothing to answer with.
        assert_eq!(document.source_position(Position::new(9, 0)), None);
    }

    #[test]
    fn spans_convert_to_ranges() {
        let document = document("abc\ndefgh\n");
        let span = Span::new("a.sv", 2, 3, 2, 6);
        assert_eq!(
            document.span_range(&span),
            Range::new(Position::new(1, 2), Position::new(1, 5))
        );
        // A span in a file the buffer does not describe still yields a range.
        let other = Span::new("b.sv", 4, 2, 4, 5);
        assert_eq!(
            approximate_span_range(&other),
            Range::new(Position::new(3, 1), Position::new(3, 4))
        );
    }

    #[test]
    fn changes_apply_full_and_incremental_edits() {
        let mut document = document("module top;\nendmodule\n");
        document.apply_change(
            Some(Range::new(Position::new(0, 7), Position::new(0, 10))),
            "cpu",
        );
        assert_eq!(document.text, "module cpu;\nendmodule\n");

        // Inserting a line in the middle moves the tail.
        document.apply_change(
            Some(Range::new(Position::new(1, 0), Position::new(1, 0))),
            "  logic x;\n",
        );
        assert_eq!(document.text, "module cpu;\n  logic x;\nendmodule\n");

        document.apply_change(None, "module leaf;\nendmodule\n");
        assert_eq!(document.text, "module leaf;\nendmodule\n");
    }
}
