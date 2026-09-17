import { memo } from "react";
import { convertFileSrc } from "@tauri-apps/api/core";

import { formatLatency, formatTokens } from "../lib/format";
import { t } from "../i18n";
import type { ContentPart, Message } from "../types/chat";
import { Markdown } from "./Markdown";
import { RoutingRibbon } from "./RoutingRibbon";
import { ThinkingBlock } from "./ThinkingBlock";
import { ToolActivity, toCallViews } from "./ToolActivity";

function textOf(message: Message): string {
  return message.content
    .filter((p) => p.type === "text")
    .map((p) => p.text)
    .join("");
}

/** One image part in a user bubble; falls back to a stub when the path
 * didn't hydrate (file moved, §7.2). */
function UserImage({ part }: { part: Extract<ContentPart, { type: "image" }> }) {
  if (!part.processedPath) {
    return (
      <div
        title={part.mime}
        className="flex h-20 w-28 items-center justify-center rounded-xl border border-[color:var(--color-edge)] bg-[color:var(--color-panel)] text-[color:var(--color-muted)]"
      >
        🖼
      </div>
    );
  }
  return (
    <img
      src={convertFileSrc(part.processedPath)}
      alt=""
      className="max-h-56 rounded-xl border border-[color:var(--color-accent)]/25 object-cover"
    />
  );
}

export const MessageBubble = memo(function MessageBubble({
  message,
  switched = false,
}: {
  message: Message;
  /** Precomputed by the list: this turn ran on a different model. */
  switched?: boolean;
}) {
  if (message.role === "user") {
    const images = message.content.filter((p) => p.type === "image");
    const text = textOf(message);
    return (
      <div className="flex justify-end">
        <div className="max-w-[80%] space-y-2">
          {images.length > 0 && (
            <div className="flex flex-wrap justify-end gap-1.5">
              {images.map((img) => (
                <UserImage
                  key={img.attachmentId}
                  part={img as Extract<ContentPart, { type: "image" }>}
                />
              ))}
            </div>
          )}
          {text && (
            <div className="whitespace-pre-wrap rounded-2xl rounded-br-md border border-[color:var(--color-accent)]/25 bg-[color:var(--color-accent)]/15 px-4 py-2.5 text-sm leading-relaxed text-[color:var(--color-ink)]">
              {text}
            </div>
          )}
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
      {message.routing && (
        <RoutingRibbon
          decision={message.routing.decision}
          finalTarget={message.routing.finalTarget}
          latencyMs={message.routing.latencyMs}
          switched={switched}
        />
      )}
      {message.reasoning && (
        <ThinkingBlock
          text={message.reasoning}
          active={message.status === "streaming"}
        />
      )}
      {message.toolCalls.length > 0 && (
        <ToolActivity calls={toCallViews(message.toolCalls)} />
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