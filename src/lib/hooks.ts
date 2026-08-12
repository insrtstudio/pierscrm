import { useEffect, useRef, useState } from "react";

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
