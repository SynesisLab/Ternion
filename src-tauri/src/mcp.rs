//! MCP client (design §6.5): Ternion is an MCP client for stdio servers —
//! child processes speaking newline-delimited JSON-RPC 2.0 (initialize →
//! notifications/initialized → tools/list → tools/call). A server's tools
//! merge into the orchestrator's tool list namespaced `mcp__<server>__<tool>`
//! so a server can never shadow a bundled tool. Every call gates through the
//! §6.6 matrix (ask by default, grantable) like the shell tool.
//!
//! The transport seam mirrors the provider tests: a session runs over any
//! `AsyncRead`/`AsyncWrite` pair, so protocol tests drive it through an
//! in-memory duplex instead of a real child process.

use std::{
    collections::HashMap,
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
    sync::Arc,
    time::Duration,
};

use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::{oneshot, Mutex as AsyncMutex};

use base64::Engine as _;

use crate::db::Database;
use crate::tools::{ToolError, ToolOutcome};
use crate::types::ToolSpec;

/// Every MCP tool name starts with this prefix — the namespace that keeps a
/// server's tool from shadowing a bundled one (§6.5).
pub const MCP_PREFIX: &str = "mcp__";

const HANDSHAKE_TIMEOUT_MS: u64 = 15_000;
const LIST_TIMEOUT_MS: u64 = 15_000;
const CALL_TIMEOUT_MS: u64 = 120_000;

/// `mcp__<server>__<tool>` → (server, tool). The tool half may itself
/// contain `__`; the first split wins. Non-MCP names → None.
pub fn parse_ref(name: &str) -> Option<(String, String)> {
    let rest = name.strip_prefix(MCP_PREFIX)?;
    let (server, tool) = rest.split_once("__")?;
    if server.is_empty() || tool.is_empty() {
        return None;
    }
    Some((server.to_string(), tool.to_string()))
}

pub fn ref_name(server: &str, tool: &str) -> String {
    format!("{MCP_PREFIX}{server}__{tool}")
}

/// The server-name part of a tool name: lowercased ASCII alphanumerics,
/// everything else collapsed to single dashes (names stay unambiguous in
/// `mcp__<name>__<tool>` and never carry `__`).
pub fn sanitize_server_name(name: &str) -> String {
    let clean: String = name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' })
        .collect();
    let mut out = String::with_capacity(clean.len());
    let mut pending_dash = true; // also trims a leading dash
    for ch in clean.chars() {
        if ch == '-' {
            if !pending_dash {
                out.push('-');
                pending_dash = true;
            }
        } else {
            out.push(ch);
            pending_dash = false;
        }
    }
    out.trim_end_matches('-').to_string()
}

/// One connected MCP server: the JSON-RPC transport plus its discovered
/// tools. Lives behind the manager for the app's whole run; a dead session
/// (child exited) is respawned on next use.
pub struct McpSession {
    next_id: AtomicU64,
    pending: Arc<AsyncMutex<HashMap<u64, oneshot::Sender<Result<serde_json::Value, String>>>>>,
    writer: AsyncMutex<Box<dyn AsyncWrite + Send + Unpin>>,
    /// Raw tools from the last `tools/list` (un-namespaced).
    pub tools: AsyncMutex<Vec<ToolSpec>>,
    dead: Arc<AtomicBool>,
    /// Windows job object handle holding the child — closing it (i.e. Ternion
    /// exiting) kills the server, so no MCP child outlives the app.
    #[cfg(windows)]
    _job: std::sync::atomic::AtomicIsize,
}

