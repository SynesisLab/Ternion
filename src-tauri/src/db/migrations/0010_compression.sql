-- §6.5c context compression: the progressive summary and the message id it
-- covers, cached per conversation and folded in on the next compression pass.
ALTER TABLE conversations ADD COLUMN compression_summary TEXT;
ALTER TABLE conversations ADD COLUMN compression_upto TEXT;