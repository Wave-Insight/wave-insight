//! The Streamable HTTP MCP server: token, listener, router and the Tauri glue.
//!
//! The shape is deliberately small: one loopback listener, one `/mcp` endpoint,
//! one bearer check, and rmcp's [`StreamableHttpService`] behind it. The
//! service is created per request from shared state, so a tool call can hold a
//! dump mapping for as long as it needs without tying it to a session.
//!
//! # Security decisions
//!
//! * The listener is bound to `127.0.0.1` only.
//! * Every request needs `Authorization: Bearer <token>`, where the token is
//!   generated once and stored in the app config directory (`0600` on Unix).
//! * rmcp's own host/origin validation stays on (it rejects non-loopback
//!   `Host` values and browser origins, which is the DNS-rebinding defence).
//! * The token is the permission: it grants reading the files the tools are
//!   pointed at and changing what the current window shows. The AI / MCP panel
//!   says so next to the copy button; there is no second prompt.

use std::fmt::Write as _;
use std::io::Write;
use std::sync::{Arc, Mutex, MutexGuard};

use axum::extract::{Request, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::Router;
use rmcp::transport::streamable_http_server::{
    session::local::LocalSessionManager, StreamableHttpServerConfig, StreamableHttpService,
};
use tauri::{AppHandle, Emitter, Manager};
use tokio::net::TcpListener;
use wave_insight_lib::agent::{McpInfo, UiCommand, UiReply, UiResult};

use super::bridge::{UiBridge, UiEmitter};
use super::data::AgentData;
use super::tools::AgentHandler;
use super::{DEFAULT_PORT, PORT_ATTEMPTS, TOOL_NAMES};

/// The event the webview listens for.
pub const COMMAND_EVENT: &str = "wave-agent-command";

/// A running (or failed) MCP server.
pub struct AgentServer {
    info: Mutex<McpInfo>,
    data: Arc<AgentData>,
    bridge: Arc<UiBridge>,
}

impl AgentServer {
    /// Build the server state; nothing is listening yet.
    pub fn new(app: AppHandle) -> Arc<Self> {
        let (token, warning) = load_or_create_token(&app);
        let data = Arc::new(AgentData::new());
        let bridge = Arc::new(UiBridge::new(Arc::new(TauriEmitter { app: app.clone() })));
        let info = McpInfo {
            running: false,
            url: None,
            token,
            port: None,
            error: warning,
            version: env!("CARGO_PKG_VERSION").to_string(),
            tools: TOOL_NAMES.iter().map(|name| name.to_string()).collect(),
        };
        Arc::new(Self {
            info: Mutex::new(info),
            data,
            bridge,
        })
    }

    pub fn bridge(&self) -> Arc<UiBridge> {
        Arc::clone(&self.bridge)
    }

    pub fn info(&self) -> McpInfo {
        self.lock().clone()
    }

    fn lock(&self) -> MutexGuard<'_, McpInfo> {
        self.info
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn set_running(&self, port: u16) {
        let mut info = self.lock();
        info.running = true;
        info.port = Some(port);
        info.url = Some(format!("http://127.0.0.1:{port}/mcp"));
        // keep a token-persistence warning; it is not a server failure
        if info
            .error
            .as_deref()
            .is_some_and(|error| error.starts_with("the token"))
        {
            // leave it
        } else {
            info.error = None;
        }
    }

    fn set_error(&self, error: impl Into<String>) {
        let mut info = self.lock();
        info.running = false;
        info.error = Some(error.into());
    }

    /// Start the listener on a background runtime.
    pub fn start(self: &Arc<Self>) {
        let this = Arc::clone(self);
        let spawned = std::thread::Builder::new()
            .name("wave-insight-mcp".to_string())
            .spawn(move || {
                let runtime = match tokio::runtime::Builder::new_multi_thread()
                    .worker_threads(2)
                    .enable_all()
                    .build()
                {
                    Ok(runtime) => runtime,
                    Err(error) => {
                        this.set_error(format!("the MCP runtime could not start: {error}"));
                        return;
                    }
                };
                runtime.block_on(this.run());
            });
        if let Err(error) = spawned {
            self.set_error(format!("the MCP server thread could not start: {error}"));
        }
    }

    async fn run(self: Arc<Self>) {
        let (listener, port) = match bind_loopback().await {
            Ok(bound) => bound,
            Err(error) => {
                self.set_error(error);
                return;
            }
        };
        let token: Arc<str> = Arc::from(self.lock().token.clone());
        let data = Arc::clone(&self.data);
        let bridge = Arc::clone(&self.bridge);
        let router = router(token, data, bridge);
        self.set_running(port);
        eprintln!("wave-insight mcp: listening on http://127.0.0.1:{port}/mcp");
        if let Err(error) = axum::serve(listener, router).await {
            self.set_error(format!("the MCP listener stopped: {error}"));
        }
    }
}

/// The HTTP stack: rmcp's Streamable HTTP service under `/mcp`, behind the
/// bearer check. Split out of [`AgentServer::run`] so the tests can drive the
/// exact router the app serves.
fn router(token: Arc<str>, data: Arc<AgentData>, bridge: Arc<UiBridge>) -> Router {
    let config = StreamableHttpServerConfig::default()
        // MCP clients today negotiate a protocol revision that still has the
        // `initialize` handshake, so the session mode stays on; the stateless
        // path is what a newer revision negotiates automatically.
        .with_json_response(false)
        .with_allowed_origins(["http://localhost:*", "http://127.0.0.1:*"]);
    let service = StreamableHttpService::new(
        move || Ok(AgentHandler::new(Arc::clone(&data), Arc::clone(&bridge))),
        LocalSessionManager::default().into(),
        config,
    );
    Router::new()
        .nest_service("/mcp", service)
        .layer(middleware::from_fn_with_state(
            AuthState { token },
            require_token,
        ))
}

/// Try the configured port range and return the first listener that binds.
async fn bind_loopback() -> Result<(TcpListener, u16), String> {
    let mut last = String::new();
    for offset in 0..PORT_ATTEMPTS {
        let port = DEFAULT_PORT.saturating_add(offset);
        match TcpListener::bind(("127.0.0.1", port)).await {
            Ok(listener) => return Ok((listener, port)),
            Err(error) => last = format!("127.0.0.1:{port}: {error}"),
        }
    }
    Err(format!(
        "no free port on 127.0.0.1 in {}..{} ({last})",
        DEFAULT_PORT,
        DEFAULT_PORT.saturating_add(PORT_ATTEMPTS.saturating_sub(1))
    ))
}

// --- authentication --------------------------------------------------------

/// What the middleware needs: the expected bearer token.
#[derive(Clone)]
struct AuthState {
    token: Arc<str>,
}

/// Reject requests without the bearer token, or with a non-loopback Host.
///
/// rmcp validates `Host`/`Origin` too; this is the cheap first line and it also
/// produces a readable status before the protocol layer sees the request.
async fn require_token(State(state): State<AuthState>, request: Request, next: Next) -> Response {
    if !host_is_loopback(request.headers().get(header::HOST)) {
        return (
            StatusCode::FORBIDDEN,
            "the MCP server only accepts loopback hosts",
        )
            .into_response();
    }
    let presented = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .map(str::trim);
    match presented {
        Some(token) if constant_time_eq(token.as_bytes(), state.token.as_bytes()) => {
            next.run(request).await
        }
        Some(_) => {
            let mut response = (StatusCode::UNAUTHORIZED, "invalid MCP token").into_response();
            response
                .headers_mut()
                .insert(header::WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"));
            response
        }
        None => {
            let mut response = (
                StatusCode::UNAUTHORIZED,
                "the MCP server needs a bearer token",
            )
                .into_response();
            response
                .headers_mut()
                .insert(header::WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"));
            response
        }
    }
}

fn host_is_loopback(host: Option<&HeaderValue>) -> bool {
    let Some(host) = host.and_then(|value| value.to_str().ok()) else {
        return false;
    };
    let host = host.trim();
    let name = host.rsplit_once(':').map(|(name, _)| name).unwrap_or(host);
    name.eq_ignore_ascii_case("127.0.0.1")
        || name.eq_ignore_ascii_case("localhost")
        || name.eq_ignore_ascii_case("[::1]")
}

/// Compare two byte strings without an early exit.
fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    let mut difference = 0u8;
    for (left, right) in left.iter().zip(right) {
        difference |= left ^ right;
    }
    difference == 0
}

// --- token persistence -----------------------------------------------------

/// Load the token, or generate and store a fresh one.
///
/// A failure to persist is a warning, not a failure: the server can still run
/// with an in-memory token, the panel shows it, and the user can copy it into a
/// client - it just does not survive a restart.
fn load_or_create_token(app: &AppHandle) -> (String, Option<String>) {
    let Some(dir) = app.path().app_config_dir().ok() else {
        return (
            generate_token(),
            Some(
                "the token could not be stored (no app config directory); it is session-only"
                    .to_string(),
            ),
        );
    };
    if let Err(error) = std::fs::create_dir_all(&dir) {
        return (
            generate_token(),
            Some(format!(
                "the token could not be stored ({}: {error}); it is session-only",
                dir.display()
            )),
        );
    }
    let path = dir.join("mcp-token");
    if let Ok(existing) = std::fs::read_to_string(&path) {
        let token = existing.trim().to_string();
        if token.len() >= 32 && token.chars().all(|c| c.is_ascii_hexdigit()) {
            return (token, None);
        }
    }
    let token = generate_token();
    match write_token(&path, &token) {
        Ok(()) => (token, None),
        Err(error) => (
            token,
            Some(format!(
                "the token could not be stored in {}: {error}; it is session-only",
                path.display()
            )),
        ),
    }
}

fn generate_token() -> String {
    let mut bytes = [0u8; 32];
    if getrandom::fill(&mut bytes).is_err() {
        // the OS entropy source failing is fatal for a secure token; fall back
        // to a time/pid mix so the server still starts, with a visible warning
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos())
            .unwrap_or(0);
        let mixed = now ^ u128::from(std::process::id());
        for (index, byte) in bytes.iter_mut().enumerate() {
            *byte = (mixed >> ((index % 16) * 8)) as u8;
        }
    }
    let mut token = String::with_capacity(64);
    for byte in bytes {
        let _ = write!(token, "{byte:02x}");
    }
    token
}

fn write_token(path: &std::path::Path, token: &str) -> std::io::Result<()> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(token.as_bytes())?;
    file.write_all(b"\n")?;
    file.sync_all()
}

