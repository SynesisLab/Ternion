//! OpenWebUI compatibility (design §6.5b/§6.5d): an OpenWebUI manifest — a
//! Python class whose entry points Ternion maps onto its own surfaces — is
//! parsed per use and executed through a generated shim (the manifest source
//! with a small `__main__` block appended) run by a local Python
//! interpreter. A missing interpreter surfaces as an error, not a crash.
//!
//! Three manifest kinds:
//! - **Skills / Tools** (`class Tools`): every public method becomes a tool,
//!   merged namespaced `owui__<manifest>__<method>` like MCP servers and
//!   gated through the §6.6 matrix (ask by default, grantable).
//! - **Filters** (`class Filter`, Functions): `inlet` (and optional
//!   `outlet`) middleware — the inlet transforms the assembled request body
//!   before the model sees it. Best-effort: a failing filter is skipped.
//! - **Pipes** (`class Pipe`, Functions): the `pipe` method runs as a
//!   pseudo-model behind a synthetic endpoint (`PIPE_ENDPOINT`).

use crate::db::Database;
use crate::types::{ChatContent, ChatMessage, ChatRole, ToolSpec};

/// Every OpenWebUI tool name starts with this prefix — the namespace that
/// keeps a manifest's methods from shadowing bundled or MCP tools (§6.5b).
pub const OWUI_PREFIX: &str = "owui__";

/// Pipe pseudo-models (§6.5d) are surfaced as `pipe__<sanitized>@<PIPE_ENDPOINT>`
/// — a bare `pipe__` prefix plus a synthetic endpoint id that
/// `AppState::provider_for` special-cases. The prefix keeps them from ever
/// colliding with real model names on real endpoints.
pub const PIPE_PREFIX: &str = "pipe__";

/// The synthetic endpoint every enabled Pipe manifest serves (§6.5d). Never
/// probed, never listed among real endpoints; `provider_for` special-cases
/// it.
pub const PIPE_ENDPOINT: &str = "ep_ternion_pipes";

/// Which surface a manifest exposes (§6.5d). Kinds key the stored row
/// (`owui_tools.kind`) and the save-time validation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManifestKind {
    /// `class Tools` — model-callable methods (OpenWebUI Skills).
    Tools,
    /// `class Filter` — inlet/outlet request middleware (Functions).
    Filter,
    /// `class Pipe` — a pseudo-model (Functions).
    Pipe,
}

impl ManifestKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Tools => "tools",
            Self::Filter => "filter",
            Self::Pipe => "pipe",
        }
    }
}

/// Parse a stored kind string; unknown values fall back to `tools` (the
/// original rows' semantics).
pub fn parse_kind(kind: &str) -> ManifestKind {
    match kind.trim().to_ascii_lowercase().as_str() {
        "filter" => ManifestKind::Filter,
        "pipe" => ManifestKind::Pipe,
        _ => ManifestKind::Tools,
    }
}

/// Detect a manifest's kind from its class name — `class Filter` / `class
/// Pipe` anywhere in the source wins over the Tools default.
pub fn detect_kind(source: &str) -> ManifestKind {
    for line in source.replace("\r\n", "\n").lines() {
        let Some(rest) = line.trim_start().strip_prefix("class ") else {
            continue;
        };
        match rest
            .split(|c: char| c == '(' || c == ':' || c.is_whitespace())
            .next()
            .unwrap_or("")
        {
            "Filter" => return ManifestKind::Filter,
            "Pipe" => return ManifestKind::Pipe,
            _ => {}
        }
    }
    ManifestKind::Tools
}

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

    let (class_idx, class_indent) = find_class_preferred(&lines, "Tools")?;

    let defs = method_defs(&lines, class_idx, class_indent);
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

/// The class a Tools manifest's methods live in: `preferred` (the OpenWebUI
/// convention) when present, else the first class in the file. (line, indent).
fn find_class_preferred(lines: &[&str], preferred: &str) -> Result<(usize, usize), String> {
    let mut first: Option<(usize, usize)> = None;
    for (i, line) in lines.iter().enumerate() {
        let Some(rest) = line.strip_prefix("class ") else {
            continue;
        };
        let indent = line.len() - line.trim_start().len();
        let name = rest
            .split(|c: char| c == '(' || c == ':' || c.is_whitespace())
            .next()
            .unwrap_or("");
        if name == preferred {
            return Ok((i, indent));
        }
        if first.is_none() {
            first = Some((i, indent));
        }
    }
    first.ok_or_else(|| "no Python class found in the manifest".to_string())
}

