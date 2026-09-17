import { Channel, invoke } from "@tauri-apps/api/core";

import type {
  Attachment,
  ChatSendResult,
  Conversation,
  Message,
  ModelInfo,
  RoutingEvent,
  TriadReport,
} from "../types/chat";
import type { StreamEvent } from "../types/stream";

/**
 * Typed wrappers around Tauri IPC commands. Every command the Rust core
 * exposes gets one function here; the frontend never calls `invoke`
 * directly elsewhere. `sendChat` is long-running: it resolves when the
 * stream ends, with deltas arriving through the Channel callback.
 */

export async function ping(): Promise<string> {
  return invoke<string>("ping");
}

// -- Models ---------------------------------------------------------------

export async function listModels(): Promise<ModelInfo[]> {
  return invoke<ModelInfo[]>("list_models");
}

/** Capability record for one (endpoint, model) — design §5.4/§8.2. */
export interface ModelRecord {
  endpointId: string;
  /** Bare model name (no @endpoint suffix). */
  model: string;
  capabilities: string[];
  contextTokens: number | null;
  role: string | null;
  vramEstimateGb: number | null;
  verifiedAt: number | null;
}

export interface SaveModelRecordArgs {
  endpointId: string;
  model: string;
  capabilities: string[];
  contextTokens: number | null;
}

/** Save a capability record (verified by the act of editing, §5.4). */
export async function saveModelRecord(
  args: SaveModelRecordArgs,
): Promise<ModelRecord> {
  return invoke<ModelRecord>("save_model_record", { args });
}

export async function clearModelRecord(
  endpointId: string,
  model: string,
): Promise<void> {
  return invoke("clear_model_record", { endpointId, model });
}

/** Re-probe an Ollama model via /api/show and store the facts. */
export async function verifyModel(
  endpointId: string,
  model: string,
): Promise<ModelRecord> {
  return invoke<ModelRecord>("verify_model", { endpointId, model });
}

// -- Chat streaming ---------------------------------------------------------

export interface ChatSendArgs {
  conversationId: string;
  /** Client-generated id; makes retries idempotent. */
  userMessageId: string;
  content: string;
  model: string;
  /** Attachment rows (§7.2) saved before sending; linked to the message. */
  attachmentIds: string[];
}

export function sendChat(
  args: ChatSendArgs,
  onEvent: (ev: StreamEvent) => void,
): Promise<ChatSendResult> {
  const channel = new Channel<StreamEvent>();
  channel.onmessage = onEvent;
  return invoke<ChatSendResult>("chat_send", { args, onEvent: channel });
}

export function stopChat(conversationId: string): Promise<void> {
  return invoke("chat_stop", { conversationId });
}

// -- Attachments (design §7) -----------------------------------------------

/** Image bytes (clipboard paste) → IMG pipeline → stored row. */
export function saveAttachment(
  dataBase64: string,
  sourceName?: string,
): Promise<Attachment> {
  return invoke<Attachment>("save_attachment", {
    args: { dataBase64, sourceName },
  });
}

/** Drag-drop / picked file paths; reading stays in Rust (§10.3). */
export function saveAttachmentFile(path: string): Promise<Attachment> {
  return invoke<Attachment>("save_attachment_file", { path });
}

/** §7.1 region capture: a screen rect (overlay-relative physical px) goes
 * through the GDI capturer and IMG pipeline; resolves with the stored row. */
export function captureRegion(args: {
  x: number;
  y: number;
  width: number;
  height: number;
}): Promise<Attachment> {
  return invoke<Attachment>("capture_region", { args });
}

/** §9.3: start a region capture on behalf of a window ("main" | "quick") —
 * the stored attachment is announced to that window's composer. */
export function startCapture(target: string): Promise<void> {
  return invoke("start_capture", { target });
}

/** §9.3: surface the main window and select a conversation there. */
export function openInMain(conversationId: string): Promise<void> {
  return invoke("open_in_main", { conversationId });
}

/** Router log drawer (§9.2): recent decisions, newest first. */
export function listRoutingEvents(
  conversationId: string,
): Promise<RoutingEvent[]> {
  return invoke<RoutingEvent[]>("list_routing_events", { conversationId });
}

/** Follow-up chips (§3.7): returns and clears the cached set. */
export function takeSuggestions(conversationId: string): Promise<string[]> {
  return invoke<string[]>("take_suggestions", { conversationId });
}

/** §3.11 Triad report: aggregates over every routing event and message. */
export function triadReport(): Promise<TriadReport> {
  return invoke<TriadReport>("triad_report");
}

/** Clear every learned §3.11 threshold bump and its override counter. */
export function resetAdaptive(): Promise<void> {
  return invoke("reset_adaptive");
}

// -- Conversations --------------------------------------------------------

export function listConversations(): Promise<Conversation[]> {
  return invoke<Conversation[]>("list_conversations");
}

export function createConversation(): Promise<Conversation> {
  return invoke<Conversation>("create_conversation");
}

export function renameConversation(id: string, title: string): Promise<void> {
  return invoke("rename_conversation", { id, title });
}

export function deleteConversation(id: string): Promise<void> {
  return invoke("delete_conversation", { id });
}

export function setConversationModel(
  id: string,
  model: string | null,
): Promise<void> {
  return invoke("set_conversation_model", { id, model });
}

/** Bind 0–3 workspace roots (§6.3); the backend canonicalizes + validates. */
export function setConversationWorkspaces(
  id: string,
  roots: string[],
): Promise<void> {
  return invoke("set_conversation_workspaces", { id, roots });
}

export function getMessages(conversationId: string): Promise<Message[]> {
  return invoke<Message[]>("get_messages", { conversationId });
}

