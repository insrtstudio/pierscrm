// Chaining dates and bringing clubs back: tour routing around a gig, the
// thank-you / rebook loop after it, the night's assets and the "a joué à"
// list (see src-tauri/src/commands/routing.rs). Drafts are only returned:
// they open in the compose window, nothing is sent from here.
import { invoke } from "@tauri-apps/api/core";

export type Warmth = "discussion" | "replied" | "booked" | "contacted" | "cold" | "silent" | "new";

export interface RouteSuggestion {
  kind: "contact" | "venue";
  contact_id?: number | null;
  venue_id?: number | null;
  name: string;
  city: string;
  country?: string | null;
  distance_km: number;
  warmth: number;
  warmth_label: Warmth;
  status?: string | null;
  score: number;
  has_email: boolean;
  previously_booked: boolean;
}

export interface TourPlan {
  event_id: number;
  title: string;
  venue?: string | null;
  city?: string | null;
  date: string;
  located: boolean;
  radius_km: number;
  free_days: string[];
  best_day?: string | null;
  suggestions: RouteSuggestion[];
}

export interface TourAction {
  event_id: number;
  title: string;
  date: string;
  city?: string | null;
  free_days: string[];
  count: number;
  warm: number;
  top: string[];
}

export interface PitchDraft {
  subject: string;
  body: string;
  lang: string;
  generated_by: string;
  note?: string | null;
}

export type FollowupKind = "thanks" | "rebook";

export interface GigFollowup {
  kind: FollowupKind;
  event_id: number;
  contact_id: number;
  contact_name: string;
  title: string;
  venue?: string | null;
  city?: string | null;
  gig_date: string;
  due: string;
}

export interface GigRecap {
  event_id: number;
  photos: string;
  videos: string;
  attendance: string;
  quote: string;
  quote_author: string;
  played_at: boolean;
  updated_at?: string | null;
}

export interface PlayedAt {
  event_id: number;
  venue: string;
  city?: string | null;
  date: string;
  line_fr: string;
  line_en: string;
}

export interface PushResult {
  id: string;
  title: string;
  created: boolean;
  count: number;
}

export const tourPlan = (eventId: number, radiusKm?: number) =>
  invoke<TourPlan>("routing_tour_plan", { eventId, radiusKm: radiusKm ?? null });
export const tourActions = () => invoke<TourAction[]>("routing_tour_actions");
export const pitchDraft = (eventId: number, s: Pick<RouteSuggestion, "contact_id" | "venue_id">, day: string) =>
  invoke<PitchDraft>("routing_pitch_draft", {
    eventId,
    contactId: s.contact_id ?? null,
    venueId: s.venue_id ?? null,
    day,
  });
export const gigFollowups = () => invoke<GigFollowup[]>("routing_followups");
export const followupDraft = (eventId: number, kind: FollowupKind) =>
  invoke<PitchDraft>("routing_followup_draft", { eventId, kind });
export const followupDone = (eventId: number, kind: FollowupKind | "routing") =>
  invoke<void>("routing_followup_done", { eventId, kind });
export const getGigRecap = (eventId: number) => invoke<GigRecap>("gig_recap_get", { eventId });
export const saveGigRecap = (recap: GigRecap) => invoke<GigRecap>("gig_recap_save", { recap });
export const playedAtList = (artistId?: number | null) =>
  invoke<PlayedAt[]>("played_at_list", { artistId: artistId ?? null });
export const pushPlayedAt = (artistId?: number | null) =>
  invoke<PushResult>("played_at_push", { artistId: artistId ?? null });
