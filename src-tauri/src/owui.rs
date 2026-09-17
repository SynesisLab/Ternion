//! OpenWebUI Tools compatibility (design §6.5b, backlog): an OpenWebUI
//! "Tools" manifest — a Python `class Tools` whose public methods are the
//! tools — is parsed into OpenAI JSON-schema specs and merged namespaced
//! `owui__<manifest>__<method>`, the same adapter shape as MCP servers, so
//! the ecosystem's existing definitions load without core changes.
//!
//! Execution shells the method out to a local Python interpreter through a
//! generated runner (the manifest source with a small `__main__` block
//! appended). A missing interpreter surfaces as a tool error, not a crash;
//! every call gates through the §6.6 matrix (ask by default, grantable)
//! like MCP tools.

use crate::db::Database;
use crate::types::ToolSpec;

/// Every OpenWebUI tool name starts with this prefix — the namespace that
/// keeps a manifest's methods from shadowing bundled or MCP tools (§6.5b).
pub const OWUI_PREFIX: &str = "owui__";

const CALL_TIMEOUT_MS: u64 = 120_000;
const PROBE_TIMEOUT_MS: u64 = 5_000;

/// Interpreters tried in order. On Windows the `py` launcher needs `-3`.
#[cfg(windows)]
const PYTHON_CANDIDATES: &[&str] = &["python", "py", "python3"];
#[cfg(not(windows))]
const PYTHON_CANDIDATES: &[&str] = &["python3", "python"];

/// `owui__<manifest>__<method>` → (manifest, method). The method half may
/// itself contain `__`; the first split wins. Non-owui names → None.
pub fn parse_ref(name: &str) -> Option<(String, String)> {
    let rest = name.strip_prefix(OWUI_PREFIX)?;
    let (manifest, method) = rest.split_once("__")?;
    if manifest.is_empty() || method.is_empty() {
        return None;
    }
    Some((manifest.to_string(), method.to_string()))
}

// -- manifest parsing -------------------------------------------------------

/// One parsed method of a `class Tools` manifest.
#[derive(Debug)]
pub struct OwuiMethod {
    pub name: String,
    pub description: String,
    pub params: Vec<OwuiParam>,
}

#[derive(Debug)]
pub struct OwuiParam {
    pub name: String,
    /// JSON-schema type, when the annotation mapped cleanly.
    pub ty: Option<String>,
    pub required: bool,
    pub description: Option<String>,
}

/// Parse an OpenWebUI Tools manifest: every public method of the `class
/// Tools` (the OpenWebUI convention; the first class otherwise) becomes a
/// tool — docstring is the description, `:param name: desc` lines describe
/// parameters, type annotations map to JSON-schema types, and defaults make
/// a parameter optional. Line-based and deliberately tolerant: manifests
/// that don't parse come back as an error naming why.
pub fn parse_manifest(source: &str) -> Result<Vec<OwuiMethod>, String> {
    let normalized = source.replace("\r\n", "\n");
    let lines: Vec<&str> = normalized.lines().collect();

    // The tool class: `class Tools` per the convention, else the first
    // class in the file.
    let mut class_idx: Option<(usize, usize)> = None; // (line, indent)
    for (i, line) in lines.iter().enumerate() {
        let Some(rest) = line.strip_prefix("class ") else {
            continue;
        };
        let indent = line.len() - line.trim_start().len();
        let name = rest
            .split(|c: char| c == '(' || c == ':' || c.is_whitespace())
            .next()
            .unwrap_or("");
        if name == "Tools" {
            class_idx = Some((i, indent));
            break;
        }
        if class_idx.is_none() {
            class_idx = Some((i, indent));
        }
    }
    let Some((class_idx, class_indent)) = class_idx else {
        return Err("no Python class found in the manifest".into());
    };

    // Method candidates: def lines indented deeper than the class. The
    // shallowest indent level is the method level — deeper defs are nested
    // helpers inside method bodies.
    let mut defs: Vec<(usize, usize)> = Vec::new();
    for (i, line) in lines.iter().enumerate().skip(class_idx + 1) {
        let trimmed = line.trim_start();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if !trimmed.starts_with("def ") {
            continue;
        }
        let indent = line.len() - trimmed.len();
        if indent > class_indent {
            defs.push((i, indent));
        }
    }
    let method_indent = defs.iter().map(|(_, ind)| *ind).min();
    let Some(method_indent) = method_indent else {
        return Err("the class defines no public tool methods".into());
    };

    let mut methods = Vec::new();
    for (i, indent) in defs {
        if indent != method_indent {
            continue; // nested helper
        }
        let name = method_name(lines[i]);
        if name.starts_with('_') {
            continue; // private / dunder
        }
        let (sig, after_sig) = scan_signature(&lines, i);
        let params = parse_params(&sig);
        let (description, param_docs) = docstring_of(&lines, after_sig);
        let mut owned = params;
        for p in owned.iter_mut() {
            p.description = param_docs.get(&p.name).cloned();
        }
        methods.push(OwuiMethod {
            name,
            description,
            params: owned,
        });
    }
    if methods.is_empty() {
        return Err("the class defines no public tool methods".into());
    }
    Ok(methods)
}

