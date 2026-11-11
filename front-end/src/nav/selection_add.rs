//! Verdi's "select a block of code, add every signal it mentions".
//!
//! The Verdi flow the user asked for is: select a chunk of RTL (a declaration
//! block, an `always` body, a port map) and press one shortcut; every signal
//! the block involves lands in the wave view. The wave pane and the source
//! editor already agree on what a *click* on an identifier means
//! ([`SourceClick`] -> App::candidates_at), so this module only adds the pure
//! part of the block flow:
//!
//! 1. [`clicks`] turns the editor selection into one [`SourceClick`] per
//!    identifier, with absolute source positions (the selection can start in the
//!    middle of a line). It reuses the inlay-hint scanner
//!    ([`crate::lsp::inlay::identifiers`]) so hints and block-adds never
//!    disagree about what a name is, and it skips what is obviously not a
//!    signal: SystemVerilog keywords, macros, escaped identifiers, the
//!    fragments a numeric literal leaves behind (1'b1 -> b1, 8'h0a -> h0a),
//!    comments and string literals.
//! 2. [`plan`] resolves every click through the *same* lookup a click takes and
//!    keeps exactly the signals the user can add: unique paths in first-seen
//!    order, the words the design answered ambiguously (they need the picker,
//!    which a click opens) and the words no elaborated design knows.
//!
//! Nothing here touches the DOM, monaco or the store, so the whole rule set is
//! host tested; the editor glue
//! ([`crate::code_reader::monaco_glue`]) only reads the selection and the app
//! only dispatches the batch.

use std::collections::HashSet;

use crate::lsp::inlay::identifiers;
use crate::nav::SourceClick;

/// The most lines one selection may cover.
///
/// A selection is a user gesture over a *block*, not over a whole file; the cap
/// keeps a select-all on a 50k line file from turning one shortcut into tens of
/// thousands of design lookups. The scan says so in the status line when it
/// cuts a selection ([`SelectedClicks::truncated`]).
pub const MAX_SELECTION_LINES: u32 = 500;

/// The most identifiers one selection may contribute.
///
/// The batch that adds them is capped at
/// [`crate::state::bulk::BULK_ADD_CAP`] (2000) rows, and a block of RTL cannot
/// name more than a few thousand distinct signals; the cap only bounds the work
/// a pathological selection (a huge concatenation, a generated file) can cause.
pub const MAX_IDENTIFIERS: usize = 4000;

/// The active editor selection, as the monaco glue reads it.
///
/// Positions are monaco's: 1-based lines and columns, and `end_col` is
/// *exclusive* (the last selected character of the end line is `end_col - 1`).
/// [`EditorSelection::lines`] holds the full text of every line the selection
/// touches, so the first and last line can be clamped to the selected columns
/// without re-reading the model.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EditorSelection {
    /// The design source the selection belongs to (the database key).
    pub file: String,
    /// 1-based start position (monaco getSelection().startLineNumber/startColumn).
    pub start_line: u32,
    pub start_col: u32,
    /// 1-based end position; the column is exclusive.
    pub end_line: u32,
    pub end_col: u32,
    /// Full text of the lines `start_line` ..= `end_line` (capped at
    /// [`MAX_SELECTION_LINES`]).
    pub lines: Vec<String>,
    /// The selection covered more than [`MAX_SELECTION_LINES`] lines and
    /// `lines` only holds the first part of it.
    pub truncated: bool,
}

/// One line of the selection: the number and the half-open column window that is
/// actually selected on it.
struct LineWindow {
    line: u32,
    /// First selected column (1-based, inclusive).
    from: u32,
    /// One past the last selected column.
    to: u32,
}

impl EditorSelection {
    /// Whether the selection covers at least one character.
    pub fn is_empty(&self) -> bool {
        self.lines.is_empty() || (0..self.lines.len()).all(|index| self.window_at(index).is_none())
    }

