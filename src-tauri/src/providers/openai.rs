//! OpenAI-compatible adapter (design §5.3): SSE streaming via
//! `POST {base}/v1/chat/completions`, discovery via `/v1/models`.
//! Compiles to the same normalized stream as the Ollama adapter (§5.5) —
//! the UI never knows which provider produced a token.

// Constructed by `build_providers` once endpoint profiles flow through
// (M3.3); until then only the tests exercise it.
#![allow(dead_code)]

use futures::future::BoxFuture;
use futures::StreamExt;
use serde_json::Value;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::{ndjson::LineSplitter, probe, Provider, ProviderError};
use crate::types::{ChatContent, ChatRequest, ChatRole, ContentPart, StatusPhase, StreamEvent};

pub struct OpenAiAdapter {
    endpoint_id: String,
    base_url: String,
    /// Bearer token; None for keyless local servers (LM Studio, llama.cpp).
    api_key: Option<String>,
    /// Extra request headers from the profile (§5.1: proxies, org routing).
    headers: Vec<(String, String)>,
    http: reqwest::Client,
}

impl OpenAiAdapter {
    pub fn new(
        endpoint_id: &str,
        base_url: &str,
        api_key: Option<String>,
        headers: Vec<(String, String)>,
        http: reqwest::Client,
    ) -> Self {
        Self {
            endpoint_id: endpoint_id.to_string(),
            base_url: base_url.trim_end_matches('/').to_string(),
            api_key,
            headers,
            http,
        }
    }

    fn with_common(&self, req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        let mut req = req;
        if let Some(key) = &self.api_key {
            req = req.bearer_auth(key);
        }
        for (k, v) in &self.headers {
            req = req.header(k, v);
        }
        req
    }

    pub(crate) async fn list_models_impl(&self) -> Result<Vec<crate::types::ModelInfo>, ProviderError> {
        let url = probe::openai_models_url(&self.base_url);
        let resp = self
            .with_common(self.http.get(&url))
            .send()
            .await
            .map_err(|e| ProviderError::EndpointUnreachable(e.to_string()))?;
        if !resp.status().is_success() {
            return Err(ProviderError::Http(
                resp.status().as_u16(),
                resp.text().await.unwrap_or_default(),
            ));
        }
        let value: Value = resp
            .json()
            .await
            .map_err(|e| ProviderError::Malformed(format!("models list: {e}")))?;
        let ids = probe::parse_models_list(&value)
            .map_err(|e| ProviderError::Malformed(format!("models list: {e}")))?;
        // /v1/models exposes nothing reliable (§5.4) — capability facts are
        // attached later by the capability registry, not here.
        Ok(ids
            .into_iter()
            .map(|id| crate::types::ModelInfo {
                id: id.clone(),
                display_name: id,
                size_bytes: None,
                parameter_size: None,
                quantization_level: None,
                family: None,
                context_length: None,
                capabilities: Vec::new(),
            })
            .collect())
    }
}

impl Provider for OpenAiAdapter {
    fn id(&self) -> &str {
        &self.endpoint_id
    }

    fn kind(&self) -> super::EndpointKind {
        super::EndpointKind::OpenAiCompat
    }

    fn list_models(&self) -> BoxFuture<'_, Result<Vec<crate::types::ModelInfo>, ProviderError>> {
        Box::pin(self.list_models_impl())
    }

    fn chat(
        &self,
        req: ChatRequest,
        events: mpsc::Sender<StreamEvent>,
        cancel: CancellationToken,
    ) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            self.chat_impl(req, &events, &cancel).await;
        })
    }
}

