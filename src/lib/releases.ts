import { invoke } from "@tauri-apps/api/core";

// Releases board (src-tauri/src/commands/releases.rs).

export type CardKind = "mastering" | "meta" | "distri" | "pitch" | "dj" | "playlists" | "groover" | "release" | "recap" | "custom";
export const CARD_KINDS: CardKind[] = ["meta", "distri", "pitch", "dj", "playlists", "groover", "mastering", "release", "recap", "custom"];
export const CAMPAIGN_KINDS: CardKind[] = ["dj", "playlists"];
export type Col = "todo" | "doing" | "waiting" | "done";
export const COLS: Col[] = ["todo", "doing", "waiting", "done"];

export interface Release {
  id: number;
  name: string;
  genre: string;
  master_state: "master" | "unmastered";
  artist: string;
  listen_url: string | null;
  notes: string | null;
  release_date: string | null;
}

export interface Step {
  label: string;
  done: boolean;
}

export interface CardData {
  fields?: Record<string, string>;
  checklist?: Step[];
  text?: string;
  notes?: string;
  url?: string;
  lang?: "en" | "fr";
}

export interface Card {
  id: number;
  release_id: number;
  release_name: string;
  kind: CardKind;
  title: string;
  due_date: string;
  col: Col;
  pos: number;
  data: CardData;
  targets: number;
  drafts: number;
  sent: number;
  replies: number;
  followups_due: number;
}

export interface Target {
  id: number;
  card_id: number;
  name: string;
  why: string | null;
  email: string | null;
  instagram: string | null;
  trackstack: string | null;
  source_url: string | null;
  channel: "email" | "instagram" | "none";
  subject: string | null;
  body: string | null;
  dm: string | null;
  followup: string | null;
  status: "found" | "draft" | "queued" | "sent" | "failed" | "skipped";
  error: string | null;
  sent_at: string | null;
  fu_status: "queued" | "sent" | "failed" | "skipped" | null;
  fu_sent_at: string | null;
  replied: boolean;
  opened_at: string | null;
  open_count: number;
  followup_due: boolean;
}

export interface InstaStatus {
  queued: number;
  sent_today: number;
  cap: number;
  busy: boolean;
  need_login: boolean;
  paused: boolean;
}

export interface Board {
  releases: Release[];
  cards: Card[];
  insta: InstaStatus;
  sender: string | null;
  ai_ready: boolean;
  search_ready: boolean;
}

export interface CardDetail {
  card: Card;
  release: Release;
  targets: Target[];
}

export interface ReleaseProgress {
  card_id: number;
  stage: "find" | "draft" | "send" | "insta";
  label: string;
  done: number;
  total: number;
  finished: boolean;
  error: string | null;
}

export const releasesBoard = () => invoke<Board>("releases_board");
export const releaseSave = (release: Partial<Release> & { name: string }) => invoke<number>("release_save", { release });
export const releaseValidate = (id: number, date: string) => invoke<void>("release_validate", { id, date });
export const releaseDelete = (id: number) => invoke<void>("release_delete", { id });
export const cardAdd = (releaseId: number, kind: CardKind, dueDate: string, title?: string) =>
  invoke<number>("card_add", { releaseId, kind, title: title ?? null, dueDate });
export const cardMove = (id: number, change: { col?: Col; dueDate?: string; title?: string }) =>
  invoke<void>("card_move", { id, col: change.col ?? null, dueDate: change.dueDate ?? null, title: change.title ?? null });
export const cardSetData = (id: number, data: CardData, col?: Col) => invoke<void>("card_set_data", { id, data, col: col ?? null });
export const cardDelete = (id: number) => invoke<void>("card_delete", { id });
export const cardDetail = (id: number) => invoke<CardDetail>("card_detail", { id });
export const cardFind = (id: number, names: string[]) => invoke<void>("card_find", { id, names });
export const cardWrite = (id: number) => invoke<string>("card_write", { id });
export const cardSend = (cardId: number, ids: number[], followup: boolean) => invoke<number>("card_send", { cardId, ids, followup });
export const targetSave = (edit: Pick<Target, "id" | "channel" | "email" | "instagram" | "subject" | "body" | "dm" | "followup">) =>
  invoke<void>("target_save", { edit });
export const targetsMark = (ids: number[], mark: "skip" | "restore" | "replied" | "not_replied" | "no_followup") =>
  invoke<void>("targets_mark", { ids, mark });
export const instaLogin = () => invoke<void>("insta_login");
export const instaQueue = (run: boolean) => invoke<void>("insta_queue", { run });
export const instaSetCap = (cap: number) => invoke<void>("insta_set_cap", { cap });

/// The metadata sheet a distributor asks for. `required` ones count as missing when empty.
export const META_FIELDS: { key: string; required: boolean; wide?: boolean }[] = [
  { key: "title", required: true },
  { key: "version", required: false },
  { key: "artists", required: true },
  { key: "label", required: true },
  { key: "genre", required: true },
  { key: "release_date", required: true },
  { key: "isrc", required: true },
  { key: "upc", required: true },
  { key: "bpm", required: false },
  { key: "key", required: false },
  { key: "duration", required: false },
  { key: "language", required: true },
  { key: "explicit", required: true },
  { key: "writers", required: true, wide: true },
  { key: "producers", required: false, wide: true },
  { key: "publisher", required: false, wide: true },
  { key: "artwork", required: true, wide: true },
];

export const metaMissing = (data: CardData) => META_FIELDS.filter((f) => f.required && !(data.fields?.[f.key] ?? "").trim()).length;
