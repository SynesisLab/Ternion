import { invoke } from "@tauri-apps/api/core";

import type { Conversation, Message, ModelInfo } from "../types/chat";

/**
 * Typed wrappers around Tauri IPC commands. Every command the Rust core
 * exposes gets one function here; the frontend never calls `invoke`
 * directly elsewhere. (Streaming chat wrappers live with the store —
 * they carry a Channel.)
 */

export async function ping(): Promise<string> {
  return invoke<string>("ping");
}

// -- Models ---------------------------------------------------------------

export async function listModels(): Promise<ModelInfo[]> {
  return invoke<ModelInfo[]>("list_models");
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