/// The identifier after `def ` on a def line.
fn method_name(line: &str) -> String {
    let rest = line.trim_start().trim_start_matches("def ");
    rest.trim_start()
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
        .collect()
}

/// The parameter segment of a def: everything from the first `(` to its
/// matching `)`, following wrapped signature lines. The returned index is
/// the line after the signature (where the docstring would start).
fn scan_signature(lines: &[&str], def_idx: usize) -> (String, usize) {
    let mut out = String::new();
    let mut depth = 0i32;
    let mut started = false;
    let mut j = def_idx;
    while j < lines.len() {
        for ch in lines[j].chars() {
            match ch {
                '(' => {
                    depth += 1;
                    started = true;
                    out.push(ch);
                }
                '[' | '{' => {
                    depth += 1;
                    out.push(ch);
                }
                ')' | ']' | '}' => {
                    depth -= 1;
                    out.push(ch);
                    if started && depth == 0 {
                        return (out, j + 1);
                    }
                }
                _ => out.push(ch),
            }
        }
        out.push(' ');
        j += 1;
    }
    (out, j)
}

/// Split a signature's inner parameter list on top-level commas (quotes and
/// bracket nesting respected), then parse each piece.
fn parse_params(sig: &str) -> Vec<OwuiParam> {
    let inner = match (sig.find('('), sig.rfind(')')) {
        (Some(a), Some(b)) if b > a => &sig[a + 1..b],
        _ => return Vec::new(),
    };
    let mut out = Vec::new();
    for piece in split_top_level(inner, ',') {
        let piece = piece.trim();
        if piece.is_empty() || piece == "self" || piece == "cls" || piece.starts_with('*') {
            continue;
        }
        let Some(p) = parse_param(piece) else {
            continue;
        };
        out.push(p);
    }
    out
}

/// Split on `sep` at bracket depth 0, ignoring separators inside quotes.
fn split_top_level(s: &str, sep: char) -> Vec<String> {
    let mut parts = Vec::new();
    let mut cur = String::new();
    let mut depth = 0i32;
    let mut quote: Option<char> = None;
    let mut escaped = false;
    for ch in s.chars() {
        if let Some(q) = quote {
            cur.push(ch);
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == q {
                quote = None;
            }
            continue;
        }
        match ch {
            '"' | '\'' => {
                quote = Some(ch);
                cur.push(ch);
            }
            '(' | '[' | '{' => {
                depth += 1;
                cur.push(ch);
            }
            ')' | ']' | '}' => {
                depth -= 1;
                cur.push(ch);
            }
            c if c == sep && depth == 0 => {
                parts.push(cur.clone());
                cur.clear();
            }
            _ => cur.push(ch),
        }
    }
    parts.push(cur);
    parts
}