impl OpenAiAdapter {
    async fn chat_impl(
        &self,
        req: ChatRequest,
        events: &mpsc::Sender<StreamEvent>,
        cancel: &CancellationToken,
    ) {
        let url = format!("{}/chat/completions", chat_base(&self.base_url));
        let body = build_chat_body(&req);

        if events
            .send(StreamEvent::Status {
                phase: StatusPhase::Connecting,
            })
            .await
            .is_err()
        {
            return;
        }

        let resp = match self
            .with_common(self.http.post(&url))
            .json(&body)
            .send()
            .await
        {
            Ok(resp) => resp,
            Err(e) => {
                let _ = events
                    .send(StreamEvent::Error {
                        code: "endpoint_unreachable".into(),
                        message: e.to_string(),
                        retryable: true,
                    })
                    .await;
                return;
            }
        };

        if !resp.status().is_success() {
            let status = resp.status().as_u16();
            let text = resp.text().await.unwrap_or_default();
            let _ = events
                .send(StreamEvent::Error {
                    code: "http_error".into(),
                    message: format!("HTTP {status}: {}", summarize_error(&text)),
                    retryable: status >= 500 || status == 429,
                })
                .await;
            return;
        }

        let mut stream = resp.bytes_stream();
        let mut splitter = LineSplitter::new();

        loop {
            let chunk = tokio::select! {
                biased;

                // Dropping the response closes the connection (§5.3).
                _ = cancel.cancelled() => return,

                chunk = stream.next() => match chunk {
                    Some(Ok(chunk)) => chunk,
                    Some(Err(e)) => {
                        let _ = events
                            .send(StreamEvent::Error {
                                code: "stream_error".into(),
                                message: e.to_string(),
                                retryable: true,
                            })
                            .await;
                        return;
                    }
                    None => {
                        // Server closed without [DONE]: flush any trailing
                        // line, then end normally.
                        if let Some(line) = splitter.finish() {
                            for ev in parse_stream_line(&line) {
                                if events.send(ev).await.is_err() {
                                    return;
                                }
                            }
                        }
                        let _ = events.send(StreamEvent::Done).await;
                        return;
                    }
                }
            };

            for line in splitter.push(&chunk) {
                for ev in parse_stream_line(&line) {
                    let is_done = matches!(ev, StreamEvent::Done);
                    if events.send(ev).await.is_err() {
                        return;
                    }
                    if is_done {
                        return;
                    }
                }
            }
        }
    }
}

/// Chat-completions base: strip a trailing `/v1` only when we'd double it —
/// the path always gets exactly one `/v1`.
fn chat_base(base_url: &str) -> String {
    let base = base_url.trim_end_matches('/');
    if base.rsplit('/').next() == Some("v1") {
        base.to_string()
    } else {
        format!("{base}/v1")
    }
}

/// Error bodies vary (`{"error": {"message": …}}` or a plain string); pull
/// out something readable without leaking the full body into the log.
fn summarize_error(text: &str) -> String {
    serde_json::from_str::<Value>(text)
        .ok()
        .and_then(|v| {
            v.get("error")
                .and_then(|e| e.get("message").and_then(|m| m.as_str()).map(String::from))
                .or_else(|| v.get("error").and_then(|e| e.as_str()).map(String::from))
                .or_else(|| v.get("message").and_then(|m| m.as_str()).map(String::from))
        })
        .unwrap_or_else(|| text.chars().take(300).collect())
}

// ---------------------------------------------------------------------------
// SSE line → normalized events (pure, unit-tested)
// ---------------------------------------------------------------------------

