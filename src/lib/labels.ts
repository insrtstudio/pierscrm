import { invoke } from "@tauri-apps/api/core";
import type { Step } from "./residency";

// Labels: the list of record labels and the demo campaigns sent to them
// (src-tauri/src/commands/labels.rs).

export type DemoPolicy = "email" | "form" | "closed";

export type TargetState =
  | "queued"
  | "unreachable"
  | "to_submit"
  | "to_validate"
  | "scheduled"
  | "failed"
  | "sent"
  | "followed"
  | "replied"
  | "declined"
  | "no_answer"
  | "bounced"
  | "withdrawn"
  | "signed";

export type Outcome = "replied" | "declined" | "no_answer" | "bounced" | "withdrawn" | "signed";

export interface LabelDemoRef {
  campaign_id: number;
  track: string;
  state: TargetState;
}

export interface Label {
  id: number;
  name: string;
  country?: string | null;
  genres?: string | null;
  website?: string | null;
  email?: string | null;
  contact_name?: string | null;
  demo_policy: DemoPolicy;
  demo_url?: string | null;
  tier: number;
  notes?: string | null;
  source?: string | null;
  archived: boolean;
  suppressed: boolean;
  demos: LabelDemoRef[];
}

export interface LabelInput {
  id?: number | null;
  name: string;
  country?: string | null;
  genres?: string | null;
  website?: string | null;
  email?: string | null;
  contact_name?: string | null;
  demo_policy: DemoPolicy;
  demo_url?: string | null;
  tier: number;
  notes?: string | null;
}

export interface DemoCard {
  id: number;
  track: string;
  status: "active" | "paused" | "done" | string;
  created_at: string;
  current_wave: number;
  paused_reason?: string | null;
  total: number;
  queued: number;
  todo: number;
  waiting: number;
  replied: number;
  closed: number;
  listened: number;
  signed: boolean;
}

export interface Target {
  id: number;
  label_id: number;
  name: string;
  country?: string | null;
  email?: string | null;
  demo_policy: DemoPolicy;
  demo_url?: string | null;
  tier: number;
  rank: number;
  wave?: number | null;
  state: TargetState;
  outcome_by?: "auto" | "user" | null;
  contact_id?: number | null;
  draft_id?: number | null;
  note?: string | null;
  sent_at?: string | null;
  followup_at?: string | null;
  followup_sent: boolean;
  closes_at?: string | null;
  opens: number;
  views: number;
  listens: number;
  completion?: number | null;
  reply_intent?: string | null;
  reply_summary?: string | null;
}

export interface DemoView {
  card: DemoCard;
  press_kit_id?: string | null;
  press_kit_title?: string | null;
  listen_url?: string | null;
  /// The insrt page this demo created (null: none, or a page picked from the list).
  kit_owned_id?: string | null;
  steps: Step[];
  window: { days: number[]; start: string; end: string };
  wave_size: number;
  close_days: number;
  insrt_ready: boolean;
  smtp_ready: boolean;
  sender: string | null;
  next_send?: string | null;
  targets: Target[];
}

export interface DemoPatch {
  track: string;
  press_kit_id?: string | null;
  press_kit_title?: string | null;
  listen_url?: string | null;
  steps: Step[];
  wave_size: number;
  close_days: number;
}

/// States that free the demo for the next wave.
export const CLOSED_STATES: TargetState[] = ["declined", "no_answer", "bounced", "withdrawn"];

export const labelsList = () => invoke<Label[]>("labels_list");
export const saveLabel = (label: LabelInput) => invoke<number>("labels_save", { label });
export const setLabelsArchived = (ids: number[], archived: boolean) =>
  invoke<void>("labels_set_archived", { ids, archived });
export const setLabelsTier = (ids: number[], tier: number) => invoke<void>("labels_set_tier", { ids, tier });
export const importRadarLabels = () => invoke<number>("labels_import_radar");

export const demosList = () => invoke<DemoCard[]>("label_demos_list");
export const createDemo = (track: string, pressKitId?: string | null, pressKitTitle?: string | null, listenUrl?: string | null) =>
  invoke<number>("label_demo_create", {
    track,
    pressKitId: pressKitId ?? null,
    pressKitTitle: pressKitTitle ?? null,
    listenUrl: listenUrl ?? null,
  });
export const getDemo = (id: number) => invoke<DemoView>("label_demo_get", { id });
export const saveDemo = (id: number, patch: DemoPatch) => invoke<void>("label_demo_save", { id, patch });
export const setDemoTargets = (id: number, labelIds: number[]) =>
  invoke<void>("label_demo_set_targets", { id, labelIds });
export const addLabelsToDemo = (id: number, labelIds: number[]) =>
  invoke<number>("label_demo_add_labels", { id, labelIds });
export const openWave = (id: number) => invoke<number>("label_demo_open_wave", { id });
export const createDemoKit = (id: number) => invoke<{ id: string; title: string }>("label_demo_create_kit", { id });
/// What the insrt page of a demo shows right now (site: GET press-kits/<id>/page).
export interface PagePreview {
  kit: { id: string; title: string; audience?: string | null };
  artist: { name: string; slug: string; genres: string[] };
  previewUrl?: string | null;
  tracks: { title: string; durationSec?: number | null; heading?: string | null }[];
  numbers: Record<"fr" | "en", { display: string; label: string; source: "youtube" | "spotify" | "manual" }[]>;
  videos: { title: string; views?: number | null }[];
  /// What each language of the page shows.
  bio: { fr: string; en: string };
  /// false: that language falls back to the other one.
  bioOwn?: { fr: boolean; en: boolean };
  /// Track downloads open on the page.
  downloads?: boolean;
  links: { streaming: string[]; social: string[] };
  heroPhoto: boolean;
  epkPdf: boolean;
  contacts: { booking: boolean; management: boolean };
  metrics: { fetchedAt?: string | null; spotify: boolean; youtube: boolean; errors: string[] };
}
export const demoPage = (id: number) => invoke<PagePreview>("label_demo_page", { id });
export const redraftDemo = (id: number) => invoke<number>("label_demo_redraft", { id });
export const setTargetOutcome = (targetId: number, outcome: Outcome | null) =>
  invoke<void>("label_target_set_outcome", { targetId, outcome });
export const markSubmitted = (targetId: number, done: boolean) =>
  invoke<void>("label_target_mark_submitted", { targetId, done });
export const setDemoStatus = (id: number, status: "active" | "paused" | "done") =>
  invoke<void>("label_demo_set_status", { id, status });
