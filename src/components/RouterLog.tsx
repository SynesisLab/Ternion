import { useEffect, useState } from "react";

import { t } from "../i18n";
import { formatLatency, formatTokens } from "../lib/format";
import { listRoutingEvents } from "../lib/ipc";
import type { RoutingEvent } from "../types/chat";
import type { Target } from "../types/stream";

const TARGET_STYLES: Record<Target, string> = {
  scout: "text-emerald-300 border-emerald-400/30 bg-emerald-400/10",
  titan: "text-rose-300 border-rose-400/30 bg-rose-400/10",
};

function timeOf(ts: number): string {
  return new Date(ts).toLocaleTimeString();
}

/**
 * The router log drawer (§9.2): every persisted routing decision for the
 * conversation, newest first — with the override marker on pinned turns.
 */
export function RouterLog({
  open,
  conversationId,
  onClose,
}: {
  open: boolean;
  conversationId: string | null;
  onClose: () => void;
}) {
  const [events, setEvents] = useState<RoutingEvent[] | null>(null);
  const [expanded, setExpanded] = useState<string | null>(null);

  useEffect(() => {
    if (!open || !conversationId) return;
    let alive = true;
    setEvents(null);
    void listRoutingEvents(conversationId)
      .then((rows) => {
        if (alive) setEvents(rows);
      })
      .catch(() => {
        if (alive) setEvents([]);
      });
    return () => {
      alive = false;
    };
  }, [open, conversationId]);

  if (!open) return null;

  return (
    <div
      className="fixed inset-0 z-40 flex justify-end bg-black/40"
      onMouseDown={(e) => {
        if (e.target === e.currentTarget) onClose();
      }}
    >
      <aside className="flex h-full w-full max-w-lg flex-col border-l border-[color:var(--color-edge)] bg-[color:var(--color-panel)] shadow-2xl">
        <div className="flex items-center justify-between border-b border-[color:var(--color-edge)] px-4 py-3">
          <div className="text-sm font-semibold">{t("routerlog.title")}</div>
          <button
            type="button"
            onClick={onClose}
            className="rounded px-2 text-sm text-[color:var(--color-muted)] hover:text-[color:var(--color-ink)]"
          >
            ✕
          </button>
        </div>
        <div className="flex-1 overflow-y-auto p-3">
          {events === null && (
            <div className="p-4 text-sm text-[color:var(--color-muted)]">…</div>
          )}
          {events?.length === 0 && (
            <div className="p-4 text-sm text-[color:var(--color-muted)]">
              {t("routerlog.empty")}
            </div>
          )}
          <div className="space-y-2">
            {(events ?? []).map((ev) => {
              const openRow = expanded === ev.id;
              return (
                <div
                  key={ev.id}
                  className="rounded-lg border border-[color:var(--color-edge)] bg-[color:var(--color-bg)] px-3 py-2.5 text-xs"
                >
                  <button
                    type="button"
                    onClick={() => setExpanded(openRow ? null : ev.id)}
                    className="flex w-full flex-wrap items-center gap-1.5 text-left"
                  >
                    <span className="tabular-nums text-[color:var(--color-muted)]">
                      {timeOf(ev.ts)}
                    </span>
                    <span
                      className={`rounded border px-1.5 py-0.5 font-medium uppercase tracking-wide ${TARGET_STYLES[ev.finalTarget]}`}
                    >
                      {ev.finalTarget}
                    </span>
                    <span className="text-[color:var(--color-muted)]">
                      · {Math.round(ev.decision.confidence * 100)}%
                    </span>
                    <span className="text-[color:var(--color-muted)]">
                      · {formatLatency(ev.latencyMs)}
                    </span>
                    <span className="min-w-0 flex-1 truncate text-[color:var(--color-ink)]">
                      {ev.actualModel}
                    </span>
                    {ev.overrideKind === "manual" && (
                      <span className="rounded border border-[color:var(--color-edge)] px-1.5 py-0.5 text-[10px] uppercase tracking-wide text-[color:var(--color-muted)]">
                        {t("routerlog.override")}
                      </span>
                    )}
                    <span
                      className={`text-[10px] text-[color:var(--color-muted)] transition-transform ${openRow ? "rotate-90" : ""}`}
                    >
                      ▸
                    </span>
                  </button>
                  <div className="mt-1 text-[11px] text-[color:var(--color-muted)]">
                    {ev.decision.reason}
                  </div>
                  {openRow && (
                    <div className="mt-2 space-y-1 border-t border-[color:var(--color-edge)] pt-2 text-[11px] text-[color:var(--color-muted)]">
                      <div>
                        source: <span className="text-[color:var(--color-ink)]">{ev.decision.source}</span>
                        {" · "}
                        complexity {ev.decision.complexity}
                        {" · "}
                        {t("routerlog.estIn")} {formatTokens(ev.decision.estInTokens)}
                        {" · "}
                        {t("routerlog.estOut")} {formatTokens(ev.decision.estOutTokens)}
                      </div>
                      <div>
                        flags:{" "}
                        <span className="text-[color:var(--color-ink)]">
                          {Object.entries(ev.decision.flags)
                            .filter(([, on]) => on)
                            .map(([flag]) => flag)
                            .join(", ") || "none"}
                        </span>
                      </div>
                      {ev.decision.handoffNote && (
                        <div>
                          {t("routerlog.note")}:{" "}
                          <span className="text-[color:var(--color-ink)]">
                            {ev.decision.handoffNote}
                          </span>
                        </div>
                      )}
                    </div>
                  )}
                </div>
              );
            })}
          </div>
        </div>
      </aside>
    </div>
  );
}