/// One parameter piece: `name[: type][ = default]`.
fn parse_param(piece: &str) -> Option<OwuiParam> {
    let mut depth = 0i32;
    let mut quote: Option<char> = None;
    let mut escaped = false;
    let mut colon_at: Option<usize> = None;
    let mut eq_at: Option<usize> = None;
    for (k, ch) in piece.char_indices() {
        if let Some(q) = quote {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == q {
                quote = None;
            }
            continue;
        }
        match ch {
            '"' | '\'' => quote = Some(ch),
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth -= 1,
            ':' if depth == 0 && colon_at.is_none() => colon_at = Some(k),
            '=' if depth == 0 && eq_at.is_none() => eq_at = Some(k),
            _ => {}
        }
    }
    let name_end = match (colon_at, eq_at) {
        (Some(a), Some(b)) => a.min(b),
        (Some(a), None) => a,
        (None, Some(b)) => b,
        (None, None) => piece.len(),
    };
    let name = piece[..name_end].trim().to_string();
    // OpenWebUI injects context params (dunder names) — they are not tools'
    // arguments and the model never sees them.
    let valid_start = matches!(name.chars().next(), Some(c) if c.is_ascii_alphabetic() || c == '_');
    if name.is_empty() || name.starts_with("__") || !valid_start {
        return None;
    }
    let ty = colon_at.map(|a| {
        let end = eq_at.unwrap_or(piece.len());
        piece[a + 1..end].trim().to_string()
    });
    let has_default = eq_at.is_some();
    let (mapped, optional) = ty.as_deref().map(json_type_of).unwrap_or((None, false));
    Some(OwuiParam {
        name,
        ty: mapped,
        required: !has_default && !optional,
        description: None,
    })
}

/// Map a Python annotation to a JSON-schema type. `Optional[X]` and
/// `X | None` unions map to X's type and mark the parameter optional.
fn json_type_of(ann: &str) -> (Option<String>, bool) {
    let ann = ann.trim();
    if let Some(inner) = ann
        .strip_prefix("Optional[")
        .and_then(|s| s.strip_suffix(']'))
    {
        let (t, _) = json_type_of(inner);
        return (t, true);
    }
    if ann.contains('|') {
        let mut ty: Option<String> = None;
        let mut optional = false;
        for part in ann.split('|') {
            let p = part.trim();
            if p == "None" {
                optional = true;
            } else if ty.is_none() {
                ty = json_type_of(p).0;
            }
        }
        return (ty, optional);
    }
    let plain = ann.trim_start_matches("typing.").trim();
    let mapped = match plain {
        "str" | "string" | "String" => Some("string".to_string()),
        "int" | "integer" | "Int" => Some("integer".to_string()),
        "float" | "number" | "Float" => Some("number".to_string()),
        "bool" | "boolean" | "Bool" => Some("boolean".to_string()),
        s if s.starts_with("list")
            || s.starts_with("List")
            || s.starts_with("Sequence")
            || s.starts_with("tuple")
            || s.starts_with("Tuple") =>
        {
            Some("array".to_string())
        }
        s if s.starts_with("dict")
            || s.starts_with("Dict")
            || s.starts_with("Mapping") =>
        {
            Some("object".to_string())
        }
        _ => None,
    };
    (mapped, false)
}

/// Docstring after a def: the lines before the first blank or `:param`
/// line are the description; `:param name: desc` lines map to parameters.
/// Returns (description, param docs).
fn docstring_of(lines: &[&str], from: usize) -> (String, std::collections::HashMap<String, String>) {
    // Skip blanks and comments to find the opening triple quote.
    let mut i = from;
    while i < lines.len() {
        let t = lines[i].trim_start();
        if t.is_empty() || t.starts_with('#') {
            i += 1;
            continue;
        }
        break;
    }
    let Some(open_line) = lines.get(i) else {
        return (String::new(), std::collections::HashMap::new());
    };
    let t = open_line.trim_start();
    for delim in ["\"\"\"", "'''"] {
        if let Some(rest) = t.strip_prefix(delim) {
            return match rest.find(delim) {
                Some(p) => parse_docstring_body(&rest[..p]),
                None => {
                    let mut body = String::from(rest);
                    body.push('\n');
                    let mut j = i + 1;
                    let mut closed = false;
                    while j < lines.len() {
                        if let Some(p) = lines[j].find(delim) {
                            body.push_str(&lines[j][..p]);
                            closed = true;
                            break;
                        }
                        body.push_str(lines[j]);
                        body.push('\n');
                        j += 1;
                    }
                    if !closed {
                        return (String::new(), std::collections::HashMap::new());
                    }
                    parse_docstring_body(&body)
                }
            };
        }
    }
    (String::new(), std::collections::HashMap::new())
}

