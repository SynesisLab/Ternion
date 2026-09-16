import { t } from "../i18n";
import type { ModelInfo } from "../types/chat";

const BADGE_STYLES: Record<string, string> = {
  vision: "text-sky-300 border-sky-400/30 bg-sky-400/10",
  tools: "text-teal-300 border-teal-400/30 bg-teal-400/10",
  thinking: "text-violet-300 border-violet-400/30 bg-violet-400/10",
};

/** The Triad pins, in chip order. */
const PINS = [
  { value: "auto", label: t("chat.pin.auto"), title: t("chat.pin.autoTitle") },
  { value: "scout", label: t("chat.pin.scout"), title: t("chat.pin.scoutTitle") },
  { value: "titan", label: t("chat.pin.titan"), title: t("chat.pin.titanTitle") },
] as const;

function chipClass(active: boolean): string {
  return active
    ? "bg-[color:var(--color-accent)] text-white"
    : "text-[color:var(--color-muted)] hover:text-[color:var(--color-ink)] hover:bg-[color:var(--color-panel)]";
}

/**
 * The per-chat model selection (§9.4): a triad pin chip group — Auto lets the
 * router choose per message; Scout/Titan force a role — plus the explicit
 * model list for M0-style direct pinning.
 */
export function ModelPicker({
  models,
  pin,
  onPin,
  disabled,
}: {
  models: ModelInfo[];
  /** 'auto' | 'scout' | 'titan' | explicit model id. */
  pin: string;
  onPin: (value: string) => void;
  disabled?: boolean;
}) {
  const isExplicit = pin !== "auto" && pin !== "scout" && pin !== "titan";
  const selected = models.find((m) => m.id === (isExplicit ? pin : ""));
  const badges = (selected?.capabilities ?? []).filter((c) => c in BADGE_STYLES);

  return (
    <div className="flex items-center gap-2">
      <div
        className="flex items-center rounded-lg border border-[color:var(--color-edge)] bg-[color:var(--color-panel)] p-0.5"
        role="group"
        aria-label="Model selection"
      >
        {PINS.map((p) => (
          <button
            key={p.value}
            type="button"
            title={p.title}
            disabled={disabled}
            onClick={() => onPin(p.value)}
            className={`rounded-md px-2.5 py-1 text-xs font-medium transition-colors disabled:opacity-50 ${chipClass(pin === p.value)}`}
          >
            {p.label}
          </button>
        ))}
      </div>
      <select
        value={isExplicit ? pin : ""}
        disabled={disabled || models.length === 0}
        onChange={(e) => e.target.value && onPin(e.target.value)}
        className="max-w-40 rounded-md border border-[color:var(--color-edge)] bg-[color:var(--color-panel)] px-2 py-1 text-sm text-[color:var(--color-ink)] outline-none focus:border-[color:var(--color-accent)]"
      >
        {!isExplicit && <option value="">{t("chat.pin.models")}…</option>}
        {models.map((m) => (
          <option key={m.id} value={m.id}>
            {m.id}
          </option>
        ))}
      </select>
      <div className="flex items-center gap-1">
        {badges.map((cap) => (
          <span
            key={cap}
            className={`rounded border px-1.5 py-0.5 text-[10px] font-medium uppercase tracking-wide ${BADGE_STYLES[cap]}`}
          >
            {cap}
          </span>
        ))}
      </div>
    </div>
  );
}