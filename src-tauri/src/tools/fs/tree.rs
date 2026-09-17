//! `fs_tree` (§6.2): ASCII tree to a depth cap. `.git` / VCS dirs are
//! skipped; a total-entry cap keeps a big repo from flooding the context.

use futures::future::BoxFuture;

use super::super::{Tool, ToolError, ToolExecCtx, ToolOutcome};
use super::arg_usize;

const DEFAULT_DEPTH: usize = 3;
const MAX_DEPTH: usize = 6;
const MAX_ENTRIES: usize = 1_000;

pub struct FsTree;

impl Tool for FsTree {
    fn spec(&self) -> crate::types::ToolSpec {
        crate::types::ToolSpec {
            name: "fs_tree".into(),
            description: "ASCII directory tree inside a bound workspace to a depth cap (default 3). Skips VCS folders."
                .into(),
            input_schema: serde_json::json!({
                "type": "object",
                "required": ["path"],
                "properties": {
                    "path": { "type": "string", "description": "Root directory of the tree" },
                    "depth": { "type": "integer", "description": "Levels below the root (default 3, max 6)" }
                }
            }),
        }
    }

    fn execute(
        &self,
        args: serde_json::Value,
        ctx: &ToolExecCtx,
    ) -> BoxFuture<'_, Result<ToolOutcome, ToolError>> {
        // Sync tool: run now, wrap in a ready future (see fs_list).
        let result = (|| {
            let resolved = super::resolve_arg(&args, ctx, "path")?;
            let depth = arg_usize(&args, "depth", DEFAULT_DEPTH, 1, MAX_DEPTH);
            run(&resolved.path, depth).map(ToolOutcome::Ok)
        })();
        Box::pin(std::future::ready(result))
    }
}

struct Budget {
    entries: usize,
}

impl Budget {
    fn take(&mut self) -> bool {
        if self.entries >= MAX_ENTRIES {
            return false;
        }
        self.entries += 1;
        true
    }
}

fn run(dir: &std::path::Path, depth: usize) -> Result<String, ToolError> {
    if !dir.is_dir() {
        return Err(ToolError::Exec(
            "path is not a directory — fs_tree lists folders".into(),
        ));
    }
    let mut out = String::new();
    let mut budget = Budget { entries: 0 };
    let truncated = walk(dir, "", depth, &mut budget, &mut out);
    if truncated {
        out.push_str(&format!("[… tree truncated at {MAX_ENTRIES} entries]\n"));
    }
    Ok(out)
}

/// Git-style ASCII tree: `|-- name`, `` `-- name``, `|   ` / `    ` continuations.
/// Returns true if the entry cap was hit.
fn walk(dir: &std::path::Path, prefix: &str, depth: usize, budget: &mut Budget, out: &mut String) -> bool {
    let mut entries: Vec<(bool, String)> = Vec::new();
    let Ok(rd) = std::fs::read_dir(dir) else {
        return false; // unreadable dir: skip silently
    };
    for entry in rd.flatten() {
        let Ok(name) = entry.file_name().into_string() else { continue };
        let Ok(meta) = entry.metadata() else { continue };
        if meta.is_dir() && is_vcs(&name) {
            continue;
        }
        entries.push((meta.is_dir(), name));
    }
    // Dirs first, then names, case-insensitive — same order as fs_list.
    entries.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then_with(|| a.1.to_lowercase().cmp(&b.1.to_lowercase()))
    });

    let total = entries.len();
    for (i, (is_dir, name)) in entries.iter().enumerate() {
        if !budget.take() {
            return true;
        }
        let last = i + 1 == total;
        let branch = if last { "`-- " } else { "|-- " };
        let label = if *is_dir { format!("{name}/") } else { name.clone() };
        out.push_str(&format!("{prefix}{branch}{label}\n"));
        if *is_dir && depth > 1 {
            let child_prefix = if last { format!("{prefix}    ") } else { format!("{prefix}|   ") };
            if walk(&dir.join(name), &child_prefix, depth - 1, budget, out) {
                return true;
            }
        }
    }
    false
}

fn is_vcs(name: &str) -> bool {
    matches!(name.to_ascii_lowercase().as_str(), ".git" | ".hg" | ".svn")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx_with(root: &std::path::Path) -> ToolExecCtx {
        ToolExecCtx {
            workspaces: vec![root.display().to_string()],
        }
    }

    #[test]
    fn draws_ascii_tree_with_dirs_first() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("main.rs"), "").unwrap();
        std::fs::create_dir(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("src/lib.rs"), "").unwrap();
        std::fs::write(dir.path().join("src/main.rs"), "").unwrap();
        let out = run(&dir.path().canonicalize().unwrap(), DEFAULT_DEPTH).unwrap();
        assert!(out.starts_with("|-- src/\n"), "{out}");
        assert!(out.contains("|   |-- lib.rs\n"), "{out}");
        assert!(out.contains("|   `-- main.rs\n"), "{out}");
        assert!(out.contains("`-- main.rs\n"), "{out}");
        // No VCS connectors beyond depth.
        assert!(out.lines().count() == 4, "{out}");
    }

    #[test]
    fn depth_limits_recursion() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("a/b/c")).unwrap();
        std::fs::write(dir.path().join("a/b/c/deep.txt"), "").unwrap();
        let out = run(&dir.path().canonicalize().unwrap(), 2).unwrap();
        assert!(out.contains("b/"), "{out}");
        assert!(!out.contains("c/"), "depth 2 stops at b: {out}");
    }

    #[test]
    fn vcs_dirs_are_skipped() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join(".git")).unwrap();
        std::fs::write(dir.path().join(".git/config"), "").unwrap();
        std::fs::write(dir.path().join("keep.txt"), "").unwrap();
        let out = run(&dir.path().canonicalize().unwrap(), DEFAULT_DEPTH).unwrap();
        assert!(out.contains("keep.txt"), "{out}");
        assert!(!out.contains(".git"), "{out}");
    }

    #[test]
    fn entry_cap_truncates() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().join("d");
        std::fs::create_dir_all(&base).unwrap();
        for i in 0..MAX_ENTRIES + 5 {
            std::fs::write(base.join(format!("f{i:05}.txt")), "").unwrap();
        }
        let out = run(&base.canonicalize().unwrap(), DEFAULT_DEPTH).unwrap();
        assert!(out.contains("[… tree truncated at 1000 entries]"), "{out}");
    }

    #[test]
    fn files_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("f.txt");
        std::fs::write(&p, "x").unwrap();
        let msg = run(&p.canonicalize().unwrap(), DEFAULT_DEPTH)
            .unwrap_err()
            .message();
        assert!(msg.contains("not a directory"), "{msg}");
    }

    #[tokio::test]
    async fn executes_through_the_guard() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "").unwrap();
        let out = FsTree
            .execute(serde_json::json!({"path": "."}), &ctx_with(dir.path()))
            .await;
        let out = crate::tools::fs::unwrap_ok(out.unwrap());
        assert!(out.contains("a.txt"), "{out}");
    }
}