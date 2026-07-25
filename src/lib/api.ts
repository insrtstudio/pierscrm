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
export const sendBulk = (p: {
  campaign_id?: number | null;
  contact_ids: number[];
  subject: string;
  body: string;
}) =>
  invoke<BulkResult>("send_bulk", {
    campaignId: p.campaign_id ?? null,
    contactIds: p.contact_ids,
    subject: p.subject,
    body: p.body,
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
