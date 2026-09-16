import { useAutoScroll } from "../hooks/useAutoScroll";
import { t } from "../i18n";
import { useChatStore, type StreamDraft } from "../store/chatStore";
import { Markdown } from "./Markdown";
import { MessageBubble } from "./MessageBubble";
import { RoutingRibbon } from "./RoutingRibbon";
import { ThinkingBlock } from "./ThinkingBlock";

const PHASE_LABELS = {
  routing: t("chat.status.routing"),
  connecting: t("chat.status.connecting"),
  loading_model: t("chat.status.loading_model"),
} as const;

/** Live assistant panel driven by the stream draft (updates ~30 ms). */
function LiveBubble({ draft }: { draft: StreamDraft }) {
  const blank = draft.text.length === 0 && draft.reasoning.length === 0;
  return (
    <div className="max-w-[85%] space-y-2">
      {draft.routing && (
        <RoutingRibbon
          decision={draft.routing.decision}
          finalTarget={draft.routing.finalTarget}
        />
      )}
      {draft.reasoning && <ThinkingBlock text={draft.reasoning} active />}
      {draft.text.length > 0 ? (
        <div className="prose prose-invert prose-sm max-w-none break-words text-[color:var(--color-ink)] prose-pre:bg-[#0d1017] prose-code:before:hidden prose-code:after:hidden">
          <Markdown text={draft.text} />
        </div>
      ) : (
        blank && (
          <div className="flex items-center gap-2 text-xs text-[color:var(--color-muted)]">
            <span className="h-2 w-2 animate-pulse rounded-full bg-[color:var(--color-accent)]" />
            {draft.error
              ? `${draft.error.code}: ${draft.error.message}`
              : draft.phase
                ? PHASE_LABELS[draft.phase]
                : t("chat.status.connecting")}
          </div>
        )
      )}
      {draft.text.length > 0 && draft.error && (
        <div className="rounded-lg border border-[color:var(--color-danger)]/30 bg-[color:var(--color-danger)]/10 px-3 py-2 text-xs text-[color:var(--color-danger)]">
          {draft.error.code}: {draft.error.message}
        </div>
      )}
    </div>
  );
}

export function MessageList({ conversationId }: { conversationId: string }) {
  const messages = useChatStore((s) => s.messagesByConv[conversationId]);
  const draft = useChatStore((s) => s.drafts[conversationId]);
  const streaming = useChatStore((s) => s.streaming[conversationId] ?? false);

  const scrollKey = `${messages?.length ?? 0}:${draft?.text.length ?? 0}:${draft?.reasoning.length ?? 0}`;
  const ref = useAutoScroll<HTMLDivElement>(scrollKey);

  if (!messages) return null;

  return (
    <div ref={ref} className="flex-1 overflow-y-auto">
      <div className="mx-auto flex max-w-3xl flex-col gap-5 px-6 py-6">
        {messages.length === 0 && !streaming && <EmptyState />}
        {messages.map((m, i) => {
          // ⚡ switch marker (§9.1): the previous assistant turn ran on a
          // different model than this one.
          const prevAssistant = [...messages.slice(0, i)]
            .reverse()
            .find((p) => p.role === "assistant");
          const switched =
            m.role === "assistant" &&
            m.routing != null &&
            prevAssistant?.modelId != null &&
            m.modelId != null &&
            prevAssistant.modelId !== m.modelId;
          return (
            <MessageBubble
              key={m.id === "streaming" ? `stub-${m.createdAt}` : m.id}
              message={m}
              switched={switched}
            />
          );
        })}
        {streaming && draft && <LiveBubble draft={draft} />}
      </div>
    </div>
  );
}

function EmptyState() {
  return (
    <div className="flex h-full min-h-[50vh] flex-col items-center justify-center text-center">
      <div className="mb-3 flex items-center gap-1.5" aria-hidden>
        <span className="h-6 w-6 rounded-full bg-[color:var(--color-accent)]/80" />
        <span className="h-6 w-6 rounded-full bg-[color:var(--color-accent-2)]/70" />
        <span className="h-6 w-6 rounded-full bg-[color:var(--color-muted)]/50" />
      </div>
      <div className="text-lg font-semibold tracking-tight">
        {t("chat.empty.title")}
      </div>
      <div className="mt-1 max-w-md text-sm text-[color:var(--color-muted)]">
        {t("chat.empty.subtitle")}
      </div>
    </div>
  );
}