fn parse_docstring_body(body: &str) -> (String, std::collections::HashMap<String, String>) {
    let mut desc_lines: Vec<&str> = Vec::new();
    let mut docs = std::collections::HashMap::new();
    // The description is the leading prose (ends at the first blank line);
    // `:param` lines may follow it, and prose after those stays out.
    let mut desc_done = false;
    for raw in body.lines() {
        let line = raw.trim();
        if let Some(rest) = line.strip_prefix(":param ") {
            desc_done = true;
            let rest = rest.trim();
            // `name:` or `name (type):`
            if let Some(colon) = rest.find(':') {
                let name_part = rest[..colon].trim();
                let name = name_part
                    .split_whitespace()
                    .next()
                    .unwrap_or_default()
                    .to_string();
                let desc = rest[colon + 1..].trim().to_string();
                if !name.is_empty() {
                    docs.insert(name, desc);
                }
            }
            continue;
        }
        if line.starts_with(':') {
            continue; // :return:, :raises: — not parameter docs
        }
        if line.is_empty() {
            if !desc_lines.is_empty() {
                desc_done = true;
            }
            continue;
        }
        if desc_done {
            continue; // trailing prose after the description/:param block
        }
        desc_lines.push(line);
    }
    let mut description = desc_lines.join(" ");
    if description.chars().count() > 500 {
        description = description.chars().take(500).collect::<String>() + "…";
    }
    (description, docs)
}

// -- specs ------------------------------------------------------------------

/// JSON-schema specs namespaced `owui__<manifest>__<method>`.
pub fn specs_for(manifest_name: &str, methods: &[OwuiMethod]) -> Vec<ToolSpec> {
    let ns = crate::mcp::sanitize_server_name(manifest_name);
    methods
        .iter()
        .map(|m| ToolSpec {
            name: format!("{OWUI_PREFIX}{ns}__{}", m.name),
            description: m.description.clone(),
            input_schema: schema_of(m),
        })
        .collect()
}

fn schema_of(m: &OwuiMethod) -> serde_json::Value {
    let mut props = serde_json::Map::new();
    let mut required = Vec::new();
    for p in &m.params {
        let mut prop = serde_json::Map::new();
        if let Some(t) = &p.ty {
            prop.insert("type".into(), serde_json::json!(t));
        }
        if let Some(d) = &p.description {
            prop.insert("description".into(), serde_json::json!(d));
        }
        props.insert(p.name.clone(), serde_json::Value::Object(prop));
        if p.required {
            required.push(p.name.clone());
        }
    }
    serde_json::json!({"type": "object", "properties": props, "required": required})
}

/// Specs for every enabled tool row; rows whose manifest no longer parses
/// are skipped with a warning (same policy as failing MCP servers).
pub async fn enabled_specs(db: &Database) -> Vec<ToolSpec> {
    let rows = db.list_owui_tools().await.unwrap_or_default();
    let mut out = Vec::new();
    for row in rows.iter().filter(|r| r.enabled) {
        match parse_manifest(&row.source) {
            Ok(methods) => out.extend(specs_for(&row.name, &methods)),
            Err(e) => log::warn!("owui: skipping tool `{}` — {e}", row.name),
        }
    }
    out
}

// -- execution --------------------------------------------------------------

/// Appended to the manifest source: dispatches argv[2] on `class Tools`
/// with argv[1] (JSON object) as keyword arguments, printing the result.
const RUNNER: &str = r#"

# --- Ternion tool runner (appended) ---
import json as _tjson
import sys as _tsys

