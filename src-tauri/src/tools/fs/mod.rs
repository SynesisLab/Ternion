//! File-system tools (design §6.2): reads in M2.4 (`fs_list`, `fs_read`,
//! `fs_stat`, `fs_search`, `fs_tree` — all auto-allowed); mutating tools in
//! M2.5 (`fs_write`, `fs_edit`, `fs_move`, `fs_copy`, `fs_delete`,
//! `fs_mkdir`), registered in M2.6 behind the §6.6 permission gate. Every
//! path the model supplies goes through `guard::resolve` / `resolve_new`
//! before any FS call; results are untrusted text (§10.3).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::{Tool, ToolError, ToolExecCtx, ToolOutcome};

pub mod delete;
pub mod edit;
pub mod list;
pub mod mkdir;
pub mod read;
pub mod search;
pub mod stat;
pub mod transfer;
pub mod tree;
pub mod write;

/// The bundled read tools (M2.4). Registered only for conversations with a
/// bound workspace — the orchestrator gates on `workspace_roots`.
pub fn bundled_read_tools() -> Vec<Arc<dyn Tool>> {
    vec![
        Arc::new(list::FsList),
        Arc::new(read::FsRead),
        Arc::new(stat::FsStat),
        Arc::new(search::FsSearch),
        Arc::new(tree::FsTree),
    ]
}

/// The bundled mutating tools (M2.5 executors). Registered in M2.6, where
/// the §6.6 permission matrix gates every call before execution.
#[allow(dead_code)]
pub fn bundled_mutating_tools() -> Vec<Arc<dyn Tool>> {
    vec![
        Arc::new(write::FsWrite),
        Arc::new(edit::FsEdit),
        Arc::new(transfer::FsMove),
        Arc::new(transfer::FsCopy),
        Arc::new(delete::FsDelete),
        Arc::new(mkdir::FsMkdir),
    ]
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

/// Resolve a path argument (model-supplied) through the §6.3 guard.
pub fn resolve_arg(
    args: &serde_json::Value,
    ctx: &ToolExecCtx,
    key: &str,
) -> Result<super::guard::ResolvedPath, ToolError> {
    let Some(raw) = args.get(key).and_then(|v| v.as_str()) else {
        return Err(ToolError::Exec(format!("missing `{key}` argument")));
    };
    let roots: Vec<PathBuf> = ctx.workspaces.iter().map(PathBuf::from).collect();
    super::guard::resolve(raw, &roots).map_err(|e| ToolError::Exec(e.to_string()))
}

/// Like `resolve_arg`, but the target may not exist yet — for tools that
/// create files/directories (fs_write, fs_mkdir, move/copy destinations).
pub fn resolve_new_arg(
    args: &serde_json::Value,
    ctx: &ToolExecCtx,
    key: &str,
) -> Result<super::guard::ResolvedPath, ToolError> {
    let Some(raw) = args.get(key).and_then(|v| v.as_str()) else {
        return Err(ToolError::Exec(format!("missing `{key}` argument")));
    };
    let roots: Vec<PathBuf> = ctx.workspaces.iter().map(PathBuf::from).collect();
    super::guard::resolve_new(raw, &roots).map_err(|e| ToolError::Exec(e.to_string()))
}

/// Optional string argument.
pub fn arg_str<'a>(args: &'a serde_json::Value, key: &str) -> Option<&'a str> {
    args.get(key).and_then(|v| v.as_str())
}

/// Optional integer argument, clamped to `[default.min, max]`.
pub fn arg_usize(
    args: &serde_json::Value,
    key: &str,
    default: usize,
    min: usize,
    max: usize,
) -> usize {
    args.get(key)
        .and_then(|v| v.as_u64())
        .map(|v| (v as usize).clamp(min, max))
        .unwrap_or(default)
}

/// Case-insensitive `*` / `?` glob against a file name (no path separators).
pub fn glob_match(pattern: &str, text: &str) -> bool {
    fn go(p: &[u8], t: &[u8]) -> bool {
        match (p.first(), t.first()) {
            (None, None) => true,
            (Some(b'*'), _) => go(&p[1..], t) || (!t.is_empty() && go(p, &t[1..])),
            (Some(b'?'), Some(_)) => go(&p[1..], &t[1..]),
            (Some(&c), Some(&d)) if c.eq_ignore_ascii_case(&d) => go(&p[1..], &t[1..]),
            _ => false,
        }
    }
    go(
        pattern.to_ascii_lowercase().as_bytes(),
        text.to_ascii_lowercase().as_bytes(),
    )
}

/// Extension-based MIME guess for the common cases (§6.2 fs_stat).
pub fn mime_for(ext: &str) -> &'static str {
    match ext.to_ascii_lowercase().as_str() {
        "txt" | "log" | "csv" | "ini" | "cfg" | "bat" | "ps1" | "cmd" => "text/plain",
        "md" | "markdown" => "text/markdown",
        "json" => "application/json",
        "toml" => "application/toml",
        "yaml" | "yml" => "application/yaml",
        "xml" => "application/xml",
        "html" | "htm" => "text/html",
        "css" => "text/css",
        "js" | "mjs" | "cjs" => "text/javascript",
        "ts" | "tsx" | "jsx" => "text/plain",
        "py" | "rb" | "rs" | "go" | "java" | "kt" | "c" | "h" | "cpp" | "hpp" | "cs" | "php"
        | "sh" => "text/plain",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "bmp" => "image/bmp",
        "ico" => "image/vnd.microsoft.icon",
        "pdf" => "application/pdf",
        "zip" => "application/zip",
        "gz" => "application/gzip",
        "7z" => "application/x-7z-compressed",
        "rar" => "application/vnd.rar",
        "exe" | "dll" => "application/vnd.microsoft.portable-executable",
        "msi" => "application/x-msi",
        "wav" => "audio/wav",
        "mp3" => "audio/mpeg",
        "mp4" => "video/mp4",
        "avi" | "mkv" | "mov" => "video/x-msvideo",
        _ => "application/octet-stream",
    }
}

pub fn mime_for_path(path: &Path) -> &'static str {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default();
    mime_for(ext)
}

/// `2026-09-16 10:31:00` (UTC) from a Unix timestamp — no chrono dependency;
/// listings and stats only need a stable, human-readable stamp.
pub fn format_epoch(secs: u64) -> String {
    let days = secs / 86_400;
    let rem = secs % 86_400;
    let (y, m, d) = civil_from_days(days as i64);
    format!(
        "{y:04}-{m:02}-{d:02} {:02}:{:02}:{:02}",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

/// Days-since-epoch → civil date (Howard Hinnant's algorithm).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
pub(crate) fn unwrap_ok(out: ToolOutcome) -> String {
    match out {
        ToolOutcome::Ok(s) => s,
        other => panic!("expected Ok outcome, got {other:?}"),
    }
}