impl McpSession {
    /// Wire a session to any byte stream pair — the test seam. A reader task
    /// matches responses to parked requests by id; server→client
    /// notifications and requests are ignored (v1).
    pub fn start<R, W>(reader: R, writer: W) -> Arc<McpSession>
    where
        R: AsyncRead + Send + Unpin + 'static,
        W: AsyncWrite + Send + Unpin + 'static,
    {
        let pending: Arc<AsyncMutex<HashMap<u64, oneshot::Sender<Result<serde_json::Value, String>>>>> =
            Arc::new(AsyncMutex::new(HashMap::new()));
        let dead = Arc::new(AtomicBool::new(false));

        let reader_pending = pending.clone();
        let reader_dead = dead.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(reader).lines();
            loop {
                let Ok(Some(line)) = lines.next_line().await else {
                    break; // EOF / error — the child is gone
                };
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }
                let Ok(msg) = serde_json::from_str::<serde_json::Value>(line) else {
                    continue;
                };
                let Some(id) = msg.get("id").and_then(|v| v.as_u64()) else {
                    continue; // server notification (or id-less) — ignored
                };
                let reply = if let Some(err) = msg.get("error") {
                    Err(err
                        .get("message")
                        .and_then(|v| v.as_str())
                        .unwrap_or("server error")
                        .to_string())
                } else {
                    Ok(msg.get("result").cloned().unwrap_or(serde_json::Value::Null))
                };
                if let Some(tx) = reader_pending.lock().await.remove(&id) {
                    let _ = tx.send(reply);
                }
            }
            reader_dead.store(true, Ordering::SeqCst);
            let mut map = reader_pending.lock().await;
            for (_, tx) in map.drain() {
                let _ = tx.send(Err("MCP server exited before responding".into()));
            }
        });

        Arc::new(Self {
            next_id: AtomicU64::new(0),
            pending,
            writer: AsyncMutex::new(Box::new(writer)),
            tools: AsyncMutex::new(Vec::new()),
            dead,
            #[cfg(windows)]
            _job: std::sync::atomic::AtomicIsize::new(0),
        })
    }

    pub fn is_dead(&self) -> bool {
        self.dead.load(Ordering::SeqCst)
    }

    /// One JSON-RPC request/response round trip with a timeout.
    pub async fn request(
        &self,
        method: &str,
        params: serde_json::Value,
        timeout_ms: u64,
    ) -> Result<serde_json::Value, String> {
        if self.is_dead() {
            return Err("MCP server is not running".into());
        }
        let id = self.next_id.fetch_add(1, Ordering::SeqCst) + 1;
        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(id, tx);

        let line = serde_json::json!({
            "jsonrpc": "2.0", "id": id, "method": method, "params": params
        });
        let mut payload = line.to_string();
        payload.push('\n');
        let write = async {
            let mut w = self.writer.lock().await;
            w.write_all(payload.as_bytes()).await?;
            w.flush().await
        }
        .await;
        if let Err(e) = write {
            self.pending.lock().await.remove(&id);
            return Err(format!("write to MCP server failed: {e}"));
        }

        match tokio::time::timeout(Duration::from_millis(timeout_ms), rx).await {
            Err(_) => {
                self.pending.lock().await.remove(&id);
                Err(format!("{method} timed out after {timeout_ms} ms"))
            }
            Ok(Err(_)) => Err("MCP server dropped the response".into()),
            Ok(Ok(Err(e))) => Err(e),
            Ok(Ok(Ok(v))) => Ok(v),
        }
    }

    /// initialize handshake + tools/list (fills `tools`).
    pub async fn handshake(&self) -> Result<(), String> {
        self.request(
            "initialize",
            serde_json::json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": {
                    "name": "Ternion",
                    "version": env!("CARGO_PKG_VERSION")
                }
            }),
            HANDSHAKE_TIMEOUT_MS,
        )
        .await
        .map_err(|e| format!("initialize: {e}"))?;
        let _ = self.notify("notifications/initialized").await;
        self.load_tools().await
    }

    pub async fn notify(&self, method: &str) -> Result<(), String> {
        let line = serde_json::json!({"jsonrpc": "2.0", "method": method, "params": {}});
        let mut payload = line.to_string();
        payload.push('\n');
        let mut w = self.writer.lock().await;
        w.write_all(payload.as_bytes())
            .await
            .map_err(|e| format!("write to MCP server failed: {e}"))?;
        w.flush().await.map_err(|e| format!("write to MCP server failed: {e}"))
    }

    pub async fn load_tools(&self) -> Result<(), String> {
        let result = self
            .request("tools/list", serde_json::json!({}), LIST_TIMEOUT_MS)
            .await
            .map_err(|e| format!("tools/list: {e}"))?;
        let mut specs = Vec::new();
        if let Some(arr) = result.get("tools").and_then(|v| v.as_array()) {
            for tool in arr {
                let Some(name) = tool.get("name").and_then(|v| v.as_str()) else {
                    continue;
                };
                specs.push(ToolSpec {
                    name: name.to_string(),
                    description: tool
                        .get("description")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_string(),
                    input_schema: tool
                        .get("inputSchema")
                        .cloned()
                        .unwrap_or_else(|| serde_json::json!({"type": "object"})),
                });
            }
        }
        *self.tools.lock().await = specs;
        Ok(())
    }

    /// Execute one tool. Text content blocks join with newlines; image
    /// content blocks are decoded and stored as attachments (§7.3-style);
    /// `isError` marks the outcome as an error fed back to the model.
    pub async fn call_tool(
        &self,
        tool: &str,
        args: serde_json::Value,
        attachments_dir: Option<&std::path::Path>,
    ) -> Result<ToolOutcome, ToolError> {
        let result = self
            .request(
                "tools/call",
                serde_json::json!({"name": tool, "arguments": args}),
                CALL_TIMEOUT_MS,
            )
            .await
            .map_err(|e| ToolError::Exec(format!("mcp: {e}")))?;

        let is_error = result
            .get("isError")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let mut text_parts: Vec<String> = Vec::new();
        let mut images = Vec::new();
        if let Some(parts) = result.get("content").and_then(|v| v.as_array()) {
            for part in parts {
                match part.get("type").and_then(|v| v.as_str()) {
                    Some("text") => {
                        if let Some(t) = part.get("text").and_then(|v| v.as_str()) {
                            text_parts.push(t.to_string());
                        }
                    }
                    Some("image") => {
                        let data = part.get("data").and_then(|v| v.as_str()).unwrap_or_default();
                        match base64::engine::general_purpose::STANDARD
                            .decode(data)
                            .ok()
                            .as_ref()
                            .and_then(|raw| attachments_dir.and_then(|dir| crate::img::store(dir, raw).ok()))
                        {
                            Some(img) => images.push(img),
                            None => text_parts.push("[an image result could not be stored]".into()),
                        }
                    }
                    _ => {}
                }
            }
        }
        let text = if text_parts.is_empty() {
            "(empty result)".to_string()
        } else {
            text_parts.join("\n")
        };
        Ok(if is_error {
            ToolOutcome::Err(text)
        } else if images.is_empty() {
            ToolOutcome::Ok(text)
        } else {
            ToolOutcome::WithImages { text, images }
        })
    }
}