if __name__ == "__main__":
    _args = _tjson.loads(_tsys.argv[1]) if len(_tsys.argv) > 1 else {}
    _inst = Tools()
    _fn = getattr(_inst, _tsys.argv[2], None)
    if _fn is None:
        _tsys.stderr.write("unknown method " + _tsys.argv[2])
        raise SystemExit(3)
    _out = _fn(**_args)
    _tsys.stdout.write(str(_out))
"#;

fn shim_source(manifest_source: &str) -> String {
    format!("{}{}", manifest_source.trim_end(), RUNNER)
}

/// Interpreters on this machine, most likely first. Windows ships the `py`
/// launcher with python.org installs; Microsoft Store installs provide
/// `python` (its no-op alias stub fails the probe and is skipped).
fn python_argv(py: &str, extra: &[String]) -> Vec<String> {
    let mut v: Vec<String> = Vec::new();
    if py == "py" {
        v.push("-3".into());
    }
    v.push("-X".into());
    v.push("utf8".into());
    v.extend(extra.iter().cloned());
    v
}

fn spawn_python(extra: Vec<String>) -> Result<tokio::process::Child, String> {
    let mut last = String::new();
    for py in PYTHON_CANDIDATES {
        let mut cmd = tokio::process::Command::new(py);
        cmd.args(python_argv(py, &extra));
        use std::process::Stdio;
        cmd.stdin(std::process::Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(windows)]
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW: no console flash
        match cmd.spawn() {
            Ok(child) => return Ok(child),
            Err(e) => last = format!("{py}: {e}"),
        }
    }
    Err(format!("no Python interpreter found ({last})"))
}

/// Run a spawned child to completion, collecting stdout/stderr, with a
/// hard kill on timeout. Returns (stdout, stderr, exit_code-or-minus).
async fn collect(
    child: tokio::process::Child,
    timeout_ms: u64,
) -> (String, String, Option<i32>) {
    use tokio::io::AsyncReadExt;
    let mut child = child;
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let out_task = tokio::spawn(async move {
        let mut buf = Vec::new();
        if let Some(mut s) = stdout {
            let _ = s.read_to_end(&mut buf).await;
        }
        buf
    });
    let err_task = tokio::spawn(async move {
        let mut buf = Vec::new();
        if let Some(mut s) = stderr {
            let _ = s.read_to_end(&mut buf).await;
        }
        buf
    });
    let status = match tokio::time::timeout(
        std::time::Duration::from_millis(timeout_ms),
        child.wait(),
    )
    .await
    {
        Ok(Ok(s)) => Some(s.code()),
        _ => {
            let _ = child.start_kill();
            let _ = tokio::time::timeout(
                std::time::Duration::from_millis(500),
                child.wait(),
            )
            .await;
            None
        }
    };
    let join = tokio::time::timeout(std::time::Duration::from_secs(2), async move {
        (out_task.await.unwrap_or_default(), err_task.await.unwrap_or_default())
    });
    let (out, err) = match join.await {
        Ok((o, e)) => (o, e),
        Err(_) => (Vec::new(), Vec::new()),
    };
    (
        String::from_utf8_lossy(&out).to_string(),
        String::from_utf8_lossy(&err).to_string(),
        status.flatten(),
    )
}

