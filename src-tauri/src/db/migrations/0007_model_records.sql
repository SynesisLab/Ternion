-- Capability records per (endpoint, model) — design §5.4/§8.2. Discovery
-- fills what it can; user edits (or an /api/show verify) win per field and
-- survive discovery gaps. `role` is schema forward-compat only in v1.
CREATE TABLE IF NOT EXISTS models (
  endpoint_id TEXT NOT NULL,
  model TEXT NOT NULL,
  capabilities TEXT,            -- JSON array of tags: vision/tools/thinking/…
  context_tokens INTEGER,
  role TEXT,
  vram_estimate_gb REAL,
  verified_at INTEGER,
  PRIMARY KEY (endpoint_id, model)
);