//! `fs_write` (§6.2): create / overwrite / append, atomic (tmp + rename).
//! Parent directories must exist unless `create_dirs` is set (fs_mkdir is
//! the explicit path). Registered in M2.6 behind the §6.6 permission gate.

use futures::future::BoxFuture;

use super::super::{Tool, ToolError, ToolExecCtx, ToolOutcome};
#[cfg(test)]
use super::resolve_arg;

pub struct FsWrite;

impl Tool for FsWrite {
    fn spec(&self) -> crate::types::ToolSpec {
        crate::types::ToolSpec {
            name: "fs_write".into(),
            description: "Create or overwrite a text file inside a bound workspace (atomic tmp+rename), or append with mode=\"append\"."
                .into(),
            input_schema: serde_json::json!({
                "type": "object",
                "required": ["path", "content"],
                "properties": {
                    "path": { "type": "string", "description": "File to write (absolute inside a workspace, or workspace-relative)" },
                    "content": { "type": "string", "description": "Full file contents (UTF-8)" },
                    "mode": { "type": "string", "description": "\"overwrite\" (default) or \"append\"" },
                    "create_dirs": { "type": "boolean", "description": "Create missing parent directories (default false)" }
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
            let resolved = super::resolve_new_arg(&args, ctx, "path")?;
            let content = args
                .get("content")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::Exec("missing `content` argument".into()))?;
            let append = super::arg_str(&args, "mode").is_some_and(|m| m == "append");
            let create_dirs = args.get("create_dirs").is_some_and(|v| v.as_bool() == Some(true));
            run(&resolved.path, content, append, create_dirs).map(ToolOutcome::Ok)
        })();
        Box::pin(std::future::ready(result))
    }
}

fn run(
    path: &std::path::Path,
    content: &str,
    append: bool,
    create_dirs: bool,
) -> Result<String, ToolError> {
    if path.is_dir() {
        return Err(ToolError::Exec("path is a directory".into()));
    }
    let parent = path
        .parent()
        .ok_or_else(|| ToolError::Exec("path has no parent".into()))?;
    if !parent.exists() {
        if !create_dirs {
            return Err(ToolError::Exec(
                "parent directory does not exist — pass create_dirs=true or run fs_mkdir first"
                    .into(),
            ));
        }
        std::fs::create_dir_all(parent)
            .map_err(|e| ToolError::Exec(format!("create_dir_all: {e}")))?;
    }
    if append {
        use std::io::Write as _;
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .map_err(|e| ToolError::Exec(format!("append: {e}")))?;
        f.write_all(content.as_bytes())
            .map_err(|e| ToolError::Exec(format!("append: {e}")))?;
        return Ok(format!("appended {} bytes to {}", content.len(), path.display()));
    }
    // Atomic overwrite: write a sibling temp file, then rename over.
    let tmp = parent.join(format!(
        ".ternion-tmp-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::write(&tmp, content).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        ToolError::Exec(format!("write: {e}"))
    })?;
    std::fs::rename(&tmp, path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        ToolError::Exec(format!("rename: {e}"))
    })?;
    Ok(format!(
        "wrote {} bytes to {}",
        content.len(),
        path.display()
    ))
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
    fn creates_and_overwrites_atomically() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("f.txt");
        run(&p, "one\n", false, false).unwrap();
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "one\n");
        run(&p, "two", false, false).unwrap();
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "two");
        // No tmp leftovers.
        assert!(std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(Result::ok)
            .all(|e| e.file_name() == "f.txt"));
    }

    #[test]
    fn append_preserves_existing_content() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("log.txt");
        run(&p, "one\n", false, false).unwrap();
        run(&p, "two\n", true, false).unwrap();
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "one\ntwo\n");
    }

    #[test]
    fn create_dirs_is_explicit() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a/b/f.txt");
        let err = run(&p, "x", false, false).unwrap_err().message();
        assert!(err.contains("parent directory does not exist"), "{err}");
        run(&p, "x", false, true).unwrap();
        assert!(p.exists());
    }

    #[test]
    fn directory_target_refused() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("sub");
        std::fs::create_dir(&sub).unwrap();
        assert!(run(&sub, "x", false, false).is_err());
    }

    #[tokio::test]
    async fn resolves_creation_paths_through_the_guard() {
        let dir = tempfile::tempdir().unwrap();
        let out = FsWrite
            .execute(
                serde_json::json!({"path": "notes/todo.md", "content": "hi", "create_dirs": true}),
                &ctx_with(dir.path()),
            )
            .await;
        let out = crate::tools::fs::unwrap_ok(out.unwrap());
        assert!(out.contains("todo.md"), "{out}");
        assert!(dir.path().join("notes/todo.md").exists());
    }

    #[tokio::test]
    async fn outside_roots_denied() {
        let dir = tempfile::tempdir().unwrap();
        let out = FsWrite
            .execute(
                serde_json::json!({"path": std::env::var("WINDIR").unwrap(), "content": "x"}),
                &ctx_with(dir.path()),
            )
            .await;
        assert!(matches!(out, Err(ToolError::Exec(_))), "{out:?}");
    }
}