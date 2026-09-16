import { useEffect, useRef, useState } from "react";

import { t } from "../i18n";

export function Composer({
  onSend,
  onStop,
  streaming,
  disabled,
  offline,
}: {
  onSend: (text: string) => void;
  onStop: () => void;
  streaming: boolean;
  /** No model available / not initialized. */
  disabled?: boolean;
  offline?: boolean;
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
      <div className="mx-auto flex max-w-3xl items-end gap-2">
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
  );
}