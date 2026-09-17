/**
 * Chat state (zustand). The DB is the source of truth: messages are refetched
 * after every stream resolves; token deltas live only in ephemeral `drafts`,
 * coalesced at ~30 ms so a fast stream doesn't re-render per token.
 *
 * M1: the per-chat selection is a Triad *pin* — 'auto' (router), 'scout',
 * 'titan', or an explicit model id — persisted on the conversation row. The
 * router's decision arrives as a `routing` stream event and is shown live in
 * the composer-adjacent ribbon and, once persisted, on the message itself.
 */

import { create } from "zustand";

import {
  createConversation,
  deleteConversation,
  getMessages,
  listConversations,
  listModels,
  renameConversation as apiRenameConversation,
  sendChat,
  setConversationWorkspaces,
  setConversationModel,
  setHandoffPolicy,
  stopChat,
  takeSuggestions,
  respondApproval as apiRespondApproval,
  type ApprovalReply,
  type HandoffOffer,
} from "../lib/ipc";
import { t } from "../i18n";
import { newId } from "../lib/uuid";
import type {
  ApprovalRequestView,
  Attachment,
  ConnectionStatus,
  Conversation,
  Message,
  ModelInfo,
  ToolCallView,
} from "../types/chat";
import type {
  RoutingDecision,
  StatusPhase,
  StreamEvent,
  Target,
} from "../types/stream";

/** One stream per conversation in M0; the draft is what the UI renders live. */
export interface StreamDraft {
  phase: StatusPhase | null;
  /** Router decision for this turn (§3.2), shown while it routes/streams. */
  routing: { decision: RoutingDecision; finalTarget: Target } | null;
  text: string;
  reasoning: string;
  /** Live tool activity (§6.1) — replaced by persisted rows after refetch. */
  tools: ToolCallView[];
  usage: { tokensIn: number; tokensOut: number; latencyMs: number } | null;
  error: { code: string; message: string; retryable: boolean } | null;
}

function emptyDraft(): StreamDraft {
  return {
    phase: null,
    routing: null,
    text: "",
    reasoning: "",
    tools: [],
    usage: null,
    error: null,
  };
}

/** Draft flush cadence (design: ~30 ms — 2 frames at 60 Hz). */
const FLUSH_MS = 30;

/** Pin values that are not explicit model ids. */
export function isRolePin(pin: string): boolean {
  return pin === "auto" || pin === "scout" || pin === "titan";
}

/** Apply a §5.6 hand-off: pin the current chat to the offered endpoint's
 * first model — a qualified ref, so the next turn routes there and the
 * §3.6 hand-off machinery (digest + recent turns) carries the thread. */
function applyHandoff(offer: HandoffOffer) {
  const model = offer.models[0];
  if (!model) return;
  useChatStore.getState().setModel(`${model}@${offer.endpointId}`);
}

interface ChatStore {
  // data
  conversations: Conversation[];
  activeId: string | null;
  /** Lazily loaded per conversation; replaced wholesale after each stream. */
  messagesByConv: Record<string, Message[]>;
  models: ModelInfo[];
  connection: ConnectionStatus;
  /**
   * The active conversation's Triad pin: 'auto' | 'scout' | 'titan' | model id.
   */
  pin: string;
  /** Last explicitly chosen model — the M0 fallback when the router is off. */
  model: string;

  // streaming
  drafts: Record<string, StreamDraft>;
  streaming: Record<string, boolean>;
  /** Scout ran past its output ceiling last turn (§3.6) — offer ⚡ continue. */
  escalation: Record<string, boolean>;
  /** Follow-up chips from the Herald sidecar (§3.7), per conversation. */
  suggestions: Record<string, string[]>;
  /** §7.6 vision gap on the last routed turn (suggested model, "" = none
   * known); null once cleared. */
  visionGap: Record<string, string | null>;
  /** Mutating tools awaiting a decision (§6.6), oldest first. */
  approvals: ApprovalRequestView[];
  /** §5.6 pending provider hand-off offer (null once resolved). */
  handoff: HandoffOffer | null;