/// The class line of an exact Functions manifest (`class Filter`, `class
/// Pipe`) — unlike Tools there is no first-class fallback: the OpenWebUI
/// convention name is required.
fn find_class_named(lines: &[&str], name: &str) -> Option<(usize, usize)> {
    for (i, line) in lines.iter().enumerate() {
        let Some(rest) = line.trim_start().strip_prefix("class ") else {
            continue;
        };
        let indent = line.len() - line.trim_start().len();
        let class = rest
            .split(|c: char| c == '(' || c == ':' || c.is_whitespace())
            .next()
            .unwrap_or("");
        if class == name {
            return Some((i, indent));
        }
    }
    None
}

/// def lines of the class at (class_idx, class_indent), indented deeper than
/// it — (line index, indent). Deeper defs inside method bodies are filtered
/// later by the shallowest-indent rule.
fn method_defs(lines: &[&str], class_idx: usize, class_indent: usize) -> Vec<(usize, usize)> {
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
    defs
}

/// The entry points of an OpenWebUI Filter (§6.5d): `inlet` is required, the
/// (not yet executed) `outlet` is optional.
#[derive(Debug, Clone, Copy)]
pub struct FilterManifest {
    pub inlet: bool,
    pub outlet: bool,
}

/// Parse an OpenWebUI Filter: a `class Filter` whose `inlet` method
/// transforms the request body (the optional `outlet` is recognized but not
/// executed — §6.5d v1 runs inlets only).
pub fn parse_filter(source: &str) -> Result<FilterManifest, String> {
    let normalized = source.replace("\r\n", "\n");
    let lines: Vec<&str> = normalized.lines().collect();
    let Some((class_idx, class_indent)) = find_class_named(&lines, "Filter") else {
        return Err("no `class Filter` found in the manifest".into());
    };
    let defs = method_defs(&lines, class_idx, class_indent);
    let method_indent = defs.iter().map(|(_, ind)| *ind).min();
    let Some(method_indent) = method_indent else {
        return Err("`class Filter` defines no methods".into());
    };
    let mut manifest = FilterManifest {
        inlet: false,
        outlet: false,
    };
    for (i, indent) in defs {
        if indent != method_indent {
            continue; // nested helper
        }
        match method_name(lines[i]).as_str() {
            "inlet" => manifest.inlet = true,
            "outlet" => manifest.outlet = true,
            _ => {}
        }
    }
    if !manifest.inlet {
        return Err("a Filter needs an `inlet` method (OpenWebUI Functions format)".into());
    }
    Ok(manifest)
}

/// Validate an OpenWebUI Pipe (§6.5d): a `class Pipe` with a `pipe` method —
/// the entry point that runs as a pseudo-model.
pub fn parse_pipe(source: &str) -> Result<(), String> {
    let normalized = source.replace("\r\n", "\n");
    let lines: Vec<&str> = normalized.lines().collect();
    let Some((class_idx, class_indent)) = find_class_named(&lines, "Pipe") else {
        return Err("no `class Pipe` found in the manifest".into());
    };
    let defs = method_defs(&lines, class_idx, class_indent);
    let method_indent = defs.iter().map(|(_, ind)| *ind).min();
    let Some(method_indent) = method_indent else {
        return Err("`class Pipe` defines no methods".into());
    };
    if !defs.iter().any(|(i, indent)| {
        *indent == method_indent && method_name(lines[*i]) == "pipe"
    }) {
        return Err("a Pipe needs a `pipe` method (OpenWebUI Functions format)".into());
    }
    Ok(())
}

