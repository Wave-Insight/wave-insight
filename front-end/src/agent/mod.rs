//! The desktop AI integration's webview half.
//!
//! The MCP server lives in the Tauri backend (`src-tauri/src/agent/`). It reads
//! waveform files itself, but the *view* - which rows are on the wave, which
//! time range is visible, where the cursors sit - is store state that only this
//! side owns. So the backend sends a [`UiCommand`] as a Tauri event and waits
//! for the answer this module sends back through the `agent_reply` command.
//!
//! ```text
//! MCP tool -> UiBridge -> window event "wave-agent-command"
//!                                |
//!                                v
//!                     Msg::AgentCommand -> App::handle_agent_command
//!                                |
//!                                v
//!                     store action(s) -> UiResult -> agent_reply
//! ```
//!
//! Everything here is `tauri`-gated: the browser build has no backend to listen
//! to, and compiling this module there would only add dead code.

pub mod listener;
pub mod panel;
pub mod reply;
pub mod snapshot;

pub use listener::attach_agent_listener;
pub use panel::AgentPanel;
pub use reply::send_reply;

use wave_insight_lib::agent::UiCommand;

/// One command the backend sent, on its way to the store.
#[derive(Clone, Debug, PartialEq)]
pub struct AgentCommand {
    pub id: u64,
    pub command: UiCommand,
}
