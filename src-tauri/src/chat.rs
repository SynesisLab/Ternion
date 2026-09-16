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
    router::{
        config::{Pin, TriadConfig},
        herald, policy, sidecars,
    },
    state::{AppState, StreamEntry, DEFAULT_ENDPOINT},
    types::{
        ChatContent, ChatMessage, ChatParams, ChatRequest, ChatRole, ChatSendResult, ContentPart,
        DecisionSource, MessageStatus, Message as DbMessage, ModelRole, RoutingDecision,
        StatusPhase, StreamEvent, Target,
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
    let endpoint_id = DEFAULT_ENDPOINT.to_string();
    let now = ids::now_ms();

    // Stale suggestion chips from the previous exchange are void now.
    state.take_suggestions(&conv_id);

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
    //    The model id is provisional; routing corrects it below.
    state
        .db
        .insert_assistant_placeholder(
            message_id.to_string(),
            conv_id.clone(),
            args.model.clone(),
            endpoint_id.clone(),
            now,
        )
        .await?;

    // 4. History for context and routing signals.
    let history = state.db.get_messages(conv_id.clone()).await?;

    // 5. Route this turn (Triad §3.2): pin resolution → Herald classify →
    //    policy engine. Emits Status{routing} + Routing{decision} to the UI
    //    and persists the routing event.
    let routed = route_turn(state, args, &history, message_id, cancel, &forward).await?;

    // 6. Assemble context. On a model switch (escalation/de-escalation/pin
    //    change, §3.6) the new model receives the rolling digest + a
    //    prior-model note + only the recent tail; same-model turns get the
    //    full history as before.
    let conv = state
        .db
        .get_conversation(conv_id.clone())
        .await?
        .ok_or_else(|| CmdError::internal("conversation vanished mid-send"))?;
    // Last *completed* assistant decides whether this turn is a handoff — the
    // provisional placeholder row (status streaming, args.model) must not
    // count, or every auto-routed first turn looks like a model switch.
    let switching = match history
        .iter()
        .rev()
        .find(|m| m.role == ChatRole::Assistant && m.status != MessageStatus::Streaming)
    {
        Some(prev) => prev.model_id.as_deref() != Some(routed.model.as_str()),
        None => false,
    };
    let mut messages: Vec<ChatMessage> = Vec::with_capacity(history.len() + 3);
    if let Some(system_prompt) = conv
        .system_prompt
        .clone()
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
    if switching {
        if let Some(digest) = state.db.get_conversation_digest(conv_id.clone()).await? {
            if !digest.trim().is_empty() {
                messages.push(ChatMessage {
                    id: ids::new_id(),
                    role: ChatRole::System,
                    content: ChatContent::Text(format!(
                        "ROLLING DIGEST (task state from earlier turns): {digest}"
                    )),
                    tool_calls: None,
                    tool_call_id: None,
                });
            }
        }
        if !routed.decision.handoff_note.is_empty() {
            let prev_role = last_assistant_role(&history);
            let prev = prev_role
                .map(|r| format!("the {r} model"))
                .unwrap_or_else(|| "a different model".to_string());
            messages.push(ChatMessage {
                id: ids::new_id(),
                role: ChatRole::System,
                content: ChatContent::Text(format!(
                    "Prior-model note: you are taking over from {prev}. \
                     Handoff from the router: {}",
                    routed.decision.handoff_note
                )),
                tool_calls: None,
                tool_call_id: None,
            });
        }
    }
    let cfg = TriadConfig::load(&state.settings);
    let history_slice: &[DbMessage] = if switching {
        let keep = cfg.handoff_recent_messages as usize;
        if history.len() > keep {
            &history[history.len() - keep..]
        } else {
            &history
        }
    } else {
        &history
    };
    for msg in history_slice {
        if msg.id == message_id || msg.status == MessageStatus::Streaming {
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

    // 7. Request with params from settings; keep-alive follows the role.
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
    let req = ChatRequest {
        endpoint_id: endpoint_id.clone(),
        model: routed.model.clone(),
        messages,
        tools: None, // tool runtime is M2
        params: ChatParams {
            temperature: Some(temperature),
            top_p: None,
            num_ctx: Some(num_ctx),
            max_tokens: None,
            json_schema: None,
            keep_alive: Some(routed.keep_alive.clone()),
        },
    };

    forward(&StreamEvent::Status {
        phase: StatusPhase::Connecting,
    });

    // 8. Spawn the provider stream.
    let (tx, mut rx) = mpsc::channel::<StreamEvent>(64);
    let provider: Arc<dyn Provider> = state.provider_for(DEFAULT_ENDPOINT)?;
    let cancel_for_task = cancel.clone();
    tokio::spawn(async move {
        provider.chat(req, tx, cancel_for_task).await;
    });

    // 9. Consume: forward every event, accumulate, flush periodically.
    let mut text = String::new();
    let mut reasoning = String::new();
    let mut usage: Option<(u64, u64, u64)> = None;
    let mut last_flush = Instant::now();
    let mut final_status = MessageStatus::Complete;
    let mut final_error: Option<String> = None;

    loop {
        tokio::select! {
            biased;

            _ = cancel.cancelled() => {
                final_status = MessageStatus::Stopped;
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
                        final_status = MessageStatus::Error;
                        final_error = Some(format!("{code}: {message}"));
                        break;
                    }
                    StreamEvent::Done => {
                        final_status = MessageStatus::Complete;
                        break;
                    }
                    // status / routing / tool_* (M2): forwarded above.
                    _ => {}
                }

                if text.len() >= FLUSH_CHARS || last_flush.elapsed() >= FLUSH_INTERVAL {
                    persist_partial(state, message_id, &reasoning, &text).await?;
                    last_flush = Instant::now();
                }
            }
        }
    }

    // 10. Final persist (covers any un-flushed partial).
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

    // Fallback title (instant): first user message, only while still NULL.
    // The Herald sidecar refines it after the second exchange.
    state
        .db
        .auto_title(conv_id.clone(), title_from(&args.content))
        .await?;

    // 11. Sidecars (§3.7) — out-of-band, best-effort, never block the result.
    spawn_sidecars(state, args, &history, &text, switching, final_status).await;

    // Scout ran past its output ceiling (§3.6 trigger 4) — the UI offers
    // "⚡ Continue with Titan".
    let escalation_available = routed.role == Some(ModelRole::Scout)
        && final_status == MessageStatus::Complete
        && tokens_out > u64::from(cfg.scout_output_ceiling);

    Ok(ChatSendResult {
        message_id: message_id.to_string(),
        status: final_status,
        tokens_in,
        tokens_out,
        latency_ms,
        escalation_available,
    })
}