  initError: string | null;

  // actions
  init: () => Promise<void>;
  refreshModels: () => Promise<void>;
  selectConversation: (id: string) => Promise<void>;
  newConversation: () => Promise<void>;
  renameConversation: (id: string, title: string) => Promise<void>;
  deleteConversation: (id: string) => Promise<void>;
  /** Bind one more workspace root (§6.3, max 3 — backend validates). */
  bindWorkspace: (id: string, path: string) => Promise<boolean>;
  /** Unbind a workspace root from the conversation. */
  unbindWorkspace: (id: string, path: string) => void;
  /** Pin change: 'auto' | 'scout' | 'titan' | explicit model id. */
  setPin: (value: string) => void;
  /** Explicit model fallback (M0-style picker selection). */
  setModel: (modelId: string) => void;
  sendMessage: (text: string, attachments?: Attachment[]) => Promise<void>;
  /** Resolve a pending approval (§6.6): allow/deny, scope, optional edit. */
  respondApproval: (requestId: string, reply: ApprovalReply) => Promise<void>;
  /** §5.6: a provider came online — surface (or auto-apply) the offer. */
  offerHandoff: (offer: HandoffOffer) => void;
  /** Accept the pending offer: pin the current chat to the new endpoint. */
  acceptHandoff: () => void;
  dismissHandoff: () => void;
  /** "Never for this endpoint" (§5.6): persist the policy + dismiss. */
  neverHandoff: () => void;
  /** ⚡ Continue with Titan (§3.6): pins titan, asks for the rest of the answer. */
  continueWithTitan: () => Promise<void>;
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
  pin: "auto",
  model: "",

