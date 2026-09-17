/** IPC DTOs — must mirror the serde structs in src-tauri/src/types.rs. */

import type { RoutingDecision, Target } from "./stream";

export type ConnectionStatus = "ok" | "down" | "unknown";

/**
 * The Triad pin on a conversation (§3.10): 'auto' routes through Herald,
 * 'scout'/'titan' force a role, any other value pins an explicit model.
 */
export type PinValue = "auto" | "scout" | "titan";

export type ChatRole = "system" | "user" | "assistant" | "tool";
export type MessageStatus = "streaming" | "complete" | "stopped" | "error";

export type ContentPart =
  | { type: "text"; text: string }
  | {
      type: "image";
      attachmentId: string;
      mime: string;
      dataBase64?: string;
    };

export interface Conversation {
  id: string;
  title: string | null;
  createdAt: number;
  updatedAt: number;
  /** 'auto' | 'scout' | 'titan' | explicit model id | null (= 'auto'). */
  pinnedModel: string | null;
  workspaceRoots: string[];
  systemPrompt: string | null;
  archived: boolean;
  /** Rolling digest maintained by the Herald sidecar (§3.6). */
  digest: string | null;
}

/** Routing outcome joined onto a message (design §9.2 ribbon data). */
export interface MessageRouting {
  decision: RoutingDecision;
  finalTarget: Target;
  actualModel: string;
  latencyMs: number;
}

/** One executed tool call behind an assistant message (tool runtime §6.1). */
export interface ToolCallRow {
  id: string;
  messageId: string;
  tool: string;
  args: string | null;
  result: string | null;
  /** running | ok | error | denied */
  status: "running" | "ok" | "error" | "denied";
  /** null until the §6.6 permission matrix (auto | ask | session | always). */
  permissionMode: string | null;
  createdAt: number;
}

/** Live/persisted view shape for the tool-activity renderer. */
export interface ToolCallView {
  callId: string;
  name: string;
  args: string;
  result: string | null;
  status: ToolCallRow["status"];
}

export interface Message {
  id: string;
  conversationId: string;
  role: ChatRole;
  content: ContentPart[];
  modelRole: string | null;
  modelId: string | null;
  endpointId: string | null;
  tokensIn: number | null;
  tokensOut: number | null;
  latencyMs: number | null;
  reasoning: string | null;
  status: MessageStatus;
  error: string | null;
  createdAt: number;
  /** Router decision behind an assistant message; null for user messages. */
  routing: MessageRouting | null;
  /** Executed tool calls behind an assistant message, oldest first. */
  toolCalls: ToolCallRow[];
}

/** A mutating tool waiting for the user's decision (§6.6). */
export interface ApprovalRequestView {
  id: string;
  callId: string;
  /** Conversation the call belongs to (matched from the stream). */
  conversationId: string;
  tool: string;
  path: string;
  secondaryPath: string | null;
  summary: string;
  /** Unified diff for fs_write/fs_edit; null for the other tools. */
  diff: string | null;
  /** fs_write/fs_edit only: the full content that would land. */
  resultContent: string | null;
}

/** Result of a finished `chat_send` invocation. */
export interface ChatSendResult {
  messageId: string;
  status: MessageStatus;
  tokensIn: number;
  tokensOut: number;
  latencyMs: number;
  /** Scout ran past its output ceiling — the UI offers "Continue with Titan". */
  escalationAvailable: boolean;
}

/** One persisted router decision (router log drawer, design §9.2). */
export interface RoutingEvent {
  id: string;
  ts: number;
  conversationId: string;
  messageId: string;
  decision: RoutingDecision;
  finalTarget: Target;
  actualModel: string;
  latencyMs: number;
  /** "manual" when a user pin chose the model; null for router decisions. */
  overrideKind: string | null;
}

export interface ModelInfo {
  /** Model reference (§5.1): bare for the built-in, "model@endpoint" for profiles. */
  id: string;
  displayName: string;
  /** Owning endpoint profile id ("ep_local_ollama" for bare ids). */
  endpointId: string;
  sizeBytes: number | null;
  parameterSize: string | null;
  quantizationLevel: string | null;
  family: string | null;
  contextLength: number | null;
  /** e.g. ["completion", "vision", "tools", "thinking"] */
  capabilities: string[];
}