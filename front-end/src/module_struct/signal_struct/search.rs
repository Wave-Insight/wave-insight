//! The signal list's search: case, whole word and regex toggles over one query.
//!
//! The plain modes are pure string logic, so they are unit tested on the host
//! target like the rest of the renderer's non-DOM code. The regex mode is
//! compiled with the browser's own engine (js_sys::RegExp, no new Rust
//! dependency); only the pattern and the flags are built here, which keeps the
//! "what does the toggle mean" question testable.

use wasm_bindgen::{JsCast, JsValue};

/// What the search bar's three toggles say.
///
/// `Default` is the behaviour the signal list had before the toggles existed:
/// a case-insensitive substring match, so nothing changes for a user who never
/// touches them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MatchOptions {
    /// `Aa`: match case exactly.
    pub case_sensitive: bool,
    /// `ab`: only whole words (Verilog identifiers: `[A-Za-z0-9_$]`).
    pub whole_word: bool,
    /// `.*`: the query is a regular expression.
    pub regex: bool,
}

impl MatchOptions {
    /// The `RegExp` flags for the regex mode.
    ///
    /// No `g`: `RegExp::test` is stateful with a global pattern (it keeps
    /// `lastIndex`), so every second row would be skipped.
    pub fn regex_flags(&self) -> &'static str {
        if self.case_sensitive {
            ""
        } else {
            "i"
        }
    }
}

/// The pattern handed to `RegExp::new` for a query.
///
/// The whole-word toggle wraps the query in `\b(?:...)\b`; the non-capturing
/// group keeps a query with alternation (`a|b`) one word rather than two
/// half-anchored alternatives.
pub fn regex_pattern(query: &str, options: MatchOptions) -> String {
    if options.whole_word {
        format!("\\b(?:{query})\\b")
    } else {
        query.to_string()
    }
}

/// Compile `query` with the browser's `RegExp`, or `None` when the pattern is
/// invalid.
///
/// `RegExp::new` is a JS *constructor*: an invalid pattern throws, and
/// wasm-bindgen turns a thrown constructor into a panic. Constructing through
/// `Reflect::construct` keeps the error a value, so the search bar can say
/// "invalid regular expression" while the user is still typing.
pub fn compile_regex(query: &str, options: MatchOptions) -> Option<js_sys::RegExp> {
    let constructor = js_sys::Reflect::get(&js_sys::global(), &JsValue::from_str("RegExp")).ok()?;
    let constructor = constructor.dyn_into::<js_sys::Function>().ok()?;
    let args = js_sys::Array::new();
    args.push(&JsValue::from_str(&regex_pattern(query, options)));
    args.push(&JsValue::from_str(options.regex_flags()));
    js_sys::Reflect::construct(&constructor, &args)
        .ok()?
        .dyn_into::<js_sys::RegExp>()
        .ok()
}

/// `true` when `name` matches `query` in the plain (non-regex) modes.
///
/// An empty query matches everything, which is what makes the clear button and
/// the initial state "show the whole scope".
pub fn matches_plain(name: &str, query: &str, options: MatchOptions) -> bool {
    if query.is_empty() {
        return true;
    }
    let haystack = fold(name, options);
    let needle = fold(query, options);
    if !options.whole_word {
        return haystack.contains(&needle);
    }
    // whole word: some occurrence must sit between two non-word characters
    let mut from = 0;
    while let Some(offset) = haystack[from..].find(&needle) {
        let start = from + offset;
        let end = start + needle.len();
        let before_ok = haystack[..start]
            .chars()
            .next_back()
            .is_none_or(|character| !is_word_char(character));
        let after_ok = haystack[end..]
            .chars()
            .next()
            .is_none_or(|character| !is_word_char(character));
        if before_ok && after_ok {
            return true;
        }
        // step past the first character of this occurrence, on a char boundary
        let step = needle.chars().next().map(char::len_utf8).unwrap_or(1);
        from = start + step;
    }
    false
}

