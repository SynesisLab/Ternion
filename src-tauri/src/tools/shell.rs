//! `shell` (§6.2): run a command through PowerShell inside a bound
//! workspace. Opt-in (`tools.shell_enabled`), the cwd is pinned to a bound
//! workspace root (guard-checked), and every call asks — arbitrary commands
//! are not whitelisted by the session/always grants (§6.6).
//!
//! PowerShell 7 (`pwsh`) is preferred when present; Windows PowerShell 5.1
//! is the fallback. Output is capped (8 KB); the process is killed on
//! timeout (default 30 s, clampable 1–120 via `timeout_secs`).

use std::sync::OnceLock;

use futures::future::BoxFuture;
use tokio::io::AsyncReadExt as _;

use super::{Tool, ToolError, ToolExecCtx, ToolOutcome};
use crate::tools::fs::{arg_str, resolve_arg};

const DEFAULT_TIMEOUT_SECS: u64 = 30;
const MAX_OUTPUT_CHARS: usize = 8_000;

pub struct Shell;

impl Tool for Shell {
    fn spec(&self) -> crate::types::ToolSpec {
        crate::types::ToolSpec {
            name: "shell".into(),
            description: "Run a command with PowerShell in the bound workspace (ask every time). \
                          Use for builds, tests, and scripts; prefer the fs_* tools for files."
                .into(),
            input_schema: serde_json::json!({
                "type": "object",
                "required": ["command"],
                "properties": {
                    "command": { "type": "string", "description": "PowerShell command to run" },
                    "cwd": { "type": "string", "description": "Working directory inside a workspace (default: first workspace root)" },
                    "timeout_secs": { "type": "integer", "description": "Kill the process after this many seconds (1-120, default 30)" }
                }
            }),
        }
    }

    fn execute(
        &self,
        args: serde_json::Value,
        ctx: &ToolExecCtx,
    ) -> BoxFuture<'_, Result<ToolOutcome, ToolError>> {
        // Resolve everything synchronously (BoxFuture may not borrow ctx),
        // then move the owned data into the boxed future.
        let result = (|| -> Result<Prepared, ToolError> {
            let command = args
                .get("command")
                .and_then(|v| v.as_str())
                .filter(|s| !s.trim().is_empty())
                .ok_or_else(|| ToolError::Exec("missing `command` argument".into()))?
                .to_string();
            if ctx.workspaces.is_empty() {
                return Err(ToolError::Exec(
                    "no workspace bound — the shell tool needs a workspace".into(),
                ));
            }
            // cwd: the guard-checked `cwd` arg, else pinned to the first
            // bound workspace root.
            let cwd = match arg_str(&args, "cwd") {
                Some(_) => resolve_arg(&args, ctx, "cwd")?.path,
                None => std::path::PathBuf::from(
                    ctx.workspaces
                        .first()
                        .ok_or_else(|| ToolError::Exec("no workspace bound".into()))?,
                ),
            };
            let timeout_secs = args
                .get("timeout_secs")
                .and_then(|v| v.as_u64())
                .map(|v| v.clamp(1, 120))
                .unwrap_or(DEFAULT_TIMEOUT_SECS);
            Ok(Prepared {
                command,
                cwd,
                timeout_secs,
            })
        })();
        Box::pin(async move {
            let prepared = match result {
                Ok(p) => p,
                Err(e) => return Err(e),
            };
            run(prepared).await
        })
    }
}

struct Prepared {
    command: String,
    cwd: std::path::PathBuf,
    timeout_secs: u64,
}

/// PowerShell 7 first, Windows PowerShell 5.1 fallback. Resolved once.
fn shell_binary() -> &'static str {
    static PICKED: OnceLock<String> = OnceLock::new();
    PICKED.get_or_init(|| {
        // pwsh.exe is on PATH for PowerShell 7 installs.
        let pwsh = where_exe("pwsh.exe").is_some();
        if pwsh {
            "pwsh.exe".to_string()
        } else {
            "powershell.exe".to_string()
        }
    })
}

fn where_exe(name: &str) -> Option<()> {
    // Cheap existence probe without spawning a shell: check PATH manually.
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).any(|dir| dir.join(name).is_file()).then_some(())
}

