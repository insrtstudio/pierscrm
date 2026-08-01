import { invoke } from "@tauri-apps/api/core";
import type {
  Artist,
  BudgetItem,
  BulkResult,
  Campaign,
  Contact,
  DashboardStats,
  EmailLog,
  Event,
  FilePreview,
  Followup,
  ImportResult,
  Kpi,
  SendResult,
  SmtpConfig,
  Task,
  Template,
  VisaCountry,
  VisaDossier,
} from "./types";

// ---- Artists ----
export const listArtists = () => invoke<Artist[]>("list_artists");
export const getArtist = (id: number) => invoke<Artist | null>("get_artist", { id });
export const saveArtist = (artist: Artist) => invoke<number>("save_artist", { artist });
export const deleteArtist = (id: number) => invoke<void>("delete_artist", { id });
export const imageToDataUrl = (path: string) =>
  invoke<string>("image_to_data_url", { path });

// ---- Contacts ----
export const listContacts = (p: {
  category?: string;
  status?: string;
  search?: string;
  artist_id?: number | null;
}) =>
  invoke<Contact[]>("list_contacts", {
    category: p.category,
    status: p.status,
    search: p.search,
    artistId: p.artist_id ?? null,
  });
export const getContact = (id: number) => invoke<Contact | null>("get_contact", { id });
export const createContact = (contact: Contact) => invoke<number>("create_contact", { contact });
export const updateContact = (contact: Contact) => invoke<void>("update_contact", { contact });
export const updateContactStatus = (id: number, status: string) =>
  invoke<void>("update_contact_status", { id, status });
export const deleteContact = (id: number) => invoke<void>("delete_contact", { id });
export const deleteContacts = (ids: number[]) => invoke<void>("delete_contacts", { ids });

// ---- Import ----
export const previewFile = (path: string) => invoke<FilePreview>("preview_file", { path });
export const importFile = (req: {
  path: string;
  sheet?: string;
  category: string;
  mapping: Record<string, number>;
  skip_rows: number;
}) => invoke<ImportResult>("import_file", { req });

// ---- Email ----
export const getSmtpConfig = () => invoke<SmtpConfig>("get_smtp_config");
export const saveSmtpConfig = (config: SmtpConfig) => invoke<void>("save_smtp_config", { config });
export const testSmtp = () => invoke<boolean>("test_smtp");
export const sendEmail = (p: {
  contact_id?: number | null;
  campaign_id?: number | null;
  to: string;
  subject: string;
  body: string;
}) =>
  invoke<SendResult>("send_email", {
    contactId: p.contact_id ?? null,
    campaignId: p.campaign_id ?? null,
    to: p.to,
    subject: p.subject,
    body: p.body,
  });
export const listEmails = (contactId?: number) =>
  invoke<EmailLog[]>("list_emails", { contactId: contactId ?? null });
export const listFollowups = () => invoke<Followup[]>("list_followups");
export const dismissFollowup = (contactId: number) =>
  invoke<void>("dismiss_followup", { contactId });
export const sendBulk = (p: {
  campaign_id?: number | null;
  contact_ids: number[];
  subject: string;
  body: string;
  extra_vars?: Record<string, string>;
}) =>
  invoke<BulkResult>("send_bulk", {
    campaignId: p.campaign_id ?? null,
    contactIds: p.contact_ids,
    subject: p.subject,
    body: p.body,
    extraVars: p.extra_vars ?? {},
  });
export const applyOpens = (
  opens: { token: string; opened_at?: string; count?: number }[]
) => invoke<number>("apply_opens", { opens });

// ---- Visa ----
export const listVisaCountries = () =>
  invoke<VisaCountry[]>("list_visa_countries");
export const saveVisaCountry = (country: VisaCountry) =>
  invoke<void>("save_visa_country", { country });
export const deleteVisaCountry = (code: string) =>
  invoke<void>("delete_visa_country", { code });
export const listDossiers = () => invoke<VisaDossier[]>("list_dossiers");
export const saveDossier = (dossier: VisaDossier) =>
  invoke<number>("save_dossier", { dossier });
export const deleteDossier = (id: number) => invoke<void>("delete_dossier", { id });

// ---- Campaigns ----
export const listCampaigns = () => invoke<Campaign[]>("list_campaigns");
export const saveCampaign = (campaign: Campaign) =>
  invoke<number>("save_campaign", { campaign });
export const deleteCampaign = (id: number) => invoke<void>("delete_campaign", { id });

// ---- Events ----
export const listEvents = (p: {
  from?: string;
  to?: string;
  artist_id?: number | null;
}) =>
  invoke<Event[]>("list_events", {
    from: p.from ?? null,
    to: p.to ?? null,
    artistId: p.artist_id ?? null,
  });
export const saveEvent = (event: Event) => invoke<number>("save_event", { event });
export const deleteEvent = (id: number) => invoke<void>("delete_event", { id });

// ---- Templates ----
export const listTemplates = () => invoke<Template[]>("list_templates");
export const saveTemplate = (template: Template) => invoke<number>("save_template", { template });
export const deleteTemplate = (id: number) => invoke<void>("delete_template", { id });