/// Spawn the server's command line through `cmd /c` (§6.5: stdio servers run
/// as OS children on Windows; `.cmd` shims like npx need the shell). Stderr
/// is piped and drained into the log — a chatty server must not fill the
/// pipe and deadlock.
pub fn spawn_stdio_server(command: &str) -> Result<tokio::process::Child, String> {
    #[cfg(windows)]
    let mut cmd = {
        let mut c = tokio::process::Command::new("cmd");
        c.args(["/c", command]);
        c
    };
    #[cfg(not(windows))]
    let mut cmd = {
        let mut c = tokio::process::Command::new("sh");
        c.args(["-c", command]);
        c
    };
    use std::process::Stdio;
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW: no console flash
    cmd.spawn().map_err(|e| format!("spawn `{command}`: {e}"))
}

/// Finish wiring a spawned child: drain stderr, reap the exit code, hold a
/// kill-on-close job object (Windows), and handshake the MCP protocol.
pub async fn connect_child(
    mut child: tokio::process::Child,
    label: &str,
) -> Result<Arc<McpSession>, String> {
    #[cfg(windows)]
    let job = child.raw_handle().map(keep_child_in_job_object).unwrap_or(0);
    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| "MCP server has no stdin".to_string())?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "MCP server has no stdout".to_string())?;
    if let Some(stderr) = child.stderr.take() {
        let label = label.to_string();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(l)) = lines.next_line().await {
                log::debug!("mcp[{label}] stderr: {l}");
            }
        });
    }
    // Reap the exit status so the child does not linger as a zombie.
    tokio::spawn(async move {
        let _ = child.wait().await;
    });

    let session = McpSession::start(stdout, stdin);
    #[cfg(windows)]
    session
        ._job
        .store(job, std::sync::atomic::Ordering::Relaxed);
    session.handshake().await.map_err(|e| format!("mcp[{label}]: {e}"))?;
    Ok(session)
}

/// Put the child into a kill-on-close job object: on any Ternion exit
/// (graceful or crash) every spawned MCP server dies with it. The job handle
/// must stay open for the child's lifetime — closing it kills the child, so
/// the session holds it. Best effort: 0 means "not managed".
#[cfg(windows)]
fn keep_child_in_job_object(raw: std::os::windows::io::RawHandle) -> isize {
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
        SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };
    unsafe {
        let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
        if job.is_null() {
            return 0;
        }
        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        if SetInformationJobObject(
            job,
            JobObjectExtendedLimitInformation,
            &info as *const _ as *const core::ffi::c_void,
            std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        ) == 0
            || AssignProcessToJobObject(job, raw) == 0
        {
            return 0;
        }
        job as isize
    }
}

