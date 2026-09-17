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
        ApprovalReply, ChatContent, ChatMessage, ChatParams, ChatRequest, ChatRole,
        ChatSendResult, ContentPart, DecisionSource, MessageStatus, Message as DbMessage,
        ModelRole, RoutingDecision, StatusPhase, StreamEvent, Target,
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
            tool_name: None,
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
                    tool_name: None,
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
                tool_name: None,
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
            tool_name: None,
        });
    }

    // 7. Params from settings; keep-alive follows the role.
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
    let max_hops: u32 = state
        .settings
        .get_or(crate::settings::keys::TOOLS_MAX_HOPS, "12")
        .parse()
        .unwrap_or(12);

    // 8. Tool loop (§6.1): the model emits tool_call → validate → execute →
    //    `<tool_result>` message appended → it continues, up to `max_hops`,
    //    then a forced no-tools summary. With no workspace bound the model
    //    has zero FS access (§6.3) — no `tools` array at all, M0 behavior.
    //    MCP servers (M3) extend this predicate beyond workspace-bound tools.
    let tool_specs = if conv.workspace_roots.is_empty() {
        Vec::new()
    } else {
        state.tool_registry.specs()
    };
    let mut all_text = String::new();
    let mut all_reasoning = String::new();
    let mut usage_total = (0u64, 0u64, 0u64);
    let mut final_status = MessageStatus::Complete;
    let mut final_error: Option<String> = None;
    let mut hops = 0u32;

    loop {
        let tools = (!tool_specs.is_empty() && hops < max_hops).then(|| tool_specs.clone());
        let req = ChatRequest {
            endpoint_id: endpoint_id.clone(),
            model: routed.model.clone(),
            messages: std::mem::take(&mut messages),
            tools,
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

        // Spawn the provider stream for this hop.
        let (tx, mut rx) = mpsc::channel::<StreamEvent>(64);
        let provider: Arc<dyn Provider> = state.provider_for(DEFAULT_ENDPOINT)?;
        let cancel_for_task = cancel.clone();
        tokio::spawn(async move {
            provider.chat(req, tx, cancel_for_task).await;
        });

        // Consume: forward every event, accumulate, flush periodically.
        let hop = pump_stream(state, message_id, cancel, &forward, &mut rx, &all_text).await;

        all_text.push_str(&hop.text);
        all_reasoning.push_str(&hop.reasoning);
        usage_total.0 = usage_total.0.saturating_add(hop.usage.unwrap_or_default().0);
        usage_total.1 = usage_total.1.saturating_add(hop.usage.unwrap_or_default().1);
        usage_total.2 = usage_total.2.saturating_add(hop.usage.unwrap_or_default().2);

        if hop.status != MessageStatus::Complete {
            final_status = hop.status;
            final_error = hop.error;
            break;
        }
        if hop.calls.is_empty() {
            break; // plain text answer — the turn is done
        }

        hops += 1;
        if hops >= max_hops {
            // Budget exhausted: the next (final) call gets no `tools`, and the
            // model is told to wrap up with what it has (forced summary).
            messages.push(ChatMessage {
                id: ids::new_id(),
                role: ChatRole::System,
                content: ChatContent::Text(
                    "Tool budget exhausted for this turn. Summarize what you have \
                     learned and answer now without further tool calls."
                        .into(),
                ),
                tool_calls: None,
                tool_call_id: None,
                tool_name: None,
            });
        }

        // The assistant's tool-call hop joins the context so the provider
        // sees the exchange it started.
        messages.push(ChatMessage {
            id: message_id.to_string(),
            role: ChatRole::Assistant,
            content: ChatContent::Text(hop.text.clone()),
            tool_calls: Some(hop.calls.clone()),
            tool_call_id: None,
            tool_name: None,
        });

        // Validate → execute → wrap → append, one result message per call.
        for call in &hop.calls {
            let (result_text, is_error) = execute_tool_call(
                state,
                message_id,
                call,
                &conv.workspace_roots,
                &forward,
                cancel,
            )
            .await?;
            let envelope = format!(
                r#"<tool_result id="{}" source="untrusted">{result_text}</tool_result>"#,
                call.id
            );
            forward(&StreamEvent::ToolResult {
                call_id: call.id.clone(),
                content: result_text.clone(),
                is_error: Some(is_error),
            });
            messages.push(ChatMessage {
                id: ids::new_id(),
                role: ChatRole::Tool,
                content: ChatContent::Text(envelope),
                tool_calls: None,
                tool_call_id: Some(call.id.clone()),
                tool_name: Some(call.name.clone()),
            });
        }
    }

    // 9. Final persist (covers any un-flushed partial).
    let reasoning_opt = if all_reasoning.is_empty() {
        None
    } else {
        Some(all_reasoning.clone())
    };
    state
        .db
        .update_assistant_message(
            message_id.to_string(),
            parts_for(&all_text),
            reasoning_opt,
            final_status,
            Some(usage_total.0),
            Some(usage_total.1),
            Some(usage_total.2),
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

    // 10. Sidecars (§3.7) — out-of-band, best-effort, never block the result.
    spawn_sidecars(state, args, &history, &all_text, switching, final_status).await;

    // Scout ran past its output ceiling (§3.6 trigger 4) — the UI offers
    // "⚡ Continue with Titan". Summed across tool hops.
    let escalation_available = routed.role == Some(ModelRole::Scout)
        && final_status == MessageStatus::Complete
        && usage_total.1 > u64::from(cfg.scout_output_ceiling);

    Ok(ChatSendResult {
        message_id: message_id.to_string(),
        status: final_status,
        tokens_in: usage_total.0,
        tokens_out: usage_total.1,
        latency_ms: usage_total.2,
        escalation_available,
    })
}

/// One model stream's output: the accumulated text/reasoning, usage, terminal
/// status, and any tool calls the model emitted (wire call ids preserved —
/// persistence re-keys them, §6.1).
struct StreamHop {
    text: String,
    reasoning: String,
    usage: Option<(u64, u64, u64)>,
    status: MessageStatus,
    error: Option<String>,
    calls: Vec<crate::types::ToolCall>,
}

/// Consume one provider stream: forward every event, accumulate, flush
/// periodically. Tool-call events are collected into `calls` (Start gives
/// id+name; Delta appends to the arguments JSON) — execution happens after
/// the stream ends, in the orchestrator loop.
async fn pump_stream(
    state: &AppState,
    message_id: &str,
    cancel: &tokio_util::sync::CancellationToken,
    forward: &Arc<dyn Fn(&StreamEvent) + Send + Sync>,
    rx: &mut mpsc::Receiver<StreamEvent>,
    base_text: &str,
) -> StreamHop {
    let mut hop = StreamHop {
        text: String::new(),
        reasoning: String::new(),
        usage: None,
        status: MessageStatus::Complete,
        error: None,
        calls: Vec::new(),
    };
    let mut last_flush = Instant::now();

    loop {
        tokio::select! {
            biased;

            _ = cancel.cancelled() => {
                hop.status = MessageStatus::Stopped;
                return hop;
            }

            ev = rx.recv() => {
                let Some(ev) = ev else {
                    return hop; // provider ended without done
                };

                forward(&ev);

                match ev {
                    StreamEvent::TextDelta { text: t } => hop.text.push_str(&t),
                    StreamEvent::ReasoningDelta { text: t } => hop.reasoning.push_str(&t),
                    StreamEvent::Usage { tokens_in, tokens_out, latency_ms } => {
                        hop.usage = Some((tokens_in, tokens_out, latency_ms));
                    }
                    StreamEvent::ToolCallStart { index, id, name } => {
                        let i = index as usize;
                        while hop.calls.len() <= i {
                            hop.calls.push(crate::types::ToolCall {
                                id: String::new(),
                                name: String::new(),
                                args: String::new(),
                            });
                        }
                        hop.calls[i] = crate::types::ToolCall {
                            id,
                            name,
                            args: String::new(),
                        };
                    }
                    StreamEvent::ToolCallDelta { index, args_delta } => {
                        if let Some(call) = hop.calls.get_mut(index as usize) {
                            call.args.push_str(&args_delta);
                        }
                    }
                    StreamEvent::Error { code, message, .. } => {
                        hop.status = MessageStatus::Error;
                        hop.error = Some(format!("{code}: {message}"));
                        return hop;
                    }
                    StreamEvent::Done => {
                        hop.status = MessageStatus::Complete;
                        return hop;
                    }
                    // status / routing / tool_result: forwarded above.
                    _ => {}
                }

                if hop.text.len() >= FLUSH_CHARS || last_flush.elapsed() >= FLUSH_INTERVAL {
                    let combined = format!("{base_text}{}", hop.text);
                    // Best-effort mid-stream flush; the final persist covers
                    // any gap (§6.1: never abort the turn on a flush error).
                    let _ = persist_partial(state, message_id, &hop.reasoning, &combined).await;
                    last_flush = Instant::now();
                }
            }
        }
    }
}

/// Validate → gate → execute one tool call, persisting the outcome row.
/// Validation failures and unknown tools produce an `error` row fed back to
/// the model; the loop always continues so it can correct course (§6.1).
///
/// §6.6 gate for mutating tools, in order: workspace path (the guard error
/// surfaces from the tool itself), session grant → persisted row → default
/// ask. An "ask" pauses the stream on an ApprovalRequest until the user
/// replies (or the stream is cancelled — which denies, never aborts).
async fn execute_tool_call(
    state: &AppState,
    message_id: &str,
    call: &crate::types::ToolCall,
    workspaces: &[String],
    forward: &Arc<dyn Fn(&StreamEvent) + Send + Sync>,
    cancel: &tokio_util::sync::CancellationToken,
) -> Result<(String, bool), CmdError> {
    let row_id = ids::new_id();
    let now = ids::now_ms();

    let Some(tool) = state.tool_registry.get(&call.name) else {
        let msg = format!("unknown tool `{}`", call.name);
        state
            .db
            .insert_tool_call(
                row_id.clone(),
                message_id.to_string(),
                call.name.clone(),
                call.args.clone(),
                now,
            )
            .await?;
        state
            .db
            .finish_tool_call(row_id, msg.clone(), "error".to_string(), Some("auto".into()))
            .await?;
        return Ok((msg, true));
    };

    let mut args: serde_json::Value = serde_json::from_str(&call.args)
        .unwrap_or(serde_json::Value::Null);
    if let Err(e) = crate::tools::validate_args(&tool.spec().input_schema, &args) {
        let msg = e.message();
        state
            .db
            .insert_tool_call(
                row_id.clone(),
                message_id.to_string(),
                call.name.clone(),
                call.args.clone(),
                now,
            )
            .await?;
        state
            .db
            .finish_tool_call(row_id, msg.clone(), "error".to_string(), Some("auto".into()))
            .await?;
        return Ok((msg, true));
    }

    state
        .db
        .insert_tool_call(
            row_id.clone(),
            message_id.to_string(),
            call.name.clone(),
            call.args.clone(),
            now,
        )
        .await?;

    // ---- §6.6 permission matrix ----------------------------------------
    let mut permission_mode = "auto".to_string();
    if crate::permissions::is_mutating(&call.name) {
        // One guard resolution up front, shared by the payload and the
        // edit-in-place rewrite (None when the target doesn't exist yet).
        let path_key = crate::permissions::main_path_key(&call.name);
        let raw = args
            .get(path_key)
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        let roots: Vec<std::path::PathBuf> =
            workspaces.iter().map(std::path::PathBuf::from).collect();
        let resolved = crate::tools::guard::resolve(&raw, &roots).ok().map(|r| r.path);

        match resolve_policy(state, &call.name, &args, workspaces).await.as_str() {
            "always" => permission_mode = "always".to_string(),
            "session" => permission_mode = "session".to_string(),
            _ => {
                match request_approval(state, call, &args, workspaces, resolved.as_deref(), forward, cancel).await? {
                    ApprovalDecision::Denied => {
                        let msg = "user denied this operation".to_string();
                        state
                            .db
                            .finish_tool_call(
                                row_id.clone(),
                                msg.clone(),
                                "error".to_string(),
                                Some("denied".into()),
                            )
                            .await?;
                        return Ok((msg, true));
                    }
                    ApprovalDecision::Allowed { mode, edited } => {
                        permission_mode = mode;
                        if let Some(content) = edited {
                            apply_edited_content(&call.name, &mut args, content, resolved.as_deref());
                        }
                    }
                }
            }
        }
    }

    let ctx = crate::tools::ToolExecCtx {
        workspaces: workspaces.to_vec(),
    };
    let executed = tool.execute(args, &ctx).await;
    let (result_text, is_error) = match executed {
        Ok(outcome) => {
            let (text, err) = outcome.into_parts();
            (
                crate::tools::clamp_result(text),
                err,
            )
        }
        Err(e) => (e.message(), true),
    };
    let status = if is_error { "error" } else { "ok" };
    state
        .db
        .finish_tool_call(row_id, result_text.clone(), status.to_string(), Some(permission_mode))
        .await?;
    Ok((result_text, is_error))
}

/// Session grant → persisted "always" row → default ask.
/// Returns "session" | "always" | "ask".
async fn resolve_policy(
    state: &AppState,
    tool: &str,
    args: &serde_json::Value,
    workspaces: &[String],
) -> String {
    // The matrix keys on the root that contains the affected path. If the
    // raw arg names no bound workspace the tool itself will fail the guard;
    // the gate stays on the safe side (ask).
    let path_key = crate::permissions::main_path_key(tool);
    let raw = args
        .get(path_key)
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    let Some(root) = matrix_root(workspaces, raw) else {
        return "ask".to_string();
    };

    if state.session_grants.lock().await.contains(tool, &root) {
        return "session".to_string();
    }
    if state
        .db
        .tool_permission(tool.to_string(), root.clone())
        .await
        .ok()
        .flatten()
        .is_some_and(|mode| mode == "always")
    {
        return "always".to_string();
    }
    "ask".to_string()
}

/// The workspace-root string that lexically contains `raw` — the matrix's
/// key. Relative paths resolve to the first bound root (matches fs tools);
/// absolute paths to the bound root that contains them; unresolvable → None.
fn matrix_root<'a>(workspaces: &'a [String], raw: &str) -> Option<String> {
    if raw.is_empty() {
        return workspaces.first().cloned();
    }
    if std::path::Path::new(raw).is_absolute() {
        let raw_lower = raw.to_lowercase();
        workspaces
            .iter()
            .find(|w| {
                let prefix = format!("{}\\", w.to_lowercase().trim_end_matches('\\'));
                raw_lower.starts_with(&prefix)
                    || raw_lower
                        == w.to_lowercase().trim_end_matches('\\').to_string()
            })
            .cloned()
    } else {
        workspaces.first().cloned()
    }
}

