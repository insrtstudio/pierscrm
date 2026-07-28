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
