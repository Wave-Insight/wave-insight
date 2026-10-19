//! `lsp` — the desktop stdio transport for the design-database language server.
//!
//! The protocol logic lives in `wave_insight_lib::lsp` and is shared with the
//! wasm build; this binary only does three things:
//!
//! * load the SystemVerilog files named on the command line into the design
//!   database, so elaboration works for design files the editor has not opened
//!   yet,
//! * frame JSON-RPC over stdin/stdout with `lsp-server`,
//! * hand each message's JSON text to [`LspServer::handle_message`] and send the
//!   replies back.
//!
//! Diagnostic chatter goes to stderr: stdout belongs to the protocol.
//!
//! Usage: `lsp [--stdio] [--top <module>] [<source.sv> ...]`
//!
//! Note: the transport decodes a whole LSP frame before handing it over, so a
//! client that writes malformed JSON-RPC breaks the stream (the protocol core
//! itself answers `-32700`; the wasm path exercises that).

use std::path::PathBuf;

use lsp_server::{Connection, Message, Notification, Request, RequestId, Response, ResponseError};
use serde_json::Value;
use wave_insight_lib::design::DesignDb;
use wave_insight_lib::lsp::{normalize_path, LspServer};

const USAGE: &str = "\
usage: lsp [--stdio] [--top <module>] [<source.sv> ...]

Runs the wave-insight language server over stdin/stdout.

  --stdio          accepted for clients that always pass it (it is the only transport)
  --top <module>   elaborate this module instead of the automatic root module
  <source.sv>      SystemVerilog files to load before the client opens anything
";

/// Command line options of the `lsp` binary.
#[derive(Debug, Default, PartialEq, Eq)]
struct Options {
    top: Option<String>,
    sources: Vec<PathBuf>,
}

#[derive(Debug, PartialEq, Eq)]
enum ParseOutcome {
    Options(Options),
    Help,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let options = match parse_options(std::env::args().skip(1)) {
        Ok(ParseOutcome::Options(options)) => options,
        Ok(ParseOutcome::Help) => {
            print!("{USAGE}");
            return Ok(());
        }
        Err(error) => {
            eprintln!("lsp: {error}\n\n{USAGE}");
            std::process::exit(2);
        }
    };
    serve(options)
}

fn parse_options<I>(args: I) -> Result<ParseOutcome, String>
where
    I: IntoIterator<Item = String>,
{
    let mut options = Options::default();
    let mut args = args.into_iter();
    while let Some(argument) = args.next() {
        match argument.as_str() {
            // Clients such as VS Code always pass `--stdio`.
            "--stdio" => {}
            "-h" | "--help" => return Ok(ParseOutcome::Help),
            "--top" => {
                let Some(top) = args.next() else {
                    return Err("`--top` needs a module name".to_owned());
                };
                options.top = Some(top);
            }
            other if other.starts_with("--top=") => {
                options.top = Some(other["--top=".len()..].to_owned());
            }
            other if other.starts_with('-') => {
                return Err(format!("unknown option `{other}`"));
            }
            source => options.sources.push(PathBuf::from(source)),
        }
    }
    Ok(ParseOutcome::Options(options))
}

/// Load the design, then pump messages until the client exits.
fn serve(options: Options) -> Result<(), Box<dyn std::error::Error>> {
    let mut db = DesignDb::new();
    for source in &options.sources {
        // `didOpen` keys documents by their (absolute) URI, so the preloaded
        // files must use the same spelling or the same file would end up as two
        // differently keyed copies with colliding module names.
        let source = normalize_path(source);
        if !db.add_file(&source) {
            eprintln!("lsp: could not read `{}`", source.display());
        }
    }
    for diagnostic in db.diagnostics() {
        eprintln!("lsp: {diagnostic}");
    }
    eprintln!(
        "lsp: loaded {} file(s), {} module(s){}",
        db.files().len(),
        db.modules().len(),
        match &options.top {
            Some(top) => format!(", top `{top}`"),
            None => String::new(),
        }
    );

    let mut server = LspServer::with_db(db);
    server.set_top(options.top);

    let (connection, io_threads) = Connection::stdio();
    while let Ok(message) = connection.receiver.recv() {
        match message {
            Message::Request(request) => {
                forward(&mut server, &connection, request_text(&request))?;
            }
            Message::Notification(notification) => {
                forward(&mut server, &connection, notification_text(&notification))?;
            }
            // This server never sends requests, so a response cannot be for it.
            Message::Response(response) => {
                eprintln!("lsp: ignoring an unexpected response for id {}", response.id);
            }
        }
        if server.exit_requested() {
            break;
        }
    }
    drop(connection);
    io_threads.join()?;
    Ok(())
}

/// Send one JSON-RPC message through the core and write its replies.
fn forward(
    server: &mut LspServer,
    connection: &Connection,
    message: String,
) -> Result<(), Box<dyn std::error::Error>> {
    for reply in server.handle_message(&message) {
        let reply = decode_reply(&reply)?;
        connection
            .sender
            .send(reply)
            .map_err(|error| format!("cannot send a reply to the client: {error}"))?;
    }
    Ok(())
}

