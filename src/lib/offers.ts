// Offers, booking documents, payments and advancing: types and Tauri commands
// (see src-tauri/src/commands/offers).
import { invoke } from "@tauri-apps/api/core";

export const OFFER_STATUSES = ["draft", "sent", "accepted", "declined", "expired"] as const;
export type OfferStatus = (typeof OFFER_STATUSES)[number];
export const FEE_TYPES = ["flat", "versus", "door"] as const;
export const TRAVEL_BY = ["promoter", "artist", "split"] as const;

export interface Offer {
  id?: number | null;
  contact_id?: number | null;
  artist_id?: number | null;
  event_id?: number | null;
  status: OfferStatus | string;
  date?: string | null;
  venue?: string | null;
  city?: string | null;
  set_time?: string | null;
  /// Minutes.
  set_length?: number | null;
  fee_amount?: number | null;
  currency: string;
  fee_type: string;
  deal_terms?: string | null;
  deposit_pct: number;
  travel_by?: string | null;
  travel_from?: string | null;
  travel_to?: string | null;
  hotel?: string | null;
  meals?: string | null;
  guest_list?: number | null;
  tech_requirements?: string | null;
  answer_by?: string | null;
  language: "fr" | "en" | string;
  notes?: string | null;
  sent_at?: string | null;
  accepted_at?: string | null;
  deposit_due?: string | null;
  deposit_paid_at?: string | null;
  balance_due?: string | null;
  balance_paid_at?: string | null;
  created_at?: string | null;
  updated_at?: string | null;
  contact_name?: string | null;
  contact_email?: string | null;
  artist_name?: string | null;
}

export interface SavedOffer {
  id: number;
  event_id?: number | null;
}

export interface Payment {
  offer_id: number;
  event_id?: number | null;
  contact_id?: number | null;
  title: string;
  gig_date?: string | null;
  part: "deposit" | "balance" | string;
  amount: number;
  currency: string;
  due?: string | null;
  paid_at?: string | null;
  overdue: boolean;
  invoice_number?: string | null;
}

export interface Invoice {
  id: number;
  offer_id?: number | null;
  part: string;
  number: string;
  amount: number;
  currency: string;
  issued_on: string;
  due_on?: string | null;
  file_path?: string | null;
}

export interface AdvItem {
  key: string;
  offset: number;
  due: string;
  /// null (open), "done" or "na".
  state?: string | null;
  done_at?: string | null;
}

export interface GigAdvancing {
  event_id: number;
  title: string;
  date: string;
  venue?: string | null;
  city?: string | null;
  contact_id?: number | null;
  offer_id?: number | null;
  items: AdvItem[];
  email_due: string;
  email_sent_at?: string | null;
}

export const DOC_KINDS = ["contract", "rider", "invoice_deposit", "invoice_balance"] as const;
export type DocKind = (typeof DOC_KINDS)[number];

export interface GeneratedDoc {
  kind: DocKind | string;
  path: string;
  file_name: string;
  invoice_number?: string | null;
}

export interface EmailDraft {
  contact_id?: number | null;
  to: string;
  subject: string;
  body: string;
  attachments: GeneratedDoc[];
}

export type DraftPurpose = "offer" | "contract" | "invoice_deposit" | "invoice_balance";

export const listOffers = (p: { contactId?: number | null; eventId?: number | null } = {}) =>
  invoke<Offer[]>("list_offers", { contactId: p.contactId ?? null, eventId: p.eventId ?? null });
export const getOffer = (id: number) => invoke<Offer>("get_offer", { id });
export const newOfferForContact = (contactId: number) =>
  invoke<Offer>("new_offer_for_contact", { contactId });
export const saveOffer = (offer: Offer) => invoke<SavedOffer>("save_offer", { offer });
export const setOfferStatus = (id: number, status: string) =>
  invoke<SavedOffer>("set_offer_status", { id, status });
export const deleteOffer = (id: number) => invoke<void>("delete_offer", { id });

export const listOfferPayments = () => invoke<Payment[]>("list_offer_payments");
export const setOfferPayment = (offerId: number, part: string, paidAt: string | null) =>
  invoke<void>("set_offer_payment", { offerId, part, paidAt });
export const listOfferInvoices = (offerId: number) =>
  invoke<Invoice[]>("list_offer_invoices", { offerId });

export const listAdvancing = (until?: string) =>
  invoke<GigAdvancing[]>("list_advancing", { until: until ?? null });
export const getEventAdvancing = (eventId: number) =>
  invoke<GigAdvancing | null>("get_event_advancing", { eventId });
export const setAdvancingItem = (eventId: number, item: string, value: string | null) =>
  invoke<void>("set_advancing_item", { eventId, item, value });

export const generateOfferDocument = (offerId: number, kind: DocKind) =>
  invoke<GeneratedDoc>("generate_offer_document", { offerId, kind });
export const listOfferDocuments = (offerId: number) =>
  invoke<GeneratedDoc[]>("list_offer_documents", { offerId });
export const openOfferDocument = (path: string) => invoke<void>("open_offer_document", { path });
export const offerEmailDraft = (offerId: number, purpose: DraftPurpose) =>
  invoke<EmailDraft>("offer_email_draft", { offerId, purpose });
export const advancingEmailDraft = (eventId: number) =>
  invoke<EmailDraft>("advancing_email_draft", { eventId });

/// Every query an offer change can affect (Agenda event, budget line, contact
/// status, payments, advancing).
export const OFFER_QUERY_KEYS = [
  "offers",
  "offer_payments",
  "advancing",
  "events",
  "budget",
  "contacts",
  "dashboard",
  "kpis",
  "insrt_gig",
];

export function money(amount: number | null | undefined, currency = "EUR", lang = "fr"): string {
  if (amount == null || Number.isNaN(amount)) return "";
  try {
    return new Intl.NumberFormat(lang.startsWith("fr") ? "fr-FR" : "en-GB", {
      style: "currency",
      currency: currency || "EUR",
      maximumFractionDigits: amount % 1 === 0 ? 0 : 2,
    }).format(amount);
  } catch {
    return `${amount} ${currency}`;
  }
}

export function todayIso(): string {
  const d = new Date();
  const p = (n: number) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())}`;
}
