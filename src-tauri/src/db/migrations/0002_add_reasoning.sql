-- Reasoning passthrough (design §5.3): thinking models stream a separate
-- reasoning channel; persisted so the collapsible block survives reloads.
-- Never fed back to non-reasoning models.
ALTER TABLE messages ADD COLUMN reasoning TEXT;