/// One SSE data line → 0..n normalized StreamEvents. Handles `data: [DONE]`,
/// reasoning_content deltas (DeepSeek/Qwen style §5.3), incremental tool-call
/// argument fragments, and usage-chunk variants (some servers send usage in
/// the final chunk with empty choices, some never send it at all).
pub(crate) fn parse_stream_line(line: &str) -> Vec<StreamEvent> {
    let trimmed = line.trim();
    if trimmed.is_empty() || trimmed.starts_with(':') {
        return vec![]; // blank line or SSE comment (keep-alive)
    }
    let Some(payload) = trimmed.strip_prefix("data:") else {
        return vec![]; // event:, id:, retry: fields — not data payloads
    };
    let payload = payload.trim();

    if payload == "[DONE]" {
        return vec![StreamEvent::Done];
    }
    let Ok(value) = serde_json::from_str::<Value>(payload) else {
        log::debug!("openai: skipping malformed SSE data line");
        return vec![];
    };

    // In-band error (some proxies stream errors with HTTP 200).
    if let Some(message) = value
        .get("error")
        .and_then(|e| {
            e.get("message")
                .and_then(|m| m.as_str())
                .map(String::from)
                .or_else(|| e.as_str().map(String::from))
        })
    {
        return vec![StreamEvent::Error {
            code: "provider_error".into(),
            message,
            retryable: false,
        }];
    }

    let mut out = Vec::new();
    if let Some(choice) = value
        .get("choices")
        .and_then(|c| c.as_array())
        .and_then(|c| c.first())
    {
        if let Some(delta) = choice.get("delta") {
            // Reasoning passthrough (§5.3): rendered as a thinking block and
            // never fed back — ChatMessage carries no reasoning field.
            if let Some(reasoning) = delta
                .get("reasoning_content")
                .and_then(|r| r.as_str())
                .or_else(|| delta.get("reasoning").and_then(|r| r.as_str()))
            {
                if !reasoning.is_empty() {
                    out.push(StreamEvent::ReasoningDelta {
                        text: reasoning.to_string(),
                    });
                }
            }
            if let Some(content) = delta.get("content").and_then(|c| c.as_str()) {
                if !content.is_empty() {
                    out.push(StreamEvent::TextDelta {
                        text: content.to_string(),
                    });
                }
            }
            if let Some(calls) = delta.get("tool_calls").and_then(|c| c.as_array()) {
                for (position, call) in calls.iter().enumerate() {
                    out.extend(parse_tool_call(call, position));
                }
            }
        }
    }

    // Usage variants: the final chunk (sometimes with empty choices) may carry
    // `"usage": {"prompt_tokens": …, "completion_tokens": …}`.
    if let Some(usage) = value.get("usage").filter(|u| u.is_object()) {
        out.push(StreamEvent::Usage {
            tokens_in: usage.get("prompt_tokens").and_then(|v| v.as_u64()).unwrap_or(0),
            tokens_out: usage.get("completion_tokens").and_then(|v| v.as_u64()).unwrap_or(0),
            latency_ms: 0, // the orchestrator times the turn itself
        });
    }

    out
}

/// One element of `delta.tool_calls[]`. OpenAI streams a call as fragments:
/// the first carries `id` + `function.name` (→ ToolCallStart), later ones
/// append to `function.arguments` (→ ToolCallDelta). Both may land together.
fn parse_tool_call(call: &Value, position: usize) -> Vec<StreamEvent> {
    let index = call
        .get("index")
        .and_then(|v| v.as_u64())
        .map(|v| v as u32)
        .unwrap_or(position as u32);
    let function = call.get("function");
    let id = call
        .get("id")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty());
    let name = function
        .and_then(|f| f.get("name"))
        .and_then(|n| n.as_str())
        .filter(|s| !s.is_empty());
    let args_delta = function
        .and_then(|f| f.get("arguments"))
        .and_then(|a| a.as_str())
        .unwrap_or("");

    let mut out = Vec::new();
    // A call can't start without its name — fragments that only carry the id
    // (rare) are skipped rather than started nameless.
    if let Some(name) = name {
        out.push(StreamEvent::ToolCallStart {
            index,
            id: id.map(String::from).unwrap_or_else(|| format!("call_{index}")),
            name: name.to_string(),
        });
    }
    if !args_delta.is_empty() {
        out.push(StreamEvent::ToolCallDelta {
            index,
            args_delta: args_delta.to_string(),
        });
    }
    out
}

// ---------------------------------------------------------------------------
// Request body (pure, unit-tested)
// ---------------------------------------------------------------------------

pub(crate) fn build_chat_body(req: &ChatRequest) -> Value {
    let mut body = serde_json::json!({
        "model": req.model,
        "messages": map_messages(&req.messages),
        "stream": true,
    });
    if let Some(t) = req.params.temperature {
        body["temperature"] = serde_json::json!(t);
    }
    if let Some(p) = req.params.top_p {
        body["top_p"] = serde_json::json!(p);
    }
    if let Some(m) = req.params.max_tokens {
        body["max_tokens"] = serde_json::json!(m);
    }
    if let Some(tools) = &req.tools {
        body["tools"] = serde_json::json!(tools
            .iter()
            .map(|t| serde_json::json!({
                "type": "function",
                "function": {
                    "name": t.name,
                    "description": t.description,
                    "parameters": t.input_schema,
                },
            }))
            .collect::<Vec<_>>());
    }
    if let Some(schema) = &req.params.json_schema {
        // Structured output (§5.2 Herald contract, OpenAI form).
        body["response_format"] = serde_json::json!({
            "type": "json_schema",
            "json_schema": {
                "name": "response",
                "strict": false,
                "schema": schema,
            },
        });
    }
    body
}