/// One turn's routing outcome: the resolved model + Triad role, the policy
/// decision (persisted to `routing_events` and forwarded to the UI), and the
/// keep-alive policy that follows the role.
struct RoutedTurn {
    model: String,
    role: Option<ModelRole>,
    decision: RoutingDecision,
    keep_alive: String,
}

/// Triad routing for one turn (design §3.5 precedence): user pins first
/// (manual), then Herald classify → policy engine when auto. Always emits
/// `Routing` so the UI can show who is about to answer, and always persists
/// a `routing_events` row (§3.11) — manual pins included.
async fn route_turn(
    state: &AppState,
    args: &ChatSendArgs,
    history: &[DbMessage],
    message_id: &str,
    cancel: &tokio_util::sync::CancellationToken,
    forward: &Arc<dyn Fn(&StreamEvent) + Send + Sync>,
) -> Result<RoutedTurn, CmdError> {
    let cfg = TriadConfig::load(&state.settings);
    let conv = state
        .db
        .get_conversation(args.conversation_id.clone())
        .await?
        .ok_or_else(|| CmdError::internal("conversation vanished before routing"))?;
    let pin = Pin::parse(conv.pinned_model.as_deref());
    let started = Instant::now();

    let mut decision = RoutingDecision::default();
    let mut role: Option<ModelRole> = None;
    let mut override_kind: Option<String> = None;
    let mut herald_latency: Option<u64> = None;
    let mut keep_alive = state
        .settings
        .get_or(crate::settings::keys::CHAT_KEEP_ALIVE, "10m");
    let model: String;

    let triad_off = !cfg.enabled || cfg.skip_router || !cfg.routing_available();
    match pin {
        Pin::Model(m) => {
            let target = cfg.roles.role_of(&m);
            role = target.map(role_of);
            decision.target = target.unwrap_or(Target::Scout);
            decision.reason = "pinned model".into();
            decision.source = DecisionSource::Manual;
            decision.confidence = 1.0;
            override_kind = Some("manual".into());
            model = m;
        }
        Pin::Role(t) => {
            model = cfg
                .roles
                .model_for(t)
                .unwrap_or_else(|| args.model.as_str())
                .to_string();
            role = Some(role_of(t));
            decision.target = t;
            decision.reason = format!("pinned {} for this chat", t.as_str());
            decision.source = DecisionSource::Manual;
            decision.confidence = 1.0;
            override_kind = Some("manual".into());
            keep_alive = role_keep_alive(t, &cfg);
        }
        Pin::Auto => {
            if triad_off {
                model = args.model.clone();
                decision.reason = if cfg.skip_router {
                    "router disabled".into()
                } else {
                    "triad not configured".into()
                };
                decision.source = DecisionSource::Manual;
                decision.confidence = 1.0;
                override_kind = Some("manual".into());
            } else {
                // Routing latency is visible work, not a silent stall (§3.2).
                forward(&StreamEvent::Status {
                    phase: StatusPhase::Routing,
                });

                let turns_on_titan = turns_on_titan(history);
                let mut ctx = herald::route_context_for(&args.content, false);
                ctx.turns_on_titan = turns_on_titan;
                apply_capability_facts(state, &cfg, &mut ctx);

                let herald_decision = match cfg.roles.herald.as_deref() {
                    Some(herald_model) => {
                        let provider = state.provider_for(DEFAULT_ENDPOINT)?;
                        // Tail = completed rows before the just-added user
                        // message (which is passed separately as
                        // latest_message) — skip it and any streaming stubs.
                        let tail: Vec<ChatMessage> = history
                            .iter()
                            .filter(|m| {
                                m.status != MessageStatus::Streaming
                                    && m.id != args.user_message_id
                            })
                            .map(to_chat_message)
                            .collect();
                        let input = herald::ClassifyInput {
                            history_tail: &tail,
                            latest_message: &args.content,
                            sticky_on_titan: turns_on_titan > 0,
                        };
                        match herald::classify(
                            provider.as_ref(),
                            herald_model,
                            &input,
                            cfg.herald_timeout_ms,
                            &cfg.herald_keep_alive,
                            cancel,
                        )
                        .await
                        {
                            Ok((d, ms)) => {
                                herald_latency = Some(ms);
                                Some(d)
                            }
                            Err(e) => {
                                log::warn!("herald classify failed: {e}");
                                None
                            }
                        }
                    }
                    None => None,
                };

                decision = policy::decide(&cfg, &ctx, herald_decision);
                role = Some(role_of(decision.target));
                model = cfg
                    .roles
                    .model_for(decision.target)
                    .unwrap_or_else(|| args.model.as_str())
                    .to_string();
                keep_alive = role_keep_alive(decision.target, &cfg);
            }
        }
    }

    // Persist (§3.11) and attach the outcome to the assistant row.
    let event_id = ids::new_id();
    let latency_ms =
        herald_latency.unwrap_or_else(|| started.elapsed().as_millis() as u64);
    state
        .db
        .insert_routing_event(
            event_id.clone(),
            args.conversation_id.clone(),
            message_id.to_string(),
            decision.clone(),
            decision.target,
            model.clone(),
            latency_ms,
            override_kind,
        )
        .await?;
    state
        .db
        .set_message_routing(
            message_id.to_string(),
            role,
            model.clone(),
            DEFAULT_ENDPOINT.to_string(),
            Some(event_id.clone()),
        )
        .await?;

    forward(&StreamEvent::Routing {
        decision: decision.clone(),
        final_target: decision.target,
    });

    Ok(RoutedTurn {
        model,
        role,
        decision,
        keep_alive,
    })
}

