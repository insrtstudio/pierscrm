export type Category = "venue" | "lineup" | "major" | "other";

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
  created_at?: string | null;
  updated_at?: string | null;
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
}

export interface SendResult {
  ok: boolean;
  error?: string | null;
  tracked: boolean;
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
  created_at?: string | null;
  updated_at?: string | null;
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
  budget_min: number;
  budget_max: number;
  budget_actual: number;
  revenue_actual: number;
  tasks_done: number;
  tasks_total: number;
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
  nb_tracks?: number | null;
  genre?: string | null;
  statut: string;
  score: number;
  editorial: boolean;
  site_web?: string | null;
  best_email?: string | null;
  nb_emails: number;
  enriched: boolean;
  in_pipeline: boolean;
}

export interface RadarStats {
  total: number;
  qualifie: number;
  with_email: number;
  editorial: number;
  in_pipeline: number;
  genres: [string, number][];
}
