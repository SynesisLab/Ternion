//! Normalized core types — mirrors of the design doc's Appendix A TypeScript
//! union. Serialization shape is part of the IPC contract with the frontend
//! (`src/types/stream.ts` must mirror it exactly); a unit test guards the tags.

use serde::{Deserialize, Serialize};

/// Triad role of a model (M1; stored on messages now for forward compatibility).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelRole {
    Herald,
    Scout,
    Titan,
}

/// Routing target (M1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Target {
    Scout,
    Titan,
}

impl Target {
    pub fn as_str(&self) -> &'static str {
        match self {
            Target::Scout => "scout",
            Target::Titan => "titan",
        }
    }
}

impl ModelRole {
    pub fn as_str(&self) -> &'static str {
        match self {
            ModelRole::Herald => "herald",
            ModelRole::Scout => "scout",
            ModelRole::Titan => "titan",
        }
    }
}

/// Deterministic flags from the router (design §3.3).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RoutingFlags {
    pub vision: bool,
    pub tools: bool,
    pub code: bool,
    pub long_form: bool,
    pub multi_step: bool,
    pub sensitive: bool,
}

/// Where a routing decision came from (design §3.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecisionSource {
    Herald,
    Heuristic,
    HardRule,
    Manual,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RoutingDecision {
    pub target: Target,
    pub confidence: f32,
    pub complexity: u8,
    pub reason: String,
    pub flags: RoutingFlags,
    pub est_in_tokens: u32,
    pub est_out_tokens: u32,
    pub handoff_note: String,
    pub source: DecisionSource,
}

impl Default for RoutingDecision {
    fn default() -> Self {
        Self {
            target: Target::Scout,
            confidence: 0.0,
            complexity: 1,
            reason: String::new(),
            flags: RoutingFlags::default(),
            est_in_tokens: 0,
            est_out_tokens: 0,
            handoff_note: String::new(),
            source: DecisionSource::Heuristic,
        }
    }
}

