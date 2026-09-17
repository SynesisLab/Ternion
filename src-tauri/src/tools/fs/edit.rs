//! `fs_edit` (§6.2): exact-string replace, multi-hunk-capable via repeated
//! calls. Fails on ambiguity (0 matches, or more than one unless
//! `expected_occurrences` is set) — keeps small models reliable and makes
//! diffs minimal and reviewable. Registered in M2.6 behind the §6.6 gate.

use futures::future::BoxFuture;

use super::super::{Tool, ToolError, ToolExecCtx, ToolOutcome};
use super::resolve_arg;

pub struct FsEdit;

impl Tool for FsEdit {
    fn spec(&self) -> crate::types::ToolSpec {
        crate::types::ToolSpec {
            name: "fs_edit".into(),
            description: "Replace an exact string in a text file inside a bound workspace. Fails unless the string matches exactly expected_occurrences times (default 1)."
                .into(),
            input_schema: serde_json::json!({
                "type": "object",
                "required": ["path", "old_string", "new_string"],
                "properties": {
                    "path": { "type": "string", "description": "File to edit" },
                    "old_string": { "type": "string", "description": "Exact text to find (include surrounding lines to disambiguate)" },
                    "new_string": { "type": "string", "description": "Replacement text (may be empty to delete)" },
                    "expected_occurrences": { "type": "integer", "description": "How many times old_string should match (default 1)" }
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
            let old = args
                .get("old_string")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::Exec("missing `old_string` argument".into()))?;
            let new = args
                .get("new_string")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::Exec("missing `new_string` argument".into()))?;
            let expected = args
                .get("expected_occurrences")
                .and_then(|v| v.as_u64())
                .map(|v| v as usize);
            run(&resolved.path, old, new, expected).map(ToolOutcome::Ok)
        })();
        Box::pin(std::future::ready(result))
    }
}

fn run(
    path: &std::path::Path,
    old: &str,
    new: &str,
    expected: Option<usize>,
) -> Result<String, ToolError> {
    if old.is_empty() {
        return Err(ToolError::Exec("old_string must not be empty".into()));
    }
    let bytes = std::fs::read(path).map_err(|_| ToolError::Exec("path not found".into()))?;
    if bytes.iter().take(8_192).any(|b| *b == 0) {
        return Err(ToolError::Exec(
            "binary file — fs_edit works on text files only".into(),
        ));
    }
    let text = String::from_utf8_lossy(&bytes).into_owned();
    let count = text.matches(old).count();
    let want = expected.unwrap_or(1);
    if count == 0 {
        return Err(ToolError::Exec(format!(
            "old_string not found in {} — copy it exactly from the file",
            path.display()
        )));
    }
    if count != want {
        return Err(ToolError::Exec(format!(
            "old_string matches {count} time(s) but expected_occurrences={want} — \
             include more surrounding lines to disambiguate, or set expected_occurrences={count}"
        )));
    }
    let edited = text.replace(old, new);
    // Atomic: sibling temp + rename (same as fs_write).
    let parent = path
        .parent()
        .ok_or_else(|| ToolError::Exec("path has no parent".into()))?;
    let tmp = parent.join(format!(
        ".ternion-tmp-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::write(&tmp, &edited).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        ToolError::Exec(format!("write: {e}"))
    })?;
    std::fs::rename(&tmp, path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        ToolError::Exec(format!("rename: {e}"))
    })?;
    Ok(format!(
        "replaced {count} occurrence(s) in {} ({} → {} bytes)",
        path.display(),
        bytes.len(),
        edited.len()
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
    fn replaces_single_occurrence() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("f.txt");
        std::fs::write(&p, "alpha beta gamma").unwrap();
        run(&p, "beta", "BETA", None).unwrap();
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "alpha BETA gamma");
    }

    #[test]
    fn ambiguity_requires_expected_occurrences() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("f.txt");
        std::fs::write(&p, "x\nx\n").unwrap();
        let err = run(&p, "x", "y", None).unwrap_err().message();
        assert!(err.contains("matches 2 time(s)"), "{err}");
        run(&p, "x", "y", Some(2)).unwrap();
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "y\ny\n");
    }

    #[test]
    fn zero_matches_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("f.txt");
        std::fs::write(&p, "abc").unwrap();
        let err = run(&p, "zzz", "y", None).unwrap_err().message();
        assert!(err.contains("not found"), "{err}");
    }

    #[test]
    fn empty_replacement_deletes_text() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("f.txt");
        std::fs::write(&p, "keep <bad> keep").unwrap();
        run(&p, "<bad>", "", None).unwrap();
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "keep  keep");
    }

    #[test]
    fn binary_files_refused() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("blob.bin");
        std::fs::write(&p, [0u8, 1, 2]).unwrap();
        let err = run(&p, "x", "y", None).unwrap_err().message();
        assert!(err.contains("text files only"), "{err}");
    }

    #[tokio::test]
    async fn executes_through_the_guard() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("f.txt"), "target here").unwrap();
        let out = FsEdit
            .execute(
                serde_json::json!({"path": "f.txt", "old_string": "target", "new_string": "TARGET"}),
                &ctx_with(dir.path()),
            )
            .await;
        let out = crate::tools::fs::unwrap_ok(out.unwrap());
        assert!(out.contains("1 occurrence"), "{out}");
    }
}