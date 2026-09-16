//! Pure NDJSON handling for Ollama's `/api/chat` stream (no async, no I/O —
//! fully unit-testable). Shape verified live against Ollama 0.34.1:
//!
//! delta:  {"model":"m","created_at":"…","message":{"role":"assistant","content":"Hi"},"done":false}
//! thinking delta: {"message":{"thinking":"…","content":""},"done":false}
//! final:  {"done":true,"done_reason":"stop","total_duration":…,"prompt_eval_count":17,"eval_count":4,…}
//! error:  {"error":"model 'x' not found"}

use crate::types::{StatusPhase, StreamEvent};

/// Reassembles NDJSON lines from arbitrary byte-chunk boundaries.
pub(crate) struct LineSplitter {
    buf: String,
}

impl LineSplitter {
    pub fn new() -> Self {
        Self { buf: String::new() }
    }

    /// Feed one chunk; returns every complete line it completed.
    pub fn push(&mut self, chunk: &[u8]) -> Vec<String> {
        self.buf.push_str(&String::from_utf8_lossy(chunk));
        let mut lines = Vec::new();
        while let Some(idx) = self.buf.find('\n') {
            let mut line: String = self.buf.drain(..=idx).collect();
            line.pop(); // '\n'
            if line.ends_with('\r') {
                line.pop(); // CRLF tolerance
            }
            lines.push(line);
        }
        lines
    }

    /// Any trailing bytes after the final newline (normally none).
    pub fn finish(self) -> Option<String> {
        let rest = self.buf.trim();
        if rest.is_empty() {
            None
        } else {
            Some(rest.to_string())
        }
    }
}

/// One `/api/chat` NDJSON line → 0..n normalized StreamEvents.
/// Never panics; malformed input is skipped (logged at debug).
pub(crate) fn parse_chat_line(line: &str) -> Vec<StreamEvent> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return vec![];
    }
    let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed) else {
        log::debug!("ollama: skipping malformed NDJSON line");
        return vec![];
    };

    if let Some(message) = value.get("error").and_then(|e| e.as_str()) {
        let code = if message.contains("not found") {
            "model_not_found"
        } else {
            "ollama_error"
        };
        return vec![StreamEvent::Error {
            code: code.to_string(),
            message: message.to_string(),
            retryable: false,
        }];
    }

    let done = value.get("done").and_then(|d| d.as_bool()).unwrap_or(false);

    if done {
        match value.get("done_reason").and_then(|r| r.as_str()) {
            // Model was loaded but nothing was generated (warmup request) —
            // not a completion; keep waiting for the real stream.
            Some("load") => {
                log::debug!("ollama: model load notification (not terminal)");
                return vec![];
            }
            _ => {}
        }
    }

    let mut events = Vec::new();
    if let Some(message) = value.get("message") {
        if let Some(thinking) = message.get("thinking").and_then(|t| t.as_str()) {
            if !thinking.is_empty() {
                events.push(StreamEvent::ReasoningDelta {
                    text: thinking.to_string(),
                });
            }
        }
        if let Some(content) = message.get("content").and_then(|c| c.as_str()) {
            if !content.is_empty() {
                events.push(StreamEvent::TextDelta {
                    text: content.to_string(),
                });
            }
        }
        // message.tool_calls exist in the wire format but M0 ignores them —
        // the tool runtime (M2) is the seam.
        if message.get("tool_calls").is_some() {
            log::debug!("ollama: tool_calls present but tool runtime is M2; ignoring");
        }
    }

    if done {
        let tokens_in = value.get("prompt_eval_count").and_then(|v| v.as_u64()).unwrap_or(0);
        let tokens_out = value.get("eval_count").and_then(|v| v.as_u64()).unwrap_or(0);
        let latency_ms = value
            .get("total_duration")
            .and_then(|v| v.as_f64())
            .map(|ns| (ns / 1_000_000.0) as u64)
            .unwrap_or(0);
        events.push(StreamEvent::Usage {
            tokens_in,
            tokens_out,
            latency_ms,
        });
        events.push(StreamEvent::Done);
    }

    events
}

