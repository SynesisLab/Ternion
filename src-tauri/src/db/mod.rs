//! SQLite persistence layer (WAL). A single `rusqlite` connection behind a
//! mutex, with every operation run in `tokio::task::spawn_blocking` so the
//! async runtime never blocks on disk I/O. The mutex is never held across an
//! `.await` — it's locked inside the blocking closure only.

pub(crate) mod migrations;

use std::{
    path::Path,
    sync::{Arc, Mutex},
};

use rusqlite::{params, Connection, Row};

use crate::{
    error::CmdError,
    types::{
        ChatRole, ContentPart, Conversation, Message, MessageRouting, MessageStatus, ModelRole,
        RoutingDecision, RoutingEvent, Target, ToolCallRow,
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
    })
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
    rows.collect()
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
}