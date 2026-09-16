import { useCallback, useEffect, useState } from "react";

import { ModelPicker } from "./components/ModelPicker";
import { StatusChip } from "./components/StatusChip";
import { listModels } from "./lib/ipc";
import type { ConnectionStatus, ModelInfo } from "./types/chat";

// Step 6: header shell with live model discovery. The chat surface arrives
// in step 8; this placeholder proves the picker + status chip against real Ollama.
export default function App() {
  const [models, setModels] = useState<ModelInfo[]>([]);
  const [status, setStatus] = useState<ConnectionStatus>("unknown");
  const [model, setModel] = useState("");

  const load = useCallback(() => {
    setStatus("unknown");
    listModels()
      .then((ms) => {
        setModels(ms);
        setStatus("ok");
        setModel((current) => current || ms[0]?.id || "");
      })
      .catch(() => setStatus("down"));
  }, []);

  useEffect(load, [load]);

  return (
    <div className="flex h-screen flex-col bg-[color:var(--color-bg)] text-[color:var(--color-ink)]">
      <header className="flex items-center justify-between border-b border-[color:var(--color-edge)] px-4 py-2.5">
        <div className="text-sm font-semibold tracking-tight">Ternion</div>
        <div className="flex items-center gap-3">
          <ModelPicker models={models} value={model} onChange={setModel} />
          <StatusChip status={status} onRetry={load} />
        </div>
      </header>
      <main className="flex flex-1 items-center justify-center">
        <div className="text-sm text-[color:var(--color-muted)]">
          Chat surface arrives in step 8
        </div>
      </main>
    </div>
  );
}