//! One-stream orchestration (design §3.2 lifecycle): history assembly, event
//! pump with throttled persistence, cancellation, usage capture, auto-title.
//!
//! Testability seam: the UI forwarding step is a plain closure, so the pump
//! is testable without a Tauri AppHandle (fake provider in tests).

use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::mpsc;

use crate::{
    commands::chat::ChatSendArgs,
    error::CmdError,
    ids,
    providers::Provider,
    state::{AppState, StreamEntry, DEFAULT_ENDPOINT},
    types::{
        ChatContent, ChatMessage, ChatParams, ChatRequest, ChatRole, ChatSendResult, ContentPart,
        StatusPhase, StreamEvent,
    },
};

/// Max partial-persist interval / size (design: ~2-3 DB writes per second
/// while streaming, one final write).
const FLUSH_INTERVAL: Duration = Duration::from_millis(400);
const FLUSH_CHARS: usize = 1024;

/// Run one send-to-completion exchange. Registers the stream for
/// cancellation; returns the final result.
pub async fn send(
    state: &crate::state::AppState,
    args: ChatSendArgs,
    forward: Arc<dyn Fn(&StreamEvent) + Send + Sync>,
) -> Result<ChatSendResult, CmdError> {
    // 1. Guard: one active stream per conversation.
    if state
        .streams
        .read()
        .unwrap_or_else(|p| p.into_inner())
        .contains_key(&args.conversation_id)
    {
        return Err(CmdError::stream_active());
    }

    let cancel = tokio_util::sync::CancellationToken::new();
    let message_id = ids::new_id();
    state
        .streams
        .write()
        .unwrap_or_else(|p| p.into_inner())
        .insert(
            args.conversation_id.clone(),
            StreamEntry {
                cancel: cancel.clone(),
                message_id: message_id.clone(),
            },
        );

    let result =
        stream_once(state, &args, &message_id, &cancel, forward).await;

    state
        .streams
        .write()
        .unwrap_or_else(|p| p.into_inner())
        .remove(&args.conversation_id);
    result
}

