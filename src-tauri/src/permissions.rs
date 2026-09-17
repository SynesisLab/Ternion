//! Permission system (design §6.6): a three-mode matrix per
//! `(tool × workspace root)` for the mutating tools.
//!
//! - **Ask** (default): a modal before every call; the model stream pauses
//!   (never cancelled) while the request is pending.
//! - **Session**: allowed for this app session (in-memory, lost on exit).
//! - **Always**: remembered per tool × root in `tool_permissions`.
//!
//! `fs_write` / `fs_edit` carry a unified diff and full resulting content in
//! the approval request so the modal can show what would change and let the
//! user take over the content.

use std::collections::HashSet;

/// Tools that mutate the filesystem — every one of them is gated by the
/// matrix and may hold session/always grants. Reads are auto-allowed
/// (§6.2, M2.4) and never gate.
pub const MUTATING_TOOLS: &[&str] = &[
    "fs_write", "fs_edit", "fs_move", "fs_copy", "fs_delete", "fs_mkdir",
];

pub fn is_mutating(tool: &str) -> bool {
    MUTATING_TOOLS.contains(&tool)
}

/// Every tool the matrix gates: the mutators plus `shell`, which asks on
/// every call — arbitrary commands can never be whitelisted (§6.6) — plus
/// every `mcp__*` / `owui__*` tool: third-party code behaves like the shell
/// tool (§6.5, §6.5b), ask by default but grantable per tool.
pub const GATED_TOOLS: &[&str] = &["shell"];

pub fn needs_gate(tool: &str) -> bool {
    is_mutating(tool)
        || GATED_TOOLS.contains(&tool)
        || tool.starts_with(crate::mcp::MCP_PREFIX)
        || tool.starts_with(crate::owui::OWUI_PREFIX)
}

/// Whether a tool may hold a session/always grant at all. Shell is excluded:
/// approving one command must never pre-approve the next. MCP and OpenWebUI
/// tools are grantable — one third-party tool ≠ every tool of its source.
pub fn grantable(tool: &str) -> bool {
    is_mutating(tool)
        || tool.starts_with(crate::mcp::MCP_PREFIX)
        || tool.starts_with(crate::owui::OWUI_PREFIX)
}

/// The path argument key a tool's matrix lookup keys on (fs_move/fs_copy
/// operate on `from`; everything else uses `path`).
pub fn main_path_key(tool: &str) -> &'static str {
    match tool {
        "fs_move" | "fs_copy" => "from",
        _ => "path",
    }
}

/// In-memory session grants — `(tool, root)` pairs allowed until exit.
/// Roots are matched case-insensitively (Windows paths).
#[derive(Default)]
pub struct SessionGrants(HashSet<(String, String)>);

impl SessionGrants {
    pub fn grant(&mut self, tool: &str, root: &str) {
        self.0.insert((tool.to_string(), root.to_lowercase()));
    }

    pub fn contains(&self, tool: &str, root: &str) -> bool {
        self.0.contains(&(tool.to_string(), root.to_lowercase()))
    }

    pub fn clear(&mut self) {
        self.0.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mutating_tool_set_is_the_fs_mutators() {
        for tool in MUTATING_TOOLS {
            assert!(is_mutating(tool), "{tool} must gate");
            assert!(needs_gate(tool), "{tool} must gate");
            assert!(grantable(tool), "{tool} must be grantable");
        }
        assert!(needs_gate("shell"), "shell always gates (§6.2/§6.6)");
        assert!(!grantable("shell"), "shell asks every time");
        for read in [
            "fs_list", "fs_read", "fs_stat", "fs_search", "fs_tree", "echo", "ctx",
        ] {
            assert!(!is_mutating(read), "{read} must not gate");
            assert!(!needs_gate(read), "{read} must not gate");
            assert!(!grantable(read), "{read} must not be grantable");
        }
    }

    #[test]
    fn mcp_tools_gate_like_the_shell_but_are_grantable() {
        assert!(needs_gate("mcp__github__create_issue"));
        assert!(needs_gate("mcp__fs__read_file"), "even reads gate by default");
        assert!(grantable("mcp__github__create_issue"));
        assert!(!needs_gate("mcp"));
        assert!(!needs_gate("mcp_"));
    }

    #[test]
    fn owui_tools_gate_like_mcp_tools() {
        assert!(needs_gate("owui__weather-tools__get_weather"));
        assert!(grantable("owui__weather-tools__get_weather"));
        assert!(!needs_gate("owui"));
        assert!(!needs_gate("owui_"));
    }

    #[test]
    fn path_key_follows_the_tool() {
        assert_eq!(main_path_key("fs_move"), "from");
        assert_eq!(main_path_key("fs_copy"), "from");
        assert_eq!(main_path_key("fs_write"), "path");
        assert_eq!(main_path_key("fs_delete"), "path");
    }

    #[test]
    fn session_grants_are_per_tool_and_root() {
        let mut grants = SessionGrants::default();
        assert!(!grants.contains("fs_write", "C:\\w"));
        grants.grant("fs_write", "C:\\w");
        assert!(grants.contains("fs_write", "C:\\w"));
        assert!(grants.contains("fs_write", "c:\\W"), "roots match case-insensitively");
        assert!(!grants.contains("fs_edit", "C:\\w"));
        grants.clear();
        assert!(!grants.contains("fs_write", "C:\\w"));
    }
}