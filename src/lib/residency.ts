// Residency campaigns: types and Tauri commands (see src-tauri/src/commands/residency).
import { invoke } from "@tauri-apps/api/core";
import type { Campaign } from "./types";

export const ASSET_TYPES = ["mix", "video", "photo", "epk", "stat", "link"] as const;
export type AssetType = (typeof ASSET_TYPES)[number];

export interface Asset {
  type: AssetType | string;
  title: string;
  url: string;
  note: string;
}

export interface Step {
  offset_days: number;
  subject: string;
  body: string;
  subject_en: string;
  body_en: string;
  extra_asset?: string | null;
}

export interface Zone {
  presets: string[];
  cities: string[];
  countries: string[];
}

export interface Window {
  /// 1 = Monday ... 7 = Sunday
  days: number[];
  start: string;
  end: string;
}

export interface ResidencyConfig {
  pitch_fr: string;
  pitch_en: string;
  concept: string;
  ask: "monthly" | "bimonthly" | string;
  assets: Asset[];
  press_kit_id?: string | null;
  press_kit_title?: string | null;
  steps: Step[];
  window: Window;
  daily_max: number;
  zone: Zone;
  lang: "auto" | "fr" | "en" | string;
  capacity_min: number;
  capacity_max: number;
  min_events: number;
}

export interface Residency {
  campaign_id: number;
  config: ResidencyConfig;
  autopilot: boolean;
  autopilot_confirmed_at?: string | null;
  paused_reason?: string | null;
  paused_kind?: "soft" | "hard" | null;
}

export interface StatusLine {
  autopilot: boolean;
  confirmed: boolean;
  paused_reason?: string | null;
  paused_kind?: string | null;
  sent_today: number;
  daily_max: number;
  quota_remaining: number;
  /// UTC "YYYY-MM-DD HH:MM:SS"
  next_send_at?: string | null;
  scheduled: number;
  drafts: number;
  sent_total: number;
  smtp_ok: boolean;
  last_sync_ok?: string | null;
}

export interface AiCost {
  calls: number;
  input_tokens: number;
  output_tokens: number;
  cost_usd: number;
  cost_eur: number;
}

export interface ResidencyView {
  campaign: Campaign;
  residency: Residency;
  status: StatusLine;
  visas: { country_code: string; country_name: string }[];
  cost: AiCost;
  ai_configured: boolean;
  insrt_configured: boolean;
  ai_model: string;
}

export interface Candidate {
  key: string;
  venue_id?: number | null;
  contact_id?: number | null;
  name: string;
  city?: string | null;
  country: string;
  capacity?: number | null;
  capacity_unknown: boolean;
  events: number;
  score_qualif: number;
  rank: number;
  artists: string[];
  artist_count: number;
  email: string;
  email_rank: number;
  first_name?: string | null;
  lang: string;
  source: "discovery" | "pipeline";
}

export interface BuildResult {
  candidates: Candidate[];
  excluded_suppressed: number;
  excluded_recent: number;
  excluded_declined: number;
  excluded_in_audience: number;
  excluded_placeholder: number;
  excluded_no_email: number;
}

export interface BuildOptions {
  capacity_min?: number;
  capacity_max?: number;
  include_unknown_capacity?: boolean;
  min_events?: number;
  limit?: number;
}

export const BOARD_COLUMNS = [
  "to_send",
  "sent",
  "followed_up",
  "engaged",
  "interested",
  "later",
  "declined",
  "wrong",
] as const;
export type BoardColumn = (typeof BOARD_COLUMNS)[number];

export interface BoardRow {
  contact_id: number;
  name: string;
  email?: string | null;
  city?: string | null;
  status: string;
  column: BoardColumn;
  steps_sent: number;
  last_sent_at?: string | null;
  /// UTC time, or "asap"
  next_at?: string | null;
  next_step?: number | null;
  opened: boolean;
  views: number;
  listen_seconds: number;
  feedback: number;
  replied: boolean;
  intent?: string | null;
  sentiment?: string | null;
  summary?: string | null;
  suppressed: boolean;
  lang?: string | null;
}

export interface MessageRow {
  id: number;
  contact_id: number;
  name: string;
  email?: string | null;
  step: number;
  subject: string;
  body: string;
  lang: string;
  status: "draft" | "scheduled" | "sent" | "skipped" | "failed";
  scheduled_for?: string | null;
  sent_at?: string | null;
  press_url?: string | null;
  note?: string | null;
  generated_by?: string | null;
  unresolved: string[];
}

export interface GenResult {
  created: number;
  with_ai: number;
  press_links: number;
  failed: number;
  errors: string[];
}

export interface ReplyLine {
  contact_id?: number | null;
  name: string;
  sentiment: string;
  intent: string;
  summary: string;
}