/// OpenAI wire format (§5.3): text content as a plain string, image parts as
/// data URLs (`data:<mime>;base64,…` — never raw file paths), assistant tool
/// calls with `arguments` as JSON text (our ToolCall.args verbatim), tool
/// results keyed by `tool_call_id`.
fn map_messages(messages: &[crate::types::ChatMessage]) -> Vec<Value> {
    messages
        .iter()
        .map(|msg| {
            let mut m = serde_json::json!({ "role": msg.role });
            match &msg.content {
                ChatContent::Text(text) => {
                    m["content"] = serde_json::json!(text);
                }
                ChatContent::Parts(parts) => {
                    let mut text = String::new();
                    let mut wire_parts = Vec::new();
                    for part in parts {
                        match part {
                            ContentPart::Text { text: t } => text.push_str(t),
                            ContentPart::Image {
                                mime, data_base64, ..
                            } => {
                                if let Some(b64) = data_base64 {
                                    wire_parts.push(serde_json::json!({
                                        "type": "image_url",
                                        "image_url": {
                                            "url": format!("data:{mime};base64,{b64}"),
                                        },
                                    }));
                                }
                            }
                        }
                    }
                    if wire_parts.is_empty() {
                        m["content"] = serde_json::json!(text);
                    } else {
                        if !text.is_empty() {
                            wire_parts
                                .insert(0, serde_json::json!({ "type": "text", "text": text }));
                        }
                        m["content"] = serde_json::Value::Array(wire_parts);
                    }
                }
            }
            match msg.role {
                ChatRole::Assistant => {
                    if let Some(calls) = &msg.tool_calls {
                        m["tool_calls"] = serde_json::json!(calls
                            .iter()
                            .map(|c| serde_json::json!({
                                "id": c.id,
                                "type": "function",
                                "function": {
                                    "name": c.name,
                                    "arguments": c.args,
                                },
                            }))
                            .collect::<Vec<_>>());
                    }
                }
                ChatRole::Tool => {
                    if let Some(id) = &msg.tool_call_id {
                        m["tool_call_id"] = serde_json::json!(id);
                    }
                }
                _ => {}
            }
            m
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{ChatMessage, ChatParams, ToolCall, ToolSpec};

    fn kinds(events: &[StreamEvent]) -> Vec<&'static str> {
        events
            .iter()
            .map(|e| match e {
                StreamEvent::Status { .. } => "status",
                StreamEvent::Routing { .. } => "routing",
                StreamEvent::ReasoningDelta { .. } => "reasoning_delta",
                StreamEvent::TextDelta { .. } => "text_delta",
                StreamEvent::ToolCallStart { .. } => "tool_call_start",
                StreamEvent::ToolCallDelta { .. } => "tool_call_delta",
                StreamEvent::ToolResult { .. } => "tool_result",
                StreamEvent::ApprovalRequest { .. } => "approval_request",
                StreamEvent::Usage { .. } => "usage",
                StreamEvent::Error { .. } => "error",
                StreamEvent::Done => "done",
            })
            .collect()
    }

    #[test]
    fn sse_lines_parse_to_deltas() {
        assert!(parse_stream_line("").is_empty());
        assert!(parse_stream_line(": keep-alive comment").is_empty());
        assert!(parse_stream_line("event: ping").is_empty());

        let evs = parse_stream_line(r#"data: {"choices":[{"delta":{"content":"Hel"}}]}"#);
        assert_eq!(kinds(&evs), vec!["text_delta"]);
        match &evs[0] {
            StreamEvent::TextDelta { text } => assert_eq!(text, "Hel"),
            _ => unreachable!(),
        }

        // `data:[DONE]` (no space) still terminates.
        assert_eq!(kinds(&parse_stream_line("data:[DONE]")), vec!["done"]);
    }

    #[test]
    fn reasoning_content_becomes_a_thinking_delta() {
        // DeepSeek/Qwen spelling…
        let evs = parse_stream_line(
            r#"data: {"choices":[{"delta":{"reasoning_content":"let me think"}}]}"#,
        );
        assert_eq!(kinds(&evs), vec!["reasoning_delta"]);
        // …and the bare `reasoning` spelling used by some servers.
        let evs = parse_stream_line(r#"data: {"choices":[{"delta":{"reasoning":"hmm"}}]}"#);
        assert_eq!(kinds(&evs), vec!["reasoning_delta"]);
    }

    #[test]
    fn tool_call_fragments_start_then_accumulate() {
        let evs = parse_stream_line(
            r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_abc","function":{"name":"fs_read","arguments":"{\"pa"}}]}}]}"#,
        );
        assert_eq!(kinds(&evs), vec!["tool_call_start", "tool_call_delta"]);
        match &evs[0] {
            StreamEvent::ToolCallStart { index, id, name } => {
                assert_eq!(*index, 0);
                assert_eq!(id, "call_abc");
                assert_eq!(name, "fs_read");
            }
            _ => unreachable!(),
        }
        match &evs[1] {
            StreamEvent::ToolCallDelta { index, args_delta } => {
                assert_eq!(*index, 0);
                assert_eq!(args_delta, r#"{"pa"#);
            }
            _ => unreachable!(),
        }

        // Continuation fragment: delta only, same index.
        let evs = parse_stream_line(
            r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"th\":\"x\"}"}}]}}]}"#,
        );
        assert_eq!(kinds(&evs), vec!["tool_call_delta"]);
        match &evs[0] {
            StreamEvent::ToolCallDelta { index, args_delta } => {
                assert_eq!(*index, 0);
                assert_eq!(args_delta, r#"th":"x"}"#);
            }
            _ => unreachable!(),
        }
    }

    #[test]
    fn final_chunk_usage_and_done_variants() {
        // Usage inside the final chunk, choices present.
        let evs = parse_stream_line(
            r#"data: {"choices":[{"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":17,"completion_tokens":4}}"#,
        );
        assert_eq!(kinds(&evs), vec!["usage"]);
        match &evs[0] {
            StreamEvent::Usage { tokens_in, tokens_out, .. } => {
                assert_eq!(*tokens_in, 17);
                assert_eq!(*tokens_out, 4);
            }
            _ => unreachable!(),
        }

        // Usage-only final chunk (empty choices list) still parses.
        let evs = parse_stream_line(
            r#"data: {"choices":[],"usage":{"prompt_tokens":9,"completion_tokens":3}}"#,
        );
        assert_eq!(kinds(&evs), vec!["usage"]);

        // No usage at all — no synthetic zeros (orchestrator tolerates).
        let evs = parse_stream_line(r#"data: {"choices":[{"delta":{},"finish_reason":"stop"}]}"#);
        assert!(evs.is_empty());
    }

    #[test]
    fn in_band_error_becomes_an_error_event() {
        let evs = parse_stream_line(
            r#"data: {"error":{"message":"model not found","type":"invalid_request_error"}}"#,
        );
        assert_eq!(kinds(&evs), vec!["error"]);
        match &evs[0] {
            StreamEvent::Error { message, retryable, .. } => {
                assert_eq!(message, "model not found");
                assert!(!retryable);
            }
            _ => unreachable!(),
        }
    }

    #[test]
    fn malformed_data_lines_are_skipped() {
        assert!(parse_stream_line("data: not json").is_empty());
    }

    #[test]
    fn build_body_maps_params_and_tools() {
        let req = ChatRequest {
            endpoint_id: "e".into(),
            model: "qwen2.5-7b-instruct".into(),
            messages: vec![],
            tools: Some(vec![ToolSpec {
                name: "fs_read".into(),
                description: "Read a file".into(),
                input_schema: serde_json::json!({"type": "object"}),
            }]),
            params: ChatParams {
                temperature: Some(0.3),
                top_p: Some(0.9),
                num_ctx: Some(8192), // no OpenAI equivalent — dropped
                max_tokens: Some(1024),
                json_schema: None,
                keep_alive: Some("10m".into()), // no OpenAI equivalent — dropped
            },
        };
        let body = build_chat_body(&req);
        assert_eq!(body["model"], "qwen2.5-7b-instruct");
        assert_eq!(body["stream"], true);
        let temperature = body["temperature"].as_f64().unwrap();
        assert!((temperature - 0.3).abs() < 1e-3);
        // f32 0.9 widens to 0.8999… in JSON — compare with tolerance.
        let top_p = body["top_p"].as_f64().unwrap();
        assert!((top_p - 0.9).abs() < 1e-3, "top_p: {top_p}");
        assert_eq!(body["max_tokens"], 1024);
        assert!(body.get("options").is_none(), "ollama options don't apply");
        assert!(body.get("keep_alive").is_none());
        let tools = body["tools"].as_array().expect("tools");
        assert_eq!(tools[0]["function"]["name"], "fs_read");
        assert_eq!(tools[0]["function"]["parameters"]["type"], "object");
    }

    #[test]
    fn build_body_schema_becomes_response_format() {
        let req = ChatRequest {
            endpoint_id: "e".into(),
            model: "m".into(),
            messages: vec![],
            tools: None,
            params: ChatParams {
                json_schema: Some(serde_json::json!({"type": "object"})),
                ..Default::default()
            },
        };
        let body = build_chat_body(&req);
        assert_eq!(body["response_format"]["type"], "json_schema");
        assert_eq!(body["response_format"]["json_schema"]["schema"]["type"], "object");
    }

    #[test]
    fn messages_map_text_tool_calls_and_tool_results() {
        let messages = vec![
            ChatMessage {
                id: "m1".into(),
                role: ChatRole::User,
                content: ChatContent::Text("read it".into()),
                tool_calls: None,
                tool_call_id: None,
                tool_name: None,
            },
            ChatMessage {
                id: "m2".into(),
                role: ChatRole::Assistant,
                content: ChatContent::Text(String::new()),
                tool_calls: Some(vec![ToolCall {
                    id: "call_0".into(),
                    name: "fs_read".into(),
                    args: r#"{"path":"C:/w/a.txt"}"#.into(),
                }]),
                tool_call_id: None,
                tool_name: None,
            },
            ChatMessage {
                id: "m3".into(),
                role: ChatRole::Tool,
                content: ChatContent::Text("file body".into()),
                tool_calls: None,
                tool_call_id: Some("call_0".into()),
                tool_name: Some("fs_read".into()),
            },
        ];
        let body = build_chat_body(&ChatRequest {
            endpoint_id: "e".into(),
            model: "m".into(),
            messages,
            tools: None,
            params: ChatParams::default(),
        });
        let msgs = body["messages"].as_array().expect("messages");

        assert_eq!(msgs[0]["role"], "user");
        assert_eq!(msgs[0]["content"], "read it");

        // Assistant tool calls: arguments stay JSON TEXT (OpenAI form).
        assert_eq!(msgs[1]["role"], "assistant");
        let calls = msgs[1]["tool_calls"].as_array().expect("calls");
        assert_eq!(calls[0]["id"], "call_0");
        assert_eq!(calls[0]["type"], "function");
        assert_eq!(calls[0]["function"]["name"], "fs_read");
        assert_eq!(calls[0]["function"]["arguments"], r#"{"path":"C:/w/a.txt"}"#);

        // Tool result: keyed by tool_call_id; tool_name is Ollama-only.
        assert_eq!(msgs[2]["role"], "tool");
        assert_eq!(msgs[2]["content"], "file body");
        assert_eq!(msgs[2]["tool_call_id"], "call_0");
        assert!(msgs[2].get("tool_name").is_none());
    }

    #[test]
    fn image_parts_become_data_urls() {
        let messages = vec![ChatMessage {
            id: "m1".into(),
            role: ChatRole::User,
            content: ChatContent::Parts(vec![
                ContentPart::Text { text: "what is this?".into() },
                ContentPart::Image {
                    attachment_id: "a1".into(),
                    mime: "image/jpeg".into(),
                    data_base64: Some("AA==".into()),
                },
            ]),
            tool_calls: None,
            tool_call_id: None,
            tool_name: None,
        }];
        let body = build_chat_body(&ChatRequest {
            endpoint_id: "e".into(),
            model: "m".into(),
            messages,
            tools: None,
            params: ChatParams::default(),
        });
        let content = body["messages"][0]["content"].as_array().expect("parts");
        assert_eq!(content[0]["type"], "text");
        assert_eq!(content[0]["text"], "what is this?");
        assert_eq!(content[1]["type"], "image_url");
        assert_eq!(
            content[1]["image_url"]["url"],
            "data:image/jpeg;base64,AA=="
        );
    }

    #[test]
    fn chat_base_normalizes_v1_once() {
        assert_eq!(chat_base("http://127.0.0.1:1234"), "http://127.0.0.1:1234/v1");
        assert_eq!(chat_base("http://127.0.0.1:1234/"), "http://127.0.0.1:1234/v1");
        assert_eq!(chat_base("http://127.0.0.1:1234/v1"), "http://127.0.0.1:1234/v1");
        // A deeper custom base path is preserved as-is.
        assert_eq!(chat_base("http://gw.corp/api/v1"), "http://gw.corp/api/v1");
    }

    #[test]
    fn bearer_and_custom_headers_flow_into_requests() {
        // Cheap smoke: constructing the adapter with a key is all the config
        // path needs; the header application itself is exercised by the
        // socket test below.
        let adapter = OpenAiAdapter::new(
            "ep_test",
            "http://127.0.0.1:1/",
            Some("sk-x".into()),
            vec![("X-Org".into(), "acme".into())],
            reqwest::Client::new(),
        );
        assert_eq!(adapter.endpoint_id, "ep_test");
        assert_eq!(adapter.base_url, "http://127.0.0.1:1");
    }

    /// End-to-end SSE pump against a raw socket pretending to be an
    /// OpenAI-compatible server — proves status → deltas → usage → done
    /// normalization and the connection setup (bearer, headers, path).
    #[tokio::test]
    async fn sse_stream_through_a_local_socket() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 4096];
            let read = sock.read(&mut buf).await.unwrap();
            let request = String::from_utf8_lossy(&buf[..read]).to_string();
            assert!(request.starts_with("POST /v1/chat/completions"), "path: {request}");
            assert!(request.contains("authorization: Bearer sk-test"), "auth header");
            assert!(request.contains("x-org: acme"), "custom header");

            let payload = concat!(
                r#"data: {"choices":[{"delta":{"content":"Hel"}}]}"#, "\n\n",
                r#"data: {"choices":[{"delta":{"content":"lo"}}]}"#, "\n\n",
                r#"data: {"choices":[{"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":5,"completion_tokens":2}}"#, "\n\n",
                "data: [DONE]\n\n",
            );
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{payload}",
                payload.len(),
            );
            sock.write_all(response.as_bytes()).await.unwrap();
            // Hold the socket until the client is done reading.
            let _ = tokio::time::timeout(std::time::Duration::from_secs(5), sock.read(&mut buf)).await;
        });

        let adapter = OpenAiAdapter::new(
            "ep_test",
            &format!("http://127.0.0.1:{port}"),
            Some("sk-test".into()),
            vec![("X-Org".into(), "acme".into())],
            reqwest::Client::new(),
        );
        let (tx, mut rx) = mpsc::channel::<StreamEvent>(64);
        let req = ChatRequest {
            endpoint_id: "ep_test".into(),
            model: "local-model".into(),
            messages: vec![ChatMessage {
                id: "m1".into(),
                role: ChatRole::User,
                content: ChatContent::Text("hi".into()),
                tool_calls: None,
                tool_call_id: None,
                tool_name: None,
            }],
            tools: None,
            params: ChatParams::default(),
        };
        let cancel = CancellationToken::new();
        adapter.chat(req, tx, cancel).await;

        let mut text = String::new();
        let mut got_status = false;
        let mut usage = None;
        let mut got_done = false;
        while let Some(ev) = rx.recv().await {
            match ev {
                StreamEvent::Status { .. } => got_status = true,
                StreamEvent::TextDelta { text: t } => text.push_str(&t),
                StreamEvent::Usage { tokens_in, tokens_out, .. } => {
                    usage = Some((tokens_in, tokens_out))
                }
                StreamEvent::Done => {
                    got_done = true;
                    break;
                }
                StreamEvent::Error { message, .. } => panic!("error event: {message}"),
                _ => {}
            }
        }
        server.await.unwrap();
        assert!(got_status);
        assert_eq!(text, "Hello");
        assert_eq!(usage, Some((5, 2)));
        assert!(got_done);
    }

    /// Live check against a real OpenAI-compatible server (run explicitly:
    /// `cargo test -- --ignored` with LM_STUDIO_BASE set).
    #[tokio::test]
    #[ignore = "requires an OpenAI-compatible server (LM_STUDIO_BASE)"]
    async fn live_list_models() {
        let base = std::env::var("LM_STUDIO_BASE").unwrap_or("http://127.0.0.1:1234".into());
        let adapter = OpenAiAdapter::new("ep_test", &base, None, Vec::new(), reqwest::Client::new());
        let models = adapter.list_models_impl().await.expect("live list_models");
        println!("live openai-compat models: {models:#?}");
        assert!(!models.is_empty());
    }
}