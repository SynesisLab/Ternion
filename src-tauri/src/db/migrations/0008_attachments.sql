-- Image attachments (design §7.2). 0001 already created the §8.2
-- attachments table (paths + dimensions + sha256 for dedupe); M3 adds the
-- created_at stamp and the message index. `message_id` is nullable there:
-- an attachment exists (and is previewed) before its message row is
-- persisted at send time.
ALTER TABLE attachments ADD COLUMN created_at INTEGER;
CREATE INDEX IF NOT EXISTS idx_attachments_message ON attachments(message_id);