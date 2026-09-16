//! Sidecar tasks (design §3.7): thread titles, rolling digests, follow-up
//! suggestions. Always Herald, never a worker model; run out-of-band so they
//! never block or interrupt the main stream. Every failure is logged and
//! dropped — sidecars are strictly best-effort.

use std::future::Future;
use std::sync::Arc;

use tokio_util::sync::CancellationToken;

use super::herald::{complete_once, HeraldError};
use crate::{
    db::Database,
    providers::Provider,
    state::AppState,
    types::{ChatContent, ChatMessage, ChatParams, ChatRequest, ChatRole},
};

/// Everything a spawned sidecar task owns — cloned out of `AppState` so the
/// task has no borrow on it.
#[derive(Clone)]
pub struct SidecarCtx {
    pub db: Database,
    pub provider: Arc<dyn Provider>,
    pub herald_model: String,
    pub timeout_ms: u64,
    pub keep_alive: String,
    pub suggestions: Arc<std::sync::RwLock<std::collections::HashMap<String, Vec<String>>>>,
}

fn text_request(ctx: &SidecarCtx, system: &str, user: &str) -> ChatRequest {
    ChatRequest {
        endpoint_id: String::new(),
        model: ctx.herald_model.clone(),
        messages: vec![
            ChatMessage {
                id: String::new(),
                role: ChatRole::System,
                content: ChatContent::Text(system.into()),
                tool_calls: None,
                tool_call_id: None,
                tool_name: None,
            },
            ChatMessage {
                id: String::new(),
                role: ChatRole::User,
                content: ChatContent::Text(user.into()),
                tool_calls: None,
                tool_call_id: None,
                tool_name: None,
            },
        ],
        tools: None,
        params: ChatParams {
            temperature: Some(0.4),
            top_p: None,
            num_ctx: None,
            max_tokens: None,
            json_schema: None,
            keep_alive: Some(ctx.keep_alive.clone()),
        },
    }
}

/// One Herald completion, bounded by the configured timeout. On timeout the
/// token is cancelled so the provider drops the response and frees the slot
/// (§5.3 drop-response semantics).
async fn complete_bounded(ctx: &SidecarCtx, req: ChatRequest) -> Result<String, HeraldError> {
    let cancel = CancellationToken::new();
    let fut = complete_once(ctx.provider.as_ref(), req, &cancel);
    tokio::pin!(fut);
    match tokio::time::timeout(
        std::time::Duration::from_millis(ctx.timeout_ms),
        fut.as_mut(),
    )
    .await
    {
        Ok(res) => res,
        Err(_) => {
            cancel.cancel();
            Err(HeraldError::Timeout(ctx.timeout_ms))
        }
    }
}

/// Fire-and-forget runner: log failures, never panic, never block.
fn spawn_logged(
    what: &'static str,
    conversation_id: String,
    fut: impl Future<Output = Result<(), HeraldError>> + Send + 'static,
) {
    tokio::spawn(async move {
        if let Err(e) = fut.await {
            log::warn!("sidecar {what} failed for {conversation_id}: {e}");
        }
    });
}

/// Title sidecar: overwrite the truncation fallback with a Herald title.
pub fn spawn_title(ctx: SidecarCtx, conversation_id: String, transcript: String) {
    spawn_logged("title", conversation_id.clone(), async move {
        let started = std::time::Instant::now();
        let req = text_request(
            &ctx,
            "Generate a concise chat title for the conversation below. \
             Output ONLY the title text — 3 to 6 words, no quotes, no period.",
            &transcript,
        );
        let raw = complete_bounded(&ctx, req).await?;
        let title = raw.trim().trim_matches('"').trim().to_string();
        if title.is_empty() || title.chars().count() > 80 {
            return Err(HeraldError::Parse(format!("implausible title: {title:?}")));
        }
        log::info!(
            "sidecar title ({} ms): {title:?}",
            started.elapsed().as_millis()
        );
        ctx.db.retitle_conversation(conversation_id, title).await?;
        Ok(())
    });
}

/// Rolling digest sidecar (§3.6): task state / decisions / open questions,
/// cached per thread, injected as a system message on model handoffs.
pub fn spawn_digest(ctx: SidecarCtx, conversation_id: String, transcript: String) {
    spawn_logged("digest", conversation_id.clone(), async move {
        let req = text_request(
            &ctx,
            "Summarize the state of the task in this conversation for handing \
             off to another assistant. Cover: the current task, decisions made, \
             open questions. At most 100 words. Output ONLY the summary.",
            &transcript,
        );
        let raw = complete_bounded(&ctx, req).await?;
        let digest = raw.trim().to_string();
        if digest.is_empty() {
            return Err(HeraldError::Parse("empty digest".into()));
        }
        ctx.db.set_conversation_digest(conversation_id, digest).await?;
        Ok(())
    });
}

