// insrt.studio links of the CRM: EPK link activity of a contact and the
// confirmed gigs published on the artist page (see src-tauri/src/commands/
// residency/report.rs and src-tauri/src/commands/insrt_gigs.rs).
import { invoke } from "@tauri-apps/api/core";

export interface TrackListen {
  title: string;
  plays: number;
  seconds: number;
  completion?: number | null;
  last_at: string;
}

export interface FeedbackItem {
  at: string;
  track_title?: string | null;
  rating?: number | null;
  comment?: string | null;
}

export interface ActivityItem {
  at: string;
  kind: "view" | "listen" | "feedback" | "download" | string;
  track_title?: string | null;
  seconds?: number | null;
  completion?: number | null;
  comment?: string | null;
  campaign_id?: number | null;
}

export interface ContactActivity {
  contact_id: number;
  link_url?: string | null;
  views: number;
  first_view_at?: string | null;
  last_view_at?: string | null;
  downloads: number;
  listen_seconds: number;
  hot: boolean;
  tracks: TrackListen[];
  feedback: FeedbackItem[];
  recent: ActivityItem[];
  last_at?: string | null;
  synced_at?: string | null;
}

export const contactActivity = (contactId: number) =>
  invoke<ContactActivity>("insrt_contact_activity", { contactId });
export const syncInsrtActivity = () => invoke<number>("insrt_sync_activity");

export interface GigState {
  event_id: number;
  publish: boolean;
  country?: string | null;
  ticket_url?: string | null;
  pushed_at?: string | null;
  last_error?: string | null;
  /** The site shows this date right now. */
  live: boolean;
  /** What the site shows matches the CRM. */
  in_sync: boolean;
}

export const getInsrtGig = (eventId: number) => invoke<GigState>("insrt_gig_get", { eventId });
export const setInsrtGig = (
  eventId: number,
  v: { publish: boolean; country?: string | null; ticket_url?: string | null }
) =>
  invoke<GigState>("insrt_gig_set", {
    eventId,
    publish: v.publish,
    country: v.country || null,
    ticketUrl: v.ticket_url || null,
  });

/** "4 min 05" / "45 s" */
export function listenTime(seconds: number): string {
  const s = Math.round(seconds);
  if (s < 60) return `${s} s`;
  const m = Math.floor(s / 60);
  const r = s % 60;
  return r ? `${m} min ${String(r).padStart(2, "0")}` : `${m} min`;
}