/// One part of a message body. `content` in the DB is a JSON array of these
/// (design §8.2: "normalized JSON (parts, tool calls)") — M2 adds tool parts,
/// M3 adds image parts, no migration needed.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", rename_all_fields = "camelCase")]
pub enum ContentPart {
    Text { text: String },
    Image {
        attachment_id: String,
        mime: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        data_base64: Option<String>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChatRole {
    System,
    User,
    Assistant,
    Tool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub args: String,
}

/// Message body as stored/sent: plain string or a parts array (Appendix A).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ChatContent {
    Text(String),
    Parts(Vec<ContentPart>),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatMessage {
    pub id: String,
    pub role: ChatRole,
    pub content: ChatContent,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    /// For `role: tool` messages: which declared tool produced this result
    /// (Ollama's native format wants `tool_name` on tool messages).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ChatParams {
    pub temperature: Option<f32>,
    pub top_p: Option<f32>,
    pub num_ctx: Option<u32>,
    pub max_tokens: Option<u32>,
    pub json_schema: Option<serde_json::Value>,
    pub keep_alive: Option<String>,
}

/// OpenAI JSON-schema form of a tool declaration (works for both adapters).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatRequest {
    pub endpoint_id: String,
    pub model: String,
    pub messages: Vec<ChatMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<ToolSpec>>,
    #[serde(default)]
    pub params: ChatParams,
}

/// The single normalized stream protocol every provider compiles to
/// (design §5.5). M0 emits status/reasoning_delta/text_delta/usage/error/done;
/// routing (M1) and tool_call_* (M2) variants exist but stay unused.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case", rename_all_fields = "camelCase")]
pub enum StreamEvent {
    Status { phase: StatusPhase },
    Routing { decision: RoutingDecision, final_target: Target },
    ReasoningDelta { text: String },
    TextDelta { text: String },
    ToolCallStart { index: u32, id: String, name: String },
    ToolCallDelta { index: u32, args_delta: String },
    ToolResult { call_id: String, content: String, is_error: Option<bool> },
    /// §6.6: a mutating tool is waiting for the user. The stream pauses
    /// (never cancelled) until `respond_approval` resolves this id.
    ApprovalRequest {
        id: String,
        call_id: String,
        tool: String,
        /// Primary affected path (the `path`/`from` argument, resolved).
        path: String,
        /// Secondary path for fs_move/fs_copy (the `to` argument).
        secondary_path: Option<String>,
        /// One-line human summary, e.g. "overwrites an existing file (1.2 KB)".
        summary: String,
        /// Unified diff for fs_write/fs_edit; None for the other tools.
        diff: Option<String>,
        /// fs_write/fs_edit only: the full content that would land. Shown and
        /// editable in the modal ("Edit in place" replaces the tool arg).
        result_content: Option<String>,
    },
    Usage { tokens_in: u64, tokens_out: u64, latency_ms: u64 },
    Error { code: String, message: String, retryable: bool },
    Done,
}

/// The user's decision on an ApprovalRequest (sent via `respond_approval`).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalReply {
    pub allow: bool,
    /// "" (allow once) | "session" | "always" — only read when allow.
    #[serde(default)]
    pub mode: String,
    /// Edit-in-place: replacement content for fs_write/fs_edit.
    #[serde(default)]
    pub edited_content: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StatusPhase {
    Routing,
    Connecting,
    LoadingModel,
}

// ---------------------------------------------------------------------------
// DTOs exchanged with the frontend over IPC
// ---------------------------------------------------------------------------

/// Lifecycle of a persisted message row (M0 additive column on `messages`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageStatus {
    Streaming,
    Complete,
    Stopped,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Conversation {
    pub id: String,
    pub title: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    /// M0: NULL → default model, else the explicit model id chosen per chat.
    /// M1 reinterprets this as the Triad pin ('auto' | 'scout' | 'titan').
    pub pinned_model: Option<String>,
    pub workspace_roots: Vec<String>,
    pub system_prompt: Option<String>,
    pub archived: bool,
    /// Rolling digest maintained by the Herald sidecar (§3.6) — injected as a
    /// system message on model handoffs. Surfaced for the router log drawer.
    pub digest: Option<String>,
}

/// Routing outcome joined onto a message (design §9.2 ribbon data).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageRouting {
    pub decision: RoutingDecision,
    pub final_target: Target,
    pub actual_model: String,
    pub latency_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Message {
    pub id: String,
    pub conversation_id: String,
    pub role: ChatRole,
    pub content: Vec<ContentPart>,
    /// Reasoning passthrough (thinking models); rendered in a collapsible
    /// block, never fed back to non-reasoning models.
    pub reasoning: Option<String>,
    pub model_role: Option<ModelRole>,
    pub model_id: Option<String>,
    pub endpoint_id: Option<String>,
    pub tokens_in: Option<u64>,
    pub tokens_out: Option<u64>,
    pub latency_ms: Option<u64>,
    pub status: MessageStatus,
    pub error: Option<String>,
    pub created_at: i64,
    /// Router decision behind this assistant message (§3.11), joined from
    /// `routing_events` — None for user messages and pre-M1 rows.
    pub routing: Option<MessageRouting>,
    /// Executed tool calls behind this assistant message (§6.1), oldest
    /// first. Empty for user messages and turns without tool activity.
    #[serde(default)]
    pub tool_calls: Vec<ToolCallRow>,
}

/// Discovery record for the model picker (from Ollama /api/tags).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelInfo {
    pub id: String,
    pub display_name: String,
    pub size_bytes: Option<u64>,
    pub parameter_size: Option<String>,
    pub quantization_level: Option<String>,
    pub family: Option<String>,
    pub context_length: Option<u32>,
    /// e.g. ["completion", "vision", "tools", "thinking"]
    pub capabilities: Vec<String>,
}