/// Outcome of parking an ApprovalRequest.
enum ApprovalDecision {
    Allowed { mode: String, edited: Option<String> },
    Denied,
}

async fn request_approval(
    state: &AppState,
    call: &crate::types::ToolCall,
    args: &serde_json::Value,
    workspaces: &[String],
    resolved: Option<&std::path::Path>,
    forward: &Arc<dyn Fn(&StreamEvent) + Send + Sync>,
    cancel: &tokio_util::sync::CancellationToken,
) -> Result<ApprovalDecision, CmdError> {
    let id = ids::new_id();
    let tool = call.name.clone();
    let path_key = crate::permissions::main_path_key(&tool);
    let raw = args
        .get(path_key)
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let raw_to = args
        .get("to")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    // Display path: the canonical workspace path when it resolves, else the
    // raw argument (not-yet-existing creation targets).
    let display = resolved
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| raw.clone());

    let (summary, diff, result_content) = build_approval_payload(&tool, args, &display);
    forward(&StreamEvent::ApprovalRequest {
        id: id.clone(),
        call_id: call.id.clone(),
        tool: tool.clone(),
        path: display,
        secondary_path: raw_to,
        summary: summary.clone(),
        diff,
        result_content: result_content.clone(),
    });

    let (tx, rx) = tokio::sync::oneshot::channel::<ApprovalReply>();
    state.approvals.lock().await.insert(id.clone(), tx);

    // §6.6: the stream pauses (never cancelled) while pending. A stop on the
    // conversation denies the pending request — the model sees a denial and
    // the turn unwinds as cancelled, not aborted.
    let reply = tokio::select! {
        _ = cancel.cancelled() => {
            state.approvals.lock().await.remove(&id);
            ApprovalReply { allow: false, mode: String::new(), edited_content: None }
        }
        r = rx => {
            r.unwrap_or(ApprovalReply { allow: false, mode: String::new(), edited_content: None })
        }
    };

    if !reply.allow {
        return Ok(ApprovalDecision::Denied);
    }
    let mode = match reply.mode.as_str() {
        "session" => {
            if let Some(root) = matrix_root(workspaces, &raw) {
                state.session_grants.lock().await.grant(&tool, &root);
            }
            "session".to_string()
        }
        "always" => {
            if let Some(root) = matrix_root(workspaces, &raw) {
                state
                    .db
                    .set_tool_permission(tool.clone(), root, "always".to_string())
                    .await?;
            }
            "always".to_string()
        }
        _ => "ask".to_string(),
    };
    Ok(ApprovalDecision::Allowed {
        mode,
        edited: reply.edited_content.filter(|s| !s.is_empty()),
    })
}