fn to_chat_message(msg: &DbMessage) -> ChatMessage {
    ChatMessage {
        id: msg.id.clone(),
        role: msg.role,
        content: ChatContent::Parts(msg.content.clone()),
        tool_calls: None,
        tool_call_id: None,
    }
}

/// Consecutive completed assistant turns currently on Titan — the sticky
/// window input (§3.6). Streaming placeholders don't break or extend it.
fn turns_on_titan(history: &[DbMessage]) -> u32 {
    let mut n = 0u32;
    for msg in history.iter().rev() {
        if msg.status == MessageStatus::Streaming {
            continue; // our own placeholder
        }
        match msg.role {
            ChatRole::Assistant => {
                if msg.model_role == Some(ModelRole::Titan) {
                    n += 1;
                } else {
                    break;
                }
            }
            _ => continue, // user/system rows don't break the streak
        }
    }
    n
}

/// Capability facts for the scout role from the discovery cache — `None`
/// fields mean "unknown", which disables rather than guesses (§3.5 rule 2).
fn apply_capability_facts(state: &AppState, cfg: &TriadConfig, ctx: &mut policy::RouteContext) {
    if let Some(scout) = cfg.roles.scout.as_deref().and_then(|m| state.registry_model(m)) {
        ctx.scout_vision = Some(scout.capabilities.iter().any(|c| c == "vision"));
        ctx.scout_context_tokens = scout.context_length;
    }
}

