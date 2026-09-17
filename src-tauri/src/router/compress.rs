//! Context compression (design §6.5c, backlog): when the assembled context
//! approaches the routed model's window, older history is summarized and
//! replaced with a compact summary; the recent tail stays verbatim. The
//! budget is per-request — computed against the routed model's effective
//! window — and the summarization is progressive: a previously stored
//! summary folds in, so each pass re-summarizes only the turns newer than
//! the last compressed point (stored per conversation). Strictly
//! best-effort: any failure degrades to plain truncation, never blocking.

use tokio_util::sync::CancellationToken;

use super::herald;
use crate::{
    providers::Provider,
    state::{AppState, DEFAULT_ENDPOINT},
    types::{
        ChatContent, ChatMessage, ChatParams, ChatRequest, ChatRole, ContentPart,
        Message as DbMessage, MessageStatus,
    },
};

/// Cheap token estimate: ~4 chars per token (roughly right for English and
/// code; generous for CJK). Order-of-magnitude is all planning needs.
pub const CHARS_PER_TOKEN: usize = 4;
/// Fixed overhead per message (role tokens, delimiters, tool envelopes).
const MESSAGE_OVERHEAD_TOKENS: usize = 8;
/// Fixed estimate for an image part: vision token counts vary wildly, and
/// a fixed order-of-magnitude keeps the budget honest.
const IMAGE_TOKENS: usize = 1024;
/// Pad for the turn's reply — the input side is measured, this only
/// reserves output room (¼ of the window, clamped).
pub fn reserve_tokens(window: u32) -> u32 {
    (window / 4).clamp(512, 4096)
}
/// Slack for system messages added around the tail (digest, prior-model
/// note, this summary itself).
pub const SYSTEM_HEADROOM_TOKENS: u32 = 768;
/// Below this estimate the whole compression path (db reads, window
/// lookup, tool-json sizing) is skipped — no usable window is smaller.
pub const MIN_CHECK_TOKENS: usize = 2048;
/// Summarizer completion timeout: longer than a sidecar digest (prefixes
/// are bigger) but bounded so a stalled summarizer never stalls the send.
pub const COMPRESSION_TIMEOUT_MS: u64 = 20_000;

pub fn estimate_text_tokens(text: &str) -> usize {
    text.chars().count().div_ceil(CHARS_PER_TOKEN)
}

pub fn estimate_parts_tokens(parts: &[ContentPart]) -> usize {
    parts
        .iter()
        .map(|p| match p {
            ContentPart::Text { text } => estimate_text_tokens(text),
            ContentPart::Image { .. } => IMAGE_TOKENS,
        })
        .sum()
}

pub fn estimate_message_tokens(msg: &DbMessage) -> usize {
    MESSAGE_OVERHEAD_TOKENS + estimate_parts_tokens(&msg.content)
}

/// Total estimate over a history slice, skipping streaming placeholders —
/// the assembly loop skips them too, so estimates and reality align.
pub fn estimate_history_tokens(history: &[DbMessage]) -> usize {
    history
        .iter()
        .filter(|m| m.status != MessageStatus::Streaming)
        .map(estimate_message_tokens)
        .sum()
}

/// Effective context window for the routed model (§6.5c): the model
/// record's verified window when known, else the `num_ctx` setting. On
/// local Ollama the request's `num_ctx` IS the server-side window, so a
/// larger record is clamped down to it; cloud endpoints have no such clamp.
pub fn effective_window(record_tokens: Option<u32>, num_ctx: u32, local: bool) -> u32 {
    match record_tokens {
        Some(w) if local => w.min(num_ctx),
        Some(w) => w,
        None => num_ctx,
    }
}

/// How much recent history fits the budget: walk newest → oldest, keeping
/// whole messages until the budget is exhausted. `keep_from` indexes the
/// slice — `history[..keep_from]` is summarized away, `history[keep_from..]`
/// stays verbatim. `keep_from == 0` → it all fits, no compression.
pub struct Plan {
    pub keep_from: usize,
}

pub fn plan(history: &[DbMessage], budget_tokens: usize) -> Plan {
    let mut acc = 0usize;
    for (i, msg) in history.iter().enumerate().rev() {
        if msg.status == MessageStatus::Streaming {
            continue; // our own placeholder — skipped by assembly too
        }
        acc += estimate_message_tokens(msg);
        if acc > budget_tokens {
            // Keep at least the newest message verbatim (the user's current
            // input must not be summarized away); only a single giant
            // oldest message would summarize itself entirely.
            let keep_from = if i == 0 { 1 } else { i };
            return Plan { keep_from };
        }
    }
    Plan { keep_from: 0 }
}

