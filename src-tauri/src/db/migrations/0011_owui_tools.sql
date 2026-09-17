-- OpenWebUI "Tools" manifests (§6.5b): raw Python `class Tools` source,
-- parsed per use; the display name is user-chosen.
CREATE TABLE owui_tools (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    source TEXT NOT NULL,
    enabled INTEGER NOT NULL DEFAULT 1,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);