import { useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";

import { ChatHeader } from "./components/ChatHeader";
import { Composer } from "./components/Composer";
import { MessageList } from "./components/MessageList";
import { SettingsDialog } from "./components/SettingsDialog";
import { Sidebar } from "./components/Sidebar";
import { useChatStore } from "./store/chatStore";

export default function App() {
  const init = useChatStore((s) => s.init);
  const conversations = useChatStore((s) => s.conversations);
  const activeId = useChatStore((s) => s.activeId);
  const models = useChatStore((s) => s.models);
  const model = useChatStore((s) => s.model);
  const setModel = useChatStore((s) => s.setModel);
  const connection = useChatStore((s) => s.connection);
  const refreshModels = useChatStore((s) => s.refreshModels);
  const renameConversation = useChatStore((s) => s.renameConversation);
  const newConversation = useChatStore((s) => s.newConversation);
  const sendMessage = useChatStore((s) => s.sendMessage);
  const stop = useChatStore((s) => s.stop);
  const initError = useChatStore((s) => s.initError);

  const [settingsOpen, setSettingsOpen] = useState(false);
  const streaming = useChatStore(
    (s) => (activeId ? (s.streaming[activeId] ?? false) : false),
  );

  useEffect(() => {
    void init();
  }, [init]);

  // Tray "New Chat" action (emitter lands in M0.10 with the tray itself).
  useEffect(() => {
    const unlisten = listen("ternion://new-chat", () => {
      void newConversation();
    });
    return () => {
      void unlisten.then((f) => f());
    };
  }, [newConversation]);

  const active = conversations.find((c) => c.id === activeId);

  if (!active) {
    return (
      <div className="flex h-screen items-center justify-center bg-[color:var(--color-bg)] text-sm text-[color:var(--color-muted)]">
        {initError ? `Error: ${initError}` : "Loading…"}
      </div>
    );
  }

  return (
    <div className="flex h-screen bg-[color:var(--color-bg)] text-[color:var(--color-ink)]">
      <Sidebar />
      <div className="flex min-w-0 flex-1 flex-col">
        <ChatHeader
          conversation={active}
          models={models}
          model={model}
          setModel={setModel}
          connection={connection}
          refreshModels={() => void refreshModels()}
          renameConversation={renameConversation}
          disabled={streaming}
          onOpenSettings={() => setSettingsOpen(true)}
        />
        <MessageList conversationId={active.id} />
        <Composer
          onSend={sendMessage}
          onStop={() => void stop()}
          streaming={streaming}
          disabled={models.length === 0 || model === ""}
          offline={connection === "down"}
        />
      </div>
      <SettingsDialog open={settingsOpen} onClose={() => setSettingsOpen(false)} />
    </div>
  );
}