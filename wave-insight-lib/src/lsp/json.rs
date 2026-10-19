//! Minimal JSON reader and writer.
//!
//! The language server core has to run unchanged on the desktop (behind a
//! stdio transport) and inside the browser (wasm), and `wave-insight-lib`
//! deliberately has no `serde_json` dependency.  LSP messages only need a
//! small, fully specified part of JSON, so this module implements exactly that:
//!
//! * a [`Json`] value type,
//! * a strict [`Json::parse`] with a nesting limit (a hostile or broken client
//!   cannot blow the stack),
//! * a compact writer that always emits valid JSON,
//! * accessors/constructors that keep the request handlers readable.
//!
//! The parser rejects what RFC 8259 rejects — trailing commas, unquoted keys,
//! lone surrogates, control characters inside strings — so a malformed client
//! message turns into a JSON-RPC `-32700` parse error instead of a panic or a
//! silently dropped request.

use std::fmt;
use std::fmt::Write as _;

/// Deepest object/array nesting [`Json::parse`] accepts.
pub const MAX_DEPTH: usize = 128;

/// One JSON value.
#[derive(Clone, Debug, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    Integer(i64),
    /// Only produced by literals with a fraction or an exponent, or by
    /// integers that do not fit `i64`.  LSP numbers are integers, so the
    /// integer representation is used whenever it is possible.
    Float(f64),
    String(String),
    Array(Vec<Json>),
    /// Insertion order is preserved: JSON objects are unordered, but stable
    /// output keeps responses easy to read and to test.
    Object(Vec<(String, Json)>),
}

impl Json {
    // -- construction ------------------------------------------------------

    /// A JSON string.
    pub fn string(value: impl Into<String>) -> Json {
        Json::String(value.into())
    }

    /// A JSON integer.
    pub fn int(value: i64) -> Json {
        Json::Integer(value)
    }

    /// A JSON object; the entries keep their insertion order.
    pub fn object<K: Into<String>>(entries: impl IntoIterator<Item = (K, Json)>) -> Json {
        Json::Object(entries.into_iter().map(|(key, value)| (key.into(), value)).collect())
    }

    /// A JSON array.
    pub fn array(items: impl IntoIterator<Item = Json>) -> Json {
        Json::Array(items.into_iter().collect())
    }

    // -- accessors ---------------------------------------------------------

    pub fn is_null(&self) -> bool {
        matches!(self, Json::Null)
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Json::String(value) => Some(value),
            _ => None,
        }
    }

    /// `Some` for integer values and for floats without a fractional part
    /// (some clients spell `1` as `1.0`).
    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Json::Integer(value) => Some(*value),
            Json::Float(value)
                if value.fract() == 0.0
                    && *value >= i64::MIN as f64
                    && *value <= i64::MAX as f64 =>
            {
                Some(*value as i64)
            }
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Json::Bool(value) => Some(*value),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&[Json]> {
        match self {
            Json::Array(items) => Some(items),
            _ => None,
        }
    }

    pub fn as_object(&self) -> Option<&[(String, Json)]> {
        match self {
            Json::Object(entries) => Some(entries),
            _ => None,
        }
    }

    /// The member `key` of an object, `None` for anything else (including a
    /// missing member).
    pub fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Object(entries) => entries
                .iter()
                .find(|(name, _)| name == key)
                .map(|(_, value)| value),
            _ => None,
        }
    }

    pub fn get_str(&self, key: &str) -> Option<&str> {
        self.get(key).and_then(Json::as_str)
    }

    pub fn get_i64(&self, key: &str) -> Option<i64> {
        self.get(key).and_then(Json::as_i64)
    }

    pub fn get_bool(&self, key: &str) -> Option<bool> {
        self.get(key).and_then(Json::as_bool)
    }

    pub fn get_array(&self, key: &str) -> Option<&[Json]> {
        self.get(key).and_then(Json::as_array)
    }

    // -- output ------------------------------------------------------------

    /// Compact JSON text.  Compact (not pretty) because the stdio transport
    /// puts a `Content-Length` header in front of it, and the wasm side has to
    /// parse it again anyway.
    pub fn to_text(&self) -> String {
        let mut out = String::new();
        self.write_into(&mut out);
        out
    }

    fn write_into(&self, out: &mut String) {
        match self {
            Json::Null => out.push_str("null"),
            Json::Bool(true) => out.push_str("true"),
            Json::Bool(false) => out.push_str("false"),
            Json::Integer(value) => {
                let _ = write!(out, "{value}");
            }
            Json::Float(value) => {
                if value.is_finite() {
                    let _ = write!(out, "{value}");
                } else {
                    // JSON has no NaN/infinity spelling.
                    out.push_str("null");
                }
            }
            Json::String(value) => write_string(value, out),
            Json::Array(items) => {
                out.push('[');
                for (index, item) in items.iter().enumerate() {
                    if index > 0 {
                        out.push(',');
                    }
                    item.write_into(out);
                }
                out.push(']');
            }
            Json::Object(entries) => {
                out.push('{');
                for (index, (key, value)) in entries.iter().enumerate() {
                    if index > 0 {
                        out.push(',');
                    }
                    write_string(key, out);
                    out.push(':');
                    value.write_into(out);
                }
                out.push('}');
            }
        }
    }

    // -- input -------------------------------------------------------------

    /// Parse one JSON document.  Trailing non-whitespace is an error, so a
    /// stream of concatenated messages cannot be misread as one.
    pub fn parse(text: &str) -> Result<Json, JsonError> {
        let mut parser = Parser {
            bytes: text.as_bytes(),
            position: 0,
        };
        parser.skip_whitespace();
        let value = parser.value(0)?;
        parser.skip_whitespace();
        if parser.position != parser.bytes.len() {
            return Err(parser.error("trailing characters after the JSON value"));
        }
        Ok(value)
    }
}

