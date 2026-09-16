import { useEffect, useRef } from "react";

/**
 * Pin-to-bottom auto scroll: follows new content while the user is at the
 * bottom, stops following the moment they scroll up. Re-checks on every
 * change of `contentKey`.
 */
export function useAutoScroll<T extends HTMLElement>(contentKey: unknown) {
  const ref = useRef<T | null>(null);
  const pinned = useRef(true);

  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    const onScroll = () => {
      pinned.current = el.scrollHeight - el.scrollTop - el.clientHeight < 40;
    };
    el.addEventListener("scroll", onScroll, { passive: true });
    return () => el.removeEventListener("scroll", onScroll);
  }, []);

  useEffect(() => {
    const el = ref.current;
    if (el && pinned.current) el.scrollTop = el.scrollHeight;
  }, [contentKey]);

  return ref;
}