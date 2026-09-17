//! `fs_mkdir` (§6.2): create directories (recursive, like mkdir -p).
//! Registered in M2.6 behind the §6.6 gate.

use futures::future::BoxFuture;

use super::super::{Tool, ToolError, ToolExecCtx, ToolOutcome};
#[cfg(test)]
use super::resolve_new_arg;

pub struct FsMkdir;

impl Tool for FsMkdir {
    fn spec(&self) -> crate::types::ToolSpec {
        crate::types::ToolSpec {
            name: "fs_mkdir".into(),
            description: "Create a directory inside a bound workspace (creates missing parents, like mkdir -p)."
                .into(),
            input_schema: serde_json::json!({
                "type": "object",
                "required": ["path"],
                "properties": {
                    "path": { "type": "string", "description": "Directory to create" }
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
            let resolved = super::resolve_new_arg(&args, ctx, "path")?;
            run(&resolved.path).map(ToolOutcome::Ok)
        })();
        Box::pin(std::future::ready(result))
    }
}

fn run(path: &std::path::Path) -> Result<String, ToolError> {
    if path.exists() {
        return Err(ToolError::Exec(format!(
            "{} already exists ({}); fs_mkdir creates new folders",
            path.display(),
            if path.is_dir() { "as a directory" } else { "as a file" }
        )));
    }
    std::fs::create_dir_all(path).map_err(|e| ToolError::Exec(format!("mkdir: {e}")))?;
    Ok(format!("created {}", path.display()))
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
    fn creates_nested_directories() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a/b/c");
        run(&p).unwrap();
        assert!(p.is_dir());
        // Idempotent-style: existing directory is an explicit error.
        assert!(run(&p).is_err());
    }

    #[tokio::test]
    async fn executes_through_the_guard() {
        let dir = tempfile::tempdir().unwrap();
        let out = FsMkdir
            .execute(
                serde_json::json!({"path": "new/nested"}),
                &ctx_with(dir.path()),
            )
            .await;
        let out = crate::tools::fs::unwrap_ok(out.unwrap());
        assert!(out.contains("new\\nested"), "{out}");
        assert!(dir.path().join("new/nested").is_dir());
    }
}