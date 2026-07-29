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
  const iso = v.includes("T") ? v : v.replace(" ", "T") + (v.length > 10 ? "Z" : "");
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
