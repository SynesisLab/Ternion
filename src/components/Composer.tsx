import { useEffect, useRef, useState } from "react";

import { t } from "../i18n";

export function Composer({
  onSend,
  onStop,
  streaming,
  disabled,
  offline,
  suggestions = [],
  onSuggestion,
  escalation = false,
  onContinueTitan,
}: {
  onSend: (text: string) => void;
  onStop: () => void;
  streaming: boolean;
  /** No model available / not initialized. */
  disabled?: boolean;
  offline?: boolean;
  /** Follow-up chips from the Herald sidecar (§3.7). */
  suggestions?: string[];
  onSuggestion?: (text: string) => void;
  /** Scout ran past its output ceiling last turn (§3.6). */
  escalation?: boolean;
  onContinueTitan?: () => void;
}) {
  const [text, setText] = useState("");
  const taRef = useRef<HTMLTextAreaElement>(null);

  // Autosize up to ~200px.
  useEffect(() => {
    const ta = taRef.current;
    if (!ta) return;
    ta.style.height = "auto";
    ta.style.height = `${Math.min(ta.scrollHeight, 200)}px`;
  }, [text]);

  const canSend = !disabled && !streaming && text.trim().length > 0;

  const send = () => {
    if (!canSend) return;
    onSend(text);
    setText("");
    // Refocus after the bubble insert re-renders.
    requestAnimationFrame(() => taRef.current?.focus());
  };

  const onKeyDown = (e: React.KeyboardEvent<HTMLTextAreaElement>) => {
    if (e.key === "Enter" && !e.shiftKey && !e.nativeEvent.isComposing) {
      e.preventDefault();
      send();
    }
  };

  return (
    <div className="border-t border-[color:var(--color-edge)] bg-[color:var(--color-bg)] px-6 py-4">
      <div className="mx-auto max-w-3xl">
        {escalation && !streaming && onContinueTitan && (
          <div className="mb-2 flex items-center gap-3 rounded-xl border border-[color:var(--color-accent)]/30 bg-[color:var(--color-accent)]/10 px-3 py-2 text-xs text-[color:var(--color-ink)]">
            <span className="flex-1">{t("chat.escalation.offer")}</span>
            <button
              type="button"
              onClick={onContinueTitan}
              className="shrink-0 rounded-lg bg-[color:var(--color-accent)] px-3 py-1.5 font-medium text-white hover:opacity-90"
            >
              {t("chat.escalation.continue")}
            </button>
          </div>
        )}
        {!streaming && suggestions.length > 0 && onSuggestion && (
          <div className="mb-2 flex flex-wrap items-center gap-2">
            <span className="text-[11px] uppercase tracking-wide text-[color:var(--color-muted)]">
              {t("chat.suggestions.label")}
            </span>
            {suggestions.map((s) => (
              <button
                key={s}
                type="button"
                onClick={() => onSuggestion(s)}
                className="rounded-full border border-[color:var(--color-edge)] bg-[color:var(--color-panel)] px-3 py-1 text-xs text-[color:var(--color-ink)] transition-colors hover:border-[color:var(--color-accent)]"
              >
                {s}
              </button>
            ))}
          </div>
        )}
        <div className="flex items-end gap-2">
          <textarea
            ref={taRef}
            rows={1}
            value={text}
            onChange={(e) => setText(e.target.value)}
            onKeyDown={onKeyDown}
            placeholder={
              offline
                ? t("chat.input.placeholder.offline")
                : t("chat.input.placeholder")
            }
            disabled={disabled}
            className="max-h-[200px] flex-1 resize-none rounded-xl border border-[color:var(--color-edge)] bg-[color:var(--color-panel)] px-3.5 py-2.5 text-sm leading-relaxed text-[color:var(--color-ink)] placeholder:text-[color:var(--color-muted)] outline-none focus:border-[color:var(--color-accent)] disabled:opacity-50"
          />
          {streaming ? (
            <button
              type="button"
              onClick={onStop}
              className="shrink-0 rounded-xl border border-[color:var(--color-danger)]/40 bg-[color:var(--color-danger)]/10 px-4 py-2.5 text-sm font-medium text-[color:var(--color-danger)] hover:bg-[color:var(--color-danger)]/20"
            >
              {t("chat.input.stop")}
            </button>
          ) : (
            <button
              type="button"
              onClick={send}
              disabled={!canSend}
              className="shrink-0 rounded-xl bg-[color:var(--color-accent)] px-4 py-2.5 text-sm font-medium text-white hover:opacity-90 disabled:cursor-not-allowed disabled:opacity-40"
            >
              {t("chat.input.send")}
            </button>
          )}
        </div>
      </div>
    </div>
  );
}