impl From<&str> for Json {
    fn from(value: &str) -> Json {
        Json::String(value.to_owned())
    }
}

impl From<String> for Json {
    fn from(value: String) -> Json {
        Json::String(value)
    }
}

impl From<bool> for Json {
    fn from(value: bool) -> Json {
        Json::Bool(value)
    }
}

impl From<i64> for Json {
    fn from(value: i64) -> Json {
        Json::Integer(value)
    }
}

impl From<u32> for Json {
    fn from(value: u32) -> Json {
        Json::Integer(i64::from(value))
    }
}

fn write_string(value: &str, out: &mut String) {
    out.push('"');
    for character in value.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            character if (character as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", character as u32);
            }
            character => out.push(character),
        }
    }
    out.push('"');
}

/// A malformed JSON input, with the byte offset that made it fail.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JsonError {
    message: String,
    offset: usize,
}

impl JsonError {
    pub fn message(&self) -> &str {
        &self.message
    }

    pub fn offset(&self) -> usize {
        self.offset
    }
}

impl fmt::Display for JsonError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{} (at byte {})", self.message, self.offset)
    }
}

impl std::error::Error for JsonError {}

struct Parser<'a> {
    bytes: &'a [u8],
    position: usize,
}

impl Parser<'_> {
    fn error(&self, message: impl Into<String>) -> JsonError {
        JsonError {
            message: message.into(),
            offset: self.position,
        }
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.position).copied()
    }

    fn next(&mut self) -> Option<u8> {
        let byte = self.peek()?;
        self.position += 1;
        Some(byte)
    }

    fn skip_whitespace(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.position += 1;
        }
    }

    fn expect_byte(&mut self, byte: u8, context: &str) -> Result<(), JsonError> {
        match self.next() {
            Some(found) if found == byte => Ok(()),
            Some(found) => Err(self.error(format!(
                "expected `{}` in {context}, found `{}`",
                char::from(byte),
                char::from(found)
            ))),
            None => Err(self.error(format!(
                "expected `{}` in {context}, found the end of the input",
                char::from(byte)
            ))),
        }
    }

    fn value(&mut self, depth: usize) -> Result<Json, JsonError> {
        if depth > MAX_DEPTH {
            return Err(self.error(format!("nesting deeper than {MAX_DEPTH} levels")));
        }
        match self.peek() {
            Some(b'{') => self.object(depth),
            Some(b'[') => self.array(depth),
            Some(b'"') => self.string().map(Json::String),
            Some(b't') => self.literal("true", Json::Bool(true)),
            Some(b'f') => self.literal("false", Json::Bool(false)),
            Some(b'n') => self.literal("null", Json::Null),
            Some(b'-' | b'0'..=b'9') => self.number(),
            Some(byte) => Err(self.error(format!(
                "expected a JSON value, found `{}`",
                char::from(byte)
            ))),
            None => Err(self.error("expected a JSON value, found the end of the input")),
        }
    }

    fn literal(&mut self, text: &str, value: Json) -> Result<Json, JsonError> {
        if self.bytes[self.position..].starts_with(text.as_bytes()) {
            self.position += text.len();
            Ok(value)
        } else {
            Err(self.error(format!("expected `{text}`")))
        }
    }

    fn object(&mut self, depth: usize) -> Result<Json, JsonError> {
        self.expect_byte(b'{', "an object")?;
        let mut entries = Vec::new();
        self.skip_whitespace();
        if self.peek() == Some(b'}') {
            self.position += 1;
            return Ok(Json::Object(entries));
        }
        loop {
            self.skip_whitespace();
            let key = self.string()?;
            self.skip_whitespace();
            self.expect_byte(b':', "an object member")?;
            self.skip_whitespace();
            let value = self.value(depth + 1)?;
            entries.push((key, value));
            self.skip_whitespace();
            match self.next() {
                Some(b',') => continue,
                Some(b'}') => return Ok(Json::Object(entries)),
                Some(found) => {
                    return Err(self.error(format!(
                        "expected `,` or `}}` in an object, found `{}`",
                        char::from(found)
                    )))
                }
                None => return Err(self.error("unterminated object")),
            }
        }
    }

    fn array(&mut self, depth: usize) -> Result<Json, JsonError> {
        self.expect_byte(b'[', "an array")?;
        let mut items = Vec::new();
        self.skip_whitespace();
        if self.peek() == Some(b']') {
            self.position += 1;
            return Ok(Json::Array(items));
        }
        loop {
            self.skip_whitespace();
            items.push(self.value(depth + 1)?);
            self.skip_whitespace();
            match self.next() {
                Some(b',') => continue,
                Some(b']') => return Ok(Json::Array(items)),
                Some(found) => {
                    return Err(self.error(format!(
                        "expected `,` or `]` in an array, found `{}`",
                        char::from(found)
                    )))
                }
                None => return Err(self.error("unterminated array")),
            }
        }
    }

    fn string(&mut self) -> Result<String, JsonError> {
        self.expect_byte(b'"', "a string")?;
        let mut out = String::new();
        loop {
            let Some(byte) = self.peek() else {
                return Err(self.error("unterminated string"));
            };
            match byte {
                b'"' => {
                    self.position += 1;
                    return Ok(out);
                }
                b'\\' => {
                    self.position += 1;
                    self.escape(&mut out)?;
                }
                0x00..=0x1f => {
                    return Err(self.error("unescaped control character in a string"));
                }
                _ => {
                    let rest = &self.bytes[self.position..];
                    let length = utf8_length(rest[0]);
                    if length == 0 || rest.len() < length {
                        return Err(self.error("invalid UTF-8 in a string"));
                    }
                    let Ok(text) = std::str::from_utf8(&rest[..length]) else {
                        return Err(self.error("invalid UTF-8 in a string"));
                    };
                    out.push_str(text);
                    self.position += length;
                }
            }
        }
    }

    fn escape(&mut self, out: &mut String) -> Result<(), JsonError> {
        let Some(byte) = self.next() else {
            return Err(self.error("unterminated escape sequence"));
        };
        match byte {
            b'"' => out.push('"'),
            b'\\' => out.push('\\'),
            b'/' => out.push('/'),
            b'b' => out.push('\u{8}'),
            b'f' => out.push('\u{c}'),
            b'n' => out.push('\n'),
            b'r' => out.push('\r'),
            b't' => out.push('\t'),
            b'u' => {
                let first = self.hex4()?;
                let code = if (0xd800..=0xdbff).contains(&first) {
                    // A high surrogate must be followed by a low one.
                    if self.peek() != Some(b'\\') {
                        return Err(self.error("lone high surrogate in a string"));
                    }
                    self.position += 1;
                    if self.next() != Some(b'u') {
                        return Err(self.error("lone high surrogate in a string"));
                    }
                    let second = self.hex4()?;
                    if !(0xdc00..=0xdfff).contains(&second) {
                        return Err(self.error("invalid low surrogate in a string"));
                    }
                    0x1_0000 + ((first - 0xd800) << 10) + (second - 0xdc00)
                } else if (0xdc00..=0xdfff).contains(&first) {
                    return Err(self.error("lone low surrogate in a string"));
                } else {
                    first
                };
                match char::from_u32(code) {
                    Some(character) => out.push(character),
                    None => return Err(self.error("invalid unicode escape")),
                }
            }
            other => {
                return Err(self.error(format!(
                    "unknown escape sequence `\\{}`",
                    char::from(other)
                )))
            }
        }
        Ok(())
    }

    fn hex4(&mut self) -> Result<u32, JsonError> {
        let mut value = 0u32;
        for _ in 0..4 {
            let Some(byte) = self.next() else {
                return Err(self.error("truncated unicode escape"));
            };
            let digit = match byte {
                b'0'..=b'9' => u32::from(byte - b'0'),
                b'a'..=b'f' => u32::from(byte - b'a') + 10,
                b'A'..=b'F' => u32::from(byte - b'A') + 10,
                _ => return Err(self.error("invalid unicode escape")),
            };
            value = value * 16 + digit;
        }
        Ok(value)
    }

    fn number(&mut self) -> Result<Json, JsonError> {
        let start = self.position;
        if self.peek() == Some(b'-') {
            self.position += 1;
        }
        match self.peek() {
            Some(b'0') => self.position += 1,
            Some(byte) if byte.is_ascii_digit() => {
                while matches!(self.peek(), Some(byte) if byte.is_ascii_digit()) {
                    self.position += 1;
                }
            }
            _ => return Err(self.error("expected a digit")),
        }

        let mut is_float = false;
        if self.peek() == Some(b'.') {
            is_float = true;
            self.position += 1;
            if !matches!(self.peek(), Some(byte) if byte.is_ascii_digit()) {
                return Err(self.error("expected a digit after the decimal point"));
            }
            while matches!(self.peek(), Some(byte) if byte.is_ascii_digit()) {
                self.position += 1;
            }
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            is_float = true;
            self.position += 1;
            if matches!(self.peek(), Some(b'+' | b'-')) {
                self.position += 1;
            }
            if !matches!(self.peek(), Some(byte) if byte.is_ascii_digit()) {
                return Err(self.error("expected a digit in the exponent"));
            }
            while matches!(self.peek(), Some(byte) if byte.is_ascii_digit()) {
                self.position += 1;
            }
        }

        let text = std::str::from_utf8(&self.bytes[start..self.position])
            .map_err(|_| self.error("invalid number"))?;
        if !is_float {
            if let Ok(integer) = text.parse::<i64>() {
                return Ok(Json::Integer(integer));
            }
        }
        text.parse::<f64>()
            .map(Json::Float)
            .map_err(|_| self.error("invalid number"))
    }
}