/// Registry of live MCP sessions keyed by server id, over the `mcp_servers`
/// table. Sessions spawn lazily on first use and are reused across turns.
#[derive(Default)]
pub struct McpManager {
    sessions: AsyncMutex<HashMap<String, Arc<McpSession>>>,
}

impl McpManager {
    /// Namespaced specs of every enabled server's tools. A server that fails
    /// to connect (or is still failing) is skipped — it never blocks chat.
    pub async fn enabled_specs(&self, db: &Database) -> Vec<ToolSpec> {
        let servers = db.list_mcp_servers().await.unwrap_or_default();
        let mut out = Vec::new();
        for server in servers.iter().filter(|s| s.enabled) {
            let session = match self.ensure(server).await {
                Ok(s) => s,
                Err(e) => {
                    log::warn!("mcp: skipping server `{}` — {e}", server.name);
                    continue;
                }
            };
            let ns = sanitize_server_name(&server.name);
            for tool in session.tools.lock().await.iter() {
                out.push(ToolSpec {
                    name: ref_name(&ns, &tool.name),
                    description: tool.description.clone(),
                    input_schema: tool.input_schema.clone(),
                });
            }
        }
        out
    }

    async fn ensure(&self, server: &crate::types::McpServer) -> Result<Arc<McpSession>, String> {
        {
            let map = self.sessions.lock().await;
            if let Some(existing) = map.get(&server.id) {
                if !existing.is_dead() {
                    return Ok(existing.clone());
                }
            }
        }
        let child = spawn_stdio_server(&server.command)?;
        let session = connect_child(child, &server.name).await?;
        self.sessions
            .lock()
            .await
            .insert(server.id.clone(), session.clone());
        Ok(session)
    }

    /// Execute a namespaced tool call: `mcp__<server>__<tool>`.
    pub async fn call(
        &self,
        db: &Database,
        attachments_dir: Option<&std::path::Path>,
        namespaced: &str,
        args: serde_json::Value,
    ) -> Result<ToolOutcome, ToolError> {
        let Some((server_name, tool)) = parse_ref(namespaced) else {
            return Err(ToolError::Exec("not an MCP tool name".into()));
        };
        let servers = db
            .list_mcp_servers()
            .await
            .map_err(|e| ToolError::Exec(format!("mcp: {e}")))?;
        let server = servers
            .iter()
            .find(|s| s.enabled && sanitize_server_name(&s.name) == server_name)
            .ok_or_else(|| ToolError::Exec(format!("mcp: unknown or disabled server `{server_name}`")))?;
        let session = self.ensure(server).await.map_err(ToolError::Exec)?;
        session.call_tool(&tool, args, attachments_dir).await
    }

    /// Drop cached sessions — config changed, so the next use re-spawns with
    /// the new command.
    pub async fn invalidate(&self, id: Option<&str>) {
        let mut map = self.sessions.lock().await;
        match id {
            Some(id) => {
                map.remove(id);
            }
            None => map.clear(),
        }
    }

