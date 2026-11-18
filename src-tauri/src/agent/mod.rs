//! The desktop AI integration: a local MCP server over the native backend.
//!
//! # Why a server instead of an embedded model
//!
//! The interesting dumps are the large ones, and the app already has the right
//! machinery for them: `dumps::DumpRegistry` memory-maps a file and answers
//! bounded queries, so one signal's value costs a streamed lookup rather than a
//! copy of the whole trace into wasm. An embedded chat panel would have to
//! reimplement the model provider protocol, streaming and key storage, and it
//! would still need a tool loop. Speaking MCP instead means an existing client
//! (Claude Code, Cursor, ...) is the conversation, and this app is the tool
//! server:
//!
//! ```text
//! MCP client --Streamable HTTP + bearer token--> agent::server (127.0.0.1)
//!                                                  |-- agent::data  -> mmap dump
//!                                                  `-- agent::bridge -> webview store
//! ```
//!
//! # Two halves
//!
//! * The **data tools** (`list_dumps` aside, they take a file path) answer from
//!   the registry directly. They never need the webview, so they work whether
//!   the app is showing that dump or not, and `extract_changes` streams a CSV
//!   instead of stuffing a history through a JSON answer.
//! * The **control tools** change what the window shows. The state lives in the
//!   webview's store, so they go through [`bridge::UiBridge`]: the server emits
//!   a Tauri event, the front end dispatches the equivalent action and replies
//!   with the outcome.
//!
//! # Security
//!
//! The listener is loopback-only and every request needs the bearer token from
//! the app config directory. The token is the permission: whoever holds it can
//! read the files it points at and change the current view, which is what the
//! AI / MCP panel says next to the copy button. The server never writes outside
//! the system temp directory.

pub mod bridge;
pub mod data;
pub mod extract;
pub mod server;
pub mod tools;

/// The tools the server exposes, in the order the panel lists them.
pub const TOOL_NAMES: &[&str] = &[
    "list_dumps",
    "find_signals",
    "get_values",
    "get_changes",
    "search_value",
    "summarize_signals",
    "extract_changes",
    "add_signals",
    "remove_signals",
    "set_time_range",
    "set_cursors",
];

/// The first port the server tries.
pub const DEFAULT_PORT: u16 = 7437;
/// How many ports above [`DEFAULT_PORT`] are tried before giving up.
pub const PORT_ATTEMPTS: u16 = 10;

/// Open a dump for a read-only query, mapping it when the platform allows it.
///
/// Extraction uses this instead of the agent cache: it may run for minutes and
/// must not hold a cache slot (or its mapping) while it does.
pub(crate) fn open_for_read(
    path: &std::path::Path,
) -> Result<wave_insight_lib::waveform::Waveform, String> {
    dumps::open_waveform(path)
}

pub(crate) use crate::dumps;