/// User-managed endpoint profile (design §5.1). The built-in local Ollama
/// (`ep_local_ollama`, from the `ollama.base_url` setting) is synthesized, not
/// a table row. `api_key_ref` is a keyring handle — the secret is read from
/// Windows Credential Manager only at request time (§10.1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EndpointProfile {
    pub id: String,
    /// "ollama" | "openai" (OpenAI-compatible)
    pub kind: String,
    pub name: String,
    pub base_url: String,
    pub api_key_ref: Option<String>,
    /// Extra request headers (proxies, org routing) — §5.1.
    #[serde(default)]
    pub headers: std::collections::HashMap<String, String>,
    pub enabled: bool,
    pub notes: Option<String>,
}

/// Result of a connection test (design §5.1: "latency + model count surfaced
/// in UI"). Unreachable endpoints come back as `ok: false` + `error` so the
/// settings dialog shows them inline instead of an IPC error.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EndpointTestResult {
    pub ok: bool,
    pub latency_ms: u64,
    pub model_count: usize,
    /// First few model ids, for a quick eyeball check.
    pub models: Vec<String>,
    pub error: Option<String>,
}

/// Result of a finished `chat_send` invocation.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatSendResult {
    pub message_id: String,
    pub status: MessageStatus,
    pub tokens_in: u64,
    pub tokens_out: u64,
    pub latency_ms: u64,
    /// M1: the answer ran on Scout past `scout_output_ceiling` — the UI
    /// offers "⚡ Continue with Titan".
    pub escalation_available: bool,
}

/// One persisted router decision (router log drawer, design §9.2).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RoutingEvent {
    pub id: String,
    pub ts: i64,
    pub conversation_id: String,
    pub message_id: String,
    pub decision: RoutingDecision,
    pub final_target: Target,
    pub actual_model: String,
    pub latency_ms: u64,
    /// "manual" when a user pin chose the model; NULL for router decisions.
    pub override_kind: Option<String>,
}

