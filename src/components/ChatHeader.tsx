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
  pin,
  onPin,
  connection,
  refreshModels,
  renameConversation,
  bindWorkspace,
  unbindWorkspace,
  disabled,
  onOpenSettings,
  onOpenRouterLog,
}: {
  conversation: Conversation;
  models: ModelInfo[];
  /** 'auto' | 'scout' | 'titan' | explicit model id. */
  pin: string;
  onPin: (value: string) => void;
  connection: ConnectionStatus;
  refreshModels: () => void;
  renameConversation: (id: string, title: string) => Promise<void>;
  /** Bind one more workspace root (§6.3); resolves false on failure. */
  bindWorkspace: (id: string, path: string) => Promise<boolean>;
  unbindWorkspace: (id: string, path: string) => void;
  disabled?: boolean;
  onOpenSettings: () => void;
  onOpenRouterLog: () => void;
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
    <header className="flex flex-col border-b border-[color:var(--color-edge)]">
      <div className="flex items-center justify-between gap-3 px-4 py-2.5">
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
          <ModelPicker models={models} pin={pin} onPin={onPin} disabled={disabled} />
          <StatusChip status={connection} onRetry={refreshModels} />
          <button
            type="button"
            onClick={onOpenRouterLog}
            title={t("routerlog.open")}
            className="rounded px-2 py-1 text-sm text-[color:var(--color-muted)] hover:bg-[color:var(--color-panel)] hover:text-[color:var(--color-ink)]"
          >
            🧭
          </button>
          <button
            type="button"
            onClick={onOpenSettings}
            title={t("settings.title")}
            className="rounded px-2 py-1 text-sm text-[color:var(--color-muted)] hover:bg-[color:var(--color-panel)] hover:text-[color:var(--color-ink)]"
          >
            ⚙
          </button>
        </div>
      </div>
      <WorkspaceBar
        conversation={conversation}
        bindWorkspace={bindWorkspace}
        unbindWorkspace={unbindWorkspace}
        disabled={disabled}
      />
    </header>
  );
}

/** Collapsible workspace strip (§6.3): bound-root chips + inline bind input. */
function WorkspaceBar({
  conversation,
  bindWorkspace,
  unbindWorkspace,
  disabled,
}: {
  conversation: Conversation;
  bindWorkspace: (id: string, path: string) => Promise<boolean>;
  unbindWorkspace: (id: string, path: string) => void;
  disabled?: boolean;
}) {
  const [open, setOpen] = useState(false);
  const [adding, setAdding] = useState(false);
  const [draft, setDraft] = useState("");

  useEffect(() => {
    setOpen(false);
    setAdding(false);
    setDraft("");
  }, [conversation.id]);

  const roots = conversation.workspaceRoots ?? [];

  const bind = async () => {
    const path = draft.trim();
    if (!path) return;
    const ok = await bindWorkspace(conversation.id, path);
    if (ok) {
      setDraft("");
      setAdding(false);
    }
  };

  if (!open) {
    return (
      <div className="px-4 pb-1">
        <button
          type="button"
          onClick={() => setOpen(true)}
          className="flex w-full items-center gap-2 truncate rounded px-1 py-0.5 text-xs text-[color:var(--color-muted)] hover:text-[color:var(--color-ink)]"
        >
          <span className="shrink-0 font-semibold">{t("workspace.title")}</span>
          {roots.length === 0 ? (
            <span className="truncate">{t("workspace.none")}</span>
          ) : (
            <span className="truncate">{roots.join("  ·  ")}</span>
          )}
        </button>
      </div>
    );
  }

  return (
    <div className="flex flex-wrap items-center gap-2 px-4 pb-2">
      <button
        type="button"
        onClick={() => setOpen(false)}
        className="shrink-0 text-xs font-semibold text-[color:var(--color-muted)] hover:text-[color:var(--color-ink)]"
        title={t("workspace.title")}
      >
        ▾ {t("workspace.title")}
      </button>
      {roots.map((root) => (
        <span
          key={root}
          className="flex max-w-64 items-center gap-1 rounded border border-[color:var(--color-edge)] bg-[color:var(--color-panel)] px-2 py-0.5 text-xs"
          title={root}
        >
          <span className="truncate">{root}</span>
          <button
            type="button"
            disabled={disabled}
            onClick={() => unbindWorkspace(conversation.id, root)}
            title={t("workspace.unbind")}
            className="text-[color:var(--color-muted)] hover:text-[color:var(--color-ink)] disabled:opacity-40"
          >
            ✕
          </button>
        </span>
      ))}
      {adding ? (
        <span
          className="flex items-center gap-1"
          onBlur={(e) => {
            // Close only when focus leaves input AND button (click blurs
            // the input before the click lands).
            if (!e.currentTarget.contains(e.relatedTarget as Node | null)) {
              setAdding(false);
            }
          }}
        >
          <input
            autoFocus
            value={draft}
            onChange={(e) => setDraft(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter") void bind();
              if (e.key === "Escape") setAdding(false);
            }}
            placeholder={t("workspace.addHint")}
            className="w-64 rounded border border-[color:var(--color-accent)] bg-[color:var(--color-bg)] px-2 py-0.5 text-xs outline-none"
          />
          <button
            type="button"
            onClick={() => void bind()}
            className="rounded bg-[color:var(--color-accent)] px-2 py-0.5 text-xs font-semibold text-[color:var(--color-bg)]"
          >
            {t("workspace.bind")}
          </button>
        </span>
      ) : (
        <button
          type="button"
          disabled={disabled || roots.length >= 3}
          onClick={() => {
            setDraft("");
            setAdding(true);
          }}
          className="rounded border border-dashed border-[color:var(--color-edge)] px-2 py-0.5 text-xs text-[color:var(--color-muted)] hover:border-[color:var(--color-accent)] hover:text-[color:var(--color-ink)] disabled:opacity-40"
        >
          + {t("workspace.add")}
        </button>
      )}
    </div>
  );
}