/// Execute one namespaced tool: resolve the enabled row, re-parse to
/// validate the method, then run it through the shim interpreter.
pub async fn call(
    db: &Database,
    namespaced: &str,
    args: serde_json::Value,
) -> Result<String, String> {
    let (manifest, method) = parse_ref(namespaced)
        .ok_or_else(|| format!("not an OpenWebUI tool name: {namespaced}"))?;
    let rows = db
        .list_owui_tools()
        .await
        .map_err(|e| format!("owui lookup failed: {e}"))?;
    let Some(row) = rows
        .iter()
        .find(|t| t.enabled && crate::mcp::sanitize_server_name(&t.name) == manifest)
    else {
        return Err(format!("no enabled OpenWebUI tool named `{manifest}`"));
    };
    let methods = parse_manifest(&row.source)
        .map_err(|e| format!("manifest `{}`: {e}", row.name))?;
    if !methods.iter().any(|m| m.name == method) {
        return Err(format!("`{}` has no method `{method}`", row.name));
    }

    let args_json = serde_json::to_string(&args).unwrap_or_else(|_| "{}".into());
    // Unique per-call temp file — two calls of one tool never collide, and
    // each call's copy is removed after the run.
    let dir = std::env::temp_dir().join("ternion-owui");
    let _ = tokio::fs::create_dir_all(&dir).await;
    let path = dir.join(format!("{}-{}.py", row.id, crate::ids::new_id()));
    if let Err(e) = tokio::fs::write(&path, shim_source(&row.source)).await {
        return Err(format!("owui script write failed: {e}"));
    }
    let script = path.to_string_lossy().to_string();
    let extra = vec![script, args_json, method];
    let child = spawn_python(extra);
    let (out, err, code) = match child {
        Ok(c) => collect(c, CALL_TIMEOUT_MS).await,
        Err(e) => {
            let _ = tokio::fs::remove_file(&path).await;
            return Err(e);
        }
    };
    let _ = tokio::fs::remove_file(&path).await;
    match code {
        Some(0) if out.trim().is_empty() => Ok("(empty result)".to_string()),
        Some(0) => Ok(out.trim().to_string()),
        Some(c) => {
            let err = err.trim();
            if err.is_empty() {
                Err(format!("tool exited with code {c}"))
            } else {
                Err(format!("tool failed: {err}"))
            }
        }
        None => Err("tool call timed out".into()),
    }
}

