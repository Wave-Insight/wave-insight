//! The control half: a request/response bridge to the webview.
//!
//! The state that "add this signal" or "show t=1000..2000" changes lives in the
//! webview's store, so the MCP tools cannot perform it themselves: they emit a
//! Tauri event carrying a [`UiCommand`] and wait for the webview to send the
//! matching [`UiResult`] back through the `agent_reply` command.
//!
//! The bridge is deliberately small and boring:
//!
//! * every request gets an id; a stray or late reply for an unknown id is
//!   dropped instead of panicking;
//! * a request times out, because a reloaded webview or a buggy listener must
//!   turn into an error message the model can act on, not a hung tool call;
//! * nothing here touches the waveform, so the timeout is the only failure
//!   mode this layer adds.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

use tokio::sync::oneshot;
use wave_insight_lib::agent::{UiCommand, UiReply, UiResult};

/// How long a UI command may take before the tool call reports a timeout.
pub const UI_TIMEOUT: Duration = Duration::from_secs(5);

/// How the bridge reaches the webview (Tauri in the app, a test double in the
/// tests).
pub trait UiEmitter: Send + Sync {
    /// Deliver one command. An `Err` means "the webview is not there".
    fn emit(&self, id: u64, command: &UiCommand) -> Result<(), String>;
}

/// A pending request: the channel the webview's reply arrives on.
struct Pending {
    tx: oneshot::Sender<Result<UiResult, String>>,
}

/// The request/response bridge between the MCP server and the webview.
pub struct UiBridge {
    emitter: std::sync::Arc<dyn UiEmitter>,
    pending: Mutex<HashMap<u64, Pending>>,
    next: AtomicU64,
}

impl UiBridge {
    pub fn new(emitter: std::sync::Arc<dyn UiEmitter>) -> Self {
        Self {
            emitter,
            pending: Mutex::new(HashMap::new()),
            next: AtomicU64::new(1),
        }
    }

    fn lock(&self) -> MutexGuard<'_, HashMap<u64, Pending>> {
        self.pending
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Send one command and wait for the webview's answer.
    pub async fn request(&self, command: UiCommand) -> Result<UiResult, String> {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.lock().insert(id, Pending { tx });
        if let Err(error) = self.emitter.emit(id, &command) {
            self.lock().remove(&id);
            return Err(format!("the window could not be reached: {error}"));
        }
        match tokio::time::timeout(UI_TIMEOUT, rx).await {
            Ok(Ok(Ok(result))) => Ok(result),
            Ok(Ok(Err(error))) => Err(error),
            // the sender was dropped without an answer
            Ok(Err(_)) => Err(format!(
                "the window dropped the request ({}): reload it and try again",
                command_kind(&command)
            )),
            Err(_) => {
                self.lock().remove(&id);
                Err(format!(
                    "the window did not answer within {}s ({}): reload it and try again",
                    UI_TIMEOUT.as_secs(),
                    command_kind(&command)
                ))
            }
        }
    }

    /// Deliver the webview's reply. An unknown id is ignored: it belongs to a
    /// request that already timed out.
    pub fn reply(&self, reply: UiReply) {
        let Some(pending) = self.lock().remove(&reply.id) else {
            return;
        };
        let outcome = match (reply.result, reply.error) {
            (Some(result), _) => Ok(result),
            (None, Some(error)) => Err(error),
            (None, None) => Err("the window sent an empty answer".to_string()),
        };
        // the receiver may be gone (a timed-out request): dropping is fine
        let _ = pending.tx.send(outcome);
    }

    /// How many requests are waiting (diagnostics and tests).
    #[allow(dead_code)]
    pub fn pending(&self) -> usize {
        self.lock().len()
    }
}