/// Length of the UTF-8 sequence that starts with `lead`, `0` when the byte
/// cannot start one.
fn utf8_length(lead: u8) -> usize {
    match lead {
        0x00..=0x7f => 1,
        0xc2..=0xdf => 2,
        0xe0..=0xef => 3,
        0xf0..=0xf4 => 4,
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_lsp_shaped_messages() {
        let message = Json::parse(
            r#"{"jsonrpc":"2.0","id":7,"method":"textDocument/hover","params":{"position":{"line":12,"character":4},"textDocument":{"uri":"file:///C:/a.sv"}}}"#,
        )
        .expect("valid message");
        assert_eq!(message.get_str("method"), Some("textDocument/hover"));
        assert_eq!(message.get_i64("id"), Some(7));
        let position = message.get("params").and_then(|params| params.get("position"));
        assert_eq!(position.and_then(|position| position.get_i64("line")), Some(12));
        assert_eq!(
            message
                .get("params")
                .and_then(|params| params.get("textDocument"))
                .and_then(|document| document.get_str("uri")),
            Some("file:///C:/a.sv")
        );
    }

    #[test]
    fn parses_every_value_kind_and_escapes() {
        let value = Json::parse(
            r#"{"null":null,"t":true,"f":false,"i":-12,"big":9007199254740993,"d":1.5,"e":2e3,"s":"a\"b\\c\/d\n\t\u00e9\ud83d\ude00","a":[1,[2],{}]}"#,
        )
        .expect("valid document");
        assert_eq!(value.get("null"), Some(&Json::Null));
        assert_eq!(value.get("t"), Some(&Json::Bool(true)));
        assert_eq!(value.get("f"), Some(&Json::Bool(false)));
        assert_eq!(value.get_i64("i"), Some(-12));
        assert_eq!(value.get_i64("big"), Some(9_007_199_254_740_993));
        assert_eq!(value.get("d"), Some(&Json::Float(1.5)));
        assert_eq!(value.get("e"), Some(&Json::Float(2000.0)));
        assert_eq!(
            value.get_str("s"),
            Some("a\"b\\c/d\n\t\u{e9}\u{1f600}")
        );
        assert_eq!(value.get_array("a").map(<[Json]>::len), Some(3));
        assert_eq!(
            value.get_array("a").and_then(|items| items[1].as_array()),
            Some(&[Json::Integer(2)][..])
        );
    }

    #[test]
    fn rejects_malformed_documents() {
        for broken in [
            "",
            "   ",
            "{",
            "[1,]",
            "{\"a\":1,}",
            "{a:1}",
            "{'a':1}",
            "{\"a\" 1}",
            "\"unterminated",
            "\"bad\\x\"",
            "\"\\ud800\"",
            "\"\\ud800\\u0041\"",
            "01",
            "-",
            "1.",
            "1e",
            "tru",
            "null null",
            "{} []",
        ] {
            assert!(
                Json::parse(broken).is_err(),
                "`{broken}` must not parse, got {:?}",
                Json::parse(broken)
            );
        }
    }

    #[test]
    fn rejects_deep_nesting_instead_of_overflowing_the_stack() {
        let deep = format!("{}1{}", "[".repeat(MAX_DEPTH + 2), "]".repeat(MAX_DEPTH + 2));
        let error = Json::parse(&deep).expect_err("must be rejected");
        assert!(error.message().contains("nesting"), "{error}");
        // One level less is fine.
        let ok = format!("{}1{}", "[".repeat(MAX_DEPTH), "]".repeat(MAX_DEPTH));
        assert!(Json::parse(&ok).is_ok());
    }

    #[test]
    fn writer_output_reparses_to_the_same_value_and_stays_valid_json() {
        let value = Json::object([
            ("jsonrpc", Json::string("2.0")),
            ("id", Json::Integer(3)),
            (
                "result",
                Json::array([
                    Json::string("line\nbreak\tand \"quotes\" \\ \u{7}"),
                    Json::Bool(true),
                    Json::Null,
                    Json::object([("nested", Json::Float(0.5))]),
                ]),
            ),
        ]);
        let text = value.to_text();
        assert!(text.starts_with(r#"{"jsonrpc":"2.0","id":3,"result":["line\nbreak"#));
        assert_eq!(Json::parse(&text).expect("own output parses"), value);
        // Control characters must be escaped.
        assert!(!text.contains('\u{7}'));
        assert!(text.contains("\\u0007"));
    }

    #[test]
    fn accessors_are_lenient_about_missing_members() {
        let value = Json::parse(r#"{"a":{"b":[1,2]}}"#).expect("valid");
        assert!(value.get_str("missing").is_none());
        assert!(value.get("a").and_then(|a| a.get_str("b")).is_none());
        assert_eq!(value.get("a").and_then(|a| a.get_i64("b")), None);
        assert_eq!(Json::Integer(4).get("a"), None);
        assert_eq!(Json::Float(3.0).as_i64(), Some(3));
        assert_eq!(Json::Float(3.5).as_i64(), None);
    }
}