/// Parse a manifest and probe for a usable interpreter — the Save-dialog
/// test. Tool names are surfaced even when no interpreter exists, so the
/// user can see what would load.
pub async fn test_connect(source: &str) -> crate::types::OwuiTestResult {
    let tools = match parse_manifest(source) {
        Ok(methods) => methods.into_iter().map(|m| m.name).collect::<Vec<_>>(),
        Err(e) => {
            return crate::types::OwuiTestResult {
                ok: false,
                latency_ms: 0,
                tools: Vec::new(),
                error: Some(e),
            }
        }
    };
    let started = std::time::Instant::now();
    let extra = vec!["-c".to_string(), "print('ok')".to_string()];
    match spawn_python(extra) {
        Err(e) => crate::types::OwuiTestResult {
            ok: false,
            latency_ms: 0,
            tools,
            error: Some(e),
        },
        Ok(child) => {
            let (out, _err, _code) = collect(child, PROBE_TIMEOUT_MS).await;
            if out.trim() == "ok" {
                crate::types::OwuiTestResult {
                    ok: true,
                    latency_ms: started.elapsed().as_millis() as u64,
                    tools,
                    error: None,
                }
            } else {
                crate::types::OwuiTestResult {
                    ok: false,
                    latency_ms: 0,
                    tools,
                    error: Some("python did not respond as expected".into()),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
"""
title: Weather Tools
author: tester
version: 0.1
description: Demo manifest for parsing.
"""

class Tools:
    def get_weather(self, city: str, units: str = "metric", detail: bool = False) -> str:
        """
        Get the current weather for a city.

        :param city: The city name, e.g. Taipei
        :param units: metric or imperial
        :param detail: include hourly detail
        """
        return f"weather in {city}"

    def _helper(self, x: int) -> int:
        return x

    def stats(self, rows: list, meta: dict = None) -> str:
        """
        Summarize rows.
        :param rows: input rows
        :param meta: optional metadata
        """
        return "ok"
"#;

    #[test]
    fn parse_extracts_methods_params_and_docs() {
        let methods = parse_manifest(SAMPLE).unwrap();
        assert_eq!(methods.len(), 2, "private helpers skipped: {methods:?}");
        let weather = &methods[0];
        assert_eq!(weather.name, "get_weather");
        assert_eq!(weather.description, "Get the current weather for a city.");
        let names: Vec<&str> = weather.params.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, vec!["city", "units", "detail"]);
        let units = &weather.params[1];
        assert_eq!(units.ty.as_deref(), Some("string"));
        assert!(!units.required, "defaults make params optional");
        let city = &weather.params[0];
        assert!(city.required);
        assert_eq!(city.description.as_deref(), Some("The city name, e.g. Taipei"));
        assert_eq!(weather.params[2].ty.as_deref(), Some("boolean"));
        let stats = &methods[1];
        assert_eq!(stats.params[0].ty.as_deref(), Some("array"));
        assert_eq!(stats.params[1].ty.as_deref(), Some("object"));
        assert!(!stats.params[1].required);
    }

    #[test]
    fn specs_are_namespaced_with_json_schema() {
        let methods = parse_manifest(SAMPLE).unwrap();
        let specs = specs_for("Weather Tools", &methods);
        assert_eq!(specs[0].name, "owui__weather-tools__get_weather");
        let schema = &specs[0].input_schema;
        assert_eq!(schema["type"], "object");
        assert_eq!(schema["properties"]["city"]["type"], "string");
        assert_eq!(
            schema["properties"]["city"]["description"],
            "The city name, e.g. Taipei"
        );
        let required = schema["required"].as_array().unwrap();
        assert_eq!(required.len(), 1, "only city is required");
        assert_eq!(required[0], "city");
    }

    #[test]
    fn parse_rejects_garbage() {
        assert!(parse_manifest("no python here").is_err());
        assert!(parse_manifest("class Empty:\n    pass\n").is_err());
    }

    #[test]
    fn parse_handles_wrapped_signatures_and_untyped_params() {
        let src = "class Tools:\n    def run(self, task: str, mode,\n            priority=1) -> str:\n        \"\"\"Run a task.\n        :param task: what to run\n        \"\"\"\n        return task\n";
        let methods = parse_manifest(src).unwrap();
        let m = &methods[0];
        assert_eq!(m.name, "run");
        assert_eq!(m.params[0].ty.as_deref(), Some("string"));
        assert!(m.params[0].required);
        assert!(m.params[1].required, "untyped without default is required");
        assert!(m.params[1].ty.is_none());
        assert!(!m.params[2].required, "untyped with a default is optional");
        assert!(m.params[2].ty.is_none());
    }

    #[test]
    fn parse_tolerates_typing_generics_and_optional() {
        let src = "class Tools:\n    def go(self, ids: list[int], cache: Optional[dict] = None) -> str:\n        \"\"\"Doc.\"\"\"\n        return \"ok\"\n";
        let m = &parse_manifest(src).unwrap()[0];
        assert_eq!(m.params[0].ty.as_deref(), Some("array"));
        assert_eq!(m.params[1].ty.as_deref(), Some("object"));
        assert!(!m.params[1].required, "Optional marks the param optional");
    }

    #[test]
    fn parse_skips_openwebui_context_injections() {
        let src = "class Tools:\n    def pipe(self, user_message: str, __user__: dict = {}, __event_emitter__=None) -> str:\n        \"\"\"Pipe.\n        :param user_message: the message\n        \"\"\"\n        return user_message\n";
        let m = &parse_manifest(src).unwrap()[0];
        let names: Vec<&str> = m.params.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, vec!["user_message"], "dunder injections skipped");
    }

    #[test]
    fn parse_ref_splits_manifest_and_method() {
        assert_eq!(
            parse_ref("owui__weather-tools__get_weather"),
            Some(("weather-tools".into(), "get_weather".into()))
        );
        assert_eq!(parse_ref("mcp__x__y"), None);
        assert_eq!(parse_ref("fs_read"), None);
    }

    #[test]
    fn shim_appends_the_runner() {
        let shim = shim_source("class Tools:\n    pass\n");
        assert!(shim.starts_with("class Tools:"));
        assert!(shim.contains("_fn(**_args)"));
        assert!(shim.contains("Ternion tool runner"));
    }

    #[tokio::test]
    async fn test_connect_reports_parse_failures_without_python() {
        let bad = test_connect("def broken(:").await;
        assert!(!bad.ok);
        assert!(bad.tools.is_empty());
        assert!(bad.error.is_some());
    }
}