async fn stream_once(
    state: &AppState,
    args: &ChatSendArgs,
    message_id: &str,
    cancel: &tokio_util::sync::CancellationToken,
    forward: Arc<dyn Fn(&StreamEvent) + Send + Sync>,
) -> Result<ChatSendResult, CmdError> {
    let conv_id = args.conversation_id.clone();
    let model = args.model.clone();
    let endpoint_id = DEFAULT_ENDPOINT.to_string();
    let now = ids::now_ms();

    // 2. Idempotent user message (client-generated id; retry-safe).
    state
        .db
        .insert_user_message(
            args.user_message_id.clone(),
            conv_id.clone(),
            vec![ContentPart::Text {
                text: args.content.clone(),
            }],
            now,
        )
        .await?;

    // 3. Assistant placeholder — a crash mid-stream leaves a visible stub.
    state
        .db
        .insert_assistant_placeholder(
            message_id.to_string(),
            conv_id.clone(),
            model.clone(),
            endpoint_id.clone(),
            now,
        )
        .await?;

    // 4. Assemble context: history + optional system prompt. The placeholder
    // and other stale streaming rows are excluded.
    let history = state.db.get_messages(conv_id.clone()).await?;
    let mut messages: Vec<ChatMessage> = Vec::with_capacity(history.len() + 1);
    if let Some(system_prompt) = state
        .db
        .get_conversation(conv_id.clone())
        .await?
        .and_then(|c| c.system_prompt)
        .filter(|s| !s.trim().is_empty())
    {
        messages.push(ChatMessage {
            id: ids::new_id(),
            role: ChatRole::System,
            content: ChatContent::Text(system_prompt),
            tool_calls: None,
            tool_call_id: None,
        });
    }
    for msg in &history {
        if msg.id == message_id || msg.status == crate::types::MessageStatus::Streaming {
            continue; // placeholder(s) — crashed stubs were swept to 'error' at startup
        }
        messages.push(ChatMessage {
            id: msg.id.clone(),
            role: msg.role,
            content: ChatContent::Parts(msg.content.clone()),
            tool_calls: None,
            tool_call_id: None,
        });
    }

    // 5. Request with params from settings.
    let temperature: f32 = state
        .settings
        .get_or(crate::settings::keys::CHAT_TEMPERATURE, "0.7")
        .parse()
        .unwrap_or(0.7);
    let num_ctx: u32 = state
        .settings
        .get_or(crate::settings::keys::CHAT_CONTEXT_TOKENS, "8192")
        .parse()
        .unwrap_or(8192);
    let keep_alive = state.settings.get_or(crate::settings::keys::CHAT_KEEP_ALIVE, "10m");
    let req = ChatRequest {
        endpoint_id: endpoint_id.clone(),
        model: model.clone(),
        messages,
        tools: None, // tool runtime is M2
        params: ChatParams {
            temperature: Some(temperature),
            top_p: None,
            num_ctx: Some(num_ctx),
            max_tokens: None,
            json_schema: None,
            keep_alive: Some(keep_alive),
        },
    };

    forward(&StreamEvent::Status {
        phase: StatusPhase::Connecting,
    });

    // 6. Spawn the provider stream.
    let (tx, mut rx) = mpsc::channel::<StreamEvent>(64);
    let provider: Arc<dyn Provider> = state.provider_for(DEFAULT_ENDPOINT)?;
    let cancel_for_task = cancel.clone();
    tokio::spawn(async move {
        provider.chat(req, tx, cancel_for_task).await;
    });

    // 7. Consume: forward every event, accumulate, flush periodically.
    let mut text = String::new();
    let mut reasoning = String::new();
    let mut usage: Option<(u64, u64, u64)> = None;
    let mut last_flush = Instant::now();
    let mut final_status = crate::types::MessageStatus::Complete;
    let mut final_error: Option<String> = None;

    loop {
        tokio::select! {
            biased;

            _ = cancel.cancelled() => {
                final_status = crate::types::MessageStatus::Stopped;
                break;
            }

            ev = rx.recv() => {
                let Some(ev) = ev else { break }; // provider ended without done

                forward(&ev);

                match ev {
                    StreamEvent::TextDelta { text: t } => text.push_str(&t),
                    StreamEvent::ReasoningDelta { text: t } => reasoning.push_str(&t),
                    StreamEvent::Usage { tokens_in, tokens_out, latency_ms } => {
                        usage = Some((tokens_in, tokens_out, latency_ms));
                    }
                    StreamEvent::Error { code, message, .. } => {
                        final_status = crate::types::MessageStatus::Error;
                        final_error = Some(format!("{code}: {message}"));
                        break;
                    }
                    StreamEvent::Done => {
                        final_status = crate::types::MessageStatus::Complete;
                        break;
                    }
                    // status / routing (M1) / tool_* (M2): forwarded above.
                    _ => {}
                }

                if text.len() >= FLUSH_CHARS || last_flush.elapsed() >= FLUSH_INTERVAL {
                    persist_partial(state, message_id, &reasoning, &text).await?;
                    last_flush = Instant::now();
                }
            }
        }
    }

    // 8. Final persist (covers any un-flushed partial).
    let (tokens_in, tokens_out, latency_ms) = usage.unwrap_or((0, 0, 0));
    let reasoning_opt = if reasoning.is_empty() {
        None
    } else {
        Some(reasoning.clone())
    };
    state
        .db
        .update_assistant_message(
            message_id.to_string(),
            parts_for(&text),
            reasoning_opt,
            final_status,
            Some(tokens_in),
            Some(tokens_out),
            Some(latency_ms),
            final_error.clone(),
        )
        .await?;

    state
        .db
        .touch_conversation(conv_id.clone(), ids::now_ms())
        .await?;

    // Sidecar title seam (Herald replaces this in M1): first user message
    // becomes the title, only while it's still NULL.
    state
        .db
        .auto_title(conv_id.clone(), title_from(&args.content))
        .await?;

    Ok(ChatSendResult {
        message_id: message_id.to_string(),
        status: final_status,
        tokens_in,
        tokens_out,
        latency_ms,
    })
}

async fn persist_partial(
    state: &AppState,
    message_id: &str,
    reasoning: &str,
    text: &str,
) -> Result<(), CmdError> {
    state
        .db
        .update_assistant_message(
            message_id.to_string(),
            parts_for(text),
            if reasoning.is_empty() { None } else { Some(reasoning.to_string()) },
            crate::types::MessageStatus::Streaming,
            None,
            None,
            None,
            None,
        )
        .await
}

fn parts_for(text: &str) -> Vec<ContentPart> {
    if text.is_empty() {
        Vec::new()
    } else {
        vec![ContentPart::Text { text: text.to_string() }]
    }
}

