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
    types::{ChatRole, ContentPart, Conversation, Message, MessageStatus, ModelRole},
};

type DbResult<T> = Result<T, rusqlite::Error>;

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
                        workspace_roots, system_prompt, archived
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
                 SET content = ?2, status = ?3, tokens_in = ?4, tokens_out = ?5,
                     latency_ms = ?6, error = ?7
                 WHERE id = ?1",
                params![id, json, status_str, tokens_in, tokens_out, latency_ms, error],
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
    })
}

fn row_to_message(row: &Row) -> rusqlite::Result<Message> {
    let role: String = row.get("role")?;
    let content: String = row.get("content")?;
    Ok(Message {
        id: row.get("id")?,
        conversation_id: row.get("conversation_id")?,
        role: parse_role(&role),
        content: parse_parts(&content),
        model_role: parse_model_role(row.get("model_role")?),
        model_id: row.get("model_id")?,
        endpoint_id: row.get("endpoint_id")?,
        tokens_in: row.get::<_, Option<i64>>("tokens_in")?.map(|v| v.max(0) as u64),
        tokens_out: row.get::<_, Option<i64>>("tokens_out")?.map(|v| v.max(0) as u64),
        latency_ms: row.get::<_, Option<i64>>("latency_ms")?.map(|v| v.max(0) as u64),
        status: parse_status(row.get("status")?),
        error: row.get("error")?,
        created_at: row.get("created_at")?,
    })
}

fn get_conversation_sync(conn: &Connection, id: &str) -> DbResult<Option<Conversation>> {
    conn.query_row(
        "SELECT id, title, created_at, updated_at, pinned_model,
                workspace_roots, system_prompt, archived
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
        "SELECT id, conversation_id, role, content, model_role, model_id, endpoint_id,
                tokens_in, tokens_out, latency_ms, status, error, created_at
         FROM messages WHERE conversation_id = ?1
         ORDER BY created_at, rowid LIMIT ?2",
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
        assert!(seeds.iter().any(|(k, v)| k == "ollama.base_url"));
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
}