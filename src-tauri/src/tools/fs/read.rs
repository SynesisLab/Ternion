//! `fs_read` (§6.2): text files with a 1-based line range, chunk-capped.
//! Directories are refused; binary files are refused with their MIME;
//! images get a metadata reference (inlined image attachments land with
//! vision support).

use futures::future::BoxFuture;

use super::super::{Tool, ToolError, ToolExecCtx, ToolOutcome};
use super::{arg_usize, mime_for_path};

const DEFAULT_LINES: usize = 2_000;
const MAX_LINES: usize = 2_000;
const MAX_CHARS: usize = 48_000;

pub struct FsRead;

impl Tool for FsRead {
    fn spec(&self) -> crate::types::ToolSpec {
        crate::types::ToolSpec {
            name: "fs_read".into(),
            description: "Read a text file inside a bound workspace, numbered lines. Use offset/limit for ranges (1-based). Images and binary files are not returned as text."
                .into(),
            input_schema: serde_json::json!({
                "type": "object",
                "required": ["path"],
                "properties": {
                    "path": { "type": "string", "description": "File to read (absolute inside a workspace, or workspace-relative)" },
                    "offset": { "type": "integer", "description": "1-based first line (default 1)" },
                    "limit": { "type": "integer", "description": "Max lines to return (default 2000)" }
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
            let offset = arg_usize(&args, "offset", 1, 1, u32::MAX as usize);
            let limit = arg_usize(&args, "limit", DEFAULT_LINES, 1, MAX_LINES);
            run(&resolved.path, offset, limit).map(ToolOutcome::Ok)
        })();
        Box::pin(std::future::ready(result))
    }
}

fn run(path: &std::path::Path, offset: usize, limit: usize) -> Result<String, ToolError> {
    let meta = std::fs::metadata(path).map_err(|_| ToolError::Exec("path not found".into()))?;
    if meta.is_dir() {
        return Err(ToolError::Exec(
            "path is a directory — use fs_list or fs_tree".into(),
        ));
    }

    // Images: metadata reference only — inline vision attachments land in the
    // vision milestone (§10.5); a text dump would corrupt the context.
    let mime = mime_for_path(path);
    if mime.starts_with("image/") {
        return Ok(format!(
            "[image file: {}]\nmime: {mime}\nsize: {} bytes\n[contents not inlined — image attachment delivery lands with vision support]",
            path.display(),
            meta.len()
        ));
    }

    let bytes = std::fs::read(path).map_err(|e| ToolError::Exec(format!("read: {e}")))?;
    if bytes.iter().take(8_192).any(|b| *b == 0) {
        return Err(ToolError::Exec(format!(
            "binary file ({mime}) — not readable as text"
        )));
    }
    let text = String::from_utf8_lossy(&bytes);

    // Trim the trailing newline so the last line isn't an empty artifact.
    let text = text.strip_suffix('\n').unwrap_or(&text);

    let mut out = String::new();
    let mut emitted = 0usize;
    let mut last_line = 0usize;
    let mut chars_used = 0usize;
    let total_lines = text.lines().count();
    for (idx, line) in text.lines().enumerate().skip(offset.saturating_sub(1)) {
        if emitted >= limit {
            break;
        }
        let line_no = idx + 1;
        let rendered = format!("{line_no:>6} | {line}\n");
        if chars_used + rendered.chars().count() > MAX_CHARS && emitted > 0 {
            break;
        }
        chars_used += rendered.chars().count();
        out.push_str(&rendered);
        emitted += 1;
        last_line = line_no;
    }
    if emitted == 0 {
        out.push_str(&format!(
            "[no lines in range — the file has {total_lines} line(s)]\n"
        ));
        return Ok(out);
    }
    let remaining = total_lines - last_line;
    if remaining > 0 {
        out.push_str(&format!(
            "[… {remaining} more lines — use offset={}]\n",
            last_line + 1
        ));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx_with(root: &std::path::Path) -> ToolExecCtx {
        ToolExecCtx {
            workspaces: vec![root.display().to_string()],
        }
    }

    fn thirty_lines() -> String {
        (1..=30).map(|i| format!("line {i}")).collect::<Vec<_>>().join("\n")
    }

    #[test]
    fn reads_numbered_lines() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("f.txt");
        std::fs::write(&p, thirty_lines()).unwrap();
        let out = run(&p, 1, DEFAULT_LINES).unwrap();
        assert!(out.starts_with("     1 | line 1"), "{out}");
        assert!(out.contains("30 | line 30"));
        assert!(!out.contains("[…"), "whole file fits: {out}");
    }

    #[test]
    fn offset_and_limit_slice_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("f.txt");
        std::fs::write(&p, thirty_lines()).unwrap();
        let out = run(&p, 5, 3).unwrap();
        assert!(out.contains("| line 5"), "{out}");
        assert!(out.contains("| line 7"));
        assert!(!out.contains("| line 4"));
        assert!(out.contains("[… 23 more lines — use offset=8]"), "{out}");
    }

    #[test]
    fn past_eof_reports_range() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("f.txt");
        std::fs::write(&p, "only\n").unwrap();
        let out = run(&p, 10, 5).unwrap();
        assert!(out.contains("has 1 line"), "{out}");
    }

    #[test]
    fn directories_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("sub");
        std::fs::create_dir(&sub).unwrap();
        assert!(run(&sub.canonicalize().unwrap(), 1, 10)
            .unwrap_err()
            .message()
            .contains("directory"));
    }

    #[test]
    fn binary_files_are_refused_with_mime() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("blob.bin");
        std::fs::write(&p, [0u8, 1, 2, 3]).unwrap();
        let msg = run(&p.canonicalize().unwrap(), 1, 10).unwrap_err().message();
        assert!(msg.contains("binary file"), "{msg}");
        assert!(msg.contains("application/octet-stream"), "{msg}");
    }

    #[test]
    fn images_get_a_metadata_reference() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("pic.png");
        std::fs::write(&p, [0x89, b'P', b'N', b'G']).unwrap();
        let out = run(&p.canonicalize().unwrap(), 1, DEFAULT_LINES).unwrap();
        assert!(out.contains("image/png"), "{out}");
        assert!(out.contains("contents not inlined"), "{out}");
        assert!(!out.contains('|'), "no line-number dump: {out}");
    }

    #[test]
    fn missing_file_is_not_found() {
        let dir = tempfile::tempdir().unwrap();
        let msg = run(&dir.path().join("nope.txt"), 1, 10).unwrap_err().message();
        assert!(msg.contains("not found"), "{msg}");
    }

    #[tokio::test]
    async fn executes_through_the_guard() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("notes.txt"), "hello\n").unwrap();
        let out = FsRead
            .execute(
                serde_json::json!({"path": "notes.txt"}),
                &ctx_with(dir.path()),
            )
            .await;
        let out = crate::tools::fs::unwrap_ok(out.unwrap());
        assert!(out.contains("hello"), "{out}");
    }
}