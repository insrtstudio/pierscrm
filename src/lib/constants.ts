import type { Status } from "./types";

/** Status badges on the insrt.studio ramp: sunset = hot, amber = warm, greys = cold. */
export const STATUS_STYLES: Record<Status, string> = {
  to_contact: "text-fg-subtle bg-muted",
  to_evaluate: "text-fg-subtle bg-muted",
  low_priority: "text-fg-faint bg-muted",
  contacted: "text-warn bg-warn/15",
  followed_up: "text-warn bg-warn/15",
  in_discussion: "text-accent-2 bg-accent-soft",
  confirmed: "text-accent-fg bg-accent",
  declined: "text-fg-faint bg-muted",
  no_answer: "text-fg-faint bg-muted",
};

/** Bar / dot color per status (hex), matching the badge ramp. */
export const STATUS_DOT: Record<Status, string> = {
  to_contact: "#a9a49b",
  to_evaluate: "#8b8680",
  low_priority: "#c9c5bd",
  contacted: "#f2b880",
  followed_up: "#ffa66b",
  in_discussion: "#ff7a3d",
  confirmed: "#ff5a1f",
  declined: "#4a4744",
  no_answer: "#6b6560",
};

export const CATEGORIES = ["venue", "curator", "lineup", "major", "other"] as const;

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
