-- §6.6: the permission matrix is remembered per (tool × workspace root).
-- "always" grants persist here; "ask" removes the row (the default).
CREATE TABLE tool_permissions (
  tool TEXT NOT NULL,
  root TEXT NOT NULL,
  mode TEXT NOT NULL,           -- always (ask/session are transient)
  PRIMARY KEY (tool, root)
);