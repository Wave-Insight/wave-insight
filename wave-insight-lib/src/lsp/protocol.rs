//! The JSON-RPC and LSP shapes the server core speaks, and nothing else.
//!
//! Keeping the envelope and the four LSP value types here (instead of pulling
//! in `lsp-server`/`lsp-types`) is what lets the same core run behind the
//! desktop stdio transport and inside the browser: both sides exchange JSON-RPC
//! text, and only the desktop binary knows about `lsp-server` framing.
//!
//! Everything is intentionally small — the server never sends requests, so
//! there is no request builder, and the `initialize` result is assembled by the
//! server from these primitives.

use super::json::Json;

// JSON-RPC and LSP error codes the server uses.
pub const PARSE_ERROR: i32 = -32700;
pub const INVALID_REQUEST: i32 = -32600;
pub const METHOD_NOT_FOUND: i32 = -32601;
pub const INVALID_PARAMS: i32 = -32602;
pub const SERVER_NOT_INITIALIZED: i32 = -32002;

/// One decoded incoming JSON-RPC message.
#[derive(Clone, Debug, PartialEq)]
pub enum Incoming {
    Request {
        id: Json,
        method: String,
        params: Json,
    },
    Notification {
        method: String,
        params: Json,
    },
    /// A response to a request this server never sends; accepted and ignored so
    /// a client that answers unsolicited `window/showMessageRequest` cannot
    /// break the stream.
    Response {
        id: Json,
    },
}

/// A JSON-RPC error object plus the code the server sends it with.
#[derive(Clone, Debug, PartialEq)]
pub struct RpcError {
    pub code: i32,
    pub message: String,
    pub data: Option<Json>,
}

impl RpcError {
    pub fn new(code: i32, message: impl Into<String>) -> RpcError {
        RpcError {
            code,
            message: message.into(),
            data: None,
        }
    }

    pub fn invalid_request(message: impl Into<String>) -> RpcError {
        RpcError::new(INVALID_REQUEST, message)
    }

    pub fn invalid_params(message: impl Into<String>) -> RpcError {
        RpcError::new(INVALID_PARAMS, message)
    }

    pub fn method_not_found(method: &str) -> RpcError {
        RpcError::new(METHOD_NOT_FOUND, format!("method `{method}` is not supported"))
    }

    pub fn server_not_initialized(method: &str) -> RpcError {
        RpcError::new(
            SERVER_NOT_INITIALIZED,
            format!("`{method}` arrived before `initialize`"),
        )
    }

    #[allow(dead_code)] // part of the JSON-RPC error surface; wired up with diagnostics
    pub fn with_data(mut self, data: Json) -> RpcError {
        self.data = Some(data);
        self
    }
}

impl std::fmt::Display for RpcError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{} (code {})", self.message, self.code)
    }
}

impl std::error::Error for RpcError {}

/// Decode one JSON-RPC message.
pub fn parse_incoming(text: &str) -> Result<Incoming, RpcError> {
    let value = Json::parse(text)
        .map_err(|error| RpcError::new(PARSE_ERROR, format!("invalid JSON: {error}")))?;
    if value.as_object().is_none() {
        return Err(RpcError::new(
            INVALID_REQUEST,
            "a JSON-RPC message must be a JSON object",
        ));
    }

    let id = value.get("id").cloned();
    let method = value.get_str("method").map(str::to_owned);
    let params = value.get("params").cloned().unwrap_or(Json::Null);
    match (method, id) {
        (Some(method), Some(id)) => Ok(Incoming::Request { id, method, params }),
        (Some(method), None) => Ok(Incoming::Notification { method, params }),
        (None, Some(id)) => Ok(Incoming::Response { id }),
        (None, None) => Err(RpcError::new(
            INVALID_REQUEST,
            "a JSON-RPC message needs a `method` or an `id`",
        )),
    }
}

/// A successful response envelope.
pub fn result_text(id: &Json, result: Json) -> String {
    Json::object([
        ("jsonrpc", Json::string("2.0")),
        ("id", id.clone()),
        ("result", result),
    ])
    .to_text()
}

/// An error response envelope.
pub fn error_text(id: &Json, error: &RpcError) -> String {
    let mut fields = vec![
        ("code", Json::Integer(i64::from(error.code))),
        ("message", Json::string(error.message.clone())),
    ];
    if let Some(data) = &error.data {
        fields.push(("data", data.clone()));
    }
    Json::object([
        ("jsonrpc", Json::string("2.0")),
        ("id", id.clone()),
        ("error", Json::object(fields)),
    ])
    .to_text()
}

/// A server-initiated notification envelope (reserved for diagnostics).
#[allow(dead_code)] // wired up by `publishDiagnostics` in the next round
pub fn notification_text(method: &str, params: Json) -> String {
    Json::object([
        ("jsonrpc", Json::string("2.0")),
        ("method", Json::string(method)),
        ("params", params),
    ])
    .to_text()
}

// ---------------------------------------------------------------------------
// LSP positions and ranges
// ---------------------------------------------------------------------------

