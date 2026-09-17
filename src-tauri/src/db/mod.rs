//! SQLite persistence layer (WAL). A single `rusqlite` connection behind a
//! mutex, with every operation run in `tokio::task::spawn_blocking` so the
//! async runtime never blocks on disk I/O. The mutex is never held across an
//! `.await` — it's locked inside the blocking closure only.

pub(crate) mod migrations;

use std::{
    collections::HashMap,
    path::Path,
    sync::{Arc, Mutex},
};

use rusqlite::{params, Connection, OptionalExtension, Row};

use crate::{
    error::CmdError,
    types::{
        Attachment, ChatRole, ContentPart, Conversation, DecisionSource, EndpointProfile,
        Message, MessageRouting, MessageStatus, McpServer, ModelRecord, ModelRole, OwuiTool,
        RoleStat, RoutingDecision, RoutingEvent, Target, ToolCallRow, TriadReport,
    },
    ids,
};

type DbResult<T> = Result<T, rusqlite::Error>;

/// Cloneable: the inner connection sits behind an `Arc` — sidecar tasks
/// (Herald titles, digests, suggestions) own their own handle.
#[derive(Clone)]
pub struct Database {
    conn: Arc<Mutex<Connection>>,
}

impl Database {
    pub fn open(path: &Path) -> DbResult<Self> {
        let conn = Connection::open(path)?;
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;
             PRAGMA foreign_keys = ON;
             PRAGMA busy_timeout = 5000;",
        )?;
        migrations::run(&conn)?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    /// Run an operation on the connection inside spawn_blocking.
    async fn run<T, F>(&self, op: F) -> Result<T, CmdError>
    where
        F: FnOnce(&Connection) -> DbResult<T> + Send + 'static,
        T: Send + 'static,
    {
        let conn = Arc::clone(&self.conn);
        tokio::task::spawn_blocking(move || {
            let guard = conn.lock().unwrap_or_else(|p| p.into_inner());
            op(&guard)
        })
        .await
        .map_err(|e| CmdError::internal(format!("database task failed: {e}")))?
        .map_err(CmdError::from)
    }

    /// Direct (sync) access for startup paths that run before the runtime is
    /// serving commands — no contention at that point.
    pub fn with_conn_sync<T>(&self, f: impl FnOnce(&Connection) -> DbResult<T>) -> DbResult<T> {
        let guard = self.conn.lock().unwrap_or_else(|p| p.into_inner());
        f(&guard)
    }

    // -- settings ----------------------------------------------------------

    pub async fn get_all_settings(&self) -> Result<Vec<(String, String)>, CmdError> {
        self.run(|c| {
            let mut stmt = c.prepare("SELECT key, value FROM settings")?;
            let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
            rows.collect()
        })
        .await
    }

    pub async fn set_setting(&self, key: &str, value: &str) -> Result<(), CmdError> {
        let key = key.to_string();
        let value = value.to_string();
        self.run(move |c| {
            c.execute(
                "INSERT INTO settings (key, value) VALUES (?1, ?2)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                params![key, value],
            )
            .map(|_| ())
        })
        .await
    }

    /// Adaptive-tuning rows (§3.11) are one setting per flag class — a reset
    /// clears each one rather than a single named key.
    pub async fn delete_settings_with_prefix(&self, prefix: String) -> Result<usize, CmdError> {
        self.run(move |c| {
            c.execute(
                "DELETE FROM settings WHERE key LIKE ?1 || '%'",
                params![prefix],
            )
        })
        .await
    }

    // -- endpoint profiles (M3, design §5.1) --------------------------------

    /// User profiles, oldest first (the synthesized built-in is prepended by
    /// the command layer).
    pub async fn list_endpoint_profiles(&self) -> Result<Vec<EndpointProfile>, CmdError> {
        self.run(|c| {
            let mut stmt = c.prepare(
                "SELECT id, kind, name, base_url, api_key_ref, headers, enabled, notes
                 FROM endpoint_profiles ORDER BY created_at, rowid",
            )?;
            let rows = stmt.query_map([], |r| row_to_profile(r))?;
            rows.collect()
        })
        .await
    }

    pub async fn get_endpoint_profile(
        &self,
        id: String,
    ) -> Result<Option<EndpointProfile>, CmdError> {
        self.run(move |c| {
            let profile = c
                .query_row(
                    "SELECT id, kind, name, base_url, api_key_ref, headers, enabled, notes
                     FROM endpoint_profiles WHERE id = ?1",
                    params![id],
                    |r| row_to_profile(r),
                )
                .optional()?;
            Ok(profile)
        })
        .await
    }

    /// Upsert on `id`; the command layer owns validation and timestamps.
    pub async fn upsert_endpoint_profile(
        &self,
        profile: EndpointProfile,
        now: i64,
    ) -> Result<(), CmdError> {
        let headers = serde_json::to_string(&profile.headers)
            .map_err(|e| CmdError::internal(format!("serialize headers: {e}")))?;
        self.run(move |c| {
            c.execute(
                "INSERT INTO endpoint_profiles
                    (id, kind, name, base_url, api_key_ref, headers, enabled, notes,
                     created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?9)
                 ON CONFLICT(id) DO UPDATE SET
                    kind = excluded.kind, name = excluded.name,
                    base_url = excluded.base_url, api_key_ref = excluded.api_key_ref,
                    headers = excluded.headers, enabled = excluded.enabled,
                    notes = excluded.notes, updated_at = excluded.updated_at",
                params![
                    profile.id,
                    profile.kind,
                    profile.name,
                    profile.base_url,
                    profile.api_key_ref,
                    headers,
                    profile.enabled as i64,
                    profile.notes,
                    now,
                ],
            )
            .map(|_| ())
        })
        .await
    }

    pub async fn delete_endpoint_profile(&self, id: String) -> Result<bool, CmdError> {
        self.run(move |c| {
            let changed = c
                .execute("DELETE FROM endpoint_profiles WHERE id = ?1", params![id])?;
            Ok(changed > 0)
        })
        .await
    }

    // -- MCP servers (§6.5) ---------------------------------------------------

    /// Configured MCP stdio servers, oldest first.
    pub async fn list_mcp_servers(&self) -> Result<Vec<McpServer>, CmdError> {
        self.run(|c| {
            let mut stmt = c.prepare(
                "SELECT id, name, command, enabled FROM mcp_servers ORDER BY created_at, rowid",
            )?;
            let rows = stmt.query_map([], |r| {
                Ok(McpServer {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    command: r.get(2)?,
                    enabled: r.get::<_, i64>(3)? != 0,
                })
            })?;
            rows.collect()
        })
        .await
    }

    /// Upsert on `id`; the command layer owns validation and timestamps.
    pub async fn upsert_mcp_server(&self, server: McpServer, now: i64) -> Result<(), CmdError> {
        self.run(move |c| {
            c.execute(
                "INSERT INTO mcp_servers (id, name, command, enabled, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?5)
                 ON CONFLICT(id) DO UPDATE SET
                    name = excluded.name, command = excluded.command,
                    enabled = excluded.enabled, updated_at = excluded.updated_at",
                params![server.id, server.name, server.command, server.enabled as i64, now],
            )
            .map(|_| ())
        })
        .await
    }

    pub async fn delete_mcp_server(&self, id: String) -> Result<bool, CmdError> {
        self.run(move |c| {
            let changed = c.execute("DELETE FROM mcp_servers WHERE id = ?1", params![id])?;
            Ok(changed > 0)
        })
        .await
    }

    // -- OpenWebUI tools (§6.5b) ----------------------------------------------

    /// Stored OpenWebUI manifests, oldest first.
    pub async fn list_owui_tools(&self) -> Result<Vec<OwuiTool>, CmdError> {
        self.run(|c| {
            let mut stmt = c.prepare(
                "SELECT id, name, source, enabled, kind FROM owui_tools ORDER BY created_at, rowid",
            )?;
            let rows = stmt.query_map([], |r| {
                Ok(OwuiTool {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    source: r.get(2)?,
                    enabled: r.get::<_, i64>(3)? != 0,
                    kind: r.get::<_, String>(4)?,
                })
            })?;
            rows.collect()
        })
        .await
    }

    /// Upsert on `id`; the command layer owns validation and timestamps.
    pub async fn upsert_owui_tool(&self, tool: OwuiTool, now: i64) -> Result<(), CmdError> {
        self.run(move |c| {
            c.execute(
                "INSERT INTO owui_tools (id, name, source, enabled, kind, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)
                 ON CONFLICT(id) DO UPDATE SET
                    name = excluded.name, source = excluded.source,
                    enabled = excluded.enabled, kind = excluded.kind,
                    updated_at = excluded.updated_at",
                params![tool.id, tool.name, tool.source, tool.enabled as i64, tool.kind, now],
            )
            .map(|_| ())
        })
        .await
    }