/// The `kind` tag of a command, for error messages.
pub fn command_kind(command: &UiCommand) -> &'static str {
    match command {
        UiCommand::Snapshot => "snapshot",
        UiCommand::AddSignals { .. } => "add_signals",
        UiCommand::RemoveSignals { .. } => "remove_signals",
        UiCommand::SetTimeRange { .. } => "set_time_range",
        UiCommand::SetCursors { .. } => "set_cursors",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;
    use std::sync::Arc;

    /// An emitter that answers immediately, the way the webview does.
    struct Answering;

    impl UiEmitter for Answering {
        fn emit(&self, id: u64, _command: &UiCommand) -> Result<(), String> {
            let _ = id;
            Ok(())
        }
    }

    /// An emitter that records what it was asked to send.
    struct Recording {
        count: AtomicUsize,
        fail: bool,
    }

    impl UiEmitter for Recording {
        fn emit(&self, id: u64, _command: &UiCommand) -> Result<(), String> {
            self.count.fetch_add(1, Ordering::Relaxed);
            assert!(id > 0);
            if self.fail {
                Err("no webview".to_string())
            } else {
                Ok(())
            }
        }
    }

    /// An emitter that never answers, to exercise the timeout.
    struct Silent;

    impl UiEmitter for Silent {
        fn emit(&self, _id: u64, _command: &UiCommand) -> Result<(), String> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn a_reply_answers_the_matching_request() {
        let bridge = Arc::new(UiBridge::new(Arc::new(Answering)));
        let bridge_for_reply = Arc::clone(&bridge);
        let request = tokio::spawn({
            let bridge = Arc::clone(&bridge);
            async move {
                bridge
                    .request(UiCommand::SetCursors {
                        one: Some(wave_insight_lib::agent::CursorMove::Set { time: 1 }),
                        two: None,
                    })
                    .await
            }
        });
        // the request is pending until the webview answers
        while bridge.pending() == 0 {
            tokio::task::yield_now().await;
        }
        let id = {
            let pending = bridge_for_reply.lock();
            *pending.keys().next().unwrap()
        };
        bridge_for_reply.reply(UiReply::ok(
            id,
            UiResult::SetCursors(wave_insight_lib::agent::SetCursorsResult {
                one: Some(1),
                two: None,
            }),
        ));
        let result = request.await.unwrap().unwrap();
        assert_eq!(result.kind(), "set_cursors");
        assert_eq!(bridge.pending(), 0);
        // a late reply for an unknown id is ignored, not a panic
        bridge.reply(UiReply::error(999, "late"));
    }

    #[tokio::test]
    async fn a_listener_error_clears_the_pending_request() {
        let bridge = Arc::new(UiBridge::new(Arc::new(Silent)));
        let request = tokio::spawn({
            let bridge = Arc::clone(&bridge);
            async move { bridge.request(UiCommand::Snapshot).await }
        });
        while bridge.pending() == 0 {
            tokio::task::yield_now().await;
        }
        let id = *bridge.lock().keys().next().unwrap();
        bridge.reply(UiReply::error(id, "the UI refused"));
        let error = request.await.unwrap().unwrap_err();
        assert_eq!(error, "the UI refused");
        assert_eq!(bridge.pending(), 0);
    }

    #[tokio::test]
    async fn an_unreachable_webview_is_an_error_not_a_hang() {
        let emitter = Arc::new(Recording {
            count: AtomicUsize::new(0),
            fail: true,
        });
        let bridge = UiBridge::new(emitter.clone());
        let error = bridge.request(UiCommand::Snapshot).await.unwrap_err();
        assert!(error.contains("no webview"), "{error}");
        assert_eq!(emitter.count.load(Ordering::Relaxed), 1);
        assert_eq!(bridge.pending(), 0);
    }

    #[test]
    fn a_command_kind_is_the_snake_case_name() {
        assert_eq!(command_kind(&UiCommand::Snapshot), "snapshot");
        assert_eq!(
            command_kind(&UiCommand::AddSignals { signals: vec![] }),
            "add_signals"
        );
    }
}
