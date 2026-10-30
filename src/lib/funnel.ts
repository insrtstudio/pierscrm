// Conversion funnel, subject A/B tests and weekly digest (src-tauri/src/commands/funnel.rs).
import { invoke } from "@tauri-apps/api/core";

export const STAGES = [
  "sent",
  "opened",
  "listened",
  "replied",
  "positive",
  "offer",
  "confirmed",
  "collected",
] as const;
export type Stage = (typeof STAGES)[number];

export type Stages = Record<Stage, number> & {
  /** Fees of the collected gigs (EUR). */
  fee_collected: number;
};

export interface Conversion {
  from: Stage;
  to: Stage;
  rate: number | null;
}

export interface FunnelRow {
  key: string;
  label: string;
  kind?: string | null;
  stages: Stages;
  reply_rate: number | null;
}

export interface Arm {
  label: string;
  example: string;
  sent: number;
  opened: number;
  replied: number;
  positive: number;
  reply_rate: number | null;
}

export type Verdict = "insufficient" | "significant" | "trend" | "none";

export interface VariantGroup {
  campaign_id: number;
  campaign_name: string;
  kind: "residency" | "standard" | string;
  template_id: number | null;
  template_name: string | null;
  step: number | null;
  arms: Arm[];
  verdict: Verdict;
  p_value: number | null;
  best: string | null;
  can_keep: boolean;
}

export interface FilterOptions {
  campaigns: [number, string][];
  categories: string[];
  countries: string[];
  cities: string[];
}

export interface FunnelReport {
  from: string | null;
  to: string | null;
  total: Stages;
  conversions: Conversion[];
  median_reply_hours: number | null;
  median_reply_to_confirmed_days: number | null;
  listened_no_reply: number;
  by_city: FunnelRow[];
  by_country: FunnelRow[];
  by_campaign: FunnelRow[];
  by_category: FunnelRow[];
  by_step: FunnelRow[];
  by_template: FunnelRow[];
  by_weekday: FunnelRow[];
  by_hour: FunnelRow[];
  by_slot: FunnelRow[];
  variants: VariantGroup[];
  options: FilterOptions;
}

export interface FunnelQuery {
  from?: string | null;
  to?: string | null;
  campaign_id?: number | null;
  kind?: "residency" | "standard" | null;
  category?: string | null;
  country?: string | null;
  city?: string | null;
}

export interface Suggestion {
  code: string;
  values: Record<string, string>;
  /** French sentence (fallback). */
  text: string;
}

export interface Activity {
  sent: number;
  replies: number;
  positive: number;
  offers: number;
  confirmed: number;
}

export interface Digest {
  week_start: string;
  week_end: string;
  cohort: Stages;
  activity: Activity;
  best_campaign: FunnelRow | null;
  worst_campaign: FunnelRow | null;
  variants: VariantGroup[];
  suggestions: Suggestion[];
  generated_at: string;
  emailed_at?: string | null;
}

/** Setting that turns on the Monday digest email (to the SMTP sender). */
export const DIGEST_EMAIL_SETTING = "funnel_digest_email";

export const funnelReport = (query: FunnelQuery) =>
  invoke<FunnelReport>("funnel_report", { query });

export const keepVariant = (g: Pick<VariantGroup, "campaign_id" | "template_id" | "step">, label: string) =>
  invoke<string>("funnel_keep_variant", {
    campaignId: g.campaign_id,
    templateId: g.template_id,
    step: g.step,
    label,
  });

export const funnelDigest = (refresh = false) => invoke<Digest>("funnel_digest", { refresh });