    pub async fn delete_owui_tool(&self, id: String) -> Result<bool, CmdError> {
        self.run(move |c| {
            let changed = c.execute("DELETE FROM owui_tools WHERE id = ?1", params![id])?;
            Ok(changed > 0)
        })
        .await
    }

    // -- model records (capability registry, §5.4) ---------------------------

    pub async fn list_model_records(&self) -> Result<Vec<ModelRecord>, CmdError> {
        self.run(|c| {
            let mut stmt = c.prepare(
                "SELECT endpoint_id, model, capabilities, context_tokens,
                        role, vram_estimate_gb, verified_at
                 FROM models ORDER BY endpoint_id, model",
            )?;
            let rows = stmt.query_map([], |r| {
                Ok(ModelRecord {
                    endpoint_id: r.get("endpoint_id")?,
                    model: r.get("model")?,
                    capabilities: serde_json::from_str(
                        &r.get::<_, Option<String>>("capabilities")?.unwrap_or_default(),
                    )
                    .unwrap_or_default(),
                    context_tokens: r.get("context_tokens")?,
                    role: r.get("role")?,
                    vram_estimate_gb: r.get("vram_estimate_gb")?,
                    verified_at: r.get("verified_at")?,
                })
            })?;
            rows.collect()
        })
        .await
    }

    /// One record, for targeted lookups (compression budget, capability facts).
    pub async fn get_model_record(
        &self,
        endpoint_id: String,
        model: String,
    ) -> Result<Option<ModelRecord>, CmdError> {
        self.run(move |c| {
            let mut stmt = c.prepare(
                "SELECT endpoint_id, model, capabilities, context_tokens,
                        role, vram_estimate_gb, verified_at
                 FROM models WHERE endpoint_id = ?1 AND model = ?2",
            )?;
            let mut rows = stmt.query(params![endpoint_id, model])?;
            match rows.next()? {
                Some(r) => Ok(Some(ModelRecord {
                    endpoint_id: r.get("endpoint_id")?,
                    model: r.get("model")?,
                    capabilities: serde_json::from_str(
                        &r.get::<_, Option<String>>("capabilities")?.unwrap_or_default(),
                    )
                    .unwrap_or_default(),
                    context_tokens: r.get("context_tokens")?,
                    role: r.get("role")?,
                    vram_estimate_gb: r.get("vram_estimate_gb")?,
                    verified_at: r.get("verified_at")?,
                })),
                None => Ok(None),
            }
        })
        .await
    }

    pub async fn upsert_model_record(&self, record: ModelRecord) -> Result<(), CmdError> {
        self.run(move |c| {
            c.execute(
                "INSERT INTO models (endpoint_id, model, capabilities, context_tokens,
                                     role, vram_estimate_gb, verified_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                 ON CONFLICT(endpoint_id, model) DO UPDATE SET
                   capabilities = ?3, context_tokens = ?4, role = ?5,
                   vram_estimate_gb = ?6, verified_at = ?7",
                params![
                    record.endpoint_id,
                    record.model,
                    serde_json::to_string(&record.capabilities).ok(),
                    record.context_tokens,
                    record.role,
                    record.vram_estimate_gb,
                    record.verified_at,
                ],
            )?;
            Ok(())
        })
        .await
    }

    pub async fn delete_model_record(
        &self,
        endpoint_id: String,
        model: String,
    ) -> Result<bool, CmdError> {
        self.run(move |c| {
            let changed = c.execute(
                "DELETE FROM models WHERE endpoint_id = ?1 AND model = ?2",
                params![endpoint_id, model],
            )?;
            Ok(changed > 0)
        })
        .await
    }

    // -- attachments (§7.2/§8.2) ---------------------------------------------

    pub async fn insert_attachment(&self, att: Attachment) -> Result<(), CmdError> {
        self.run(move |c| {
            c.execute(
                "INSERT INTO attachments (id, message_id, kind, path, processed_path,
                                          mime, width, height, bytes, sha256, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                params![
                    att.id,
                    att.message_id,
                    att.kind,
                    att.path,
                    att.processed_path,
                    att.mime,
                    att.width,
                    att.height,
                    att.bytes,
                    att.sha256,
                    att.created_at,
                ],
            )?;
            Ok(())
        })
        .await
    }

    /// Re-link an existing attachment to a persisted message row.
    pub async fn link_attachment(
        &self,
        id: String,
        message_id: String,
    ) -> Result<(), CmdError> {
        self.run(move |c| {
            c.execute(
                "UPDATE attachments SET message_id = ?2 WHERE id = ?1",
                params![id, message_id],
            )?;
            Ok(())
        })
        .await
    }

    pub async fn get_attachment(&self, id: String) -> Result<Option<Attachment>, CmdError> {
        self.run(move |c| {
            let att = c
                .query_row(
                    "SELECT id, message_id, kind, path, processed_path, mime,
                            width, height, bytes, sha256, created_at
                     FROM attachments WHERE id = ?1",
                    params![id],
                    |r| row_to_attachment(r),
                )
                .optional()?;
            Ok(att)
        })
        .await
    }

    pub async fn list_message_attachments(
        &self,
        message_id: String,
    ) -> Result<Vec<Attachment>, CmdError> {
        self.run(move |c| {
            let mut stmt = c.prepare(
                "SELECT id, message_id, kind, path, processed_path, mime,
                        width, height, bytes, sha256, created_at
                 FROM attachments WHERE message_id = ?1 ORDER BY created_at, rowid",
            )?;
            let rows = stmt.query_map(params![message_id], |r| row_to_attachment(r))?;
            rows.collect()
        })
        .await
    }

    // -- conversations -----------------------------------------------------

    pub async fn list_conversations(&self) -> Result<Vec<Conversation>, CmdError> {
        self.run(|c| {
            let mut stmt = c.prepare(
                "SELECT id, title, created_at, updated_at, pinned_model,
                        workspace_roots, system_prompt, archived, digest
                 FROM conversations WHERE archived = 0
                 ORDER BY updated_at DESC, rowid DESC",
            )?;
            let rows = stmt.query_map([], |r| row_to_conversation(r))?;
            rows.collect()
        })
        .await
    }

    pub async fn create_conversation(&self, id: String, now: i64) -> Result<Conversation, CmdError> {
        self.run(move |c| {
            c.execute(
                "INSERT INTO conversations (id, title, created_at, updated_at)
                 VALUES (?1, NULL, ?2, ?2)",
                params![id, now],
            )?;
            match get_conversation_sync(c, &id) {
                Ok(Some(conv)) => Ok(conv),
                Ok(None) => Err(rusqlite::Error::QueryReturnedNoRows),
                Err(e) => Err(e),
            }
        })
        .await
    }

    pub async fn get_conversation(&self, id: String) -> Result<Option<Conversation>, CmdError> {
        self.run(move |c| get_conversation_sync(c, &id)).await
    }

    pub async fn rename_conversation(&self, id: String, title: String) -> Result<(), CmdError> {
        self.run(move |c| {
            c.execute(
                "UPDATE conversations SET title = ?2 WHERE id = ?1",
                params![id, title],
            )
            .map(|_| ())
        })
        .await
    }

    pub async fn delete_conversation(&self, id: String) -> Result<(), CmdError> {
        self.run(move |c| {
            c.execute("DELETE FROM conversations WHERE id = ?1", params![id])
                .map(|_| ())
        })
        .await
    }

    pub async fn set_conversation_model(
        &self,
        id: String,
        model: Option<String>,
    ) -> Result<(), CmdError> {
        self.run(move |c| {
            c.execute(
                "UPDATE conversations SET pinned_model = ?2 WHERE id = ?1",
                params![id, model],
            )
            .map(|_| ())
        })
        .await
    }

    /// Bind the conversation's workspace roots (§6.3) — the caller
    /// (command layer) canonicalizes and caps the list.
    pub async fn set_conversation_workspaces(
        &self,
        id: String,
        roots: Vec<String>,
    ) -> Result<(), CmdError> {
        let json = serde_json::to_string(&roots)
            .map_err(|e| CmdError::internal(format!("serialize roots: {e}")))?;
        self.run(move |c| {
            c.execute(
                "UPDATE conversations SET workspace_roots = ?2 WHERE id = ?1",
                params![id, json],
            )
            .map(|_| ())
        })
        .await
    }

