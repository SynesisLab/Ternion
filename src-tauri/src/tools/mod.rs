//! Bundled tool runtime core (design §6): the registry the orchestrator
//! loops over, the executor trait, and argument validation against each
//! tool's own JSON schema. Read tools (§6.2) register here in M2.4;
//! mutating tools follow in M2.5 behind the §6.6 gate.

use std::sync::Arc;

use futures::future::BoxFuture;

pub mod fs;
pub mod guard;
pub mod shell;

/// Execution context handed to tools: the conversation's canonical workspace
/// roots (§6.3 — the only folders FS tools may touch), plus the attachments
/// directory when images produced by tools should be captured (§7.3: fs_read
/// on an image becomes an attachment). Tool results fed to the model are
/// always treated as untrusted text (§10.3).
#[derive(Debug, Clone, Default)]
pub struct ToolExecCtx {
    pub workspaces: Vec<String>,
    /// None in tool-free sidecar paths; set to the attachments dir otherwise.
    pub attachments: Option<std::path::PathBuf>,
}

/// Terminal state of one executed tool call (§6.1).
#[derive(Debug, Clone, PartialEq)]
pub enum ToolOutcome {
    Ok(String),
    /// Text plus image files captured during execution (§7.3): the
    /// orchestrator persists the rows and attaches image parts.
    WithImages {
        text: String,
        images: Vec<crate::img::StoredImage>,
    },
    Err(String),
}

impl ToolOutcome {
    pub fn into_parts(self) -> (String, bool) {
        match self {
            ToolOutcome::Ok(text) => (text, false),
            ToolOutcome::WithImages { text, .. } => (text, false),
            ToolOutcome::Err(text) => (text, true),
        }
    }
}

/// A bundled tool: an OpenAI JSON-schema declaration plus its executor.
pub trait Tool: Send + Sync {
    fn spec(&self) -> crate::types::ToolSpec;
    fn execute(
        &self,
        args: serde_json::Value,
        ctx: &ToolExecCtx,
    ) -> BoxFuture<'_, Result<ToolOutcome, ToolError>>;
}

#[derive(Debug)]
pub enum ToolError {
    /// Args failed the tool's schema validation — listed per problem.
    Validation(String),
    /// The tool ran and failed (e.g. path guard).
    Exec(String),
}

impl ToolError {
    pub fn message(&self) -> String {
        match self {
            ToolError::Validation(m) | ToolError::Exec(m) => m.clone(),
        }
    }
}

/// The orchestrator's tool set. Immutable after startup (bundled tools only;
/// MCP servers merge in M3 behind the same seam).
pub struct ToolRegistry {
    tools: Vec<Arc<dyn Tool>>,
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::bundled()
    }
}

impl ToolRegistry {
    /// The bundled tool set (design §6.2): five auto-allowed FS reads, the
    /// six mutating FS tools (§6.6 matrix), and the opt-in shell tool. The
    /// orchestrator filters the shell spec by the `tools.shell_enabled`
    /// setting — the registry stays complete for MCP-style lookups (M3).
    pub fn bundled() -> Self {
        let mut tools = fs::bundled_read_tools();
        tools.extend(fs::bundled_mutating_tools());
        tools.push(Arc::new(shell::Shell));
        Self { tools }
    }

    pub fn register(&mut self, tool: Arc<dyn Tool>) {
        self.tools.push(tool);
    }

    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }

    pub fn specs(&self) -> Vec<crate::types::ToolSpec> {
        self.tools.iter().map(|t| t.spec()).collect()
    }

    pub fn get(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.tools.iter().find(|t| t.spec().name == name).cloned()
    }
}

/// Validate an arguments object against the tool's declared JSON schema —
/// the subset our bundled tools declare: object root, `required` presence,
/// and per-property primitive types. Schemas using advanced keywords pass
/// unchecked (the bundled set is flat); adopting the `jsonschema` crate is
/// the upgrade path if a tool ever needs it.
pub fn validate_args(
    schema: &serde_json::Value,
    args: &serde_json::Value,
) -> Result<(), ToolError> {
    let Some(obj) = args.as_object() else {
        return Err(ToolError::Validation("arguments must be a JSON object".into()));
    };

    let mut problems: Vec<String> = Vec::new();

    if let Some(required) = schema.get("required").and_then(|r| r.as_array()) {
        for name in required {
            let Some(key) = name.as_str() else { continue };
            if !obj.contains_key(key) {
                problems.push(format!("missing required argument `{key}`"));
            }
        }
    }

    if let Some(properties) = schema.get("properties").and_then(|p| p.as_object()) {
        for (key, prop_schema) in properties {
            let Some(value) = obj.get(key) else { continue };
            let Some(expected) = prop_schema.get("type").and_then(|t| t.as_str()) else {
                continue;
            };
            let actual_ok = match expected {
                "string" => value.is_string(),
                "number" => value.is_number(),
                "integer" => value.is_i64() || value.is_u64(),
                "boolean" => value.is_boolean(),
                "array" => value.is_array(),
                "object" => value.is_object(),
                _ => true, // unknown type keyword: not enforced
            };
            if !actual_ok {
                problems.push(format!("`{key}` must be a {expected}"));
            }
        }
    }

    if problems.is_empty() {
        Ok(())
    } else {
        Err(ToolError::Validation(problems.join("; ")))
    }
}