/// Build the human summary + diff/content payload for a mutating tool.
/// fs_write/fs_edit carry the resulting content (≤8 KB) so the modal can
/// show and edit it; everything else summarizes.
fn build_approval_payload(
    tool: &str,
    args: &serde_json::Value,
    resolved_path: &str,
) -> (String, Option<String>, Option<String>) {
    const MAX_DIFF_CHARS: usize = 8_192;
    let path = std::path::Path::new(resolved_path);
    match tool {
        "fs_write" => {
            let content = args
                .get("content")
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            let append = args
                .get("mode")
                .and_then(|v| v.as_str())
                .is_some_and(|m| m == "append");
            if append {
                return (
                    format!("appends {} bytes to {resolved_path}", content.len()),
                    None,
                    None,
                );
            }
            let existing = std::fs::read_to_string(path).ok();
            let summary = match &existing {
                Some(old) => {
                    format!("overwrites an existing file ({} bytes)", old.len())
                }
                None => format!("creates a new file ({} bytes)", content.len()),
            };
            let diff = existing.map(|old| {
                let full = unified_diff(&old, content);
                truncate_diff(&full, MAX_DIFF_CHARS)
            });
            (summary, diff, Some(content.to_string()))
        }
        "fs_edit" => {
            let old_string = args
                .get("old_string")
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            let new_string = args
                .get("new_string")
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            let existing = std::fs::read_to_string(path).ok();
            let summary = "replaces a snippet in an existing file".to_string();
            let diff = existing.as_ref().map(|old| {
                let after = old.replace(old_string, new_string);
                let full = unified_diff(old, &after);
                truncate_diff(&full, MAX_DIFF_CHARS)
            });
            let result_content = existing.map(|old| old.replace(old_string, new_string));
            (summary, diff, result_content)
        }
        "fs_move" | "fs_copy" => {
            let to = args.get("to").and_then(|v| v.as_str()).unwrap_or("?");
            let verb = if tool == "fs_move" { "moves" } else { "copies" };
            (format!("{verb} {resolved_path} → {to}"), None, None)
        }
        "fs_delete" => (
            format!("moves {resolved_path} to the Recycle Bin"),
            None,
            None,
        ),
        "fs_mkdir" => (format!("creates directory {resolved_path}"), None, None),
        _ => (format!("runs {tool}"), None, None),
    }
}

