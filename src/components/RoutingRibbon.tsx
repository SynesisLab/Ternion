import { t, type I18nKey } from "../i18n";
import { formatLatency } from "../lib/format";
import type { RoutingDecision, Target } from "../types/stream";

const TARGET_STYLES: Record<Target, string> = {
  scout: "text-emerald-300 border-emerald-400/30 bg-emerald-400/10",
  titan: "text-rose-300 border-rose-400/30 bg-rose-400/10",
};

const SOURCE_KEY: Record<RoutingDecision["source"], I18nKey> = {
  herald: "chat.ribbon.source.herald",
  heuristic: "chat.ribbon.source.heuristic",
  hard_rule: "chat.ribbon.source.hard_rule",
  manual: "chat.ribbon.source.manual",
};

/**
 * The routing ribbon (§9.1): who routed this turn, where it went, why —
 * e.g. "🟣 Herald → 🟢 Scout · 87% · 0.8s · short factual question" plus an
 * ⚡ marker when the previous turn ran on a different model. Works for both
 * the live stream (decision only) and the persisted message (with latency).
 */
export function RoutingRibbon({
  decision,
  finalTarget,
  latencyMs = null,
  switched = false,
}: {
  decision: RoutingDecision;
  finalTarget: Target;
  /** Herald classify latency (only known once the message is persisted). */
  latencyMs?: number | null;
  /** This turn ran on a different model than the previous assistant turn. */
  switched?: boolean;
}) {
  return (
    <div className="flex flex-wrap items-center gap-1.5 text-[11px] text-[color:var(--color-muted)]">
      <span className="flex items-center gap-1">
        <span className="text-[color:var(--color-accent-2)]">🟣</span>
        <span>{t(SOURCE_KEY[decision.source])}</span>
      </span>
      <span>→</span>
      <span
        className={`rounded border px-1.5 py-0.5 font-medium uppercase tracking-wide ${TARGET_STYLES[finalTarget]}`}
      >
        {finalTarget}
      </span>
      {decision.source === "herald" && (
        <span>· {Math.round(decision.confidence * 100)}%</span>
      )}
      {latencyMs != null && <span>· {formatLatency(latencyMs)}</span>}
      {decision.reason && (
        <span className="max-w-64 truncate" title={decision.reason}>
          · {decision.reason}
        </span>
      )}
      {switched && (
        <span
          title={decision.handoffNote || undefined}
          className="rounded border border-[color:var(--color-accent)]/30 bg-[color:var(--color-accent)]/10 px-1.5 py-0.5 font-medium text-[color:var(--color-accent)]"
        >
          ⚡ {t("chat.ribbon.switched")}
        </span>
      )}
    </div>
  );
}