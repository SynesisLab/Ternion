import type { ModelInfo } from "../types/chat";

const BADGE_STYLES: Record<string, string> = {
  vision: "text-sky-300 border-sky-400/30 bg-sky-400/10",
  tools: "text-teal-300 border-teal-400/30 bg-teal-400/10",
  thinking: "text-violet-300 border-violet-400/30 bg-violet-400/10",
};

export function ModelPicker({
  models,
  value,
  onChange,
  disabled,
}: {
  models: ModelInfo[];
  value: string;
  onChange: (modelId: string) => void;
  disabled?: boolean;
}) {
  const selected = models.find((m) => m.id === value);
  const badges = (selected?.capabilities ?? []).filter((c) => c in BADGE_STYLES);

  return (
    <div className="flex items-center gap-2">
      <select
        value={value}
        disabled={disabled || models.length === 0}
        onChange={(e) => onChange(e.target.value)}
        className="rounded-md border border-[color:var(--color-edge)] bg-[color:var(--color-panel)] px-2 py-1 text-sm text-[color:var(--color-ink)] outline-none focus:border-[color:var(--color-accent)]"
      >
        {models.length === 0 && <option value="">No models found</option>}
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