import { useEffect, useState } from "react";
import { keepPreviousData, useInfiniteQuery, useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import type { TFunction } from "i18next";
import { openUrl } from "@tauri-apps/plugin-opener";
import clsx from "clsx";
import { ExternalLink, Globe, Mail, RefreshCw, ShieldCheck, Sparkles, UserPlus, Users } from "lucide-react";
import { backendError } from "../lib/backendMessages";
import { formatDate } from "../lib/format";
import { useDebounced } from "../lib/hooks";
import {
  fitRecompute,
  viCountPromoters,
  viListPromoters,
  viPromotePromoter,
  viPromotePromoters,
  viPromoterDetail,
  viStartPromoterEnrich,
  viClassifyPromoters,
  viSetPromoterType,
  ORG_TYPES,
  type OrgType,
  type PromoterFilter,
  type PromoterRow,
  type PromoterSort,
  type PromoteSummary,
} from "../lib/targets";
import { viVenueStats } from "../lib/api";
import { EmptyState } from "./Layout";
import { Modal, useToast } from "./ui";
import { AddToCampaign } from "./BulkActions";
import { FitLine, FitScore, TimingBadge } from "./FitTiming";
import { DmPanel, IgHandle, InstagramSearchButton } from "./InstagramDm";

const PAGE = 100;

/// "type_label_" / "one_label_": `label` alone is already the "Type" caption.
const typeKey = (prefix: "type" | "one", t: OrgType) => `promoters.${prefix}_${t === "label" ? "label_" : t}`;

/// What a row really is, when it is not a party collective.
function OrgTypeBadge({ type, note, source }: { type?: OrgType | null; note?: string | null; source?: string | null }) {
  const { t } = useTranslation();
  if (!type || type === "collectif") return null;
  const by = source ? t(`promoters.type_by_${source}`) : "";
  return (
    <span className="badge mt-0.5 bg-warn/15 text-warn" title={[by, note].filter(Boolean).join(" : ")}>
      {t(typeKey("one", type))}
    </span>
  );
}

/// Toast for a promoted selection: "3 added to Outreach · 1 was already there".
export function promoteSummaryText(t: TFunction, s: PromoteSummary) {
  return [
    t("scouting.promoted", { count: s.added }),
    s.already > 0 && t("scouting.already", { count: s.already }),
    s.no_email > 0 && t("scouting.no_email", { count: s.no_email }),
  ]
    .filter(Boolean)
    .join(" · ");
}

/// Discovery › Promoters: collectives and promoters seen on RA, with their
/// nights over 12 months, the rooms they use, the address found for them, the
/// fit score and the right moment to write. Promoting one creates a contact of
/// category "promoter" that campaigns and residencies can target.
export function PromotersTab({ initialEmail = false }: { initialEmail?: boolean } = {}) {
  const { t } = useTranslation();
  const toast = useToast();
  const qc = useQueryClient();
  const [search, setSearch] = useState("");
  const dSearch = useDebounced(search, 250);
  const [pays, setPays] = useState("all");
  const [hasEmail, setHasEmail] = useState(initialEmail);
  const [fitOnly, setFitOnly] = useState(false);
  // Only real party collectives by default (org_type.rs sorts out clubs,
  // labels, agencies, festivals and media listed as promoters on RA).
  const [orgType, setOrgType] = useState<OrgType | "all">("collectif");
  const [sort, setSort] = useState<PromoterSort>("fit");
  const [openId, setOpenId] = useState<number | null>(null);
  const [selected, setSelected] = useState<Set<number>>(new Set());

  const filter: PromoterFilter = {
    search: dSearch || undefined,
    pays,
    has_email: hasEmail || undefined,
    fit_min: fitOnly ? 40 : undefined,
    org_type: orgType,
  };
  useEffect(() => setSelected(new Set()), [dSearch, pays, hasEmail, fitOnly, orgType, sort]);

  const { data: pages, fetchNextPage, hasNextPage, isFetchingNextPage, isLoading } = useInfiniteQuery({
    queryKey: ["vi_promoters", filter, sort],
    queryFn: ({ pageParam }) => viListPromoters(filter, sort, PAGE, pageParam),
    initialPageParam: 0,
    getNextPageParam: (last, all) => (last.length < PAGE ? undefined : all.length * PAGE),
    placeholderData: keepPreviousData,
  });
  const rows = pages?.pages.flat() ?? [];
  const { data: total } = useQuery({
    queryKey: ["vi_promoters", "count", filter],
    queryFn: () => viCountPromoters(filter),
    placeholderData: keepPreviousData,
  });
  const { data: stats } = useQuery({ queryKey: ["vi_venue_stats"], queryFn: viVenueStats });

  const refresh = () => {
    qc.invalidateQueries({ queryKey: ["vi_promoters"] });
    qc.invalidateQueries({ queryKey: ["contacts"] });
  };
  const promote = useMutation({
    mutationFn: viPromotePromoter,
    onSuccess: () => {
      refresh();
      toast(t("scouting.promoter_promoted"), "ok");
    },
    onError: (e) => toast(backendError(e), "error"),
  });
  const promoteSelected = useMutation({
    mutationFn: () => viPromotePromoters([...selected]),
    onSuccess: (s) => {
      setSelected(new Set());
      refresh();
      toast(promoteSummaryText(t, s), "ok");
    },
    onError: (e) => toast(backendError(e), "error"),
  });
  // Every reachable collective of the current filter (not just the loaded
  // page), minus those already in Outreach.
  const promoteAll = useMutation({
    mutationFn: async () => {
      const all = await viListPromoters(filter, sort, 100_000, 0);
      const ids = all.filter((p) => !p.crm_contact_id && (!!p.best_email || !!p.instagram)).map((p) => p.id);
      return ids.length ? viPromotePromoters(ids) : null;
    },
    onSuccess: (s) => {
      if (!s) return toast(t("scouting.promote_all_none"), "ok");
      setSelected(new Set());
      refresh();
      toast(promoteSummaryText(t, s), "ok");
    },
    onError: (e) => toast(backendError(e), "error"),
  });
  const classify = useMutation({
    mutationFn: () => viClassifyPromoters(false),
    onSuccess: (s) => {
      refresh();
      toast(
        s.checked
          ? t("promoters.classified", { checked: s.checked, collectif: s.collectif, other: s.other, ig: s.instagram_rejected })
          : t("promoters.classified_none"),
        "ok"
      );
    },
    onError: (e) => toast(backendError(e), "error"),
  });
  const enrich = useMutation({
    mutationFn: () => viStartPromoterEnrich(selected.size ? { ids: [...selected] } : {}),
    onSuccess: () => toast(t("promoters.enrich_started"), "ok"),
    onError: (e) => toast(backendError(e), "error"),
  });
  const recompute = useMutation({
    mutationFn: () => fitRecompute(),
    onSuccess: (s) => {
      refresh();
      qc.invalidateQueries({ queryKey: ["vi_venues"] });
      toast(t("fit.recomputed", { venues: s.venues_scored, promoters: s.promoters_scored }), "ok");
    },
    onError: (e) => toast(backendError(e), "error"),
  });

  const selectable = (p: PromoterRow) => !!p.crm_contact_id || !!p.best_email || !!p.instagram;
  const selectedContactIds = async () => {
    const ids: number[] = [];
    for (const p of rows) {
      if (selected.has(p.id) && selectable(p)) ids.push(p.crm_contact_id ?? (await viPromotePromoter(p.id)));
    }
    return ids;
  };
  const toggle = (id: number) =>
    setSelected((s) => {
      const n = new Set(s);
      n.has(id) ? n.delete(id) : n.add(id);
      return n;
    });

  const chips = [
    { on: hasEmail, set: setHasEmail, label: t("venues.s_with_email") },
    { on: fitOnly, set: setFitOnly, label: t("fit.chip") },
  ];

  return (
    <div className="space-y-4">
      <p className="text-xs text-fg-subtle">{t("promoters.help")}</p>
      <div className="flex flex-wrap items-center gap-2">
        <input
          className="input max-w-xs flex-1"
          placeholder={t("promoters.search")}
          value={search}
          onChange={(e) => setSearch(e.target.value)}
        />
        <select className="input w-auto" value={pays} onChange={(e) => setPays(e.target.value)}>
          <option value="all">{t("venues.all_countries")}</option>
          {(stats?.top_countries ?? []).map((c) => (
            <option key={c.pays} value={c.pays}>
              {c.pays}
            </option>
          ))}
        </select>
        <select
          className="input w-auto"
          value={orgType}
          onChange={(e) => setOrgType(e.target.value as OrgType | "all")}
          aria-label={t("promoters.type_label")}
        >
          {ORG_TYPES.map((ty) => (
            <option key={ty} value={ty}>
              {t(typeKey("type", ty))}
            </option>
          ))}
          <option value="all">{t("promoters.type_all")}</option>
        </select>
        <select className="input w-auto" value={sort} onChange={(e) => setSort(e.target.value as PromoterSort)}>
          {(["fit", "events", "recent", "name"] as const).map((s) => (
            <option key={s} value={s}>
              {t(`promoters.sort_${s}`)}
            </option>
          ))}
        </select>
        <button
          className="btn-outline ml-auto"
          onClick={() => classify.mutate()}
          disabled={classify.isPending}
          title={t("promoters.classify_hint")}
        >
          <ShieldCheck size={15} className={classify.isPending ? "animate-pulse" : ""} />
          {t("promoters.classify")}
        </button>
        <button className="btn-outline" onClick={() => recompute.mutate()} disabled={recompute.isPending}>
          <RefreshCw size={15} className={recompute.isPending ? "animate-spin" : ""} />
          {t("fit.recompute")}
        </button>
        <button
          className="btn-outline"
          onClick={() => promoteAll.mutate()}
          disabled={promoteAll.isPending || !total}
          title={t("scouting.promote_all_hint")}
        >
          <UserPlus size={15} />
          {t("scouting.promote_all")}
        </button>
        <InstagramSearchButton ids={[...selected]} />
        <button
          className="btn-primary"
          onClick={() => enrich.mutate()}
          disabled={enrich.isPending}
          title={t("promoters.enrich_hint")}
        >
          <Sparkles size={15} />
          {selected.size ? t("promoters.enrich_selected", { count: selected.size }) : t("promoters.enrich")}
        </button>
      </div>

      <div className="flex flex-wrap items-center gap-2">
        {chips.map((c, i) => (
          <button
            key={i}
            onClick={() => c.set((v: boolean) => !v)}
            className={clsx(
              "badge cursor-pointer border transition-colors",
              c.on ? "border-accent bg-accent text-accent-fg" : "border-border text-fg-subtle hover:border-accent-2 hover:text-fg"
            )}
            aria-pressed={c.on}
          >
            {c.label}
          </button>
        ))}
        <span className="ml-auto text-2xs tabular text-fg-subtle">
          {t("promoters.count", { count: total ?? rows.length })}
        </span>
      </div>

      {selected.size > 0 && (
        <div className="flex flex-wrap items-center gap-3 rounded-lg border border-border bg-surface px-4 py-2 text-sm">
          <span className="text-fg-subtle">{t("contacts.selected", { count: selected.size })}</span>
          <button
            className="btn-outline py-1.5"
            onClick={() => promoteSelected.mutate()}
            disabled={promoteSelected.isPending}
            title={t("scouting.promote_selected_hint")}
          >
            <UserPlus size={13} />
            {t("scouting.promote_selected", { count: selected.size })}
          </button>
          <AddToCampaign resolve={selectedContactIds} onDone={() => (setSelected(new Set()), refresh())} />
          <button className="btn-ghost ml-auto py-1.5 text-xs" onClick={() => setSelected(new Set())}>
            {t("common.cancel")}
          </button>
        </div>
      )}

      {rows.length === 0 ? (
        <div className="card">
          <EmptyState
            icon={Users}
            title={isLoading ? t("common.loading") : t("promoters.empty")}
            hint={isLoading ? undefined : t("promoters.empty_hint")}
          />
        </div>
      ) : (
        <div className="card max-h-[calc(100vh-340px)] overflow-auto">
          <table className="tbl">
            <thead>
              <tr>
                <th />
                <th className="text-right">{t("fit.col")}</th>
                <th>{t("promoters.col_name")}</th>
                <th className="text-right">{t("promoters.col_events")}</th>
                <th>{t("promoters.col_venues")}</th>
                <th>{t("promoters.col_last")}</th>
                <th>{t("venues.contact")}</th>
                <th>{t("fit.col_timing")}</th>
                <th />
              </tr>
            </thead>
            <tbody>
              {rows.map((p) => (
                <tr key={p.id} className="cursor-pointer hover:bg-muted" onClick={() => setOpenId(p.id)}>
                  <td onClick={(e) => e.stopPropagation()}>
                    <input
                      type="checkbox"
                      className="accent-accent"
                      checked={selected.has(p.id)}
                      onChange={() => toggle(p.id)}
                    />
                  </td>
                  <td className="text-right">
                    <FitScore score={p.fit_score} detail={p.fit_detail} />
                  </td>
                  <td className="max-w-[260px]">
                    <div className="truncate font-semibold">{p.nom}</div>
                    <div className="truncate text-2xs text-fg-subtle">
                      {[p.ville, p.pays].filter(Boolean).join(", ")}
                    </div>
                    <OrgTypeBadge type={p.org_type} note={p.org_type_note} source={p.org_type_source} />
                    <FitLine detail={p.fit_detail} />
                  </td>
                  <td
                    className="text-right tabular"
                    title={p.nb_events_12m > 0 ? t("promoters.events_hint", { total: p.nb_events_total }) : t("promoters.events_legacy_hint")}
                  >
                    {/* No 12-month detail: the old total, marked as such. */}
                    {p.nb_events_12m > 0 ? p.nb_events_12m : <span className="text-fg-faint">~{p.nb_events_total}</span>}
                  </td>
                  <td className="max-w-[220px] truncate text-xs text-fg-subtle" title={p.venues.join(", ")}>
                    {p.venues.join(", ")}
                    {p.nb_venues > p.venues.length && ` +${p.nb_venues - p.venues.length}`}
                  </td>
                  <td className="whitespace-nowrap text-xs text-fg-subtle">{formatDate(p.last_event)}</td>
                  <td>
                    <div className="flex items-center gap-2 text-fg-subtle">
                      {p.best_email ? (
                        <span className="inline-flex items-center gap-1 text-ok" title={p.best_email}>
                          <Mail size={13} />
                          {p.nb_emails > 1 ? p.nb_emails : ""}
                        </span>
                      ) : (
                        <Mail size={13} className="text-fg-faint" />
                      )}
                      <Globe size={13} className={p.site_web ? "text-fg-subtle" : "text-fg-faint"} />
                      {!p.enriched && <span className="badge bg-muted text-fg-faint">{t("venues.not_enriched")}</span>}
                    </div>
                    <IgHandle handle={p.instagram} />
                  </td>
                  <td>
                    <TimingBadge timing={p.timing} />
                  </td>
                  <td className="text-right" onClick={(e) => e.stopPropagation()}>
                    {p.crm_contact_id ? (
                      <span className="badge bg-ok/15 text-ok">{t("radar.in_pipeline")}</span>
                    ) : (
                      (p.best_email || p.instagram) && (
                        <button
                          className="btn-outline py-1.5"
                          onClick={() => promote.mutate(p.id)}
                          disabled={promote.isPending}
                        >
                          <UserPlus size={13} />
                          {t("radar.promote")}
                        </button>
                      )
                    )}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
          {hasNextPage && (
            <div className="p-3 text-center">
              <button className="btn-ghost text-xs" onClick={() => fetchNextPage()} disabled={isFetchingNextPage}>
                {t("promoters.more")}
              </button>
            </div>
          )}
        </div>
      )}

      {openId != null && <PromoterFicheModal id={openId} onClose={() => setOpenId(null)} />}
    </div>
  );
}

function PromoterFicheModal({ id, onClose }: { id: number; onClose: () => void }) {
  const { t } = useTranslation();
  const qc = useQueryClient();
  const { data: f } = useQuery({ queryKey: ["vi_promoter", id], queryFn: () => viPromoterDetail(id) });
  const setType = useMutation({
    mutationFn: (ty: OrgType | null) => viSetPromoterType(id, ty),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ["vi_promoter", id] });
      qc.invalidateQueries({ queryKey: ["vi_promoters"] });
    },
  });
  return (
    <Modal open onClose={onClose} title={f?.row.nom ?? "…"} width="max-w-2xl">
      {!f ? (
        <div className="p-8 text-center text-fg-subtle">
          <RefreshCw size={18} className="mx-auto animate-spin" />
        </div>
      ) : (
        <div className="space-y-4 text-sm">
          <div className="flex flex-wrap items-center gap-2">
            <select
              className="input h-8 w-auto py-0 text-xs"
              value={f.row.org_type ?? "collectif"}
              onChange={(e) => setType.mutate(e.target.value === "" ? null : (e.target.value as OrgType))}
              aria-label={t("promoters.type_label")}
            >
              {ORG_TYPES.map((ty) => (
                <option key={ty} value={ty}>
                  {t(typeKey("one", ty))}
                </option>
              ))}
              {f.row.org_type_source === "manual" && <option value="">{t("promoters.type_reset")}</option>}
            </select>
            <span className="text-2xs text-fg-subtle">
              {f.row.org_type_source && t(`promoters.type_by_${f.row.org_type_source}`)}
              {f.row.org_type_note && ` : ${f.row.org_type_note}`}
            </span>
          </div>
          {f.row.instagram_rejected && !f.instagram && (
            <div className="text-2xs text-fg-faint">{t("promoters.ig_rejected", { handle: f.row.instagram_rejected })}</div>
          )}
          <div className="flex flex-wrap items-center gap-2">
            <FitScore score={f.row.fit_score} detail={f.row.fit_detail} />
            <FitLine detail={f.row.fit_detail} />
            <TimingBadge timing={f.row.timing} />
          </div>
          <div className="text-2xs text-fg-subtle">
            {t("fit.timing_lead", { weeks: f.row.timing.lead_weeks })} (
            {t(f.lead_source === "observed" ? "fit.lead_observed" : "fit.lead_default")})
          </div>
          <div className="flex flex-wrap gap-2">
            {f.row.ra_url && (
              <button className="btn-outline py-1.5 text-xs" onClick={() => openUrl(f.row.ra_url!)}>
                <ExternalLink size={13} /> RA
              </button>
            )}
            {f.row.site_web && (
              <button className="btn-outline py-1.5 text-xs" onClick={() => openUrl(f.row.site_web!)}>
                <Globe size={13} /> {t("promoters.site")}
              </button>
            )}
            {f.instagram && (
              <button
                className="btn-outline py-1.5 text-xs"
                onClick={() => openUrl(`https://www.instagram.com/${f.instagram}/`)}
              >
                @{f.instagram}
              </button>
            )}
          </div>
          <DmPanel target={{ promoterId: f.row.id }} handle={f.instagram} />
          <section>
            <div className="eyebrow mb-1 text-fg">{t("promoters.contacts", { count: f.contacts.length })}</div>
            {f.contacts.length === 0 ? (
              <div className="text-xs text-fg-faint">{t("promoters.no_contact")}</div>
            ) : (
              <ul className="space-y-1 text-xs">
                {f.contacts.map((c) => (
                  <li key={c.id} className="flex items-center gap-2">
                    <Mail size={12} className="text-ok" />
                    <span className="font-medium">{c.valeur}</span>
                    <span className="text-fg-faint">
                      {t(`venues.role_${c.role_devine || "autre"}`, c.role_devine || "")} ·{" "}
                      {c.source_method === "indie_bible" ? "Indie Bible" : c.source_method}
                    </span>
                  </li>
                ))}
              </ul>
            )}
          </section>
          <section>
            <div className="eyebrow mb-1 text-fg">{t("promoters.col_venues")}</div>
            <ul className="space-y-0.5 text-xs">
              {f.venues.map((v) => (
                <li key={v.venue_id}>
                  <span className="font-medium">{v.nom}</span>
                  <span className="text-fg-subtle">
                    {" "}
                    {v.ville} · {t("promoters.nights", { count: v.nb_events })}
                    {v.capacite_est ? ` · ${t("residency.cap", { n: v.capacite_est })}` : ""}
                  </span>
                </li>
              ))}
            </ul>
          </section>
          <section>
            <div className="eyebrow mb-1 text-fg">{t("promoters.recent_events")}</div>
            {f.events.length === 0 ? (
              <div className="text-xs text-fg-faint">{t("promoters.no_events")}</div>
            ) : (
              <ul className="space-y-1 text-xs">
                {f.events.map((e, i) => (
                  <li key={i}>
                    <span className="tabular text-fg-subtle">{formatDate(e.date_event)}</span>{" "}
                    <span className="font-medium">{e.titre ?? "·"}</span>
                    <span className="text-fg-subtle"> @ {e.venue}</span>
                    {e.artists.length > 0 && <div className="text-2xs text-fg-faint">{e.artists.join(", ")}</div>}
                  </li>
                ))}
              </ul>
            )}
          </section>
        </div>
      )}
    </Modal>
  );
}
