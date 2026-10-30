export type Category = "venue" | "curator" | "lineup" | "major" | "other";

export type Status =
  | "to_contact"
  | "to_evaluate"
  | "low_priority"
  | "contacted"
  | "followed_up"
  | "in_discussion"
  | "confirmed"
  | "declined"
  | "no_answer";

export const STATUSES: Status[] = [
  "to_contact",
  "to_evaluate",
  "low_priority",
  "contacted",
  "followed_up",
  "in_discussion",
  "confirmed",
  "declined",
  "no_answer",
];

export interface Artist {
  id?: number;
  name: string;
  real_name?: string | null;
  tagline?: string | null;
  bio?: string | null;
  genres?: string | null;
  city?: string | null;
  country?: string | null;
  avatar?: string | null;
  email?: string | null;
  phone?: string | null;
  booking_email?: string | null;
  website?: string | null;
  instagram?: string | null;
  soundcloud?: string | null;
  spotify?: string | null;
  apple_music?: string | null;
  beatport?: string | null;
  youtube?: string | null;
  press_quotes?: string | null;
  achievements?: string | null;
  links?: string | null;
  mix_url?: string | null;
  tech_rider?: string | null;
  fee_range?: string | null;
  stats?: string | null;
  audience_cities?: string | null;
  spotify_artist_id?: string | null;
  created_at?: string | null;
  updated_at?: string | null;
}

export interface Contact {
  id?: number;
  artist_id?: number | null;
  category: string;
  priority?: string | null;
  name: string;
  promoter?: string | null;
  venue?: string | null;
  type?: string | null;
  area?: string | null;
  scale?: string | null;
  date?: string | null;
  time?: string | null;
  format?: string | null;
  reason?: string | null;
  contact_channel?: string | null;
  email?: string | null;
  email_status?: string | null;
  status: string;
  first_contact?: string | null;
  follow_up?: string | null;
  notes?: string | null;
  website?: string | null;
  tags?: string | null;
  organisation_id?: number | null;
  /// Read-only, joined by the backend.
  organisation_name?: string | null;
  created_at?: string | null;
  updated_at?: string | null;
}

export interface Organisation {
  id?: number;
  name: string;
  email_domain?: string | null;
  notes?: string | null;
  contact_count?: number;
}

/// A suggested organisation (contacts sharing a non-webmail domain, or one
/// exact address). Nothing is grouped until the user accepts it.
export interface OrgProposal {
  key: string;
  kind: "domain" | "email";
  name: string;
  email_domain: string;
  shared_email?: string | null;
  contact_ids: number[];
  contact_names: string[];
  /// Join this existing organisation instead of creating one.
  organisation_id?: number | null;
  organisation_name?: string | null;
}

export interface OrganisationDetail {
  contacts: Contact[];
  venues: { id: number; nom: string; ville?: string | null; pays?: string | null; contact_id: number }[];
}

export interface Template {
  id?: number;
  name: string;
  subject: string;
  body: string;
  created_at?: string | null;
}

export interface Campaign {
  id?: number;
  name: string;
  purpose?: string | null;
  event_name?: string | null;
  artist_id?: number | null;
  target_date?: string | null;
  status: string;
  color?: string | null;
  created_at?: string | null;
  updated_at?: string | null;
  sent_count: number;
  opened_count: number;
  reached_count?: number;
  replied_count?: number;
  /// Audience (campaign_contacts): still to mail, and all recipients.
  planned_count?: number;
  audience_count?: number;
  /// "standard" | "residency" (residency campaigns open their own page).
  kind?: string;
}

/// grouped: not mailed itself, its organisation got the email through another contact.
export const AUDIENCE_STATES = [
  "planned",
  "sent",
  "grouped",
  "replied",
  "bounced",
  "unsubscribed",
] as const;
export type AudienceState = (typeof AUDIENCE_STATES)[number];

/// A campaign recipient: the contact plus its state in that campaign.
export interface AudienceRow extends Contact {
  state: AudienceState;
  added_at: string;
  suppressed: boolean;
}

export const CAMPAIGN_STATUSES = ["active", "scheduled", "done", "archived"] as const;

