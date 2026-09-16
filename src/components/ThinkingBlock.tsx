import { useEffect, useState } from "react";

import { t } from "../i18n";

/** Collapsible reasoning block. Open while streaming, folded after. */
export function ThinkingBlock({
  text,
  active,
}: {
  text: string;
  active: boolean;
}) {
  const [open, setOpen] = useState(active);

  useEffect(() => {
    if (active) setOpen(true);
  }, [active]);

  if (!text) return null;

  return (
    <div className="rounded-lg border border-[color:var(--color-edge)] bg-[color:var(--color-panel)]/60">
      <button
        type="button"
        onClick={() => setOpen((o) => !o)}
        className="flex w-full items-center gap-2 px-3 py-1.5 text-left text-xs font-medium text-[color:var(--color-muted)] hover:text-[color:var(--color-ink)]"
      >
        <span
          className={`inline-block transition-transform ${open ? "rotate-90" : ""}`}
        >
          ▸
        </span>
        {active ? t("chat.thinking.active") : t("chat.thinking.done")}
      </button>
      {open && (
        <div className="max-h-60 overflow-y-auto whitespace-pre-wrap border-t border-[color:var(--color-edge)] px-3 py-2 text-xs leading-relaxed text-[color:var(--color-muted)]">
          {text}
        </div>
      )}
    </div>
  );
}