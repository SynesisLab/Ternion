/** IPC DTOs — must mirror the serde structs in src-tauri/src/types.rs. */

export type ConnectionStatus = "ok" | "down" | "unknown";

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
  /** M0: null → default model, else the explicit model id chosen per chat. */
  pinnedModel: string | null;
  workspaceRoots: string[];
  systemPrompt: string | null;
  archived: boolean;
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
}

export interface ModelInfo {
  id: string;
  displayName: string;
  sizeBytes: number | null;
  parameterSize: string | null;
  quantizationLevel: string | null;
  family: string | null;
  contextLength: number | null;
  /** e.g. ["completion", "vision", "tools", "thinking"] */
  capabilities: string[];
}