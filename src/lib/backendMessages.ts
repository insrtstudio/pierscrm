import i18n from "../i18n";
import type { ViRunProgress } from "./types";

// The Rust side of Discovery (harvest / enrich / radar / ra) no longer returns
// French sentences: errors are stable codes ("vi.enrich_none") and progress
// lines carry a `code` plus `args`. This is the one place they become text, in
// the UI language. Anything unknown (a raw network error, an older run's
// message) is shown as-is.

/** Text for an error a Discovery command rejected with. */
export function backendError(e: unknown): string {
  const raw =
    typeof e === "string" ? e : e instanceof Error ? e.message : String((e as any)?.toString?.() ?? e);
  return translateCode(raw) ?? raw;
}

/** `backend.<code>` when the code is known, else null. Also handles "code: detail". */
export function translateCode(raw?: string | null): string | null {
  if (!raw) return null;
  const s = raw.trim();
  const key = `backend.${s}`;
  if (i18n.exists(key)) return i18n.t(key);
  // Worker errors can be "radar.spotify_not_configured" or a prefixed detail.
  const [code, ...rest] = s.split(": ");
  if (rest.length && i18n.exists(`backend.${code}`)) return `${i18n.t(`backend.${code}`)} (${rest.join(": ")})`;
  return null;
}

/** Live progress line of a run, translated from its code when present. */
export function progressText(p?: ViRunProgress | null): string | null {
  if (!p) return null;
  if (p.code && i18n.exists(`vi_progress.${p.code}`)) {
    return i18n.t(`vi_progress.${p.code}`, (p.args ?? {}) as Record<string, unknown>);
  }
  return p.message ?? null;
}
