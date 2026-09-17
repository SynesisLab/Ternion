import { useEffect, useState } from "react";

import { t } from "../i18n";
import type { ApprovalReply } from "../lib/ipc";
import { useChatStore } from "../store/chatStore";

/**
 * §6.6 approval modal: every mutating tool call passes through here. The
 * stream stays paused (never cancelled) while this is open; Enter allows
 * once, Escape rejects. "Edit in place" lets the user take over the exact
 * content fs_write/fs_edit would land, which is then sent as the tool's own
 * argument.
 */

export function ApprovalDialog() {
  const approvals = useChatStore((s) => s.approvals);
  const respondApproval = useChatStore((s) => s.respondApproval);
  // Queue order: the executor awaits one call at a time; show the oldest.
  const current = approvals[0] ?? null;

  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState("");

  // Reset the editor per request.
  useEffect(() => {
    setEditing(false);
    setDraft(current?.resultContent ?? "");
  }, [current?.id, current?.resultContent]);

  const reply = (allow: boolean, mode: string): void => {
    if (!current) return;
    const edited = editing && current.resultContent !== null ? draft : null;
    const reply: ApprovalReply = {
      allow,
      mode,
      editedContent: allow ? (edited ?? null) : null,
    };
    void respondApproval(current.id, reply);
  };

  // Enter = allow once, Escape = reject.
  useEffect(() => {
    if (!current) return;
    const onKey = (e: KeyboardEvent): void => {
      if (e.key === "Escape") {
        reply(false, "");
      } else if (e.key === "Enter" && !e.shiftKey && !editing) {
        reply(true, "");
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [current, editing, reply]);

  if (!current) return null;
  const editable = current.resultContent !== null;

  return (
    <div className="fixed inset-0 z-[60] flex items-center justify-center bg-black/50 p-4">
      <div className="w-full max-w-xl rounded-xl border border-[color:var(--color-edge)] bg-[color:var(--color-panel)] p-5 shadow-2xl">
        <div className="mb-1 flex items-center justify-between">
          <div className="text-sm font-semibold text-[color:var(--color-ink)]">
            {t("approval.title")}
          </div>
          <span className="rounded bg-[color:var(--color-bg)] px-1.5 py-0.5 font-mono text-xs text-[color:var(--color-muted)]">
            {current.tool}
          </span>
        </div>
        <div className="text-sm text-[color:var(--color-ink)]">
          {t("approval.asking")}{" "}
          <span className="font-medium">{current.summary}</span>
        </div>
        <div className="mt-1 break-all font-mono text-xs text-[color:var(--color-muted)]">
          {current.path}
          {current.secondaryPath ? (
            <>
              {" → "}
              <span className="text-[color:var(--color-ink)]">
                {current.secondaryPath}
              </span>
            </>
          ) : null}
        </div>

        {current.tool === "fs_delete" ? (
          <div className="mt-3 rounded-lg border border-[color:var(--color-edge)] bg-[color:var(--color-bg)] p-2.5 text-xs text-[color:var(--color-muted)]">
            {t("approval.deleteNote")}
          </div>
        ) : null}

        {current.diff ? (
          <div className="mt-3">
            <div className="mb-1 text-xs font-medium text-[color:var(--color-muted)]">
              {t("approval.diff")}
            </div>
            <pre className="max-h-64 overflow-auto rounded-lg border border-[color:var(--color-edge)] bg-[color:var(--color-bg)] p-2.5 font-mono text-xs leading-relaxed">
              {current.diff.split("\n").map((line, i) => (
                <div
                  key={i}
                  className={
                    line.startsWith("+")
                      ? "text-[color:var(--color-accent-2)]"
                      : line.startsWith("-")
                        ? "text-[color:var(--color-danger)]"
                        : "text-[color:var(--color-muted)]"
                  }
                >
                  {line || " "}
                </div>
              ))}
            </pre>
          </div>
        ) : null}

        {editable && current.resultContent !== null ? (
          <div className="mt-3">
            {editing ? (
              <textarea
                autoFocus
                value={draft}
                onChange={(e) => setDraft(e.target.value)}
                className="h-56 w-full resize-y rounded-lg border border-[color:var(--color-accent)] bg-[color:var(--color-bg)] p-2.5 font-mono text-xs outline-none"
              />
            ) : (
              <button
                type="button"
                onClick={() => setEditing(true)}
                className="rounded-lg border border-[color:var(--color-edge)] px-3 py-1.5 text-xs text-[color:var(--color-ink)] hover:border-[color:var(--color-accent)]"
              >
                {t("approval.edit")}
              </button>
            )}
          </div>
        ) : null}

        <div className="mt-4 flex flex-wrap items-center justify-end gap-2">
          <button
            type="button"
            onClick={() => reply(false, "")}
            className="rounded-lg border border-[color:var(--color-edge)] px-3 py-1.5 text-sm text-[color:var(--color-ink)] hover:border-[color:var(--color-danger)]"
          >
            {t("approval.reject")}
          </button>
          <button
            type="button"
            onClick={() => reply(true, "always")}
            className="rounded-lg border border-[color:var(--color-edge)] px-3 py-1.5 text-sm text-[color:var(--color-ink)] hover:border-[color:var(--color-accent)]"
          >
            {t("approval.allowAlways")}
          </button>
          <button
            type="button"
            onClick={() => reply(true, "session")}
            className="rounded-lg border border-[color:var(--color-edge)] px-3 py-1.5 text-sm text-[color:var(--color-ink)] hover:border-[color:var(--color-accent)]"
          >
            {t("approval.allowSession")}
          </button>
          <button
            type="button"
            onClick={() => reply(true, "")}
            className="rounded-lg bg-[color:var(--color-accent)] px-3 py-1.5 text-sm font-medium text-white hover:opacity-90"
          >
            {t("approval.allowOnce")}
          </button>
        </div>
      </div>
    </div>
  );
}