fn role_of(t: Target) -> ModelRole {
    match t {
        Target::Scout => ModelRole::Scout,
        Target::Titan => ModelRole::Titan,
    }
}

fn role_keep_alive(t: Target, cfg: &TriadConfig) -> String {
    match t {
        Target::Scout => cfg.scout_keep_alive.clone(),
        Target::Titan => cfg.titan_keep_alive.clone(),
    }
}

/// Role of the most recent completed assistant turn, for handoff notes.
fn last_assistant_role(history: &[DbMessage]) -> Option<String> {
    history
        .iter()
        .rev()
        .find(|m| m.role == ChatRole::Assistant && m.status != MessageStatus::Streaming)
        .and_then(|m| m.model_role.map(|r| r.as_str().to_string()))
}

/// Fire-and-forget Herald sidecars after a stream (§3.7): refined title,
/// rolling digest on schedule, follow-up suggestions. Never blocks, never fails.
async fn spawn_sidecars(
    state: &AppState,
    args: &ChatSendArgs,
    history: &[DbMessage],
    answer: &str,
    switching: bool,
    final_status: MessageStatus,
) {
    let Some(ctx) = sidecars::sidecar_ctx(state) else {
        return; // no Herald assigned — sidecars are off
    };
    let cfg = TriadConfig::load(&state.settings);
    if !cfg.enabled {
        return;
    }

    let assistant_count = history
        .iter()
        .filter(|m| m.role == ChatRole::Assistant && m.status != MessageStatus::Streaming)
        .count()
        + 1;
    let with_answer = |limit: usize, chars: usize| -> String {
        let mut t = sidecars::transcript(history, limit, chars);
        if !answer.trim().is_empty() {
            t.push_str(&format!("[assistant] {}\n", truncate_chars(answer, 600)));
        }
        t
    };

    // Title: replace the truncation fallback once, after the 2nd exchange.
    if cfg.sidecar_titles && assistant_count >= 2 {
        let first_user = history
            .iter()
            .find(|m| m.role == ChatRole::User)
            .map(|m| {
                m.content
                    .iter()
                    .filter_map(|p| match p {
                        ContentPart::Text { text } => Some(text.as_str()),
                        ContentPart::Image { .. } => None,
                    })
                    .collect::<String>()
            })
            .unwrap_or_default();
        let fallback_title = title_from(&first_user);
        if let Some(title) = state
            .db
            .get_conversation(args.conversation_id.clone())
            .await
            .ok()
            .flatten()
            .and_then(|c| c.title)
        {
            if title == fallback_title {
                sidecars::spawn_title(
                    ctx.clone(),
                    args.conversation_id.clone(),
                    with_answer(6, 300),
                );
            }
        }
    }

    // Rolling digest: every 3 turns, or right after a model handoff (§3.6).
    if switching || assistant_count % 3 == 0 {
        sidecars::spawn_digest(
            ctx.clone(),
            args.conversation_id.clone(),
            with_answer(6, 300),
        );
    }

    // Follow-up chips after each completed answer.
    if final_status == MessageStatus::Complete && cfg.sidecar_suggestions {
        sidecars::spawn_suggestions(
            ctx,
            args.conversation_id.clone(),
            with_answer(4, 400),
        );
    }
}

fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let truncated: String = s.chars().take(max).collect();
        format!("{truncated}…")
    }
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

// ---------------------------------------------------------------------------
// Tests — shared FakeProvider (providers::test_support) + M1 routing flows.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        providers::test_support::{app_with, app_with_settings, FakeProvider},
        settings::keys,
    };

    fn args_with_id(id: &str, content: &str) -> ChatSendArgs {
        ChatSendArgs {
            conversation_id: "c1".into(),
            user_message_id: id.into(),
            content: content.into(),
            model: "fallback-model".into(),
        }
    }

    fn args(content: &str) -> ChatSendArgs {
        args_with_id("um1", content)
    }

    fn noop_forward() -> Arc<dyn Fn(&StreamEvent) + Send + Sync> {
        Arc::new(|_| {})
    }

    /// Test-only entry mirroring the command body (forward closure).
    async fn chat_send_inner(
        state: &AppState,
        args: ChatSendArgs,
    ) -> Result<ChatSendResult, CmdError> {
        send(state, args, noop_forward()).await
    }

    async fn create_conv(state: &AppState) {
        state
            .db
            .create_conversation("c1".into(), ids::now_ms())
            .await
            .unwrap();
    }

    async fn messages(state: &AppState) -> Vec<DbMessage> {
        state.db.get_messages("c1".into()).await.unwrap()
    }

    fn triad_roles() -> Vec<(&'static str, &'static str)> {
        vec![
            (keys::TRIAD_ROLE_HERALD, "herald-model"),
            (keys::TRIAD_ROLE_SCOUT, "scout-model"),
            (keys::TRIAD_ROLE_TITAN, "titan-model"),
        ]
    }

    /// Herald classify reply: structured-output JSON arriving as a text delta.
    fn classify_json(target: &str, conf: f32, note: &str) -> Vec<StreamEvent> {
        vec![StreamEvent::TextDelta {
            text: format!(
                r#"{{"target":"{target}","confidence":{conf:.2},"complexity":2,"reason":"test","flags":{{"vision":false,"tools":false,"code":false,"long_form":false,"multi_step":false,"sensitive":false}},"est_in_tokens":120,"est_out_tokens":300,"handoff_note":"{note}"}}"#
            )
            .into(),
        }]
    }

    fn main_stream(text: &str, tokens_out: u64) -> Vec<StreamEvent> {
        vec![
            StreamEvent::TextDelta { text: text.into() },
            StreamEvent::Usage { tokens_in: 17, tokens_out, latency_ms: 123 },
            StreamEvent::Done,
        ]
    }

    fn suggestions_script() -> Vec<StreamEvent> {
        vec![
            StreamEvent::TextDelta {
                text: r#"["Try a follow-up","Show an example","Explain more"]"#.into(),
            },
            StreamEvent::Done,
        ]
    }

    /// System-prompt text of a captured provider request.
    fn system_text_of(req: &ChatRequest) -> String {
        req.messages
            .iter()
            .filter(|m| m.role == ChatRole::System)
            .filter_map(|m| match &m.content {
                ChatContent::Text(t) => Some(t.clone()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    async fn poll_suggestions(state: &AppState) -> Vec<String> {
        for _ in 0..200 {
            let got = state
                .suggestions
                .read()
                .unwrap_or_else(|p| p.into_inner())
                .get("c1")
                .cloned();
            if let Some(items) = got {
                if !items.is_empty() {
                    return items;
                }
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("suggestions did not land within 2s");
    }

    async fn poll_title(state: &AppState) -> String {
        for _ in 0..200 {
            if let Some(conv) = state.db.get_conversation("c1".into()).await.unwrap() {
                if conv.title.as_deref() == Some("Auth Migration Plan") {
                    return conv.title.unwrap();
                }
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("sidecar title did not land within 2s");
    }

    // -- M0 regression suite (ported onto the shared fake provider) ---------

    #[tokio::test]
    async fn complete_stream_persists_usage_and_title() {
        let (_dir, state) = app_with(FakeProvider::scripted(main_stream("hello", 4)));
        create_conv(&state).await;

        let result = chat_send_inner(&state, args("What is 15% of 82?")).await.unwrap();
        assert_eq!(result.status, MessageStatus::Complete);
        assert_eq!(result.tokens_in, 17);
        assert_eq!(result.tokens_out, 4);
        assert_eq!(result.latency_ms, 123);
        assert!(!result.escalation_available, "no triad roles configured");

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
        let (_dir, state) = app_with(FakeProvider::cancelling_after(vec![
            StreamEvent::TextDelta { text: "par".into() },
            StreamEvent::TextDelta { text: "tial".into() },
        ]));
        create_conv(&state).await;

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
        ]));
        create_conv(&state).await;

        let result = chat_send_inner(&state, args("hi")).await.unwrap();
        assert_eq!(result.status, MessageStatus::Error);
        let msgs = messages(&state).await;
        assert_eq!(msgs[1].status, MessageStatus::Error);
        assert!(msgs[1].error.as_deref().unwrap().contains("model_not_found"));
    }

    #[tokio::test]
    async fn double_send_is_rejected() {
        let (_dir, state) = app_with(FakeProvider::scripted(vec![]));
        create_conv(&state).await;

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
        ]));
        create_conv(&state).await;

        chat_send_inner(&state, args("reason about this")).await.unwrap();
        let msgs = messages(&state).await;
        assert_eq!(msgs[1].reasoning.as_deref(), Some("let me think"));
    }

    // -- M1: routing ---------------------------------------------------------

    #[tokio::test]
    async fn auto_pin_routes_to_assigned_scout_role() {
        let (_dir, state) = app_with_settings(
            FakeProvider::scripted(main_stream("about 12.3", 20)),
            &[(keys::TRIAD_ROLE_SCOUT, "scout-model")],
        );
        create_conv(&state).await;

        let result = chat_send_inner(&state, args("What is 15% of 82?")).await.unwrap();
        assert_eq!(result.status, MessageStatus::Complete);

        let msgs = messages(&state).await;
        assert_eq!(msgs[1].model_id.as_deref(), Some("scout-model"));
        assert_eq!(msgs[1].model_role, Some(ModelRole::Scout));

        let events = state.db.list_routing_events("c1".into(), 10).await.unwrap();
        assert_eq!(events.len(), 1);
        let ev = &events[0];
        assert_eq!(ev.message_id, msgs[1].id);
        assert_eq!(ev.decision.source, DecisionSource::Heuristic);
        assert_eq!(ev.final_target, Target::Scout);
        assert_eq!(ev.actual_model, "scout-model");
        assert!(ev.override_kind.is_none());
        assert!((ev.decision.confidence - 0.5).abs() < 1e-3);
    }

    #[tokio::test]
    async fn herald_decision_is_honored() {
        let provider = FakeProvider::scripted_sequence(vec![
            classify_json("titan", 0.91, "refactor auth flow to refresh tokens"),
            main_stream("here is the refactor", 900),
        ]);
        let received = provider.received.clone();
        let (_dir, state) = app_with_settings(provider, &triad_roles());
        create_conv(&state).await;

        let result = chat_send_inner(
            &state,
            args(
                "Refactor auth to use refresh tokens across these 5 files: \
                 session.rs, login.rs, token.rs, guard.rs, api.rs",
            ),
        )
        .await
        .unwrap();

        let msgs = messages(&state).await;
        assert_eq!(msgs[1].model_id.as_deref(), Some("titan-model"));
        assert_eq!(msgs[1].model_role, Some(ModelRole::Titan));
        assert_eq!(result.tokens_out, 900, "usage comes from the main stream");

        let reqs = received.lock().unwrap_or_else(|p| p.into_inner()).clone();
        assert!(reqs.len() >= 2, "classify + main (+ best-effort sidecars)");

        let classify = &reqs[0];
        assert_eq!(classify.model, "herald-model");
        assert_eq!(classify.params.temperature, Some(0.0));
        assert!(classify.params.json_schema.is_some());
        assert_eq!(classify.params.keep_alive.as_deref(), Some("24h"));

        let main = &reqs[1];
        assert_eq!(main.model, "titan-model");
        assert_eq!(main.params.keep_alive.as_deref(), Some("3m"));
        assert!(main.params.json_schema.is_none());

        let events = state.db.list_routing_events("c1".into(), 10).await.unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].decision.source, DecisionSource::Herald);
        assert_eq!(events[0].final_target, Target::Titan);
        assert_eq!(events[0].actual_model, "titan-model");
        assert!((events[0].decision.confidence - 0.91).abs() < 1e-3);
        assert_eq!(
            events[0].decision.handoff_note,
            "refactor auth flow to refresh tokens"
        );
        assert!(events[0].override_kind.is_none());
    }

    #[tokio::test]
    async fn manual_pin_routes_directly() {
        let provider = FakeProvider::scripted(main_stream("done", 5));
        let received = provider.received.clone();
        let (_dir, state) = app_with_settings(
            provider,
            &[
                (keys::TRIAD_ROLE_SCOUT, "scout-model"),
                (keys::TRIAD_ROLE_TITAN, "titan-model"),
            ],
        );
        create_conv(&state).await;
        state
            .db
            .set_conversation_model("c1".into(), Some("titan".into()))
            .await
            .unwrap();

        chat_send_inner(&state, args("hello")).await.unwrap();

        let reqs = received.lock().unwrap_or_else(|p| p.into_inner()).clone();
        assert_eq!(reqs.len(), 1, "pinned chats skip Herald entirely");
        assert_eq!(reqs[0].model, "titan-model");
        assert_eq!(reqs[0].params.keep_alive.as_deref(), Some("3m"));

        let msgs = messages(&state).await;
        assert_eq!(msgs[1].model_role, Some(ModelRole::Titan));

        let events = state.db.list_routing_events("c1".into(), 10).await.unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].decision.source, DecisionSource::Manual);
        assert_eq!(events[0].override_kind.as_deref(), Some("manual"));
        assert_eq!(events[0].actual_model, "titan-model");
    }

    #[tokio::test]
    async fn escalation_available_past_scout_ceiling() {
        // Over the ceiling → the UI may offer "⚡ Continue with Titan".
        {
            let (_dir, state) = app_with_settings(
                FakeProvider::scripted(main_stream("long answer", 100)),
                &[
                    (keys::TRIAD_ROLE_SCOUT, "scout-model"),
                    (keys::TRIAD_SCOUT_OUTPUT_CEILING, "5"),
                ],
            );
            create_conv(&state).await;
            state
                .db
                .set_conversation_model("c1".into(), Some("scout".into()))
                .await
                .unwrap();

            let result = chat_send_inner(&state, args("write something")).await.unwrap();
            assert!(result.escalation_available, "100 tokens > ceiling 5");
        }

        // Under the ceiling → no escalation offered.
        {
            let (_dir, state) = app_with_settings(
                FakeProvider::scripted(main_stream("short", 3)),
                &[
                    (keys::TRIAD_ROLE_SCOUT, "scout-model"),
                    (keys::TRIAD_SCOUT_OUTPUT_CEILING, "500"),
                ],
            );
            create_conv(&state).await;
            state
                .db
                .set_conversation_model("c1".into(), Some("scout".into()))
                .await
                .unwrap();

            let result = chat_send_inner(&state, args("hi")).await.unwrap();
            assert!(!result.escalation_available);
        }
    }

    // -- M1: handoff ---------------------------------------------------------

    #[tokio::test]
    async fn fresh_conversation_is_not_a_handoff() {
        let provider = FakeProvider::scripted_sequence(vec![
            classify_json("scout", 0.9, "begin the task"),
            main_stream("hello there", 5),
        ]);
        let received = provider.received.clone();
        let (_dir, state) = app_with_settings(provider, &triad_roles());
        create_conv(&state).await;

        chat_send_inner(&state, args("hello")).await.unwrap();

        let reqs = received.lock().unwrap_or_else(|p| p.into_inner()).clone();
        assert!(reqs.len() >= 2);

        // The just-added user message must not be duplicated into the tail
        // (the 8 few-shot pairs make the other user messages).
        let hello_count = reqs[0]
            .messages
            .iter()
            .filter(|m| m.role == ChatRole::User)
            .filter(|m| match &m.content {
                ChatContent::Text(t) => t.contains("hello"),
                _ => false,
            })
            .count();
        assert_eq!(hello_count, 1, "latest message appears exactly once");

        let handoff = system_text_of(&reqs[1]);
        assert!(
            !handoff.contains("taking over"),
            "first turn of a fresh conversation is not a model switch: {handoff}"
        );
        assert!(!handoff.contains("ROLLING DIGEST"));
    }

    #[tokio::test]
    async fn handoff_carries_digest_and_note_on_switch() {
        let provider = FakeProvider::scripted_sequence(vec![
            classify_json("scout", 0.85, "continue the auth refactor"),
            main_stream("continuing", 8),
        ]);
        let received = provider.received.clone();
        let (_dir, state) = app_with_settings(provider, &triad_roles());
        create_conv(&state).await;

        // A prior turn that ran on some other model, plus a stored digest.
        state
            .db
            .insert_assistant_placeholder(
                "a1".into(),
                "c1".into(),
                "model-a".into(),
                "ep_local_ollama".into(),
                ids::now_ms(),
            )
            .await
            .unwrap();
        state
            .db
            .update_assistant_message(
                "a1".into(),
                vec![ContentPart::Text { text: "prior answer".into() }],
                None,
                MessageStatus::Complete,
                None,
                None,
                None,
                None,
            )
            .await
            .unwrap();
        state
            .db
            .set_conversation_digest("c1".into(), "prior work: migrated session store".into())
            .await
            .unwrap();

        chat_send_inner(&state, args("keep going")).await.unwrap();

        let reqs = received.lock().unwrap_or_else(|p| p.into_inner()).clone();
        assert!(reqs.len() >= 2);
        let handoff = system_text_of(&reqs[1]);
        assert!(
            handoff.contains("ROLLING DIGEST (task state from earlier turns): prior work"),
            "digest system message present: {handoff}"
        );
        assert!(handoff.contains("taking over from"), "prior-model note: {handoff}");
        assert!(handoff.contains("continue the auth refactor"), "note: {handoff}");

        let msgs = messages(&state).await;
        let answer = msgs.last().unwrap();
        assert_eq!(answer.model_id.as_deref(), Some("scout-model"));
    }

    // -- M1: sidecars --------------------------------------------------------

    #[tokio::test]
    async fn suggestions_sidecar_populates_cache() {
        let (_dir, state) = app_with_settings(
            FakeProvider::scripted_sequence(vec![main_stream("ok", 3), suggestions_script()]),
            &[
                (keys::TRIAD_ROLE_HERALD, "herald-model"),
                (keys::TRIAD_ROLE_SCOUT, "scout-model"),
            ],
        );
        create_conv(&state).await;
        state
            .db
            .set_conversation_model("c1".into(), Some("scout".into()))
            .await
            .unwrap();

        chat_send_inner(&state, args("hello")).await.unwrap();

        let chips = poll_suggestions(&state).await;
        assert_eq!(
            chips,
            vec!["Try a follow-up", "Show an example", "Explain more"]
        );

        // Taking the chips clears the cache (one-shot per turn).
        assert_eq!(state.take_suggestions("c1").len(), 3);
        assert!(state.take_suggestions("c1").is_empty());
    }

    #[tokio::test]
    async fn sidecar_title_refines_after_second_exchange() {
        let (_dir, state) = app_with_settings(
            FakeProvider::scripted_sequence(vec![
                main_stream("first answer", 3),
                main_stream("second answer", 3),
                vec![
                    StreamEvent::TextDelta { text: "Auth Migration Plan".into() },
                    StreamEvent::Done,
                ],
            ]),
            &[
                (keys::TRIAD_ROLE_HERALD, "herald-model"),
                (keys::TRIAD_ROLE_SCOUT, "scout-model"),
                (keys::TRIAD_SIDECAR_SUGGESTIONS, "false"),
            ],
        );
        create_conv(&state).await;
        state
            .db
            .set_conversation_model("c1".into(), Some("scout".into()))
            .await
            .unwrap();

        chat_send_inner(&state, args("What is 15% of 82?")).await.unwrap();
        let conv = state.db.get_conversation("c1".into()).await.unwrap().unwrap();
        assert_eq!(
            conv.title.as_deref(),
            Some("What is 15% of 82?"),
            "fallback title after the first exchange"
        );

        chat_send_inner(&state, args_with_id("um2", "and 30% of 90?"))
            .await
            .unwrap();
        let title = poll_title(&state).await;
        assert_eq!(title, "Auth Migration Plan");
    }
}