export interface Event {
  id?: number;
  artist_id?: number | null;
  contact_id?: number | null;
  title: string;
  venue?: string | null;
  city?: string | null;
  date: string;
  start_time?: string | null;
  end_time?: string | null;
  status: string;
  fee?: number | null;
  notes?: string | null;
  created_at?: string | null;
  updated_at?: string | null;
}

export const EVENT_STATUSES = ["hold", "confirmed", "cancelled"] as const;

export interface Followup {
  contact_id: number;
  name: string;
  email?: string | null;
  venue?: string | null;
  area?: string | null;
  status: string;
  last_email: string;
  days_since: number;
  email_count: number;
  opened: boolean;
  due?: string;
}

/// A contact whose follow-ups were dismissed ("stop following up"), restorable.
export interface DismissedFollowup {
  contact_id: number;
  name: string;
  venue?: string | null;
  area?: string | null;
  status: string;
}

export interface EmailLog {
  id?: number;
  contact_id?: number | null;
  campaign_id?: number | null;
  to_addr: string;
  subject: string;
  body: string;
  status: string;
  error?: string | null;
  track_token?: string | null;
  opened_at?: string | null;
  open_count: number;
  sent_at?: string | null;
  message_id?: string | null;
  replied_at?: string | null;
}

export interface EmailFilter {
  campaign_id?: number | null;
  status?: "sent" | "failed" | null;
  opened?: boolean | null;
  replied?: boolean | null;
  search?: string | null;
}

export interface EmailPage {
  rows: EmailLog[];
  total: number;
}

export const SUPPRESSION_REASONS = ["bounce", "unsubscribe", "manual", "refused"] as const;

export interface Suppression {
  id: number;
  address: string;
  reason: string;
  note?: string | null;
  created_at: string;
}

export interface SendQuota {
  cap: number;
  sent_today: number;
  remaining: number;
}

export interface ReplySyncResult {
  scanned: number;
  replies: number;
  bounces: number;
  unsubscribes: number;
  no_answer: number;
  /// Another sync was already running.
  skipped?: boolean;
}

export interface ReplySyncStatus {
  host: string;
  derived: boolean;
  last_sync?: string | null;
  last_error?: string | null;
  last_result?: ReplySyncResult | null;
}

export interface BudgetItem {
  id?: number;
  category?: string | null;
  item: string;
  min_cost: number;
  max_cost: number;
  actual?: number | null;
  kind: "expense" | "revenue";
  notes?: string | null;
  sort: number;
  /// Revenue line generated for a confirmed Agenda gig (read-only amount).
  event_id?: number | null;
}

export interface Task {
  id?: number;
  period?: string | null;
  title: string;
  done: boolean;
  owner?: string | null;
  due_date?: string | null;
  sort: number;
}

export interface Kpi {
  id?: number;
  artist_id?: number | null;
  goal?: string | null;
  kpi?: string | null;
  target?: string | null;
  actual?: string | null;
  sort: number;
}

export interface SmtpConfig {
  host: string;
  port: number;
  username: string;
  password: string;
  from_name: string;
  from_email: string;
  encryption: "starttls" | "tls" | "none";
}

// ---- Venue Intelligence ----
export interface ViArea {
  id?: number;
  slug: string;
  ra_area_id?: number | null;
  libelle?: string | null;
  pays?: string | null;
  actif: boolean;
  resolved_at?: string | null;
}

export interface ViReferenceArtist {
  id?: number;
  nom: string;
  nom_normalise?: string | null;
  tier: number;
  genres?: string | null;
  actif: boolean;
}

export interface ViRun {
  id: number;
  type_: string;
  statut: string;
  params?: string | null;
  stats?: string | null;
  started_at?: string | null;
  finished_at?: string | null;
  erreur?: string | null;
  created_at?: string | null;
  tasks_total: number;
  tasks_done: number;
  tasks_echec: number;
}