/// The summarizer's input plan (progressive fold): a stored summary covering
/// everything up to its `upto` id is folded in — only the turns newer than
/// that point are summarized this pass.
pub struct FoldPlan<'a> {
    /// The previous summary to fold in (None → summarize the whole prefix).
    pub seed: Option<String>,
    /// Messages that still need summarizing.
    pub to_summarize: &'a [DbMessage],
    /// Last message id the NEW summary will cover.
    pub upto_id: String,
}

pub fn fold<'a>(prev: Option<(String, String)>, prefix: &'a [DbMessage]) -> FoldPlan<'a> {
    let upto_id = match prefix.last() {
        Some(m) => m.id.clone(),
        None => {
            return FoldPlan {
                seed: None,
                to_summarize: &[],
                upto_id: String::new(),
            }
        }
    };
    match prev {
        Some((summary, upto)) if !summary.trim().is_empty() => {
            match prefix.iter().position(|m| m.id == upto) {
                // Stored point inside the prefix: fold — summarize only the
                // newer turns, seeded with the stored summary.
                Some(pos) => FoldPlan {
                    seed: Some(summary),
                    to_summarize: &prefix[pos + 1..],
                    upto_id,
                },
                // Stored point no longer exists here: full re-summarize.
                None => FoldPlan {
                    seed: None,
                    to_summarize: prefix,
                    upto_id,
                },
            }
        }
        _ => FoldPlan {
            seed: None,
            to_summarize: prefix,
            upto_id,
        },
    }
}