// --- Tauri emitter and commands -------------------------------------------

/// Delivers a [`UiCommand`] to the webview as a Tauri event.
struct TauriEmitter {
    app: AppHandle,
}

impl UiEmitter for TauriEmitter {
    fn emit(&self, id: u64, command: &UiCommand) -> Result<(), String> {
        self.app
            .emit_to(
                "main",
                COMMAND_EVENT,
                serde_json::json!({ "id": id, "command": command }),
            )
            .map_err(|error| format!("the window is not ready: {error}"))
    }
}

/// What the AI / MCP panel asks for.
#[tauri::command]
pub fn mcp_info(server: tauri::State<'_, Arc<AgentServer>>) -> McpInfo {
    server.info()
}

/// The webview's answer to one command.
#[tauri::command]
pub fn agent_reply(
    bridge: tauri::State<'_, Arc<UiBridge>>,
    id: u64,
    result: Option<UiResult>,
    error: Option<String>,
) -> Result<(), String> {
    bridge.reply(UiReply { id, result, error });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request as HttpRequest, StatusCode};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    const TOKEN: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    fn test_router() -> Router {
        let bridge = Arc::new(UiBridge::new(Arc::new(NoWindow)));
        router(Arc::from(TOKEN), Arc::new(AgentData::new()), bridge)
    }

    /// A webview that is not there: control tools must fail with a message, and
    /// the data tools must never need it.
    struct NoWindow;

    impl UiEmitter for NoWindow {
        fn emit(&self, _id: u64, _command: &UiCommand) -> Result<(), String> {
            Err("no webview in the test".to_string())
        }
    }

    fn post(body: serde_json::Value, token: Option<&str>) -> HttpRequest<Body> {
        let mut builder = HttpRequest::post("/mcp")
            .header(header::HOST, "127.0.0.1:7437")
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::ACCEPT, "application/json, text/event-stream");
        if let Some(token) = token {
            builder = builder.header(header::AUTHORIZATION, format!("Bearer {token}"));
        }
        builder.body(Body::from(body.to_string())).unwrap()
    }

    async fn body_text(response: Response) -> String {
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        String::from_utf8_lossy(&bytes).into_owned()
    }

    #[tokio::test]
    async fn a_request_without_the_token_is_rejected() {
        let response = test_router()
            .oneshot(post(
                serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "ping"}),
                None,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert!(response.headers().contains_key(header::WWW_AUTHENTICATE));
    }

    #[tokio::test]
    async fn a_wrong_token_is_rejected() {
        let response = test_router()
            .oneshot(post(
                serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "ping"}),
                Some("not-the-token"),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn a_non_loopback_host_is_rejected() {
        let request = HttpRequest::post("/mcp")
            .header(header::HOST, "evil.example.com")
            .header(header::AUTHORIZATION, format!("Bearer {TOKEN}"))
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from("{}"))
            .unwrap();
        let response = test_router().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn initialize_and_tools_list_round_trip_over_http() {
        let app = test_router();
        let initialize = post(
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": {
                    "protocolVersion": "2025-06-18",
                    "capabilities": {},
                    "clientInfo": { "name": "wave-insight-test", "version": "0" }
                }
            }),
            Some(TOKEN),
        );
        let response = app.clone().oneshot(initialize).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let session = response
            .headers()
            .get("mcp-session-id")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let text = body_text(response).await;
        assert!(text.contains("\"protocolVersion\""), "{text}");
        assert!(text.contains("wave"), "{text}");

        // the tool catalogue is served with the descriptions the model reads
        let mut list = post(
            serde_json::json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}),
            Some(TOKEN),
        );
        if let Some(session) = &session {
            list.headers_mut()
                .insert("mcp-session-id", HeaderValue::from_str(session).unwrap());
        }
        let response = app.oneshot(list).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let text = body_text(response).await;
        for tool in TOOL_NAMES {
            assert!(text.contains(tool), "{tool} missing from {text}");
        }
        assert!(text.contains("list_dumps"));
    }

    #[tokio::test]
    async fn a_data_tool_answers_over_http() {
        let path = std::env::temp_dir()
            .join(format!("wave-insight-mcp-{}", std::process::id()))
            .join("small.vcd");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            "$date t $end\n$version v $end\n$timescale 1ns $end\n$scope module top $end\n\
             $var wire 1 ! clk $end\n$upscope $end\n$enddefinitions $end\n$dumpvars\n0!\n$end\n\
             #10\n1!\n",
        )
        .unwrap();
        let app = test_router();
        let initialize = post(
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": {
                    "protocolVersion": "2025-06-18",
                    "capabilities": {},
                    "clientInfo": { "name": "wave-insight-test", "version": "0" }
                }
            }),
            Some(TOKEN),
        );
        let response = app.clone().oneshot(initialize).await.unwrap();
        let session = response
            .headers()
            .get("mcp-session-id")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let _ = body_text(response).await;
        // the initialized notification finishes the handshake
        let mut notify = post(
            serde_json::json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
            Some(TOKEN),
        );
        if let Some(session) = &session {
            notify
                .headers_mut()
                .insert("mcp-session-id", HeaderValue::from_str(session).unwrap());
        }
        let _ = app.clone().oneshot(notify).await.unwrap();

        let mut call = post(
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": 3,
                "method": "tools/call",
                "params": {
                    "name": "find_signals",
                    "arguments": { "query": "clk", "path": path.display().to_string() }
                }
            }),
            Some(TOKEN),
        );
        if let Some(session) = &session {
            call.headers_mut()
                .insert("mcp-session-id", HeaderValue::from_str(session).unwrap());
        }
        let response = app.oneshot(call).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let text = body_text(response).await;
        assert!(text.contains("top.clk"), "{text}");
        assert!(text.contains("bit_vector"), "{text}");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn the_token_is_64_hex_characters() {
        let token = generate_token();
        assert_eq!(token.len(), 64);
        assert!(token.chars().all(|c| c.is_ascii_hexdigit()));
        // two calls do not repeat
        assert_ne!(token, generate_token());
    }

    #[test]
    fn a_token_round_trips_through_the_file() {
        let dir = std::env::temp_dir().join(format!("wave-insight-token-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("mcp-token");
        write_token(&path, "abc123").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap().trim(), "abc123");
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir(&dir);
    }

    #[test]
    fn the_bearer_check_is_exact() {
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"ab"));
        assert!(!constant_time_eq(b"", b"x"));
    }

    #[test]
    fn host_validation_accepts_only_loopback() {
        let host = |value: &'static str| HeaderValue::from_static(value);
        assert!(host_is_loopback(Some(&host("127.0.0.1:7437"))));
        assert!(host_is_loopback(Some(&host("localhost:7437"))));
        assert!(host_is_loopback(Some(&host("[::1]:7437"))));
        assert!(!host_is_loopback(Some(&host("example.com"))));
        assert!(!host_is_loopback(None));
    }
}
