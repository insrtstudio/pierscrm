import { useEffect, useMemo, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { listen } from "@tauri-apps/api/event";
import { openUrl } from "@tauri-apps/plugin-opener";
import {
  Play,
  RefreshCw,
  ExternalLink,
  Plus,
  Trash2,
  Radio,
  MapPin,
  Check,
  Sparkles,
  Mail,
  Phone,
  Globe,
  X,
  Copy,
  CalendarDays,
  Users,
} from "lucide-react";
import clsx from "clsx";
import {
  viListAreas,
  viListReferenceArtists,
  viListRuns,
  viListVenues,
  viResolveAllAreas,
  viResolveArea,
  viResumeRun,
  viSaveArea,
  viSaveReferenceArtist,
  viDeleteReferenceArtist,
  viStartHarvest,
  viStartEnrich,
  viVenueDetail,
  listArtists,
} from "../lib/api";
import type {
  ViArea,
  ViReferenceArtist,
  ViRun,
  ViRunProgress,
  ViVenueFiche,
} from "../lib/types";
import { PageHeader, EmptyState } from "../components/Layout";
import { Field, useToast, useConfirm } from "../components/ui";

type Tab = "venues" | "runs" | "areas" | "artists";

const VENUE_STATUS_STYLE: Record<string, string> = {
  candidat: "bg-muted text-fg-subtle",
  qualifie: "bg-accent text-accent-fg",
  valide: "bg-emerald-500/15 text-emerald-500",
  rejete: "bg-muted text-fg-faint",
  archive: "bg-muted text-fg-faint",
};

export function Venues() {
  const { t } = useTranslation();
  const [tab, setTab] = useState<Tab>("venues");
  const tabs: Tab[] = ["venues", "runs", "areas", "artists"];
  return (
    <div>
      <PageHeader kicker="INTELLIGENCE" title={t("venues.title")} subtitle={t("venues.subtitle")} />
      <div className="px-8 py-6">
        <div className="segmented mb-6">
          {tabs.map((tb) => (
            <button key={tb} data-active={tab === tb} className="segmented-item" onClick={() => setTab(tb)}>
              {t(`venues.tab_${tb}`)}
            </button>
          ))}
        </div>
        {tab === "venues" && <VenuesTab />}
        {tab === "runs" && <RunsTab />}
        {tab === "areas" && <AreasTab />}
        {tab === "artists" && <ArtistsTab />}
      </div>
    </div>
  );
}

// ---------------- Venues ----------------

function VenuesTab() {
  const { t } = useTranslation();
  const toast = useToast();
  const qc = useQueryClient();
  const [statut, setStatut] = useState("qualifie");
  const [search, setSearch] = useState("");
  const [enriching, setEnriching] = useState(false);
  const [openId, setOpenId] = useState<number | null>(null);
  const [targetArtist, setTargetArtist] = useState<number | "">("");
  const { data: venues = [] } = useQuery({
    queryKey: ["vi_venues", statut, search],
    queryFn: () => viListVenues({ statut, search, limit: 400 }),
    // While an enrichment run is filling in contacts, keep the list fresh.
    refetchInterval: enriching ? 4000 : false,
  });
  const { data: artists = [] } = useQuery({ queryKey: ["artists"], queryFn: listArtists });

  // City targeting: float venues in the chosen artist's audience cities to the top.
  const cityNorm = (s: string) =>
    s.toLowerCase().normalize("NFD").replace(/[̀-ͯ]/g, "").trim();
  const targetCities = useMemo(() => {
    const a = artists.find((x) => x.id === targetArtist);
    const raw = a?.audience_cities;
    if (!raw) return null;
    const list = raw.split(",").map(cityNorm).filter(Boolean);
    return list.length ? list : null;
  }, [artists, targetArtist]);
  const isTarget = (ville?: string | null) =>
    !!targetCities &&
    !!ville &&
    targetCities.some((c) => cityNorm(ville).includes(c) || c.includes(cityNorm(ville)));
  const displayVenues = useMemo(() => {
    if (!targetCities) return venues;
    return [...venues].sort((a, b) => (isTarget(b.ville) ? 1 : 0) - (isTarget(a.ville) ? 1 : 0));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [venues, targetCities]);

  // Stop the auto-refresh once every enrich run has finished.
  useEffect(() => {
    if (!enriching) return;
    const un = listen<ViRunProgress>("vi:run-progress", (e) => {
      if (e.payload.statut === "termine") {
        setEnriching(false);
        qc.invalidateQueries({ queryKey: ["vi_venues"] });
      }
    });
    return () => {
      un.then((f) => f());
    };
  }, [enriching, qc]);

  const enrich = async () => {
    setEnriching(true);
    try {
      await viStartEnrich();
      toast(t("venues.enrich_started"), "ok");
    } catch (e: any) {
      setEnriching(false);
      const msg = e?.toString?.() ?? "error";
      toast(msg.includes("enrichir") ? t("venues.enrich_none") : msg, "error");
    }
  };

  return (
    <div className="space-y-4">
      <div className="flex flex-wrap items-center gap-2">
        <input
          className="input max-w-xs flex-1"
          placeholder={t("venues.search")}
          value={search}
          onChange={(e) => setSearch(e.target.value)}
        />
        <select className="input w-auto" value={statut} onChange={(e) => setStatut(e.target.value)}>
          <option value="all">{t("common.all")}</option>
          {["qualifie", "valide", "candidat", "rejete"].map((s) => (
            <option key={s} value={s}>
              {t(`venues.st_${s}`)}
            </option>
          ))}
        </select>
        <select
          className="input w-auto"
          value={targetArtist}
          onChange={(e) => setTargetArtist(e.target.value ? Number(e.target.value) : "")}
          title={t("venues.target_hint")}
        >
          <option value="">{t("venues.target_none")}</option>
          {artists.map((a) => (
            <option key={a.id} value={a.id}>
              {t("venues.target_by", { name: a.name })}
            </option>
          ))}
        </select>
        <button className="btn-primary ml-auto" onClick={enrich} disabled={enriching} title={t("venues.enrich_hint")}>
          <Sparkles size={15} className={enriching ? "animate-pulse" : ""} />
          {t("venues.enrich")}
        </button>
      </div>
      <p className="text-2xs text-fg-subtle">{t("venues.enrich_hint")}</p>

      {venues.length === 0 ? (
        <div className="card">
          <EmptyState icon={Radio} title={t("venues.no_venues")} />
        </div>
      ) : (
        <div className="card max-h-[calc(100vh-360px)] overflow-auto">
          <table className="tbl">
            <thead>
              <tr>
                <th className="w-16 text-right">{t("venues.score")}</th>
                <th>{t("venues.name")}</th>
                <th>{t("venues.city")}</th>
                <th>{t("venues.contact")}</th>
                <th className="text-right">{t("venues.evidence")}</th>
                <th>{t("venues.status")}</th>
                <th></th>
              </tr>
            </thead>
            <tbody>
              {displayVenues.map((v) => (
                <tr
                  key={v.id}
                  className="cursor-pointer hover:bg-muted"
                  onClick={() => setOpenId(v.id)}
                >
                  <td className="text-right">
                    <span className="text-lg font-black tabular text-accent">{v.score_qualif}</span>
                  </td>
                  <td className="font-semibold">
                    {v.nom}
                    {isTarget(v.ville) && (
                      <span className="badge ml-2 bg-accent text-accent-fg">{t("venues.target_match")}</span>
                    )}
                  </td>
                  <td className="text-fg-subtle">
                    {[v.ville, v.pays].filter(Boolean).join(", ")}
                  </td>
                  <td>
                    <div className="flex items-center gap-2 text-fg-subtle">
                      {v.best_email ? (
                        <span className="inline-flex items-center gap-1 text-emerald-500" title={v.best_email}>
                          <Mail size={13} />
                          {v.nb_emails > 1 ? v.nb_emails : ""}
                        </span>
                      ) : (
                        <Mail size={13} className="text-fg-faint" />
                      )}
                      <Phone size={13} className={v.telephone ? "text-fg-subtle" : "text-fg-faint"} />
                      <Globe size={13} className={v.site_web ? "text-fg-subtle" : "text-fg-faint"} />
                      {!v.enriched && (
                        <span className="badge bg-muted text-fg-faint">{t("venues.not_enriched")}</span>
                      )}
                    </div>
                  </td>
                  <td className="text-right font-bold tabular">{v.nb_evidence}</td>
                  <td>
                    <span className={clsx("badge", VENUE_STATUS_STYLE[v.statut])}>
                      {t(`venues.st_${v.statut}`, v.statut)}
                    </span>
                  </td>
                  <td className="text-right">
                    {v.ra_url && (
                      <button
                        className="btn-ghost px-2 py-1.5"
                        title={t("venues.open_ra")}
                        onClick={(e) => {
                          e.stopPropagation();
                          openUrl(v.ra_url!).catch(() => {});
                        }}
                      >
                        <ExternalLink size={15} />
                      </button>
                    )}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
      <div className="text-2xs text-fg-subtle">{t("venues.count_venues", { count: venues.length })}</div>

      {openId != null && <VenueFicheModal id={openId} onClose={() => setOpenId(null)} />}
    </div>
  );
}

// ---------------- Venue fiche modal ----------------

const ROLE_STYLE: Record<string, string> = {
  booking: "bg-accent text-accent-fg",
  management: "bg-indigo-500/15 text-indigo-400",
  presse: "bg-amber-500/15 text-amber-500",
  general: "bg-muted text-fg-subtle",
  reservation: "bg-muted text-fg-faint",
  autre: "bg-muted text-fg-faint",
};

function VenueFicheModal({ id, onClose }: { id: number; onClose: () => void }) {
  const { t } = useTranslation();
  const toast = useToast();
  const { data: fiche } = useQuery<ViVenueFiche>({
    queryKey: ["vi_venue_detail", id],
    queryFn: () => viVenueDetail(id),
  });

  const copy = (val: string) => {
    navigator.clipboard?.writeText(val).then(
      () => toast(t("venues.copied"), "ok"),
      () => {}
    );
  };

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/60 p-6" onClick={onClose}>
      <div
        className="card max-h-[85vh] w-full max-w-2xl overflow-auto p-0"
        onClick={(e) => e.stopPropagation()}
      >
        {!fiche ? (
          <div className="p-8 text-center text-fg-subtle">
            <RefreshCw size={18} className="mx-auto animate-spin" />
          </div>
        ) : (
          <>
            <div className="sticky top-0 z-10 flex items-start justify-between border-b-2 border-border bg-bg px-6 py-4">
              <div>
                <div className="kicker">{t("venues.fiche")}</div>
                <h2 className="text-xl font-black">{fiche.nom}</h2>
                <div className="mt-0.5 text-xs text-fg-subtle">
                  {[fiche.ville, fiche.pays].filter(Boolean).join(", ")}
                  {fiche.capacite_est ? ` · ${t("venues.fiche_capacity")} ${fiche.capacite_est}` : ""}
                </div>
              </div>
              <div className="flex items-center gap-2">
                <span className={clsx("badge", VENUE_STATUS_STYLE[fiche.statut])}>
                  {t(`venues.st_${fiche.statut}`, fiche.statut)}
                </span>
                <span className="text-lg font-black tabular text-accent">{fiche.score_qualif}</span>
                <button className="btn-ghost px-2 py-1.5" onClick={onClose}>
                  <X size={16} />
                </button>
              </div>
            </div>

            <div className="space-y-6 p-6">
              {/* Coordinates */}
              <div className="grid grid-cols-1 gap-2 sm:grid-cols-2">
                <FicheLine icon={Globe} label={t("venues.fiche_website")} value={fiche.site_web} onOpen={fiche.site_web ? () => openUrl(fiche.site_web!).catch(() => {}) : undefined} />
                <FicheLine icon={Phone} label={t("venues.fiche_phone")} value={fiche.telephone} onCopy={fiche.telephone ? () => copy(fiche.telephone!) : undefined} />
                <FicheLine icon={MapPin} label={t("venues.fiche_address")} value={fiche.adresse} />
                <FicheLine icon={ExternalLink} label="RA" value={fiche.ra_url} onOpen={fiche.ra_url ? () => openUrl(fiche.ra_url!).catch(() => {}) : undefined} />
              </div>

              {/* Emails found */}
              <div>
                <div className="kicker mb-2 flex items-center gap-2">
                  <Mail size={13} /> {t("venues.fiche_contacts")}
                </div>
                {fiche.contacts.filter((c) => c.type_ === "email").length === 0 ? (
                  <p className="text-xs text-fg-subtle">{t("venues.fiche_no_contacts")}</p>
                ) : (
                  <div className="space-y-1.5">
                    {fiche.contacts
                      .filter((c) => c.type_ === "email")
                      .map((c) => (
                        <div key={c.id} className="flex items-center gap-2 border-2 border-border px-3 py-2">
                          <span className={clsx("badge", ROLE_STYLE[c.role_devine || "autre"])}>
                            {t(`venues.role_${c.role_devine || "autre"}`, c.role_devine || "")}
                          </span>
                          <a href={`mailto:${c.valeur}`} className="font-mono text-xs text-fg hover:text-accent">
                            {c.valeur}
                          </a>
                          <span className="ml-auto text-2xs tabular text-fg-faint">
                            {c.source_method === "mailto" ? "mailto" : "html"} · {c.score}
                          </span>
                          <button className="btn-ghost px-1.5 py-1" title={t("venues.copy")} onClick={() => copy(c.valeur)}>
                            <Copy size={13} />
                          </button>
                        </div>
                      ))}
                  </div>
                )}
              </div>

              {/* Evidence */}
              {fiche.evidence.length > 0 && (
                <div>
                  <div className="kicker mb-2 flex items-center gap-2">
                    <CalendarDays size={13} /> {t("venues.fiche_evidence")}
                  </div>
                  <div className="flex flex-wrap gap-1.5">
                    {fiche.evidence.map((ev, i) => (
                      <span key={i} className="inline-flex items-center gap-1.5 border-2 border-border px-2 py-1 text-2xs">
                        <span className="font-semibold">{ev.artiste}</span>
                        <span className="text-fg-faint">{ev.date_event}</span>
                      </span>
                    ))}
                  </div>
                </div>
              )}

              {/* Promoters */}
              {fiche.promoters.length > 0 && (
                <div>
                  <div className="kicker mb-2 flex items-center gap-2">
                    <Users size={13} /> {t("venues.fiche_promoters")}
                  </div>
                  <div className="flex flex-wrap gap-1.5">
                    {fiche.promoters.map((p, i) => (
                      <span key={i} className="inline-flex items-center gap-1.5 border-2 border-border px-2 py-1 text-2xs">
                        <span className="font-semibold">{p.nom}</span>
                        <span className="text-fg-faint tabular">{p.nb_events}</span>
                      </span>
                    ))}
                  </div>
                </div>
              )}
            </div>
          </>
        )}
      </div>
    </div>
  );
}

function FicheLine({
  icon: Icon,
  label,
  value,
  onOpen,
  onCopy,
}: {
  icon: any;
  label: string;
  value?: string | null;
  onOpen?: () => void;
  onCopy?: () => void;
}) {
  return (
    <div className="flex items-start gap-2 border-2 border-border px-3 py-2">
      <Icon size={14} className="mt-0.5 shrink-0 text-fg-faint" />
      <div className="min-w-0 flex-1">
        <div className="text-2xs uppercase tracking-wide text-fg-faint">{label}</div>
        <div className="truncate text-xs text-fg">{value || "-"}</div>
      </div>
      {value && onOpen && (
        <button className="btn-ghost px-1.5 py-1" onClick={onOpen}>
          <ExternalLink size={13} />
        </button>
      )}
      {value && onCopy && (
        <button className="btn-ghost px-1.5 py-1" onClick={onCopy}>
          <Copy size={13} />
        </button>
      )}
    </div>
  );
}

// ---------------- Runs / harvest ----------------

function RunsTab() {
  const { t } = useTranslation();
  const toast = useToast();
  const qc = useQueryClient();
  const year = new Date().getFullYear();

  const { data: areas = [] } = useQuery({ queryKey: ["vi_areas"], queryFn: viListAreas });
  const { data: runs = [] } = useQuery({
    queryKey: ["vi_runs"],
    queryFn: viListRuns,
    refetchInterval: 4000,
  });

  const resolved = areas.filter((a) => a.ra_area_id != null);
  const [selected, setSelected] = useState<Set<number>>(new Set());
  useEffect(() => {
    setSelected(new Set(resolved.map((a) => a.id!)));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [resolved.length]);

  const [yearFrom, setYearFrom] = useState(year - 1);
  const [yearTo, setYearTo] = useState(year);
  const [launching, setLaunching] = useState(false);

  // Live progress per run.
  const [progress, setProgress] = useState<Record<number, ViRunProgress>>({});
  useEffect(() => {
    const un = listen<ViRunProgress>("vi:run-progress", (e) => {
      setProgress((p) => ({ ...p, [e.payload.run_id]: e.payload }));
      if (e.payload.statut === "termine") qc.invalidateQueries({ queryKey: ["vi_runs"] });
    });
    return () => {
      un.then((f) => f());
    };
  }, [qc]);

  const launch = async () => {
    const ids = [...selected];
    if (ids.length === 0) return toast(t("venues.to_resolve"), "error");
    setLaunching(true);
    try {
      await viStartHarvest({ area_ids: ids, year_from: yearFrom, year_to: yearTo });
      qc.invalidateQueries({ queryKey: ["vi_runs"] });
      toast(t("venues.launch"), "ok");
    } catch (e: any) {
      toast(e?.toString?.() ?? "error", "error");
    } finally {
      setLaunching(false);
    }
  };

  const resume = async (id: number) => {
    try {
      await viResumeRun(id);
      qc.invalidateQueries({ queryKey: ["vi_runs"] });
    } catch (e: any) {
      toast(e?.toString?.() ?? "error", "error");
    }
  };

  return (
    <div className="grid grid-cols-1 gap-5 lg:grid-cols-[340px_1fr]">
      {/* Launcher */}
      <div className="card h-fit p-5">
        <div className="kicker mb-3">{t("venues.new_run")}</div>
        {resolved.length === 0 ? (
          <p className="text-xs text-fg-subtle">{t("venues.area_help")}</p>
        ) : (
          <>
            <div className="mb-3 max-h-52 overflow-y-auto border-2 border-border">
              {resolved.map((a) => {
                const on = selected.has(a.id!);
                return (
                  <button
                    key={a.id}
                    onClick={() =>
                      setSelected((s) => {
                        const n = new Set(s);
                        n.has(a.id!) ? n.delete(a.id!) : n.add(a.id!);
                        return n;
                      })
                    }
                    className="flex w-full items-center gap-2 border-b border-border px-3 py-1.5 text-left text-xs hover:bg-muted"
                  >
                    <span
                      className={clsx(
                        "flex h-4 w-4 shrink-0 items-center justify-center border",
                        on ? "border-accent bg-accent text-accent-fg" : "border-border-strong"
                      )}
                    >
                      {on && <Check size={11} />}
                    </span>
                    <span className="font-semibold">{a.libelle || a.slug}</span>
                    <span className="ml-auto text-fg-faint">{a.pays}</span>
                  </button>
                );
              })}
            </div>
            <div className="mb-3 grid grid-cols-2 gap-3">
              <Field label={t("venues.year_from")}>
                <input
                  type="number"
                  className="input"
                  value={yearFrom}
                  onChange={(e) => setYearFrom(Number(e.target.value))}
                />
              </Field>
              <Field label={t("venues.year_to")}>
                <input
                  type="number"
                  className="input"
                  value={yearTo}
                  onChange={(e) => setYearTo(Number(e.target.value))}
                />
              </Field>
            </div>
            <button className="btn-primary w-full" onClick={launch} disabled={launching}>
              <Play size={15} />
              {t("venues.launch")}
            </button>
            <p className="mt-3 text-2xs leading-relaxed text-fg-subtle">{t("venues.running_note")}</p>
          </>
        )}
      </div>

      {/* Runs list */}
      <div className="space-y-3">
        {runs.length === 0 ? (
          <div className="card">
            <EmptyState icon={Radio} title={t("venues.no_runs")} />
          </div>
        ) : (
          runs.map((r) => <RunCard key={r.id} run={r} live={progress[r.id]} onResume={() => resume(r.id)} />)
        )}
      </div>
    </div>
  );
}

function RunCard({
  run,
  live,
  onResume,
}: {
  run: ViRun;
  live?: ViRunProgress;
  onResume: () => void;
}) {
  const { t } = useTranslation();
  const total = live?.tasks_total ?? run.tasks_total;
  const done = live?.tasks_done ?? run.tasks_done;
  const echec = live?.tasks_echec ?? run.tasks_echec;
  const statut = live?.statut ?? run.statut;
  const pct = total ? Math.round(((done + echec) / total) * 100) : 0;
  const active = statut === "en_cours";
  const canResume = statut !== "termine" && !active;

  return (
    <div className="card p-4">
      <div className="flex items-center justify-between">
        <div className="flex items-center gap-2.5">
          {active && <RefreshCw size={14} className="animate-spin text-accent" />}
          <span className="font-bold">
            {t("venues.run")} #{run.id}
          </span>
          <span
            className={clsx(
              "badge",
              statut === "termine"
                ? "bg-emerald-500/15 text-emerald-500"
                : active
                ? "bg-accent text-accent-fg"
                : "bg-muted text-fg-subtle"
            )}
          >
            {statut}
          </span>
        </div>
        <div className="flex items-center gap-3 text-2xs tabular text-fg-subtle">
          {live && (
            <>
              <span>
                {live.venues} {t("venues.venues_found")}
              </span>
              <span className="text-accent-2">
                {live.evidence} {t("venues.evidence_found")}
              </span>
            </>
          )}
          {canResume && (
            <button className="btn-outline py-1.5" onClick={onResume}>
              <Play size={13} />
              {t("venues.resume")}
            </button>
          )}
        </div>
      </div>
      <div className="mt-3 flex items-center gap-3">
        <div className="h-2 flex-1 overflow-hidden bg-muted">
          <div className="h-full bg-accent transition-all" style={{ width: `${pct}%` }} />
        </div>
        <span className="text-2xs tabular text-fg-subtle">
          {done}/{total} {t("venues.tasks")}
          {echec > 0 && ` · ${echec} ${t("venues.errors")}`}
        </span>
      </div>
    </div>
  );
}

// ---------------- Areas ----------------

function AreasTab() {
  const { t } = useTranslation();
  const qc = useQueryClient();
  const toast = useToast();
  const { data: areas = [] } = useQuery({ queryKey: ["vi_areas"], queryFn: viListAreas });
  const [resolvingAll, setResolvingAll] = useState(false);

  const resolvedCount = areas.filter((a) => a.ra_area_id != null).length;

  const resolveAll = async () => {
    setResolvingAll(true);
    try {
      const n = await viResolveAllAreas();
      qc.invalidateQueries({ queryKey: ["vi_areas"] });
      toast(`${n}`, "ok");
    } catch (e: any) {
      toast(e?.toString?.() ?? "error", "error");
    } finally {
      setResolvingAll(false);
    }
  };

  return (
    <div className="space-y-4">
      <div className="flex items-center justify-between">
        <div className="text-2xs uppercase tracking-wide text-fg-subtle">
          {t("venues.active_areas", { resolved: resolvedCount, total: areas.length })}
        </div>
        <button className="btn-primary" onClick={resolveAll} disabled={resolvingAll}>
          <RefreshCw size={15} className={resolvingAll ? "animate-spin" : ""} />
          {resolvingAll ? t("venues.resolving") : t("venues.resolve_all")}
        </button>
      </div>
      <p className="text-2xs text-fg-subtle">{t("venues.area_help")}</p>
      <div className="card overflow-hidden">
        <table className="tbl">
          <thead>
            <tr>
              <th>{t("venues.slug")}</th>
              <th>{t("venues.country")}</th>
              <th className="text-right">{t("venues.area_id")}</th>
              <th></th>
            </tr>
          </thead>
          <tbody>
            {areas.map((a) => (
              <AreaRow key={a.id} area={a} />
            ))}
          </tbody>
        </table>
      </div>
    </div>
  );
}

function AreaRow({ area }: { area: ViArea }) {
  const { t } = useTranslation();
  const qc = useQueryClient();
  const [id, setId] = useState(area.ra_area_id?.toString() ?? "");
  const [busy, setBusy] = useState(false);

  const resolve = async () => {
    setBusy(true);
    try {
      const r = await viResolveArea(area.id!);
      if (r != null) setId(String(r));
      qc.invalidateQueries({ queryKey: ["vi_areas"] });
    } finally {
      setBusy(false);
    }
  };
  const saveManual = async () => {
    await viSaveArea({ ...area, ra_area_id: id ? Number(id) : null });
    qc.invalidateQueries({ queryKey: ["vi_areas"] });
  };

  return (
    <tr>
      <td className="font-mono text-xs">{area.slug}</td>
      <td className="text-fg-subtle">{area.pays}</td>
      <td className="text-right">
        <input
          className="input w-24 py-1 text-right text-xs"
          value={id}
          onChange={(e) => setId(e.target.value)}
          onBlur={saveManual}
          placeholder="-"
        />
      </td>
      <td className="text-right">
        {area.ra_area_id != null ? (
          <span className="badge bg-emerald-500/15 text-emerald-500">
            <MapPin size={11} /> {t("venues.resolved")}
          </span>
        ) : (
          <button className="btn-outline py-1.5" onClick={resolve} disabled={busy}>
            <RefreshCw size={13} className={busy ? "animate-spin" : ""} />
            {t("venues.resolve")}
          </button>
        )}
      </td>
    </tr>
  );
}

// ---------------- Reference artists ----------------

function ArtistsTab() {
  const { t } = useTranslation();
  const qc = useQueryClient();
  const confirm = useConfirm();
  const { data: artists = [] } = useQuery({
    queryKey: ["vi_ref_artists"],
    queryFn: viListReferenceArtists,
  });
  const [nom, setNom] = useState("");
  const [tier, setTier] = useState(1);

  const saveMut = useMutation({
    mutationFn: (a: ViReferenceArtist) => viSaveReferenceArtist(a),
    onSuccess: () => qc.invalidateQueries({ queryKey: ["vi_ref_artists"] }),
  });
  const delMut = useMutation({
    mutationFn: (id: number) => viDeleteReferenceArtist(id),
    onSuccess: () => qc.invalidateQueries({ queryKey: ["vi_ref_artists"] }),
  });

  const active = artists.filter((a) => a.actif).length;
  const byTier = (tr: number) => artists.filter((a) => a.tier === tr);

  return (
    <div className="space-y-5">
      <div className="flex flex-wrap items-end gap-2">
        <Field label={t("venues.artist_name")} className="flex-1">
          <input className="input" value={nom} onChange={(e) => setNom(e.target.value)} />
        </Field>
        <Field label={t("venues.tier")}>
          <select className="input w-24" value={tier} onChange={(e) => setTier(Number(e.target.value))}>
            {[1, 2, 3].map((tr) => (
              <option key={tr} value={tr}>
                {tr}
              </option>
            ))}
          </select>
        </Field>
        <button
          className="btn-primary"
          disabled={!nom.trim()}
          onClick={() => {
            saveMut.mutate({ nom: nom.trim(), tier, actif: true });
            setNom("");
          }}
        >
          <Plus size={15} />
          {t("venues.add_artist")}
        </button>
        <div className="ml-auto text-2xs uppercase tracking-wide text-fg-subtle">
          {t("venues.ref_count", { count: artists.length, active })}
        </div>
      </div>

      {[1, 2, 3].map((tr) => (
        <div key={tr}>
          <div className="kicker mb-2">
            {t("venues.tier")} {tr}
          </div>
          <div className="flex flex-wrap gap-2">
            {byTier(tr).map((a) => (
              <span
                key={a.id}
                className={clsx(
                  "group inline-flex items-center gap-1.5 border-2 px-2.5 py-1 text-xs font-semibold transition-colors",
                  a.actif ? "border-accent/40 text-fg" : "border-border text-fg-faint line-through"
                )}
              >
                <button onClick={() => saveMut.mutate({ ...a, actif: !a.actif })}>{a.nom}</button>
                <button
                  className="text-fg-faint opacity-0 transition-opacity hover:text-accent group-hover:opacity-100"
                  onClick={() => confirm(t("common.confirm_delete")) && delMut.mutate(a.id!)}
                >
                  <Trash2 size={12} />
                </button>
              </span>
            ))}
          </div>
        </div>
      ))}
    </div>
  );
}
