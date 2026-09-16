import type { ConnectionStatus } from "../types/chat";

const DOT: Record<ConnectionStatus, string> = {
  ok: "bg-emerald-400",
  down: "bg-[color:var(--color-danger)]",
  unknown: "bg-amber-300",
};

const LABEL: Record<ConnectionStatus, string> = {
  ok: "Ollama",
  down: "Ollama unreachable — retry",
  unknown: "Checking Ollama…",
};

export function StatusChip({
  status,
  onRetry,
}: {
  status: ConnectionStatus;
  onRetry: () => void;
}) {
  return (
    <button
      type="button"
      onClick={onRetry}
      title={status === "down" ? "Click to retry" : undefined}
      className="flex items-center gap-2 rounded-full border border-[color:var(--color-edge)] bg-[color:var(--color-panel)] px-3 py-1 text-xs text-[color:var(--color-muted)] transition-colors hover:text-[color:var(--color-ink)]"
    >
      <span className={`h-2 w-2 rounded-full ${DOT[status]}`} />
      <span>{LABEL[status]}</span>
    </button>
  );
}