fn title_from(content: &str) -> String {
    let collapsed = content.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.chars().count() <= 60 {
        collapsed
    } else {
        let truncated: String = collapsed.chars().take(60).collect();
        format!("{truncated}…")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        error::CmdError,
        providers::{EndpointKind, ProviderError},
        state::AppState,
        types::{MessageStatus, ModelInfo},
    };
    use std::collections::HashMap;

    /// Scripted provider: emits `events` in order; optionally waits for the
    /// cancellation token before continuing.
    struct FakeProvider {
        events: Vec<StreamEvent>,
        wait_cancel_at: Option<usize>,
    }

    impl FakeProvider {
        fn scripted(events: Vec<StreamEvent>) -> Self {
            Self {
                events,
                wait_cancel_at: None,
            }
        }

        fn cancelling_after(events: Vec<StreamEvent>) -> Self {
            Self {
                events,
                wait_cancel_at: Some(1), // after the first event is sent
            }
        }
    }

    impl Provider for FakeProvider {
        fn id(&self) -> &str {
            "ep_fake"
        }
        fn kind(&self) -> EndpointKind {
            EndpointKind::Ollama
        }
        fn list_models(&self) -> futures::future::BoxFuture<'_, Result<Vec<ModelInfo>, ProviderError>> {
            Box::pin(async { Ok(vec![]) })
        }
        fn chat(
            &self,
            _req: ChatRequest,
            tx: mpsc::Sender<StreamEvent>,
            cancel: tokio_util::sync::CancellationToken,
        ) -> futures::future::BoxFuture<'_, ()> {
            let events = self.events.clone();
            let wait_cancel_at = self.wait_cancel_at;
            Box::pin(async move {
                for (i, ev) in events.into_iter().enumerate() {
                    if wait_cancel_at == Some(i) {
                        cancel.cancelled().await;
                        return;
                    }
                    if tx.send(ev).await.is_err() {
                        return;
                    }
                }
            })
        }
    }

    async fn app_with(provider: FakeProvider) -> (tempfile::TempDir, AppState) {
        let dir = tempfile::tempdir().unwrap();
        let db = crate::db::Database::open(&dir.path().join("t.db")).unwrap();
        let providers: HashMap<String, Arc<dyn Provider>> =
            HashMap::from([("ep_local_ollama".into(), Arc::new(provider) as Arc<dyn Provider>)]);
        let settings = crate::settings::SettingsCache::new(HashMap::from([
            ("ollama.base_url".to_string(), "http://127.0.0.1:11434".to_string()),
            ("chat.temperature".to_string(), "0.7".to_string()),
            ("chat.context_tokens".to_string(), "8192".to_string()),
            ("chat.keep_alive".to_string(), "10m".to_string()),
        ]));
        let state = AppState::new(db, settings, reqwest::Client::new(), providers);
        (dir, state)
    }

    fn args(content: &str) -> ChatSendArgs {
        ChatSendArgs {
            conversation_id: "c1".into(),
            user_message_id: "um1".into(),
            content: content.into(),
            model: "fake-model".into(),
        }
    }

    fn noop_forward() -> Arc<dyn Fn(&StreamEvent) + Send + Sync> {
        Arc::new(|_| {})
    }

    async fn messages(state: &AppState) -> Vec<crate::types::Message> {
        state.db.get_messages("c1".into()).await.unwrap()
    }

    #[tokio::test]
    async fn complete_stream_persists_usage_and_title() {
        let (_dir, state) = app_with(FakeProvider::scripted(vec![
            StreamEvent::TextDelta { text: "hel".into() },
            StreamEvent::TextDelta { text: "lo".into() },
            StreamEvent::Usage { tokens_in: 17, tokens_out: 4, latency_ms: 123 },
            StreamEvent::Done,
        ]))
        .await;

        state
            .db
            .create_conversation("c1".into(), ids::now_ms())
            .await
            .unwrap();

        let result = chat_send_inner(&state, args("What is 15% of 82?")).await.unwrap();
        assert_eq!(result.status, MessageStatus::Complete);
        assert_eq!(result.tokens_in, 17);
        assert_eq!(result.tokens_out, 4);
        assert_eq!(result.latency_ms, 123);

        let msgs = messages(&state).await;
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[1].status, MessageStatus::Complete);
        assert_eq!(msgs[1].tokens_in, Some(17));
        assert_eq!(msgs[1].latency_ms, Some(123));
        match &msgs[1].content[0] {
            ContentPart::Text { text } => assert_eq!(text, "hello"),
            _ => unreachable!(),
        }

        // Auto-title from the first user message.
        let conv = state.db.get_conversation("c1".into()).await.unwrap().unwrap();
        assert_eq!(conv.title.as_deref(), Some("What is 15% of 82?"));
    }

    #[tokio::test]
    async fn cancel_persists_partial_as_stopped() {
        let (_dir, state) =
            app_with(FakeProvider::cancelling_after(vec![
                StreamEvent::TextDelta { text: "par".into() },
                StreamEvent::TextDelta { text: "tial".into() },
            ]))
            .await;
        state
            .db
            .create_conversation("c1".into(), ids::now_ms())
            .await
            .unwrap();

        // Cancel as soon as the first delta is forwarded. The fake provider
        // parks on the token after its first event, so someone must flip it —
        // join! polls both futures on this runtime without needing 'static.
        let first_delta = Arc::new(tokio::sync::Notify::new());
        let notify = first_delta.clone();
        let forward = Arc::new(move |ev: &StreamEvent| {
            if matches!(ev, StreamEvent::TextDelta { .. }) {
                notify.notify_one();
            }
        }) as Arc<dyn Fn(&StreamEvent) + Send + Sync>;

        let (result, ()) = tokio::join!(
            send(&state, args("write an essay"), forward),
            async {
                first_delta.notified().await;
                let entry = state
                    .streams
                    .read()
                    .unwrap_or_else(|p| p.into_inner())
                    .get("c1")
                    .cloned()
                    .expect("stream registered before first delta");
                entry.cancel.cancel();
            },
        );
        let result = result.unwrap();
        assert_eq!(result.status, MessageStatus::Stopped);
        let msgs = messages(&state).await;
        assert_eq!(msgs[1].status, MessageStatus::Stopped);
        match &msgs[1].content[0] {
            ContentPart::Text { text } => assert_eq!(text, "par"),
            _ => unreachable!(),
        }
    }

    #[tokio::test]
    async fn error_event_persists_error_row() {
        let (_dir, state) = app_with(FakeProvider::scripted(vec![
            StreamEvent::TextDelta { text: "x".into() },
            StreamEvent::Error {
                code: "model_not_found".into(),
                message: "model 'x' not found".into(),
                retryable: false,
            },
        ]))
        .await;
        state
            .db
            .create_conversation("c1".into(), ids::now_ms())
            .await
            .unwrap();

        let result = chat_send_inner(&state, args("hi")).await.unwrap();
        assert_eq!(result.status, MessageStatus::Error);
        let msgs = messages(&state).await;
        assert_eq!(msgs[1].status, MessageStatus::Error);
        assert!(msgs[1].error.as_deref().unwrap().contains("model_not_found"));
    }

    #[tokio::test]
    async fn double_send_is_rejected() {
        let (_dir, state) = app_with(FakeProvider::scripted(vec![])).await;
        state
            .db
            .create_conversation("c1".into(), ids::now_ms())
            .await
            .unwrap();

        // Simulate an active stream.
        state
            .streams
            .write()
            .unwrap_or_else(|p| p.into_inner())
            .insert(
                "c1".into(),
                StreamEntry {
                    cancel: tokio_util::sync::CancellationToken::new(),
                    message_id: "m0".into(),
                },
            );

        let err = chat_send_inner(&state, args("hi")).await.unwrap_err();
        assert_eq!(err.code, "stream_active");
    }

    #[tokio::test]
    async fn thinking_stream_persists_reasoning() {
        let (_dir, state) = app_with(FakeProvider::scripted(vec![
            StreamEvent::ReasoningDelta { text: "let me think".into() },
            StreamEvent::TextDelta { text: "42".into() },
            StreamEvent::Usage { tokens_in: 5, tokens_out: 2, latency_ms: 10 },
            StreamEvent::Done,
        ]))
        .await;
        state
            .db
            .create_conversation("c1".into(), ids::now_ms())
            .await
            .unwrap();

        chat_send_inner(&state, args("reason about this")).await.unwrap();
        let msgs = messages(&state).await;
        assert_eq!(msgs[1].reasoning.as_deref(), Some("let me think"));
    }

    /// Test-only entry mirroring the command body (forward closure).
    async fn chat_send_inner(
        state: &AppState,
        args: ChatSendArgs,
    ) -> Result<ChatSendResult, CmdError> {
        send(state, args, noop_forward()).await
    }
}