// "Réponses à traiter": replies awaiting Thibault and stalled threads, with
// their drafts (see src-tauri/src/commands/reply_desk.rs).
import { invoke } from "@tauri-apps/api/core";
import type { SendResult } from "./types";

export interface DeskItem {
  id: number;
  kind: "reply" | "nudge";
  state: string;
  contact_id: number | null;
  contact: string;
  venue: string;
  campaign_id: number | null;
  campaign: string | null;
  to_addr: string;
  /** interested | question | unknown (no classification) */
  intent: string | null;
  summary: string | null;
  lang: string;
  subject: string;
  body: string;
  /** Model id, or "template" without an API key. */
  draft_by: string | null;
  edited: boolean;
  since: string;
  hours_waiting: number;
  /** The programmer has been waiting for more than 24h. */
  overdue: boolean;
  their_subject: string | null;
  their_text: string | null;
  their_at: string | null;
  our_subject: string | null;
  our_text: string | null;
  our_at: string | null;
}

export interface DeskList {
  items: DeskItem[];
  snoozed: number;
  nudge_days: number;
  notify: boolean;
}

export interface DeskCount {
  open: number;
  overdue: number;
}

export interface DeskPass {
  created: number;
  drafted: number;
  closed: number;
}

export const deskList = () => invoke<DeskList>("desk_list");
export const deskCount = () => invoke<DeskCount>("desk_count");
export const deskRefresh = () => invoke<DeskPass>("desk_refresh");
export const deskSave = (id: number, subject: string, body: string) =>
  invoke<void>("desk_save", { id, subject, body });
export const deskSend = (id: number, subject: string, body: string) =>
  invoke<SendResult>("desk_send", { id, subject, body });
export const deskIgnore = (id: number) => invoke<void>("desk_ignore", { id });
export const deskSnooze = (id: number, hours?: number) =>
  invoke<void>("desk_snooze", { id, hours: hours ?? null });
export const deskRedraft = (id: number) => invoke<void>("desk_redraft", { id });

/** Settings keys (read/written with get_setting / set_setting). */
export const DESK_NOTIFY_KEY = "desk_notify";
export const DESK_NUDGE_DAYS_KEY = "desk_nudge_days";
