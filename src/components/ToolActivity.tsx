import { useState } from "react";

import { t } from "../i18n";
import type { ToolCallRow, ToolCallView } from "../types/chat";

const STATUS_ICON: Record<ToolCallRow["status"], string> = {
  running: "◌",
  ok: "✓",
  error: "✗",
  denied: "⊘",
};

const STATUS_COLOR: Record<ToolCallRow["status"], string> = {
  running: "text-[color:var(--color-accent)]",
  ok: "text-[color:var(--color-accent-2)]",
  error: "text-[color:var(--color-danger)]",
  denied: "text-[color:var(--color-warn)]",
};

/** Map persisted rows onto the unified view shape. */
export function toCallViews(rows: ToolCallRow[]): ToolCallView[] {
  return rows.map((r) => ({
    callId: r.id,
    name: r.tool,
    args: r.args ?? "",
    result: r.result,
    status: r.status,
  }));
}

/**
 * Collapsible list of one assistant turn's tool calls (§6.1) — the same
 * component serves the live draft and the persisted message. Collapsed by
 * default once the turn is over; open while anything is still running.
 */
export function ToolActivity({
  calls,
  open: openProp,
}: {
  calls: ToolCallView[];
  /** Pin the open state (live render) — omit to make it user-toggled. */
  open?: boolean;
}) {
  const [openState, setOpen] = useState(false);
  const open = openProp ?? openState;

  if (calls.length === 0) return null;
  const running = calls.some((c) => c.status === "running");
  const failed = calls.filter((c) => c.status === "error").length;

  return (
    <div className="overflow-hidden rounded-lg border border-[color:var(--color-muted)]/20 bg-[color:var(--color-panel)]/60 text-xs">
      <button
        onClick={() => setOpen(!open)}
        className="flex w-full items-center gap-2 px-3 py-1.5 text-[11px] font-medium text-[color:var(--color-muted)] hover:text-[color:var(--color-ink)]"
      >
        <span aria-hidden>{open ? "▾" : "▸"}</span>
        <span>
          {t("chat.tools.activity")} ({calls.length})
        </span>
        {running && (
          <span className="flex items-center gap-1 text-[color:var(--color-accent)]">
            <span className="h-1.5 w-1.5 animate-pulse rounded-full bg-[color:var(--color-accent)]" />
            {t("chat.tools.running")}
          </span>
        )}
        {failed > 0 && (
          <span className="text-[color:var(--color-danger)]">
            {failed} {t("chat.tools.failed")}
          </span>
        )}
      </button>
      {open && (
        <ul className="space-y-2 border-t border-[color:var(--color-muted)]/15 px-3 py-2">
          {calls.map((c) => (
            <li key={c.callId} className="space-y-1">
              <div className="flex items-center gap-2 font-mono text-[11px]">
                <span className={STATUS_COLOR[c.status]} aria-hidden>
                  {STATUS_ICON[c.status]}
                </span>
                <span className="text-[color:var(--color-ink)]">{c.name}</span>
              </div>
              {c.args.trim().length > 0 && (
                <pre className="max-h-24 overflow-auto whitespace-pre-wrap break-all rounded bg-black/30 px-2 py-1 font-mono text-[10px] text-[color:var(--color-muted)]">
                  {c.args}
                </pre>
              )}
              {c.result != null && (
                <pre
                  className={
                    c.status === "error"
                      ? "max-h-32 overflow-auto whitespace-pre-wrap break-all rounded border border-[color:var(--color-danger)]/25 bg-[color:var(--color-danger)]/10 px-2 py-1 font-mono text-[10px] text-[color:var(--color-danger)]"
                      : "max-h-32 overflow-auto whitespace-pre-wrap break-all rounded bg-black/30 px-2 py-1 font-mono text-[10px] text-[color:var(--color-ink)]"
                  }
                >
                  {c.result}
                </pre>
              )}
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}