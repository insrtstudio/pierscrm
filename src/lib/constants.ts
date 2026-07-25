import type { Status } from "./types";

/** Tailwind classes for each pipeline status badge (text + subtle bg). */
export const STATUS_STYLES: Record<Status, string> = {
  to_contact: "text-slate-500 bg-slate-500/10",
  to_evaluate: "text-amber-500 bg-amber-500/10",
  low_priority: "text-zinc-400 bg-zinc-400/10",
  contacted: "text-blue-500 bg-blue-500/10",
  followed_up: "text-indigo-500 bg-indigo-500/10",
  in_discussion: "text-violet-500 bg-violet-500/10",
  confirmed: "text-emerald-500 bg-emerald-500/10",
  declined: "text-rose-500 bg-rose-500/10",
  no_answer: "text-neutral-500 bg-neutral-500/10",
};

/** Dot color per status for charts / indicators. */
export const STATUS_DOT: Record<Status, string> = {
  to_contact: "#64748b",
  to_evaluate: "#f59e0b",
  low_priority: "#a1a1aa",
  contacted: "#3b82f6",
  followed_up: "#6366f1",
  in_discussion: "#8b5cf6",
  confirmed: "#10b981",
  declined: "#f43f5e",
  no_answer: "#737373",
};

export const CATEGORIES = ["venue", "lineup", "major", "other"] as const;

/** Fields that can be targeted by the import column mapper. */
export const IMPORT_FIELDS = [
  "name",
  "priority",
  "promoter",
  "venue",
  "type",
  "area",
  "scale",
  "date",
  "time",
  "format",
  "reason",
  "contact_channel",
  "email",
  "email_status",
  "status",
  "first_contact",
  "follow_up",
  "notes",
  "website",
  "tags",
] as const;
