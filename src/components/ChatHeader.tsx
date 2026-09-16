import { useEffect, useState } from "react";

import { t } from "../i18n";
import type {
  ConnectionStatus,
  Conversation,
  ModelInfo,
} from "../types/chat";
import { ModelPicker } from "./ModelPicker";
import { StatusChip } from "./StatusChip";

export function ChatHeader({
  conversation,
  models,
  model,
  setModel,
  connection,
  refreshModels,
  renameConversation,
  disabled,
  onOpenSettings,
}: {
  conversation: Conversation;
  models: ModelInfo[];
  model: string;
  setModel: (id: string) => void;
  connection: ConnectionStatus;
  refreshModels: () => void;
  renameConversation: (id: string, title: string) => Promise<void>;
  disabled?: boolean;
  onOpenSettings: () => void;
}) {
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState("");

  useEffect(() => {
    setEditing(false);
  }, [conversation.id]);

  const commit = () => {
    const title = draft.trim();
    setEditing(false);
    if (title) void renameConversation(conversation.id, title);
  };

  return (
    <header className="flex items-center justify-between gap-3 border-b border-[color:var(--color-edge)] px-4 py-2.5">
      <div className="flex min-w-0 items-center gap-3">
        {editing ? (
          <input
            autoFocus
            value={draft}
            onChange={(e) => setDraft(e.target.value)}
            onBlur={commit}
            onKeyDown={(e) => {
              if (e.key === "Enter") commit();
              if (e.key === "Escape") setEditing(false);
            }}
            className="w-64 rounded border border-[color:var(--color-accent)] bg-[color:var(--color-bg)] px-2 py-1 text-sm font-semibold outline-none"
          />
        ) : (
          <button
            type="button"
            onClick={() => {
              setDraft(conversation.title ?? "");
              setEditing(true);
            }}
            className="max-w-72 truncate rounded px-1 py-0.5 text-sm font-semibold tracking-tight hover:bg-[color:var(--color-panel)]"
            title={t("header.renameTitle")}
          >
            {conversation.title ?? t("sidebar.untitled")}
          </button>
        )}
      </div>
      <div className="flex shrink-0 items-center gap-3">
        <ModelPicker models={models} value={model} onChange={setModel} disabled={disabled} />
        <StatusChip status={connection} onRetry={refreshModels} />
        <button
          type="button"
          onClick={onOpenSettings}
          title={t("settings.title")}
          className="rounded px-2 py-1 text-sm text-[color:var(--color-muted)] hover:bg-[color:var(--color-panel)] hover:text-[color:var(--color-ink)]"
        >
          ⚙
        </button>
      </div>
    </header>
  );
}