/// Cap on one tool result fed back to the model — protects the context
/// budget from a runaway tool (fs_read has its own, tighter caps; the shell
/// tool caps at 8 KB per §6.4). Oversized results are truncated with a note.
pub const MAX_RESULT_CHARS: usize = 16_384;

pub fn clamp_result(text: String) -> String {
    if text.chars().count() <= MAX_RESULT_CHARS {
        return text;
    }
    let mut cut: usize = 0;
    for (i, _) in text.char_indices() {
        if i > MAX_RESULT_CHARS {
            break;
        }
        cut = i;
    }
    format!(
        "{}\n[truncated — result exceeded {MAX_RESULT_CHARS} characters]",
        &text[..cut]
    )
}

#[cfg(test)]
pub mod testkit {
    //! Test-only tool used by the orchestrator loop tests.

    use super::*;

    pub struct EchoTool {
        pub result: String,
    }

    impl Tool for EchoTool {
        fn spec(&self) -> crate::types::ToolSpec {
            crate::types::ToolSpec {
                name: "echo".into(),
                description: "Echo the given text back".into(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "required": ["text"],
                    "properties": { "text": { "type": "string" } }
                }),
            }
        }

        fn execute(
            &self,
            args: serde_json::Value,
            _ctx: &ToolExecCtx,
        ) -> BoxFuture<'_, Result<ToolOutcome, ToolError>> {
            let result = self.result.clone();
            Box::pin(async move {
                let text = args["text"].as_str().unwrap_or("").to_string();
                if result.is_empty() {
                    Ok(ToolOutcome::Ok(format!("echo: {text}")))
                } else {
                    Ok(ToolOutcome::Ok(result))
                }
            })
        }
    }

    pub struct FailTool;

    impl Tool for FailTool {
        fn spec(&self) -> crate::types::ToolSpec {
            crate::types::ToolSpec {
                name: "fail".into(),
                description: "Always fails".into(),
                input_schema: serde_json::json!({"type": "object"}),
            }
        }

        fn execute(
            &self,
            _args: serde_json::Value,
            _ctx: &ToolExecCtx,
        ) -> BoxFuture<'_, Result<ToolOutcome, ToolError>> {
            Box::pin(async { Err(ToolError::Exec("boom".into())) })
        }
    }

    /// Echoes the execution context — asserts workspace wiring end-to-end.
    pub struct CtxTool;

    impl Tool for CtxTool {
        fn spec(&self) -> crate::types::ToolSpec {
            crate::types::ToolSpec {
                name: "ctx".into(),
                description: "Reports the execution context".into(),
                input_schema: serde_json::json!({"type": "object"}),
            }
        }

        fn execute(
            &self,
            _args: serde_json::Value,
            ctx: &ToolExecCtx,
        ) -> BoxFuture<'_, Result<ToolOutcome, ToolError>> {
            let workspaces = ctx.workspaces.join("|");
            Box::pin(async move { Ok(ToolOutcome::Ok(format!("workspaces:{workspaces}"))) })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ToolSpec;

    fn schema() -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "required": ["path"],
            "properties": {
                "path": { "type": "string" },
                "from": { "type": "integer" },
                "deep": { "type": "boolean" },
            }
        })
    }

    fn spec(schema: serde_json::Value) -> ToolSpec {
        ToolSpec {
            name: "t".into(),
            description: "t".into(),
            input_schema: schema,
        }
    }

    #[test]
    fn valid_args_pass() {
        let args = serde_json::json!({"path": "C:/w", "deep": true});
        assert!(validate_args(&schema(), &args).is_ok());
        // Absent optional properties are fine.
        assert!(validate_args(&schema(), &serde_json::json!({"path": "x"})).is_ok());
    }

    #[test]
    fn missing_required_fails_with_all_problems() {
        let args = serde_json::json!({"deep": "not a bool"});
        let err = validate_args(&schema(), &args).unwrap_err();
        let msg = err.message();
        assert!(msg.contains("missing required argument `path`"), "{msg}");
        assert!(msg.contains("`deep` must be a boolean"), "{msg}");
    }

    #[test]
    fn non_object_args_rejected() {
        assert!(validate_args(&schema(), &serde_json::json!("x")).is_err());
        assert!(validate_args(&schema(), &serde_json::json!(null)).is_err());
    }

    #[test]
    fn integer_takes_whole_numbers_only() {
        let args = serde_json::json!({"path": "x", "from": 1.5});
        assert!(validate_args(&schema(), &args).is_err());
    }

    #[test]
    fn registry_specs_and_lookup() {
        let reg = ToolRegistry::bundled();
        assert!(!reg.is_empty());
        // 5 auto-allowed reads (M2.4) + 6 gated mutators (M2.6) + shell (M2.7).
        assert_eq!(reg.specs().len(), 12);
        for name in [
            "fs_list", "fs_read", "fs_stat", "fs_search", "fs_tree", "fs_write", "fs_edit",
            "fs_move", "fs_copy", "fs_delete", "fs_mkdir", "shell",
        ] {
            assert!(reg.get(name).is_some(), "{name} registered");
        }
        assert!(reg.get("nope").is_none());
    }

    #[test]
    fn clamp_result_truncates_long_output() {
        let ok = clamp_result("short".into());
        assert_eq!(ok, "short");
        let long = "x".repeat(MAX_RESULT_CHARS + 100);
        let clamped = clamp_result(long);
        assert!(clamped.contains("[truncated"));
        assert!(clamped.chars().count() < MAX_RESULT_CHARS + 100);
    }
}