export interface Counts {
  sent: number;
  bounced: number;
  delivered: number;
  opened: number;
  views: number;
  listens: number;
  feedback: number;
  downloads: number;
  replies: number;
}

export interface HotSignal {
  contact_id: number;
  campaign_id?: number | null;
  name: string;
  kind: string;
  track_title?: string | null;
  seconds: number;
  comment?: string | null;
  at: string;
}

export interface ReportData {
  day: string;
  yesterday: Counts;
  total: Counts;
  by_intent: Record<string, number>;
  by_sentiment: Record<string, number>;
  replies_yesterday: ReplyLine[];
  followups_today: string[];
  hot: HotSignal[];
  next_actions: string[];
  autopilot: boolean;
  paused_reason?: string | null;
  audience: number;
  remaining_to_send: number;
}

export interface ReportRow {
  day: string;
  data: ReportData | null;
  synthesis?: string | null;
  created_at: string;
}

export interface LogRow {
  at: string;
  level: string;
  code: string;
  message: string;
}

export interface PressKit {
  id: string;
  title: string;
  artistSlug?: string | null;
  updatedAt?: string | null;
}

export interface ResidencyAction {
  key: string;
  kind: "plan" | "reply" | "unread" | "later" | "wrong" | "hot";
  day: string;
  campaign_id?: number | null;
  campaign?: string | null;
  contact_id?: number | null;
  name: string;
  insight_id?: number | null;
  intent?: string | null;
  sentiment?: string | null;
  summary?: string | null;
  suggested_subject?: string | null;
  suggested_body?: string | null;
  mentioned_email?: string | null;
  track_title?: string | null;
  seconds?: number | null;
  count?: number | null;
  next_at?: string | null;
}

export const residencyPresets = () => invoke<{ key: string; label: string }[]>("residency_presets");
export const createResidency = (name: string, zone: Zone) => invoke<number>("residency_create", { name, zone });
export const getResidency = (campaignId: number) => invoke<ResidencyView>("residency_get", { campaignId });
export const saveResidency = (campaignId: number, config: ResidencyConfig) =>
  invoke<void>("residency_save", { campaignId, config });
export const setAutopilot = (campaignId: number, on: boolean) =>
  invoke<void>("residency_set_autopilot", { campaignId, on });
export const buildTarget = (campaignId: number, options?: BuildOptions) =>
  invoke<BuildResult>("residency_build_target", { campaignId, options: options ?? null });
export const addTargets = (
  campaignId: number,
  picks: { venue_id?: number | null; contact_id?: number | null; email: string; first_name?: string | null }[]
) => invoke<number>("residency_add_targets", { campaignId, picks });
export const removeTarget = (campaignId: number, contactId: number) =>
  invoke<void>("residency_remove_target", { campaignId, contactId });
export const residencyBoard = (campaignId: number) => invoke<BoardRow[]>("residency_board", { campaignId });
export const generateDrafts = (campaignId: number, contactIds?: number[] | null, regenerate?: boolean) =>
  invoke<GenResult>("residency_generate", { campaignId, contactIds: contactIds ?? null, regenerate: regenerate ?? false });
export const residencyMessages = (campaignId: number) => invoke<MessageRow[]>("residency_messages", { campaignId });
export const saveMessage = (id: number, subject: string, body: string) =>
  invoke<void>("residency_save_message", { id, subject, body });
export const setMessageStatus = (id: number, status: "scheduled" | "draft" | "skipped") =>
  invoke<void>("residency_set_message_status", { id, status });
export const validateAll = (campaignId: number) =>
  invoke<{ scheduled: number; unresolved: number }>("residency_validate_all", { campaignId });
export const residencyReports = (campaignId: number) => invoke<ReportRow[]>("residency_reports", { campaignId });
export const reportNow = (campaignId: number) => invoke<boolean>("residency_report_now", { campaignId });
export const residencyLog = (campaignId?: number | null, limit?: number) =>
  invoke<LogRow[]>("residency_log", { campaignId: campaignId ?? null, limit: limit ?? 100 });
export const residencyActions = () => invoke<ResidencyAction[]>("residency_actions");
export const markHandled = (insightId: number) => invoke<void>("residency_mark_handled", { insightId });
export const pressKits = () => invoke<PressKit[]>("residency_press_kits");
export const aiCostMonth = (campaignId?: number | null) =>
  invoke<AiCost>("ai_cost_month", { campaignId: campaignId ?? null });

/// A backend UTC timestamp ("YYYY-MM-DD HH:MM:SS") as a Date.
export function utcDate(s?: string | null): Date | null {
  if (!s || s === "asap") return null;
  const d = new Date(s.replace(" ", "T") + "Z");
  return isNaN(d.getTime()) ? null : d;
}
