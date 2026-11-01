// Instagram: handles and the assisted DM (src-tauri/src/commands/social.rs).
// Nothing is ever sent from the app: the DM is drafted here, pasted and sent
// by hand on Instagram, then recorded with "DM envoyé".
import { invoke } from "@tauri-apps/api/core";

/** One DM target: exactly one of the ids is enough. */
export interface DmTargetRef {
  contactId?: number | null;
  venueId?: number | null;
  promoterId?: number | null;
}

export interface DmDraft {
  body: string;
  lang: string;
  handle?: string | null;
  link?: string | null;
  /** The model that wrote it, or "template". */
  generated_by: string;
  name: string;
  contact_id?: number | null;
  venue_id?: number | null;
  promoter_id?: number | null;
  note?: string | null;
}

export interface SocialTouch {
  id: number;
  contact_id?: number | null;
  venue_id?: number | null;
  promoter_id?: number | null;
  channel: string;
  handle?: string | null;
  body?: string | null;
  /** UTC, "YYYY-MM-DD HH:MM:SS". */
  sent_at: string;
  name: string;
  status?: string | null;
}

const ids = (r: DmTargetRef) => ({
  contactId: r.contactId ?? null,
  venueId: r.venueId ?? null,
  promoterId: r.promoterId ?? null,
});

export const socialDmDraft = (r: DmTargetRef) => invoke<DmDraft>("social_dm_draft", ids(r));

export const socialRecordDm = (r: DmTargetRef, handle: string | null, body: string) =>
  invoke<SocialTouch>("social_record_dm", { ...ids(r), handle, body });

/** DMs sent between two days (YYYY-MM-DD, inclusive), or for one target. */
export const listSocialTouches = (p: { since?: string; until?: string } & DmTargetRef = {}) =>
  invoke<SocialTouch[]>("list_social_touches", { since: p.since ?? null, until: p.until ?? null, ...ids(p) });

export const socialSetInstagram = (r: { venueId?: number; promoterId?: number }, handle: string | null) =>
  invoke<string | null>("social_set_instagram", { venueId: r.venueId ?? null, promoterId: r.promoterId ?? null, handle });

/** Queue the Instagram search over collectives (selection, else every enriched one without a handle). */
export const viStartPromoterInstagram = (p: { ids?: number[]; minEvents?: number } = {}) =>
  invoke<number>("vi_start_promoter_instagram", { ids: p.ids ?? null, minEvents: p.minEvents ?? null });

/** A handle as typed ("@Club", "instagram.com/club/") down to "club"; null when empty. */
export function cleanHandle(raw?: string | null): string | null {
  if (!raw) return null;
  let s = raw.trim();
  const m = s.match(/instagr(?:am\.com|\.am)\/(?:_u\/)?([A-Za-z0-9_.]+)/i);
  if (m) s = m[1];
  s = s.replace(/^@/, "").replace(/\/+$/, "").toLowerCase();
  return /^[a-z0-9_.]{1,30}$/.test(s) ? s : null;
}

export const instagramProfileUrl =(handle: string) => `https://www.instagram.com/${handle}/`;
/** Opens the conversation with the account (falls back to the profile on Instagram's side). */
export const instagramDmUrl = (handle: string) => `https://ig.me/m/${handle}`;