// ---- Budget ----
export const listBudget = () => invoke<BudgetItem[]>("list_budget");
export const saveBudgetItem = (item: BudgetItem) => invoke<number>("save_budget_item", { item });
export const deleteBudgetItem = (id: number) => invoke<void>("delete_budget_item", { id });

// ---- Tasks ----
export const listTasks = () => invoke<Task[]>("list_tasks");
export const saveTask = (task: Task) => invoke<number>("save_task", { task });
export const deleteTask = (id: number) => invoke<void>("delete_task", { id });

// ---- KPIs ----
export const listKpis = (artistId?: number | null) =>
  invoke<Kpi[]>("list_kpis", { artistId: artistId ?? null });
export const saveKpi = (kpi: Kpi) => invoke<number>("save_kpi", { kpi });
export const deleteKpi = (id: number) => invoke<void>("delete_kpi", { id });

// ---- Settings + Dashboard ----
export const getSetting = (key: string) => invoke<string | null>("get_setting", { key });
export const setSetting = (key: string, value: string) =>
  invoke<void>("set_setting", { key, value });
export const dashboardStats = () => invoke<DashboardStats>("dashboard_stats");

// ---- Venue Intelligence ----
import type {
  ViArea,
  ViReferenceArtist,
  ViRun,
  ViRunTask,
  ViVenueFiche,
  ViVenueRow,
  ViVenueStats,
} from "./types";

export const viListAreas = () => invoke<ViArea[]>("vi_list_areas");
export const viSaveArea = (area: ViArea) => invoke<number>("vi_save_area", { area });
export const viResolveArea = (id: number) => invoke<number | null>("vi_resolve_area", { id });
export const viResolveAllAreas = () => invoke<number>("vi_resolve_all_areas");
export const viListReferenceArtists = () =>
  invoke<ViReferenceArtist[]>("vi_list_reference_artists");
export const viSaveReferenceArtist = (artist: ViReferenceArtist) =>
  invoke<number>("vi_save_reference_artist", { artist });
export const viDeleteReferenceArtist = (id: number) =>
  invoke<void>("vi_delete_reference_artist", { id });
export const viStartHarvest = (p: {
  area_ids: number[];
  year_from: number;
  year_to: number;
}) =>
  invoke<number>("vi_start_harvest", {
    areaIds: p.area_ids,
    yearFrom: p.year_from,
    yearTo: p.year_to,
  });
export const viResumeRun = (runId: number) => invoke<void>("vi_resume_run", { runId });
export const viStopRun = (runId: number) => invoke<void>("vi_stop_run", { runId });
export const viRunTasks = (runId: number, onlyFailed = true) =>
  invoke<ViRunTask[]>("vi_run_tasks", { runId, onlyFailed });
export const viListRuns = () => invoke<ViRun[]>("vi_list_runs");
export const viListVenues = (p: {
  statut?: string;
  pays?: string;
  search?: string;
  has_email?: boolean;
  has_phone?: boolean;
  has_website?: boolean;
  contacted?: boolean;
  played?: boolean;
  limit?: number;
  offset?: number;
}) =>
  // Tauri maps camelCase JS -> snake_case Rust params, so multi-word args MUST be
  // sent camelCased (hasEmail), not snake_cased, or they silently stay None.
  invoke<ViVenueRow[]>("vi_list_venues", {
    statut: p.statut,
    pays: p.pays,
    search: p.search,
    hasEmail: p.has_email,
    hasPhone: p.has_phone,
    hasWebsite: p.has_website,
    contacted: p.contacted,
    played: p.played,
    limit: p.limit,
    offset: p.offset,
  });
export const viVenueStats = () => invoke<ViVenueStats>("vi_venue_stats");
export const viStartEnrich = (force?: boolean) =>
  invoke<number>("vi_start_enrich", { force: force ?? false });
export const viVenueDetail = (id: number) =>
  invoke<ViVenueFiche>("vi_venue_detail", { id });

// ---- Pulse ----
import type { PulseKpis, PulseReport, PulseSeries, TrackedTrack, WatchRow } from "./types";

export const pulseSnapshot = () => invoke<PulseReport>("pulse_snapshot");
export const pulseKpis = (artistSpotifyId?: string) =>
  invoke<PulseKpis>("pulse_kpis", { artistSpotifyId: artistSpotifyId ?? null });
export const pulseSeries = (p: { artist?: string; track?: string; days: number }) =>
  invoke<PulseSeries>("pulse_series", {
    artistSpotifyId: p.artist ?? null,
    trackSpotifyId: p.track ?? null,
    days: p.days,
  });
export const pulseTrackedList = () => invoke<TrackedTrack[]>("pulse_tracked_list");
export const pulseTrackedAdd = (input: string, artistId?: number) =>
  invoke<void>("pulse_tracked_add", { input, artistId: artistId ?? null });
export const pulseTrackedToggle = (id: number, active: boolean) =>
  invoke<void>("pulse_tracked_toggle", { id, active });
export const pulseWatchlist = () => invoke<WatchRow[]>("pulse_watchlist");
export const pulseWatchlistAdd = (input: string) =>
  invoke<void>("pulse_watchlist_add", { input });
export const pulseWatchlistToggle = (id: number, active: boolean) =>
  invoke<void>("pulse_watchlist_toggle", { id, active });
