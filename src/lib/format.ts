// Shared, locale-aware formatters. Single source of truth so numbers and dates
// read consistently across every screen (the audit found euro() duplicated and
// raw DB datetimes printed in the email log).
import i18n from "../i18n";

const locale = () => (i18n.language === "en" ? "en-US" : "fr-FR");

export const euro = (n?: number | null) =>
  new Intl.NumberFormat(locale(), {
    style: "currency",
    currency: "EUR",
    maximumFractionDigits: 0,
  }).format(n || 0);

// SQLite stores `datetime('now')` as UTC "YYYY-MM-DD HH:MM:SS". Normalise to an
// ISO string the Date constructor reads as UTC before formatting to local time.
function toDate(v?: string | null): Date | null {
  if (!v) return null;
  if (v.length === 10) return parseDay(v);
  const iso = v.includes("T") ? v : v.replace(" ", "T") + "Z";
  const d = new Date(iso);
  return isNaN(d.getTime()) ? null : d;
}

export function formatDate(v?: string | null, fallback = "·"): string {
  const d = toDate(v);
  if (!d) return v || fallback;
  return d.toLocaleDateString(locale(), { day: "2-digit", month: "short", year: "numeric" });
}

export function formatDateTime(v?: string | null, fallback = "·"): string {
  const d = toDate(v);
  if (!d) return v || fallback;
  return d.toLocaleString(locale(), {
    day: "2-digit",
    month: "short",
    hour: "2-digit",
    minute: "2-digit",
  });
}

// Calendar dates ("YYYY-MM-DD") are local days, not instants. `new Date("2026-11-01")`
// is UTC midnight, so west of UTC it printed the day before, and `toISOString()`
// on a local date shifted it back a day in Paris/Amsterdam.
export function parseDay(v?: string | null): Date | null {
  const m = v ? /^(\d{4})-(\d{2})-(\d{2})/.exec(v) : null;
  if (!m) return null;
  return new Date(Number(m[1]), Number(m[2]) - 1, Number(m[3]));
}

export function toDay(d: Date): string {
  return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-${String(
    d.getDate()
  ).padStart(2, "0")}`;
}

export const today = () => toDay(new Date());

/// Whole calendar days from today to `v` (negative when past).
export function daysUntil(v?: string | null): number | null {
  const d = parseDay(v);
  if (!d) return null;
  const now = new Date();
  const t0 = new Date(now.getFullYear(), now.getMonth(), now.getDate());
  return Math.round((d.getTime() - t0.getTime()) / 86_400_000);
}