/// The characters a whole-word match must not split.
///
/// Verilog identifiers may contain `$` (and `_`, digits, letters); everything
/// else (`.`, `[`, `:`) is a boundary, so `data` matches `data[7:0]` but not
/// `data_valid`.
pub fn is_word_char(character: char) -> bool {
    character.is_alphanumeric() || character == '_' || character == '$'
}

fn fold(text: &str, options: MatchOptions) -> String {
    if options.case_sensitive {
        text.to_string()
    } else {
        text.to_lowercase()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PLAIN: MatchOptions = MatchOptions {
        case_sensitive: false,
        whole_word: false,
        regex: false,
    };

    #[test]
    fn the_default_is_a_case_insensitive_substring_match() {
        assert_eq!(MatchOptions::default(), PLAIN);
        assert!(matches_plain("data_valid", "DATA", PLAIN));
        assert!(matches_plain("data_valid", "a_v", PLAIN));
        assert!(!matches_plain("data_valid", "xyz", PLAIN));
        // an empty query shows everything
        assert!(matches_plain("anything", "", PLAIN));
    }

    #[test]
    fn case_sensitive_only_matches_the_same_case() {
        let options = MatchOptions {
            case_sensitive: true,
            ..PLAIN
        };
        assert!(matches_plain("data_valid", "data", options));
        assert!(!matches_plain("data_valid", "DATA", options));
        // and the case toggle must not change the substring semantics
        assert!(matches_plain("data_valid", "a_v", options));
    }

    #[test]
    fn whole_word_does_not_split_an_identifier() {
        let options = MatchOptions {
            whole_word: true,
            ..PLAIN
        };
        // a whole identifier matches ...
        assert!(matches_plain("data", "data", options));
        assert!(matches_plain("data_valid", "data_valid", options));
        // ... a fragment of one does not
        assert!(!matches_plain("data_valid", "data", options));
        assert!(!matches_plain("data_valid", "valid", options));
        assert!(!matches_plain("u_data", "data", options));
        // punctuation and brackets are word boundaries
        assert!(matches_plain("data[7:0]", "data", options));
        assert!(matches_plain("top.clk", "clk", options));
        assert!(matches_plain("top.u_cpu.pc", "u_cpu", options));
        // a later occurrence still counts when the first one is embedded
        assert!(matches_plain("data_valid data", "data", options));
        // `$` is an identifier character in Verilog
        assert!(!matches_plain("data$valid", "data", options));
    }

    #[test]
    fn whole_word_is_case_insensitive_unless_case_sensitive_is_set() {
        let options = MatchOptions {
            whole_word: true,
            ..PLAIN
        };
        assert!(matches_plain("DATA", "data", options));
        let options = MatchOptions {
            whole_word: true,
            case_sensitive: true,
            ..PLAIN
        };
        assert!(!matches_plain("DATA", "data", options));
        assert!(matches_plain("DATA", "DATA", options));
    }

    #[test]
    fn the_regex_pattern_wraps_whole_words_and_keeps_groups() {
        let plain = MatchOptions {
            regex: true,
            ..PLAIN
        };
        assert_eq!(regex_pattern("^d.*a$", plain), "^d.*a$");
        let word = MatchOptions {
            regex: true,
            whole_word: true,
            ..PLAIN
        };
        // the non-capturing group keeps `a|b` a single word, not two
        assert_eq!(regex_pattern("a|b", word), "\\b(?:a|b)\\b");
    }

    #[test]
    fn the_regex_flags_never_make_test_stateful() {
        let insensitive = MatchOptions {
            regex: true,
            ..PLAIN
        };
        assert_eq!(insensitive.regex_flags(), "i");
        let sensitive = MatchOptions {
            regex: true,
            case_sensitive: true,
            ..PLAIN
        };
        assert_eq!(sensitive.regex_flags(), "");
        // a global flag would make `test` skip rows; the contract is "no g"
        assert!(!insensitive.regex_flags().contains('g'));
        assert!(!sensitive.regex_flags().contains('g'));
    }
}
