//! `fs_list` (§6.2): one-level directory listing with optional name-glob
//! filter, sizes and mtimes. Dirs first, then names (case-insensitive).

use futures::future::BoxFuture;

use super::super::{Tool, ToolError, ToolExecCtx, ToolOutcome};
use super::{arg_str, format_epoch};

const MAX_ENTRIES: usize = 500;

pub struct FsList;

impl Tool for FsList {
    fn spec(&self) -> crate::types::ToolSpec {
        crate::types::ToolSpec {
            name: "fs_list".into(),
            description: "List a directory inside a bound workspace: entries with type, size, modified time. Non-recursive; use fs_tree for structure."
                .into(),
            input_schema: serde_json::json!({
                "type": "object",
                "required": ["path"],
                "properties": {
                    "path": { "type": "string", "description": "Directory to list (absolute inside a workspace, or workspace-relative)" },
                    "glob": { "type": "string", "description": "Optional filename filter, e.g. \"*.rs\"" }
                }
            }),
        }
    }

    fn execute(
        &self,
        args: serde_json::Value,
        ctx: &ToolExecCtx,
    ) -> BoxFuture<'_, Result<ToolOutcome, ToolError>> {
        // Sync tool: run now, wrap in a ready future (the boxed future must
        // not borrow `ctx` — it carries `&self`'s lifetime).
        let result = (|| {
            let resolved = super::resolve_arg(&args, ctx, "path")?;
            let glob = arg_str(&args, "glob").map(str::to_string);
            run(&resolved.path, glob).map(ToolOutcome::Ok)
        })();
        Box::pin(std::future::ready(result))
    }
}

fn run(dir: &std::path::Path, glob: Option<String>) -> Result<String, ToolError> {
    if !dir.is_dir() {
        return Err(ToolError::Exec(
            "path is not a directory — use fs_read for files".into(),
        ));
    }
    let mut dirs: Vec<(String, u64)> = Vec::new();
    let mut files: Vec<(String, u64, u64)> = Vec::new();
    for entry in std::fs::read_dir(dir).map_err(|e| ToolError::Exec(format!("read_dir: {e}")))? {
        let entry =
            entry.map_err(|e| ToolError::Exec(format!("read_dir: {e}")))?;
        let Ok(name) = entry.file_name().into_string() else {
            continue; // non-UTF-8 name: skip, it isn't model-visible anyway
        };
        if let Some(glob) = &glob && !super::glob_match(glob, &name) {
            continue;
        }
        let Ok(meta) = entry.metadata() else { continue };
        if meta.is_dir() {
            dirs.push((name, mtime_of(&meta)));
        } else {
            files.push((name, meta.len(), mtime_of(&meta)));
        }
    }
    dirs.sort_by(|a, b| a.0.to_lowercase().cmp(&b.0.to_lowercase()));
    files.sort_by(|a, b| a.0.to_lowercase().cmp(&b.0.to_lowercase()));

    let mut out = String::new();
    let total = dirs.len() + files.len();
    for (name, mtime) in dirs.iter().take(MAX_ENTRIES) {
        out.push_str(&format!("d          -  {}  {name}/\n", format_epoch(*mtime)));
    }
    let dir_shown = dirs.len().min(MAX_ENTRIES);
    let file_budget = MAX_ENTRIES.saturating_sub(dir_shown);
    for (name, size, mtime) in files.iter().take(file_budget) {
        out.push_str(&format!(
            "f {size:>10}  {}  {name}\n",
            format_epoch(*mtime)
        ));
    }
    let shown = out.lines().count();
    if shown < total {
        out.push_str(&format!("[… {} more entries not shown]\n", total - shown));
    }
    if total == 0 {
        out.push_str("(empty directory)\n");
    }
    Ok(out)
}

fn mtime_of(meta: &std::fs::Metadata) -> u64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx_with(root: &std::path::Path) -> ToolExecCtx {
        ToolExecCtx {
            workspaces: vec![root.display().to_string()],
            attachments: None,
        }
    }

    #[test]
    fn lists_dirs_first_then_files_with_sizes() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("b.txt"), "12345").unwrap();
        std::fs::write(dir.path().join("a.txt"), "1").unwrap();
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        let out = run(&dir.path().canonicalize().unwrap(), None).unwrap();
        let lines: Vec<&str> = out.lines().collect();
        assert!(lines[0].starts_with('d'), "dirs first: {out}");
        assert!(lines[0].contains("sub/"));
        assert_eq!(lines.len(), 3, "{out}");
        assert!(lines.iter().any(|l| l.contains("b.txt") && l.contains("5")));
        assert!(lines.iter().any(|l| l.contains("a.txt") && l.contains("1")));
    }

    #[test]
    fn glob_filters_names() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("keep.rs"), "x").unwrap();
        std::fs::write(dir.path().join("skip.txt"), "x").unwrap();
        let out = run(&dir.path().canonicalize().unwrap(), Some("*.rs".into())).unwrap();
        assert!(out.contains("keep.rs"), "{out}");
        assert!(!out.contains("skip.txt"), "{out}");
    }

    #[test]
    fn entry_cap_truncates() {
        let dir = tempfile::tempdir().unwrap();
        for i in 0..MAX_ENTRIES + 10 {
            std::fs::write(dir.path().join(format!("f{i:04}.txt")), "").unwrap();
        }
        let out = run(&dir.path().canonicalize().unwrap(), None).unwrap();
        assert_eq!(out.lines().count(), MAX_ENTRIES + 1, "cap + note");
        assert!(out.contains("[… 10 more entries not shown]"), "{out}");
    }

    #[tokio::test]
    async fn guard_denies_outside_paths() {
        let dir = tempfile::tempdir().unwrap();
        let windir = std::env::var("WINDIR").unwrap();
        let out = FsList
            .execute(
                serde_json::json!({"path": windir}),
                &ctx_with(dir.path()),
            )
            .await;
        assert!(matches!(out, Err(ToolError::Exec(_))), "{out:?}");
    }

    #[test]
    fn empty_directory_note() {
        let dir = tempfile::tempdir().unwrap();
        let out = run(&dir.path().canonicalize().unwrap(), None).unwrap();
        assert!(out.contains("empty"), "{out}");
    }

    #[tokio::test]
    async fn executes_through_the_guard() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("docs")).unwrap();
        let out = FsList
            .execute(
                serde_json::json!({"path": "docs", "glob": "*.md"}),
                &ctx_with(dir.path()),
            )
            .await;
        let out = crate::tools::fs::unwrap_ok(out.unwrap());
        assert!(out.contains("empty"), "{out}");
    }

    #[tokio::test]
    async fn listing_a_file_refers_to_fs_read() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("notes.md"), "hi").unwrap();
        let msg = FsList
            .execute(
                serde_json::json!({"path": "notes.md"}),
                &ctx_with(dir.path()),
            )
            .await
            .unwrap_err()
            .message();
        assert!(msg.contains("not a directory"), "{msg}");
        assert!(msg.contains("fs_read"), "{msg}");
    }
}