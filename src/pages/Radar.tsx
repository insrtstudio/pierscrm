import { useEffect, useRef, useState } from "react";
import { keepPreviousData, useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { useDebounced } from "../lib/hooks";
import { listen } from "@tauri-apps/api/event";
import { openUrl } from "@tauri-apps/plugin-opener";
import {
  Radar as RadarIcon,
  Play,
  RefreshCw,
  Mail,
  ExternalLink,
  UserPlus,
  Check,
  Music2,
  AlertTriangle,
  ShieldCheck,
  Ban,
  RotateCcw,
  UserMinus,
  Search,
  Send,
} from "lucide-react";
import clsx from "clsx";
import {
  radarEnrich,
  radarHarvest,
  radarList,
  radarReject,
  radarRestore,
  radarUnpromote,
  radarPromote,
  radarPromoteBulk,
  radarRepairContacts,
  radarStats,
  viListRuns,
} from "../lib/api";
import type { ViRunProgress } from "../lib/types";
import { PageHeader, EmptyState } from "../components/Layout";
import { useConfirm, useToast } from "../components/ui";
import { backendError, progressText } from "../lib/backendMessages";

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

/// Every source is queried on every scan. Picking sources was a decision the user
/// had no basis to make (and "spotify" silently returned nothing without API
/// keys, "labels" never matched at all), so the scan just casts the wide net and
/// reports what each source gave back.
const SOURCES = ["spotify", "deezer", "youtube", "labels"];

type Filter = "all" | "email" | "official" | "pipeline" | "rejected";
type Kind = "" | "playlist" | "label" | "channel";

/// Rows fetched per "load more" step. The command has no offset, so paging just
/// raises the limit.
const PAGE = 500;

export function Radar() {
  const { t } = useTranslation();
  const toast = useToast();
  const confirm = useConfirm();
  const qc = useQueryClient();

  const [genres, setGenres] = useState<string[]>(["tech house", "house"]);
  const [customGenre, setCustomGenre] = useState("");
  // The scan's run id. Component state is lost when the user leaves the page, so
  // on mount it is restored from the runs table (a radar run still 'en_cours').
  const [runId, setRunId] = useState<number | null>(null);
  const running = runId != null;
  const [progress, setProgress] = useState<ViRunProgress | null>(null);
  const [limit, setLimit] = useState(PAGE);

  const [filter, setFilter] = useState<Filter>("all");
  const [kind, setKind] = useState<Kind>("");
  const [search, setSearch] = useState("");
  const dSearch = useDebounced(search, 250);
  // Rows ticked for a bulk reject (or restore, in the rejected view).
  const [selected, setSelected] = useState<Set<number>>(new Set());
  useEffect(() => {
    setLimit(PAGE);
    setSelected(new Set());
  }, [filter, kind, dSearch]);

  const { data: stats } = useQuery({
    queryKey: ["radar_stats"],
    queryFn: radarStats,
    refetchInterval: running ? 4000 : false,
  });
  const {
    data: rows = [],
    isError,
    error,
    refetch,
  } = useQuery({
    queryKey: ["radar_list", filter, kind, dSearch, limit],
    queryFn: () => radarList({ filter, kind: kind || undefined, search: dSearch, limit }),
    placeholderData: keepPreviousData,
    refetchInterval: running ? 4000 : false,
  });

  useEffect(() => {
    let alive = true;
    viListRuns()
      .then((runs) => {
        const r = runs.find((x) => x.type_.startsWith("radar_") && x.statut === "en_cours");
        if (alive && r) setRunId((cur) => cur ?? r.id);
      })
      .catch(() => {});
    return () => {
      alive = false;
    };
  }, []);

  // Venues runs share this event, so only our own run may stop the spinner.
  const runIdRef = useRef(runId);
  runIdRef.current = runId;
  useEffect(() => {
    const un = listen<ViRunProgress>("vi:run-progress", (e) => {
      if (e.payload.run_id !== runIdRef.current) return;
      setProgress(e.payload);
      if (e.payload.statut !== "en_cours") {
        setRunId(null);
        qc.invalidateQueries({ queryKey: ["radar_list"] });
        qc.invalidateQueries({ queryKey: ["radar_stats"] });
        qc.invalidateQueries({ queryKey: ["vi_runs"] });
      }
    });
    return () => {
      un.then((f) => f());
    };
  }, [qc]);

  const toggleGenre = (g: string) =>
    setGenres((s) => (s.includes(g) ? s.filter((x) => x !== g) : [...s, g]));

  const scan = async () => {
    const gs = [...genres];
    if (customGenre.trim()) gs.push(customGenre.trim());
    if (!gs.length) return toast(t("radar.pick_hint"), "error");
    setRunId(-1);
    setProgress(null);
    try {
      // One call covers finding curators and then hunting their addresses.
      setRunId(await radarHarvest(gs, SOURCES));
      qc.invalidateQueries({ queryKey: ["vi_runs"] });
      toast(t("radar.scan_started"), "ok");
    } catch (e: any) {
      setRunId(null);
      toast(backendError(e), "error");
    }
  };

  // Manual re-run of the email hunt over every curator still without a usable
  // address, including those already searched once.
  const retryEmails = async () => {
    setRunId(-1);
    setProgress(null);
    try {
      setRunId(await radarEnrich(true));
      qc.invalidateQueries({ queryKey: ["vi_runs"] });
      toast(t("radar.retry_started"), "ok");
    } catch (e: any) {
      setRunId(null);
      toast(backendError(e), "error");
    }
  };
  // Same hunt for a single curator, whatever it already has on file.
  const retryOne = async (id: number) => {
    setRunId(-1);
    setProgress(null);
    try {
      setRunId(await radarEnrich(true, [id]));
      qc.invalidateQueries({ queryKey: ["vi_runs"] });
      toast(t("radar.retry_one_started"), "ok");
    } catch (e: any) {
      setRunId(null);
      toast(backendError(e), "error");
    }
  };

  // A promotion creates a contact: every view that counts or lists contacts
  // (contacts list, dashboard tiles, campaign audiences) has to refetch.
  const refreshAfterPromote = () => {
    qc.invalidateQueries({ queryKey: ["radar_list"] });
    qc.invalidateQueries({ queryKey: ["radar_stats"] });
    qc.invalidateQueries({ queryKey: ["contacts"] });
    qc.invalidateQueries({ queryKey: ["dashboard"] });
    qc.invalidateQueries({ queryKey: ["campaigns"] });
  };
  const promote = useMutation({
    mutationFn: (id: number) => radarPromote(id),
    onSuccess: () => {
      refreshAfterPromote();
      toast(t("radar.promoted"), "ok");
    },
    onError: (e: any) => toast(backendError(e), "error"),
  });
  const promoteBulk = useMutation({
    mutationFn: () => radarPromoteBulk(),
    onSuccess: (n) => {
      refreshAfterPromote();
      toast(t("radar.promoted_bulk", { count: n }), "ok");
    },
    onError: (e: any) => toast(backendError(e), "error"),
  });
  const reject = useMutation({
    mutationFn: (ids: number[]) => radarReject(ids),
    onSuccess: (n) => {
      setSelected(new Set());
      refreshAfterPromote();
      toast(t("radar.rejected_n", { count: n }), "ok");
    },
    onError: (e: any) => toast(backendError(e), "error"),
  });
  const restore = useMutation({
    mutationFn: (ids: number[]) => radarRestore(ids),
    onSuccess: (n) => {
      setSelected(new Set());
      refreshAfterPromote();
      toast(t("radar.restored_n", { count: n }), "ok");
    },
    onError: (e: any) => toast(backendError(e), "error"),
  });
  const unpromote = useMutation({
    mutationFn: (id: number) => radarUnpromote(id),
    onSuccess: (deleted) => {
      refreshAfterPromote();
      toast(t(deleted ? "radar.unpromoted_deleted" : "radar.unpromoted_unlinked"), "ok");
    },
    onError: (e: any) => toast(backendError(e), "error"),
  });
  const repair = useMutation({
    mutationFn: () => radarRepairContacts(),
    onSuccess: (n) => {
      refreshAfterPromote();
      toast(t("radar.repaired", { count: n }), "ok");
    },
    onError: (e: any) => toast(backendError(e), "error"),
  });

  const filters: { k: Filter; label: string; count?: number }[] = [
    { k: "all", label: t("radar.f_all"), count: stats?.total },
    { k: "email", label: t("radar.f_email"), count: stats?.with_email },
    { k: "official", label: t("radar.f_official"), count: stats?.official },
    { k: "pipeline", label: t("radar.f_pipeline"), count: stats?.in_pipeline },
    { k: "rejected", label: t("radar.f_rejected"), count: stats?.rejected },
  ];
  const kinds: { k: Kind; label: string; count?: number }[] = [
    { k: "", label: t("radar.kind_all") },
    { k: "playlist", label: t("radar.kind_playlist"), count: stats?.playlists },
    { k: "label", label: t("radar.kind_label"), count: stats?.labels },
    { k: "channel", label: t("radar.kind_channel"), count: stats?.channels },
  ];
  // Counters are per filter, not per kind: only trust them with no kind set.
  const total = dSearch || kind ? undefined : filters.find((f) => f.k === filter)?.count;
  const rejectedView = filter === "rejected";
  const toggle = (id: number) =>
    setSelected((s) => {
      const n = new Set(s);
      n.has(id) ? n.delete(id) : n.add(id);
      return n;
    });
  const allSelected = rows.length > 0 && rows.every((r) => selected.has(r.id));
  const capped = rows.length >= limit;
  // Computed server-side: with_email - in_pipeline went negative (and hid the
  // button) as soon as a promoted curator's only address stopped counting.
  const addressable = stats?.addressable ?? 0;
  const filtered = filter !== "all" || !!kind || !!dSearch.trim();

  return (
    <div>
      <PageHeader title={t("radar.title")} subtitle={t("radar.subtitle")} />
      <div className="space-y-5 px-page py-6">
        {/* Scan: pick genres, press once. */}
        <div className="card p-5">
          <div className="kicker mb-3">{t("radar.new_scan")}</div>
          <div className="mb-4 flex flex-wrap gap-1.5">
            {GENRE_PRESETS.map((g) => (
              <button
                key={g}
                onClick={() => toggleGenre(g)}
                className={clsx(
                  "badge cursor-pointer border",
                  genres.includes(g)
                    ? "border-accent bg-accent text-accent-fg"
                    : "border-border text-fg-subtle hover:border-accent-2"
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
          <div className="flex flex-wrap items-center gap-3">
            <button className="btn-primary" onClick={scan} disabled={running}>
              <Play size={15} />
              {running ? t("radar.scanning") : t("radar.scan")}
            </button>
            <span className="text-2xs text-fg-subtle">{t("radar.scan_hint")}</span>
            <button
              className="btn-outline ml-auto"
              onClick={retryEmails}
              disabled={running}
              title={t("radar.retry_emails_hint")}
            >
              <Search size={14} />
              {t("radar.retry_emails")}
            </button>
          </div>
          {running && progress && (
            <div className="mt-3 flex items-center gap-2 text-2xs text-fg-subtle">
              <RefreshCw size={12} className="animate-spin text-accent" />
              <span className="tabular">
                {progress.tasks_done}/{progress.tasks_total}
              </span>
              {progressText(progress) && <span className="truncate font-mono">· {progressText(progress)}</span>}
            </div>
          )}
        </div>

        {/* One filter row: the three states that lead to an action. */}
        <div className="flex flex-wrap items-center gap-2">
          <div className="segmented">
            {filters.map((f) => (
              <button
                key={f.k}
                data-active={filter === f.k}
                className="segmented-item"
                onClick={() => setFilter(f.k)}
              >
                {f.label}
                {f.count != null && <span className="ml-1.5 tabular opacity-60">{f.count}</span>}
              </button>
            ))}
          </div>
          <select className="input w-auto" value={kind} onChange={(e) => setKind(e.target.value as Kind)}>
            {kinds.map((k) => (
              <option key={k.k} value={k.k}>
                {k.label}
                {k.count != null ? ` (${k.count})` : ""}
              </option>
            ))}
          </select>
          <input
            className="input max-w-xs flex-1"
            placeholder={t("radar.search")}
            value={search}
            onChange={(e) => setSearch(e.target.value)}
          />
          {selected.size > 0 && (
            <button
              className="btn-outline"
              onClick={() => (rejectedView ? restore : reject).mutate([...selected])}
              disabled={reject.isPending || restore.isPending}
            >
              {rejectedView ? <RotateCcw size={14} /> : <Ban size={14} />}
              {rejectedView
                ? `${t("radar.restore")} (${selected.size})`
                : t("radar.reject_selected", { count: selected.size })}
            </button>
          )}
          {addressable > 0 && !rejectedView && (
            <button
              className="btn-outline ml-auto"
              onClick={() => promoteBulk.mutate()}
              disabled={promoteBulk.isPending}
              title={t("radar.promote_bulk_hint")}
            >
              <UserPlus size={15} />
              {t("radar.promote_bulk_n", { count: addressable })}
            </button>
          )}
        </div>

        {(stats?.stale_contacts ?? 0) > 0 && (
          <div className="card flex flex-wrap items-center gap-3 border-accent/40 p-4">
            <AlertTriangle size={16} className="shrink-0 text-accent" />
            <div className="min-w-0 flex-1 text-xs">
              <div className="font-semibold">
                {t("radar.stale_n", { count: stats!.stale_contacts })}
              </div>
              <div className="text-2xs text-fg-subtle">{t("radar.stale_hint")}</div>
            </div>
            <button
              className="btn-outline"
              onClick={() => repair.mutate()}
              disabled={repair.isPending}
            >
              <ShieldCheck size={14} />
              {t("radar.repair")}
            </button>
          </div>
        )}

        {/* List. A failed query must say so: this page showed "no curators" for
            weeks while the command was in fact erroring out on every call. */}
        {isError ? (
          <div className="card p-8 text-center">
            <AlertTriangle size={20} className="mx-auto mb-2 text-accent" />
            <div className="text-sm font-bold">{t("radar.load_error")}</div>
            <div className="mx-auto mt-1 max-w-lg break-words font-mono text-2xs text-fg-subtle">
              {String((error as any)?.message ?? error)}
            </div>
            <button className="btn-outline mx-auto mt-4" onClick={() => refetch()}>
              <RefreshCw size={14} />
              {t("common.retry")}
            </button>
          </div>
        ) : rows.length === 0 ? (
          <div className="card">
            {/* With a filter or search active, "run a scan" is the wrong advice. */}
            {filtered ? (
              <EmptyState
                icon={RadarIcon}
                title={t("radar.empty_filtered")}
                hint={
                  filter === "email" || filter === "official"
                    ? t("radar.empty_filtered_email_hint")
                    : t("radar.empty_filtered_hint")
                }
              />
            ) : (
              <EmptyState icon={RadarIcon} title={t("radar.empty")} hint={t("radar.empty_hint")} />
            )}
          </div>
        ) : (
          <div className="card max-h-[calc(100vh-400px)] overflow-auto">
            <table className="tbl">
              <thead>
                <tr>
                  <th className="w-8">
                    <input
                      type="checkbox"
                      aria-label={t("radar.select_all")}
                      checked={allSelected}
                      onChange={() =>
                        setSelected(allSelected ? new Set() : new Set(rows.map((r) => r.id)))
                      }
                    />
                  </th>
                  <th>{t("radar.curator")}</th>
                  <th className="w-32 text-right">{t("radar.followers")}</th>
                  <th className="w-64">{t("radar.email")}</th>
                  <th className="w-56"></th>
                </tr>
              </thead>
              <tbody>
                {rows.map((c) => (
                  <tr key={c.id}>
                    <td>
                      <input
                        type="checkbox"
                        aria-label={c.nom}
                        checked={selected.has(c.id)}
                        onChange={() => toggle(c.id)}
                      />
                    </td>
                    <td>
                      <div className="flex items-center gap-2 font-semibold">
                        <Music2 size={12} className="shrink-0 text-accent-2" />
                        <span className="truncate">{c.nom}</span>
                        {/* Where the promoted contact really stands, not just "in pipeline". */}
                        {c.in_pipeline && (
                          <span className="badge shrink-0 bg-ok/15 text-ok">
                            <Check size={11} />{" "}
                            {c.contact_status && c.contact_status !== "to_contact"
                              ? t(`status.${c.contact_status}`, c.contact_status)
                              : t("radar.in_pipeline")}
                          </span>
                        )}
                        {c.emailed && (
                          <span className="badge shrink-0 bg-accent-2/15 text-accent-2" title={t("radar.emailed")}>
                            <Send size={11} />
                          </span>
                        )}
                      </div>
                      <div className="text-2xs text-fg-subtle">
                        {/* YouTube channels carry their own name as owner. */}
                        {[
                          c.owner_name !== c.nom ? c.owner_name : null,
                          t(`radar.kind_${c.kind}`, c.kind),
                          c.source,
                          c.genre,
                        ]
                          .filter(Boolean)
                          .join(" · ")}
                      </div>
                    </td>
                    <td className="text-right font-bold tabular">
                      {c.followers?.toLocaleString() ?? "·"}
                    </td>
                    <td>
                      {c.best_email ? (
                        <div className="flex items-center gap-1.5">
                          <a
                            href={`mailto:${c.best_email}`}
                            className="inline-flex min-w-0 items-center gap-1.5 font-mono text-2xs text-ok hover:underline"
                            title={c.best_email}
                          >
                            <Mail size={12} className="shrink-0" />
                            <span className="truncate">{c.best_email}</span>
                          </a>
                          {c.official && (
                            <span
                              className="badge shrink-0 bg-accent-2/15 text-accent-2"
                              title={t("radar.official_hint")}
                            >
                              <ShieldCheck size={11} /> {t("radar.official")}
                            </span>
                          )}
                        </div>
                      ) : (
                        <span className="text-2xs text-fg-faint">
                          {c.enriched ? t("radar.no_email") : t("radar.email_pending")}
                        </span>
                      )}
                    </td>
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
                        {!c.in_pipeline && c.best_email && !c.rejected && (
                          <button
                            className="btn-outline py-1.5"
                            onClick={() => promote.mutate(c.id)}
                            disabled={promote.isPending}
                          >
                            <UserPlus size={13} />
                            {t("radar.promote")}
                          </button>
                        )}
                        {c.in_pipeline && (
                          <button
                            className="btn-ghost px-2 py-1.5"
                            aria-label={t("radar.unpromote")}
                            title={t("radar.unpromote")}
                            onClick={() => confirm(t("radar.unpromote_confirm")) && unpromote.mutate(c.id)}
                            disabled={unpromote.isPending}
                          >
                            <UserMinus size={14} />
                          </button>
                        )}
                        {!c.rejected && (
                          <button
                            className="btn-ghost px-2 py-1.5"
                            aria-label={t("radar.retry_one")}
                            title={t("radar.retry_one_hint")}
                            onClick={() => retryOne(c.id)}
                            disabled={running}
                          >
                            <Search size={14} />
                          </button>
                        )}
                        {c.rejected ? (
                          <button
                            className="btn-ghost px-2 py-1.5"
                            aria-label={t("radar.restore")}
                            title={t("radar.restore")}
                            onClick={() => restore.mutate([c.id])}
                            disabled={restore.isPending}
                          >
                            <RotateCcw size={14} />
                          </button>
                        ) : (
                          <button
                            className="btn-ghost px-2 py-1.5 text-fg-faint hover:text-accent"
                            aria-label={t("radar.reject")}
                            title={t("radar.reject_hint")}
                            onClick={() => reject.mutate([c.id])}
                            disabled={reject.isPending}
                          >
                            <Ban size={14} />
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
        <div className="flex items-center gap-3 text-2xs text-fg-subtle">
          <span>
            {capped && total != null && total > rows.length
              ? t("radar.count_of", { shown: rows.length, count: total })
              : t("radar.count", { count: rows.length })}
          </span>
          {capped && (
            <button className="btn-ghost px-2 py-1 text-2xs" onClick={() => setLimit((l) => l + PAGE)}>
              {t("radar.load_more")}
            </button>
          )}
        </div>
      </div>
    </div>
  );
}
