//! Herald — the delegator (design §3.1/§3.3/§3.4, Appendix B). One
//! classification call per user message: smallest model, temperature 0,
//! structured output, strict timeout. A failure here is never fatal — the
//! caller falls back to the heuristic router.

use std::time::Instant;

use tokio_util::sync::CancellationToken;

use super::policy;
use crate::{
    providers::Provider,
    types::{
        ChatContent, ChatMessage, ChatParams, ChatRequest, ChatRole, ContentPart, DecisionSource,
        RoutingDecision, RoutingFlags, StreamEvent, Target,
    },
};

/// Herald never streams to the user and must stay terse (design: num_predict ≤ 200).
const HERALD_MAX_TOKENS: u32 = 200;

/// Appendix B system prompt, verbatim minus the few-shot note (the examples
/// ship as message pairs below — small models follow them better that way).
const HERALD_SYSTEM: &str = r#"You are Herald, the router inside Ternion, a desktop AI assistant.
Ternion has two worker models:
- "scout": a small, fast model. Best for greetings, casual chat, short factual
  questions, quick rewrites, simple single-file edits, file lookups, short
  translations, basic math, short summaries.
- "titan": a large, capable model. Best for multi-step reasoning, multi-file or
  architecture-level coding, long documents, detailed image analysis, multi-tool
  agent work, and planning.

Your job: read the conversation and the latest user message, then decide which
model should answer. Output ONLY one JSON object — no prose, no markdown.

Schema:
{"target":"scout"|"titan","confidence":0.0-1.0,"complexity":1-5,
 "reason":"<=12 words",
 "flags":{"vision":bool,"tools":bool,"code":bool,"long_form":bool,
          "multi_step":bool,"sensitive":bool},
 "est_in_tokens":int,"est_out_tokens":int,
 "handoff_note":"<=30 words: task digest for the receiving model"}

Rules:
- If an image is attached, set flags.vision=true (the system enforces a
  vision-capable model; still pick the intended target).
- If the user asks for multiple files, large scope, or a plan → titan.
- Short follow-ups inside an ongoing large task → titan (continuity matters).
- When genuinely torn, choose "scout" — hard rules will correct capability gaps.
- handoff_note must orient the receiving model in one sentence."#;

