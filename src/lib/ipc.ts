import { Channel, invoke } from "@tauri-apps/api/core";

import type {
  ChatSendResult,
  Conversation,
  Message,
  ModelInfo,
  RoutingEvent,
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

// -- Chat streaming ---------------------------------------------------------

export interface ChatSendArgs {
  conversationId: string;
  /** Client-generated id; makes retries idempotent. */
  userMessageId: string;
  content: string;
  model: string;
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