import { invoke } from "@tauri-apps/api/core";

/// Protecting the sending address (commands/deliverability): DNS
/// authentication, the shared sending budget and sender health.

export type DnsStatus = "ok" | "warn" | "fail" | "unknown";

export interface DnsFix {
  rtype: string;
  host: string;
  value: string;
}

export interface DnsCheck {
  key: "spf" | "dkim" | "dmarc" | "mx" | string;
  status: DnsStatus;
  summary: string;
  detail?: string | null;
  found?: string | null;
  fix?: DnsFix | null;
}

export interface DnsReport {
  domain: string;
  provider?: string | null;
  checks: DnsCheck[];
  checked_at: string;
  /// SPF and DKIM pass: the autopilot may run.
  ready: boolean;
}

export interface BudgetConfig {
  start_per_day: number;
  weekly_growth_pct: number;
  ceiling: number;
  hourly_cap: number;
  per_domain_daily: number;
  spacing_min_s: number;
  spacing_max_s: number;
  bulk_spacing_min_s: number;
  bulk_spacing_max_s: number;
  warmup_start?: string | null;
}

export interface BudgetStatus {
  config: BudgetConfig;
  warmup_start: string;
  warmup_day: number;
  daily_limit: number;
  sent_today: number;
  sent_hour: number;
  next_at?: string | null;
  ramp: number[];
  capped_domains: string[];
}

export interface SenderHealth {
  sender: string;
  sent: number;
  bounces: number;
  complaints: number;
  unsubscribes: number;
  replies: number;
  bounce_rate: number;
  complaint_rate: number;
  unsubscribe_rate: number;
  reply_rate: number;
}

export interface HealthThresholds {
  bounce_pct: number;
  complaint_pct: number;
  unsubscribe_pct: number;
  min_sample: number;
}

export interface DeliverabilityPause {
  reason: string;
  since: string;
  sender: string;
}

export interface DeliverabilityStatus {
  sender: string;
  budget: BudgetStatus;
  health: SenderHealth[];
  thresholds: HealthThresholds;
  pause?: DeliverabilityPause | null;
  dns?: DnsReport | null;
  one_click_unsubscribe: boolean;
}

export const deliverabilityStatus = () => invoke<DeliverabilityStatus>("deliverability_status");
export const checkDns = () => invoke<DnsReport>("deliverability_check_dns");
export const saveBudget = (config: BudgetConfig) =>
  invoke<BudgetConfig>("deliverability_save_budget", { config });
export const saveThresholds = (thresholds: HealthThresholds) =>
  invoke<HealthThresholds>("deliverability_save_thresholds", { thresholds });
export const resumeSending = () => invoke<void>("deliverability_resume");

/// Shared query (Settings, Emails, the banner); every send invalidates "emails".
export const deliverabilityQuery = {
  queryKey: ["emails", "deliverability"],
  queryFn: deliverabilityStatus,
  refetchInterval: 60_000,
};

/// UTC "YYYY-MM-DD HH:MM:SS" from the backend, as a local time.
export function utcToLocal(s: string): Date {
  return new Date(s.replace(" ", "T") + "Z");
}