fn unified_diff(before: &str, after: &str) -> String {
    let diff = similar::TextDiff::from_lines(before, after);
    let mut out = String::new();
    for hunk in diff.unified_diff().context_radius(2).iter_hunks() {
        out.push_str(&hunk.to_string());
        out.push('\n');
    }
    out
}

fn truncate_diff(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        return s.to_string();
    }
    let cut: String = s.chars().take(max_chars).collect();
    format!("{cut}\n[… diff truncated]")
}

/// Edit-in-place (§6.6): the modal's replacement content becomes the tool's
/// own argument, so the executor stays the single code path. For fs_edit the
/// replacement is the whole resulting file — expressed as old_string = the
/// entire existing content, which matches exactly once.
fn apply_edited_content(
    tool: &str,
    args: &mut serde_json::Value,
    content: String,
    resolved: Option<&std::path::Path>,
) {
    match tool {
        "fs_write" => {
            if let Some(obj) = args.as_object_mut() {
                obj.insert("content".into(), serde_json::Value::String(content));
            }
        }
        "fs_edit" => {
            let Some(resolved) = resolved else {
                return; // target didn't resolve — run the original edit
            };
            let Ok(existing) = std::fs::read_to_string(resolved) else {
                return; // binary/unreadable — run the original edit
            };
            if existing.is_empty() {
                return; // whole-file replacement impossible; the edit errors below
            }
            if let Some(obj) = args.as_object_mut() {
                obj.insert("old_string".into(), serde_json::Value::String(existing));
                obj.insert("new_string".into(), serde_json::Value::String(content));
                obj.remove("expected_occurrences");
            }
        }
        _ => {}
    }
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
                // H3 (§3.5): ≥ 2 tool results already in context ⇒ Titan.
                ctx.prior_tool_results = state
                    .db
                    .count_prior_tool_calls(args.conversation_id.clone(), ids::now_ms())
                    .await
                    .unwrap_or(0)
                    .max(0) as usize;
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
        tool_name: None,
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
    use std::sync::Mutex;

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

    // -- M2.2a: tool runtime loop --------------------------------------------

    /// One hop that emits a tool call (no herald configured in these tests, so
    /// `received[i]` indexes stay aligned with orchestrator hops).
    fn tool_call_script(name: &str, args: &str, text: &str) -> Vec<StreamEvent> {
        vec![
            StreamEvent::TextDelta { text: text.into() },
            StreamEvent::ToolCallStart { index: 0, id: "call_0".into(), name: name.into() },
            StreamEvent::ToolCallDelta { index: 0, args_delta: args.into() },
            StreamEvent::Usage { tokens_in: 9, tokens_out: 6, latency_ms: 11 },
            StreamEvent::Done,
        ]
    }

    fn with_echo_tool(state: &mut AppState) {
        state
            .tool_registry
            .register(Arc::new(crate::tools::testkit::EchoTool {
                result: String::new(),
            }));
    }

    /// Bind a workspace root on c1 — tool specs are only declared for
    /// conversations with a bound workspace (§6.3, M2.4).
    async fn bind_root(state: &AppState, conv: &str) {
        let root = tempfile::tempdir().unwrap(); // string storage only
        state
            .db
            .set_conversation_workspaces(
                conv.to_string(),
                vec![root.path().display().to_string()],
            )
            .await
            .unwrap();
    }

    fn last_tool_message(req: &ChatRequest) -> &ChatMessage {
        req.messages
            .iter()
            .rev()
            .find(|m| m.role == ChatRole::Tool)
            .expect("request carries a tool result message")
    }

    #[tokio::test]
    async fn tool_loop_executes_call_and_feeds_result() {
        let provider = FakeProvider::scripted_sequence(vec![
            tool_call_script("echo", r#"{"text":"ping"}"#, "checking"),
            main_stream("answered", 5),
        ]);
        let received = provider.received.clone();
        let (_dir, mut state) = app_with(provider);
        with_echo_tool(&mut state);
        create_conv(&state).await;
        bind_root(&state, "c1").await;

        let collected = Arc::new(Mutex::new(Vec::new()));
        let sink = collected.clone();
        let forward = Arc::new(move |ev: &StreamEvent| {
            sink.lock().unwrap_or_else(|p| p.into_inner()).push(ev.clone())
        }) as Arc<dyn Fn(&StreamEvent) + Send + Sync>;
        let result = send(&state, args("look up something"), forward).await.unwrap();

        // Usage sums across both hops (9+17 in, 6+5 out, 11+123 ms).
        assert_eq!(result.status, MessageStatus::Complete);
        assert_eq!(result.tokens_in, 26);
        assert_eq!(result.tokens_out, 11);
        assert_eq!(result.latency_ms, 134);

        let reqs = received.lock().unwrap_or_else(|p| p.into_inner()).clone();
        // Hop 0 declares the registered tools (bundled fs reads + echo);
        // hop 1 still has them (budget not exhausted).
        let specs0 = reqs[0].tools.as_ref().expect("tools declared on hop 0");
        assert!(specs0.iter().any(|s| s.name == "echo"), "{:?}", specs0.iter().map(|s| s.name.clone()).collect::<Vec<_>>());
        assert!(reqs[1].tools.is_some());

        // The context for hop 1: assistant echo of the tool call + the
        // untrusted-annotated result envelope.
        let asst = reqs[1]
            .messages
            .iter()
            .rev()
            .find(|m| m.role == ChatRole::Assistant && m.tool_calls.is_some())
            .expect("assistant tool_calls echoed into context");
        let calls = asst.tool_calls.as_ref().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].id, "call_0");
        assert_eq!(calls[0].args, r#"{"text":"ping"}"#);

        let tool_msg = last_tool_message(&reqs[1]);
        assert_eq!(tool_msg.tool_call_id.as_deref(), Some("call_0"));
        assert_eq!(tool_msg.tool_name.as_deref(), Some("echo"));
        match &tool_msg.content {
            ChatContent::Text(t) => assert!(
                t.contains(r#"<tool_result id="call_0" source="untrusted">echo: ping</tool_result>"#),
                "{t}"
            ),
            _ => unreachable!(),
        }

        // Persisted outcome row.
        let msgs = messages(&state).await;
        let rows = state.db.list_tool_calls(msgs[1].id.clone()).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].tool, "echo");
        assert_eq!(rows[0].status, "ok");
        assert_eq!(rows[0].result.as_deref(), Some("echo: ping"));

        // Final message concatenates hop texts and closes on the answer.
        match &msgs[1].content[0] {
            ContentPart::Text { text } => assert_eq!(text, "checkinganswered"),
            _ => unreachable!(),
        }

        // A ToolResult event was forwarded live for the UI.
        let events = collected.lock().unwrap_or_else(|p| p.into_inner()).clone();
        assert!(events.iter().any(|ev| matches!(
            ev,
            StreamEvent::ToolResult {
                call_id,
                content,
                is_error: Some(false)
            } if call_id == "call_0" && content == "echo: ping"
        )));
    }

    #[tokio::test]
    async fn tool_call_validation_failure_does_not_execute() {
        let provider = FakeProvider::scripted_sequence(vec![
            tool_call_script("echo", "{}", ""),
            main_stream("recovered", 4),
        ]);
        let received = provider.received.clone();
        let (_dir, mut state) = app_with(provider);
        with_echo_tool(&mut state);
        create_conv(&state).await;

        let result = chat_send_inner(&state, args("hi")).await.unwrap();
        assert_eq!(result.status, MessageStatus::Complete, "loop continues past bad args");

        let reqs = received.lock().unwrap_or_else(|p| p.into_inner()).clone();
        let tool_msg = last_tool_message(&reqs[1]);
        match &tool_msg.content {
            ChatContent::Text(t) => {
                assert!(t.contains("missing required argument `text`"), "{t}");
            }
            _ => unreachable!(),
        }

        let msgs = messages(&state).await;
        let rows = state.db.list_tool_calls(msgs[1].id.clone()).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].status, "error");
        assert!(
            rows[0].result.as_deref().unwrap_or("").contains("missing required argument"),
            "row records the validation failure: {:?}",
            rows[0].result
        );
    }

    #[tokio::test]
    async fn unknown_tool_becomes_error_result() {
        let provider = FakeProvider::scripted_sequence(vec![
            tool_call_script("nosuch", "{}", ""),
            main_stream("recovered", 4),
        ]);
        let received = provider.received.clone();
        let (_dir, state) = app_with(provider);
        create_conv(&state).await;

        let result = chat_send_inner(&state, args("hi")).await.unwrap();
        assert_eq!(result.status, MessageStatus::Complete);

        let reqs = received.lock().unwrap_or_else(|p| p.into_inner()).clone();
        let tool_msg = last_tool_message(&reqs[1]);
        match &tool_msg.content {
            ChatContent::Text(t) => assert!(t.contains("unknown tool `nosuch`"), "{t}"),
            _ => unreachable!(),
        }

        let msgs = messages(&state).await;
        let rows = state.db.list_tool_calls(msgs[1].id.clone()).await.unwrap();
        assert_eq!(rows[0].status, "error");
        assert_eq!(rows[0].result.as_deref(), Some("unknown tool `nosuch`"));
    }

    #[tokio::test]
    async fn tool_budget_caps_hops() {
        let provider = FakeProvider::scripted_sequence(vec![
            tool_call_script("echo", r#"{"text":"a"}"#, "hop1"),
            tool_call_script("echo", r#"{"text":"b"}"#, "hop2"),
            main_stream("summary", 7),
        ]);
        let received = provider.received.clone();
        let (_dir, mut state) = app_with_settings(
            provider,
            &[(keys::TOOLS_MAX_HOPS, "1")],
        );
        with_echo_tool(&mut state);
        create_conv(&state).await;
        bind_root(&state, "c1").await;

        let result = chat_send_inner(&state, args("go")).await.unwrap();
        assert_eq!(result.status, MessageStatus::Complete);

        let reqs = received.lock().unwrap_or_else(|p| p.into_inner()).clone();
        assert!(reqs[0].tools.is_some());
        // After the budget is consumed: no tools, and the forced-summary
        // system instruction is on the wire.
        assert!(reqs[1].tools.is_none());
        assert!(
            system_text_of(&reqs[1]).contains("Tool budget exhausted for this turn."),
            "forced summary instruction: {}",
            system_text_of(&reqs[1])
        );
        assert!(reqs[2].tools.is_none());

        // Both calls executed and persisted.
        let msgs = messages(&state).await;
        let rows = state.db.list_tool_calls(msgs[1].id.clone()).await.unwrap();
        assert_eq!(rows.len(), 2);
    }

    #[tokio::test]
    async fn no_workspace_bound_sends_no_tool_specs() {
        let provider = FakeProvider::scripted(main_stream("hi", 2));
        let received = provider.received.clone();
        let (_dir, state) = app_with(provider);
        create_conv(&state).await;
        chat_send_inner(&state, args("hi")).await.unwrap();
        let reqs = received.lock().unwrap_or_else(|p| p.into_inner()).clone();
        assert!(
            reqs[0].tools.is_none(),
            "no workspace bound ⇒ model has zero FS access (§6.3)"
        );
    }

    /// Bind a workspace root on c1 and keep the tempdir alive — for tests
    /// that exercise tools against real files.
    async fn bind_root_keep(state: &AppState, conv: &str) -> tempfile::TempDir {
        let root = tempfile::tempdir().unwrap();
        state
            .db
            .set_conversation_workspaces(
                conv.to_string(),
                vec![root.path().display().to_string()],
            )
            .await
            .unwrap();
        root
    }

    /// Resolve the first pending approval from the "UI side", concurrently
    /// with the awaiting send.
    async fn respond_first(state: &AppState, reply: ApprovalReply) {
        for _ in 0..400 {
            let mut approvals = state.approvals.lock().await;
            if let Some(id) = approvals.keys().next().cloned() {
                let tx = approvals.remove(&id).unwrap();
                let _ = tx.send(reply);
                return;
            }
            drop(approvals);
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        panic!("no approval request appeared within 2s");
    }

    #[tokio::test]
    async fn mutating_tool_denial_fed_back_without_execution() {
        let provider = FakeProvider::scripted_sequence(vec![
            tool_call_script("fs_write", r#"{"path":"new.txt","content":"hi"}"#, "writing"),
            main_stream("acknowledged", 4),
        ]);
        let received = provider.received.clone();
        let (_dir, state) = app_with(provider);
        create_conv(&state).await;
        let root = bind_root_keep(&state, "c1").await;

        let collected = Arc::new(Mutex::new(Vec::new()));
        let sink = collected.clone();
        let forward = Arc::new(move |ev: &StreamEvent| {
            sink.lock().unwrap_or_else(|p| p.into_inner()).push(ev.clone())
        }) as Arc<dyn Fn(&StreamEvent) + Send + Sync>;
        let deny = ApprovalReply { allow: false, mode: String::new(), edited_content: None };
        let (result, ()) = tokio::join!(
            send(&state, args("write it"), forward),
            respond_first(&state, deny),
        );
        let result = result.unwrap();
        assert_eq!(result.status, MessageStatus::Complete, "loop continues past a denial");

        // An ApprovalRequest reached the UI with the payload.
        let events = collected.lock().unwrap_or_else(|p| p.into_inner()).clone();
        let req_event = events.iter().find_map(|ev| match ev {
            StreamEvent::ApprovalRequest {
                tool, path, result_content, ..
            } => Some((tool.clone(), path.clone(), result_content.clone())),
            _ => None,
        })
        .expect("approval request forwarded");
        assert_eq!(req_event.0, "fs_write");
        assert!(req_event.1.ends_with("new.txt"), "{}", req_event.1);
        assert_eq!(req_event.2.as_deref(), Some("hi"));

        // The model sees the denial; nothing was written.
        let reqs = received.lock().unwrap_or_else(|p| p.into_inner()).clone();
        let tool_msg = last_tool_message(&reqs[1]);
        match &tool_msg.content {
            ChatContent::Text(t) => assert!(t.contains("user denied this operation"), "{t}"),
            _ => unreachable!(),
        }
        assert!(!root.path().join("new.txt").exists(), "denied write must not land");

        // Persisted as a denied row.
        let msgs = messages(&state).await;
        let rows = state.db.list_tool_calls(msgs[1].id.clone()).await.unwrap();
        assert_eq!(rows[0].status, "error");
        assert_eq!(rows[0].permission_mode.as_deref(), Some("denied"));
    }

    #[tokio::test]
    async fn session_grant_auto_allows_mutating_tool() {
        let provider = FakeProvider::scripted_sequence(vec![
            tool_call_script("fs_write", r#"{"path":"granted.txt","content":"v"}"#, "writing"),
            main_stream("done", 2),
        ]);
        let (_dir, state) = app_with(provider);
        create_conv(&state).await;
        let root = bind_root_keep(&state, "c1").await;
        state
            .session_grants
            .lock()
            .await
            .grant("fs_write", &root.path().display().to_string());

        let result = chat_send_inner(&state, args("go")).await.unwrap();
        assert_eq!(result.status, MessageStatus::Complete);

        let msgs = messages(&state).await;
        let rows = state.db.list_tool_calls(msgs[1].id.clone()).await.unwrap();
        assert_eq!(rows[0].status, "ok");
        assert_eq!(rows[0].permission_mode.as_deref(), Some("session"));
        assert_eq!(
            std::fs::read_to_string(root.path().join("granted.txt")).unwrap(),
            "v"
        );
    }

    #[tokio::test]
    async fn persisted_always_grant_auto_allows_mutating_tool() {
        let provider = FakeProvider::scripted_sequence(vec![
            tool_call_script("fs_write", r#"{"path":"always.txt","content":"v"}"#, "writing"),
            main_stream("done", 2),
        ]);
        let (_dir, state) = app_with(provider);
        create_conv(&state).await;
        let root = bind_root_keep(&state, "c1").await;
        state
            .db
            .set_tool_permission(
                "fs_write".into(),
                root.path().display().to_string(),
                "always".into(),
            )
            .await
            .unwrap();

        let result = chat_send_inner(&state, args("go")).await.unwrap();
        assert_eq!(result.status, MessageStatus::Complete);

        let msgs = messages(&state).await;
        let rows = state.db.list_tool_calls(msgs[1].id.clone()).await.unwrap();
        assert_eq!(rows[0].status, "ok");
        assert_eq!(rows[0].permission_mode.as_deref(), Some("always"));
        assert_eq!(
            std::fs::read_to_string(root.path().join("always.txt")).unwrap(),
            "v"
        );
    }

    #[test]
    fn matrix_root_lexical_containment() {
        let ws = vec!["C:\\work".to_string()];
        assert_eq!(matrix_root(&ws, r"C:\work\a.txt").as_deref(), Some("C:\\work"));
        assert_eq!(matrix_root(&ws, r"c:\WORK\a.txt").as_deref(), Some("C:\\work"));
        assert_eq!(matrix_root(&ws, "rel.txt").as_deref(), Some("C:\\work"));
        assert_eq!(matrix_root(&ws, r"D:\other\a.txt"), None);
    }

    #[test]
    fn approval_payload_write_variants() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("f.txt");
        std::fs::write(&p, "one\ntwo\n").unwrap();

        // Overwrite: diff shows the change, full content carried for editing.
        let args = serde_json::json!({"path": p.display().to_string(), "content": "one\nTWO\n"});
        let (summary, diff, content) =
            build_approval_payload("fs_write", &args, &p.display().to_string());
        assert!(summary.contains("overwrites"), "{summary}");
        let diff = diff.expect("diff for overwrite");
        assert!(diff.contains("-two"), "{diff}");
        assert!(diff.contains("+TWO"), "{diff}");
        assert_eq!(content.as_deref(), Some("one\nTWO\n"));

        // Creation: no diff, summary names the file.
        std::fs::remove_file(&p).unwrap();
        let (summary, diff, content) =
            build_approval_payload("fs_write", &args, &p.display().to_string());
        assert!(summary.contains("creates a new file"), "{summary}");
        assert!(diff.is_none());
        assert_eq!(content.as_deref(), Some("one\nTWO\n"));

        // Append: no diff, no content takeover.
        let args = serde_json::json!({"path": p.display().to_string(), "content": "x", "mode": "append"});
        let (summary, diff, content) =
            build_approval_payload("fs_write", &args, &p.display().to_string());
        assert!(summary.contains("appends"), "{summary}");
        assert!(diff.is_none());
        assert!(content.is_none());
    }

    #[test]
    fn approval_payload_edit_replaces_all_occurrences() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("f.txt");
        std::fs::write(&p, "a X b X c\n").unwrap();
        let args = serde_json::json!({
            "path": p.display().to_string(),
            "old_string": "X",
            "new_string": "Y"
        });
        let (summary, diff, content) =
            build_approval_payload("fs_edit", &args, &p.display().to_string());
        assert!(summary.contains("replaces"), "{summary}");
        assert_eq!(content.as_deref(), Some("a Y b Y c\n"));
        let diff = diff.expect("diff");
        assert!(diff.contains("-a X b X c"), "{diff}");
        assert!(diff.contains("+a Y b Y c"), "{diff}");
    }

    #[test]
    fn approval_payload_delete_mentions_recycle_bin() {
        let args = serde_json::json!({"path": "somewhere/old.txt"});
        let (summary, diff, content) =
            build_approval_payload("fs_delete", &args, "somewhere/old.txt");
        assert!(summary.contains("Recycle Bin"), "{summary}");
        assert!(diff.is_none() && content.is_none());
    }

    #[tokio::test]
    async fn tool_exec_ctx_carries_conversation_workspaces() {
        let provider = FakeProvider::scripted_sequence(vec![
            tool_call_script("ctx", "{}", ""),
            main_stream("done", 2),
        ]);
        let (_dir, mut state) = app_with(provider);
        state
            .tool_registry
            .register(Arc::new(crate::tools::testkit::CtxTool));
        create_conv(&state).await;
        let root = tempfile::tempdir().unwrap();
        let canonical = std::fs::canonicalize(root.path()).unwrap();
        state
            .db
            .set_conversation_workspaces(
                "c1".into(),
                vec![canonical.display().to_string()],
            )
            .await
            .unwrap();

        chat_send_inner(&state, args("hi")).await.unwrap();

        // The executing tool saw the bound root (canonical, verbatim form)
        // through its context.
        let msgs = messages(&state).await;
        let rows = state.db.list_tool_calls(msgs[1].id.clone()).await.unwrap();
        let result = rows[0].result.as_deref().unwrap_or("");
        assert_eq!(result, format!("workspaces:{}", canonical.display()));
    }

    #[tokio::test]
    async fn prior_tool_results_promote_to_titan() {
        // Turn 1: two tool calls land (H3 counts rows); turn 2's heuristic
        // routing must pick up prior_tool_results ≥ 2 ⇒ Titan.
        let provider = FakeProvider::scripted_sequence(vec![
            vec![
                StreamEvent::TextDelta { text: "using tools".into() },
                StreamEvent::ToolCallStart { index: 0, id: "call_0".into(), name: "echo".into() },
                StreamEvent::ToolCallDelta { index: 0, args_delta: r#"{"text":"a"}"#.into() },
                StreamEvent::ToolCallStart { index: 1, id: "call_1".into(), name: "echo".into() },
                StreamEvent::ToolCallDelta { index: 1, args_delta: r#"{"text":"b"}"#.into() },
                StreamEvent::Usage { tokens_in: 9, tokens_out: 6, latency_ms: 11 },
                StreamEvent::Done,
            ],
            main_stream("first", 3),
            main_stream("second", 3),
        ]);
        let (_dir, mut state) = app_with_settings(
            provider,
            &[
                (keys::TRIAD_ROLE_SCOUT, "scout-model"),
                (keys::TRIAD_ROLE_TITAN, "titan-model"),
            ],
        );
        with_echo_tool(&mut state);
        create_conv(&state).await;

        chat_send_inner(&state, args("hi")).await.unwrap();
        let msgs1 = messages(&state).await;
        assert_eq!(msgs1[1].model_role, Some(ModelRole::Scout), "H6 on turn 1");

        chat_send_inner(&state, args_with_id("um2", "hi again")).await.unwrap();

        let events = state.db.list_routing_events("c1".into(), 10).await.unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].decision.source, DecisionSource::Heuristic);
        assert_eq!(events[0].final_target, Target::Titan);
        assert_eq!(events[0].decision.reason, "multiple tool results");
        assert_eq!(events[0].actual_model, "titan-model");

        let msgs2 = messages(&state).await;
        assert_eq!(msgs2.last().unwrap().model_id.as_deref(), Some("titan-model"));
    }
}