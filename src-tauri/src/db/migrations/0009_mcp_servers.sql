-- §6.5: MCP stdio servers. `command` is the full command line the server is
-- spawned with (cmd /c <command> on Windows). Tools merge into the registry
-- namespaced mcp__<server>__<tool>; a server can never shadow a bundled tool.
CREATE TABLE mcp_servers (
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  command TEXT NOT NULL,
  enabled INTEGER NOT NULL DEFAULT 1,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);