export interface ViVenueRow {
  id: number;
  nom: string;
  ville?: string | null;
  pays?: string | null;
  region_cible?: string | null;
  statut: string;
  priorite: string;
  score_qualif: number;
  nb_events_periode: number;
  nb_evidence: number;
  top_promoter?: string | null;
  ra_url?: string | null;
  site_web?: string | null;
  telephone?: string | null;
  best_email?: string | null;
  nb_emails: number;
  enriched: boolean;
  crm_contact_id?: number | null;
  contacted: boolean;
  played: boolean;
  /** City is one of the targeted artist's audience cities (sorted first server-side). */
  is_target: boolean;
  /** "Books artists like ours" score 0-100 (fit.rs), null without data. */
  fit_score?: number | null;
  fit_detail?: string | null;
  /** "Bon moment pour écrire" (timing.rs). */
  timing?: import("./targets").Timing;
}

export interface CountryCount {
  pays: string;
  n: number;
}

export interface ViVenueStats {
  total: number;
  qualifie: number;
  valide: number;
  candidat: number;
  rejete: number;
  enriched: number;
  with_email: number;
  contacted: number;
  played: number;
  countries: number;
  promotable: number;
  stale_contacts: number;
  top_countries: CountryCount[];
}

export interface ViContactRow {
  id: number;
  type_: string;
  valeur: string;
  role_devine?: string | null;
  score: number;
  source_url?: string | null;
  source_method?: string | null;
  verifie: boolean;
}

export interface ViEvidenceRow {
  artiste: string;
  artiste_tier?: number | null;
  date_event: string;
  titre_event?: string | null;
  source_url: string;
}

export interface ViPromoterRow {
  nom: string;
  nb_events: number;
}

export interface ViVenueFiche {
  id: number;
  nom: string;
  ville?: string | null;
  pays?: string | null;
  adresse?: string | null;
  capacite_est?: number | null;
  site_web?: string | null;
  page_contact?: string | null;
  telephone?: string | null;
  ra_url?: string | null;
  statut: string;
  score_qualif: number;
  nb_events_periode: number;
  notes?: string | null;
  enriched_at?: string | null;
  instagram?: string | null;
  excluded: boolean;
  contacts: ViContactRow[];
  evidence: ViEvidenceRow[];
  promoters: ViPromoterRow[];
}

export interface ViRunTask {
  id: number;
  type: string;
  statut: string;
  tentatives: number;
  erreur?: string | null;
  payload?: string | null;
}

export interface ViRunStats {
  type?: string;
  duration_secs?: number;
  tasks_total?: number;
  tasks_done?: number;
  tasks_echec?: number;
  venues_total?: number;
  venues_new?: number;
  evidence_total?: number;
  evidence_new?: number;
  qualified?: number;
  emails_total?: number;
  venues_with_email?: number;
}

export interface ViRunProgress {
  run_id: number;
  statut: string;
  tasks_total: number;
  tasks_done: number;
  tasks_echec: number;
  venues: number;
  evidence: number;
  message?: string | null;
  /** Stable progress code, translated via `vi_progress.<code>` (see lib/backendMessages). */
  code?: string | null;
  args?: Record<string, unknown> | null;
}

export interface SheetPreview {
  name: string;
  headers: string[];
  rows: string[][];
  total_rows: number;
  header_row: number;
}

export interface FilePreview {
  kind: string;
  sheets: SheetPreview[];
}

export interface ImportResult {
  inserted: number;
  skipped: number;
  duplicates: number;
}

export interface SendResult {
  ok: boolean;
  error?: string | null;
  tracked: boolean;
  /// "suppressed" | "quota" | "bounced" when the guardrails stopped the send.
  code?: string | null;
}

export interface BulkProgress {
  done: number;
  total: number;
  sent: number;
  failed: number;
  skipped: number;
  current?: string | null;
}

export interface BulkResult {
  sent: number;
  failed: number;
  skipped: number;
  errors: string[];
  /// Skipped: on the suppression list.
  suppressed: string[];
  /// Hard SMTP rejection during this send (now suppressed).
  bounced: string[];
  /// Left unsent: daily cap reached.
  deferred: number;
  cap: number;
  /// Skipped: their organisation was already mailed (also counted in skipped).
  grouped?: number;
}