    pub async fn touch_conversation(&self, id: String, now: i64) -> Result<(), CmdError> {
        self.run(move |c| {
            c.execute(
                "UPDATE conversations SET updated_at = ?2 WHERE id = ?1",
                params![id, now],
            )
            .map(|_| ())
        })
        .await
    }

    // -- Tool permission matrix (§6.6) ------------------------------------

    /// Remembered "always" grants, newest first.
    pub async fn list_tool_permissions(&self) -> Result<Vec<(String, String, String)>, CmdError> {
        self.run(|c| {
            let mut stmt =
                c.prepare("SELECT tool, root, mode FROM tool_permissions ORDER BY tool, root")?;
            let rows = stmt
                .query_map([], |r| {
                    Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?))
                })?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
        .await
    }

    /// Upsert an "always" grant, or remove the row when `mode` is "ask".
    pub async fn set_tool_permission(
        &self,
        tool: String,
        root: String,
        mode: String,
    ) -> Result<(), CmdError> {
        self.run(move |c| {
            if mode == "ask" {
                c.execute(
                    "DELETE FROM tool_permissions WHERE tool = ?1 AND root = ?2",
                    params![tool, root],
                )
                .map(|_| ())
            } else {
                c.execute(
                    "INSERT INTO tool_permissions (tool, root, mode) VALUES (?1, ?2, ?3)
                     ON CONFLICT(tool, root) DO UPDATE SET mode = ?3",
                    params![tool, root, mode],
                )
                .map(|_| ())
            }
        })
        .await
    }

    /// The stored mode for one (tool × root), if any.
    pub async fn tool_permission(
        &self,
        tool: String,
        root: String,
    ) -> Result<Option<String>, CmdError> {
        self.run(move |c| {
            let mut stmt =
                c.prepare("SELECT mode FROM tool_permissions WHERE tool = ?1 AND root = ?2")?;
            let mode = stmt
                .query_row(params![tool, root], |r| r.get::<_, String>(0))
                .optional()?;
            Ok(mode)
        })
        .await
    }

    /// Sidecar title generation seam (Herald replaces this in M1): only sets
    /// the title when it's still NULL.
    pub async fn auto_title(&self, id: String, title: String) -> Result<(), CmdError> {
        self.run(move |c| {
            c.execute(
                "UPDATE conversations SET title = ?2 WHERE id = ?1 AND title IS NULL",
                params![id, title],
            )
            .map(|_| ())
        })
        .await
    }

    // -- messages ----------------------------------------------------------

    pub async fn get_messages(&self, conversation_id: String) -> Result<Vec<Message>, CmdError> {
        self.run(move |c| get_messages_sync(c, &conversation_id, 1000)).await
    }

    pub async fn insert_user_message(
        &self,
        id: String,
        conversation_id: String,
        content: Vec<ContentPart>,
        now: i64,
    ) -> Result<(), CmdError> {
        let json = serde_json::to_string(&content).map_err(|e| CmdError::internal(e.to_string()))?;
        self.run(move |c| {
            c.execute(
                "INSERT INTO messages (id, conversation_id, role, content, status, created_at)
                 VALUES (?1, ?2, 'user', ?3, 'complete', ?4)",
                params![id, conversation_id, json, now],
            )
            .map(|_| ())
        })
        .await
    }