    /// The selected column window of the line at `index` (0-based), if any.
    ///
    /// The first line starts at `start_col`, the last ends at `end_col`; the
    /// lines in between are selected whole. A window with no characters is
    /// dropped, which is what makes a collapsed selection and a selection that
    /// only reaches the newline of the last line empty.
    fn window_at(&self, index: usize) -> Option<LineWindow> {
        if self.start_line < 1 || self.start_col < 1 || self.end_line < self.start_line {
            return None;
        }
        let text = self.lines.get(index)?;
        let line = self.start_line.checked_add(index as u32)?;
        if line > self.end_line {
            return None;
        }
        // monaco counts columns in characters, like the identifier scanner
        let line_end = text.chars().count() as u32 + 1;
        let from = if line == self.start_line {
            self.start_col
        } else {
            1
        };
        let to = if line == self.end_line {
            self.end_col.min(line_end)
        } else {
            line_end
        };
        if to <= from {
            return None;
        }
        Some(LineWindow { line, from, to })
    }
}

/// Blanks comments and string literals out, keeping every character in place.
///
/// The inlay scanner is a word scanner: it has no idea that `// pipeline stage`
/// is prose. A block selection is full of comments, so the scan masks them (and
/// string literals) to spaces before looking for identifiers. A space keeps the
/// character count, so the columns the scanner reports are still the source
/// columns; a block comment can span lines, so the masker is stateful and must
/// see the selected lines in order.
#[derive(Default)]
struct Masker {
    in_block_comment: bool,
}

impl Masker {
    /// The line with comments and strings replaced by spaces (same columns).
    fn mask(&mut self, line: &str) -> String {
        let mut out = String::with_capacity(line.len());
        let mut chars = line.chars().peekable();
        while let Some(character) = chars.next() {
            if self.in_block_comment {
                if character == '*' && chars.peek() == Some(&'/') {
                    chars.next();
                    out.push(' ');
                    out.push(' ');
                    self.in_block_comment = false;
                } else {
                    out.push(' ');
                }
                continue;
            }
            match character {
                '/' if chars.peek() == Some(&'/') => {
                    // a line comment: everything after it is prose
                    out.push(' ');
                    out.push(' ');
                    for _ in chars.by_ref() {
                        out.push(' ');
                    }
                }
                '/' if chars.peek() == Some(&'*') => {
                    chars.next();
                    out.push(' ');
                    out.push(' ');
                    self.in_block_comment = true;
                }
                '"' => {
                    out.push(' ');
                    let mut escaped = false;
                    for inner in chars.by_ref() {
                        out.push(' ');
                        if escaped {
                            escaped = false;
                            continue;
                        }
                        match inner {
                            '\\' => escaped = true,
                            '"' => break,
                            _ => {}
                        }
                    }
                }
                other => out.push(other),
            }
        }
        out
    }
}

/// A selection, scanned: the clicks one shortcut would resolve, before the
/// design lookup.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SelectedClicks {
    /// One click per identifier, in source order.
    ///
    /// Duplicates are kept on purpose: the same word at two positions can name
    /// two different signals (two instances of one module body).
    pub clicks: Vec<SourceClick>,
    /// Keywords, macros, escaped identifiers and literal fragments that were
    /// skipped on purpose (not an error, just not a signal name).
    pub skipped: usize,
    /// The selection was cut at [`MAX_SELECTION_LINES`] / [`MAX_IDENTIFIERS`].
    pub truncated: bool,
}