/// A Pipe manifest's pseudo-model id: `pipe__<sanitized name>`.
pub fn pipe_ref(name: &str) -> String {
    format!("{PIPE_PREFIX}{}", crate::mcp::sanitize_server_name(name))
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

/// Specs for every enabled Tools row (§6.5b); rows whose manifest no longer
/// parses are skipped with a warning (same policy as failing MCP servers).
/// Filters and Pipes are middleware/pseudo-models (§6.5d), not model-callable
/// tools — their rows never merge into the tool surface.
pub async fn enabled_specs(db: &Database) -> Vec<ToolSpec> {
    let rows = db.list_owui_tools().await.unwrap_or_default();
    let mut out = Vec::new();
    for row in rows.iter().filter(|r| r.enabled && r.kind == "tools") {
        match parse_manifest(&row.source) {
            Ok(methods) => out.extend(specs_for(&row.name, &methods)),
            Err(e) => log::warn!("owui: skipping tool `{}` — {e}", row.name),
        }
    }
    out
}

// -- execution --------------------------------------------------------------

/// Appended to a Tools manifest: dispatches argv[2] on `class Tools` with
/// argv[1] (JSON object) as keyword arguments, printing the result.
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

/// Appended to a Filter manifest (§6.5d): dispatches argv[2] on the class
/// named argv[3] with argv[1] (the body JSON) as its single positional
/// argument — the OpenWebUI Functions calling convention — and prints the
/// result JSON-serialized. An empty output means the function returned
/// None: the OpenWebUI convention for "no change".
const FUNCTION_RUNNER: &str = r#"

# --- Ternion function runner (appended) ---
import json as _fjson
import sys as _fsys

if __name__ == "__main__":
    _args = _fjson.loads(_fsys.argv[1]) if len(_fsys.argv) > 1 else {}
    _cls = globals().get(_fsys.argv[3])
    if _cls is None:
        _fsys.stderr.write("no class " + _fsys.argv[3])
        raise SystemExit(3)
    _inst = _cls()
    _fn = getattr(_inst, _fsys.argv[2], None)
    if _fn is None:
        _fsys.stderr.write("unknown method " + _fsys.argv[2])
        raise SystemExit(3)
    _out = _fn(_args)
    _fsys.stdout.write("" if _out is None else _fjson.dumps(_out))
"#;

/// Appended to a Pipe manifest (§6.5d): calls `pipe(*args)` where argv[1] is
/// a JSON array of the OpenWebUI Pipe's positional arguments —
/// `[user_message, model_id, messages, body]`. The result prints verbatim
/// (pipes return the answer text; None means no output).
const PIPE_RUNNER: &str = r#"

# --- Ternion pipe runner (appended) ---
import json as _pjson
import sys as _psys

if __name__ == "__main__":
    _args = _pjson.loads(_psys.argv[1]) if len(_psys.argv) > 1 else []
    if not isinstance(_args, list):
        _args = [_args]
    _cls = globals().get("Pipe")
    if _cls is None:
        _psys.stderr.write("no class Pipe")
        raise SystemExit(3)
    _inst = _cls()
    _fn = getattr(_inst, "pipe", None)
    if _fn is None:
        _psys.stderr.write("unknown method pipe")
        raise SystemExit(3)
    _out = _fn(*_args)
    _psys.stdout.write("" if _out is None else str(_out))
"#;

fn shim_source(manifest_source: &str, runner: &str) -> String {
    format!("{}{}", manifest_source.trim_end(), runner)
}

/// Write a shim to a unique temp file, run it with `extra` appended after
/// the script path, collect the output with a hard kill on timeout, and
/// remove the file. The `stem` keeps temp names readable per caller.
async fn run_shim(
    stem: &str,
    source: &str,
    runner: &str,
    extra: Vec<String>,
    timeout_ms: u64,
) -> Result<(String, String, Option<i32>), String> {
    // Unique per-call temp file — two calls of one manifest never collide,
    // and each call's copy is removed after the run.
    let dir = std::env::temp_dir().join("ternion-owui");
    let _ = tokio::fs::create_dir_all(&dir).await;
    let path = dir.join(format!("{stem}-{}.py", crate::ids::new_id()));
    if let Err(e) = tokio::fs::write(&path, shim_source(source, runner)).await {
        return Err(format!("script write failed: {e}"));
    }
    let script = path.to_string_lossy().to_string();
    let mut argv = vec![script];
    argv.extend(extra);
    let child = match spawn_python(argv) {
        Ok(c) => c,
        Err(e) => {
            let _ = tokio::fs::remove_file(&path).await;
            return Err(e);
        }
    };
    let (out, err, code) = collect(child, timeout_ms).await;
    let _ = tokio::fs::remove_file(&path).await;
    Ok((out, err, code))
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
    if row.kind != "tools" {
        return Err(format!(
            "`{}` is a {} manifest, not a tool",
            row.name,
            row.kind
        ));
    }
    let methods = parse_manifest(&row.source)
        .map_err(|e| format!("manifest `{}`: {e}", row.name))?;
    if !methods.iter().any(|m| m.name == method) {
        return Err(format!("`{}` has no method `{method}`", row.name));
    }

    let args_json = serde_json::to_string(&args).unwrap_or_else(|_| "{}".into());
    let extra = vec![args_json, method];
    let (out, err, code) = run_shim(&row.id, &row.source, RUNNER, extra, CALL_TIMEOUT_MS).await?;
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

/// Run one OpenWebUI Function (§6.5d): a Filter's inlet/outlet dispatched on
/// `class Filter` with the body JSON as its positional argument. Returns the
/// JSON-serialized result; empty output means the function returned None
/// (no change). Pipes go through [`run_pipe`] — their positional-argument
/// convention differs.
async fn run_function(source: &str, method: &str, body_json: String) -> Result<String, String> {
    let extra = vec![body_json, method.to_string(), "Filter".to_string()];
    let (out, err, code) =
        run_shim("filter", source.trim_end(), FUNCTION_RUNNER, extra, CALL_TIMEOUT_MS).await?;
    match code {
        Some(0) => Ok(out.trim().to_string()),
        Some(c) => {
            let err = err.trim();
            if err.is_empty() {
                Err(format!("filter `{method}` exited with code {c}"))
            } else {
                Err(format!("filter `{method}` failed: {err}"))
            }
        }
        None => Err(format!("filter `{method}` timed out")),
    }
}

// -- Filters (§6.5d): inlet middleware --------------------------------------

/// The role name an inlet body carries for a `ChatRole`.
fn role_name(role: ChatRole) -> &'static str {
    match role {
        ChatRole::System => "system",
        ChatRole::User => "user",
        ChatRole::Assistant => "assistant",
        ChatRole::Tool => "tool",
    }
}

/// Flattened text of a message's content — the inlet body's `content` shape
/// (filters receive strings; image parts contribute a placeholder).
fn flatten_parts(content: &ChatContent) -> String {
    match content {
        ChatContent::Text(t) => t.clone(),
        ChatContent::Parts(parts) => {
            let mut out = String::new();
            for part in parts {
                match part {
                    crate::types::ContentPart::Text { text } => {
                        if !out.is_empty() {
                            out.push_str("\n\n");
                        }
                        out.push_str(text);
                    }
                    crate::types::ContentPart::Image { .. } => {
                        if !out.is_empty() {
                            out.push_str("\n\n");
                        }
                        out.push_str("[image attachment]");
                    }
                }
            }
            out
        }
    }
}

/// The inlet body's `messages` view (§6.5d): plain `{role, content}` string
/// objects — what an OpenWebUI filter sees.
fn inlet_messages_view(messages: &[ChatMessage]) -> Vec<serde_json::Value> {
    messages
        .iter()
        .map(|m| {
            serde_json::json!({
                "role": role_name(m.role),
                "content": flatten_parts(&m.content),
            })
        })
        .collect()
}

/// The text a filter returned for one message: a string, or a parts array
/// whose `text` items join (image parts contribute nothing).
fn text_of(item: &serde_json::Value) -> Option<String> {
    match item {
        serde_json::Value::String(s) => Some(s.clone()),
        serde_json::Value::Array(items) => {
            let mut out = String::new();
            for it in items {
                if it.get("type").and_then(|t| t.as_str()) == Some("text") {
                    if let Some(text) = it.get("text").and_then(|t| t.as_str()) {
                        if !out.is_empty() {
                            out.push_str("\n\n");
                        }
                        out.push_str(text);
                    }
                }
            }
            Some(out)
        }
        _ => None,
    }
}

/// Apply one filter's returned body to the message list (§6.5d), kept
/// deliberately narrow — a filter may edit message contents in place and
/// append new tail messages, but never restructure history:
/// - in-place edits apply to system/user messages only — assistant/tool rows
///   carry the tool-call pairing the providers validate, so their rewrites
///   pass through untouched;
/// - appended tail messages (system/user) join the end;
/// - removals, reorders, and role changes are ignored with a warning.
/// The `model` key of a returned body is out of scope (v1).
fn apply_inlet_result(
    messages: &mut Vec<ChatMessage>,
    result: &serde_json::Map<String, serde_json::Value>,
    name: &str,
) {
    let Some(new) = result.get("messages").and_then(|v| v.as_array()) else {
        return;
    };
    if new.len() < messages.len() {
        log::warn!("owui filter `{name}` removed messages; ignored");
        return;
    }
    for (i, msg) in messages.iter_mut().enumerate() {
        let Some(item) = new.get(i) else { break };
        let role = item.get("role").and_then(|r| r.as_str());
        if role != Some(role_name(msg.role)) {
            log::warn!("owui filter `{name}` changed message {i}'s role; ignored");
            return;
        }
        if !matches!(msg.role, ChatRole::System | ChatRole::User) {
            continue;
        }
        let Some(text) = item.get("content").and_then(text_of) else {
            continue;
        };
        if text == flatten_parts(&msg.content) {
            continue; // unchanged — keep the original (image parts survive)
        }
        msg.content = ChatContent::Text(text);
    }
    for item in new[messages.len()..].iter() {
        let Some(text) = item.get("content").and_then(|c| c.as_str()) else {
            continue;
        };
        let role = match item.get("role").and_then(|r| r.as_str()) {
            Some("system") => ChatRole::System,
            Some("user") => ChatRole::User,
            other => {
                log::warn!(
                    "owui filter `{name}` appended a `{}` message; ignored",
                    other.unwrap_or("unnamed")
                );
                continue;
            }
        };
        messages.push(ChatMessage {
            id: crate::ids::new_id(),
            role,
            content: ChatContent::Text(text.to_string()),
            tool_calls: None,
            tool_call_id: None,
            tool_name: None,
        });
    }
}

/// §6.5d: run every enabled Filter's `inlet` over the assembled request
/// body, in stored order — each filter sees the previous filter's output.
/// Best-effort like compression: a missing interpreter, a failing or
/// non-conforming filter logs a warning and is skipped; it never blocks the
/// send.
pub async fn run_inlets(db: &Database, model_ref: &str, messages: &mut Vec<ChatMessage>) {
    let rows = db.list_owui_tools().await.unwrap_or_default();
    for row in rows.iter().filter(|r| r.enabled && r.kind == "filter") {
        if let Err(e) = parse_filter(&row.source) {
            log::warn!("owui filter `{}` skipped: {e}", row.name);
            continue;
        }
        let body = serde_json::json!({
            "model": model_ref,
            "messages": inlet_messages_view(messages),
        })
        .to_string();
        let out = match run_function(&row.source, "inlet", body).await {
            Ok(out) => out,
            Err(e) => {
                log::warn!("owui filter `{}` inlet failed: {e}", row.name);
                continue;
            }
        };
        if out.is_empty() {
            continue; // returned None — the OpenWebUI convention for no change
        }
        let Ok(serde_json::Value::Object(result)) = serde_json::from_str(&out) else {
            log::warn!(
                "owui filter `{}` inlet returned a non-object; ignored",
                row.name
            );
            continue;
        };
        apply_inlet_result(messages, &result, &row.name);
    }
}

// -- Pipes (§6.5d): pseudo-models -------------------------------------------

const PIPE_TIMEOUT_MS: u64 = 300_000;

/// Run a Pipe manifest's `pipe` method (§6.5d) with the OpenWebUI positional
/// convention — `pipe(user_message, model_id, messages, body)` — and return
/// its printed output (the answer text; empty means the pipe returned None).
async fn run_pipe(source: &str, args_json: &str) -> Result<String, String> {
    let extra = vec![args_json.to_string()];
    let (out, err, code) = run_shim("pipe", source.trim_end(), PIPE_RUNNER, extra, PIPE_TIMEOUT_MS).await?;
    match code {
        Some(0) => Ok(out),
        Some(c) => {
            let err = err.trim();
            if err.is_empty() {
                Err(format!("pipe exited with code {c}"))
            } else {
                Err(format!("pipe failed: {err}"))
            }
        }
        None => Err("pipe call timed out".into()),
    }
}

/// Slice a pipe's finished output into delta-sized chunks so the UI's
/// incremental rendering stays honest — the whole text is known up front.
const PIPE_CHUNK_CHARS: usize = 512;

/// A Pipe pseudo-model (§6.5d): an enabled `class Pipe` manifest runs as if
/// it were a model — `AppState::provider_for(PIPE_ENDPOINT)` hands the
/// request here, the `pipe` method runs through the Python shim, and its
/// result streams as text chunks. A local process produces its whole output
/// before Ternion sees any of it, so the text arrives in a burst at the end;
/// token usage is unknown. Cancellation is honored between chunks.
pub struct PipeProvider {
    db: Database,
}

impl PipeProvider {
    pub fn new(db: Database) -> Self {
        Self { db }
    }
}

impl crate::providers::Provider for PipeProvider {
    fn id(&self) -> &str {
        PIPE_ENDPOINT
    }

    fn kind(&self) -> crate::providers::EndpointKind {
        crate::providers::EndpointKind::Ollama
    }

    /// Pipe discovery: the enabled Pipe manifests' display names.
    fn list_models(
        &self,
    ) -> futures::future::BoxFuture<'_, Result<Vec<crate::types::ModelInfo>, crate::providers::ProviderError>>
    {
        let db = self.db.clone();
        Box::pin(async move {
            let rows = db.list_owui_tools().await.map_err(|e| {
                crate::providers::ProviderError::Malformed(format!("pipe lookup failed: {e}"))
            })?;
            Ok(rows
                .iter()
                .filter(|r| r.enabled && r.kind == "pipe")
                .map(|r| crate::types::ModelInfo {
                    id: pipe_ref(&r.name),
                    display_name: r.name.clone(),
                    endpoint_id: PIPE_ENDPOINT.to_string(),
                    size_bytes: None,
                    parameter_size: None,
                    quantization_level: None,
                    family: Some("openwebui-pipe".to_string()),
                    context_length: None,
                    capabilities: Vec::new(),
                })
                .collect())
        })
    }

    fn chat(
        &self,
        req: crate::types::ChatRequest,
        events: tokio::sync::mpsc::Sender<crate::types::StreamEvent>,
        cancel: tokio_util::sync::CancellationToken,
    ) -> futures::future::BoxFuture<'_, ()> {
        let db = self.db.clone();
        Box::pin(async move {
            use crate::types::StreamEvent;
            let _ = events
                .send(StreamEvent::Status {
                    phase: crate::types::StatusPhase::Connecting,
                })
                .await;
            let bare = req
                .model
                .strip_prefix(PIPE_PREFIX)
                .unwrap_or(&req.model)
                .to_string();
            let rows = db.list_owui_tools().await.unwrap_or_default();
            let Some(row) = rows
                .into_iter()
                .find(|r| r.enabled && r.kind == "pipe" && crate::mcp::sanitize_server_name(&r.name) == bare)
            else {
                let _ = events
                    .send(StreamEvent::Error {
                        code: "pipe".into(),
                        message: format!("no enabled OpenWebUI pipe named `{bare}`"),
                        retryable: false,
                    })
                    .await;
                return;
            };
            let user_message = req
                .messages
                .iter()
                .rev()
                .find(|m| m.role == ChatRole::User)
                .map(|m| flatten_parts(&m.content))
                .unwrap_or_default();
            let body = serde_json::json!({
                "model": req.model,
                "messages": inlet_messages_view(&req.messages),
            });
            // The OpenWebUI Pipe convention: positional (user_message,
            // model_id, messages, body). model_id stays empty — Ternion v1
            // exposes one entry point per manifest, no pipe-internal
            // sub-model selection.
            let args = serde_json::json!([
                user_message,
                "",
                inlet_messages_view(&req.messages),
                body,
            ])
            .to_string();
            let result = run_pipe(&row.source, &args).await;
            if cancel.is_cancelled() {
                return;
            }
            match result {
                Ok(text) => {
                    for chunk in slice_chunks(&text, PIPE_CHUNK_CHARS) {
                        if cancel.is_cancelled() {
                            return;
                        }
                        if events
                            .send(StreamEvent::TextDelta { text: chunk.to_string() })
                            .await
                            .is_err()
                        {
                            return; // the consumer is gone — stop
                        }
                    }
                    let _ = events.send(StreamEvent::Done).await;
                }
                Err(e) => {
                    let _ = events
                        .send(StreamEvent::Error {
                            code: "pipe".into(),
                            message: e,
                            retryable: false,
                        })
                        .await;
                }
            }
        })
    }
}

