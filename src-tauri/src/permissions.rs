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
/// matrix. Reads are auto-allowed (§6.2, M2.4) and never gate.
pub const MUTATING_TOOLS: &[&str] = &[
    "fs_write", "fs_edit", "fs_move", "fs_copy", "fs_delete", "fs_mkdir",
];

pub fn is_mutating(tool: &str) -> bool {
    MUTATING_TOOLS.contains(&tool)
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
        }
        for read in ["fs_list", "fs_read", "fs_stat", "fs_search", "fs_tree", "echo", "ctx"] {
            assert!(!is_mutating(read), "{read} must not gate");
        }
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