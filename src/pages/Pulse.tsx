import { useMemo, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import {
  Activity,
  Camera,
  ListMusic,
  Music2,
  Plus,
  RefreshCw,
  Copy,
  Archive,
  ArchiveRestore,
} from "lucide-react";
import clsx from "clsx";
import {
  listArtists,
  pulseKpis,
  pulseSeries,
  pulseSnapshot,
  pulseTrackedAdd,
  pulseTrackedList,
  pulseTrackedToggle,
  pulseWatchlist,
  pulseWatchlistAdd,
  pulseWatchlistToggle,
} from "../lib/api";
import type { PulsePoint } from "../lib/types";
import { PageHeader, EmptyState, Loader } from "../components/Layout";
import { Field, useToast } from "../components/ui";
import { euro } from "../lib/format";

type Tab = "overview" | "playlists" | "planner";

export function Pulse() {
  const { t } = useTranslation();
  const [tab, setTab] = useState<Tab>("overview");
  const tabs: Tab[] = ["overview", "playlists", "planner"];
  return (
    <div>
      <PageHeader kicker="PULSE" title={t("pulse.title")} subtitle={t("pulse.subtitle")} />
      <div className="px-8 py-6">
        <div className="segmented mb-6">
          {tabs.map((tb) => (
            <button key={tb} data-active={tab === tb} className="segmented-item" onClick={() => setTab(tb)}>
              {t(`pulse.tab_${tb}`)}
            </button>
          ))}
        </div>
        {tab === "overview" && <OverviewTab />}
        {tab === "playlists" && <PlaylistsTab />}
        {tab === "planner" && <PlannerTab />}
      </div>
    </div>
  );
}

// ---------------- Overview ----------------

function OverviewTab() {
  const { t } = useTranslation();
  const toast = useToast();
  const qc = useQueryClient();
  const { data: artists = [] } = useQuery({ queryKey: ["artists"], queryFn: listArtists });
  const spotifyArtists = artists.filter((a) => a.spotify_artist_id?.trim());
  const [artistSid, setArtistSid] = useState<string>("");
  const effectiveArtist = artistSid || spotifyArtists[0]?.spotify_artist_id || "";

  const { data: kpis, isLoading: kpisLoading } = useQuery({
    queryKey: ["pulse_kpis", effectiveArtist],
    queryFn: () => pulseKpis(effectiveArtist || undefined),
  });
  const { data: tracked = [] } = useQuery({ queryKey: ["pulse_tracked"], queryFn: pulseTrackedList });
  const [trackSid, setTrackSid] = useState<string>("");
  const effectiveTrack = trackSid || tracked.find((x) => x.is_active)?.track_spotify_id || "";
  const [days, setDays] = useState(90);

  const { data: series } = useQuery({
    queryKey: ["pulse_series", effectiveArtist, effectiveTrack, days],
    queryFn: () => pulseSeries({ artist: effectiveArtist, track: effectiveTrack, days }),
  });

  const [snapping, setSnapping] = useState(false);
  const snap = async () => {
    setSnapping(true);
    try {
      const r = await pulseSnapshot();
      qc.invalidateQueries({ queryKey: ["pulse_kpis"] });
      qc.invalidateQueries({ queryKey: ["pulse_series"] });
      qc.invalidateQueries({ queryKey: ["pulse_tracked"] });
      qc.invalidateQueries({ queryKey: ["pulse_watchlist"] });
      if (r.errors.length) toast(r.errors[0], "error");
      else
        toast(
          t("pulse.snap_done", { a: r.artists, t: r.tracks, p: r.playlists, s: r.spend_rows }),
          "ok"
        );
    } catch (e: any) {
      toast(e?.toString?.() ?? "error", "error");
    } finally {
      setSnapping(false);
    }
  };

  const [newTrack, setNewTrack] = useState("");
  const addTrack = useMutation({
    mutationFn: () => pulseTrackedAdd(newTrack.trim()),
    onSuccess: () => {
      setNewTrack("");
      qc.invalidateQueries({ queryKey: ["pulse_tracked"] });
      toast(t("common.save"), "ok");
    },
    onError: (e: any) => toast(e?.toString?.() ?? "error", "error"),
  });
  const toggleTrack = useMutation({
    mutationFn: (p: { id: number; active: boolean }) => pulseTrackedToggle(p.id, p.active),
    onSuccess: () => qc.invalidateQueries({ queryKey: ["pulse_tracked"] }),
  });

  const delta = (d?: number | null) =>
    d == null ? null : (
      <span className={clsx("text-2xs font-bold tabular", d > 0 ? "text-emerald-500" : d < 0 ? "text-accent" : "text-fg-faint")}>
        {d > 0 ? "+" : ""}
        {d}
      </span>
    );

  if (spotifyArtists.length === 0) {
    return (
      <div className="card">
        <EmptyState icon={Activity} title={t("pulse.no_artist")} hint={t("pulse.no_artist_hint")} />
      </div>
    );
  }

  return (
    <div className="space-y-5">
      {/* Controls */}
      <div className="flex flex-wrap items-center gap-2">
        <select className="input w-auto" value={artistSid} onChange={(e) => setArtistSid(e.target.value)}>
          {spotifyArtists.map((a) => (
            <option key={a.id} value={a.spotify_artist_id!}>
              {a.name}
            </option>
          ))}
        </select>
        <select className="input w-auto" value={trackSid} onChange={(e) => setTrackSid(e.target.value)}>
          <option value="">{t("pulse.pick_track")}</option>
          {tracked
            .filter((x) => x.is_active)
            .map((x) => (
              <option key={x.id} value={x.track_spotify_id}>
                {x.name || x.track_spotify_id}
              </option>
            ))}
        </select>
        <div className="segmented">
          {[30, 90, 180].map((d) => (
            <button key={d} data-active={days === d} className="segmented-item" onClick={() => setDays(d)}>
              {d}j
            </button>
          ))}
        </div>
        <button className="btn-primary ml-auto" onClick={snap} disabled={snapping}>
          <Camera size={15} className={snapping ? "animate-pulse" : ""} />
          {snapping ? t("pulse.snapping") : t("pulse.snapshot_now")}
        </button>
      </div>
      {kpis?.last_snapshot && (
        <p className="text-2xs text-fg-faint">
          {t("pulse.last_snapshot")}: {kpis.last_snapshot}
        </p>
      )}

      {/* KPI bento */}
      {kpisLoading ? (
        <Loader />
      ) : (
        <div className="grid grid-cols-2 gap-2 lg:grid-cols-4">
          <div className="border-2 border-border px-4 py-3">
            <div className="flex items-baseline gap-2">
              <span className="text-2xl font-black tabular text-accent">{kpis?.artist_popularity ?? "·"}</span>
              {delta(kpis?.artist_delta7)}
            </div>
            <div className="text-2xs uppercase tracking-wide text-fg-faint">{t("pulse.k_artist_pop")}</div>
          </div>
          <div className="border-2 border-border px-4 py-3">
            <div className="flex items-baseline gap-2">
              <span className="truncate text-sm font-black">{kpis?.best_track_name ?? "·"}</span>
              <span className="text-2xl font-black tabular text-accent-2">{kpis?.best_track_popularity ?? ""}</span>
              {delta(kpis?.best_track_delta7)}
            </div>
            <div className="text-2xs uppercase tracking-wide text-fg-faint">{t("pulse.k_best_track")}</div>
          </div>
          <div className="border-2 border-border px-4 py-3">
            <div className="text-2xl font-black tabular">{euro(kpis?.spend_30d ?? 0)}</div>
            <div className="text-2xs uppercase tracking-wide text-fg-faint">{t("pulse.k_spend30")}</div>
          </div>
          <div className="border-2 border-border px-4 py-3">
            <div className="flex items-baseline gap-2">
              <span className="text-2xl font-black tabular">
                {kpis?.cpr_7d != null ? kpis.cpr_7d.toFixed(2) : "·"}
              </span>
              <span className="text-2xs text-fg-subtle">
                {t("pulse.vs")} {kpis?.cpr_30d != null ? kpis.cpr_30d.toFixed(2) : "·"} (30j)
              </span>
            </div>
            <div className="text-2xs uppercase tracking-wide text-fg-faint">{t("pulse.k_cpr")}</div>
          </div>
        </div>
      )}

      {/* Overlay chart */}
      <div className="card p-5">
        <div className="kicker mb-3">{t("pulse.chart_title")}</div>
        {series && series.points.some((p) => p.artist_pop != null || p.track_pop != null || p.spend > 0) ? (
          <OverlayChart points={series.points} releases={series.releases} />
        ) : (
          <p className="py-10 text-center text-xs text-fg-subtle">{t("pulse.chart_empty")}</p>
        )}
        <div className="mt-3 flex flex-wrap gap-4 text-2xs text-fg-subtle">
          <span className="inline-flex items-center gap-1.5">
            <span className="inline-block h-0.5 w-5 bg-accent" /> {t("pulse.legend_artist")}
          </span>
          <span className="inline-flex items-center gap-1.5">
            <span className="inline-block h-0.5 w-5 bg-emerald-500" /> {t("pulse.legend_track")}
          </span>
          <span className="inline-flex items-center gap-1.5">
            <span className="inline-block h-2.5 w-2.5 bg-accent-2/40" /> {t("pulse.legend_spend")}
          </span>
          <span className="inline-flex items-center gap-1.5">
            <span className="inline-block h-3 w-0.5 border-l-2 border-dashed border-fg-subtle" /> {t("pulse.legend_release")}
          </span>
        </div>
      </div>

      {/* Tracked tracks */}
      <div className="card p-5">
        <div className="mb-3 flex items-center justify-between">
          <div className="kicker">{t("pulse.tracked_title")}</div>
        </div>
        <div className="mb-3 flex gap-2">
          <input
            className="input flex-1"
            placeholder={t("pulse.track_placeholder")}
            value={newTrack}
            onChange={(e) => setNewTrack(e.target.value)}
          />
          <button className="btn-outline" disabled={!newTrack.trim() || addTrack.isPending} onClick={() => addTrack.mutate()}>
            <Plus size={15} />
            {t("common.add")}
          </button>
        </div>
        {tracked.length === 0 ? (
          <p className="text-xs text-fg-subtle">{t("pulse.tracked_empty")}</p>
        ) : (
          <div className="flex flex-wrap gap-2">
            {tracked.map((x) => (
              <span
                key={x.id}
                className={clsx(
                  "inline-flex items-center gap-2 border-2 px-2.5 py-1 text-xs font-semibold",
                  x.is_active ? "border-border" : "border-border text-fg-faint line-through"
                )}
              >
                <Music2 size={12} className="text-accent-2" />
                {x.name || x.track_spotify_id}
                {x.latest_popularity != null && (
                  <span className="tabular font-black text-accent">{x.latest_popularity}</span>
                )}
                <button
                  className="text-fg-faint hover:text-accent"
                  aria-label={x.is_active ? t("pulse.archive") : t("pulse.unarchive")}
                  onClick={() => toggleTrack.mutate({ id: x.id, active: !x.is_active })}
                >
                  {x.is_active ? <Archive size={12} /> : <ArchiveRestore size={12} />}
                </button>
              </span>
            ))}
          </div>
        )}
      </div>
    </div>
  );
}

// ---------------- Custom SVG overlay chart ----------------

function OverlayChart({ points, releases }: { points: PulsePoint[]; releases: [string, string][] }) {
  const W = 800;
  const H = 240;
  const PAD = 8;
  const maxSpend = Math.max(1, ...points.map((p) => p.spend));
  const n = points.length;
  const xw = (W - PAD * 2) / Math.max(1, n);
  const x = (i: number) => PAD + i * xw + xw / 2;
  const yPop = (v: number) => H - PAD - ((H - PAD * 2) * v) / 100;
  const ySpend = (v: number) => H - PAD - ((H - PAD * 2) * v) / maxSpend;

  const line = (get: (p: PulsePoint) => number | null | undefined) => {
    // Connect only known points; gaps stay gaps (no interpolation lies).
    let d = "";
    let pen = false;
    points.forEach((p, i) => {
      const v = get(p);
      if (v == null) {
        pen = false;
        return;
      }
      d += `${pen ? "L" : "M"}${x(i).toFixed(1)},${yPop(v).toFixed(1)} `;
      pen = true;
    });
    return d;
  };

  const releaseIdx = releases
    .map(([date, name]) => ({ i: points.findIndex((p) => p.date === date), name }))
    .filter((r) => r.i >= 0);

  const first = points[0]?.date?.slice(5) ?? "";
  const mid = points[Math.floor(n / 2)]?.date?.slice(5) ?? "";
  const last = points[n - 1]?.date?.slice(5) ?? "";

  return (
    <div>
      <div className="relative w-full overflow-hidden">
        <svg viewBox={`0 0 ${W} ${H}`} className="w-full" style={{ height: 240 }} preserveAspectRatio="none">
          {/* grid */}
          {[25, 50, 75].map((v) => (
            <line key={v} x1={PAD} x2={W - PAD} y1={yPop(v)} y2={yPop(v)} stroke="currentColor" opacity={0.08} />
          ))}
          {/* spend bars (right axis) */}
          {points.map((p, i) =>
            p.spend > 0 ? (
              <rect
                key={i}
                x={x(i) - Math.max(1, xw * 0.35)}
                y={ySpend(p.spend)}
                width={Math.max(2, xw * 0.7)}
                height={H - PAD - ySpend(p.spend)}
                className="fill-accent-2"
                opacity={0.35}
              >
                <title>{`${p.date} · ${p.spend.toFixed(2)} EUR`}</title>
              </rect>
            ) : null
          )}
          {/* release reference lines */}
          {releaseIdx.map((r, k) => (
            <g key={k}>
              <line
                x1={x(r.i)}
                x2={x(r.i)}
                y1={PAD}
                y2={H - PAD}
                stroke="currentColor"
                strokeDasharray="4 4"
                opacity={0.45}
              />
              <title>{r.name}</title>
            </g>
          ))}
          {/* popularity lines (left axis 0-100) */}
          <path d={line((p) => p.artist_pop)} fill="none" className="stroke-accent" strokeWidth={2} />
          <path d={line((p) => p.track_pop)} fill="none" className="stroke-emerald-500" strokeWidth={2} />
        </svg>
        {/* left/right axis hints */}
        <div className="pointer-events-none absolute left-1 top-1 text-2xs tabular text-fg-faint">100</div>
        <div className="pointer-events-none absolute bottom-1 left-1 text-2xs tabular text-fg-faint">0</div>
        <div className="pointer-events-none absolute right-1 top-1 text-2xs tabular text-fg-faint">
          {maxSpend.toFixed(0)}€
        </div>
      </div>
      <div className="mt-1 flex justify-between text-2xs tabular text-fg-faint">
        <span>{first}</span>
        <span>{mid}</span>
        <span>{last}</span>
      </div>
    </div>
  );
}

// ---------------- Playlists watchlist ----------------

function Spark({ values }: { values: number[] }) {
  if (values.length < 2) return <span className="text-2xs text-fg-faint">·</span>;
  const min = Math.min(...values);
  const max = Math.max(...values);
  const range = Math.max(1, max - min);
  const W = 90;
  const H = 22;
  const pts = values
    .map((v, i) => `${((i / (values.length - 1)) * W).toFixed(1)},${(H - ((v - min) / range) * H).toFixed(1)}`)
    .join(" ");
  return (
    <svg width={W} height={H} className="text-accent-2">
      <polyline points={pts} fill="none" stroke="currentColor" strokeWidth={1.5} />
    </svg>
  );
}

function PlaylistsTab() {
  const { t } = useTranslation();
  const toast = useToast();
  const qc = useQueryClient();
  const { data: rows = [], isLoading } = useQuery({ queryKey: ["pulse_watchlist"], queryFn: pulseWatchlist });
  const [input, setInput] = useState("");

  const add = useMutation({
    mutationFn: () => pulseWatchlistAdd(input.trim()),
    onSuccess: () => {
      setInput("");
      qc.invalidateQueries({ queryKey: ["pulse_watchlist"] });
      toast(t("common.save"), "ok");
    },
    onError: (e: any) => toast(e?.toString?.() ?? "error", "error"),
  });
  const toggle = useMutation({
    mutationFn: (p: { id: number; active: boolean }) => pulseWatchlistToggle(p.id, p.active),
    onSuccess: () => qc.invalidateQueries({ queryKey: ["pulse_watchlist"] }),
  });

  const dcell = (d?: number | null) =>
    d == null ? (
      <span className="text-fg-faint">·</span>
    ) : (
      <span className={clsx("font-bold tabular", d > 0 ? "text-emerald-500" : d < 0 ? "text-accent" : "text-fg-faint")}>
        {d > 0 ? "+" : ""}
        {d.toLocaleString()}
      </span>
    );

  return (
    <div className="space-y-4">
      <div className="flex gap-2">
        <input
          className="input max-w-xl flex-1"
          placeholder={t("pulse.playlist_placeholder")}
          value={input}
          onChange={(e) => setInput(e.target.value)}
        />
        <button className="btn-primary" disabled={!input.trim() || add.isPending} onClick={() => add.mutate()}>
          <Plus size={15} />
          {t("common.add")}
        </button>
      </div>
      <p className="text-2xs text-fg-subtle">{t("pulse.playlist_hint")}</p>

      {isLoading ? (
        <Loader />
      ) : rows.length === 0 ? (
        <div className="card">
          <EmptyState icon={ListMusic} title={t("pulse.watchlist_empty")} />
        </div>
      ) : (
        <div className="card overflow-x-auto">
          <table className="tbl">
            <thead>
              <tr>
                <th>{t("pulse.w_name")}</th>
                <th className="text-right">{t("pulse.w_followers")}</th>
                <th className="text-right">Δ 7j</th>
                <th className="text-right">Δ 30j</th>
                <th>{t("pulse.w_trend")}</th>
                <th></th>
              </tr>
            </thead>
            <tbody>
              {rows.map((r) => (
                <tr key={r.id} className={clsx(!r.is_active && "opacity-45")}>
                  <td>
                    <div className="font-semibold">
                      {r.name || r.playlist_spotify_id}
                      {r.contains_our_track && (
                        <span className="badge ml-2 bg-emerald-500/15 text-emerald-500">{t("pulse.w_ours")}</span>
                      )}
                    </div>
                    {r.owner_name && <div className="text-2xs text-fg-subtle">{r.owner_name}</div>}
                  </td>
                  <td className="text-right font-bold tabular">{r.followers?.toLocaleString() ?? "·"}</td>
                  <td className="text-right">{dcell(r.delta7)}</td>
                  <td className="text-right">{dcell(r.delta30)}</td>
                  <td>
                    <Spark values={r.spark} />
                  </td>
                  <td className="text-right">
                    <button
                      className="btn-ghost px-2 py-1.5"
                      aria-label={r.is_active ? t("pulse.archive") : t("pulse.unarchive")}
                      title={r.is_active ? t("pulse.archive") : t("pulse.unarchive")}
                      onClick={() => toggle.mutate({ id: r.id, active: !r.is_active })}
                    >
                      {r.is_active ? <Archive size={14} /> : <ArchiveRestore size={14} />}
                    </button>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </div>
  );
}

// ---------------- Front-load budget planner ----------------

function frontLoad(budget: number, days: number, minPerDay = 5): number[] {
  if (budget <= 0 || days <= 0) return [];
  // Exponential decay tuned so roughly 30% of the budget lands on day 1-2,
  // then clamp to the floor and redistribute the remainder proportionally.
  const k = days > 2 ? Math.max(0.35, 2.2 / days) * 2 : 0.5;
  const weights = Array.from({ length: days }, (_, i) => Math.exp(-k * i));
  const wsum = weights.reduce((a, b) => a + b, 0);
  let alloc = weights.map((w) => (budget * w) / wsum);
  // Enforce the floor without exceeding the budget.
  if (budget >= minPerDay * days) {
    for (let pass = 0; pass < 5; pass++) {
      const flooredIdx = alloc.map((v, i) => (v < minPerDay ? i : -1)).filter((i) => i >= 0);
      if (!flooredIdx.length) break;
      flooredIdx.forEach((i) => (alloc[i] = minPerDay));
      const fixed = flooredIdx.length * minPerDay;
      const freeIdx = alloc.map((_, i) => i).filter((i) => !flooredIdx.includes(i));
      const freeSum = freeIdx.reduce((a, i) => a + alloc[i], 0);
      const target = budget - fixed;
      freeIdx.forEach((i) => (alloc[i] = (alloc[i] * target) / Math.max(1, freeSum)));
    }
  }
  // Round to cents and push the rounding drift onto day 1.
  alloc = alloc.map((v) => Math.round(v * 100) / 100);
  const drift = Math.round((budget - alloc.reduce((a, b) => a + b, 0)) * 100) / 100;
  alloc[0] = Math.round((alloc[0] + drift) * 100) / 100;
  return alloc;
}

function PlannerTab() {
  const { t } = useTranslation();
  const toast = useToast();
  const [budget, setBudget] = useState(300);
  const [days, setDays] = useState(14);
  const [startDate, setStartDate] = useState(() => new Date().toISOString().slice(0, 10));

  const alloc = useMemo(() => frontLoad(budget, days), [budget, days]);
  const maxA = Math.max(1, ...alloc);
  const dates = useMemo(
    () =>
      alloc.map((_, i) => {
        const d = new Date(startDate + "T00:00:00");
        d.setDate(d.getDate() + i);
        return d.toISOString().slice(0, 10);
      }),
    [alloc, startDate]
  );

  const copyTable = () => {
    const text = alloc.map((v, i) => `${dates[i]}\t${v.toFixed(2)}`).join("\n");
    navigator.clipboard?.writeText(text).then(
      () => toast(t("pulse.copied"), "ok"),
      () => {}
    );
  };

  return (
    <div className="grid grid-cols-1 gap-5 lg:grid-cols-[320px_1fr]">
      <div className="card h-fit space-y-3 p-5">
        <div className="kicker">{t("pulse.planner_title")}</div>
        <p className="text-2xs leading-relaxed text-fg-subtle">{t("pulse.planner_hint")}</p>
        <Field label={t("pulse.p_budget")}>
          <input type="number" className="input" value={budget} min={10} onChange={(e) => setBudget(Number(e.target.value))} />
        </Field>
        <Field label={t("pulse.p_days")}>
          <input type="number" className="input" value={days} min={2} max={60} onChange={(e) => setDays(Number(e.target.value))} />
        </Field>
        <Field label={t("pulse.p_start")}>
          <input type="date" className="input" value={startDate} onChange={(e) => setStartDate(e.target.value)} />
        </Field>
        <button className="btn-outline w-full" onClick={copyTable} disabled={!alloc.length}>
          <Copy size={14} />
          {t("pulse.copy_table")}
        </button>
      </div>

      <div className="card p-5">
        {alloc.length === 0 ? (
          <p className="py-10 text-center text-xs text-fg-subtle">{t("pulse.planner_empty")}</p>
        ) : (
          <>
            <div className="mb-4 flex items-end gap-[3px]" style={{ height: 140 }}>
              {alloc.map((v, i) => (
                <div
                  key={i}
                  className="flex-1 bg-accent-2/70 transition-all"
                  style={{ height: `${(v / maxA) * 100}%`, minWidth: 3 }}
                  title={`${dates[i]} · ${v.toFixed(2)} EUR`}
                />
              ))}
            </div>
            <div className="max-h-64 overflow-auto">
              <table className="tbl">
                <thead>
                  <tr>
                    <th>{t("pulse.p_date")}</th>
                    <th className="text-right">{t("pulse.p_daily")}</th>
                    <th className="text-right">{t("pulse.p_cumul")}</th>
                  </tr>
                </thead>
                <tbody>
                  {alloc.map((v, i) => {
                    const cumul = alloc.slice(0, i + 1).reduce((a, b) => a + b, 0);
                    return (
                      <tr key={i}>
                        <td className="tabular">{dates[i]}</td>
                        <td className="text-right font-bold tabular">{v.toFixed(2)} €</td>
                        <td className="text-right tabular text-fg-subtle">
                          {cumul.toFixed(2)} € ({((cumul / budget) * 100).toFixed(0)}%)
                        </td>
                      </tr>
                    );
                  })}
                </tbody>
              </table>
            </div>
            <p className="mt-3 flex items-center gap-1.5 text-2xs text-fg-faint">
              <RefreshCw size={11} />
              {t("pulse.planner_note")}
            </p>
          </>
        )}
      </div>
    </div>
  );
}