/// UTF-8-safe chunks of at most `max` chars — never splits a codepoint.
fn slice_chunks(text: &str, max: usize) -> Vec<&str> {
    if text.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut start = 0;
    while start < text.len() {
        let end = (start + max).min(text.len());
        let mut end = end;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        out.push(&text[start..end]);
        start = end;
    }
    out
}

/// Parse a manifest and probe for a usable interpreter — the Save-dialog
/// test. Entry points are surfaced even when no interpreter exists, so the
/// user can see what would load once one is available.
pub async fn test_connect(source: &str) -> crate::types::OwuiTestResult {
    fn failure(error: String) -> crate::types::OwuiTestResult {
        crate::types::OwuiTestResult {
            ok: false,
            latency_ms: 0,
            tools: Vec::new(),
            error: Some(error),
        }
    }
    let entry_points = match detect_kind(source) {
        ManifestKind::Filter => match parse_filter(source) {
            Ok(f) => {
                let mut v = vec!["inlet".to_string()];
                if f.outlet {
                    v.push("outlet".to_string());
                }
                v
            }
            Err(e) => return failure(e),
        },
        ManifestKind::Pipe => match parse_pipe(source) {
            Ok(()) => vec!["pipe".to_string()],
            Err(e) => return failure(e),
        },
        ManifestKind::Tools => match parse_manifest(source) {
            Ok(methods) => methods.into_iter().map(|m| m.name).collect::<Vec<_>>(),
            Err(e) => return failure(e),
        },
    };
    let tools = entry_points;
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
        let shim = shim_source("class Tools:\n    pass\n", RUNNER);
        assert!(shim.starts_with("class Tools:"));
        assert!(shim.contains("_fn(**_args)"));
        assert!(shim.contains("Ternion tool runner"));
    }

    const FILTER_SAMPLE: &str = r#"