/// Every identifier of the selection, as the events a click at its position would
/// report.
///
/// The first line is clamped to `start_col` and the last to `end_col`, so a
/// selection that starts or ends in the middle of a word does not produce a
/// truncated name: the partially selected word is skipped (select the whole
/// word to add it).
pub fn clicks(selection: &EditorSelection) -> SelectedClicks {
    let mut found = SelectedClicks {
        truncated: selection.truncated,
        ..SelectedClicks::default()
    };
    let mut masker = Masker::default();
    for index in 0..selection.lines.len() {
        // the mask has to see every line of the range, in order, even when the
        // line itself contributes no identifier (a block comment can start on it)
        let Some(line) = selection.lines.get(index) else {
            break;
        };
        let code = masker.mask(line);
        let Some(window) = selection.window_at(index) else {
            continue;
        };
        for (start, end, word) in identifiers(&code) {
            // a partially selected identifier at either boundary is not the
            // name the user meant: skip it instead of resolving a prefix
            if start < window.from || end > window.to {
                continue;
            }
            if is_keyword(&word) || is_not_an_identifier(&code, start) {
                found.skipped += 1;
                continue;
            }
            if found.clicks.len() >= MAX_IDENTIFIERS {
                found.truncated = true;
                return found;
            }
            found.clicks.push(SourceClick {
                file: selection.file.clone(),
                line: window.line,
                col: start,
                word,
            });
        }
    }
    found
}

/// Whether the character before the identifier says it is not a plain name.
///
/// The scanner only starts words on a letter or an underscore, so the
/// prefixes that matter are the ones that make the word a *fragment* or a
/// differently spelled name: an apostrophe (a based number: 8'h0a -> h0a,
/// 1'b1 -> b1), a backtick (a compiler directive or macro), a backslash (an
/// escaped identifier, whose real name includes the backslash and the
/// terminating space) and a dollar (a system task or function: $display is not
/// a signal, so its name must not be resolved).
fn is_not_an_identifier(line: &str, start_col: u32) -> bool {
    let Some(index) = start_col.checked_sub(2) else {
        return false;
    };
    matches!(
        line.chars().nth(index as usize),
        Some('\'' | '`' | '\\' | '$')
    )
}

/// SystemVerilog / Verilog-2005 reserved words.
///
/// Sorted, so [`is_keyword`] can binary search it (a test keeps it sorted). The
/// list is the IEEE 1800 reserved-word list including the Verilog-2005 extras;
/// a word that is not here but still is not a signal (a function name, an
/// instance label) simply does not resolve and is counted as unresolved.
#[rustfmt::skip] // one keyword per row keeps this table sorted and readable
const KEYWORDS: &[&str] = &[
    "accept_on", "alias", "always", "always_comb", "always_ff", "always_latch", "and", "assert",
    "assign", "assume", "automatic", "before", "begin", "bind", "bins", "binsof",
    "bit", "break", "buf", "bufif0", "bufif1", "byte", "case", "casex",
    "casez", "cell", "chandle", "checker", "class", "clocking", "cmos", "config",
    "const", "constraint", "context", "continue", "cover", "covergroup", "coverpoint", "cross",
    "deassign", "default", "defparam", "design", "disable", "dist", "do", "edge",
    "else", "end", "endcase", "endchecker", "endclass", "endclocking", "endconfig", "endfunction",
    "endgenerate", "endgroup", "endinterface", "endmodule", "endpackage", "endprimitive", "endprogram", "endproperty",
    "endsequence", "endspecify", "endtable", "endtask", "enum", "event", "eventually", "expect",
    "export", "extends", "extern", "final", "first_match", "for", "force", "foreach",
    "forever", "fork", "forkjoin", "function", "generate", "genvar", "global", "highz0",
    "highz1", "if", "iff", "ifnone", "ignore_bins", "illegal_bins", "implements", "implies",
    "import", "incdir", "include", "initial", "inout", "input", "inside", "instance",
    "int", "integer", "interconnect", "interface", "intersect", "join", "join_any", "join_none",
    "large", "let", "liblist", "library", "local", "localparam", "logic", "longint",
    "macromodule", "matches", "medium", "modport", "module", "nand", "negedge", "nettype",
    "new", "nexttime", "nmos", "nor", "noshowcancelled", "not", "notif0", "notif1",
    "null", "or", "output", "package", "packed", "parameter", "pmos", "posedge",
    "primitive", "priority", "program", "property", "protected", "pull0", "pull1", "pulldown",
    "pullup", "pulsestyle_ondetect", "pulsestyle_onevent", "pure", "rand", "randc", "randcase", "randsequence",
    "rcmos", "real", "realtime", "ref", "reg", "reject_on", "release", "repeat",
    "restrict", "return", "rnmos", "rpmos", "rtran", "rtranif0", "rtranif1", "s_always",
    "s_eventually", "s_nexttime", "s_until", "s_until_with", "scalared", "sequence", "shortint", "shortreal",
    "showcancelled", "signed", "small", "solve", "specify", "specparam", "static", "string",
    "strong", "strong0", "strong1", "struct", "super", "supply0", "supply1", "sync_accept_on",
    "sync_reject_on", "table", "tagged", "task", "this", "throughout", "time", "timeprecision",
    "timeunit", "tran", "tranif0", "tranif1", "tri", "tri0", "tri1", "triand",
    "trior", "trireg", "type", "typedef", "union", "unique", "unique0", "unsigned",
    "use", "uwire", "var", "vectored", "virtual", "void", "wait", "wait_order",
    "wand", "weak", "weak0", "weak1", "while", "wildcard", "wire", "with",
    "within", "wor", "xnor", "xor",
];