/// The 8 few-shot pairs from Appendix B: greeting · math · code review ·
/// multi-file refactor · screenshot Q&A · long-doc analysis · follow-up in
/// titan task · sensitive-data request.
const FEW_SHOTS: &[(&str, &str)] = &[
    ("hey!", r#"{"target":"scout","confidence":0.98,"complexity":1,"reason":"greeting","flags":{"vision":false,"tools":false,"code":false,"long_form":false,"multi_step":false,"sensitive":false},"est_in_tokens":3,"est_out_tokens":30,"handoff_note":"casual greeting, reply briefly"}"#),
    ("what is 15% of 82?", r#"{"target":"scout","confidence":0.97,"complexity":1,"reason":"simple math","flags":{"vision":false,"tools":false,"code":false,"long_form":false,"multi_step":false,"sensitive":false},"est_in_tokens":20,"est_out_tokens":40,"handoff_note":"compute 15% of 82"}"#),
    ("Can you review this function for bugs? def add(a, b): return a + b", r#"{"target":"scout","confidence":0.75,"complexity":2,"reason":"single small function review","flags":{"vision":false,"tools":false,"code":true,"long_form":false,"multi_step":false,"sensitive":false},"est_in_tokens":80,"est_out_tokens":200,"handoff_note":"review a two-line python add function"}"#),
    ("Refactor auth to use refresh tokens across these 5 files: session.rs, login.rs, token.rs, guard.rs, api.rs", r#"{"target":"titan","confidence":0.91,"complexity":4,"reason":"multi-file refactor","flags":{"vision":false,"tools":false,"code":true,"long_form":false,"multi_step":true,"sensitive":false},"est_in_tokens":600,"est_out_tokens":2000,"handoff_note":"refactor auth flow to refresh tokens across 5 files"}"#),
    ("[image attached] why is this error happening?", r#"{"target":"scout","confidence":0.8,"complexity":2,"reason":"screenshot error question","flags":{"vision":true,"tools":false,"code":false,"long_form":false,"multi_step":false,"sensitive":false},"est_in_tokens":150,"est_out_tokens":300,"handoff_note":"diagnose error shown in screenshot"}"#),
    ("Here is a 40-page document. Analyze the risk factors in chapter 3 and compare with chapter 7.", r#"{"target":"titan","confidence":0.9,"complexity":4,"reason":"long document analysis","flags":{"vision":false,"tools":false,"code":false,"long_form":true,"multi_step":true,"sensitive":false},"est_in_tokens":4000,"est_out_tokens":1500,"handoff_note":"analyze risk factors, compare chapters 3 and 7"}"#),
    ("ok now summarize that in 3 bullets", r#"{"target":"titan","confidence":0.72,"complexity":2,"reason":"follow-up inside titan task","flags":{"vision":false,"tools":false,"code":false,"long_form":false,"multi_step":false,"sensitive":false},"est_in_tokens":50,"est_out_tokens":200,"handoff_note":"summarize prior titan answer in 3 bullets"}"#),
    ("Write an email sharing my medical test results with my employer.", r#"{"target":"titan","confidence":0.7,"complexity":3,"reason":"sensitive personal data","flags":{"vision":false,"tools":false,"code":false,"long_form":false,"multi_step":false,"sensitive":true},"est_in_tokens":40,"est_out_tokens":300,"handoff_note":"draft sensitive medical-results email carefully"}"#),
];

/// Structured-output schema handed to the provider (Ollama `format`).
fn decision_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "target": {"type": "string", "enum": ["scout", "titan"]},
            "confidence": {"type": "number"},
            "complexity": {"type": "integer"},
            "reason": {"type": "string"},
            "flags": {"type": "object", "properties": {
                "vision": {"type": "boolean"},
                "tools": {"type": "boolean"},
                "code": {"type": "boolean"},
                "long_form": {"type": "boolean"},
                "multi_step": {"type": "boolean"},
                "sensitive": {"type": "boolean"}
            }},
            "est_in_tokens": {"type": "integer"},
            "est_out_tokens": {"type": "integer"},
            "handoff_note": {"type": "string"}
        },
        "required": ["target", "confidence", "reason", "flags", "handoff_note"]
    })
}

/// What the orchestrator hands Herald: recent turns + the new message.
pub struct ClassifyInput<'a> {
    /// Recent turns, already trimmed/truncated by the caller.
    pub history_tail: &'a [ChatMessage],
    pub latest_message: &'a str,
    /// Whether Titan is currently warm for this thread (affects continuity).
    pub sticky_on_titan: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum HeraldError {
    #[error("herald timed out after {0}ms")]
    Timeout(u64),
    #[error("herald model failed: {0}")]
    Provider(String),
    #[error("unparseable herald output: {0}")]
    Parse(String),
    #[error("herald call cancelled")]
    Cancelled,
    #[error("herald task failed: {0}")]
    Task(String),
}

impl From<crate::error::CmdError> for HeraldError {
    fn from(e: crate::error::CmdError) -> Self {
        HeraldError::Task(e.to_string())
    }
}

/// Run one classification. Returns the decision (with `source` set) and the
/// wall-clock latency of the call.
pub async fn classify(
    provider: &dyn Provider,
    herald_model: &str,
    input: &ClassifyInput<'_>,
    timeout_ms: u64,
    keep_alive: &str,
    cancel: &CancellationToken,
) -> Result<(RoutingDecision, u64), HeraldError> {
    let req = build_classify_request(herald_model, input, keep_alive);
    let started = Instant::now();

    let raw = tokio::time::timeout(
        std::time::Duration::from_millis(timeout_ms),
        complete_once(provider, req, cancel),
    )
    .await
    .map_err(|_| HeraldError::Timeout(timeout_ms))??;

    let decision = parse_herald_json(&raw)?;
    Ok((decision, started.elapsed().as_millis() as u64))
}

/// Build the classify `ChatRequest` (pure; unit-tested).
pub fn build_classify_request(herald_model: &str, input: &ClassifyInput<'_>, keep_alive: &str) -> ChatRequest {
    let mut messages = Vec::with_capacity(2 + FEW_SHOTS.len() * 2);
    messages.push(ChatMessage {
        id: String::new(),
        role: ChatRole::System,
        content: ChatContent::Text(HERALD_SYSTEM.into()),
        tool_calls: None,
        tool_call_id: None,
        tool_name: None,
    });
    for (user, assistant) in FEW_SHOTS {
        messages.push(ChatMessage {
            id: String::new(),
            role: ChatRole::User,
            content: ChatContent::Text((*user).into()),
            tool_calls: None,
            tool_call_id: None,
            tool_name: None,
        });
        messages.push(ChatMessage {
            id: String::new(),
            role: ChatRole::Assistant,
            content: ChatContent::Text((*assistant).into()),
            tool_calls: None,
            tool_call_id: None,
            tool_name: None,
        });
    }
    messages.push(ChatMessage {
        id: String::new(),
        role: ChatRole::User,
        content: ChatContent::Text(render_input(input)),
        tool_calls: None,
        tool_call_id: None,
        tool_name: None,
    });

    ChatRequest {
        endpoint_id: String::new(), // filled by the caller's provider
        model: herald_model.to_string(),
        messages,
        tools: None,
        params: ChatParams {
            temperature: Some(0.0),
            top_p: None,
            num_ctx: None,
            max_tokens: Some(HERALD_MAX_TOKENS),
            json_schema: Some(decision_schema()),
            keep_alive: Some(keep_alive.to_string()),
        },
    }
}

fn render_input(input: &ClassifyInput<'_>) -> String {
    let mut out = String::new();
    if !input.history_tail.is_empty() {
        out.push_str("Recent conversation:\n");
        for msg in input.history_tail {
            let role = match msg.role {
                ChatRole::User => "user",
                ChatRole::Assistant => "assistant",
                ChatRole::System => "system",
                ChatRole::Tool => "tool",
            };
            out.push_str(&format!("[{role}] {}\n", text_of_content(&msg.content)));
        }
    }
    if input.sticky_on_titan {
        out.push_str("(A large task is currently running on titan in this thread.)\n");
    }
    out.push_str("\nLatest user message:\n");
    out.push_str(input.latest_message);
    out
}

fn text_of_content(content: &ChatContent) -> String {
    let text = match content {
        ChatContent::Text(t) => t.clone(),
        ChatContent::Parts(parts) => parts
            .iter()
            .filter_map(|p| match p {
                ContentPart::Text { text } => Some(text.as_str()),
                ContentPart::Image { .. } => Some("[image]"),
            })
            .collect::<Vec<_>>()
            .join(""),
    };
    if text.chars().count() > 400 {
        let truncated: String = text.chars().take(400).collect();
        format!("{truncated}…")
    } else {
        text
    }
}

/// Single-shot completion over the streaming provider trait: accumulate text
/// deltas until Done, surface Error, honor cancel. Used by Herald routing and
/// by the sidecar tasks. The provider future is driven to completion here —
/// a cancelled call drops its HTTP response inside the adapter, which is what
/// frees the server's generation slot.
pub async fn complete_once(
    provider: &dyn Provider,
    req: ChatRequest,
    cancel: &CancellationToken,
) -> Result<String, HeraldError> {
    let (tx, mut rx) = tokio::sync::mpsc::channel::<StreamEvent>(64);
    let mut chat_fut = provider.chat(req, tx, cancel.clone());

    let mut text = String::new();
    let mut error: Option<String> = None;
    let mut completed = false;
    loop {
        tokio::select! {
            biased;

            _ = cancel.cancelled() => break,

            ev = rx.recv() => match ev {
                Some(StreamEvent::TextDelta { text: t }) => text.push_str(&t),
                Some(StreamEvent::Error { message, .. }) => {
                    error = Some(message);
                    break;
                }
                Some(StreamEvent::Done) | None => break,
                Some(_) => {}
            },

            // Provider finished: `tx` drops with it, so `rx.recv()` above
            // returns None once any buffered events are drained.
            _ = &mut chat_fut => completed = true,
        }
    }
    if !completed {
        chat_fut.await;
    }

    if cancel.is_cancelled() {
        return Err(HeraldError::Cancelled);
    }
    match error {
        Some(message) => Err(HeraldError::Provider(message)),
        None => Ok(text),
    }
}

/// Lenient Herald JSON parse: tolerate prose wrappers, clamp ranges, default
/// missing fields. Only a missing/unknown `target` is fatal.
pub fn parse_herald_json(raw: &str) -> Result<RoutingDecision, HeraldError> {
    let candidate = extract_json_object(raw)
        .ok_or_else(|| HeraldError::Parse("no JSON object in output".into()))?;

    #[derive(serde::Deserialize)]
    struct HeraldFlags {
        vision: Option<bool>,
        tools: Option<bool>,
        code: Option<bool>,
        long_form: Option<bool>,
        multi_step: Option<bool>,
        sensitive: Option<bool>,
    }
    #[derive(serde::Deserialize)]
    struct HeraldOutput {
        target: Option<String>,
        confidence: Option<f64>,
        complexity: Option<u8>,
        reason: Option<String>,
        flags: Option<HeraldFlags>,
        est_in_tokens: Option<u64>,
        est_out_tokens: Option<u64>,
        handoff_note: Option<String>,
    }

    let out: HeraldOutput = serde_json::from_str(&candidate)
        .map_err(|e| HeraldError::Parse(format!("json: {e}")))?;

    let target = match out.target.as_deref() {
        Some("scout") => Target::Scout,
        Some("titan") => Target::Titan,
        other => return Err(HeraldError::Parse(format!("unknown target {other:?}"))),
    };

    Ok(RoutingDecision {
        target,
        confidence: out.confidence.unwrap_or(0.5).clamp(0.0, 1.0) as f32,
        complexity: out.complexity.unwrap_or(2).clamp(1, 5),
        reason: out.reason.unwrap_or_default(),
        flags: out
            .flags
            .map(|f| RoutingFlags {
                vision: f.vision.unwrap_or(false),
                tools: f.tools.unwrap_or(false),
                code: f.code.unwrap_or(false),
                long_form: f.long_form.unwrap_or(false),
                multi_step: f.multi_step.unwrap_or(false),
                sensitive: f.sensitive.unwrap_or(false),
            })
            .unwrap_or_default(),
        est_in_tokens: out.est_in_tokens.unwrap_or(0) as u32,
        est_out_tokens: out.est_out_tokens.unwrap_or(0) as u32,
        handoff_note: out.handoff_note.unwrap_or_default(),
        source: DecisionSource::Herald,
        vision_gap: None,
    })
}

/// Take the first `{ … }` block (bracket-balanced) so prose/fences around the
/// JSON don't kill the parse.
fn extract_json_object(raw: &str) -> Option<String> {
    let start = raw.find('{')?;
    let bytes = raw.as_bytes();
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    for (i, &b) in bytes.iter().enumerate().skip(start) {
        if escaped {
            escaped = false;
            continue;
        }
        match b {
            b'\\' if in_string => escaped = true,
            b'"' => in_string = !in_string,
            b'{' if !in_string => depth += 1,
            b'}' if !in_string => {
                depth -= 1;
                if depth == 0 {
                    return Some(raw[start..=i].to_string());
                }
            }
            _ => {}
        }
    }
    // Unbalanced: try from the first brace to the end (model truncated).
    Some(raw[start..].to_string())
}

/// Estimate the message signals the policy engine needs (pure; shared with
/// tests): image presence, size, code fences/keywords.
pub fn route_context_for(message: &str, has_image: bool) -> policy::RouteContext {
    policy::RouteContext {
        has_image,
        message_chars: message.chars().count(),
        message_words: message.split_whitespace().count(),
        prior_tool_results: 0, // tool runtime is M2
        code_fence_max_lines: policy::code_fence_max_lines(message),
        code_keywords: policy::code_keywords_hit(message),
        turns_on_titan: 0, // caller fills from history
        scout_vision: None,
        scout_context_tokens: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::test_support::FakeProvider;

    fn json_response(decision: &str) -> Vec<StreamEvent> {
        vec![
            StreamEvent::TextDelta {
                text: decision.into(),
            },
            StreamEvent::Done,
        ]
    }

    fn scout_json(conf: f64) -> String {
        format!(
            r#"{{"target":"scout","confidence":{conf},"complexity":1,"reason":"simple","flags":{{"vision":false,"tools":false,"code":false,"long_form":false,"multi_step":false,"sensitive":false}},"est_in_tokens":20,"est_out_tokens":30,"handoff_note":"answer briefly"}}"#
        )
    }

    #[test]
    fn parse_valid_json() {
        let d = parse_herald_json(&scout_json(0.88)).unwrap();
        assert_eq!(d.target, Target::Scout);
        assert!((d.confidence - 0.88).abs() < 1e-6);
        assert_eq!(d.complexity, 1);
        assert_eq!(d.reason, "simple");
        assert_eq!(d.handoff_note, "answer briefly");
        assert_eq!(d.source, DecisionSource::Herald);
    }

    #[test]
    fn parse_tolerates_prose_and_fences() {
        let raw = format!("Sure! Here is my decision:\n```json\n{}\n```\nDone.", scout_json(0.5));
        let d = parse_herald_json(&raw).unwrap();
        assert_eq!(d.target, Target::Scout);
    }

    #[test]
    fn parse_defaults_missing_fields_and_clamps() {
        let raw = r#"{"target":"titan","confidence":4.2,"complexity":9}"#;
        let d = parse_herald_json(raw).unwrap();
        assert_eq!(d.target, Target::Titan);
        assert!((d.confidence - 1.0).abs() < 1e-6, "clamped to 1.0");
        assert_eq!(d.complexity, 5, "clamped to 5");
        assert!(d.reason.is_empty());
        assert!(!d.flags.code);
    }

    #[test]
    fn parse_rejects_unknown_target_and_garbage() {
        assert!(parse_herald_json(r#"{"target":"scouty"}"#).is_err());
        assert!(parse_herald_json("no json here at all").is_err());
        assert!(parse_herald_json("").is_err());
    }

    #[test]
    fn classify_request_shape() {
        let input = ClassifyInput {
            history_tail: &[ChatMessage {
                id: "m1".into(),
                role: ChatRole::User,
                content: ChatContent::Text("hello there".into()),
                tool_calls: None,
                tool_call_id: None,
                tool_name: None,
            }],
            latest_message: "what is 15% of 82?",
            sticky_on_titan: false,
        };
        let req = build_classify_request("lfm2.5", &input, "24h");
        assert_eq!(req.model, "lfm2.5");
        assert_eq!(req.params.temperature, Some(0.0));
        assert_eq!(req.params.max_tokens, Some(200));
        assert_eq!(req.params.keep_alive.as_deref(), Some("24h"));
        assert!(req.params.json_schema.is_some());
        // system + 8 few-shot pairs + real input
        assert_eq!(req.messages.len(), 2 + FEW_SHOTS.len() * 2);
        assert!(matches!(req.messages.last().unwrap().role, ChatRole::User));
    }

    #[test]
    fn render_input_includes_sticky_note_and_truncates() {
        let long = "x".repeat(1000);
        let input = ClassifyInput {
            history_tail: &[ChatMessage {
                id: "m1".into(),
                role: ChatRole::Assistant,
                content: ChatContent::Text(long.clone()),
                tool_calls: None,
                tool_call_id: None,
                tool_name: None,
            }],
            latest_message: "hi",
            sticky_on_titan: true,
        };
        let rendered = render_input(&input);
        assert!(rendered.contains("running on titan"));
        assert!(rendered.len() < 600, "tail truncated to ≤400 chars + labels");
        assert!(rendered.ends_with("hi"));
    }

    #[tokio::test]
    async fn classify_success() {
        let provider = FakeProvider::scripted(json_response(&scout_json(0.9)));
        let input = ClassifyInput {
            history_tail: &[],
            latest_message: "hey",
            sticky_on_titan: false,
        };
        let (d, latency) = classify(&provider, "lfm2.5", &input, 1000, "24h", &CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(d.target, Target::Scout);
        assert!(latency <= 1000, "latency measured: {latency}");
    }

    #[tokio::test]
    async fn classify_timeout_falls_back() {
        let provider = FakeProvider::parking();
        let input = ClassifyInput {
            history_tail: &[],
            latest_message: "hey",
            sticky_on_titan: false,
        };
        let err = classify(&provider, "lfm2.5", &input, 30, "24h", &CancellationToken::new())
            .await
            .unwrap_err();
        assert!(matches!(err, HeraldError::Timeout(30)));
    }

    #[tokio::test]
    async fn classify_provider_error_surfaces() {
        let provider = FakeProvider::scripted(vec![StreamEvent::Error {
            code: "model_not_found".into(),
            message: "nope".into(),
            retryable: false,
        }]);
        let input = ClassifyInput {
            history_tail: &[],
            latest_message: "hey",
            sticky_on_titan: false,
        };
        let err = classify(&provider, "missing:model", &input, 1000, "24h", &CancellationToken::new())
            .await
            .unwrap_err();
        assert!(matches!(err, HeraldError::Provider(_)));
    }

    /// Live probe against real Ollama (`cargo test -- --ignored`).
    #[tokio::test]
    #[ignore = "requires local Ollama running on :11434"]
    async fn live_classify() {
        use crate::providers::ollama::OllamaAdapter;
        let adapter = OllamaAdapter::new(
            "ep_local_ollama",
            "http://127.0.0.1:11434",
            reqwest::Client::new(),
        );
        let models = adapter.list_models_impl().await.unwrap();
        // Use the smallest available model as a stand-in Herald.
        let model = models
            .iter()
            .map(|m| (m.size_bytes.unwrap_or(u64::MAX), m.id.clone()))
            .min()
            .unwrap()
            .1;
        println!("live classify with {model}");
        let input = ClassifyInput {
            history_tail: &[],
            latest_message: "Refactor the auth module to use refresh tokens across 5 files",
            sticky_on_titan: false,
        };
        let (d, ms) = classify(&adapter, &model, &input, 8000, "24h", &CancellationToken::new())
            .await
            .unwrap();
        println!("live decision ({ms} ms): {d:?}");
        assert_eq!(d.target, Target::Titan);
    }
}