/**
 * Stream protocol — verbatim mirror of the serde enum in
 * `src-tauri/src/types.rs` (design Appendix A). A Rust unit test
 * (`stream_event_serialization_matches_frontend_union`) asserts the serialized
 * tags/field names match this union; change both files together.
 *
 * M0 emits status / reasoning_delta / text_delta / usage / error / done.
 * The routing (M1) and tool_call_* (M2) variants exist so unknown events
 * never crash the UI; the store's switch ignores them until those milestones.
 */

export type StatusPhase = "routing" | "connecting" | "loading_model";

export type Target = "scout" | "titan";

export type DecisionSource = "herald" | "heuristic" | "hard_rule" | "manual";

export interface RoutingFlags {
  vision: boolean;
  tools: boolean;
  code: boolean;
  longForm: boolean;
  multiStep: boolean;
  sensitive: boolean;
}

export interface RoutingDecision {
  target: Target;
  confidence: number;
  /** 1..=5 */
  complexity: number;
  reason: string;
  flags: RoutingFlags;
  estInTokens: number;
  estOutTokens: number;
  handoffNote: string;
  source: DecisionSource;
}

export type StreamEvent =
  | { type: "status"; phase: StatusPhase }
  | { type: "routing"; decision: RoutingDecision; finalTarget: Target }
  | { type: "reasoning_delta"; text: string }
  | { type: "text_delta"; text: string }
  | { type: "tool_call_start"; index: number; id: string; name: string }
  | { type: "tool_call_delta"; index: number; argsDelta: string }
  | {
      type: "tool_result";
      callId: string;
      content: string;
      isError: boolean | null;
    }
  | {
      type: "approval_request";
      id: string;
      callId: string;
      tool: string;
      path: string;
      secondaryPath: string | null;
      summary: string;
      /** Unified diff for fs_write/fs_edit; null for the other tools. */
      diff: string | null;
      /** fs_write/fs_edit only: the full content that would land. */
      resultContent: string | null;
    }
  | { type: "usage"; tokensIn: number; tokensOut: number; latencyMs: number }
  | { type: "error"; code: string; message: string; retryable: boolean }
  | { type: "done" };