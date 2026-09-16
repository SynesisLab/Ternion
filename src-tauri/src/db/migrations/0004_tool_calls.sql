-- 0001_init already created `tool_calls` (design §8.2 verbatim) but without
-- the ON DELETE CASCADE on message_id or the message index — deleting a
-- conversation would strand orphaned tool rows. SQLite can't add an FK in
-- place, so rebuild the table (preserving any rows) inside the transaction.
CREATE TABLE tool_calls_new (
  id TEXT PRIMARY KEY,
  message_id TEXT REFERENCES messages(id) ON DELETE CASCADE,
  tool TEXT NOT NULL,
  args TEXT,
  result TEXT,
  status TEXT NOT NULL,          -- running | ok | error | denied
  permission_mode TEXT,
  created_at INTEGER NOT NULL
);
INSERT INTO tool_calls_new (id, message_id, tool, args, result, status, permission_mode, created_at)
  SELECT id, message_id, tool, args, result, COALESCE(status, 'error'),
         permission_mode, created_at
  FROM tool_calls;
DROP TABLE tool_calls;
ALTER TABLE tool_calls_new RENAME TO tool_calls;
CREATE INDEX idx_tool_calls_message ON tool_calls(message_id);