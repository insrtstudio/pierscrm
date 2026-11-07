import { useEffect, useMemo, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { listen } from "@tauri-apps/api/event";
import { openUrl } from "@tauri-apps/plugin-opener";
import { useNavigate } from "react-router-dom";
import { useTranslation } from "react-i18next";
import clsx from "clsx";
import { CheckCircle2, ExternalLink, Globe, Mail, RefreshCw, X } from "lucide-react";
import { viListRuns, viRunReport } from "../lib/api";
import { translateCode } from "../lib/backendMessages";
import type { RunOutcome, ViRun, ViRunProgress, ViRunStats } from "../lib/types";
import { IgHandle } from "./InstagramDm";

// What a collecte did, in words: the per-type tiles of a finished run, the
// list of every collective or venue it looked at, and the dock that follows
// a run from any page and stays up once it is over.

/** Run types whose tasks record what they found (harvest::outcome_tally). */
const REPORTED = new Set(["instagram_promoters", "enrich_promoters", "enrich"]);

export function runKindLabel(t: (k: string) => string, type_?: string | null) {
  const k = type_ ?? "harvest";
  return t(`report.kind.${["instagram_promoters", "enrich_promoters", "enrich"].includes(k) ? k : "harvest"}`);
}

export function parseStats(s?: string | null): ViRunStats | null {
  if (!s) return null;
  try {
    return JSON.parse(s);
  } catch {
    return null;
  }
}

/** SQLite "YYYY-MM-DD HH:MM:SS" (UTC) to a Date. */
export function sqlDate(s?: string | null): Date | null {
  if (!s) return null;
  const d = new Date(s.replace(" ", "T") + "Z");
  return isNaN(d.getTime()) ? null : d;
}

export function fmtClock(d: Date | null) {
  return d ? d.toLocaleTimeString("fr-FR", { hour: "2-digit", minute: "2-digit" }) : "";
}

export function fmtDuration(secs?: number) {
  if (!secs || secs < 0) return "-";
  const m = Math.floor(secs / 60);
  const s = secs % 60;
  if (m === 0) return `${s}s`;
  if (m < 60) return `${m} min ${s}s`;
  return `${Math.floor(m / 60)} h ${m % 60} min`;
}

const n0 = (v?: number) => v ?? 0;
const fmtN = (v: number) => v.toLocaleString("fr-FR");

/** Collectives or venues that gave something. */
export function hitsOf(type_: string, o: RunOutcome) {
  if (type_ === "instagram_promoters") return n0(o.found);
  if (type_ === "enrich_promoters") return n0(o.emails) + n0(o.info);
  return n0(o.emails);
}

interface Tile {
  key: string;
  value: number;
  tone?: string;
  always?: boolean;
}

function tilesFor(type_: string, o: RunOutcome, errors: number): Tile[] {
  const base: Tile[] =
    type_ === "instagram_promoters"
      ? [
          { key: "ig_found", value: n0(o.found), tone: "text-ok", always: true },
          { key: "none", value: n0(o.none), always: true },
          { key: "had", value: n0(o.had) },
        ]
      : type_ === "enrich_promoters"
      ? [
          { key: "new_emails", value: n0(o.new_emails), tone: "text-ok", always: true },
          { key: "with_email_promoters", value: n0(o.emails), tone: "text-ok", always: true },
          { key: "new_sites", value: n0(o.new_sites) },
          { key: "new_handles", value: n0(o.new_handles) },
          { key: "none", value: n0(o.none), always: true },
        ]
      : [
          { key: "new_emails", value: n0(o.new_emails), tone: "text-ok", always: true },
          { key: "with_email_venues", value: n0(o.emails), tone: "text-ok", always: true },
          { key: "none", value: n0(o.none), always: true },
          { key: "skip", value: n0(o.skip) },
        ];
  base.push({ key: "later", value: n0(o.later), tone: "text-warn" });
  base.push({ key: "errors", value: errors, tone: "text-danger" });
  return base.filter((x) => x.always || x.value > 0);
}

/// The bilan of a finished run: a headline ("42 trouvailles sur 1 800 vus")
/// and one tile per kind of result.
export function RunOutcomeSummary({ run, stats }: { run: ViRun; stats: ViRunStats }) {
  const { t } = useTranslation();
  const o = stats.outcome;
  if (!o) return null;
  const seen = n0(stats.tasks_done);
  const hits = hitsOf(run.type_, o);
  const tiles = tilesFor(run.type_, o, n0(stats.tasks_echec));
  return (
    <div className="mt-3">
      <div className="flex items-center gap-2 text-sm font-semibold">
        <CheckCircle2 size={15} className={hits > 0 ? "text-ok" : "text-fg-subtle"} />
        {t(`report.headline.${run.type_}`, { count: hits, hits: fmtN(hits), seen: fmtN(seen) })}
      </div>
      <div className="mt-2 grid grid-cols-2 gap-2 @sm:grid-cols-3 @xl:grid-cols-6">
        {tiles.map((x) => (
          <div key={x.key} className="border border-border px-3 py-2" title={t(`report.tile_hint.${x.key}`, "")}>
            <div className={clsx("font-condensed text-lg tabular", x.value > 0 ? x.tone ?? "text-fg" : "text-fg-faint")}>
              {fmtN(x.value)}
            </div>
            <div className="text-2xs uppercase tracking-wide text-fg-faint">{t(`report.tile.${x.key}`)}</div>
          </div>
        ))}
      </div>
      {n0(o.later) > 0 && <p className="mt-2 text-2xs text-fg-subtle">{t("report.later_note", { count: n0(o.later) })}</p>}
    </div>
  );
}

const HIT = new Set(["found", "emails", "info"]);
type Filter = "hits" | "none" | "had" | "later" | "error" | "all";

/// Every collective or venue the run looked at, found ones first.
export function RunReportList({ runId, live }: { runId: number; live?: boolean }) {
  const { t } = useTranslation();
  const [filter, setFilter] = useState<Filter>("hits");
  const [limit, setLimit] = useState(200);
  const { data: items = [], isLoading } = useQuery({
    queryKey: ["vi_run_report", runId],
    queryFn: () => viRunReport(runId),
    refetchInterval: live ? 5000 : false,
  });
  const counts = useMemo(() => {
    const c: Record<Filter, number> = { hits: 0, none: 0, had: 0, later: 0, error: 0, all: items.length };
    for (const it of items) {
      if (HIT.has(it.r)) c.hits++;
      else if (it.r === "none" || it.r === "skip") c.none++;
      else if (it.r === "had") c.had++;
      else if (it.r === "later") c.later++;
      else if (it.r === "error") c.error++;
    }
    return c;
  }, [items]);
  const shown = useMemo(
    () =>
      items.filter((it) =>
        filter === "all"
          ? true
          : filter === "hits"
          ? HIT.has(it.r)
          : filter === "none"
          ? it.r === "none" || it.r === "skip"
          : it.r === filter
      ),
    [items, filter]
  );
  if (isLoading) return <div className="mt-3 text-2xs text-fg-subtle">{t("common.loading", "…")}</div>;
  if (items.length === 0) return <div className="mt-3 text-2xs text-fg-subtle">{t("report.empty")}</div>;
  const chips: Filter[] = ["hits", "none", "had", "later", "error", "all"];
  return (
    <div className="mt-3">
      <div className="flex flex-wrap gap-1.5">
        {chips
          .filter((f) => f === "all" || f === "hits" || counts[f] > 0)
          .map((f) => (
            <button
              key={f}
              onClick={() => {
                setFilter(f);
                setLimit(200);
              }}
              className={clsx(
                "rounded-full border px-2.5 py-0.5 text-2xs font-medium",
                filter === f ? "border-accent bg-accent text-accent-fg" : "border-border text-fg-subtle hover:text-fg"
              )}
            >
              {t(`report.filter.${f}`)} · {fmtN(counts[f])}
            </button>
          ))}
      </div>
      <div className="mt-2 max-h-96 divide-y divide-border overflow-auto border border-border">
        {shown.length === 0 && <div className="px-3 py-3 text-2xs text-fg-subtle">{t("report.filter_empty")}</div>}
        {shown.slice(0, limit).map((it) => (
          <div key={it.task_id} className="grid grid-cols-[minmax(0,14rem)_minmax(0,1fr)_auto] items-center gap-3 px-3 py-1.5 text-xs">
            <div className="min-w-0 truncate">
              <span className="font-semibold">{it.name}</span>
              {it.city && <span className="text-fg-faint"> · {it.city}</span>}
            </div>
            <div className="flex min-w-0 flex-wrap items-center gap-x-3 gap-y-0.5">
              {it.handle && <IgHandle handle={it.handle} />}
              {it.site && (
                <button
                  className="inline-flex max-w-[16rem] items-center gap-1 truncate text-2xs text-fg-subtle hover:text-accent"
                  onClick={() => openUrl(it.site!).catch(() => {})}
                >
                  <Globe size={12} className="shrink-0" />
                  <span className="truncate">{it.site.replace(/^https?:\/\/(www\.)?/, "")}</span>
                </button>
              )}
              {it.emails.map((e) => (
                <span key={e} className="inline-flex items-center gap-1 text-2xs text-fg">
                  <Mail size={12} className="text-ok" />
                  {e}
                </span>
              ))}
              {it.r === "error" && <span className="truncate text-2xs text-danger">{translateCode(it.error) ?? it.error}</span>}
            </div>
            <span className={clsx("whitespace-nowrap rounded-full px-2 py-0.5 text-2xs", RESULT_TONE[it.r] ?? "bg-muted text-fg-subtle")}>
              {t(`report.result.${it.r}`, it.r)}
            </span>
          </div>
        ))}
        {shown.length > limit && (
          <button className="w-full px-3 py-2 text-2xs font-medium text-accent" onClick={() => setLimit((l) => l + 300)}>
            {t("report.more", { count: shown.length - limit })}
          </button>
        )}
      </div>
    </div>
  );
}

const RESULT_TONE: Record<string, string> = {
  found: "bg-ok/15 text-ok",
  emails: "bg-ok/15 text-ok",
  info: "bg-ok/10 text-ok",
  had: "bg-muted text-fg-subtle",
  none: "bg-muted text-fg-faint",
  skip: "bg-muted text-fg-faint",
  later: "bg-warn/15 text-warn",
  error: "bg-danger/15 text-danger",
};

// ---------------- Dock ----------------

const DISMISSED_KEY = "run_dock_dismissed";
const FINISHED_WINDOW_MS = 24 * 3600_000;

function readDismissed(): number[] {
  try {
    return JSON.parse(localStorage.getItem(DISMISSED_KEY) ?? "[]");
  } catch {
    return [];
  }
}

/// Bottom-right, on every page: the collectes in progress (bar, what is
/// being looked at right now) and, once one is over, its bilan until it is
/// closed. "Voir le détail" opens the run in Prospection, Lieux, Collectes.
export function RunDock() {
  const { t } = useTranslation();
  const qc = useQueryClient();
  const navigate = useNavigate();
  const [live, setLive] = useState<Record<number, ViRunProgress>>({});
  const [dismissed, setDismissed] = useState<number[]>(readDismissed);
  const { data: runs = [] } = useQuery({
    queryKey: ["vi_runs"],
    queryFn: viListRuns,
    refetchInterval: (q) => ((q.state.data as ViRun[] | undefined)?.some((r) => r.statut === "en_cours") ? 4000 : 60_000),
  });
  useEffect(() => {
    const un = listen<ViRunProgress>("vi:run-progress", (e) => {
      setLive((p) => ({ ...p, [e.payload.run_id]: e.payload }));
      if (e.payload.statut !== "en_cours") {
        qc.invalidateQueries({ queryKey: ["vi_runs"] });
        qc.invalidateQueries({ queryKey: ["vi_run_report", e.payload.run_id] });
        qc.invalidateQueries({ queryKey: ["vi_promoters"] });
        qc.invalidateQueries({ queryKey: ["vi_venues"] });
      }
    });
    return () => {
      un.then((f) => f());
    };
  }, [qc]);
  const dismiss = (id: number) => {
    const next = [...dismissed, id].slice(-200);
    setDismissed(next);
    localStorage.setItem(DISMISSED_KEY, JSON.stringify(next));
  };
  const now = Date.now();
  const visible = runs.filter((r) => {
    if (r.type_?.startsWith("radar") || dismissed.includes(r.id)) return false;
    if (r.statut === "en_cours") return true;
    if (r.statut !== "termine") return false;
    const end = sqlDate(r.finished_at);
    if (!end || now - end.getTime() > FINISHED_WINDOW_MS) return false;
    // Old runs carry no result list: nothing clear to say about them.
    const st = parseStats(r.stats);
    return !REPORTED.has(r.type_) || !!st?.outcome;
  });
  if (visible.length === 0) return null;
  const open = (id: number) => navigate(`/contacts?tab=venues&view=runs&run=${id}`);
  return (
    <div className="pointer-events-none fixed bottom-4 right-4 z-40 flex w-[22rem] flex-col gap-2">
      {visible.slice(0, 3).map((r) => {
        const p = live[r.id];
        const active = (p?.statut ?? r.statut) === "en_cours";
        const total = p?.tasks_total ?? r.tasks_total;
        const done = (p?.tasks_done ?? r.tasks_done) + (p?.tasks_echec ?? r.tasks_echec);
        const pct = total ? Math.round((done / total) * 100) : 0;
        const st = parseStats(r.stats);
        return (
          <div key={r.id} className="card pointer-events-auto p-3 shadow-lg">
            <div className="flex items-center gap-2">
              {active ? (
                <RefreshCw size={13} className="shrink-0 animate-spin text-accent" />
              ) : (
                <CheckCircle2 size={14} className="shrink-0 text-ok" />
              )}
              <span className="truncate text-xs font-semibold">
                {runKindLabel(t, r.type_)} · {active ? t("report.dock_running") : t("report.dock_done", { at: fmtClock(sqlDate(r.finished_at)) })}
              </span>
              {!active && (
                <button className="ml-auto text-fg-faint hover:text-fg" onClick={() => dismiss(r.id)} aria-label={t("common.close", "Fermer")}>
                  <X size={14} />
                </button>
              )}
            </div>
            {active ? (
              <>
                <div className="mt-2 flex items-center gap-2">
                  <div className="h-1.5 flex-1 overflow-hidden bg-muted">
                    <div className="h-full bg-accent transition-all" style={{ width: `${pct}%` }} />
                  </div>
                  <span className="text-2xs tabular text-fg-subtle">
                    {fmtN(done)}/{fmtN(total)}
                  </span>
                </div>
                {p?.args && typeof (p.args as any).name === "string" && (
                  <div className="mt-1 truncate text-2xs text-fg-subtle">{t("report.dock_looking", { name: (p.args as any).name })}</div>
                )}
              </>
            ) : (
              <div className="mt-1 text-2xs text-fg-subtle">{dockLine(t, r, st)}</div>
            )}
            <button className="mt-2 inline-flex items-center gap-1 text-2xs font-medium text-accent" onClick={() => open(r.id)}>
              {t("report.see_detail")} <ExternalLink size={11} />
            </button>
          </div>
        );
      })}
    </div>
  );
}

function dockLine(t: (k: string, o?: Record<string, unknown>) => string, r: ViRun, st: ViRunStats | null) {
  if (!st) return "";
  if (st.outcome) {
    const hits = hitsOf(r.type_, st.outcome);
    return t(`report.headline.${r.type_}`, { count: hits, hits: fmtN(hits), seen: fmtN(n0(st.tasks_done)) });
  }
  return t("report.headline.harvest", { venues: fmtN(n0(st.venues_new)), evidence: fmtN(n0(st.evidence_new)) });
}