/// Re-encode a typed message as the JSON-RPC text the core speaks.
fn request_text(request: &Request) -> String {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": request.id,
        "method": request.method,
        "params": request.params,
    })
    .to_string()
}

fn notification_text(notification: &Notification) -> String {
    serde_json::json!({
        "jsonrpc": "2.0",
        "method": notification.method,
        "params": notification.params,
    })
    .to_string()
}

/// Decode a reply envelope produced by the core into `lsp-server`'s `Message`.
fn decode_reply(reply: &str) -> Result<Message, Box<dyn std::error::Error>> {
    let value: Value = serde_json::from_str(reply)?;
    if let Some(method) = value.get("method").and_then(Value::as_str) {
        let params = value.get("params").cloned().unwrap_or(Value::Null);
        return Ok(Message::Notification(Notification {
            method: method.to_owned(),
            params,
        }));
    }

    let id = decode_request_id(value.get("id").unwrap_or(&Value::Null))?;
    let error = value.get("error").map(|error| ResponseError {
        code: error.get("code").and_then(Value::as_i64).unwrap_or(-32603) as i32,
        message: error
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("unknown error")
            .to_owned(),
        data: error.get("data").cloned(),
    });
    // `Some(Value::Null)` is important: a successful `null` result has to reach
    // the client as `"result": null`, not as a missing member.
    let result = value.get("result").cloned();
    Ok(Message::Response(Response { id, result, error }))
}

fn decode_request_id(value: &Value) -> Result<RequestId, Box<dyn std::error::Error>> {
    match value {
        Value::Number(number) => {
            let number = number.as_i64().ok_or("a request id must be an integer")?;
            let number = i32::try_from(number).map_err(|_| "a request id must fit in i32")?;
            Ok(RequestId::from(number))
        }
        Value::String(text) => Ok(RequestId::from(text.clone())),
        other => Err(format!("unsupported request id `{other}`").into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options(args: &[&str]) -> Options {
        match parse_options(args.iter().map(|argument| (*argument).to_owned()))
            .expect("valid command line")
        {
            ParseOutcome::Options(options) => options,
            ParseOutcome::Help => panic!("expected options, got help"),
        }
    }

    #[test]
    fn options_accept_stdio_top_and_sources() {
        assert_eq!(
            options(&["--stdio", "--top", "top", "rtl/top.sv", "rtl/cpu.sv"]),
            Options {
                top: Some("top".to_owned()),
                sources: vec![PathBuf::from("rtl/top.sv"), PathBuf::from("rtl/cpu.sv")],
            }
        );
        assert_eq!(
            options(&["--top=cpu"]),
            Options {
                top: Some("cpu".to_owned()),
                sources: Vec::new(),
            }
        );
        assert_eq!(options(&[]), Options::default());

        assert_eq!(
            parse_options([] as [String; 0]).expect("empty"),
            ParseOutcome::Options(Options::default())
        );
        assert!(parse_options(["--nope".to_owned()]).is_err());
        assert_eq!(
            parse_options(["--top".to_owned()]),
            Err("`--top` needs a module name".to_owned())
        );
        assert_eq!(
            parse_options(["-h".to_owned()]).expect("help"),
            ParseOutcome::Help
        );
    }

    #[test]
    fn replies_are_decoded_into_lsp_server_messages() {
        let message =
            decode_reply(r#"{"jsonrpc":"2.0","id":4,"result":{"ok":true}}"#).expect("response");
        match message {
            Message::Response(response) => {
                assert_eq!(response.id, RequestId::from(4));
                assert!(response.error.is_none());
                assert_eq!(response.result, Some(serde_json::json!({ "ok": true })));
            }
            other => panic!("expected a response, got {other:?}"),
        }

        // A `null` result survives as an explicit member.
        let message =
            decode_reply(r#"{"jsonrpc":"2.0","id":"abc","result":null}"#).expect("response");
        match message {
            Message::Response(response) => {
                assert_eq!(response.id, RequestId::from("abc".to_owned()));
                assert_eq!(response.result, Some(Value::Null));
            }
            other => panic!("expected a response, got {other:?}"),
        }

        let message = decode_reply(r#"{"jsonrpc":"2.0","method":"window/logMessage","params":{}}"#)
            .expect("notification");
        assert!(matches!(message, Message::Notification(_)));

        // A null id cannot become a `RequestId`; this is a transport-level
        // limitation documented in the module docs.
        assert!(
            decode_reply(r#"{"jsonrpc":"2.0","id":null,"error":{"code":-32700,"message":"x"}}"#)
                .is_err()
        );
    }

    #[test]
    fn a_request_round_trips_through_the_protocol_core() {
        let mut server = LspServer::new();
        let request = Request::new(RequestId::from(1), "initialize".to_owned(), serde_json::json!({}));
        let replies = server.handle_message(&request_text(&request));
        assert_eq!(replies.len(), 1);
        match decode_reply(&replies[0]).expect("valid reply") {
            Message::Response(response) => {
                assert_eq!(response.id, RequestId::from(1));
                let result = response.result.expect("initialize returns a result");
                assert!(result
                    .get("capabilities")
                    .and_then(|capabilities| capabilities.get("hoverProvider"))
                    .is_some());
            }
            other => panic!("expected a response, got {other:?}"),
        }
    }
}
