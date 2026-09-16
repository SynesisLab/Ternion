import { useEffect } from "react";

import { Composer } from "./components/Composer";
import { MessageList } from "./components/MessageList";
import { ModelPicker } from "./components/ModelPicker";
import { StatusChip } from "./components/StatusChip";
import { useChatStore } from "./store/chatStore";

export default function App() {
  const init = useChatStore((s) => s.init);
  const activeId = useChatStore((s) => s.activeId);
  const models = useChatStore((s) => s.models);
  const model = useChatStore((s) => s.model);
  const setModel = useChatStore((s) => s.setModel);
  const connection = useChatStore((s) => s.connection);
  const refreshModels = useChatStore((s) => s.refreshModels);
  const sendMessage = useChatStore((s) => s.sendMessage);
  const stop = useChatStore((s) => s.stop);
  const streaming = useChatStore(
    (s) => (activeId ? (s.streaming[activeId] ?? false) : false),
  );

  useEffect(() => {
    void init();
  }, [init]);

  if (!activeId) {
    // Pre-DB state: usually sub-second; also covers initError.
    return (
      <div className="flex h-screen items-center justify-center bg-[color:var(--color-bg)] text-sm text-[color:var(--color-muted)]">
        Loading…
      </div>
    );
  }

  return (
    <div className="flex h-screen flex-col bg-[color:var(--color-bg)] text-[color:var(--color-ink)]">
      <header className="flex items-center justify-between border-b border-[color:var(--color-edge)] px-4 py-2.5">
        <div className="text-sm font-semibold tracking-tight">Ternion</div>
        <div className="flex items-center gap-3">
          <ModelPicker
            models={models}
            value={model}
            onChange={setModel}
            disabled={streaming}
          />
          <StatusChip status={connection} onRetry={() => void refreshModels()} />
        </div>
      </header>
      <MessageList conversationId={activeId} />
      <Composer
        onSend={sendMessage}
        onStop={() => void stop()}
        streaming={streaming}
        disabled={models.length === 0 || model === ""}
        offline={connection === "down"}
      />
    </div>
  );
}