import { useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";

import { ApprovalDialog } from "./components/ApprovalDialog";
import { CaptureOverlay } from "./components/CaptureOverlay";
import { ChatHeader } from "./components/ChatHeader";
import { Composer } from "./components/Composer";
import { MessageList } from "./components/MessageList";
import { QuickAsk } from "./components/QuickAsk";
import { RouterLog } from "./components/RouterLog";
import { SettingsDialog } from "./components/SettingsDialog";
import { Sidebar } from "./components/Sidebar";
import { useChatStore } from "./store/chatStore";
import { useApplyStoredLocale, useI18n } from "./i18n";
import { useApplyStoredTheme } from "./lib/theme";

// Stable empty fallback: a selector returning a fresh `[]` each call makes
// zustand's strict equality see a change every store update → React
// re-render loop → "Maximum update depth exceeded" → blank window.
const EMPTY_SUGGESTIONS: string[] = [];

export default function App() {
  // Appearance (§10): every window (main, quick, capture) applies the theme
  // classes to documentElement; hooks stay above the label branches.
  useApplyStoredTheme();

  // §7.1/§9.3: the capture overlay and quick-ask palette mount the same
  // bundle but render only their own UI (label is fixed per window, so this
  // is stable per mount).
  const label = getCurrentWindow().label;
  if (label === "capture") {
    return <CaptureOverlay />;
  }
  if (label === "quick") {
    return <QuickAsk />;
  }
  return <MainApp />;
}

function MainApp() {
  const init = useChatStore((s) => s.init);
  const locale = useI18n((s) => s.locale);
  const conversations = useChatStore((s) => s.conversations);
  const activeId = useChatStore((s) => s.activeId);
  const selectConversation = useChatStore((s) => s.selectConversation);
  const models = useChatStore((s) => s.models);
  const pin = useChatStore((s) => s.pin);
  const setPin = useChatStore((s) => s.setPin);
  const connection = useChatStore((s) => s.connection);
  const refreshModels = useChatStore((s) => s.refreshModels);
  const renameConversation = useChatStore((s) => s.renameConversation);
  const bindWorkspace = useChatStore((s) => s.bindWorkspace);
  const unbindWorkspace = useChatStore((s) => s.unbindWorkspace);
  const newConversation = useChatStore((s) => s.newConversation);
  const sendMessage = useChatStore((s) => s.sendMessage);
  const continueWithTitan = useChatStore((s) => s.continueWithTitan);
  const stop = useChatStore((s) => s.stop);
  const initError = useChatStore((s) => s.initError);

  const [settingsOpen, setSettingsOpen] = useState(false);
  const [routerLogOpen, setRouterLogOpen] = useState(false);
  const streaming = useChatStore(
    (s) => (activeId ? (s.streaming[activeId] ?? false) : false),
  );
  const escalation = useChatStore(
    (s) => (activeId ? (s.escalation[activeId] ?? false) : false),
  );
  const suggestions = useChatStore(
    (s) => (activeId ? (s.suggestions[activeId] ?? EMPTY_SUGGESTIONS) : EMPTY_SUGGESTIONS),
  );
  const visionGap = useChatStore(
    (s) => (activeId ? (s.visionGap[activeId] ?? null) : null),
  );

  useEffect(() => {
    void init();
  }, [init]);

  // §9.5: the persisted language applies before anything else paints.
  useApplyStoredLocale();

  // Tray "New Chat" action (emitter lands in M0.10 with the tray itself).
  useEffect(() => {
    const unlisten = listen("ternion://new-chat", () => {
      void newConversation();
    });
    return () => {
      void unlisten.then((f) => f());
    };
  }, [newConversation]);

  // §9.3 "Open in main window": the quick-ask palette hands its exchange
  // over; the backend has already surfaced and focused this window.
  useEffect(() => {
    const unlisten = listen<string>("ternion://open-conversation", (ev) => {
      void selectConversation(ev.payload);
    });
    return () => {
      void unlisten.then((f) => f());
    };
  }, [selectConversation]);

  const active = conversations.find((c) => c.id === activeId);

  if (!active) {
    return (
      <div className="flex h-screen items-center justify-center bg-[color:var(--color-bg)] text-sm text-[color:var(--color-muted)]">
        {initError ? `Error: ${initError}` : "Loading…"}
      </div>
    );
  }

  return (
    <div
      key={locale}
      className="flex h-screen bg-[color:var(--color-bg)] text-[color:var(--color-ink)]"
    >
      <Sidebar />
      <div className="flex min-w-0 flex-1 flex-col">
        <ChatHeader
          conversation={active}
          models={models}
          pin={pin}
          onPin={setPin}
          connection={connection}
          refreshModels={() => void refreshModels()}
          renameConversation={renameConversation}
          bindWorkspace={bindWorkspace}
          unbindWorkspace={unbindWorkspace}
          disabled={streaming}
          onOpenSettings={() => setSettingsOpen(true)}
          onOpenRouterLog={() => setRouterLogOpen(true)}
        />
        <MessageList conversationId={active.id} />
        <Composer
          onSend={sendMessage}
          onStop={() => void stop()}
          streaming={streaming}
          disabled={models.length === 0}
          offline={connection === "down"}
          suggestions={suggestions}
          onSuggestion={(text) => void sendMessage(text)}
          escalation={escalation}
          onContinueTitan={() => void continueWithTitan()}
          visionGap={visionGap}
          onFixVision={() => {
            if (visionGap) setPin(visionGap);
          }}
        />
      </div>
      <SettingsDialog open={settingsOpen} onClose={() => setSettingsOpen(false)} />
      <ApprovalDialog />
      <RouterLog
        open={routerLogOpen}
        conversationId={activeId}
        onClose={() => setRouterLogOpen(false)}
      />
    </div>
  );
}