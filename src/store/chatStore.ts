/**
 * Chat state (zustand). The DB is the source of truth: messages are refetched
 * after every stream resolves; token deltas live only in ephemeral `drafts`,
 * coalesced at ~30 ms so a fast stream doesn't re-render per token.
 */

import { create } from "zustand";

import {
  createConversation,
  deleteConversation,
  getMessages,
  listConversations,
  listModels,
  sendChat,
  setConversationModel,
  stopChat,
} from "../lib/ipc";
import { newId } from "../lib/uuid";
import type {
  ConnectionStatus,
  Conversation,
  Message,
  ModelInfo,
} from "../types/chat";
import type { StatusPhase, StreamEvent } from "../types/stream";

/** One stream per conversation in M0; the draft is what the UI renders live. */
export interface StreamDraft {
  phase: StatusPhase | null;
  text: string;
  reasoning: string;
  usage: { tokensIn: number; tokensOut: number; latencyMs: number } | null;
  error: { code: string; message: string; retryable: boolean } | null;
}

function emptyDraft(): StreamDraft {
  return { phase: null, text: "", reasoning: "", usage: null, error: null };
}

/** Draft flush cadence (design: ~30 ms — 2 frames at 60 Hz). */
const FLUSH_MS = 30;

interface ChatStore {
  // data
  conversations: Conversation[];
  activeId: string | null;
  /** Lazily loaded per conversation; replaced wholesale after each stream. */
  messagesByConv: Record<string, Message[]>;
  models: ModelInfo[];
  connection: ConnectionStatus;
  /** Active model id (M0: explicit per conversation). */
  model: string;

  // streaming
  drafts: Record<string, StreamDraft>;
  streaming: Record<string, boolean>;

  initError: string | null;

  // actions
  init: () => Promise<void>;
  refreshModels: () => Promise<void>;
  selectConversation: (id: string) => Promise<void>;
  newConversation: () => Promise<void>;
  deleteConversation: (id: string) => Promise<void>;
  setModel: (modelId: string) => void;
  sendMessage: (text: string) => Promise<void>;
  stop: () => Promise<void>;
  applyStreamEvent: (conversationId: string, ev: StreamEvent) => void;
  refreshMessages: (conversationId: string) => Promise<void>;
}

