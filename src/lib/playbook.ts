// Agent library ("Bibliothèque de l'agent") and graduated autonomy of the
// reply desk (see src-tauri/src/commands/playbook.rs and autonomy.rs).
import { invoke } from "@tauri-apps/api/core";

export type EntryKind = "fact" | "answer" | "objection";

export interface PlaybookEntry {
  id: number;
  kind: EntryKind;
  topic: string;
  title: string;
  text_fr: string;
  text_en: string;
  /** Only approved entries reach the agent's prompts. */
  status: "draft" | "approved";
  seed_key: string | null;
  position: number;
  updated_at: string;
}

export interface ArtistFacts {
  id: number | null;
  name: string;
  tagline: string | null;
  bio: string | null;
  fee_range: string | null;
  tech_rider: string | null;
  mix_url: string | null;
  links: string | null;
  stats: string | null;
  press_quotes: string | null;
  played_at: string[];
}

export interface PlaybookView {
  entries: PlaybookEntry[];
  topics: { kind: EntryKind; topic: string }[];
  agent_name: string;
  artist: ArtistFacts;
}

export const AUTONOMY_CATEGORIES = ["music_request", "call_slots", "holding", "nudge"] as const;
export type AutonomyCategory = (typeof AUTONOMY_CATEGORIES)[number];

export interface AutonomySettings {
  enabled: boolean;
  delay_min: number;
  levels: { category: string; level: number }[];
  agent_name: string;
  min_confidence: number;
}

export interface AutoRow {
  id: number;
  desk_id: number;
  contact: string;
  category: string;
  reason: string;
  /** scheduled | sending | sent | cancelled | blocked | failed */
  state: string;
  due_at: string;
  done_at: string | null;
  note: string | null;
  subject: string;
}

export interface DeskAuto {
  desk_id: number;
  category: string | null;
  confidence: number | null;
  note: string | null;
  playbook: { id: number; topic: string; title: string }[];
  autosend_id: number | null;
  due_at: string | null;
}

export interface AutonomyOverview {
  settings: AutonomySettings;
  items: DeskAuto[];
  history: AutoRow[];
}

export const playbookList = () => invoke<PlaybookView>("playbook_list");
export const playbookSave = (entry: Partial<PlaybookEntry> & { kind: EntryKind; topic: string }) =>
  invoke<number>("playbook_save", { entry });
export const playbookSetStatus = (id: number, status: "draft" | "approved") =>
  invoke<void>("playbook_set_status", { id, status });
export const playbookDelete = (id: number) => invoke<void>("playbook_delete", { id });

export const autonomyOverview = () => invoke<AutonomyOverview>("autonomy_overview");
export const autonomySave = (settings: AutonomySettings) =>
  invoke<AutonomySettings>("autonomy_save", { settings });
export const autonomyStopAll = () => invoke<number>("autonomy_stop_all");
export const autosendCancel = (id: number) => invoke<boolean>("autosend_cancel", { id });

/** "HH:MM" local, from a UTC "YYYY-MM-DD HH:MM:SS". */
export function localTime(utc: string | null | undefined): string {
  if (!utc) return "";
  const d = new Date(utc.replace(" ", "T") + "Z");
  if (Number.isNaN(d.getTime())) return utc;
  return d.toLocaleTimeString("fr-FR", { hour: "2-digit", minute: "2-digit" });
}
