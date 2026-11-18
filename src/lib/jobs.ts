import { invoke } from "@tauri-apps/api/core";
import { applyOpens, getSetting } from "./api";

// Job search desk (src-tauri/src/commands/jobs).

export type JobStatus = "new" | "shortlisted" | "applied" | "interview" | "offer" | "rejected" | "dismissed" | "closed";
export const JOB_STATUSES: JobStatus[] = ["new", "shortlisted", "applied", "interview", "offer", "rejected", "dismissed", "closed"];

export interface Job {
  id: number;
  source: string;
  url: string | null;
  title: string;
  company: string;
  location: string;
  country: string;
  domain: string;
  contract: string | null;
  salary: string | null;
  posted_at: string | null;
  found_at: string;
  apply_kind: "email" | "form" | "unknown";
  apply_email: string | null;
  apply_url: string | null;
  status: JobStatus;
  score: number | null;
  line: string | null;
  company_brief: string | null;
  company_kind: string | null;
  company_site: string | null;
  roster: string | null;
  contact_name: string | null;
  contact_role: string | null;
  contact_email: string | null;
  contact_source: string | null;
  lang: string | null;
  next_action: string | null;
  next_action_at: string | null;
  notes: string | null;
  app_state: "draft" | "sent" | "manual" | null;
  ats_score: number | null;
  sent_at: string | null;
  opened_at: string | null;
  open_count: number;
  tracked: boolean;
  replied_at: string | null;
  followup_due: string | null;
  followup_sent_at: string | null;
}

export interface JobsBoard {
  jobs: Job[];
  new_today: number;
  to_validate: number;
  sent: number;
  opened: number;
  replies: number;
  followups_due: number;
  last_run: string | null;
  last_run_notes: string | null;
  running: boolean;
  ai_ready: boolean;
  sender: string | null;
  profile_ready: boolean;
  pdf_ready: boolean;
  adzuna_ready: boolean;
  search_ready: boolean;
}

export interface AtsCheck {
  key: string;
  ok: boolean;
  detail: string;
}

export interface AtsReport {
  score: number;
  verdict: "pass" | "borderline" | "risk";
  checks: AtsCheck[];
  keywords_found: string[];
  keywords_addable: string[];
  keywords_missing: string[];
  coverage: number;
}

export interface JobFit {
  strengths: string[];
  gaps: string[];
  keywords: string[];
  work_right: string | null;
  seniority: string | null;
  deadline: string | null;
}

export interface JobApplication {
  state: "draft" | "sent" | "manual";
  cv_path: string | null;
  cv_warnings: string[];
  ats: AtsReport | null;
  subject: string;
  body: string;
  followup_body: string;
}

export interface JobDetail {
  job: Job;
  description: string;
  fit: JobFit | null;
  application: JobApplication | null;
}

export interface JobInput {
  id?: number | null;
  title: string;
  company: string;
  location: string;
  country: string;
  url?: string | null;
  contract?: string | null;
  salary?: string | null;
  description?: string | null;
  apply_email?: string | null;
  contact_name?: string | null;
  contact_email?: string | null;
  next_action?: string | null;
  next_action_at?: string | null;
  notes?: string | null;
}

export interface JobSearch {
  id: number | null;
  domain: string;
  query: string;
  country: string;
  active: boolean;
}

export interface JobBoardSource {
  id: number | null;
  platform: string;
  token: string;
  company: string;
  active: boolean;
  last_ok?: string | null;
  last_error?: string | null;
}

export interface JobsConfig {
  searches: JobSearch[];
  boards: JobBoardSource[];
  enabled: boolean;
  min_score: number;
  daily_prepare: number;
  followup_days: number;
  profile: string;
  adzuna_app_id: string;
  adzuna_app_key: string;
}

export interface JobsProgress {
  stage: "collect" | "analyse" | "company" | "prepare" | "done";
  label: string;
  done: number;
  total: number;
  finished: boolean;
  found: number;
  added: number;
  analysed: number;
  prepared: number;
  notes: string[];
}

export interface SendResult {
  ok: boolean;
  error: string | null;
  tracked: boolean;
}

export const jobsBoard = () => invoke<JobsBoard>("jobs_board");
export const jobDetail = (id: number) => invoke<JobDetail>("job_detail", { id });
export const jobSave = (input: JobInput) => invoke<number>("job_save", { input });
export const jobSetStatus = (id: number, status: JobStatus) => invoke<void>("job_set_status", { id, status });
export const jobDelete = (id: number) => invoke<void>("job_delete", { id });
export const jobsImportSheet = (path: string) => invoke<number>("jobs_import_sheet", { path });
export const jobsConfig = () => invoke<JobsConfig>("jobs_config");
export const jobsSaveSearch = (row: JobSearch) => invoke<void>("jobs_save_search", { row });
export const jobsDeleteSearch = (id: number) => invoke<void>("jobs_delete_search", { id });
export const jobsSaveBoard = (row: JobBoardSource) => invoke<void>("jobs_save_board", { row });
export const jobsDeleteBoard = (id: number) => invoke<void>("jobs_delete_board", { id });
export const jobsSavePrefs = (prefs: {
  enabled: boolean;
  min_score: number;
  daily_prepare: number;
  followup_days: number;
  adzuna_app_id?: string;
  adzuna_app_key?: string;
}) => invoke<void>("jobs_save_prefs", { prefs });
export const jobsSaveProfile = (profile: string) => invoke<void>("jobs_save_profile", { profile });
export const jobPrepare = (id: number) => invoke<void>("job_prepare", { id });
export const jobSaveDraft = (id: number, subject: string, body: string, followup: string) =>
  invoke<void>("job_save_draft", { id, subject, body, followup });
export const jobSend = (id: number, to: string, subject: string, body: string) =>
  invoke<SendResult>("job_send", { id, to, subject, body });
export const jobSendFollowup = (id: number, to: string, body: string) =>
  invoke<SendResult>("job_send_followup", { id, to, body });
export const jobMarkApplied = (id: number) => invoke<void>("job_mark_applied", { id });
export const jobOpenCv = (id: number, reveal: boolean) => invoke<void>("job_open_cv", { id, reveal });
export const jobsCheckPdf = (path: string, postId?: number | null) =>
  invoke<AtsReport>("jobs_check_pdf", { path, postId: postId ?? null });
export const jobsRun = () => invoke<void>("jobs_run");

/// Pull the open events from the tracking endpoint (same source as the Emails
/// page) so an application shows as opened without a detour. Silent: no
/// tracking configured or an unreachable endpoint simply changes nothing.
export async function syncOpensQuiet(): Promise<number> {
  try {
    const base = (await getSetting("tracking_base_url"))?.trim().replace(/\/$/, "");
    if (!base) return 0;
    const res = await fetch(`${base}/opens.json`, { cache: "no-store" });
    if (!res.ok) return 0;
    const data = await res.json();
    const opens = Array.isArray(data)
      ? data
      : Object.entries(data).map(([token, v]: [string, any]) =>
          typeof v === "string" ? { token, opened_at: v } : { token, opened_at: v?.opened_at, count: v?.count }
        );
    return await applyOpens(opens);
  } catch {
    return 0;
  }
}
