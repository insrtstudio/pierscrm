import type { Status } from "./types";

/** Modernist status badges (uppercase, sharp). Red = hot, grays = cold. */
export const STATUS_STYLES: Record<Status, string> = {
  to_contact: "text-fg-subtle bg-muted",
  to_evaluate: "text-fg-subtle bg-muted",
  low_priority: "text-fg-faint bg-muted",
  contacted: "text-accent-2 bg-accent-soft",
  followed_up: "text-accent-2 bg-accent-soft",
  in_discussion: "text-accent bg-accent-soft",
  confirmed: "text-accent-fg bg-accent",
  declined: "text-fg-faint bg-muted",
  no_answer: "text-fg-faint bg-muted",
};

/** Bar / dot color per status (hex), matching the Modernist ramp. */
export const STATUS_DOT: Record<Status, string> = {
  to_contact: "#4d4642",
  to_evaluate: "#8a847e",
  low_priority: "#5c5651",
  contacted: "#ff7a5c",
  followed_up: "#ff7a5c",
  in_discussion: "#ec3013",
  confirmed: "#ec3013",
  declined: "#3a3532",
  no_answer: "#5c5651",
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
