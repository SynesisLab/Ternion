import { useEffect, useRef, useState } from "react";
import { convertFileSrc } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWebview } from "@tauri-apps/api/webview";

import { saveAttachment, saveAttachmentFile } from "../lib/ipc";
import { t } from "../i18n";
import type { Attachment } from "../types/chat";
import type { HandoffOffer } from "../lib/ipc";

/** Only these file extensions are offered to the backend on drop/pick. */
const IMAGE_EXT = /\.(png|jpe?g|gif|webp|bmp)$/i;

/** ArrayBuffer → base64 (chunked to avoid call-stack limits). */
function toBase64(bytes: Uint8Array): string {
  let binary = "";
  const chunk = 0x8000;
  for (let i = 0; i < bytes.length; i += chunk) {
    binary += String.fromCharCode(...bytes.subarray(i, i + chunk));
  }
  return btoa(binary);
}

export function Composer({
  onSend,
  onStop,
  streaming,
  disabled,
  offline,
  suggestions = [],
  onSuggestion,
  escalation = false,
  onContinueTitan,
  visionGap = null,
  onFixVision,
  handoff = null,
  onAcceptHandoff,
  onDismissHandoff,
  onNeverHandoff,
}: {
  onSend: (text: string, attachments: Attachment[]) => void;
  onStop: () => void;
  streaming: boolean;
  /** No model available / not initialized. */
  disabled?: boolean;
  offline?: boolean;
  /** Follow-up chips from the Herald sidecar (§3.7). */
  suggestions?: string[];
  onSuggestion?: (text: string) => void;
  /** Scout ran past its output ceiling last turn (§3.6). */
  escalation?: boolean;
  onContinueTitan?: () => void;
  /** §7.6: images this turn but the routed model lacks vision. "" = no
   * known alternative (warning only); a ref = one-click fix available. */
  visionGap?: string | null;
  onFixVision?: () => void;
  /** §5.6: a provider came online — offered, never silent. */
  handoff?: HandoffOffer | null;
  onAcceptHandoff?: () => void;
  onDismissHandoff?: () => void;
  onNeverHandoff?: () => void;
}) {
  const [text, setText] = useState("");
  const [pending, setPending] = useState<Attachment[]>([]);
  const [attachError, setAttachError] = useState(false);
  const taRef = useRef<HTMLTextAreaElement>(null);
  const fileRef = useRef<HTMLInputElement>(null);

  // Autosize up to ~200px.
  useEffect(() => {
    const ta = taRef.current;
    if (!ta) return;
    ta.style.height = "auto";
    ta.style.height = `${Math.min(ta.scrollHeight, 200)}px`;
  }, [text]);

  // §7.1: OS file drops land wherever the user released them — the listener
  // is webview-wide and every image path goes through the IMG pipeline.
  useEffect(() => {
    const unlisten = getCurrentWebview().onDragDropEvent((event) => {
      if (event.payload.type !== "drop") return;
      const images = event.payload.paths.filter((p) => IMAGE_EXT.test(p));
      if (images.length > 0) void addFiles(images);
    });
    return () => {
      void unlisten.then((f) => f());
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // §7.1: a Win+Alt+S region capture arrives as an already-stored
  // attachment row — add it straight to the pending chips.
  useEffect(() => {
    const unlisten = listen<Attachment>("ternion://capture-attached", (ev) => {
      setPending((p) => [...p, ev.payload]);
    });
    return () => {
      void unlisten.then((f) => f());
    };
  }, []);

  async function addFiles(paths: string[]) {
    setAttachError(false);
    for (const path of paths) {
      try {
        const att = await saveAttachmentFile(path);
        setPending((p) => [...p, att]);
      } catch {
        setAttachError(true);
      }
    }
  }

  async function addBlobs(files: File[]) {
    setAttachError(false);
    for (const file of files) {
      try {
        const bytes = new Uint8Array(await file.arrayBuffer());
        const att = await saveAttachment(toBase64(bytes), file.name);
        setPending((p) => [...p, att]);
      } catch {
        setAttachError(true);
      }
    }
  }

  const onPaste = (e: React.ClipboardEvent<HTMLTextAreaElement>) => {
    const files = [...e.clipboardData.items]
      .filter((i) => i.kind === "file" && i.type.startsWith("image/"))
      .map((i) => i.getAsFile())
      .filter((f): f is File => f !== null);
    if (files.length > 0) {
      e.preventDefault();
      void addBlobs(files);
    }
  };

  const canSend = !disabled && !streaming && (text.trim().length > 0 || pending.length > 0);

  const send = () => {
    if (!canSend) return;
    onSend(text, pending);
    setText("");
    setPending([]);
    setAttachError(false);
    // Refocus after the bubble insert re-renders.
    requestAnimationFrame(() => taRef.current?.focus());
  };

  const onKeyDown = (e: React.KeyboardEvent<HTMLTextAreaElement>) => {
    if (e.key === "Enter" && !e.shiftKey && !e.nativeEvent.isComposing) {
      e.preventDefault();
      send();
    }
  };

  return (
    <div className="border-t border-[color:var(--color-edge)] bg-[color:var(--color-bg)] px-6 py-4">
      <div className="mx-auto max-w-3xl">
        {handoff != null && !streaming && onAcceptHandoff && (
          <div className="mb-2 flex items-center gap-3 rounded-xl border border-[color:var(--color-accent-2)]/30 bg-[color:var(--color-accent-2)]/10 px-3 py-2 text-xs text-[color:var(--color-ink)]">
            <span className="flex-1">
              {t("chat.handoff.offer").replace("{name}", handoff.name)}{" "}
              <span className="font-mono">
                {handoff.models[0]}@{handoff.endpointId}
              </span>
              {!handoff.local && (
                <span className="block text-[11px] text-[color:var(--color-warn)]">
                  {t("chat.handoff.cost")}
                </span>
              )}
            </span>
            <button
              type="button"
              onClick={onAcceptHandoff}
              className="shrink-0 rounded-lg bg-[color:var(--color-accent)] px-3 py-1.5 font-medium text-white hover:opacity-90"
            >
              {t("chat.handoff.switch")}
            </button>
            <button
              type="button"
              onClick={onDismissHandoff}
              className="shrink-0 rounded-lg border border-[color:var(--color-edge)] px-3 py-1.5 font-medium text-[color:var(--color-ink)] hover:border-[color:var(--color-accent)]"
            >
              {t("chat.handoff.keep")}
            </button>
            {onNeverHandoff && (
              <button
                type="button"
                onClick={onNeverHandoff}
                className="shrink-0 rounded-lg border border-[color:var(--color-edge)] px-3 py-1.5 text-[color:var(--color-muted)] hover:text-[color:var(--color-ink)]"
              >
                {t("chat.handoff.never")}
              </button>
            )}
          </div>
        )}
        {escalation && !streaming && onContinueTitan && (
          <div className="mb-2 flex items-center gap-3 rounded-xl border border-[color:var(--color-accent)]/30 bg-[color:var(--color-accent)]/10 px-3 py-2 text-xs text-[color:var(--color-ink)]">
            <span className="flex-1">{t("chat.escalation.offer")}</span>
            <button
              type="button"
              onClick={onContinueTitan}
              className="shrink-0 rounded-lg bg-[color:var(--color-accent)] px-3 py-1.5 font-medium text-white hover:opacity-90"
            >
              {t("chat.escalation.continue")}
            </button>
          </div>
        )}
        {!streaming && suggestions.length > 0 && onSuggestion && (
          <div className="mb-2 flex flex-wrap items-center gap-2">
            <span className="text-[11px] uppercase tracking-wide text-[color:var(--color-muted)]">
              {t("chat.suggestions.label")}
            </span>
            {suggestions.map((s) => (
              <button
                key={s}
                type="button"
                onClick={() => onSuggestion(s)}
                className="rounded-full border border-[color:var(--color-edge)] bg-[color:var(--color-panel)] px-3 py-1 text-xs text-[color:var(--color-ink)] transition-colors hover:border-[color:var(--color-accent)]"
              >
                {s}
              </button>
            ))}
          </div>
        )}
        {visionGap != null && !streaming && (
          <div className="mb-2 flex items-center gap-3 rounded-xl border border-[color:var(--color-warn)]/30 bg-[color:var(--color-warn)]/10 px-3 py-2 text-xs text-[color:var(--color-ink)]">
            <span className="flex-1">{t("chat.vision.gap")}</span>
            {visionGap !== "" && onFixVision && (
              <button
                type="button"
                onClick={onFixVision}
                className="shrink-0 rounded-lg border border-[color:var(--color-warn)]/40 px-3 py-1.5 font-medium text-[color:var(--color-warn)] hover:bg-[color:var(--color-warn)]/20"
              >
                {t("chat.vision.fix")}
              </button>
            )}
          </div>
        )}
        {attachError && (
          <div className="mb-2 text-xs text-[color:var(--color-danger)]">
            {t("chat.attach.failed")}
          </div>
        )}
        {pending.length > 0 && (
          <div className="mb-2 flex flex-wrap gap-2">
            {pending.map((att) => (
              <div
                key={att.id}
                className="group relative overflow-hidden rounded-lg border border-[color:var(--color-edge)]"
                title={`${att.width}×${att.height}`}
              >
                <img
                  src={convertFileSrc(att.processedPath)}
                  alt=""
                  className="h-14 w-14 object-cover"
                />
                <button
                  type="button"
                  onClick={() =>
                    setPending((p) => p.filter((a) => a.id !== att.id))
                  }
                  title={t("chat.attach.remove")}
                  className="absolute top-0.5 right-0.5 flex h-4 w-4 items-center justify-center rounded-full bg-black/60 text-[10px] leading-none text-white opacity-0 transition-opacity group-hover:opacity-100"
                >
                  ×
                </button>
              </div>
            ))}
          </div>
        )}
        <div className="flex items-end gap-2">
          <input
            ref={fileRef}
            type="file"
            accept="image/png,image/jpeg,image/gif,image/webp,image/bmp"
            multiple
            className="hidden"
            onChange={(e) => {
              void addBlobs([...(e.target.files ?? [])]);
              // Allow re-picking the same file.
              e.target.value = "";
            }}
          />
          <button
            type="button"
            onClick={() => fileRef.current?.click()}
            disabled={disabled || streaming}
            title={t("chat.attach.add")}
            className="shrink-0 rounded-xl border border-[color:var(--color-edge)] bg-[color:var(--color-panel)] px-2.5 py-2.5 text-[color:var(--color-muted)] hover:border-[color:var(--color-accent)] hover:text-[color:var(--color-ink)] disabled:cursor-not-allowed disabled:opacity-40"
          >
            <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
              <path d="M21.44 11.05l-9.19 9.19a6 6 0 0 1-8.49-8.49l9.19-9.19a4 4 0 0 1 5.66 5.66l-9.2 9.19a2 2 0 0 1-2.83-2.83l8.49-8.48" />
            </svg>
          </button>
          <textarea
            ref={taRef}
            rows={1}
            value={text}
            onChange={(e) => setText(e.target.value)}
            onKeyDown={onKeyDown}
            onPaste={onPaste}
            placeholder={
              offline
                ? t("chat.input.placeholder.offline")
                : t("chat.input.placeholder")
            }
            disabled={disabled}
            className="max-h-[200px] flex-1 resize-none rounded-xl border border-[color:var(--color-edge)] bg-[color:var(--color-panel)] px-3.5 py-2.5 text-sm leading-relaxed text-[color:var(--color-ink)] placeholder:text-[color:var(--color-muted)] outline-none focus:border-[color:var(--color-accent)] disabled:opacity-50"
          />
          {streaming ? (
            <button
              type="button"
              onClick={onStop}
              className="shrink-0 rounded-xl border border-[color:var(--color-danger)]/40 bg-[color:var(--color-danger)]/10 px-4 py-2.5 text-sm font-medium text-[color:var(--color-danger)] hover:bg-[color:var(--color-danger)]/20"
            >
              {t("chat.input.stop")}
            </button>
          ) : (
            <button
              type="button"
              onClick={send}
              disabled={!canSend}
              className="shrink-0 rounded-xl bg-[color:var(--color-accent)] px-4 py-2.5 text-sm font-medium text-white hover:opacity-90 disabled:cursor-not-allowed disabled:opacity-40"
            >
              {t("chat.input.send")}
            </button>
          )}
        </div>
      </div>
    </div>
  );
}