/// Is `word` a SystemVerilog reserved word (never a signal name)?
pub fn is_keyword(word: &str) -> bool {
    KEYWORDS
        .binary_search_by(|probe| (*probe).cmp(word))
        .is_ok()
}

/// The signals a selection adds: the unique paths, and the names that need the
/// user's attention instead.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SelectionPlan {
    /// Unique hierarchical paths to add, in first-seen order.
    pub paths: Vec<String>,
    /// Distinct words the design answered with more than one candidate.
    ///
    /// A click on such a word opens the picker, so the status line names a few
    /// of them and says "click one to pick"; adding a guess would be worse than
    /// a message.
    pub ambiguous: Vec<String>,
    /// Distinct words no elaborated design knows (a function name, an instance
    /// label, ...).
    pub unresolved: Vec<String>,
}

impl SelectionPlan {
    /// Whether there is nothing to add at all.
    pub fn is_empty(&self) -> bool {
        self.paths.is_empty()
    }
}

/// Resolve every click through `resolve` (the same lookup a click takes) and
/// keep exactly the signals the block adds.
///
/// A word is counted once even when it occurs several times (a block repeats
/// `clk` in every assignment): the status line reports *names* that need the
/// user, not occurrences.
pub fn plan(
    clicks: &[SourceClick],
    mut resolve: impl FnMut(&SourceClick) -> Vec<String>,
) -> SelectionPlan {
    let mut plan = SelectionPlan::default();
    let mut paths: HashSet<String> = HashSet::new();
    let mut ambiguous: HashSet<String> = HashSet::new();
    let mut unresolved: HashSet<String> = HashSet::new();
    for click in clicks {
        let candidates = resolve(click);
        match candidates.len() {
            0 => {
                if unresolved.insert(click.word.clone()) {
                    plan.unresolved.push(click.word.clone());
                }
            }
            1 => {
                if let Some(path) = candidates.into_iter().next() {
                    if paths.insert(path.clone()) {
                        plan.paths.push(path);
                    }
                }
            }
            _ => {
                if ambiguous.insert(click.word.clone()) {
                    plan.ambiguous.push(click.word.clone());
                }
            }
        }
    }
    plan
}

/// The status detail appended to the batch's own line after a successful add.
///
/// The batch (the reducer) already says how many rows it added and how many were
/// already in the wave; this says what the *selection* contributed besides rows,
/// so an ambiguous or unknown name is never silent.
pub fn success_note(plan: &SelectionPlan) -> String {
    let mut note = String::from("from the code selection");
    if !plan.ambiguous.is_empty() {
        note.push_str(&format!(
            "; {} ambiguous ({} — click one to pick)",
            plan.ambiguous.len(),
            word_list(&plan.ambiguous)
        ));
    }
    if !plan.unresolved.is_empty() {
        note.push_str(&format!(
            "; {} not in the elaborated design ({})",
            plan.unresolved.len(),
            word_list(&plan.unresolved)
        ));
    }
    note
}