/// One summarizer completion, bounded and cancellable. Any failure
/// (timeout, provider error, cancellation, empty output) returns None and
/// the caller falls back to plain truncation with a note.
pub async fn summarize_prefix(
    provider: &dyn Provider,
    summarizer_model: &str,
    keep_alive: &str,
    seed: Option<String>,
    to_summarize: &[DbMessage],
    timeout_ms: u64,
    cancel: &CancellationToken,
) -> Option<String> {
    // Per-message truncation keeps the summarizer's own input bounded —
    // the tail side of the split is what carries full fidelity.
    let transcript = super::sidecars::transcript(to_summarize, 400, 1500);
    if transcript.trim().is_empty() {
        return None;
    }
    let user = match &seed {
        Some(prev) => {
            format!("Summary so far (already compressed):\n{prev}\n\nNewer turns to fold in:\n{transcript}")
        }
        None => transcript,
    };
    let req = ChatRequest {
        endpoint_id: String::new(),
        model: summarizer_model.to_string(),
        messages: vec![
            ChatMessage {
                id: String::new(),
                role: ChatRole::System,
                content: ChatContent::Text(
                    "Compress the conversation below into a compact summary for \
                     continuing the same task. Preserve: the current task and its \
                     state, decisions made, pinned facts, open questions, and key \
                     names, paths, and numbers. At most 300 words. Output ONLY the \
                     summary text."
                        .into(),
                ),
                tool_calls: None,
                tool_call_id: None,
                tool_name: None,
            },
            ChatMessage {
                id: String::new(),
                role: ChatRole::User,
                content: ChatContent::Text(user),
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
            keep_alive: Some(keep_alive.to_string()),
        },
    };
    let fut = herald::complete_once(provider, req, cancel);
    match tokio::time::timeout(std::time::Duration::from_millis(timeout_ms), fut).await {
        Ok(Ok(text)) => {
            let trimmed = text.trim();
            (!trimmed.is_empty()).then(|| trimmed.to_string())
        }
        _ => None,
    }
}

/// Compress a history prefix: fold the stored summary, summarize what's
/// new, persist the fold point. Returns the summary text to inject, or
/// None when compression failed (the caller then truncates with a note).
pub async fn compress_prefix(
    state: &AppState,
    conversation_id: &str,
    summarizer_ref: Option<&str>,
    routed_model: &str,
    keep_alive: &str,
    timeout_ms: u64,
    prefix: &[DbMessage],
    cancel: &CancellationToken,
) -> Option<String> {
    let prev = state
        .db
        .get_conversation_compression(conversation_id.to_string())
        .await
        .ok()
        .flatten();
    let folded = fold(prev, prefix);
    if folded.upto_id.is_empty() {
        return None; // empty prefix — nothing to compress
    }
    if folded.to_summarize.is_empty() {
        // The stored summary already covers the whole prefix.
        return folded.seed;
    }
    // Summarizer: the Herald role when assigned, else the routed model
    // itself (§6.5c "the summarizer (Herald/Titan)").
    let model_ref = summarizer_ref.unwrap_or(routed_model);
    if model_ref.starts_with(crate::owui::PIPE_PREFIX) {
        return None; // a Pipe can't summarize — degrade to the omission note
    }
    let (endpoint, bare) = crate::providers::parse_model_ref(model_ref);
    let provider = state
        .provider_for(endpoint.as_deref().unwrap_or(DEFAULT_ENDPOINT))
        .ok()?;
    let summary = summarize_prefix(
        provider.as_ref(),
        &bare,
        keep_alive,
        folded.seed,
        folded.to_summarize,
        timeout_ms,
        cancel,
    )
    .await?;
    if let Err(e) = state
        .db
        .set_conversation_compression(conversation_id.to_string(), summary.clone(), folded.upto_id)
        .await
    {
        log::warn!("compression store failed: {e}");
    }
    Some(summary)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ChatRole;

    fn msg(id: &str, text: &str) -> DbMessage {
        DbMessage {
            id: id.into(),
            conversation_id: "c1".into(),
            role: ChatRole::User,
            content: vec![ContentPart::Text { text: text.into() }],
            reasoning: None,
            model_role: None,
            model_id: None,
            endpoint_id: None,
            tokens_in: None,
            tokens_out: None,
            latency_ms: None,
            status: MessageStatus::Complete,
            error: None,
            created_at: 0,
            routing: None,
            tool_calls: Vec::new(),
        }
    }

    #[test]
    fn estimate_counts_text_and_fixed_images() {
        assert_eq!(estimate_text_tokens(""), 0);
        assert_eq!(estimate_text_tokens("abcd"), 1);
        assert_eq!(estimate_text_tokens("abcde"), 2);
        let parts = vec![
            ContentPart::Text { text: "abcd".into() },
            ContentPart::Image {
                attachment_id: "a".into(),
                mime: "image/png".into(),
                data_base64: None,
                processed_path: None,
            },
        ];
        assert_eq!(estimate_parts_tokens(&parts), 1 + IMAGE_TOKENS);
    }

    #[test]
    fn plan_keeps_the_recent_tail() {
        // ~108 tokens each; a 250-token budget fits two.
        let history: Vec<_> = (0..3).map(|i| msg(&format!("m{i}"), &"x".repeat(400))).collect();
        let p = plan(&history, 250);
        assert_eq!(p.keep_from, 1);
    }

    #[test]
    fn plan_zero_when_it_fits() {
        let history: Vec<_> = (0..3).map(|i| msg(&format!("m{i}"), &"x".repeat(40))).collect();
        assert_eq!(plan(&history, 10_000).keep_from, 0);
    }

    #[test]
    fn plan_always_keeps_the_newest_message() {
        let history = vec![msg("m0", &"x".repeat(400)), msg("m1", &"x".repeat(400))];
        let p = plan(&history, 64);
        assert_eq!(p.keep_from, 1, "the current turn stays verbatim");
    }

    #[test]
    fn plan_skips_streaming_placeholders() {
        let mut placeholder = msg("ph", "pending");
        placeholder.status = MessageStatus::Streaming;
        let history = vec![msg("m0", &"x".repeat(400)), placeholder, msg("m2", &"x".repeat(400))];
        let p = plan(&history, 150);
        assert_eq!(p.keep_from, 1, "placeholder ignored; m2 kept, m0 summarized");
    }

    #[test]
    fn effective_window_follows_the_record_with_local_clamp() {
        // Local Ollama: the request's num_ctx is the real window.
        assert_eq!(effective_window(Some(32768), 8192, true), 8192);
        // Cloud: the record is the truth.
        assert_eq!(effective_window(Some(32768), 8192, false), 32768);
        // Unknown model: the setting stands in.
        assert_eq!(effective_window(None, 8192, false), 8192);
    }

    #[test]
    fn fold_seeds_and_summarizes_only_newer_turns() {
        let prefix = vec![msg("m1", "a"), msg("m2", "b"), msg("m3", "c")];
        let f = fold(Some(("stored summary".into(), "m2".into())), &prefix);
        assert_eq!(f.seed.as_deref(), Some("stored summary"));
        assert_eq!(f.to_summarize.len(), 1);
        assert_eq!(f.to_summarize[0].id, "m3");
        assert_eq!(f.upto_id, "m3");
    }

    #[test]
    fn fold_uses_the_stored_summary_when_it_already_covers() {
        let prefix = vec![msg("m2", "a"), msg("m3", "b")];
        let f = fold(Some(("stored".into(), "m3".into())), &prefix);
        assert_eq!(f.seed.as_deref(), Some("stored"));
        assert!(f.to_summarize.is_empty());
    }

    #[test]
    fn fold_redoes_without_a_valid_seed() {
        let prefix = vec![msg("m1", "a"), msg("m2", "b")];
        // Missing upto → full re-summarize.
        let f = fold(Some(("stored".into(), "mX".into())), &prefix);
        assert!(f.seed.is_none());
        assert_eq!(f.to_summarize.len(), 2);
        // No stored summary at all → full re-summarize.
        let f = fold(None, &prefix);
        assert!(f.seed.is_none());
        assert_eq!(f.to_summarize.len(), 2);
        // Blank stored summary → ignored.
        let f = fold(Some(("  ".into(), "m1".into())), &prefix);
        assert!(f.seed.is_none());
        assert_eq!(f.to_summarize.len(), 2);
    }
}