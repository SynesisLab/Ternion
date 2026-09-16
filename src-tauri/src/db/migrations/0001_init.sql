-- Ternion schema: design doc §8.2 verbatim, plus documented additive pieces:
--   * messages.status / messages.error   (streaming lifecycle, M0)
--   * ON DELETE CASCADE on the message FK so conversation deletes are atomic
--   * idx_conversations_updated for sidebar ordering
-- Applied atomically by migrations.rs inside BEGIN/COMMIT.

CREATE TABLE conversations (
  id TEXT PRIMARY KEY, title TEXT, created_at INTEGER, updated_at INTEGER,
  pinned_model TEXT,
  workspace_roots TEXT,
  system_prompt TEXT, archived INTEGER DEFAULT 0, digest TEXT
);

CREATE TABLE messages (
  id TEXT PRIMARY KEY, conversation_id TEXT REFERENCES conversations(id) ON DELETE CASCADE,
  role TEXT, content TEXT,
  model_role TEXT, model_id TEXT, endpoint_id TEXT,
  tokens_in INTEGER, tokens_out INTEGER, latency_ms INTEGER,
  routing_event_id TEXT, created_at INTEGER,
  status TEXT NOT NULL DEFAULT 'complete',
  error TEXT
);
CREATE INDEX idx_messages_conversation ON messages (conversation_id, created_at);

CREATE TABLE attachments (
  id TEXT PRIMARY KEY, message_id TEXT REFERENCES messages(id) ON DELETE CASCADE, kind TEXT, path TEXT,
  processed_path TEXT, mime TEXT, width INTEGER, height INTEGER,
  bytes INTEGER, sha256 TEXT
);

CREATE TABLE endpoints (
  id TEXT PRIMARY KEY, kind TEXT, name TEXT, base_url TEXT,
  api_key_ref TEXT, headers TEXT, enabled INTEGER, notes TEXT
);

CREATE TABLE models (
  endpoint_id TEXT, model TEXT, display_name TEXT,
  capabilities TEXT, context_tokens INTEGER, role TEXT,
  vram_estimate_gb REAL, verified_at INTEGER,
  PRIMARY KEY (endpoint_id, model)
);

CREATE TABLE routing_events (
  id TEXT PRIMARY KEY, ts INTEGER, conversation_id TEXT, message_id TEXT,
  decision TEXT, final_target TEXT, actual_model TEXT,
  latency_ms INTEGER, override_kind TEXT
);

CREATE TABLE tool_calls (
  id TEXT PRIMARY KEY, message_id TEXT, tool TEXT, args TEXT,
  result TEXT, status TEXT, permission_mode TEXT, created_at INTEGER
);

CREATE TABLE permissions (
  scope TEXT, tool TEXT, workspace TEXT, mode TEXT, updated_at INTEGER,
  PRIMARY KEY (scope, tool, workspace)
);

CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT);

CREATE INDEX idx_conversations_updated ON conversations (updated_at DESC);

INSERT INTO endpoints (id, kind, name, base_url, api_key_ref, headers, enabled, notes)
  VALUES ('ep_local_ollama', 'ollama', 'Local Ollama', 'http://127.0.0.1:11434', NULL, '{}', 1, 'Default local Ollama');

INSERT INTO settings (key, value) VALUES
  ('ollama.base_url', 'http://127.0.0.1:11434'),
  ('chat.default_model', ''),
  ('chat.temperature', '0.7'),
  ('chat.context_tokens', '8192'),
  ('chat.keep_alive', '10m'),
  ('app.close_to_tray', 'true'),
  ('ui.theme', 'dark')
  ON CONFLICT(key) DO NOTHING;