/// Why a selection added nothing, phrased as what the user can do.
///
/// Three different "nothing": the selection is empty, it names no identifier at
/// all (prose/comments/keywords), or every name it has is ambiguous or unknown
/// (the plan says which, and a click on one opens the picker).
pub fn nothing_added(
    selection: &EditorSelection,
    found: &SelectedClicks,
    plan: &SelectionPlan,
) -> String {
    let file = if selection.file.is_empty() {
        "the selection"
    } else {
        selection.file.as_str()
    };
    if found.clicks.is_empty() {
        if selection.is_empty() {
            return format!(
                "nothing selected in {file} — select a block of code, then press Ctrl+W"
            );
        }
        return format!(
            "the selection in {file} has no signal names — only keywords, literals and comments were selected; click a name in the source to add it"
        );
    }
    let ambiguous = plan.ambiguous.len();
    let unresolved = plan.unresolved.len();
    let names = format!("{} identifier(s)", found.clicks.len());
    if ambiguous > 0 && unresolved == 0 {
        return format!(
            "the {names} of the {file} selection are ambiguous between instances ({}) — click one in the source to pick its signal",
            word_list(&plan.ambiguous)
        );
    }
    if ambiguous > 0 {
        return format!(
            "nothing in the {file} selection is a single signal: {ambiguous} ambiguous ({}), {unresolved} not in the elaborated design — click a name in the source to pick it",
            word_list(&plan.ambiguous)
        );
    }
    format!(
        "no signal of the elaborated design is named by the {file} selection ({names}) — click one in the source to see its candidates"
    )
}

/// "a, b, c +2 more": a short, readable list of names for a status line.
pub fn word_list(words: &[String]) -> String {
    const SHOWN: usize = 3;
    let mut text = words
        .iter()
        .take(SHOWN)
        .map(|word| format!("`{word}`"))
        .collect::<Vec<_>>()
        .join(", ");
    if words.len() > SHOWN {
        text.push_str(&format!(" +{} more", words.len() - SHOWN));
    }
    text
}

#[cfg(test)]
mod tests {
    use super::{
        clicks, is_keyword, nothing_added, plan, success_note, EditorSelection, SelectedClicks,
        SelectionPlan, KEYWORDS,
    };
    use crate::nav::SourceClick;

    /// The QA fixtures, inlined: the design tests below parse the same sources
    /// the browser QA run opens, so the block a scenario selects is the block
    /// tested here.
    const TOP_SV: &str = r"// Top level: one instance bound by name, one by position,
// and a parameter override on each.
module top (
    input  logic        clk,
    input  logic        reset,
    input  logic [7:0]  instr,
    output logic [15:0] result
);
    logic [7:0]  pc;
    logic [7:0]  data_out;
    logic [15:0] pc16;

    cpu #(.WIDTH(8), .DEPTH(16)) u_cpu (
        .clk(clk),
        .reset(reset),
        .instr(instr),
        .pc(pc),
        .data_out(data_out)
    );

    cpu #(16, 32) u_cpu16 (
        clk,
        reset,
        {8'b0, instr},
        pc16,
        result
    );

    assign result[7:0] = data_out;
endmodule
";

    const CPU_SV: &str = r"// Small CPU-ish leaf module used by the hierarchy / parameter tests.
