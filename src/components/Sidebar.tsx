import { useState } from "react";

import { t } from "../i18n";
import { useChatStore } from "../store/chatStore";

export function Sidebar() {
  const conversations = useChatStore((s) => s.conversations);
  const activeId = useChatStore((s) => s.activeId);
  const selectConversation = useChatStore((s) => s.selectConversation);
  const newConversation = useChatStore((s) => s.newConversation);
  const renameConversation = useChatStore((s) => s.renameConversation);
  const deleteConversation = useChatStore((s) => s.deleteConversation);

  const [query, setQuery] = useState("");
  const [editingId, setEditingId] = useState<string | null>(null);
  const [draftTitle, setDraftTitle] = useState("");

  const q = query.trim().toLowerCase();
  const visible = conversations.filter(
    (c) => !q || (c.title ?? "").toLowerCase().includes(q),
  );

  const commitRename = () => {
    if (editingId) void renameConversation(editingId, draftTitle);
    setEditingId(null);
  };

  return (
    <aside className="flex h-full w-64 shrink-0 flex-col border-r border-[color:var(--color-edge)] bg-[color:var(--color-panel)]/50">
      <div className="flex items-center gap-2 px-3 py-3">
        <button
          type="button"
          onClick={() => void newConversation()}
          className="flex-1 rounded-lg bg-[color:var(--color-accent)] px-3 py-2 text-sm font-medium text-white hover:opacity-90"
        >
          + {t("sidebar.new")}
        </button>
      </div>
      <div className="px-3 pb-2">
        <input
          type="text"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          placeholder={t("sidebar.search")}
          className="w-full rounded-lg border border-[color:var(--color-edge)] bg-[color:var(--color-bg)] px-2.5 py-1.5 text-sm outline-none focus:border-[color:var(--color-accent)]"
        />
      </div>
      <div className="flex-1 overflow-y-auto px-2 pb-3">
        {visible.map((c) => (
          <div
            key={c.id}
            className={`group flex items-center gap-1 rounded-lg px-2.5 py-2 text-sm ${
              c.id === activeId
                ? "bg-[color:var(--color-accent)]/15 text-[color:var(--color-ink)]"
                : "text-[color:var(--color-muted)] hover:bg-[color:var(--color-panel)]"
            }`}
          >
            {editingId === c.id ? (
              <input
                autoFocus
                value={draftTitle}
                onChange={(e) => setDraftTitle(e.target.value)}
                onBlur={commitRename}
                onKeyDown={(e) => {
                  if (e.key === "Enter") commitRename();
                  if (e.key === "Escape") setEditingId(null);
                }}
                className="min-w-0 flex-1 rounded border border-[color:var(--color-accent)] bg-[color:var(--color-bg)] px-1 py-0.5 text-sm outline-none"
              />
            ) : (
              <>
                <button
                  type="button"
                  onClick={() => void selectConversation(c.id)}
                  onDoubleClick={() => {
                    setEditingId(c.id);
                    setDraftTitle(c.title ?? "");
                  }}
                  className="min-w-0 flex-1 truncate text-left"
                  title={c.title ?? t("sidebar.untitled")}
                >
                  {c.title ?? t("sidebar.untitled")}
                </button>
                <button
                  type="button"
                  onClick={() => {
                    setEditingId(c.id);
                    setDraftTitle(c.title ?? "");
                  }}
                  className="hidden shrink-0 rounded px-1 text-xs text-[color:var(--color-muted)] hover:text-[color:var(--color-ink)] group-hover:block"
                  title={t("sidebar.rename")}
                >
                  ✎
                </button>
                <button
                  type="button"
                  onClick={() => {
                    if (window.confirm(t("sidebar.deleteConfirm"))) {
                      void deleteConversation(c.id);
                    }
                  }}
                  className="hidden shrink-0 rounded px-1 text-xs text-[color:var(--color-muted)] hover:text-[color:var(--color-danger)] group-hover:block"
                  title={t("sidebar.delete")}
                >
                  ✕
                </button>
              </>
            )}
          </div>
        ))}
        {visible.length === 0 && (
          <div className="px-2.5 py-3 text-xs text-[color:var(--color-muted)]">
            {t("sidebar.empty")}
          </div>
        )}
      </div>
    </aside>
  );
}