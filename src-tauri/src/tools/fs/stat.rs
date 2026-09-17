//! `fs_stat` (§6.2): type / size / timestamps / MIME for one path.

use futures::future::BoxFuture;

use super::super::{Tool, ToolError, ToolExecCtx, ToolOutcome};
use super::{format_epoch, mime_for_path};

pub struct FsStat;

impl Tool for FsStat {
    fn spec(&self) -> crate::types::ToolSpec {
        crate::types::ToolSpec {
            name: "fs_stat".into(),
            description: "File metadata inside a bound workspace: type, size, MIME, created/modified time, readonly flag."
                .into(),
            input_schema: serde_json::json!({
                "type": "object",
                "required": ["path"],
                "properties": {
                    "path": { "type": "string", "description": "File or directory to stat" }
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
            run(&resolved.path).map(ToolOutcome::Ok)
        })();
        Box::pin(std::future::ready(result))
    }
}

fn run(path: &std::path::Path) -> Result<String, ToolError> {
    let meta = std::fs::metadata(path).map_err(|_| ToolError::Exec("path not found".into()))?;
    let kind = if meta.is_dir() { "directory" } else { "file" };
    let secs = |t: std::io::Result<std::time::SystemTime>| {
        t.ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs())
            .unwrap_or(0)
    };
    Ok(format!(
        "path:     {}\ntype:     {kind}\nsize:     {} bytes\nmime:     {}\ncreated:  {}\nmodified: {}\nreadonly: {}",
        path.display(),
        meta.len(),
        mime_for_path(path),
        format_epoch(secs(meta.created())),
        format_epoch(secs(meta.modified())),
        meta.permissions().readonly()
    ))
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
    fn stats_a_file() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("notes.md");
        std::fs::write(&p, "12345").unwrap();
        let out = run(&p.canonicalize().unwrap()).unwrap();
        assert!(out.contains("type:     file"), "{out}");
        assert!(out.contains("5 bytes"), "{out}");
        assert!(out.contains("text/markdown"), "{out}");
        assert!(out.contains("readonly: false"), "{out}");
    }

    #[test]
    fn stats_a_directory() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("sub");
        std::fs::create_dir(&sub).unwrap();
        let out = run(&sub.canonicalize().unwrap()).unwrap();
        assert!(out.contains("type:     directory"), "{out}");
    }

    #[test]
    fn missing_path_errors() {
        let dir = tempfile::tempdir().unwrap();
        let msg = run(&dir.path().join("nope.txt")).unwrap_err().message();
        assert!(msg.contains("not found"), "{msg}");
    }

    #[tokio::test]
    async fn guard_gates_stat() {
        let dir = tempfile::tempdir().unwrap();
        let out = FsStat
            .execute(
                serde_json::json!({"path": std::env::var("WINDIR").unwrap()}),
                &ctx_with(dir.path()),
            )
            .await;
        assert!(matches!(out, Err(ToolError::Exec(_))), "{out:?}");
    }

    #[tokio::test]
    async fn executes_through_the_guard() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("f.rs"), "fn main() {}").unwrap();
        let out = FsStat
            .execute(serde_json::json!({"path": "f.rs"}), &ctx_with(dir.path()))
            .await;
        let out = crate::tools::fs::unwrap_ok(out.unwrap());
        assert!(out.contains("f.rs"), "{out}");
    }
}