    /// Assistant row created before streaming starts so a crash leaves a
    /// visible stub rather than a vanished reply.
    pub async fn insert_assistant_placeholder(
        &self,
        id: String,
        conversation_id: String,
        model_id: String,
        endpoint_id: String,
        now: i64,
    ) -> Result<(), CmdError> {
        self.run(move |c| {
            c.execute(
                "INSERT INTO messages (id, conversation_id, role, content, model_id, endpoint_id, status, created_at)
                 VALUES (?1, ?2, 'assistant', '[]', ?3, ?4, 'streaming', ?5)",
                params![id, conversation_id, model_id, endpoint_id, now],
            )
            .map(|_| ())
        })
        .await
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn update_assistant_message(
        &self,
        id: String,
        content: Vec<ContentPart>,
        reasoning: Option<String>,
        status: MessageStatus,
        tokens_in: Option<u64>,
        tokens_out: Option<u64>,
        latency_ms: Option<u64>,
        error: Option<String>,
    ) -> Result<(), CmdError> {
        let json = serde_json::to_string(&content).map_err(|e| CmdError::internal(e.to_string()))?;
        let status_str = status_str(status).to_string();
        // SQLite integers are i64 — convert before binding.
        let tokens_in = tokens_in.map(|v| v.min(i64::MAX as u64) as i64);
        let tokens_out = tokens_out.map(|v| v.min(i64::MAX as u64) as i64);
        let latency_ms = latency_ms.map(|v| v.min(i64::MAX as u64) as i64);
        self.run(move |c| {
            c.execute(
                "UPDATE messages
                 SET content = ?2, reasoning = ?3, status = ?4, tokens_in = ?5,
                     tokens_out = ?6, latency_ms = ?7, error = ?8
                 WHERE id = ?1",
                params![id, json, reasoning, status_str, tokens_in, tokens_out, latency_ms, error],
            )
            .map(|_| ())
        })
        .await
    }

    /// Startup recovery: any row left 'streaming' by a crash becomes 'error'
    /// with its last flushed content intact.
    pub async fn sweep_orphan_streams(&self) -> Result<usize, CmdError> {
        self.run(|c| {
            c.execute(
                "UPDATE messages
                 SET status = 'error', error = COALESCE(error, 'stream interrupted by shutdown')
                 WHERE status = 'streaming'",
                [],
            )
        })
        .await
    }

    // -- tool calls (M2, design §6.1) --------------------------------------

    /// Record the start of one tool call (status `running`). A crash
    /// mid-execution leaves the row visible rather than silently lost.
    pub async fn insert_tool_call(
        &self,
        id: String,
        message_id: String,
        tool: String,
        args_json: String,
        created_at: i64,
    ) -> Result<(), CmdError> {
        self.run(move |c| {
            c.execute(
                "INSERT INTO tool_calls (id, message_id, tool, args, status, created_at)
                 VALUES (?1, ?2, ?3, ?4, 'running', ?5)",
                params![id, message_id, tool, args_json, created_at],
            )
            .map(|_| ())
        })
        .await
    }

    /// Fill in the outcome once execution ends.
    pub async fn finish_tool_call(
        &self,
        id: String,
        result: String,
        status: String,
        permission_mode: Option<String>,
    ) -> Result<(), CmdError> {
        let mode = permission_mode;
        self.run(move |c| {
            c.execute(
                "UPDATE tool_calls SET result = ?2, status = ?3, permission_mode = ?4
                 WHERE id = ?1",
                params![id, result, status, mode],
            )
            .map(|_| ())
        })
        .await
    }

    /// All tool calls behind one assistant message, oldest first (DTO join
    /// uses the same ordering).
    pub async fn list_tool_calls(&self, message_id: String) -> Result<Vec<ToolCallRow>, CmdError> {
        self.run(move |c| {
            let mut stmt = c.prepare(
                "SELECT id, message_id, tool, args, result, status, permission_mode, created_at
                 FROM tool_calls WHERE message_id = ?1 ORDER BY created_at, rowid",
            )?;
            let rows = stmt
                .query_map([message_id], |row| {
                    Ok(ToolCallRow {
                        id: row.get("id")?,
                        message_id: row.get("message_id")?,
                        tool: row.get("tool")?,
                        args: row.get("args")?,
                        result: row.get("result")?,
                        status: row.get("status")?,
                        permission_mode: row.get("permission_mode")?,
                        created_at: row.get("created_at")?,
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
        .await
    }

    /// Tool calls across a whole conversation (older than `before_ms`) — the
    /// policy engine's H3 signal (≥ 2 prior tool results ⇒ Titan).
    pub async fn count_prior_tool_calls(
        &self,
        conversation_id: String,
        before_ms: i64,
    ) -> Result<i64, CmdError> {
        self.run(move |c| {
            c.query_row(
                "SELECT COUNT(*) FROM tool_calls tc
                 JOIN messages m ON m.id = tc.message_id
                 WHERE m.conversation_id = ?1 AND tc.created_at < ?2",
                params![conversation_id, before_ms],
                |r| r.get(0),
            )
        })
        .await
    }

    // -- routing events (M1, design §3.11) ---------------------------------

    /// Persist one router decision; returns the generated event id.
    pub async fn insert_routing_event(
        &self,
        id: String,
        conversation_id: String,
        message_id: String,
        decision: RoutingDecision,
        final_target: Target,
        actual_model: String,
        latency_ms: u64,
        override_kind: Option<String>,
    ) -> Result<(), CmdError> {
        let decision = serde_json::to_string(&decision)
            .map_err(|e| CmdError::internal(e.to_string()))?;
        self.run(move |c| {
            c.execute(
                "INSERT INTO routing_events
                    (id, ts, conversation_id, message_id, decision, final_target,
                     actual_model, latency_ms, override_kind)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    id,
                    ids::now_ms(),
                    conversation_id,
                    message_id,
                    decision,
                    final_target.as_str(),
                    actual_model,
                    latency_ms.min(i64::MAX as u64) as i64,
                    override_kind,
                ],
            )
            .map(|_| ())
        })
        .await
    }

    /// Latest routing events for a conversation, newest first (router log).
    pub async fn list_routing_events(
        &self,
        conversation_id: String,
        limit: i64,
    ) -> Result<Vec<RoutingEvent>, CmdError> {
        self.run(move |c| {
            let mut stmt = c.prepare(
                "SELECT id, ts, conversation_id, message_id, decision, final_target,
                        actual_model, latency_ms, override_kind
                 FROM routing_events WHERE conversation_id = ?1
                 ORDER BY ts DESC, rowid DESC LIMIT ?2",
            )?;
            let rows = stmt.query_map(params![conversation_id, limit], |r| {
                let decision: String = r.get("decision")?;
                Ok(RoutingEvent {
                    id: r.get("id")?,
                    ts: r.get("ts")?,
                    conversation_id: r.get("conversation_id")?,
                    message_id: r.get("message_id")?,
                    decision: serde_json::from_str(&decision).unwrap_or_else(|e| {
                        log::debug!("unparseable routing decision ({e})");
                        RoutingDecision::default()
                    }),
                    final_target: parse_target(r.get::<_, Option<String>>("final_target")?),
                    actual_model: r.get("actual_model")?,
                    latency_ms: r
                        .get::<_, Option<i64>>("latency_ms")?
                        .unwrap_or_default()
                        .max(0) as u64,
                    override_kind: r.get("override_kind")?,
                })
            })?;
            rows.collect()
        })
        .await
    }

    /// Settings → Triad report (§3.11): aggregate every routing event and
    /// completed assistant message. All counting happens in one blocking
    /// closure; escalation/de-escalation compares the persisted opinion
    /// (decision JSON) against the turn's final target.
    pub async fn triad_report(&self) -> Result<TriadReport, CmdError> {
        self.run(move |c| {
            let mut report = TriadReport::default();

            // -- routing events --------------------------------------------------
            let mut stmt = c.prepare(
                "SELECT decision, final_target, override_kind, latency_ms
                 FROM routing_events",
            )?;
            let rows = stmt.query_map([], |r| {
                Ok((
                    r.get::<_, Option<String>>("decision")?,
                    r.get::<_, Option<String>>("final_target")?,
                    r.get::<_, Option<String>>("override_kind")?,
                    r.get::<_, Option<i64>>("latency_ms")?,
                ))
            })?;
            let mut herald_latency_sum = 0u64;
            let mut herald_latency_n = 0u64;
            for row in rows {
                let (decision, final_target, override_kind, latency_ms) = row?;
                report.total_turns += 1;
                if override_kind.is_some() {
                    report.overrides += 1;
                }
                let parsed: Option<RoutingDecision> = decision
                    .as_deref()
                    .and_then(|s| serde_json::from_str(s).ok());
                match parsed.as_ref().map(|d| d.source) {
                    Some(DecisionSource::Herald) => report.herald_turns += 1,
                    Some(DecisionSource::Heuristic) => report.heuristic_turns += 1,
                    Some(DecisionSource::HardRule) => report.hard_rule_turns += 1,
                    _ => report.manual_turns += 1,
                }
                if parsed.as_ref().map(|d| d.source) == Some(DecisionSource::Herald) {
                    if let Some(ms) = latency_ms {
                        if ms >= 0 {
                            herald_latency_sum += ms as u64;
                            herald_latency_n += 1;
                        }
                    }
                }
                // Auto turns only: pins (override_kind set) are deliberate,
                // not routing misses.
                if override_kind.is_none() {
                    let said = parsed.as_ref().map(|d| d.target);
                    let ended = match final_target.as_deref() {
                        Some("titan") => Some(Target::Titan),
                        Some("scout") => Some(Target::Scout),
                        _ => None,
                    };
                    match (said, ended) {
                        (Some(Target::Scout), Some(Target::Titan)) => report.escalations += 1,
                        (Some(Target::Titan), Some(Target::Scout)) => report.deescalations += 1,
                        _ => {}
                    }
                }
            }
            report.avg_herald_latency_ms = if herald_latency_n > 0 {
                Some(herald_latency_sum / herald_latency_n)
            } else {
                None
            };
            // -- per-role usage ----------------------------------------------------
            let mut stmt = c.prepare(
                "SELECT model_role, COUNT(*), AVG(latency_ms),
                        SUM(COALESCE(tokens_in, 0)), SUM(COALESCE(tokens_out, 0))
                 FROM messages
                 WHERE role = 'assistant' AND status != 'streaming' AND model_role IS NOT NULL
                 GROUP BY model_role",
            )?;
            let role_rows = stmt.query_map([], |r| {
                Ok((
                    r.get::<_, Option<String>>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, Option<f64>>(2)?,
                    r.get::<_, Option<i64>>(3)?,
                    r.get::<_, Option<i64>>(4)?,
                ))
            })?;
            let mut roles: Vec<RoleStat> = Vec::new();
            for row in role_rows {
                let (role, turns, avg_latency, tokens_in, tokens_out) = row?;
                roles.push(RoleStat {
                    role: role.unwrap_or_default(),
                    turns: turns.max(0) as u64,
                    avg_latency_ms: avg_latency.map(|f| f.max(0.0) as u64),
                    tokens_in: tokens_in.unwrap_or_default().max(0) as u64,
                    tokens_out: tokens_out.unwrap_or_default().max(0) as u64,
                });
            }
            // §3.11 "estimated time saved vs always-Titan": replay every
            // completed Scout turn against the observed average Titan
            // latency (8 s fallback when Titan never ran — the estimate is
            // then labelled as such by `titan_baseline_ms = null`).
            const TITAN_BASELINE_FALLBACK_MS: i64 = 8_000;
            let titan_avg = roles
                .iter()
                .find(|r| r.role == "titan")
                .and_then(|r| r.avg_latency_ms)
                .map(|ms| ms as i64)
                .unwrap_or(TITAN_BASELINE_FALLBACK_MS);
            report.titan_baseline_ms = roles
                .iter()
                .find(|r| r.role == "titan")
                .and_then(|r| r.avg_latency_ms);
            let mut saved: i64 = 0;
            let mut stmt = c.prepare(
                "SELECT latency_ms FROM messages
                 WHERE role = 'assistant' AND status != 'streaming'
                   AND model_role = 'scout' AND latency_ms IS NOT NULL",
            )?;
            let lats = stmt.query_map([], |r| r.get::<_, Option<i64>>(0))?;
            for lat in lats {
                if let Some(ms) = lat? {
                    saved += (titan_avg - ms.max(0)).max(0);
                }
            }
            report.time_saved_ms = Some(saved.max(0) as u64);
            report.roles = roles;

            Ok(report)
        })
        .await
    }

    /// The most recent router (non-pin) decision for a conversation — the
    /// baseline the §3.11 adaptive loop compares a new pin against.
    pub async fn latest_auto_decision(
        &self,
        conversation_id: String,
    ) -> Result<Option<RoutingDecision>, CmdError> {
        self.run(move |c| {
            let decision: Option<String> = c
                .query_row(
                    "SELECT decision FROM routing_events
                     WHERE conversation_id = ?1 AND override_kind IS NULL
                     ORDER BY ts DESC, rowid DESC LIMIT 1",
                    params![conversation_id],
                    |r| r.get(0),
                )
                .optional()?;
            Ok(decision.and_then(|s| serde_json::from_str(&s).ok()))
        })
        .await
    }

    /// Attach routing outcomes to the assistant placeholder row.
    pub async fn set_message_routing(
        &self,
        message_id: String,
        model_role: Option<ModelRole>,
        model_id: String,
        endpoint_id: String,
        routing_event_id: Option<String>,
    ) -> Result<(), CmdError> {
        self.run(move |c| {
            c.execute(
                "UPDATE messages
                 SET model_role = ?2, model_id = ?3, endpoint_id = ?4, routing_event_id = ?5
                 WHERE id = ?1",
                params![
                    message_id,
                    model_role.map(|r| r.as_str()),
                    model_id,
                    endpoint_id,
                    routing_event_id,
                ],
            )
            .map(|_| ())
        })
        .await
    }

    // -- rolling digest (M1, design §3.6) -----------------------------------

    pub async fn set_conversation_digest(
        &self,
        id: String,
        digest: String,
    ) -> Result<(), CmdError> {
        self.run(move |c| {
            c.execute(
                "UPDATE conversations SET digest = ?2 WHERE id = ?1",
                params![id, digest],
            )
            .map(|_| ())
        })
        .await
    }

    pub async fn get_conversation_digest(&self, id: String) -> Result<Option<String>, CmdError> {
        self.run(move |c| {
            c.query_row(
                "SELECT digest FROM conversations WHERE id = ?1",
                params![id],
                |r| r.get::<_, Option<String>>(0),
            )
            .map(Some)
            .or_else(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => Ok(None),
                other => Err(other),
            })
        })
        .await
        .map(|opt| opt.flatten())
    }

    // -- context compression (§6.5c) -----------------------------------------

    /// The stored progressive summary: (summary text, last message id it
    /// covers). None when unset or unusable.
    pub async fn get_conversation_compression(
        &self,
        id: String,
    ) -> Result<Option<(String, String)>, CmdError> {
        self.run(move |c| {
            c.query_row(
                "SELECT compression_summary, compression_upto FROM conversations WHERE id = ?1",
                params![id],
                |r| {
                    let summary: Option<String> = r.get(0)?;
                    let upto: Option<String> = r.get(1)?;
                    Ok(match (summary, upto) {
                        (Some(s), Some(u)) if !s.trim().is_empty() && !u.trim().is_empty() => {
                            Some((s, u))
                        }
                        _ => None,
                    })
                },
            )
            .or_else(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => Ok(None),
                other => Err(other),
            })
        })
        .await
    }

    pub async fn set_conversation_compression(
        &self,
        id: String,
        summary: String,
        upto: String,
    ) -> Result<(), CmdError> {
        self.run(move |c| {
            c.execute(
                "UPDATE conversations SET compression_summary = ?2, compression_upto = ?3
                 WHERE id = ?1",
                params![id, summary, upto],
            )
            .map(|_| ())
        })
        .await
    }

    /// Herald sidecar re-title (overwrites the truncation fallback).
    pub async fn retitle_conversation(&self, id: String, title: String) -> Result<(), CmdError> {
        self.run(move |c| {
            c.execute(
                "UPDATE conversations SET title = ?2 WHERE id = ?1",
                params![id, title],
            )
            .map(|_| ())
        })
        .await
    }
}

// ---------------------------------------------------------------------------
// Row mapping + sync helpers (shared by async wrappers and tests)
// ---------------------------------------------------------------------------

fn status_str(status: MessageStatus) -> &'static str {
    match status {
        MessageStatus::Streaming => "streaming",
        MessageStatus::Complete => "complete",
        MessageStatus::Stopped => "stopped",
        MessageStatus::Error => "error",
    }
}

fn parse_role(s: &str) -> ChatRole {
    match s {
        "system" => ChatRole::System,
        "assistant" => ChatRole::Assistant,
        "tool" => ChatRole::Tool,
        _ => ChatRole::User,
    }
}

fn parse_status(s: Option<String>) -> MessageStatus {
    match s.as_deref() {
        Some("streaming") => MessageStatus::Streaming,
        Some("stopped") => MessageStatus::Stopped,
        Some("error") => MessageStatus::Error,
        _ => MessageStatus::Complete,
    }
}

fn parse_model_role(s: Option<String>) -> Option<ModelRole> {
    match s.as_deref() {
        Some("herald") => Some(ModelRole::Herald),
        Some("scout") => Some(ModelRole::Scout),
        Some("titan") => Some(ModelRole::Titan),
        _ => None,
    }
}

fn parse_target(s: Option<String>) -> Target {
    match s.as_deref() {
        Some("titan") => Target::Titan,
        _ => Target::Scout,
    }
}

fn parse_parts(json: &str) -> Vec<ContentPart> {
    serde_json::from_str(json).unwrap_or_else(|e| {
        log::debug!("unparseable message content ({e}); treating as empty");
        Vec::new()
    })
}

/// `headers` may be NULL (column added with the table but rows can be written
/// with no extra headers) and is a JSON object when present.
fn row_to_attachment(row: &Row) -> rusqlite::Result<Attachment> {
    Ok(Attachment {
        id: row.get("id")?,
        message_id: row.get("message_id")?,
        kind: row.get("kind")?,
        path: row.get("path")?,
        processed_path: row.get("processed_path")?,
        mime: row.get("mime")?,
        width: row.get::<_, Option<i64>>("width")?.unwrap_or(0) as u32,
        height: row.get::<_, Option<i64>>("height")?.unwrap_or(0) as u32,
        bytes: row.get("bytes")?,
        sha256: row.get("sha256")?,
        created_at: row.get("created_at")?,
    })
}

fn row_to_profile(row: &Row) -> rusqlite::Result<EndpointProfile> {
    let headers_json: Option<String> = row.get("headers")?;
    Ok(EndpointProfile {
        id: row.get("id")?,
        kind: row.get("kind")?,
        name: row.get("name")?,
        base_url: row.get("base_url")?,
        api_key_ref: row.get("api_key_ref")?,
        headers: headers_json
            .and_then(|json| serde_json::from_str(&json).ok())
            .unwrap_or_default(),
        enabled: row.get::<_, i64>("enabled")? != 0,
        notes: row.get("notes")?,
    })
}

fn row_to_conversation(row: &Row) -> rusqlite::Result<Conversation> {
    // workspace_roots may be NULL on rows created before any workspace binding.
    let roots_json: Option<String> = row.get("workspace_roots")?;
    Ok(Conversation {
        id: row.get("id")?,
        title: row.get("title")?,
        created_at: row.get("created_at")?,
        updated_at: row.get("updated_at")?,
        pinned_model: row.get("pinned_model")?,
        workspace_roots: roots_json
            .and_then(|json| serde_json::from_str(&json).ok())
            .unwrap_or_default(),
        system_prompt: row.get("system_prompt")?,
        archived: row.get::<_, i64>("archived")? != 0,
        digest: row.get("digest")?,
    })
}

fn row_to_message(row: &Row) -> rusqlite::Result<Message> {
    let role: String = row.get("role")?;
    let content: String = row.get("content")?;
    // Router decision joined from routing_events (§3.11); NULL when the row
    // has no routing_event_id (user messages, pre-M1 rows).
    let routing = match row.get::<_, Option<String>>("routing_decision")? {
        Some(json) => {
            let decision = serde_json::from_str(&json).unwrap_or_else(|e| {
                log::debug!("unparseable routing decision ({e})");
                RoutingDecision::default()
            });
            Some(MessageRouting {
                decision,
                final_target: parse_target(row.get("final_target")?),
                actual_model: row.get("actual_model")?,
                latency_ms: row
                    .get::<_, Option<i64>>("routing_latency_ms")?
                    .unwrap_or_default()
                    .max(0) as u64,
            })
        }
        None => None,
    };
    Ok(Message {
        id: row.get("id")?,
        conversation_id: row.get("conversation_id")?,
        role: parse_role(&role),
        content: parse_parts(&content),
        reasoning: row.get("reasoning")?,
        model_role: parse_model_role(row.get("model_role")?),
        model_id: row.get("model_id")?,
        endpoint_id: row.get("endpoint_id")?,
        tokens_in: row.get::<_, Option<i64>>("tokens_in")?.map(|v| v.max(0) as u64),
        tokens_out: row.get::<_, Option<i64>>("tokens_out")?.map(|v| v.max(0) as u64),
        latency_ms: row.get::<_, Option<i64>>("latency_ms")?.map(|v| v.max(0) as u64),
        status: parse_status(row.get("status")?),
        error: row.get("error")?,
        created_at: row.get("created_at")?,
        routing,
        // Attached by get_messages_sync (needs a second query; this closure
        // can't run one).
        tool_calls: Vec::new(),
    })
}

/// All tool rows behind one conversation's messages, oldest first, ready to
/// attach by `message_id` (§6.1).
fn tool_calls_for_conversation_sync(
    conn: &Connection,
    conversation_id: &str,
) -> DbResult<Vec<ToolCallRow>> {
    let mut stmt = conn.prepare(
        "SELECT tc.id, tc.message_id, tc.tool, tc.args, tc.result, tc.status,
                tc.permission_mode, tc.created_at
         FROM tool_calls tc
         JOIN messages m ON m.id = tc.message_id
         WHERE m.conversation_id = ?1
         ORDER BY tc.created_at, tc.rowid",
    )?;
    let rows = stmt.query_map(params![conversation_id], |r| {
        Ok(ToolCallRow {
            id: r.get("id")?,
            message_id: r.get("message_id")?,
            tool: r.get("tool")?,
            args: r.get("args")?,
            result: r.get("result")?,
            status: r.get("status")?,
            permission_mode: r.get("permission_mode")?,
            created_at: r.get("created_at")?,
        })
    })?;
    rows.collect()
}

fn get_conversation_sync(conn: &Connection, id: &str) -> DbResult<Option<Conversation>> {
    conn.query_row(
        "SELECT id, title, created_at, updated_at, pinned_model,
                workspace_roots, system_prompt, archived, digest
         FROM conversations WHERE id = ?1",
        params![id],
        |r| row_to_conversation(r),
    )
    .map(Some)
    .or_else(|e| match e {
        rusqlite::Error::QueryReturnedNoRows => Ok(None),
        other => Err(other),
    })
}

fn get_messages_sync(conn: &Connection, conversation_id: &str, limit: i64) -> DbResult<Vec<Message>> {
    let mut stmt = conn.prepare(
        "SELECT m.id, m.conversation_id, m.role, m.content, m.reasoning, m.model_role,
                m.model_id, m.endpoint_id, m.tokens_in, m.tokens_out, m.latency_ms,
                m.status, m.error, m.created_at,
                re.decision AS routing_decision, re.final_target,
                re.actual_model, re.latency_ms AS routing_latency_ms
         FROM messages m
         LEFT JOIN routing_events re ON re.id = m.routing_event_id
         WHERE m.conversation_id = ?1
         ORDER BY m.created_at, m.rowid LIMIT ?2",
    )?;
    let rows = stmt.query_map(params![conversation_id, limit], |r| row_to_message(r))?;
    let mut msgs: Vec<Message> = rows.collect::<Result<_, _>>()?;

    // Image parts (§7.2) carry attachment ids only; hydrate the processed
    // render path so the webview can draw them via the asset protocol. One
    // join query per read, matched back by id.
    let mut processed: HashMap<String, String> = HashMap::new();
    let mut att_stmt = conn.prepare(
        "SELECT a.id, a.processed_path
         FROM attachments a
         JOIN messages m ON a.message_id = m.id
         WHERE m.conversation_id = ?1",
    )?;
    let att_rows = att_stmt.query_map(params![conversation_id], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
    })?;
    for (id, path) in att_rows.collect::<Result<Vec<_>, _>>()? {
        processed.insert(id, path);
    }
    for msg in msgs.iter_mut() {
        for part in msg.content.iter_mut() {
            if let ContentPart::Image { attachment_id, processed_path, .. } = part {
                *processed_path = processed.get(attachment_id).cloned();
            }
        }
    }

    // Attach tool activity (§6.1) in one pass, preserving per-message order.
    let mut by_message: HashMap<String, Vec<ToolCallRow>> = HashMap::new();
    for call in tool_calls_for_conversation_sync(conn, conversation_id)? {
        by_message.entry(call.message_id.clone()).or_default().push(call);
    }
    for msg in msgs.iter_mut() {
        if let Some(calls) = by_message.remove(&msg.id) {
            msg.tool_calls = calls;
        }
    }
    Ok(msgs)
}

#[cfg(test)]
impl Database {
    /// Cross-module test fixture: a Database over a fresh temp file.
    pub async fn test_db() -> (tempfile::TempDir, Database) {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open(&dir.path().join("test.db")).unwrap();
        (dir, db)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn temp_db() -> (tempfile::TempDir, Database) {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open(&dir.path().join("test.db")).unwrap();
        (dir, db)
    }
    #[tokio::test]
    async fn reopen_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.db");
        Database::open(&path).unwrap();
        drop(Database::open(&path).unwrap());
        // Second open must succeed without re-applying migrations.
        let db = Database::open(&path).unwrap();
        let seeds = db.get_all_settings().await.unwrap();
        assert!(seeds.iter().any(|(k, _)| k == "ollama.base_url"));
    }

    #[tokio::test]
    async fn owui_tools_roundtrip() {
        let (_dir, db) = temp_db().await;
        let now = crate::ids::now_ms();
        assert!(db.list_owui_tools().await.unwrap().is_empty());

        let tool = crate::types::OwuiTool {
            id: "owui_t1".into(),
            name: "Weather Tools".into(),
            source: "class Tools:\n    def get_weather(self, city: str) -> str:\n        return \"sunny\"".into(),
            enabled: true,
            kind: "tools".into(),
        };
        db.upsert_owui_tool(tool, now).await.unwrap();
        // Edit on the same id updates in place.
        db.upsert_owui_tool(
            crate::types::OwuiTool {
                id: "owui_t1".into(),
                name: "Weather".into(),
                source: "class Tools:\n    pass".into(),
                enabled: false,
                kind: "filter".into(),
            },
            now + 1,
        )
        .await
        .unwrap();
        let rows = db.list_owui_tools().await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, "Weather");
        assert!(!rows[0].enabled);
        assert_eq!(rows[0].kind, "filter");
        assert!(rows[0].source.contains("pass"));

        assert!(db.delete_owui_tool("owui_t1".into()).await.unwrap());
        assert!(!db.delete_owui_tool("owui_t1".into()).await.unwrap());
        assert!(db.list_owui_tools().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn conversation_crud_and_cascade() {
        let (_dir, db) = temp_db().await;
        let now = crate::ids::now_ms();
        let conv = db.create_conversation("c1".into(), now).await.unwrap();
        assert_eq!(conv.id, "c1");
        assert!(conv.title.is_none());
        assert!(conv.workspace_roots.is_empty());

        db.insert_user_message("m1".into(), "c1".into(), vec![], now)
            .await
            .unwrap();
        db.insert_user_message("m2".into(), "c1".into(), vec![], now)
            .await
            .unwrap();

        db.rename_conversation("c1".into(), "renamed".into())
            .await
            .unwrap();
        let listed = db.list_conversations().await.unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].title.as_deref(), Some("renamed"));

        // Deleting the conversation cascades to its messages.
        db.delete_conversation("c1".into()).await.unwrap();
        assert!(db.list_conversations().await.unwrap().is_empty());
        assert!(db
            .get_messages("c1".into())
            .await
            .unwrap()
            .is_empty());
        let count: i64 = db
            .with_conn_sync(|c| {
                c.query_row("SELECT COUNT(*) FROM messages", [], |r| r.get(0))
            })
            .unwrap();
        assert_eq!(count, 0);
    }

    #[tokio::test]
    async fn message_roundtrip_with_parts() {
        let (_dir, db) = temp_db().await;
        let now = crate::ids::now_ms();
        db.create_conversation("c1".into(), now).await.unwrap();

        db.insert_user_message(
            "m1".into(),
            "c1".into(),
            vec![ContentPart::Text {
                text: "hello".into(),
            }],
            now,
        )
        .await
        .unwrap();
        db.insert_assistant_placeholder(
            "m2".into(),
            "c1".into(),
            "gemma3:4b".into(),
            "ep_local_ollama".into(),
            now,
        )
        .await
        .unwrap();
        db.update_assistant_message(
            "m2".into(),
            vec![ContentPart::Text {
                text: "hi there".into(),
            }],
            Some("thinking hard".into()),
            MessageStatus::Complete,
            Some(17),
            Some(4),
            Some(1234),
            None,
        )
        .await
        .unwrap();

        let msgs = db.get_messages("c1".into()).await.unwrap();
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].role, ChatRole::User);
        assert_eq!(msgs[0].content.len(), 1);
        assert_eq!(msgs[0].status, MessageStatus::Complete);
        assert_eq!(msgs[1].status, MessageStatus::Complete);
        assert_eq!(msgs[1].tokens_out, Some(4));
        assert_eq!(msgs[1].latency_ms, Some(1234));
        assert_eq!(msgs[1].reasoning.as_deref(), Some("thinking hard"));
        assert_eq!(msgs[1].model_id.as_deref(), Some("gemma3:4b"));
    }

    #[tokio::test]
    async fn orphan_stream_sweep() {
        let (_dir, db) = temp_db().await;
        let now = crate::ids::now_ms();
        db.create_conversation("c1".into(), now).await.unwrap();
        db.insert_assistant_placeholder(
            "m1".into(),
            "c1".into(),
            "gemma3:4b".into(),
            "ep_local_ollama".into(),
            now,
        )
        .await
        .unwrap();

        let swept = db.sweep_orphan_streams().await.unwrap();
        assert_eq!(swept, 1);
        let msgs = db.get_messages("c1".into()).await.unwrap();
        assert_eq!(msgs[0].status, MessageStatus::Error);
        assert_eq!(
            msgs[0].error.as_deref(),
            Some("stream interrupted by shutdown")
        );
        // Sweeping again is a no-op.
        assert_eq!(db.sweep_orphan_streams().await.unwrap(), 0);
    }

    #[tokio::test]
    async fn auto_title_only_fills_null() {
        let (_dir, db) = temp_db().await;
        let now = crate::ids::now_ms();
        db.create_conversation("c1".into(), now).await.unwrap();
        db.auto_title("c1".into(), "first".into()).await.unwrap();
        db.auto_title("c1".into(), "second".into()).await.unwrap();
        let conv = db.get_messages("c1".into()).await.unwrap(); // existence check only
        assert_eq!(conv.len(), 0);
        let listed = db.list_conversations().await.unwrap();
        assert_eq!(listed[0].title.as_deref(), Some("first"));
    }

    #[tokio::test]
    async fn settings_roundtrip() {
        let (_dir, db) = temp_db().await;
        db.set_setting("chat.temperature", "0.9").await.unwrap();
        db.set_setting("custom.new", "1").await.unwrap();
        let all = db.get_all_settings().await.unwrap();
        assert!(all.iter().any(|(k, v)| k == "chat.temperature" && v == "0.9"));
        assert!(all.iter().any(|(k, v)| k == "custom.new" && v == "1"));
    }

    /// §3.11 Triad report: source counts, escalation/de-escalation pairs,
    /// override count, herald latency average, per-role usage, and the
    /// always-Titan time-saved estimate (streaming rows excluded).
    #[tokio::test]
    async fn triad_report_aggregates_events_and_roles() {
        let (_dir, db) = temp_db().await;
        let now = crate::ids::now_ms();
        db.create_conversation("c1".into(), now).await.unwrap();

        let decision = |source: DecisionSource, target: Target| RoutingDecision {
            target,
            confidence: 0.8,
            complexity: 2,
            reason: "test".into(),
            source,
            ..RoutingDecision::default()
        };

        // Herald scout → titan: an escalation.
        db.insert_routing_event(
            "re1".into(), "c1".into(), "a1".into(),
            decision(DecisionSource::Herald, Target::Scout),
            Target::Titan, "m".into(), 180, None,
        ).await.unwrap();
        // Herald titan → titan: plain.
        db.insert_routing_event(
            "re2".into(), "c1".into(), "a2".into(),
            decision(DecisionSource::Herald, Target::Titan),
            Target::Titan, "m".into(), 210, None,
        ).await.unwrap();
        // Heuristic scout → scout.
        db.insert_routing_event(
            "re3".into(), "c1".into(), "a3".into(),
            decision(DecisionSource::Heuristic, Target::Scout),
            Target::Scout, "m".into(), 5, None,
        ).await.unwrap();
        // Pinned: an override, never a routing miss.
        db.insert_routing_event(
            "re4".into(), "c1".into(), "a4".into(),
            decision(DecisionSource::Manual, Target::Titan),
            Target::Titan, "m".into(), 0, Some("manual".into()),
        ).await.unwrap();
        // Herald titan → scout: a de-escalation.
        db.insert_routing_event(
            "re5".into(), "c1".into(), "a5".into(),
            decision(DecisionSource::Herald, Target::Titan),
            Target::Scout, "m".into(), 100, None,
        ).await.unwrap();

        async fn role_of(db: &Database, id: &str, role: ModelRole) {
            db.set_message_routing(id.into(), Some(role), "m".into(), "ep_local_ollama".into(), None)
                .await
                .unwrap();
        }
        async fn finish(db: &Database, id: &str, latency: u64, tokens_in: u64, tokens_out: u64) {
            db.update_assistant_message(
                id.into(), vec![], None, MessageStatus::Complete,
                Some(tokens_in), Some(tokens_out), Some(latency), None,
            )
            .await
            .unwrap();
        }
        for (id, role) in [
            ("a1", ModelRole::Scout),
            ("a2", ModelRole::Titan),
            ("a3", ModelRole::Scout),
            ("a4", ModelRole::Scout), // left streaming → excluded
        ] {
            db.insert_assistant_placeholder(
                id.into(), "c1".into(), "m".into(), "ep_local_ollama".into(), now,
            )
            .await
            .unwrap();
            role_of(&db, id, role).await;
        }
        finish(&db, "a1", 900, 100, 200).await;
        finish(&db, "a2", 9_000, 5_000, 3_000).await;
        finish(&db, "a3", 1_500, 120, 250).await;

        let report = db.triad_report().await.unwrap();
        assert_eq!(report.total_turns, 5);
        assert_eq!(report.herald_turns, 3);
        assert_eq!(report.heuristic_turns, 1);
        assert_eq!(report.manual_turns, 1);
        assert_eq!(report.overrides, 1);
        assert_eq!(report.escalations, 1);
        assert_eq!(report.deescalations, 1);
        assert_eq!(report.avg_herald_latency_ms, Some((180 + 210 + 100) / 3));

        let scout = report.roles.iter().find(|r| r.role == "scout").unwrap();
        assert_eq!(scout.turns, 2, "streaming row excluded");
        assert_eq!(scout.tokens_in, 220);
        assert_eq!(scout.tokens_out, 450);
        let titan = report.roles.iter().find(|r| r.role == "titan").unwrap();
        assert_eq!(titan.avg_latency_ms, Some(9_000));

        // Baseline is the observed titan average; both scout turns saved
        // against it.
        assert_eq!(report.titan_baseline_ms, Some(9_000));
        assert_eq!(report.time_saved_ms, Some((9_000 - 900) + (9_000 - 1_500)));
    }

    /// The message DTO carries its routing outcome (§9.2 ribbon) — joined
    /// from routing_events via routing_event_id, None without one.
    #[tokio::test]
    async fn get_messages_joins_routing_event() {
        let (_dir, db) = temp_db().await;
        let now = crate::ids::now_ms();
        db.create_conversation("c1".into(), now).await.unwrap();
        db.insert_user_message("u1".into(), "c1".into(), vec![], now)
            .await
            .unwrap();
        db.insert_assistant_placeholder(
            "a1".into(),
            "c1".into(),
            "gemma3:12b".into(),
            "ep_local_ollama".into(),
            now,
        )
        .await
        .unwrap();

        let decision = RoutingDecision {
            target: Target::Titan,
            confidence: 0.91,
            complexity: 4,
            reason: "multi-file refactor".into(),
            ..RoutingDecision::default()
        };
        db.insert_routing_event(
            "re1".into(),
            "c1".into(),
            "a1".into(),
            decision.clone(),
            Target::Titan,
            "gemma3:12b".into(),
            4210,
            None,
        )
        .await
        .unwrap();
        db.set_message_routing(
            "a1".into(),
            Some(ModelRole::Titan),
            "gemma3:12b".into(),
            "ep_local_ollama".into(),
            Some("re1".into()),
        )
        .await
        .unwrap();

        let msgs = db.get_messages("c1".into()).await.unwrap();
        assert!(msgs[0].routing.is_none(), "user message has no routing");
        let routing = msgs[1].routing.as_ref().unwrap();
        assert_eq!(routing.decision.target, Target::Titan);
        assert!((routing.decision.confidence - 0.91).abs() < 1e-3);
        assert_eq!(routing.decision.reason, "multi-file refactor");
        assert_eq!(routing.final_target, Target::Titan);
        assert_eq!(routing.actual_model, "gemma3:12b");
        assert_eq!(routing.latency_ms, 4210);

        // The router log reads the same rows, newest first.
        let events = db.list_routing_events("c1".into(), 10).await.unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].message_id, "a1");
    }

    /// Tool rows join onto their assistant message, in creation order (§6.1).
    #[tokio::test]
    async fn get_messages_joins_tool_calls() {
        let (_dir, db) = temp_db().await;
        let now = crate::ids::now_ms();
        db.create_conversation("c1".into(), now).await.unwrap();
        db.insert_user_message("u1".into(), "c1".into(), vec![], now)
            .await
            .unwrap();
        db.insert_assistant_placeholder(
            "a1".into(),
            "c1".into(),
            "scout".into(),
            "ep_local_ollama".into(),
            now,
        )
        .await
        .unwrap();

        db.insert_tool_call("tc1".into(), "a1".into(), "echo".into(), "{}".into(), now)
            .await
            .unwrap();
        db.insert_tool_call("tc2".into(), "a1".into(), "echo".into(), "{}".into(), now + 1)
            .await
            .unwrap();
        db.finish_tool_call(
            "tc2".into(),
            "second".into(),
            "ok".into(),
            Some("auto".into()),
        )
        .await
        .unwrap();
        db.finish_tool_call(
            "tc1".into(),
            "bad args".into(),
            "error".into(),
            Some("auto".into()),
        )
        .await
        .unwrap();

        // A second conversation's rows must not leak into this page.
        db.create_conversation("c2".into(), now).await.unwrap();
        db.insert_assistant_placeholder(
            "a2".into(),
            "c2".into(),
            "scout".into(),
            "ep_local_ollama".into(),
            now,
        )
        .await
        .unwrap();
        db.insert_tool_call("tc3".into(), "a2".into(), "echo".into(), "{}".into(), now)
            .await
            .unwrap();

        let msgs = db.get_messages("c1".into()).await.unwrap();
        assert!(msgs[0].tool_calls.is_empty(), "user message carries no tools");
        assert_eq!(msgs[1].tool_calls.len(), 2);
        assert_eq!(msgs[1].tool_calls[0].id, "tc1");
        assert_eq!(msgs[1].tool_calls[0].status, "error");
        assert_eq!(msgs[1].tool_calls[0].result.as_deref(), Some("bad args"));
        assert_eq!(msgs[1].tool_calls[1].id, "tc2");
        assert_eq!(msgs[1].tool_calls[1].status, "ok");
        assert_eq!(
            msgs[1].tool_calls[1].permission_mode.as_deref(),
            Some("auto")
        );
    }

    /// The rolling digest lives on the conversation row and rides out in the
    /// DTO for the router log drawer.
    #[tokio::test]
    async fn conversation_digest_roundtrip() {
        let (_dir, db) = temp_db().await;
        let now = crate::ids::now_ms();
        let conv = db.create_conversation("c1".into(), now).await.unwrap();
        assert!(conv.digest.is_none());

        db.set_conversation_digest("c1".into(), "task: migrate auth".into())
            .await
            .unwrap();
        let conv = db.get_conversation("c1".into()).await.unwrap().unwrap();
        assert_eq!(conv.digest.as_deref(), Some("task: migrate auth"));
        let digest = db.get_conversation_digest("c1".into()).await.unwrap();
        assert_eq!(digest.as_deref(), Some("task: migrate auth"));

        let listed = db.list_conversations().await.unwrap();
        assert_eq!(listed[0].digest.as_deref(), Some("task: migrate auth"));
    }

    /// Attachments (§7.2/§8.2): insert → get, link to a message, list by
    /// message in creation order.
    #[tokio::test]
    async fn attachments_roundtrip_and_link() {
        let (_dir, db) = temp_db().await;

        let att = Attachment {
            id: "att_1".into(),
            message_id: None,
            kind: "image".into(),
            path: "C:\\att\\abc.png".into(),
            processed_path: "C:\\att\\abc_processed.jpg".into(),
            mime: "image/png".into(),
            width: 100,
            height: 50,
            bytes: 1234,
            sha256: "abc".into(),
            created_at: 1000,
        };
        db.insert_attachment(att.clone()).await.unwrap();

        // Unlinked: get works, no message rows.
        let got = db.get_attachment("att_1".into()).await.unwrap().unwrap();
        assert_eq!(got, att);
        assert!(db
            .list_message_attachments("c1".into())
            .await
            .unwrap()
            .is_empty());

        // Link at send time; the message list picks it up. The FK needs a
        // real message row, so create one (message_id → messages).
        db.create_conversation("c1".into(), 500).await.unwrap();
        db.insert_user_message("m9".into(), "c1".into(), Vec::new(), 600)
            .await
            .unwrap();
        db.link_attachment("att_1".into(), "m9".into()).await.unwrap();
        let listed = db.list_message_attachments("m9".into()).await.unwrap();
        assert_eq!(listed[0].message_id.as_deref(), Some("m9"));

        // Unknown ids stay None.
        assert!(db.get_attachment("nope".into()).await.unwrap().is_none());
    }

    /// Capability records (§5.4): upsert overwrites per (endpoint, model),
    /// listing is ordered, delete reports whether anything was removed.
    #[tokio::test]
    async fn model_records_roundtrip() {
        let (_dir, db) = temp_db().await;

        let record = ModelRecord {
            endpoint_id: "ep_local_ollama".into(),
            model: "qwen2.5vl:7b".into(),
            capabilities: vec!["vision".into(), "tools".into()],
            context_tokens: Some(32768),
            role: None,
            vram_estimate_gb: Some(6.5),
            verified_at: Some(1000),
        };
        db.upsert_model_record(record.clone()).await.unwrap();

        let listed = db.list_model_records().await.unwrap();
        assert_eq!(listed, vec![record.clone()]);

        // Same key overwrites; empty capabilities serialize as a JSON array.
        let mut updated = record.clone();
        updated.capabilities = Vec::new();
        updated.context_tokens = None;
        updated.verified_at = Some(2000);
        db.upsert_model_record(updated.clone()).await.unwrap();
        let listed = db.list_model_records().await.unwrap();
        assert_eq!(listed, vec![updated.clone()]);

        // A second endpoint key coexists; delete only removes its own row.
        db.upsert_model_record(ModelRecord {
            endpoint_id: "ep_remote".into(),
            model: "gpt-4o".into(),
            ..Default::default()
        })
        .await
        .unwrap();
        assert_eq!(db.list_model_records().await.unwrap().len(), 2);
        assert!(db
            .delete_model_record("ep_remote".into(), "gpt-4o".into())
            .await
            .unwrap());
        assert!(!db
            .delete_model_record("ep_remote".into(), "gpt-4o".into())
            .await
            .unwrap());
        assert_eq!(db.list_model_records().await.unwrap(), vec![updated]);
    }

    /// Endpoint profiles (§5.1): full-field round-trip through upsert/get,
    /// oldest-first listing, and delete reporting.
    #[tokio::test]
    async fn endpoint_profiles_roundtrip() {
        let (_dir, db) = temp_db().await;

        let mut headers = std::collections::HashMap::new();
        headers.insert("X-Org".to_string(), "acme".to_string());
        let profile = EndpointProfile {
            id: "ep_1".into(),
            kind: "openai".into(),
            name: "Homelab LM Studio".into(),
            base_url: "http://192.168.1.20:1234".into(),
            api_key_ref: Some("endpoint.ep_1".into()),
            headers: headers.clone(),
            enabled: true,
            notes: Some("RTX 4090".into()),
        };
        db.upsert_endpoint_profile(profile.clone(), 1000).await.unwrap();

        let got = db.get_endpoint_profile("ep_1".into()).await.unwrap().unwrap();
        assert_eq!(got, profile);
        assert!(db.get_endpoint_profile("missing".into()).await.unwrap().is_none());

        // Second profile; list is oldest-first by creation.
        let second = EndpointProfile {
            id: "ep_2".into(),
            kind: "ollama".into(),
            name: "LAN Ollama".into(),
            base_url: "http://192.168.1.30:11434".into(),
            api_key_ref: None,
            headers: Default::default(),
            enabled: false,
            notes: None,
        };
        db.upsert_endpoint_profile(second.clone(), 1001).await.unwrap();
        let listed = db.list_endpoint_profiles().await.unwrap();
        assert_eq!(listed, vec![profile, second]);

        // Updating the same id keeps one row (and preserves created order).
        let mut updated = db.get_endpoint_profile("ep_1".into()).await.unwrap().unwrap();
        updated.name = "Renamed".into();
        updated.enabled = false;
        db.upsert_endpoint_profile(updated, 1002).await.unwrap();
        let listed = db.list_endpoint_profiles().await.unwrap();
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[0].name, "Renamed");
        assert!(!listed[0].enabled);
        assert_eq!(listed[0].headers.get("X-Org").map(String::as_str), Some("acme"));

        // Delete reports existence; a second delete is a no-op.
        assert!(db.delete_endpoint_profile("ep_1".into()).await.unwrap());
        assert!(!db.delete_endpoint_profile("ep_1".into()).await.unwrap());
        assert_eq!(db.list_endpoint_profiles().await.unwrap().len(), 1);
    }
}