//! `fs_delete` (§6.2): **Recycle Bin only, never permanent.** The Windows
//! Recycle Bin is the one recoverable destination, so this is the only
//! deletion path the tool exposes. Registered in M2.6 behind the §6.6 gate.

use futures::future::BoxFuture;

use super::super::{Tool, ToolError, ToolExecCtx, ToolOutcome};
#[cfg(test)]
use super::resolve_arg;

pub struct FsDelete;

impl Tool for FsDelete {
    fn spec(&self) -> crate::types::ToolSpec {
        crate::types::ToolSpec {
            name: "fs_delete".into(),
            description: "Move a file or directory inside a bound workspace to the Recycle Bin (recoverable — never a permanent delete)."
                .into(),
            input_schema: serde_json::json!({
                "type": "object",
                "required": ["path"],
                "properties": {
                    "path": { "type": "string", "description": "File or directory to recycle" }
                }
            }),
        }
    }

    fn execute(
        &self,
        args: serde_json::Value,
        ctx: &ToolExecCtx,
    ) -> BoxFuture<'_, Result<ToolOutcome, ToolError>> {
        let result = (|| {
            let resolved = super::resolve_arg(&args, ctx, "path")?;
            run(&resolved.path).map(ToolOutcome::Ok)
        })();
        Box::pin(std::future::ready(result))
    }
}

fn run(path: &std::path::Path) -> Result<String, ToolError> {
    if !path.exists() {
        return Err(ToolError::Exec("path not found".into()));
    }
    trash::delete(path).map_err(|e| ToolError::Exec(format!("recycle: {e}")))?;
    Ok(format!("moved {} to the Recycle Bin", path.display()))
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
    fn recycles_a_file() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("gone.txt");
        std::fs::write(&p, "x").unwrap();
        run(&p).unwrap();
        assert!(!p.exists(), "removed from the workspace");
    }

    #[test]
    fn missing_path_errors() {
        let dir = tempfile::tempdir().unwrap();
        assert!(run(&dir.path().join("nope.txt")).is_err());
    }

    #[tokio::test]
    async fn executes_through_the_guard() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("f.txt"), "x").unwrap();
        let out = FsDelete
            .execute(serde_json::json!({"path": "f.txt"}), &ctx_with(dir.path()))
            .await;
        let out = crate::tools::fs::unwrap_ok(out.unwrap());
        assert!(out.contains("Recycle Bin"), "{out}");
        assert!(!dir.path().join("f.txt").exists());
    }

    #[tokio::test]
    async fn outside_roots_denied() {
        let dir = tempfile::tempdir().unwrap();
        let out = FsDelete
            .execute(
                serde_json::json!({"path": std::env::var("WINDIR").unwrap()}),
                &ctx_with(dir.path()),
            )
            .await;
        assert!(matches!(out, Err(ToolError::Exec(_))), "{out:?}");
    }
}