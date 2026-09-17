//! Region-capture overlay (design §7.1): a transparent, always-on-top
//! webview spans the monitor under the cursor; the drag rect is the only
//! undimmed area, and mouseup hands physical-pixel coords to the Rust GDI
//! capturer, which stores the attachment and closes this window.

import { useEffect, useRef, useState } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";

import { captureRegion } from "../lib/ipc";

/** Below this size (CSS px) the drag counts as a click → cancel. */
const MIN_SIZE = 4;

type Rect = { x: number; y: number; w: number; h: number };

export function CaptureOverlay() {
  const [rect, setRect] = useState<Rect | null>(null);
  const drag = useRef<{ sx: number; sy: number } | null>(null);
  const [busy, setBusy] = useState(false);

  const close = () => void getCurrentWindow().close();

  // Esc cancels — the hotkey's escape hatch; nothing is captured.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") close();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const onMouseDown = (e: React.MouseEvent) => {
    if (busy) return;
    if (e.button === 2) {
      close();
      return;
    }
    if (e.button !== 0) return;
    drag.current = { sx: e.clientX, sy: e.clientY };
    setRect({ x: e.clientX, y: e.clientY, w: 0, h: 0 });
  };

  const onMouseMove = (e: React.MouseEvent) => {
    const d = drag.current;
    if (!d) return;
    setRect(normalize(d.sx, d.sy, e.clientX, e.clientY));
  };

  const onMouseUp = () => {
    const d = drag.current;
    drag.current = null;
    if (!d || busy) return;
    if (!rect || rect.w < MIN_SIZE || rect.h < MIN_SIZE) {
      close(); // a click without a drag: cancel
      return;
    }
    setBusy(true);
    // Overlay CSS px → screen physical px for the GDI call.
    const dpr = window.devicePixelRatio || 1;
    void captureRegion({
      x: Math.round(rect.x * dpr),
      y: Math.round(rect.y * dpr),
      width: Math.round(rect.w * dpr),
      height: Math.round(rect.h * dpr),
    })
      .catch(() => {
        // Failures surface in the main-window log; the chip just never
        // arrives. Closing regardless keeps the overlay from sticking.
      })
      .finally(close);
  };

  return (
    <div
      className="fixed inset-0 overflow-hidden select-none"
      style={{ cursor: "crosshair" }}
      onMouseDown={onMouseDown}
      onMouseMove={onMouseMove}
      onMouseUp={onMouseUp}
      onContextMenu={(e) => e.preventDefault()}
    >
      {rect ? (
        <div
          className="absolute border border-[color:var(--color-accent)]"
          style={{
            left: rect.x,
            top: rect.y,
            width: rect.w,
            height: rect.h,
            // The classic screenshot dim: one huge shadow darkens
            // everything outside the selection.
            boxShadow: "0 0 0 100000px rgba(0,0,0,0.35)",
          }}
        />
      ) : (
        <div className="absolute inset-0 bg-black/35" />
      )}
    </div>
  );
}

/** Drag direction agnostic: top-left origin, positive extents. */
function normalize(sx: number, sy: number, cx: number, cy: number): Rect {
  return {
    x: Math.min(sx, cx),
    y: Math.min(sy, cy),
    w: Math.abs(cx - sx),
    h: Math.abs(cy - sy),
  };
}