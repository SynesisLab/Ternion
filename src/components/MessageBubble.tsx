import { memo } from "react";

import { formatLatency, formatTokens } from "../lib/format";
import { t } from "../i18n";
import type { Message } from "../types/chat";
import { Markdown } from "./Markdown";
import { ThinkingBlock } from "./ThinkingBlock";

function textOf(message: Message): string {
  return message.content
    .filter((p) => p.type === "text")
    .map((p) => p.text)
    .join("");
}

export const MessageBubble = memo(function MessageBubble({
  message,
}: {
  message: Message;
}) {
  if (message.role === "user") {
    return (
      <div className="flex justify-end">
        <div className="max-w-[80%] whitespace-pre-wrap rounded-2xl rounded-br-md border border-[color:var(--color-accent)]/25 bg-[color:var(--color-accent)]/15 px-4 py-2.5 text-sm leading-relaxed text-[color:var(--color-ink)]">
          {textOf(message)}
        </div>
      </div>
    );
  }

  const metaBits: string[] = [];
  if (message.modelId) metaBits.push(message.modelId);
  if (message.tokensOut != null)
    metaBits.push(`${formatTokens(message.tokensOut)} out`);
  if (message.latencyMs != null) metaBits.push(formatLatency(message.latencyMs));
  const showMeta = message.status !== "streaming" && metaBits.length > 0;

  return (
    <div className="max-w-[85%] space-y-2">
      {message.reasoning && (
        <ThinkingBlock
          text={message.reasoning}
          active={message.status === "streaming"}
        />
      )}
      <div className="prose prose-invert prose-sm max-w-none break-words text-[color:var(--color-ink)] prose-pre:bg-[#0d1017] prose-code:before:hidden prose-code:after:hidden">
        <Markdown text={textOf(message)} />
      </div>
      {(showMeta || message.status === "stopped" || message.status === "error") && (
        <div className="flex items-center gap-2 text-[11px] text-[color:var(--color-muted)]">
          {showMeta && <span>{metaBits.join(" · ")}</span>}
          {message.status === "stopped" && (
            <span className="rounded border border-[color:var(--color-warn)]/30 bg-[color:var(--color-warn)]/10 px-1.5 py-0.5 font-medium text-[color:var(--color-warn)]">
              {t("chat.meta.stopped")}
            </span>
          )}
          {message.status === "error" && (
            <span
              title={message.error ?? undefined}
              className="rounded border border-[color:var(--color-danger)]/30 bg-[color:var(--color-danger)]/10 px-1.5 py-0.5 font-medium text-[color:var(--color-danger)]"
            >
              {t("chat.meta.error")}
            </span>
          )}
        </div>
      )}
    </div>
  );
});