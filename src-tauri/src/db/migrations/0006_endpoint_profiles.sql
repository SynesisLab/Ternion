-- M3.1: user-managed endpoint profiles (design §5.1). The built-in local
-- Ollama (ep_local_ollama) stays settings-driven; this table holds the extra
-- endpoints — remote Ollama boxes and OpenAI-compatible servers.
-- `api_key_ref` is a keyring handle only (§10.1): the secret itself lives in
-- Windows Credential Manager and never touches the database.
CREATE TABLE endpoint_profiles (
  id TEXT PRIMARY KEY,
  kind TEXT NOT NULL CHECK (kind IN ('ollama', 'openai')),
  name TEXT NOT NULL,
  base_url TEXT NOT NULL,
  api_key_ref TEXT,
  -- Extra request headers as a JSON object (proxies, org routing).
  headers TEXT,
  enabled INTEGER NOT NULL DEFAULT 1,
  notes TEXT,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);