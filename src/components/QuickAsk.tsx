/**
 * Quick capture palette (design §9.3): the small always-on-top window behind
 * `Win+Alt+T`. It shares the main bundle and store — asks route through the
 * full Triad pipeline (chat_send) into their own scratch conversation, and a
 * link hands the exchange back to the main window.
 */

import { useEffect, useRef, useState } from "react";
import { convertFileSrc } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";

import { openInMain, saveAttachment, startCapture } from "../lib/ipc";
import { t } from "../i18n";
import { useChatStore } from "../store/chatStore";
import type { Attachment, Message } from "../types/chat";

/** ArrayBuffer → base64 (chunked to avoid call-stack limits). */
function toBase64(bytes: Uint8Array): string {
  let binary = "";
  const chunk = 0x8000;
  for (let i = 0; i < bytes.length; i += chunk) {
    binary += String.fromCharCode(...bytes.subarray(i, i + chunk));
  }
  return btoa(binary);
}

// Stable empty fallback: a selector returning a fresh `[]` each call makes
// zustand's strict equality see a change every store update → re-render
// churn (same trap as App.tsx).
const EMPTY_MESSAGES: Message[] = [];

export function QuickAsk() {
  const init = useChatStore((s) => s.init);
  const newConversation = useChatStore((s) => s.newConversation);
  const activeId = useChatStore((s) => s.activeId);
  const messages = useChatStore((s) =>
    activeId ? (s.messagesByConv[activeId] ?? EMPTY_MESSAGES) : EMPTY_MESSAGES,
  );
  const draft = useChatStore((s) => (activeId ? s.drafts[activeId] : undefined));
  const streaming = useChatStore((s) =>
    activeId ? (s.streaming[activeId] ?? false) : false,
  );
  const connection = useChatStore((s) => s.connection);
  const model = useChatStore((s) => s.model);
  const sendMessage = useChatStore((s) => s.sendMessage);
  const stop = useChatStore((s) => s.stop);

  const [text, setText] = useState("");
  const [pending, setPending] = useState<Attachment[]>([]);
  /** This window session's scratch conversation exists (created on 1st send). */
  const [started, setStarted] = useState(false);
  const taRef = useRef<HTMLTextAreaElement>(null);
  const bodyRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    void init();
  }, [init]);

  // Esc dismisses the palette (it is a fire-and-forget surface; the exchange
  // lives on in the sidebar history).
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") void getCurrentWindow().close();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  // §9.3: a screenshot taken while the palette is open lands here.
  useEffect(() => {
    const unlisten = listen<Attachment>("ternion://capture-attached", (ev) => {
      setPending((p) => [...p, ev.payload]);
    });
    return () => {
      void unlisten.then((f) => f());
    };
  }, []);

  // Keep the newest answer visible while streaming or after a refetch.
  useEffect(() => {
    const body = bodyRef.current;
    if (body) body.scrollTop = body.scrollHeight;
  }, [messages.length, draft?.text, draft?.phase]);

  const offline = connection === "down" || model === "";
  const canSend = !offline && !streaming && (text.trim().length > 0 || pending.length > 0);

  const send = async () => {
    if (!canSend) return;
    // §9.3: quick asks get their own scratch conversation, created on the
    // first send of a window session (no empty rows from mere openings).
    if (!started) {
      await newConversation();
      setStarted(true);
    }
    const outgoing = text.trim();
    const attachments = pending;
    setText("");
    setPending([]);
    requestAnimationFrame(() => taRef.current?.focus());
    await sendMessage(outgoing, attachments);
  };

  const onKeyDown = (e: React.KeyboardEvent<HTMLTextAreaElement>) => {
    if (e.key === "Enter" && !e.shiftKey && !e.nativeEvent.isComposing) {
      e.preventDefault();
      void send();
    }
  };

  // Ctrl+V image paste into the palette (same pipeline as the composer).
  const onPaste = async (e: React.ClipboardEvent<HTMLTextAreaElement>) => {
    const files = [...e.clipboardData.items]
      .filter((i) => i.kind === "file" && i.type.startsWith("image/"))
      .map((i) => i.getAsFile())
      .filter((f): f is File => f !== null);
    if (files.length === 0) return;
    e.preventDefault();
    try {
      for (const file of files) {
        const bytes = new Uint8Array(await file.arrayBuffer());
        const att = await saveAttachment(toBase64(bytes), file.name);
        setPending((p) => [...p, att]);
      }
    } catch {
      /* drop failed paste images silently — the text still sends */
    }
  };

  const openMain = async () => {
    if (!activeId) return;
    await openInMain(activeId);
    await getCurrentWindow().close();
  };

  const hasExchange = messages.length > 0;

  return (
    <div className="flex h-screen flex-col overflow-hidden bg-[color:var(--color-bg)] text-[color:var(--color-ink)]">
      <header
        data-tauri-drag-region
        className="flex items-center gap-1 border-b border-[color:var(--color-edge)] px-3 py-2"
      >
        <span
          data-tauri-drag-region
          className="flex-1 select-none text-sm font-semibold"
        >
          {t("app.name")}
        </span>
        <button
          type="button"
          onClick={() => void startCapture("quick")}
          title={t("quick.screenshot")}
          className="rounded-lg px-2 py-1 text-xs text-[color:var(--color-muted)] hover:bg-[color:var(--color-panel)] hover:text-[color:var(--color-ink)]"
        >
          {t("quick.screenshot")}
        </button>
        {hasExchange && activeId && (
          <button
            type="button"
            onClick={() => void openMain()}
            className="rounded-lg border border-[color:var(--color-edge)] px-2 py-1 text-xs text-[color:var(--color-muted)] transition-colors hover:border-[color:var(--color-accent)] hover:text-[color:var(--color-ink)]"
          >
            {t("quick.openMain")}
          </button>
        )}
        <button
          type="button"
          onClick={() => void getCurrentWindow().close()}
          title="Close"
          className="rounded-lg px-2 py-1 text-xs text-[color:var(--color-muted)] hover:bg-[color:var(--color-panel)] hover:text-[color:var(--color-ink)]"
        >
          ✕
        </button>
      </header>

      <div ref={bodyRef} className="min-h-0 flex-1 overflow-y-auto px-3 py-3">
        {messages.map((m) => {
          if (m.id === "streaming") {
            const body = draft?.text ?? "";
            return (
              <div key="streaming" className="mb-3">
                {draft?.phase && (
                  <div className="mb-1 text-[11px] uppercase tracking-wide text-[color:var(--color-muted)]">
                    {t(`chat.status.${draft.phase}`)}
                  </div>
                )}
                <div className="whitespace-pre-wrap text-sm leading-relaxed">
                  {body || "…"}
                </div>
              </div>
            );
          }
          const isUser = m.role === "user";
          const images = m.content.filter((p) => p.type === "image");
          const textPart = m.content.find((p) => p.type === "text");
          return (
            <div key={m.id} className="mb-3">
              {isUser ? (
                <>
                  {images.length > 0 && (
                    <div className="mb-1 flex flex-wrap gap-1">
                      {images.map((img) =>
                        img.type === "image" ? (
                          <img
                            key={img.attachmentId}
                            src={img.processedPath ? convertFileSrc(img.processedPath) : ""}
                            alt=""
                            className="h-10 w-10 rounded-lg border border-[color:var(--color-edge)] object-cover"
                          />
                        ) : null,
                      )}
                    </div>
                  )}
                  <div className="whitespace-pre-wrap rounded-xl rounded-br-sm bg-[color:var(--color-accent)]/15 px-3 py-2 text-sm leading-relaxed">
                    {textPart?.type === "text" ? textPart.text : ""}
                  </div>
                </>
              ) : (
                <>
                  {m.routing && (
                    <div className="mb-1 text-[11px] text-[color:var(--color-muted)]">
                      {t("quick.routeNote")}{" "}
                      <span className="font-medium capitalize text-[color:var(--color-ink)]">
                        {m.routing.finalTarget}
                      </span>{" "}
                      · {t(`chat.ribbon.source.${m.routing.decision.source}`)}
                    </div>
                  )}
                  <div className="whitespace-pre-wrap text-sm leading-relaxed">
                    {textPart?.type === "text" ? textPart.text : ""}
                  </div>
                </>
              )}
            </div>
          );
        })}
      </div>

      <div className="border-t border-[color:var(--color-edge)] px-3 py-2.5">
        {pending.length > 0 && (
          <div className="mb-2 flex flex-wrap gap-1">
            {pending.map((att) => (
              <img
                key={att.id}
                src={convertFileSrc(att.processedPath)}
                alt=""
                className="h-10 w-10 rounded-lg border border-[color:var(--color-edge)] object-cover"
              />
            ))}
          </div>
        )}
        <div className="flex items-end gap-2">
          <textarea
            ref={taRef}
            rows={1}
            autoFocus
            value={text}
            onChange={(e) => setText(e.target.value)}
            onKeyDown={onKeyDown}
            onPaste={(e) => void onPaste(e)}
            placeholder={
              offline ? t("quick.offline") : t("quick.placeholder")
            }
            disabled={offline}
            className="max-h-28 min-h-9 flex-1 resize-none rounded-xl border border-[color:var(--color-edge)] bg-[color:var(--color-panel)] px-3 py-2 text-sm text-[color:var(--color-ink)] outline-none placeholder:text-[color:var(--color-muted)] focus:border-[color:var(--color-accent)] disabled:opacity-50"
          />
          {streaming ? (
            <button
              type="button"
              onClick={() => void stop()}
              className="shrink-0 rounded-xl border border-[color:var(--color-danger)]/40 px-3 py-2 text-xs font-medium text-[color:var(--color-danger)] hover:bg-[color:var(--color-danger)]/10"
            >
              {t("chat.input.stop")}
            </button>
          ) : (
            <button
              type="button"
              onClick={() => void send()}
              disabled={!canSend}
              className="shrink-0 rounded-xl bg-[color:var(--color-accent)] px-3 py-2 text-xs font-medium text-white hover:opacity-90 disabled:cursor-not-allowed disabled:opacity-40"
            >
              {t("chat.input.send")}
            </button>
          )}
        </div>
      </div>
    </div>
  );
}