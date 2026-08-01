import { useEffect, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { listen } from "@tauri-apps/api/event";
import { openUrl } from "@tauri-apps/plugin-opener";
import {
  Radar as RadarIcon,
  Sparkles,
  Play,
  RefreshCw,
  Mail,
  Globe,
  ExternalLink,
  UserPlus,
  Check,
  Music2,
} from "lucide-react";
import clsx from "clsx";
import {
  radarEnrich,
  radarHarvest,
  radarList,
  radarPromote,
  radarStats,
} from "../lib/api";
import type { ViRunProgress } from "../lib/types";
import { PageHeader, EmptyState } from "../components/Layout";
import { useToast } from "../components/ui";

const GENRE_PRESETS = [
  "tech house",
  "house",
  "deep house",
  "melodic techno",
  "afro house",
  "minimal",
  "electronica",
  "disco",
];
const SOURCES = ["spotify", "deezer"] as const;

export function Radar() {
  const { t } = useTranslation();
  const toast = useToast();
  const qc = useQueryClient();

  const [genres, setGenres] = useState<string[]>(["tech house", "house"]);
  const [customGenre, setCustomGenre] = useState("");
  const [sources, setSources] = useState<string[]>(["spotify", "deezer"]);
  const [running, setRunning] = useState(false);
  const [progress, setProgress] = useState<ViRunProgress | null>(null);

  // Filters
  const [statut, setStatut] = useState("qualifie");
  const [source, setSource] = useState("all");
  const [genreF, setGenreF] = useState("all");
  const [search, setSearch] = useState("");
  const [hasEmail, setHasEmail] = useState(false);
  const [hideEditorial, setHideEditorial] = useState(true);

  const { data: stats } = useQuery({
    queryKey: ["radar_stats"],
    queryFn: radarStats,
    refetchInterval: running ? 4000 : false,
  });
  const { data: rows = [] } = useQuery({
    queryKey: ["radar_list", statut, source, genreF, search, hasEmail, hideEditorial],
    queryFn: () =>
      radarList({
        statut,
        source,
        genre: genreF,
        search,
        has_email: hasEmail || undefined,
        hide_editorial: hideEditorial || undefined,
        limit: 500,
      }),
    refetchInterval: running ? 4000 : false,
  });

  useEffect(() => {
    const un = listen<ViRunProgress>("vi:run-progress", (e) => {
      setProgress(e.payload);
      const done = e.payload.statut === "termine" || e.payload.statut === "arrete";
      if (done) {
        setRunning(false);
        qc.invalidateQueries({ queryKey: ["radar_list"] });
        qc.invalidateQueries({ queryKey: ["radar_stats"] });
      }
    });
    return () => {
      un.then((f) => f());
    };
  }, [qc]);

  const toggleGenre = (g: string) =>
    setGenres((s) => (s.includes(g) ? s.filter((x) => x !== g) : [...s, g]));
  const toggleSource = (s: string) =>
    setSources((v) => (v.includes(s) ? v.filter((x) => x !== s) : [...v, s]));

  const harvest = async () => {
    const gs = [...genres];
    if (customGenre.trim()) gs.push(customGenre.trim());
    if (!gs.length || !sources.length) return toast(t("radar.pick_hint"), "error");
    setRunning(true);
    try {
      await radarHarvest(gs, sources);
      qc.invalidateQueries({ queryKey: ["vi_runs"] });
      toast(t("radar.harvest_started"), "ok");
    } catch (e: any) {
      setRunning(false);
      toast(e?.toString?.() ?? "error", "error");
    }
  };
  const enrich = async () => {
    setRunning(true);
    try {
      await radarEnrich();
      toast(t("radar.enrich_started"), "ok");
    } catch (e: any) {
      setRunning(false);
      const msg = e?.toString?.() ?? "error";
      toast(msg.includes("enrichir") ? t("radar.enrich_none") : msg, "error");
    }
  };

  const promote = useMutation({
    mutationFn: (id: number) => radarPromote(id),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ["radar_list"] });
      qc.invalidateQueries({ queryKey: ["radar_stats"] });
      toast(t("radar.promoted"), "ok");
    },
    onError: (e: any) => toast(e?.toString?.() ?? "error", "error"),
  });

  const chips = stats
    ? [
        { k: "total", label: t("radar.s_total"), v: stats.total, tone: "text-fg" },
        { k: "qualifie", label: t("radar.s_qualified"), v: stats.qualifie, tone: "text-accent" },
        {
          k: "email",
          label: t("radar.s_email"),
          v: stats.with_email,
          tone: "text-emerald-500",
          on: hasEmail,
          onClick: () => setHasEmail((x) => !x),
        },
        { k: "pipe", label: t("radar.s_pipeline"), v: stats.in_pipeline },
        { k: "edito", label: t("radar.s_editorial"), v: stats.editorial },
      ]
    : [];

  return (
    <div>
      <PageHeader kicker="RADAR" title={t("radar.title")} subtitle={t("radar.subtitle")} />
      <div className="space-y-5 px-8 py-6">
        {/* Launcher */}
        <div className="card p-5">
          <div className="kicker mb-3">{t("radar.new_scan")}</div>
          <div className="mb-3 flex flex-wrap gap-1.5">
            {GENRE_PRESETS.map((g) => (
              <button
                key={g}
                onClick={() => toggleGenre(g)}
                className={clsx(
                  "badge cursor-pointer border-2",
                  genres.includes(g) ? "border-accent bg-accent text-accent-fg" : "border-border text-fg-subtle hover:border-accent-2"
                )}
              >
                {g}
              </button>
            ))}
            <input
              className="input h-6 w-40 py-0 text-xs"
              placeholder={t("radar.custom_genre")}
              value={customGenre}
              onChange={(e) => setCustomGenre(e.target.value)}
            />
          </div>
          <div className="flex flex-wrap items-center gap-2">
            <div className="segmented">
              {SOURCES.map((s) => (
                <button key={s} data-active={sources.includes(s)} className="segmented-item" onClick={() => toggleSource(s)}>
                  {s}
                </button>
              ))}
            </div>
            <button className="btn-primary" onClick={harvest} disabled={running}>
              <Play size={15} />
              {t("radar.scan")}
            </button>
            <button className="btn-outline" onClick={enrich} disabled={running} title={t("radar.enrich_hint")}>
              <Sparkles size={15} className={running ? "animate-pulse" : ""} />
              {t("radar.enrich")}
            </button>
          </div>
          {running && progress && (
            <div className="mt-3 flex items-center gap-2 text-2xs text-fg-subtle">
              <RefreshCw size={12} className="animate-spin text-accent" />
              <span className="tabular">
                {progress.tasks_done}/{progress.tasks_total} {t("venues.tasks")}
              </span>
              {progress.message && <span className="truncate font-mono">· {progress.message}</span>}
            </div>
          )}
          <p className="mt-3 text-2xs leading-relaxed text-fg-subtle">{t("radar.disclaimer")}</p>
        </div>

        {/* Stats bento */}
        {chips.length > 0 && (
          <div className="grid grid-cols-2 gap-2 sm:grid-cols-5">
            {chips.map((c) => (
              <button
                key={c.k}
                onClick={c.onClick}
                disabled={!c.onClick}
                className={clsx(
                  "border-2 px-3 py-2 text-left transition-colors",
                  c.on ? "border-accent bg-accent/5" : "border-border",
                  c.onClick ? "cursor-pointer hover:border-accent-2" : "cursor-default"
                )}
              >
                <div className={clsx("text-xl font-black tabular", c.tone ?? "text-fg")}>{c.v}</div>
                <div className="text-2xs uppercase tracking-wide text-fg-faint">{c.label}</div>
              </button>
            ))}
          </div>
        )}

        {/* Filters */}
        <div className="flex flex-wrap items-center gap-2">
          <input
            className="input max-w-xs flex-1"
            placeholder={t("radar.search")}
            value={search}
            onChange={(e) => setSearch(e.target.value)}
          />
          <select className="input w-auto" value={statut} onChange={(e) => setStatut(e.target.value)}>
            <option value="all">{t("common.all")}</option>
            <option value="qualifie">{t("venues.st_qualifie")}</option>
            <option value="candidat">{t("venues.st_candidat")}</option>
            <option value="valide">{t("venues.st_valide")}</option>
          </select>
          <select className="input w-auto" value={source} onChange={(e) => setSource(e.target.value)}>
            <option value="all">{t("radar.all_sources")}</option>
            {SOURCES.map((s) => (
              <option key={s} value={s}>
                {s}
              </option>
            ))}
          </select>
          <select className="input w-auto" value={genreF} onChange={(e) => setGenreF(e.target.value)}>
            <option value="all">{t("radar.all_genres")}</option>
            {(stats?.genres ?? []).map(([g, n]) => (
              <option key={g} value={g}>
                {g} ({n})
              </option>
            ))}
          </select>
          <button
            className={clsx("badge cursor-pointer border-2", hasEmail ? "border-accent bg-accent text-accent-fg" : "border-border text-fg-subtle")}
            onClick={() => setHasEmail((x) => !x)}
          >
            <Mail size={11} /> {t("radar.s_email")}
          </button>
          <button
            className={clsx("badge cursor-pointer border-2", hideEditorial ? "border-accent bg-accent text-accent-fg" : "border-border text-fg-subtle")}
            onClick={() => setHideEditorial((x) => !x)}
          >
            {t("radar.hide_editorial")}
          </button>
        </div>

        {/* List */}
        {rows.length === 0 ? (
          <div className="card">
            <EmptyState icon={RadarIcon} title={t("radar.empty")} hint={t("radar.empty_hint")} />
          </div>
        ) : (
          <div className="card max-h-[calc(100vh-460px)] overflow-auto">
            <table className="tbl">
              <thead>
                <tr>
                  <th className="w-14 text-right">{t("venues.score")}</th>
                  <th>{t("radar.curator")}</th>
                  <th className="text-right">{t("radar.followers")}</th>
                  <th>{t("venues.contact")}</th>
                  <th>{t("radar.genre")}</th>
                  <th></th>
                </tr>
              </thead>
              <tbody>
                {rows.map((c) => (
                  <tr key={c.id}>
                    <td className="text-right">
                      <span className="text-lg font-black tabular text-accent">{c.score}</span>
                    </td>
                    <td>
                      <div className="flex items-center gap-2 font-semibold">
                        <Music2 size={12} className="text-accent-2" />
                        {c.nom}
                        {c.editorial && (
                          <span className="badge bg-muted text-fg-faint">{t("radar.editorial")}</span>
                        )}
                        {c.in_pipeline && (
                          <span className="badge bg-emerald-500/15 text-emerald-500">
                            <Check size={11} /> {t("radar.in_pipeline")}
                          </span>
                        )}
                      </div>
                      {c.owner_name && <div className="text-2xs text-fg-subtle">{c.owner_name} · {c.source}</div>}
                    </td>
                    <td className="text-right font-bold tabular">{c.followers?.toLocaleString() ?? "·"}</td>
                    <td>
                      <div className="flex items-center gap-2 text-fg-subtle">
                        {c.best_email ? (
                          <span className="inline-flex items-center gap-1 text-emerald-500" title={c.best_email}>
                            <Mail size={13} />
                            {c.nb_emails > 1 ? c.nb_emails : ""}
                          </span>
                        ) : (
                          <Mail size={13} className="text-fg-faint" />
                        )}
                        <Globe size={13} className={c.site_web ? "text-fg-subtle" : "text-fg-faint"} />
                      </div>
                    </td>
                    <td className="text-2xs text-fg-subtle">{c.genre}</td>
                    <td className="text-right">
                      <div className="flex justify-end gap-1">
                        {c.url && (
                          <button
                            className="btn-ghost px-2 py-1.5"
                            aria-label={t("radar.open")}
                            title={t("radar.open")}
                            onClick={() => openUrl(c.url!).catch(() => {})}
                          >
                            <ExternalLink size={14} />
                          </button>
                        )}
                        {!c.in_pipeline && c.best_email && (
                          <button
                            className="btn-outline py-1.5"
                            onClick={() => promote.mutate(c.id)}
                            disabled={promote.isPending}
                          >
                            <UserPlus size={13} />
                            {t("radar.promote")}
                          </button>
                        )}
                      </div>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}
        <div className="text-2xs text-fg-subtle">{t("radar.count", { count: rows.length })}</div>
      </div>
    </div>
  );
}