/// Follow-up suggestion chips (§3.7): 3 short next-user-messages.
pub fn spawn_suggestions(ctx: SidecarCtx, conversation_id: String, transcript: String) {
    spawn_logged("suggestions", conversation_id.clone(), async move {
        let req = text_request(
            &ctx,
            "Suggest 3 short follow-up messages the user might send next in \
             this conversation. Each under 8 words, in the user's language. \
             Output ONLY a JSON array of 3 strings — no other text.",
            &transcript,
        );
        let raw = complete_bounded(&ctx, req).await?;
        let items = parse_suggestions(&raw)?;
        ctx.suggestions
            .write()
            .unwrap_or_else(|p| p.into_inner())
            .insert(conversation_id, items);
        Ok(())
    });
}

fn parse_suggestions(raw: &str) -> Result<Vec<String>, HeraldError> {
    let start = raw.find('[').ok_or_else(|| HeraldError::Parse("no array".into()))?;
    let end = raw.rfind(']').ok_or_else(|| HeraldError::Parse("no array end".into()))?;
    let items: Vec<String> = serde_json::from_str(&raw[start..=end])
        .map_err(|e| HeraldError::Parse(format!("suggestions json: {e}")))?;
    Ok(items
        .into_iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .take(3)
        .collect())
}

/// Build a compact transcript (role-tagged, per-message truncated) from
/// history — the user text sidecars see.
pub fn transcript(history: &[crate::types::Message], max_messages: usize, max_chars: usize) -> String {
    let tail = history.iter().rev().take(max_messages).collect::<Vec<_>>();
    let mut out = String::new();
    for msg in tail.into_iter().rev() {
        let role = match msg.role {
            ChatRole::User => "user",
            ChatRole::Assistant => "assistant",
            ChatRole::System => "system",
            ChatRole::Tool => "tool",
        };
        let text: String = msg
            .content
            .iter()
            .filter_map(|p| match p {
                crate::types::ContentPart::Text { text } => Some(text.as_str()),
                crate::types::ContentPart::Image { .. } => Some("[image]"),
            })
            .collect();
        let mut chunk: String = text.chars().take(max_chars).collect();
        if text.chars().count() > max_chars {
            chunk.push('…');
        }
        out.push_str(&format!("[{role}] {chunk}\n"));
    }
    out
}

/// Clone the sidecar context out of app state, if Herald is usable at all.
pub fn sidecar_ctx(state: &AppState) -> Option<SidecarCtx> {
    let cfg = crate::router::config::TriadConfig::load(&state.settings);
    let herald_model = cfg.roles.herald.clone()?;
    let provider = state.provider_for(crate::state::DEFAULT_ENDPOINT).ok()?;
    Some(SidecarCtx {
        db: state.db.clone(),
        provider,
        herald_model,
        timeout_ms: cfg.herald_timeout_ms,
        keep_alive: cfg.herald_keep_alive.clone(),
        suggestions: state.suggestions.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_suggestions_tolerates_prose() {
        let raw = r#"Here you go: ["Explain step 2 more", "Show a code example", "What about errors?"]"#;
        let items = parse_suggestions(raw).unwrap();
        assert_eq!(items.len(), 3);
        assert_eq!(items[0], "Explain step 2 more");
    }

    #[test]
    fn parse_suggestions_rejects_garbage() {
        assert!(parse_suggestions("nope").is_err());
        assert!(parse_suggestions("[1, 2, 3]").is_err(), "non-strings rejected");
    }

    #[test]
    fn parse_suggestions_takes_three() {
        let items =
            parse_suggestions(r#"["a","b","c","d","e"]"#).unwrap();
        assert_eq!(items.len(), 3);
    }

    #[test]
    fn transcript_truncates_each_message() {
        let long = "x".repeat(500);
        let msgs = vec![crate::types::Message {
            id: "m1".into(),
            conversation_id: "c".into(),
            role: ChatRole::User,
            content: vec![crate::types::ContentPart::Text { text: long }],
            reasoning: None,
            model_role: None,
            model_id: None,
            endpoint_id: None,
            tokens_in: None,
            tokens_out: None,
            latency_ms: None,
            status: crate::types::MessageStatus::Complete,
            error: None,
            created_at: 0,
            routing: None,
        }];
        let t = transcript(&msgs, 6, 100);
        assert!(t.starts_with("[user] "));
        assert!(t.contains('…'));
        assert!(t.len() < 200);
    }
}