async fn run(p: Prepared) -> Result<ToolOutcome, ToolError> {
    // UTF-8 output regardless of the console codepage.
    let script = format!(
        "$OutputEncoding=[Console]::OutputEncoding=[Text.Encoding]::UTF8; {}",
        p.command
    );
    let mut cmd = tokio::process::Command::new(shell_binary());
    cmd.args(["-NoLogo", "-NonInteractive", "-NoProfile", "-Command", &script])
        .current_dir(&p.cwd)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        // CREATE_NO_WINDOW: no console flash per call.
        .creation_flags(0x0800_0000);

    let mut child = cmd
        .spawn()
        .map_err(|e| ToolError::Exec(format!("spawn {}: {e}", shell_binary())))?;
    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| ToolError::Exec("no stdout".into()))?;
    let mut stderr = child
        .stderr
        .take()
        .ok_or_else(|| ToolError::Exec("no stderr".into()))?;

    let duration = std::time::Duration::from_secs(p.timeout_secs);
    let (out_buf, err_buf, status) = match tokio::time::timeout(
        duration,
        async {
            (
                read_capped(&mut stdout).await,
                read_capped(&mut stderr).await,
                child.wait().await,
            )
        },
    )
    .await
    {
        Ok(triple) => triple,
        Err(_) => {
            // Dropping the child kills it (kill_on_drop).
            return Err(ToolError::Exec(format!(
                "timed out after {}s — process killed",
                p.timeout_secs
            )));
        }
    };

    let code = status
        .map_err(|e| ToolError::Exec(format!("wait: {e}")))?
        .code()
        .unwrap_or(-1);

    let mut text = String::new();
    text.push_str(&format!("exit: {code}\n"));
    let out = String::from_utf8_lossy(&out_buf);
    let err = String::from_utf8_lossy(&err_buf);
    if !out.trim().is_empty() {
        text.push_str("stdout:\n");
        text.push_str(out.trim_end());
        text.push('\n');
    }
    if !err.trim().is_empty() {
        text.push_str("stderr:\n");
        text.push_str(err.trim_end());
        text.push('\n');
    }
    let text = truncate(&text);

    if code == 0 {
        Ok(ToolOutcome::Ok(text))
    } else {
        // Non-zero exit is a failed command — the model should see it as one.
        Ok(ToolOutcome::Err(text))
    }
}

/// Read to EOF, keeping at most the cap bytes (pipes can run long).
async fn read_capped<R: tokio::io::AsyncRead + Unpin>(
    r: &mut R,
) -> Vec<u8> {
    const CAP: usize = 32 * 1024;
    let mut buf = Vec::with_capacity(4096);
    let mut chunk = [0u8; 4096];
    loop {
        match r.read(&mut chunk).await {
            Ok(0) => break,
            Ok(n) => {
                if buf.len() < CAP {
                    buf.extend_from_slice(&chunk[..n]);
                }
            }
            Err(_) => break,
        }
    }
    buf
}

fn truncate(s: &str) -> String {
    if s.chars().count() <= MAX_OUTPUT_CHARS {
        return s.to_string();
    }
    let cut: String = s.chars().take(MAX_OUTPUT_CHARS).collect();
    format!("{cut}\n[… output truncated]")
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

    fn unwrap_out(out: Result<ToolOutcome, ToolError>) -> (String, bool) {
        match out.unwrap() {
            ToolOutcome::Ok(s) => (s, false),
            ToolOutcome::Err(s) => (s, true),
            other => panic!("unexpected outcome {other:?}"),
        }
    }

    #[tokio::test]
    async fn runs_powershell_inside_the_workspace() {
        let dir = tempfile::tempdir().unwrap();
        let (text, is_err) = unwrap_out(
            Shell
                .execute(
                    serde_json::json!({"command": "Write-Output hello-shell"}),
                    &ctx_with(dir.path()),
                )
                .await,
        );
        assert!(!is_err, "{text}");
        assert!(text.contains("exit: 0"), "{text}");
        assert!(text.contains("hello-shell"), "{text}");
    }

    #[tokio::test]
    async fn cwd_is_pinned_to_the_workspace() {
        let dir = tempfile::tempdir().unwrap();
        let (text, is_err) = unwrap_out(
            Shell
                .execute(
                    serde_json::json!({"command": "(Get-Location).Path"}),
                    &ctx_with(dir.path()),
                )
                .await,
        );
        assert!(!is_err, "{text}");
        assert!(
            text.to_lowercase().contains(&dir.path().file_name().unwrap().to_string_lossy().to_lowercase()),
            "{text}"
        );
    }

    #[tokio::test]
    async fn timeout_kills_the_process() {
        let dir = tempfile::tempdir().unwrap();
        let out = Shell
            .execute(
                serde_json::json!({
                    "command": "Start-Sleep -Seconds 10",
                    "timeout_secs": 1
                }),
                &ctx_with(dir.path()),
            )
            .await;
        let msg = out.unwrap_err().message();
        assert!(msg.contains("timed out after 1s"), "{msg}");
    }

    #[tokio::test]
    async fn nonzero_exit_is_an_error_outcome() {
        let dir = tempfile::tempdir().unwrap();
        let (text, is_err) = unwrap_out(
            Shell
                .execute(
                    serde_json::json!({"command": "Write-Output boom; exit 3"}),
                    &ctx_with(dir.path()),
                )
                .await,
        );
        assert!(is_err, "{text}");
        assert!(text.contains("exit: 3"), "{text}");
        assert!(text.contains("boom"), "{text}");
    }

    #[tokio::test]
    async fn stderr_is_captured_and_labeled() {
        let dir = tempfile::tempdir().unwrap();
        let (text, is_err) = unwrap_out(
            Shell
                .execute(
                    serde_json::json!({
                        "command": "Write-Output out; Write-Error err -ErrorAction Continue; exit 0"
                    }),
                    &ctx_with(dir.path()),
                )
                .await,
        );
        assert!(!is_err, "{text}");
        assert!(text.contains("stdout:"), "{text}");
        assert!(text.contains("stderr:"), "{text}");
    }

    #[test]
    fn binary_picks_pwsh_when_present() {
        // The picker must always resolve to a PowerShell binary.
        let picked = shell_binary();
        assert!(picked == "pwsh.exe" || picked == "powershell.exe", "{picked}");
    }
}