class Filter:
    def inlet(self, body: dict) -> dict:
        body["messages"].append({"role": "system", "content": "be brief"})
        return body

    def outlet(self, body: dict) -> dict:
        return body

    def _helper(self, x: int) -> int:
        return x
"#;

    #[test]
    fn detect_kind_reads_the_class_name() {
        assert_eq!(detect_kind(SAMPLE), ManifestKind::Tools);
        assert_eq!(detect_kind(FILTER_SAMPLE), ManifestKind::Filter);
        assert_eq!(detect_kind("class Pipe:\n    def pipe(self): ..."), ManifestKind::Pipe);
        assert_eq!(detect_kind("class Foo:\n    pass\n"), ManifestKind::Tools);
        assert_eq!(
            parse_kind("Filter"),
            ManifestKind::Filter,
            "stored kinds parse case-insensitively"
        );
        assert_eq!(parse_kind("  tools "), ManifestKind::Tools);
        assert_eq!(parse_kind("nonsense"), ManifestKind::Tools);
    }

    #[test]
    fn parse_filter_finds_inlet_and_outlet() {
        let f = parse_filter(FILTER_SAMPLE).unwrap();
        assert!(f.inlet);
        assert!(f.outlet);
    }

    #[test]
    fn parse_filter_requires_the_filter_class_and_inlet() {
        assert!(parse_filter("class Tools:\n    def inlet(self, body): return body\n").is_err());
        assert!(parse_filter("class Filter:\n    def other(self, body): return body\n").is_err());
    }

    #[test]
    fn parse_pipe_validates_the_pipe_method() {
        assert!(parse_pipe("class Pipe:\n    def pipe(self, user_message: str, model_id: str, messages: list, body: dict) -> str:\n        return user_message\n").is_ok());
        assert!(parse_pipe("class Pipe:\n    def other(self, x): return x\n").is_err());
        assert!(parse_pipe("class Filter:\n    def pipe(self, m): return m\n").is_err());
    }

    #[test]
    fn pipe_refs_are_namespaced_and_sanitized() {
        assert_eq!(pipe_ref("My Pipe!"), "pipe__my-pipe");
        assert_eq!(pipe_ref("天氣 Pipe"), "pipe__pipe", "CJK names keep their ascii tail");
        assert_eq!(
            parse_ref(&pipe_ref("Weather Tools")).map(|(m, _)| m),
            None,
            "pipe ids are not owui tool refs"
        );
    }

    fn user_msg(text: &str) -> ChatMessage {
        ChatMessage {
            id: crate::ids::new_id(),
            role: ChatRole::User,
            content: ChatContent::Text(text.into()),
            tool_calls: None,
            tool_call_id: None,
            tool_name: None,
        }
    }

    fn apply(filter_out: &str, messages: &mut Vec<ChatMessage>) {
        let serde_json::Value::Object(result) = serde_json::from_str(filter_out).unwrap() else {
            panic!("test output is always an object");
        };
        apply_inlet_result(messages, &result, "f");
    }

    #[test]
    fn inlet_edits_apply_to_system_and_user_in_place() {
        let mut messages = vec![
            ChatMessage {
                id: "s1".into(),
                role: ChatRole::System,
                content: ChatContent::Text("be helpful".into()),
                tool_calls: None,
                tool_call_id: None,
                tool_name: None,
            },
            user_msg("hi"),
        ];
        let view = inlet_messages_view(&messages);
        let json = serde_json::json!([
            {"role": "system", "content": "be helpful"},
            {"role": "user", "content": "hello there"},
        ]);
        let out = serde_json::json!({"model": "m", "messages": json}).to_string();
        apply(&out, &mut messages);
        assert_eq!(
            messages[0].content,
            ChatContent::Text("be helpful".into()),
            "unchanged system keeps its original"
        );
        assert_eq!(
            messages[1].content,
            ChatContent::Text("hello there".into()),
            "edited user content applies"
        );
        // A filter's edit rides into the next filter's body view.
        assert_eq!(
            serde_json::to_value(inlet_messages_view(&messages)).unwrap()[1]["content"],
            "hello there"
        );
    }

    #[test]
    fn inlet_appends_system_and_user_tails() {
        let mut messages = vec![user_msg("hi")];
        let out = serde_json::json!({
            "messages": [
                {"role": "user", "content": "hi"},
                {"role": "system", "content": "context: X"},
                {"role": "user", "content": "remember Y"},
            ]
        })
        .to_string();
        apply(&out, &mut messages);
        assert_eq!(messages.len(), 3);
        assert_eq!(messages[1].role, ChatRole::System);
        assert_eq!(messages[2].content, ChatContent::Text("remember Y".into()));
    }

    #[test]
    fn inlet_passes_assistant_rows_through_and_ignores_removals() {
        let mut messages = vec![
            user_msg("hi"),
            ChatMessage {
                id: "a1".into(),
                role: ChatRole::Assistant,
                content: ChatContent::Text("earlier answer".into()),
                tool_calls: Some(Vec::new()),
                tool_call_id: None,
                tool_name: None,
            },
        ];
        // Rewrite of the assistant row (dropping its tool_calls) and a
        // shortened history both stay unapplied.
        let out = serde_json::json!({
            "messages": [
                {"role": "user", "content": "hi"},
                {"role": "assistant", "content": "rewritten"},
            ]
        })
        .to_string();
        apply(&out, &mut messages);
        assert_eq!(
            messages[1].content,
            ChatContent::Text("earlier answer".into()),
            "assistant rows are ours, not the filter's"
        );
        assert!(messages[1].tool_calls.is_some());
    }

    #[test]
    fn inlet_keeps_image_parts_when_unchanged() {
        let mut messages = vec![ChatMessage {
            id: "u1".into(),
            role: ChatRole::User,
            content: ChatContent::Parts(vec![
                crate::types::ContentPart::Text { text: "look".into() },
                crate::types::ContentPart::Image {
                    attachment_id: "att".into(),
                    mime: "image/png".into(),
                    data_base64: None,
                    processed_path: None,
                },
            ]),
            tool_calls: None,
            tool_call_id: None,
            tool_name: None,
        }];
        let out = serde_json::json!({
            "messages": [{"role": "user", "content": "look\n\n[image attachment]"}]
        })
        .to_string();
        apply(&out, &mut messages);
        assert!(
            matches!(&messages[0].content, ChatContent::Parts(p) if p.len() == 2),
            "an unchanged view leaves the original multimodal content in place"
        );
    }

    #[test]
    fn pipe_chunks_split_on_char_boundaries() {
        let text = "é".repeat(1000); // 2 bytes per é — 512-byte cuts would split one
        for chunk in slice_chunks(&text, 512) {
            assert!(chunk.chars().all(|c| c == 'é'));
            assert!(chunk.chars().count() <= 512);
        }
        assert_eq!(slice_chunks("", 512).len(), 0);
        assert_eq!(slice_chunks("x", 512), vec!["x"]);
    }

    #[tokio::test]
    async fn run_inlets_is_a_noop_for_missing_python_or_none_results() {
        // A filter whose inlet returns None must never change anything —
        // with or without a Python interpreter on the machine (both paths
        // log and continue).
        let (_dir, db) = crate::db::Database::test_db().await;
        db.upsert_owui_tool(
            crate::types::OwuiTool {
                id: "f1".into(),
                name: "Noop Filter".into(),
                source: "class Filter:\n    def inlet(self, body): return None\n".into(),
                enabled: true,
                kind: "filter".into(),
            },
            crate::ids::now_ms(),
        )
        .await
        .unwrap();
        let mut messages = vec![user_msg("hello")];
        run_inlets(&db, "pipe__x@ep_ternion_pipes", &mut messages).await;
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].content, ChatContent::Text("hello".into()));
    }

    #[tokio::test]
    async fn enabled_specs_skip_filter_and_pipe_rows() {
        let (_dir, db) = crate::db::Database::test_db().await;
        let now = crate::ids::now_ms();
        db.upsert_owui_tool(
            crate::types::OwuiTool {
                id: "t1".into(),
                name: "Real Tools".into(),
                source: SAMPLE.into(),
                enabled: true,
                kind: "tools".into(),
            },
            now,
        )
        .await
        .unwrap();
        db.upsert_owui_tool(
            crate::types::OwuiTool {
                id: "f1".into(),
                name: "Some Filter".into(),
                source: FILTER_SAMPLE.into(),
                enabled: true,
                kind: "filter".into(),
            },
            now,
        )
        .await
        .unwrap();
        let specs = enabled_specs(&db).await;
        assert_eq!(specs.len(), 2, "only the Tools manifest's methods merge");
        assert!(specs.iter().all(|s| s.name.starts_with("owui__")));
    }
}