module cpu #(
    parameter WIDTH = 8,
    parameter DEPTH = 16
) (
    input  logic             clk,
    input  logic             reset,
    input  logic [WIDTH-1:0] instr,
    output logic [WIDTH-1:0] pc,
    output logic [WIDTH-1:0] data_out
);
    logic [WIDTH-1:0] next_pc;
    reg   [WIDTH-1:0] pc_reg;
    wire              active;

    localparam STEP = 1;

    assign active  = reset ? 1'b0 : 1'b1;
    assign next_pc = pc_reg + 1'b1;
    assign pc      = pc_reg;
    assign data_out = instr;

    always @(posedge clk) begin
        if (reset)
            pc_reg <= {WIDTH{1'b0}};
        else
            pc_reg <= next_pc;
    end
endmodule
";

    /// A selection over the given lines, spanning the given monaco range.
    fn selection(
        file: &str,
        start_line: u32,
        start_col: u32,
        end_line: u32,
        end_col: u32,
        lines: &[&str],
    ) -> EditorSelection {
        EditorSelection {
            file: file.to_string(),
            start_line,
            start_col,
            end_line,
            end_col,
            lines: lines.iter().map(|line| (*line).to_string()).collect(),
            truncated: false,
        }
    }

    /// Whole lines starting at line number `from`, as a selection.
    fn whole_lines(file: &str, from: u32, body: &[&str]) -> EditorSelection {
        let end_line = from + body.len() as u32 - 1;
        let end_col = body
            .last()
            .map(|line| line.chars().count() as u32 + 1)
            .unwrap_or(1);
        selection(file, from, 1, end_line, end_col, body)
    }

    fn words(found: &SelectedClicks) -> Vec<String> {
        found
            .clicks
            .iter()
            .map(|click| click.word.clone())
            .collect()
    }

    fn click(line: u32, word: &str) -> SourceClick {
        SourceClick {
            file: "top.sv".to_string(),
            line,
            col: 1,
            word: word.to_string(),
        }
    }

    #[test]
    fn keywords_and_literal_fragments_are_skipped() {
        let body = [
            "    assign active  = reset ? 1'b0 : 1'b1;",
            "    assign next_pc = pc_reg + 1'b1;",
        ];
        let found = clicks(&whole_lines("cpu.sv", 18, &body));
        assert_eq!(words(&found), vec!["active", "reset", "next_pc", "pc_reg"]);
        // assign twice, three based literals
        assert_eq!(found.skipped, 5);
        assert!(!found.truncated);
        // positions are absolute: the click carries the source line and column
        assert_eq!(
            found.clicks[0],
            SourceClick {
                file: "cpu.sv".to_string(),
                line: 18,
                col: 12,
                word: "active".to_string(),
            }
        );
        assert_eq!(found.clicks[1].line, 18);
        assert_eq!(found.clicks[2].line, 19);
    }

    #[test]
    fn a_mid_line_start_only_scans_the_selected_part() {
        let line = "    assign next_pc = pc_reg + 1'b1;";
        let body = [line];
        let start = line.find("next_pc").expect("the line has next_pc") as u32 + 1;
        let end = line.chars().count() as u32 + 1;
        let found = clicks(&selection("cpu.sv", 19, start, 19, end, &body));
        assert_eq!(words(&found), vec!["next_pc", "pc_reg"]);
        assert_eq!(found.clicks[0].col, start);
        // starting inside the word must not resolve the truncated prefix
        let found = clicks(&selection("cpu.sv", 19, start + 2, 19, end, &body));
        assert_eq!(words(&found), vec!["pc_reg"]);
    }

    #[test]
    fn a_selection_that_stops_mid_word_does_not_add_the_partial_name() {
        let line = "    assign pc      = pc_reg;";
        let body = [line];
        let start = line.find("pc_reg").expect("the line has pc_reg") as u32 + 1;
        let found = clicks(&selection("top.sv", 20, 1, 20, start + 2, &body));
        // only the names that are selected whole count
        assert_eq!(words(&found), vec!["pc"]);
    }

    #[test]
    fn comments_and_strings_do_not_contribute_names() {
        let body = [
            "    assign q = d; // pipeline stage: q follows d",
            "    /* the next line is code,",
            "       not prose either */ assign r = q;",
            "    initial $display(\"q is %d\", q);",
        ];
        let found = clicks(&whole_lines("top.sv", 1, &body));
        // q/d in the assignment, r/q after the block comment, q in the call:
        // the comment prose, the string and the system task name are gone
        assert_eq!(words(&found), vec!["q", "d", "r", "q", "q"]);
        // assign twice, initial, and the $display name
        assert_eq!(found.skipped, 4);
    }

    #[test]
    fn x_and_z_literals_are_not_signals_but_a_signal_called_x_is() {
        let body = ["    assign q = 1'bx;", "    assign x = y;"];
        let found = clicks(&whole_lines("top.sv", 1, &body));
        assert_eq!(words(&found), vec!["q", "x", "y"]);
    }

    #[test]
    fn macros_and_escaped_identifiers_are_skipped() {
        let body = ["`define WIDTH 8", "    wire \\escaped_name ;"];
        let found = clicks(&whole_lines("top.sv", 1, &body));
        assert_eq!(words(&found), vec!["WIDTH"]);
        assert_eq!(
            found.skipped, 3,
            "the macro, the keyword and the escaped name"
        );
    }

    #[test]
    fn an_empty_selection_contributes_nothing() {
        // a collapsed cursor is not a selection
        let collapsed = selection("top.sv", 9, 18, 9, 18, &["    logic [7:0]  pc;"]);
        assert!(collapsed.is_empty());
        assert!(clicks(&collapsed).clicks.is_empty());
        // a selection that only reaches the newline of the next line is empty too
        let newline = selection("top.sv", 9, 21, 10, 1, &["    logic [7:0]  pc;", ""]);
        assert!(newline.is_empty());
        assert!(clicks(&newline).clicks.is_empty());
    }

    #[test]
    fn the_plan_keeps_unique_paths_and_reports_what_needs_the_user() {
        let clicks = vec![
            click(9, "clk"),
            click(9, "gate"),
            click(9, "gate"),
            click(10, "clk"),
            click(10, "foo"),
            click(11, "pc"),
        ];
        let plan = plan(&clicks, |click| match click.word.as_str() {
            "clk" => vec!["top.clk".to_string()],
            "pc" => vec!["top.pc".to_string()],
            "gate" => vec!["top.u1.gate".to_string(), "top.u2.gate".to_string()],
            _ => Vec::new(),
        });
        // unique paths in first-seen order, duplicates gone
        assert_eq!(plan.paths, vec!["top.clk", "top.pc"]);
        // a name is reported once, however often the block repeats it
        assert_eq!(plan.ambiguous, vec!["gate"]);
        assert_eq!(plan.unresolved, vec!["foo"]);
        assert!(!plan.is_empty());
    }

    #[test]
    fn the_status_texts_say_what_to_do() {
        // an empty selection: the shortcut has nothing to add
        let empty = selection("top.sv", 9, 21, 10, 1, &["", ""]);
        assert!(nothing_added(
            &empty,
            &SelectedClicks::default(),
            &SelectionPlan::default()
        )
        .contains("nothing selected in top.sv"));
        // a selection of prose only
        let prose = whole_lines("top.sv", 1, &["// just a comment"]);
        let found = clicks(&prose);
        let message = nothing_added(&prose, &found, &SelectionPlan::default());
        assert!(message.contains("has no signal names"), "{message}");
        // identifiers, but none of them a signal of the design
        let mut found = SelectedClicks::default();
        found.clicks.push(click(1, "foo"));
        let plan = SelectionPlan {
            unresolved: vec!["foo".to_string()],
            ..SelectionPlan::default()
        };
        let message = nothing_added(&prose, &found, &plan);
        assert!(
            message.contains("no signal of the elaborated design"),
            "{message}"
        );
        // every name is ambiguous: say that instead of "no such signal"
        let mut found = SelectedClicks::default();
        found.clicks.push(click(1, "gate"));
        let plan = SelectionPlan {
            ambiguous: vec!["gate".to_string()],
            ..SelectionPlan::default()
        };
        let message = nothing_added(&prose, &found, &plan);
        assert!(message.contains("ambiguous between instances"), "{message}");
        assert!(message.contains("`gate`"), "{message}");
    }

    #[test]
    fn the_success_note_names_the_ambiguous_words() {
        let clean = SelectionPlan {
            paths: vec!["top.clk".to_string()],
            ..SelectionPlan::default()
        };
        assert_eq!(success_note(&clean), "from the code selection");
        let messy = SelectionPlan {
            paths: vec!["top.clk".to_string()],
            ambiguous: vec!["gate".to_string()],
            unresolved: vec!["foo".to_string(), "bar".to_string()],
        };
        let note = success_note(&messy);
        assert!(
            note.contains("1 ambiguous (`gate` — click one to pick)"),
            "{note}"
        );
        assert!(
            note.contains("2 not in the elaborated design (`foo`, `bar`)"),
            "{note}"
        );
    }

    #[test]
    fn the_keyword_list_is_sorted_and_complete_enough() {
        // is_keyword binary searches the list, so it has to stay sorted
        let mut sorted = KEYWORDS.to_vec();
        sorted.sort_unstable();
        assert_eq!(sorted, KEYWORDS);
        for keyword in [
            "assign",
            "always_ff",
            "logic",
            "posedge",
            "wire",
            "endmodule",
        ] {
            assert!(is_keyword(keyword), "{keyword} must be a keyword");
        }
        for name in ["clk", "reset", "pc_reg", "active", "q", "data_out", "widen"] {
            assert!(!is_keyword(name), "{name} is a signal name, not a keyword");
        }
    }

    /// Resolve a click the way the app does: the exact design position first,
    /// the local-name fallback second.
    fn resolve_in(design: &crate::state::design::DesignState, click: &SourceClick) -> Vec<String> {
        let mut candidates = design.signals_at(&click.file, click.line, click.col);
        if candidates.is_empty() {
            candidates = design.signals_matching(&click.word);
        }
        candidates
    }

    #[test]
    fn a_real_design_resolves_a_top_level_block_to_three_signals() {
        // The whole chain end to end without a browser: the declaration block of
        // top.sv resolves each name the way a click would.
        let mut design = crate::state::design::DesignState::new();
        design.add_source("top.sv", TOP_SV);
        design.add_source("cpu.sv", CPU_SV);
        let body = [
            "    logic [7:0]  pc;",
            "    logic [7:0]  data_out;",
            "    logic [15:0] pc16;",
        ];
        let found = clicks(&whole_lines("top.sv", 9, &body));
        let plan = plan(&found.clicks, |click| resolve_in(&design, click));
        assert_eq!(plan.paths, vec!["top.pc", "top.data_out", "top.pc16"]);
        assert!(plan.ambiguous.is_empty(), "{:?}", plan.ambiguous);
        assert!(plan.unresolved.is_empty(), "{:?}", plan.unresolved);
    }

    #[test]
    fn a_module_body_selection_is_ambiguous_between_instances() {
        // cpu.sv is instantiated twice (u_cpu, u_cpu16): a block inside it names
        // the same signal of both instances, so the user has to pick one.
        let mut design = crate::state::design::DesignState::new();
        design.add_source("top.sv", TOP_SV);
        design.add_source("cpu.sv", CPU_SV);
        let body = [
            "    assign next_pc = pc_reg + 1'b1;",
            "    assign pc      = pc_reg;",
        ];
        let selection = whole_lines("cpu.sv", 19, &body);
        let found = clicks(&selection);
        let plan = plan(&found.clicks, |click| resolve_in(&design, click));
        assert!(plan.is_empty(), "{:?}", plan.paths);
        assert!(
            plan.ambiguous.contains(&"next_pc".to_string()),
            "{:?}",
            plan.ambiguous
        );
        assert!(
            plan.ambiguous.contains(&"pc_reg".to_string()),
            "{:?}",
            plan.ambiguous
        );
        // ... and the message tells the user what to do instead
        let message = nothing_added(&selection, &found, &plan);
        assert!(message.contains("ambiguous between instances"), "{message}");
    }
}
