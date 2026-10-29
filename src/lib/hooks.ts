import { useEffect, useRef, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { getArtistAvatar } from "./api";

/// Delay a fast-changing value (a search box) so it only reaches a query key
/// once the user pauses. Without this every keystroke mints a new React Query
/// key and fires a fresh round-trip to SQLite, so typing "amsterdam" queued nine
/// full-table LIKE scans and the UI waited on the last one.
export function useDebounced<T>(value: T, delay = 250): T {
  const [debounced, setDebounced] = useState(value);
  useEffect(() => {
    const id = setTimeout(() => setDebounced(value), delay);
    return () => clearTimeout(id);
  }, [value, delay]);
  return debounced;
}

/// Track an element's height without re-rendering on every scroll frame. Used to
/// size the virtualised venue list to the space it actually has.
export function useElementHeight<T extends HTMLElement>() {
  const ref = useRef<T | null>(null);
  const [height, setHeight] = useState(0);
  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    const ro = new ResizeObserver(([entry]) => setHeight(entry.contentRect.height));
    ro.observe(el);
    setHeight(el.getBoundingClientRect().height);
    return () => ro.disconnect();
  }, []);
  return { ref, height };
}

/// App-level intents from the native menu ("new:event", "check-updates"...).
/// The menu navigates first, and a lazy page may not be mounted yet when the
/// intent fires, so the last one is parked here and consumed on mount.
let pendingIntent: string | null = null;

export function emitIntent(id: string) {
  pendingIntent = id;
  window.dispatchEvent(new CustomEvent("app:intent", { detail: id }));
}

export function useAppIntent(id: string, handler: () => void) {
  const ref = useRef(handler);
  ref.current = handler;
  useEffect(() => {
    if (pendingIntent === id) {
      pendingIntent = null;
      ref.current();
    }
    const h = (e: Event) => {
      if ((e as CustomEvent).detail !== id) return;
      pendingIntent = null;
      ref.current();
    };
    window.addEventListener("app:intent", h);
    return () => window.removeEventListener("app:intent", h);
  }, [id]);
}

/// One artist's avatar (base64 data URL), loaded on its own and cached: the
/// `artists` list no longer carries it. The key sits under ["artist", id], so
/// invalidating an artist after an edit refreshes its avatar too.
export function useArtistAvatar(id?: number | null): string | null {
  const { data } = useQuery({
    queryKey: ["artist", id, "avatar"],
    queryFn: () => getArtistAvatar(id as number),
    enabled: id != null,
    staleTime: Infinity,
  });
  return data ?? null;
}