export const useChatStore = create<ChatStore>()((set, get) => ({
  conversations: [],
  activeId: null,
  messagesByConv: {},
  models: [],
  connection: "unknown",
  model: "",

  drafts: {},
  streaming: {},

  initError: null,

  async init() {
    try {
      const conversations = await listConversations();
      let next = conversations;
      let activeId = conversations[0]?.id ?? null;
      if (!activeId) {
        const created = await createConversation();
        next = [created, ...conversations];
        activeId = created.id;
      }
      const messages = await getMessages(activeId);
      try {
        const models = await listModels();
        set((s) => ({
          conversations: next,
          activeId,
          messagesByConv: { [activeId as string]: messages },
          models,
          connection: "ok",
          model:
            next.find((c) => c.id === activeId)?.pinnedModel ||
            s.model ||
            models[0]?.id ||
            "",
        }));
      } catch {
        // Reachable core, unreachable endpoint — chat stays disabled.
        set({
          conversations: next,
          activeId,
          messagesByConv: { [activeId as string]: messages },
          connection: "down",
        });
      }
    } catch (e) {
      set({ initError: String(e) });
    }
  },

  async refreshModels() {
    set({ connection: "unknown" });
    try {
      const models = await listModels();
      set((s) => ({
        models,
        connection: "ok",
        model: s.model || models[0]?.id || "",
      }));
    } catch {
      set({ connection: "down" });
    }
  },

  async selectConversation(id) {
    if (get().activeId === id) return;
    set({ activeId: id });
    if (!get().messagesByConv[id]) {
      const messages = await getMessages(id).catch(() => []);
      set((s) => ({
        messagesByConv: { ...s.messagesByConv, [id]: messages },
      }));
    }
    const conv = get().conversations.find((c) => c.id === id);
    if (conv?.pinnedModel) set({ model: conv.pinnedModel });
  },

  async newConversation() {
    try {
      const created = await createConversation();
      set((s) => ({
        conversations: [created, ...s.conversations],
        activeId: created.id,
        messagesByConv: { ...s.messagesByConv, [created.id]: [] },
      }));
    } catch (e) {
      set({ initError: String(e) });
    }
  },

  async deleteConversation(id) {
    try {
      await deleteConversation(id);
    } catch (e) {
      set({ initError: String(e) });
      return;
    }
    const remaining = get().conversations.filter((c) => c.id !== id);
    const wasActive = get().activeId === id;
    const nextMessages = { ...get().messagesByConv };
    delete nextMessages[id];
    set({ conversations: remaining, messagesByConv: nextMessages });
    if (wasActive) {
      if (remaining[0]) {
        await get().selectConversation(remaining[0].id);
      } else {
        const created = await createConversation().catch(() => null);
        if (created) {
          set((s) => ({
            conversations: [created],
            activeId: created.id,
            messagesByConv: { ...s.messagesByConv, [created.id]: [] },
          }));
        } else {
          set({ activeId: null });
        }
      }
    }
  },

  setModel(modelId) {
    set({ model: modelId });
    const { activeId } = get();
    if (activeId) {
      setConversationModel(activeId, modelId).catch(() => {});
    }
  },

  async sendMessage(text) {
    const trimmed = text.trim();
    const { activeId, model, streaming } = get();
    if (!activeId || !trimmed || !model || streaming[activeId]) return;

    const convId = activeId;
    const userMessageId = newId();
    const now = Date.now();
    const userMessage: Message = {
      id: userMessageId,
      conversationId: convId,
      role: "user",
      content: [{ type: "text", text: trimmed }],
      modelRole: null,
      modelId: null,
      endpointId: null,
      tokensIn: null,
      tokensOut: null,
      latencyMs: null,
      reasoning: null,
      status: "complete",
      error: null,
      createdAt: now,
    };
    const placeholder: Message = {
      ...userMessage,
      id: "streaming",
      role: "assistant",
      content: [],
      status: "streaming",
      createdAt: now + 1,
    };

    set((s) => ({
      messagesByConv: {
        ...s.messagesByConv,
        [convId]: [...(s.messagesByConv[convId] ?? []), userMessage, placeholder],
      },
      drafts: { ...s.drafts, [convId]: emptyDraft() },
      streaming: { ...s.streaming, [convId]: true },
    }));

    try {
      // Resolves when the stream ends; deltas arrive through the Channel.
      await sendChat(
        { conversationId: convId, userMessageId, content: trimmed, model },
        (ev) => get().applyStreamEvent(convId, ev),
      );
    } catch (e) {
      // IPC-level failure (e.g. `stream_active`, provider missing). Rows the
      // command did write come back in the refetch; patch live stubs to error.
      const message = String(e);
      await get().refreshMessages(convId);
      set((s) => ({
        messagesByConv: {
          ...s.messagesByConv,
          [convId]: (s.messagesByConv[convId] ?? []).map((m) =>
            m.status === "streaming"
              ? { ...m, status: "error" as const, error: message }
              : m,
          ),
        },
        drafts: withoutDraft(s.drafts, convId),
        streaming: withoutStream(s.streaming, convId),
      }));
      return;
    }

    set((s) => ({
      messagesByConv: { ...s.messagesByConv, [convId]: [] },
      drafts: withoutDraft(s.drafts, convId),
      streaming: withoutStream(s.streaming, convId),
    }));
    await get().refreshMessages(convId);

    // Auto-title / updated_at changed ordering — refresh the sidebar data.
    try {
      const conversations = await listConversations();
      set({ conversations });
    } catch {
      /* non-fatal */
    }
  },

  async stop() {
    const { activeId, streaming } = get();
    if (!activeId || !streaming[activeId]) return;
    try {
      await stopChat(activeId);
    } catch {
      /* already gone */
    }
    // The stream resolves with status "stopped"; the finalize path refetches.
  },

  applyStreamEvent(conversationId, ev) {
    switch (ev.type) {
      case "text_delta":
      case "reasoning_delta": {
        const p = pending.get(conversationId) ?? { text: "", reasoning: "" };
        if (ev.type === "text_delta") p.text += ev.text;
        else p.reasoning += ev.text;
        pending.set(conversationId, p);
        scheduleFlush(set);
        break;
      }
      case "status": {
        set((s) => ({
          drafts: {
            ...s.drafts,
            [conversationId]: {
              ...(s.drafts[conversationId] ?? emptyDraft()),
              phase: ev.phase,
            },
          },
        }));
        break;
      }
      case "usage": {
        set((s) => ({
          drafts: {
            ...s.drafts,
            [conversationId]: {
              ...(s.drafts[conversationId] ?? emptyDraft()),
              usage: {
                tokensIn: ev.tokensIn,
                tokensOut: ev.tokensOut,
                latencyMs: ev.latencyMs,
              },
            },
          },
        }));
        break;
      }
      case "error": {
        // Stop batching; the final state is authoritative.
        pending.delete(conversationId);
        set((s) => ({
          drafts: {
            ...s.drafts,
            [conversationId]: {
              ...(s.drafts[conversationId] ?? emptyDraft()),
              error: {
                code: ev.code,
                message: ev.message,
                retryable: ev.retryable,
              },
            },
          },
        }));
        break;
      }
      case "done":
        // Invoke resolves right after; the finalize path refetches from DB.
        break;
      default:
        // routing (M1) / tool_call_* (M2) — intentionally ignored in M0.
        break;
    }
  },

  async refreshMessages(conversationId) {
    const messages = await getMessages(conversationId).catch(() => null);
    if (messages) {
      set((s) => ({
        messagesByConv: { ...s.messagesByConv, [conversationId]: messages },
      }));
    }
  },
}));

// ---------------------------------------------------------------------------
// Delta coalescing — one store update per FLUSH_MS per conversation.
// ---------------------------------------------------------------------------

const pending = new Map<string, { text: string; reasoning: string }>();
let flushHandle: ReturnType<typeof setTimeout> | null = null;

function scheduleFlush(set: (fn: (s: ChatStore) => Partial<ChatStore>) => void) {
  if (flushHandle !== null) return;
  flushHandle = setTimeout(() => {
    flushHandle = null;
    const buffered = [...pending.entries()];
    if (buffered.length === 0) return;
    pending.clear();
    set((s) => {
      const drafts = { ...s.drafts };
      for (const [convId, p] of buffered) {
        const d = drafts[convId] ?? emptyDraft();
        drafts[convId] = {
          ...d,
          text: d.text + p.text,
          reasoning: d.reasoning + p.reasoning,
        };
      }
      return { drafts };
    });
  }, FLUSH_MS);
}

function withoutDraft(
  drafts: Record<string, StreamDraft>,
  id: string,
): Record<string, StreamDraft> {
  const next = { ...drafts };
  delete next[id];
  return next;
}

function withoutStream(
  streaming: Record<string, boolean>,
  id: string,
): Record<string, boolean> {
  const next = { ...streaming };
  delete next[id];
  return next;
}