/// One executed tool call behind an assistant message (tool runtime §6.1).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolCallRow {
    pub id: String,
    pub message_id: String,
    pub tool: String,
    pub args: Option<String>,
    pub result: Option<String>,
    /// running | ok | error | denied
    pub status: String,
    /// null until the §6.6 permission matrix (auto | ask | session | always)
    pub permission_mode: Option<String>,
    pub created_at: i64,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The serialized JSON tags/fields must match the frontend TS union in
    /// src/types/stream.ts — this is the protocol-drift guard.
    #[test]
    fn stream_event_serialization_matches_frontend_union() {
        let cases: Vec<(StreamEvent, &str)> = vec![
            (StreamEvent::Status { phase: StatusPhase::LoadingModel }, "status"),
            (StreamEvent::Routing {
                decision: RoutingDecision {
                    target: Target::Scout,
                    confidence: 0.9,
                    complexity: 2,
                    reason: "test".into(),
                    flags: RoutingFlags {
                        vision: false, tools: false, code: false,
                        long_form: false, multi_step: false, sensitive: false,
                    },
                    est_in_tokens: 0,
                    est_out_tokens: 0,
                    handoff_note: String::new(),
                    source: DecisionSource::Herald,
                },
                final_target: Target::Scout,
            }, "routing"),
            (StreamEvent::ReasoningDelta { text: "t".into() }, "reasoning_delta"),
            (StreamEvent::TextDelta { text: "t".into() }, "text_delta"),
            (StreamEvent::ToolCallStart { index: 0, id: "1".into(), name: "n".into() }, "tool_call_start"),
            (StreamEvent::ToolCallDelta { index: 0, args_delta: "a".into() }, "tool_call_delta"),
            (StreamEvent::ToolResult { call_id: "1".into(), content: "c".into(), is_error: None }, "tool_result"),
            (StreamEvent::ApprovalRequest {
                id: "ap1".into(),
                call_id: "1".into(),
                tool: "fs_write".into(),
                path: r"C:\w\a.txt".into(),
                secondary_path: None,
                summary: "s".into(),
                diff: None,
                result_content: None,
            }, "approval_request"),
            (StreamEvent::Usage { tokens_in: 1, tokens_out: 2, latency_ms: 3 }, "usage"),
            (StreamEvent::Error { code: "c".into(), message: "m".into(), retryable: true }, "error"),
            (StreamEvent::Done, "done"),
        ];
        for (ev, expected_tag) in cases {
            let v = serde_json::to_value(&ev).unwrap();
            assert_eq!(v["type"], expected_tag, "tag mismatch for {expected_tag}");
        }

        // Field naming (camelCase) on representative variants
        let usage = serde_json::to_value(StreamEvent::Usage {
            tokens_in: 1, tokens_out: 2, latency_ms: 3,
        }).unwrap();
        assert_eq!(usage["tokensIn"], 1);
        assert_eq!(usage["latencyMs"], 3);

        let delta = serde_json::to_value(StreamEvent::ToolCallDelta {
            index: 0, args_delta: "x".into(),
        }).unwrap();
        assert_eq!(delta["argsDelta"], "x");

        let approval = serde_json::to_value(StreamEvent::ApprovalRequest {
            id: "ap1".into(),
            call_id: "1".into(),
            tool: "fs_move".into(),
            path: r"C:\w\a.txt".into(),
            secondary_path: Some(r"C:\w\b.txt".into()),
            summary: "moves a file".into(),
            diff: None,
            result_content: None,
        }).unwrap();
        assert_eq!(approval["callId"], "1");
        assert_eq!(approval["secondaryPath"], r"C:\w\b.txt");

        let status = serde_json::to_value(StreamEvent::Status {
            phase: StatusPhase::Routing,
        }).unwrap();
        assert_eq!(status["phase"], "routing");

        // Nested RoutingDecision must serialize camelCase (mirrors the TS
        // interface in src/types/stream.ts).
        let routing = serde_json::to_value(StreamEvent::Routing {
            decision: RoutingDecision {
                target: Target::Titan,
                confidence: 0.91,
                complexity: 4,
                reason: "multi-file code".into(),
                flags: RoutingFlags { tools: true, ..Default::default() },
                est_in_tokens: 600,
                est_out_tokens: 2000,
                handoff_note: "refactor auth".into(),
                source: DecisionSource::Herald,
            },
            final_target: Target::Titan,
        }).unwrap();
        assert_eq!(routing["finalTarget"], "titan");
        assert_eq!(routing["decision"]["target"], "titan");
        let confidence = routing["decision"]["confidence"].as_f64().unwrap();
        assert!((confidence - 0.91).abs() < 1e-3, "confidence: {confidence}");
        assert_eq!(routing["decision"]["estInTokens"], 600);
        assert_eq!(routing["decision"]["estOutTokens"], 2000);
        assert_eq!(routing["decision"]["handoffNote"], "refactor auth");
        assert_eq!(routing["decision"]["source"], "herald");
        assert_eq!(routing["decision"]["flags"]["longForm"], false);
        assert_eq!(routing["decision"]["flags"]["multiStep"], false);
    }

    #[test]
    fn content_parts_serialize_like_frontend() {
        let text = serde_json::to_value(ContentPart::Text { text: "hi".into() }).unwrap();
        assert_eq!(text, serde_json::json!({ "type": "text", "text": "hi" }));

        let image = serde_json::to_value(ContentPart::Image {
            attachment_id: "a1".into(),
            mime: "image/png".into(),
            data_base64: Some("AA==".into()),
        }).unwrap();
        assert_eq!(image["type"], "image");
        assert_eq!(image["attachmentId"], "a1");
        assert_eq!(image["dataBase64"], "AA==");
    }
}