    /// One-shot connect for the Settings test button — not cached.
    pub async fn test_connect(command: &str) -> Result<(u64, Vec<String>), String> {
        let started = std::time::Instant::now();
        let child = spawn_stdio_server(command)?;
        let session = connect_child(child, "test").await?;
        let tools = session.tools.lock().await.clone();
        let latency_ms = started.elapsed().as_millis() as u64;
        Ok((latency_ms, tools.iter().map(|t| t.name.clone()).collect()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{split, DuplexStream};

    /// A canned MCP server over one half of a duplex pipe: answers
    /// initialize, lists two tools (one with a `__` in its name), echoes
    /// tools/call arguments back as text content.
    async fn fake_server(stream: DuplexStream) {
        let (mut read, mut write) = split(stream);
        let mut reader = BufReader::new(&mut read);
        let mut line = String::new();
        loop {
            line.clear();
            if reader.read_line(&mut line).await.unwrap_or(0) == 0 {
                break;
            }
            let Ok(req) = serde_json::from_str::<serde_json::Value>(line.trim()) else {
                continue;
            };
            let method = req.get("method").and_then(|v| v.as_str()).unwrap_or("");
            let result = match method {
                "initialize" => serde_json::json!({
                    "protocolVersion": "2024-11-05",
                    "capabilities": {},
                    "serverInfo": {"name": "fake", "version": "0"}
                }),
                "tools/list" => serde_json::json!({"tools": [
                    {"name": "get_weather", "description": "Weather lookup",
                     "inputSchema": {"type": "object", "required": ["city"],
                                     "properties": {"city": {"type": "string"}}}},
                    {"name": "deep__named", "description": "double-underscore name",
                     "inputSchema": {"type": "object"}}
                ]}),
                "tools/call" => serde_json::json!({
                    "content": [{"type": "text",
                                 "text": format!("echo: {}", req["params"]["arguments"])}],
                    "isError": false
                }),
                _ => serde_json::json!({}),
            };
            let resp = serde_json::json!({"jsonrpc": "2.0", "id": req["id"], "result": result});
            let mut out = resp.to_string();
            out.push('\n');
            write.write_all(out.as_bytes()).await.unwrap();
            write.flush().await.unwrap();
        }
    }

    fn connected_session() -> Arc<McpSession> {
        let (client_half, server_half) = tokio::io::duplex(8192);
        tokio::spawn(fake_server(server_half));
        let (cread, cwrite) = split(client_half);
        McpSession::start(cread, cwrite)
    }

    #[tokio::test]
    async fn handshake_lists_namespaced_tools() {
        let session = connected_session();
        session.handshake().await.unwrap();
        let tools = session.tools.lock().await;
        assert_eq!(tools.len(), 2);
        assert_eq!(tools[0].name, "get_weather");
        assert_eq!(tools[0].description, "Weather lookup");
        assert_eq!(tools[0].input_schema["required"][0], "city");
    }

    #[tokio::test]
    async fn call_tool_returns_text_content() {
        let session = connected_session();
        session.handshake().await.unwrap();
        let outcome = session
            .call_tool("get_weather", serde_json::json!({"city": "Taipei"}), None)
            .await
            .unwrap();
        let (text, is_error) = outcome.into_parts();
        assert!(!is_error);
        assert!(text.contains("echo:"), "{text}");
        assert!(text.contains("Taipei"), "{text}");
    }

    #[tokio::test]
    async fn json_rpc_errors_surface_as_tool_errors() {
        let (client_half, server_half) = tokio::io::duplex(8192);
        let (cread, cwrite) = split(client_half);
        let session = McpSession::start(cread, cwrite);
        tokio::spawn(async move {
            // Read the first request line, reply with a JSON-RPC error.
            let mut reader = BufReader::new(server_half);
            let mut line = String::new();
            reader.read_line(&mut line).await.unwrap();
            let resp = serde_json::json!({
                "jsonrpc": "2.0", "id": 1,
                "error": {"code": -32601, "message": "method not found"}
            });
            let mut out = resp.to_string();
            out.push('\n');
            let mut server_half = reader.into_inner();
            use tokio::io::AsyncWriteExt;
            server_half.write_all(out.as_bytes()).await.unwrap();
        });
        let err = session
            .call_tool("x", serde_json::json!({}), None)
            .await
            .unwrap_err();
        assert!(err.message().contains("method not found"), "{err:?}");
    }

    #[test]
    fn parse_ref_splits_on_the_first_separator() {
        assert_eq!(
            parse_ref("mcp__github-mcp__create_issue"),
            Some(("github-mcp".into(), "create_issue".into()))
        );
        // The tool half keeps its own double underscores.
        assert_eq!(
            parse_ref("mcp__srv__deep__named"),
            Some(("srv".into(), "deep__named".into()))
        );
        assert_eq!(parse_ref("fs_read"), None);
        assert_eq!(parse_ref("mcp__"), None);
        assert_eq!(parse_ref("mcp__srv__"), None);
        assert_eq!(parse_ref("mcp"), None);
    }

    #[test]
    fn sanitize_lowercases_and_dashes() {
        assert_eq!(sanitize_server_name("GitHub MCP"), "github-mcp");
        assert_eq!(sanitize_server_name("  My -- Server  "), "my-server");
        assert_eq!(sanitize_server_name("plain"), "plain");
        assert_eq!(sanitize_server_name("a__b"), "a-b");
    }

    #[test]
    fn ref_names_round_trip() {
        let name = ref_name(&sanitize_server_name("Filesystem"), "read_file");
        assert_eq!(name, "mcp__filesystem__read_file");
        let (s, t) = parse_ref(&name).unwrap();
        assert_eq!((s.as_str(), t.as_str()), ("filesystem", "read_file"));
    }
}