/// An LSP position: 0-based line, 0-based UTF-16 code-unit offset in the line.
///
/// The design database uses 1-based lines and *character* columns, so the
/// conversion lives in [`super::document`], where the buffer text is available.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Position {
    pub line: u32,
    pub character: u32,
}

impl Position {
    pub fn new(line: u32, character: u32) -> Position {
        Position { line, character }
    }

    pub fn from_json(value: &Json) -> Option<Position> {
        let line = u32::try_from(value.get("line")?.as_i64()?).ok()?;
        let character = u32::try_from(value.get("character")?.as_i64()?).ok()?;
        Some(Position { line, character })
    }

    pub fn to_json(self) -> Json {
        Json::object([
            ("line", Json::Integer(i64::from(self.line))),
            ("character", Json::Integer(i64::from(self.character))),
        ])
    }
}

/// A half-open LSP range.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Range {
    pub start: Position,
    pub end: Position,
}

impl Range {
    pub fn new(start: Position, end: Position) -> Range {
        Range { start, end }
    }

    pub fn from_json(value: &Json) -> Option<Range> {
        Some(Range {
            start: Position::from_json(value.get("start")?)?,
            end: Position::from_json(value.get("end")?)?,
        })
    }

    pub fn to_json(self) -> Json {
        Json::object([
            ("start", self.start.to_json()),
            ("end", self.end.to_json()),
        ])
    }
}

/// One `textDocument/definition` (and `references`) entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Location {
    pub uri: String,
    pub range: Range,
}

impl Location {
    pub fn new(uri: impl Into<String>, range: Range) -> Location {
        Location {
            uri: uri.into(),
            range,
        }
    }

    /// `{"uri": ..., "range": ...}`, as `textDocument/definition` and
    /// `textDocument/references` return it.
    pub fn to_json(&self) -> Json {
        Json::object([
            ("uri", Json::string(self.uri.clone())),
            ("range", self.range.to_json()),
        ])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_requests_notifications_and_responses() {
        let request = parse_incoming(
            r#"{"jsonrpc":"2.0","id":"abc","method":"textDocument/hover","params":{"x":1}}"#,
        )
        .expect("request");
        match request {
            Incoming::Request { id, method, params } => {
                assert_eq!(id, Json::String("abc".to_owned()));
                assert_eq!(method, "textDocument/hover");
                assert_eq!(params.get_i64("x"), Some(1));
            }
            other => panic!("expected a request, got {other:?}"),
        }

        let notification = parse_incoming(r#"{"jsonrpc":"2.0","method":"exit"}"#)
            .expect("notification");
        assert_eq!(
            notification,
            Incoming::Notification {
                method: "exit".to_owned(),
                params: Json::Null
            }
        );

        let response =
            parse_incoming(r#"{"jsonrpc":"2.0","id":9,"result":null}"#).expect("response");
        assert_eq!(
            response,
            Incoming::Response {
                id: Json::Integer(9)
            }
        );
    }

    #[test]
    fn malformed_messages_carry_the_right_error_code() {
        let parse_error = parse_incoming("{").expect_err("broken JSON");
        assert_eq!(parse_error.code, PARSE_ERROR);
        assert!(parse_error.message.contains("invalid JSON"), "{parse_error}");

        let invalid = parse_incoming("[1,2]").expect_err("not an object");
        assert_eq!(invalid.code, INVALID_REQUEST);

        let no_id_no_method = parse_incoming("{}").expect_err("neither id nor method");
        assert_eq!(no_id_no_method.code, INVALID_REQUEST);
    }

    #[test]
    fn envelopes_are_valid_json_rpc() {
        let result = Json::object([("ok", Json::Bool(true))]);
        let text = result_text(&Json::Integer(3), result.clone());
        let decoded = Json::parse(&text).expect("valid JSON");
        assert_eq!(decoded.get_str("jsonrpc"), Some("2.0"));
        assert_eq!(decoded.get_i64("id"), Some(3));
        assert_eq!(decoded.get("result"), Some(&result));

        let text = error_text(
            &Json::Null,
            &RpcError::new(METHOD_NOT_FOUND, "nope").with_data(Json::string("detail")),
        );
        let decoded = Json::parse(&text).expect("valid JSON");
        let error = decoded.get("error").expect("error member");
        assert_eq!(error.get_i64("code"), Some(i64::from(METHOD_NOT_FOUND)));
        assert_eq!(error.get_str("message"), Some("nope"));
        assert_eq!(error.get_str("data"), Some("detail"));
    }

    #[test]
    fn positions_and_ranges_round_trip() {
        let range = Range::new(Position::new(3, 1), Position::new(3, 9));
        let decoded = Range::from_json(&range.to_json()).expect("range");
        assert_eq!(decoded, range);
        assert!(Position::from_json(&Json::object([("line", Json::Integer(0))])).is_none());
        // Negative or non-numeric coordinates are rejected rather than wrapped.
        assert!(Position::from_json(&Json::object([
            ("line", Json::Integer(-1)),
            ("character", Json::Integer(0))
        ]))
        .is_none());
    }
}