// -- Settings ---------------------------------------------------------------

export function getSetting(key: string): Promise<string | null> {
  return invoke<string | null>("get_setting", { key });
}

export function setSetting(key: string, value: string): Promise<void> {
  return invoke("set_setting", { key, value });
}

// -- Permission matrix (§6.6) ------------------------------------------------

export interface ApprovalReply {
  allow: boolean;
  /** "" (allow once) | "session" | "always" — only read when allow. */
  mode: string;
  /** Edit-in-place: replacement content for fs_write/fs_edit. */
  editedContent: string | null;
}

export interface ToolPermissionRow {
  tool: string;
  root: string;
  mode: string;
}

export function respondApproval(
  requestId: string,
  reply: ApprovalReply,
): Promise<void> {
  return invoke("respond_approval", { requestId, reply });
}

export function listToolPermissions(): Promise<ToolPermissionRow[]> {
  return invoke<ToolPermissionRow[]>("list_tool_permissions");
}

export function setToolPermission(
  tool: string,
  root: string,
  mode: string,
): Promise<void> {
  return invoke("set_tool_permission", { tool, root, mode });
}

export function clearSessionPermissions(): Promise<void> {
  return invoke("clear_session_permissions");
}

// -- Endpoint profiles (§5.1, M3) --------------------------------------------

export interface EndpointProfile {
  id: string;
  /** "ollama" | "openai" (OpenAI-compatible). */
  kind: string;
  name: string;
  baseUrl: string;
  apiKeyRef: string | null;
  headers: Record<string, string>;
  enabled: boolean;
  notes: string | null;
}

export interface EndpointTestResult {
  ok: boolean;
  latencyMs: number;
  modelCount: number;
  models: string[];
  error: string | null;
}

export function listEndpointProfiles(): Promise<EndpointProfile[]> {
  return invoke<EndpointProfile[]>("list_endpoint_profiles");
}

export function saveEndpointProfile(
  profile: EndpointProfile,
): Promise<EndpointProfile> {
  return invoke<EndpointProfile>("save_endpoint_profile", { profile });
}

export function deleteEndpointProfile(id: string): Promise<void> {
  return invoke("delete_endpoint_profile", { id });
}

export function setEndpointApiKey(id: string, secret: string): Promise<void> {
  return invoke("set_endpoint_api_key", { id, secret });
}

export function clearEndpointApiKey(id: string): Promise<void> {
  return invoke("clear_endpoint_api_key", { id });
}

export function testEndpoint(args: {
  kind: string;
  baseUrl: string;
  endpointId?: string | null;
}): Promise<EndpointTestResult> {
  return invoke<EndpointTestResult>("test_endpoint", { args });
}

// -- MCP servers (§6.5) -------------------------------------------------------

export interface McpServer {
  id: string;
  name: string;
  /** Full command line — spawned via `cmd /c` on Windows. */
  command: string;
  enabled: boolean;
}

export interface McpTestResult {
  ok: boolean;
  latencyMs: number;
  /** Bare tool names (not yet namespaced — the server name is the user's). */
  tools: string[];
  error: string | null;
}

export function listMcpServers(): Promise<McpServer[]> {
  return invoke<McpServer[]>("list_mcp_servers");
}

export function saveMcpServer(server: McpServer): Promise<McpServer> {
  return invoke<McpServer>("save_mcp_server", { server });
}

export function deleteMcpServer(id: string): Promise<void> {
  return invoke("delete_mcp_server", { id });
}

export function testMcpServer(command: string): Promise<McpTestResult> {
  return invoke<McpTestResult>("test_mcp_server", { command });
}

// -- OpenWebUI tools + Functions (§6.5b/§6.5d) --------------------------------

/** Synthetic endpoint behind Pipe pseudo-models (mirrors the backend
 * constant in `owui.rs`). */
export const PIPE_ENDPOINT = "ep_ternion_pipes";

export interface OwuiTool {
  id: string;
  name: string;
  /** Raw Python manifest source. */
  source: string;
  enabled: boolean;
  /** 'tools' (Skills — model-callable methods), 'filter' (inlet middleware)
   * or 'pipe' (a pseudo-model) — the OpenWebUI manifest kind (§6.5d). */
  kind: "tools" | "filter" | "pipe";
}

export interface OwuiTestResult {
  ok: boolean;
  latencyMs: number;
  /** Public method names of the parsed `class Tools`. */
  tools: string[];
  error: string | null;
}

export function listOwuiTools(): Promise<OwuiTool[]> {
  return invoke<OwuiTool[]>("list_owui_tools");
}

export function saveOwuiTool(tool: OwuiTool): Promise<OwuiTool> {
  return invoke<OwuiTool>("save_owui_tool", { tool });
}

export function deleteOwuiTool(id: string): Promise<void> {
  return invoke("delete_owui_tool", { id });
}

export function testOwuiTool(source: string): Promise<OwuiTestResult> {
  return invoke<OwuiTestResult>("test_owui_tool", { source });
}

// -- §5.6 provider hand-off ---------------------------------------------------

/** Backend event payload (`ternion://handoff-available`) when an endpoint
 * transitions from down to available. */
export interface HandoffOffer {
  endpointId: string;
  name: string;
  /** "ollama" | "openai" — non-local kinds show a cost note. */
  kind: string;
  local: boolean;
  latencyMs: number;
  modelCount: number;
  /** First model ids; the switch targets the first. */
  models: string[];
  /** Persisted policy: "ask" | "always" (auto-apply) | "never". */
  policy: string;
}

export function setHandoffPolicy(
  endpointId: string,
  policy: string,
): Promise<void> {
  return invoke("set_handoff_policy", { endpointId, policy });
}