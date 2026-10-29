import { useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { openUrl } from "@tauri-apps/plugin-opener";
import { Download, ExternalLink, Eye, Flame, Headphones, MessageSquare, RefreshCw } from "lucide-react";
import clsx from "clsx";
import { contactActivity, listenTime, syncInsrtActivity, type ActivityItem } from "../lib/insrt";
import { formatDateTime } from "../lib/format";
import { errorMessage, useToast } from "./ui";

/// What a contact did on their private insrt EPK link: opens, tracks and
/// videos listened (how long, how far), feedback, EPK downloads. Read from the
/// local copy the residency engine syncs every 30 minutes; "Actualiser" asks
/// insrt now. `compact` is the residency board card version (no timeline).
export function InsrtActivityPanel({ contactId, compact = false }: { contactId: number; compact?: boolean }) {
  const { t } = useTranslation();
  const toast = useToast();
  const qc = useQueryClient();
  const [syncing, setSyncing] = useState(false);
  const [showAll, setShowAll] = useState(false);
  const { data: a } = useQuery({
    queryKey: ["insrt_activity", contactId],
    queryFn: () => contactActivity(contactId),
  });
  const refresh = async () => {
    setSyncing(true);
    try {
      const n = await syncInsrtActivity();
      await qc.invalidateQueries({ queryKey: ["insrt_activity"] });
      qc.invalidateQueries({ queryKey: ["residency_board"] });
      toast(t("insrt.synced", { count: n }), "ok");
    } catch (e) {
      toast(errorMessage(e), "error");
    } finally {
      setSyncing(false);
    }
  };
  if (!a) return null;
  const empty = a.views === 0 && a.recent.length === 0;
  const recent = showAll ? a.recent : a.recent.slice(0, 8);

  return (
    <div className={clsx(compact ? "mt-2 border-t border-border pt-2" : "mt-5 border-t border-border pt-4")}>
      {!compact && (
        <div className="mb-2 flex items-center gap-2">
          <div className="eyebrow text-fg">{t("insrt.activity_title")}</div>
          {a.hot && (
            <span className="badge bg-accent-soft text-accent-ink">
              <Flame size={10} /> {t("insrt.hot")}
            </span>
          )}
          <div className="ml-auto flex items-center gap-2">
            {a.link_url && (
              <button
                className="btn-ghost px-2 py-1 text-2xs"
                onClick={() => openUrl(a.link_url!).catch(() => {})}
                title={a.link_url}
              >
                <ExternalLink size={12} /> {t("insrt.open_link")}
              </button>
            )}
            <button className="btn-outline px-2 py-1 text-2xs" onClick={refresh} disabled={syncing}>
              <RefreshCw size={12} className={clsx(syncing && "animate-spin")} /> {t("insrt.refresh")}
            </button>
          </div>
        </div>
      )}

      {empty ? (
        <p className="text-xs text-fg-subtle">{a.link_url ? t("insrt.not_opened") : t("insrt.no_link")}</p>
      ) : (
        <>
          <div className="flex flex-wrap gap-x-4 gap-y-1 text-xs">
            <span className="inline-flex items-center gap-1">
              <Eye size={12} className="text-fg-subtle" />
              {t("insrt.views", { count: a.views })}
              {a.last_view_at && (
                <span className="text-fg-subtle">· {t("insrt.last_view", { when: formatDateTime(a.last_view_at) })}</span>
              )}
            </span>
            {a.listen_seconds > 0 && (
              <span className="inline-flex items-center gap-1">
                <Headphones size={12} className="text-fg-subtle" />
                {t("insrt.listened", { time: listenTime(a.listen_seconds) })}
              </span>
            )}
            {a.feedback.length > 0 && (
              <span className="inline-flex items-center gap-1">
                <MessageSquare size={12} className="text-fg-subtle" />
                {t("insrt.feedback_count", { count: a.feedback.length })}
              </span>
            )}
            {a.downloads > 0 && (
              <span className="inline-flex items-center gap-1">
                <Download size={12} className="text-fg-subtle" />
                {t("insrt.downloads", { count: a.downloads })}
              </span>
            )}
          </div>

          {a.tracks.length > 0 && (
            <table className="mt-3 w-full text-xs">
              <thead>
                <tr className="text-left text-2xs uppercase tracking-wide text-fg-subtle">
                  <th className="pb-1 font-normal">{t("insrt.track")}</th>
                  <th className="pb-1 text-right font-normal">{t("insrt.plays")}</th>
                  <th className="pb-1 text-right font-normal">{t("insrt.time")}</th>
                  <th className="pb-1 text-right font-normal">{t("insrt.completion")}</th>
                </tr>
              </thead>
              <tbody>
                {a.tracks.map((tr) => (
                  <tr key={tr.title} className="border-t border-border">
                    <td className="max-w-0 truncate py-1 pr-2">{tr.title === "?" ? t("insrt.unknown_track") : tr.title}</td>
                    <td className="tabular py-1 text-right">{tr.plays}</td>
                    <td className="tabular py-1 text-right">{listenTime(tr.seconds)}</td>
                    <td className="tabular py-1 text-right">
                      {tr.completion != null ? `${Math.round(tr.completion * 100)} %` : "·"}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          )}

          {a.feedback.length > 0 && (
            <ul className="mt-3 space-y-1.5 text-xs">
              {a.feedback.slice(0, compact ? 2 : 20).map((f, i) => (
                <li key={i} className="border-l-2 border-accent pl-2">
                  {f.comment && <span className="italic">« {f.comment} »</span>}
                  <span className="block text-2xs text-fg-subtle">
                    {[f.track_title, formatDateTime(f.at)].filter(Boolean).join(" · ")}
                  </span>
                </li>
              ))}
            </ul>
          )}

          {!compact && recent.length > 0 && (
            <div className="mt-3">
              <div className="mb-1 text-2xs uppercase tracking-wide text-fg-subtle">{t("insrt.timeline")}</div>
              <ul className="space-y-0.5 text-xs">
                {recent.map((e, i) => (
                  <li key={i} className="flex items-baseline gap-2">
                    <span className="shrink-0 tabular text-fg-subtle">{formatDateTime(e.at)}</span>
                    <span className="truncate">{describe(e, t)}</span>
                  </li>
                ))}
              </ul>
              {a.recent.length > 8 && (
                <button className="mt-1 text-2xs text-fg-subtle underline" onClick={() => setShowAll((v) => !v)}>
                  {showAll ? t("insrt.less") : t("insrt.more", { count: a.recent.length })}
                </button>
              )}
            </div>
          )}
        </>
      )}
      {!compact && (
        <p className="mt-2 text-2xs text-fg-faint">
          {a.synced_at ? t("insrt.synced_at", { when: formatDateTime(a.synced_at) }) : t("insrt.never_synced")}
        </p>
      )}
    </div>
  );
}

function describe(e: ActivityItem, t: (k: string, o?: Record<string, unknown>) => string): string {
  switch (e.kind) {
    case "view":
      return t("insrt.ev_view");
    case "download":
      return t("insrt.ev_download");
    case "listen":
      return t("insrt.ev_listen", {
        track: e.track_title || t("insrt.unknown_track"),
        time: listenTime(e.seconds ?? 0),
        pct: e.completion != null ? ` (${Math.round(e.completion * 100)} %)` : "",
      });
    case "feedback":
      return t("insrt.ev_feedback", { track: e.track_title || t("insrt.unknown_track"), comment: e.comment ?? "" });
    default:
      return e.kind;
  }
}