/// Unused in M0 but part of the normalized protocol seam; referenced here so
/// the variant stays exercised by tests.
#[allow(dead_code)]
fn _phase_check(_p: StatusPhase) {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::StatusPhase;

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
                StreamEvent::Usage { .. } => "usage",
                StreamEvent::Error { .. } => "error",
                StreamEvent::Done => "done",
            })
            .collect()
    }

    #[test]
    fn content_delta() {
        let line = r#"{"model":"gemma3:4b","created_at":"t","message":{"role":"assistant","content":"Hi"},"done":false}"#;
        let events = parse_chat_line(line);
        assert_eq!(kinds(&events), vec!["text_delta"]);
        match &events[0] {
            StreamEvent::TextDelta { text } => assert_eq!(text, "Hi"),
            _ => unreachable!(),
        }
    }

    #[test]
    fn thinking_then_content() {
        let line = r#"{"message":{"role":"assistant","thinking":"hmm","content":""},"done":false}"#;
        let events = parse_chat_line(line);
        assert_eq!(kinds(&events), vec!["reasoning_delta"]);
    }

    #[test]
    fn final_line_yields_usage_and_done() {
        let line = r#"{"done":true,"done_reason":"stop","total_duration":1234567890,"prompt_eval_count":17,"eval_count":4,"message":{"role":"assistant","content":""}}"#;
        let events = parse_chat_line(line);
        assert_eq!(kinds(&events), vec!["usage", "done"]);
        match &events[0] {
            StreamEvent::Usage { tokens_in, tokens_out, latency_ms } => {
                assert_eq!(*tokens_in, 17);
                assert_eq!(*tokens_out, 4);
                assert_eq!(*latency_ms, 1234); // ns → ms
            }
            _ => unreachable!(),
        }
    }

    #[test]
    fn done_reason_load_is_not_terminal() {
        let line = r#"{"done":true,"done_reason":"load","load_duration":1000000}"#;
        assert!(parse_chat_line(line).is_empty());
    }

    #[test]
    fn error_line_maps_to_error_event() {
        let line = r#"{"error":"model 'nope' not found, try pulling it first"}"#;
        let events = parse_chat_line(line);
        assert_eq!(kinds(&events), vec!["error"]);
        match &events[0] {
            StreamEvent::Error { code, message, retryable } => {
                assert_eq!(code, "model_not_found");
                assert!(message.contains("nope"));
                assert!(!retryable);
            }
            _ => unreachable!(),
        }
    }

    #[test]
    fn malformed_and_blank_lines_are_skipped() {
        assert!(parse_chat_line("not json at all").is_empty());
        assert!(parse_chat_line("").is_empty());
        assert!(parse_chat_line("   ").is_empty());
    }

    #[test]
    fn empty_deltas_produce_no_events() {
        let line = r#"{"message":{"role":"assistant","content":""},"done":false}"#;
        assert!(parse_chat_line(line).is_empty());
    }

    #[test]
    fn linesplitter_handles_crlf_and_partial_chunks() {
        let mut splitter = LineSplitter::new();
        assert_eq!(splitter.push(b"a\nb\r\n"), vec!["a", "b"]);
        // JSON object split across two chunks with no trailing newline:
        // push() returns nothing until finish() flushes the remainder.
        assert!(splitter.push(r#"{"message":{"content":"he"#.as_bytes()).is_empty());
        assert!(splitter.push(r#"llo"},"done":false}"#.as_bytes()).is_empty());
        let tail = splitter.finish().expect("trailing line should flush");
        let events = parse_chat_line(&tail);
        assert_eq!(kinds(&events), vec!["text_delta"]);
    }

    #[test]
    fn linesplitter_finish_flushes_trailing_line() {
        let mut splitter = LineSplitter::new();
        splitter.push(b"{\"done\":true}");
        assert_eq!(splitter.finish().as_deref(), Some("{\"done\":true}"));
    }

    #[test]
    fn status_phase_serializes_as_expected() {
        assert_eq!(
            serde_json::to_value(StatusPhase::LoadingModel).unwrap(),
            serde_json::json!("loading_model")
        );
    }
}