  drafts: {},
  streaming: {},
  escalation: {},
  suggestions: {},
  visionGap: {},
  approvals: [],
  handoff: null,

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
        set((s) => {
          const active = next.find((c) => c.id === activeId);
          const pinned = active?.pinnedModel;
          return {
            conversations: next,
            activeId,
            messagesByConv: { [activeId as string]: messages },
            models,
            connection: "ok",
            pin: pinned ?? "auto",
            model:
              (pinned && !isRolePin(pinned) ? pinned : null) ||
              s.model ||
              models[0]?.id ||
              "",
          };
        });
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
    const pinned = conv?.pinnedModel;
    set({
      pin: pinned ?? "auto",
      ...(pinned && !isRolePin(pinned) ? { model: pinned } : {}),
    });
    // Clear per-chat overlays from the previous conversation.
    set({ escalation: {}, suggestions: {}, visionGap: {} });
  },

  async newConversation() {
    try {
      const created = await createConversation();
      set((s) => ({
        conversations: [created, ...s.conversations],
        activeId: created.id,
        messagesByConv: { ...s.messagesByConv, [created.id]: [] },
        pin: "auto",
        escalation: {},
        suggestions: {},
      }));
    } catch (e) {
      set({ initError: String(e) });
    }
  },

  async renameConversation(id, title) {
    const trimmed = title.trim();
    if (!trimmed) return;
    try {
      await apiRenameConversation(id, trimmed);
      set((s) => ({
        conversations: s.conversations.map((c) =>
          c.id === id ? { ...c, title: trimmed } : c,
        ),
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

  async bindWorkspace(id, path) {
    const trimmed = path.trim();
    if (!trimmed) return false;
    const roots = [
      ...rootsOf(get(), id),
      ...(rootsOf(get(), id).includes(trimmed) ? [] : [trimmed]),
    ];
    if (roots.length > 3) {
      set({ initError: t("workspace.maxThree") });
      return false;
    }
    try {
      // The backend canonicalizes + validates (§6.3).
      await setConversationWorkspaces(id, roots);
      const conversations = await listConversations();
      set({ conversations, initError: null });
      return true;
    } catch (e) {
      set({ initError: String(e) });
      return false;
    }
  },

  unbindWorkspace(id, path) {
    const roots = rootsOf(get(), id).filter((r) => r !== path);
    set((s) => ({
      conversations: s.conversations.map((c) =>
        c.id === id ? { ...c, workspaceRoots: roots } : c,
      ),
    }));
    setConversationWorkspaces(id, roots).catch((e) =>
      set({ initError: String(e) }),
    );
  },

  setPin(value) {
    set({ pin: value });
    if (!isRolePin(value)) set({ model: value });
    const { activeId } = get();
    if (activeId) {
      setConversationModel(activeId, value).catch(() => {});
    }
  },

  setModel(modelId) {
    set({ model: modelId, pin: modelId });
    const { activeId } = get();
    if (activeId) {
      setConversationModel(activeId, modelId).catch(() => {});
    }
  },

  async sendMessage(text, attachments = []) {
    const trimmed = text.trim();
    const { activeId, pin, model, models, streaming } = get();
    if (!activeId || (!trimmed && attachments.length === 0) || streaming[activeId])
      return;

    // Explicit pin wins; otherwise the last explicit model is the fallback
    // the router needs when the Triad is off (M0 behavior).
    const fallback = (!isRolePin(pin) && pin) || model || models[0]?.id || "";
    if (!fallback) return;

    const convId = activeId;
    const userMessageId = newId();
    const now = Date.now();
    const userMessage: Message = {
      id: userMessageId,
      conversationId: convId,
      role: "user",
      content: [
        { type: "text", text: trimmed },
        ...attachments.map((a) => ({
          type: "image" as const,
          attachmentId: a.id,
          mime: a.mime,
          processedPath: a.processedPath,
        })),
      ],
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
      routing: null,
      toolCalls: [],
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
      // One-shot overlays are void the moment a new exchange starts.
      escalation: { ...s.escalation, [convId]: false },
      suggestions: { ...s.suggestions, [convId]: [] },
      visionGap: { ...s.visionGap, [convId]: null },
    }));

    try {
      // Resolves when the stream ends; deltas arrive through the Channel.
      const result = await sendChat(
        {
          conversationId: convId,
          userMessageId,
          content: trimmed,
          model: fallback,
          attachmentIds: attachments.map((a) => a.id),
        },
        (ev) => get().applyStreamEvent(convId, ev),
      );
      // §3.6: Scout ran past its ceiling — offer the Titan continuation
      // until the next exchange starts.
      if (result.escalationAvailable) {
        set((s) => ({ escalation: { ...s.escalation, [convId]: true } }));
      }
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
      drafts: withoutDraft(s.drafts, convId),
      streaming: withoutStream(s.streaming, convId),
    }));
    await get().refreshMessages(convId);

    // Sidecars: follow-up chips land out-of-band (§3.7), a second or two
    // after the stream — poll a few times before giving up (empty is also a
    // valid state: chips can be off or Herald may have failed). A newer send
    // on the same conversation owns the overlay, so older polls stop short.
    for (let attempt = 0; attempt < 3; attempt++) {
      const suggestions = await takeSuggestions(convId).catch(() => [] as string[]);
      if (get().streaming[convId]) return; // a newer send owns the cache now
      set((s) => ({
        suggestions: { ...s.suggestions, [convId]: suggestions },
      }));
      if (suggestions.length > 0) break;
      await new Promise((r) => setTimeout(r, 1000));
    }

    // Auto-title / updated_at changed ordering — refresh the sidebar data.
    try {
      const conversations = await listConversations();
      set({ conversations });
    } catch {
      /* non-fatal */
    }
  },

  async continueWithTitan() {
    const { activeId, escalation, streaming } = get();
    if (!activeId || !escalation[activeId] || streaming[activeId]) return;
    // Escalation is a user-visible pin change (§3.10) — the chip reflects it,
    // and switching back to Auto is one click.
    get().setPin("titan");
    await get().sendMessage(t("chat.escalation.continuePrompt"));
  },

  async respondApproval(requestId, reply) {
    await apiRespondApproval(requestId, reply);
    set((s) => ({ approvals: s.approvals.filter((a) => a.id !== requestId) }));
  },

  offerHandoff(offer) {
    // §5.6: "always" auto-applies — but never mid-stream; while streaming
    // the offer defers to the user (routing stays observable).
    const { activeId, streaming } = get();
    if (
      offer.policy === "always" &&
      !(activeId && streaming[activeId])
    ) {
      applyHandoff(offer);
      return;
    }
    set({ handoff: offer });
  },

  acceptHandoff() {
    const { handoff } = get();
    if (!handoff) return;
    applyHandoff(handoff);
    set({ handoff: null });
  },

  dismissHandoff() {
    set({ handoff: null });
  },

  neverHandoff() {
    const { handoff } = get();
    if (handoff) {
      setHandoffPolicy(handoff.endpointId, "never").catch(() => {});
    }
    set({ handoff: null });
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
      case "routing": {
        set((s) => ({
          drafts: {
            ...s.drafts,
            [conversationId]: {
              ...(s.drafts[conversationId] ?? emptyDraft()),
              routing: { decision: ev.decision, finalTarget: ev.finalTarget },
            },
          },
          // §7.6: images this turn but the routed model can't see them —
          // surface the warning chip (one-shot, cleared by the next send).
          visionGap: {
            ...s.visionGap,
            [conversationId]: ev.decision.visionGap ?? null,
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
      case "tool_call_start": {
        set((s) => {
          const d = s.drafts[conversationId] ?? emptyDraft();
          return {
            drafts: {
              ...s.drafts,
              [conversationId]: {
                ...d,
                tools: [
                  ...d.tools,
                  {
                    callId: ev.id,
                    name: ev.name,
                    args: "",
                    result: null,
                    status: "running" as const,
                  },
                ],
              },
            },
          };
        });
        break;
      }
      case "tool_call_delta": {
        set((s) => {
          const d = s.drafts[conversationId] ?? emptyDraft();
          // `index` is the wire position; starts arrive in that order, so
          // positional indexing is exact for the bundled providers.
          const tools = d.tools.slice();
          const entry = tools[ev.index];
          if (entry) {
            tools[ev.index] = { ...entry, args: entry.args + ev.argsDelta };
          }
          return {
            drafts: { ...s.drafts, [conversationId]: { ...d, tools } },
          };
        });
        break;
      }
      case "tool_result": {
        set((s) => {
          const d = s.drafts[conversationId] ?? emptyDraft();
          return {
            drafts: {
              ...s.drafts,
              [conversationId]: {
                ...d,
                tools: d.tools.map((c) =>
                  c.callId === ev.callId
                    ? {
                        ...c,
                        result: ev.content,
                        status: ev.isError ? ("error" as const) : ("ok" as const),
                      }
                    : c,
                ),
              },
            },
          };
        });
        break;
      }
      case "approval_request": {
        // §6.6: the stream pauses (never cancelled) while the modal is open.
        set((s) => ({
          approvals: [
            ...s.approvals,
            {
              id: ev.id,
              callId: ev.callId,
              conversationId,
              tool: ev.tool,
              path: ev.path,
              secondaryPath: ev.secondaryPath,
              summary: ev.summary,
              diff: ev.diff,
              resultContent: ev.resultContent,
            },
          ],
        }));
        break;
      }
      case "done":
        // Invoke resolves right after; the finalize path refetches from DB.
        break;
      default:
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

/** Bound workspace roots of a conversation (empty = no FS access, §6.3). */
function rootsOf(
  state: { conversations: Conversation[] },
  id: string,
): string[] {
  return state.conversations.find((c) => c.id === id)?.workspaceRoots ?? [];
}

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