export interface VisaCountry {
  code: string;
  name: string;
  work_rules?: string | null;
  visa_types?: string | null;
  processing_time?: string | null;
  required_docs?: string | null;
  notes?: string | null;
  official_link?: string | null;
  updated_at?: string | null;
}

export interface ChecklistItem {
  label: string;
  done: boolean;
}

export interface VisaDossier {
  id?: number;
  artist_id?: number | null;
  country_code?: string | null;
  country_name?: string | null;
  title: string;
  purpose?: string | null;
  event_date?: string | null;
  entry_date?: string | null;
  status: string;
  checklist?: string | null;
  notes?: string | null;
  event_id?: number | null;
  created_at?: string | null;
  updated_at?: string | null;
}

export interface EventVisa {
  event_id: number;
  country_code?: string | null;
  country_name?: string | null;
  dossier_id?: number | null;
  dossier_status?: string | null;
  needs_visa: boolean;
  /// Confirmed gig: the "no dossier" alert is raised for these only.
  confirmed: boolean;
}

export const VISA_STATUSES = [
  "draft",
  "preparing",
  "submitted",
  "approved",
  "rejected",
] as const;

export interface DashboardStats {
  by_status: Record<string, number>;
  by_category: Record<string, number>;
  total_contacts: number;
  emails_sent: number;
  emails_opened: number;
  budget_min: number;
  budget_max: number;
  budget_actual: number;
  /// Includes the gig fees (Agenda-linked revenue lines).
  revenue_actual: number;
  /// Part of revenue_actual from Agenda gigs: never add it on top.
  gig_fees: number;
  /// Meta Ads spend (Audience), not part of budget_actual.
  ad_spend: number;
  /// revenue_actual - budget_actual - ad_spend.
  net: number;
  tasks_done: number;
  tasks_total: number;
  tasks_overdue: number;
}

// ---- Pulse (Spotify snapshots + Meta spend) ----
export interface PulseReport {
  artists: number;
  tracks: number;
  playlists: number;
  spend_rows: number;
  errors: string[];
  ran: boolean;
}

export interface PulseKpis {
  artist_popularity?: number | null;
  artist_delta7?: number | null;
  artist_followers?: number | null;
  best_track_name?: string | null;
  best_track_popularity?: number | null;
  best_track_delta7?: number | null;
  spend_30d: number;
  cpr_7d?: number | null;
  cpr_30d?: number | null;
  last_snapshot?: string | null;
  /// Spotify left popularity / followers out (restricted for recent apps).
  spotify_metrics_unavailable: boolean;
}

export interface PulsePoint {
  date: string;
  artist_pop?: number | null;
  track_pop?: number | null;
  spend: number;
}

export interface PulseSeries {
  points: PulsePoint[];
  releases: [string, string][];
}

export interface TrackedTrack {
  id: number;
  track_spotify_id: string;
  name?: string | null;
  release_date?: string | null;
  is_active: boolean;
  latest_popularity?: number | null;
  artist_id?: number | null;
}

export interface WatchRow {
  id: number;
  playlist_spotify_id: string;
  name?: string | null;
  owner_name?: string | null;
  notes?: string | null;
  is_active: boolean;
  followers?: number | null;
  delta7?: number | null;
  delta30?: number | null;
  contains_our_track: boolean;
  spark: number[];
}

// ---- Radar (curators / labels) ----
export interface CuratorRow {
  id: number;
  source: string;
  nom: string;
  owner_name?: string | null;
  url?: string | null;
  followers?: number | null;
  genre?: string | null;
  best_email?: string | null;
  nb_emails: number;
  official: boolean;
  enriched: boolean;
  in_pipeline: boolean;
  /** playlist | label | channel */
  kind: string;
  rejected: boolean;
  /** contacts.status of the promoted contact */
  contact_status?: string | null;
  emailed: boolean;
}

export interface RadarStats {
  total: number;
  with_email: number;
  official: number;
  in_pipeline: number;
  addressable: number;
  stale_contacts: number;
  rejected: number;
  playlists: number;
  labels: number;
  channels: number;
}
