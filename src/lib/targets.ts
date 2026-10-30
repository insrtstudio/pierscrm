// Promoters / collectives, fit score ("booke des artistes comme Piers") and
// booking timing ("bon moment pour écrire"). Kept out of api.ts / types.ts so
// this work stays in its own file; see src-tauri/src/commands/{promoters,fit,timing}.rs.
import { invoke } from "@tauri-apps/api/core";

export type TimingState = "now" | "soon" | "later" | "unknown";

export interface Timing {
  state: TimingState;
  lead_weeks: number;
  /** First day it makes sense to write (YYYY-MM-DD). */
  write_from?: string | null;
  /** Month (1-12) the pitch would be for. */
  target_month: number;
  year_round: boolean;
}

export interface FitParts {
  overlap: number;
  genre: number;
  activity: number;
  capacity: number;
}

/** Parsed fit_detail. */
export interface FitDetail {
  score: number;
  matched: string[];
  genre_nights: number;
  nights: number;
  last_date?: string | null;
  capacity?: number | null;
  genre_label: string;
  parts: FitParts;
}

export function parseFit(detail?: string | null): FitDetail | null {
  if (!detail) return null;
  try {
    return JSON.parse(detail) as FitDetail;
  } catch {
    return null;
  }
}

export interface PromoterRow {
  id: number;
  nom: string;
  ville?: string | null;
  pays?: string | null;
  ra_url?: string | null;
  site_web?: string | null;
  nb_events_12m: number;
  nb_events_total: number;
  nb_venues: number;
  venues: string[];
  last_event?: string | null;
  best_email?: string | null;
  nb_emails: number;
  enriched: boolean;
  crm_contact_id?: number | null;
  house_promoter: boolean;
  fit_score?: number | null;
  fit_detail?: string | null;
  timing: Timing;
}

export interface PromoterFilter {
  search?: string;
  pays?: string;
  has_email?: boolean;
  fit_min?: number;
  include_house?: boolean;
  in_pipeline?: boolean;
  min_events?: number;
}

export interface PromoterFiche {
  row: PromoterRow;
  instagram?: string | null;
  lead_source?: string | null;
  venues: { venue_id: number; nom: string; ville?: string | null; nb_events: number; capacite_est?: number | null }[];
  events: { date_event?: string | null; titre?: string | null; venue: string; artists: string[] }[];
  contacts: {
    id: number;
    valeur: string;
    role_devine?: string | null;
    score: number;
    source_url?: string | null;
    source_method?: string | null;
  }[];
}

export type PromoterSort = "fit" | "events" | "recent" | "name";

export const viListPromoters = (filter: PromoterFilter, sort: PromoterSort, limit: number, offset: number) =>
  invoke<PromoterRow[]>("vi_list_promoters", { filter, sort, limit, offset });
export const viCountPromoters = (filter: PromoterFilter) => invoke<number>("vi_count_promoters", { filter });
export const viPromoterDetail = (id: number) => invoke<PromoterFiche>("vi_promoter_detail", { id });
export const viPromotePromoter = (id: number) => invoke<number>("vi_promote_promoter", { id });
export const viStartPromoterEnrich = (p: { ids?: number[]; force?: boolean; minEvents?: number } = {}) =>
  invoke<number>("vi_start_promoter_enrich", {
    ids: p.ids ?? null,
    force: p.force ?? false,
    minEvents: p.minEvents ?? null,
  });

export interface FitStats {
  artist_id?: number | null;
  comparables: number;
  venues_scored: number;
  promoters_scored: number;
}
export interface Comparable {
  id: number;
  nom: string;
  source: "manual" | "colineup" | "genre" | string;
}
export interface ComparableSuggestion {
  nom: string;
  source: string;
  count: number;
}

export const fitRecompute = (artistId?: number) => invoke<FitStats>("fit_recompute", { artistId: artistId ?? null });
export const fitListComparables = (artistId: number) => invoke<Comparable[]>("fit_list_comparables", { artistId });
export const fitAddComparable = (artistId: number, nom: string) =>
  invoke<boolean>("fit_add_comparable", { artistId, nom });
export const fitDeleteComparable = (id: number) => invoke<void>("fit_delete_comparable", { id });
export const fitSuggestComparables = (artistId: number) =>
  invoke<ComparableSuggestion[]>("fit_suggest_comparables", { artistId });
export const fitSeedComparables = (artistId: number, maxGenre?: number) =>
  invoke<number>("fit_seed_comparables", { artistId, maxGenre: maxGenre ?? null });
