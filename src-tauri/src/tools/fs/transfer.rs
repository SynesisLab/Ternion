//! `fs_move` / `fs_copy` (§6.2): rename/move/copy within or across bound
//! roots. Destinations may exist only when they name the same file
//! (move-overwrite is refused; copy refuses to clobber). Registered in
//! M2.6 behind the §6.6 gate.

use futures::future::BoxFuture;

use super::super::{Tool, ToolError, ToolExecCtx, ToolOutcome};
#[cfg(test)]
use super::resolve_arg;

pub struct FsMove;

impl Tool for FsMove {
    fn spec(&self) -> crate::types::ToolSpec {
        crate::types::ToolSpec {
            name: "fs_move".into(),
            description: "Move/rename a file or directory inside a bound workspace (also across two bound roots). Refuses to overwrite an existing destination."
                .into(),
            input_schema: serde_json::json!({
                "type": "object",
                "required": ["from", "to"],
                "properties": {
                    "from": { "type": "string", "description": "Existing source path" },
                    "to": { "type": "string", "description": "Destination path (must not exist)" }
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
            let from = super::resolve_arg(&args, ctx, "from")?.path;
            let to = super::resolve_new_arg(&args, ctx, "to")?.path;
            run(&from, &to).map(ToolOutcome::Ok)
        })();
        Box::pin(std::future::ready(result))
    }
}

pub struct FsCopy;

impl Tool for FsCopy {
    fn spec(&self) -> crate::types::ToolSpec {
        crate::types::ToolSpec {
            name: "fs_copy".into(),
            description: "Copy a file inside a bound workspace (also across roots). Refuses to overwrite an existing destination."
                .into(),
            input_schema: serde_json::json!({
                "type": "object",
                "required": ["from", "to"],
                "properties": {
                    "from": { "type": "string", "description": "Existing source file" },
                    "to": { "type": "string", "description": "Destination path (must not exist)" }
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
            let from = super::resolve_arg(&args, ctx, "from")?.path;
            let to = super::resolve_new_arg(&args, ctx, "to")?.path;
            copy_run(&from, &to).map(ToolOutcome::Ok)
        })();
        Box::pin(std::future::ready(result))
    }
}

fn refuse_clobber(to: &std::path::Path) -> Result<(), ToolError> {
    if to.exists() {
        return Err(ToolError::Exec(format!(
            "destination {} already exists — fs_move/fs_copy never overwrite",
            to.display()
        )));
    }
    Ok(())
}

fn run(from: &std::path::Path, to: &std::path::Path) -> Result<String, ToolError> {
    if !from.exists() {
        return Err(ToolError::Exec("source path not found".into()));
    }
    if from.is_dir() && to.starts_with(from) {
        return Err(ToolError::Exec(
            "cannot move a directory into itself".into(),
        ));
    }
    refuse_clobber(to)?;
    // rename() is atomic when it works; across volumes it fails, so fall
    // back to copy + remove (still a move: nothing is left behind).
    match std::fs::rename(from, to) {
        Ok(()) => Ok(format!("moved {} → {}", from.display(), to.display())),
        Err(_) => {
            copy_tree(from, to).map_err(|e| ToolError::Exec(format!("copy: {e}")))?;
            remove_tree(from).map_err(|e| {
                let _ = remove_tree(to);
                ToolError::Exec(format!("move fell back to copy but cleanup failed: {e}"))
            })?;
            Ok(format!(
                "moved (cross-volume) {} → {}",
                from.display(),
                to.display()
            ))
        }
    }
}

fn copy_run(from: &std::path::Path, to: &std::path::Path) -> Result<String, ToolError> {
    if !from.exists() {
        return Err(ToolError::Exec("source path not found".into()));
    }
    refuse_clobber(to)?;
    copy_tree(from, to).map_err(|e| ToolError::Exec(format!("copy: {e}")))?;
    Ok(format!("copied {} → {}", from.display(), to.display()))
}

/// Files via fs::copy; directories recursively (empty parents preserved).
fn copy_tree(from: &std::path::Path, to: &std::path::Path) -> std::io::Result<()> {
    let meta = std::fs::metadata(from)?;
    if meta.is_dir() {
        std::fs::create_dir_all(to)?;
        for entry in std::fs::read_dir(from)?.flatten() {
            copy_tree(
                &entry.path(),
                &to.join(entry.file_name()),
            )?;
        }
        Ok(())
    } else {
        std::fs::copy(from, to).map(|_| ())
    }
}

fn remove_tree(path: &std::path::Path) -> std::io::Result<()> {
    let meta = std::fs::metadata(path)?;
    if meta.is_dir() {
        std::fs::remove_dir_all(path)
    } else {
        std::fs::remove_file(path)
    }
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
    fn move_renames_within_root() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.txt");
        std::fs::write(&a, "data").unwrap();
        let b = dir.path().join("b.txt");
        run(&a, &b).unwrap();
        assert!(!a.exists());
        assert_eq!(std::fs::read_to_string(&b).unwrap(), "data");
    }

    #[test]
    fn move_refuses_to_clobber() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.txt");
        let b = dir.path().join("b.txt");
        std::fs::write(&a, "A").unwrap();
        std::fs::write(&b, "B").unwrap();
        let err = run(&a, &b).unwrap_err().message();
        assert!(err.contains("already exists"), "{err}");
        assert!(a.exists(), "source untouched");
    }

    #[test]
    fn copy_keeps_source() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "data").unwrap();
        copy_run(&dir.path().join("a.txt"), &dir.path().join("b.txt")).unwrap();
        assert!(dir.path().join("a.txt").exists());
        assert!(dir.path().join("b.txt").exists());
    }

    #[test]
    fn copies_directories_recursively() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src/inner")).unwrap();
        std::fs::write(dir.path().join("src/f.txt"), "x").unwrap();
        std::fs::write(dir.path().join("src/inner/g.txt"), "y").unwrap();
        copy_run(
            &dir.path().join("src"),
            &dir.path().join("src-copy"),
        )
        .unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.path().join("src-copy/inner/g.txt")).unwrap(),
            "y"
        );
    }

    #[tokio::test]
    async fn executes_through_the_guard() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "data").unwrap();
        let out = FsMove
            .execute(
                serde_json::json!({"from": "a.txt", "to": "renamed.txt"}),
                &ctx_with(dir.path()),
            )
            .await;
        let out = crate::tools::fs::unwrap_ok(out.unwrap());
        assert!(out.contains("renamed.txt"), "{out}");
        assert!(dir.path().join("renamed.txt").exists());
    }
}