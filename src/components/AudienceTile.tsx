import { useEffect } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { listen } from "@tauri-apps/api/event";
import { useTranslation } from "react-i18next";
import { Link } from "react-router-dom";
import { pulseKpis } from "../lib/api";
import { euro } from "../lib/format";

/// Home tile for the Audience module (Pulse): latest Spotify popularity and
/// followers of the main artist, best tracked track and Meta spend over 30 days.
/// Self-contained so the Dashboard only has to drop it in.
export function AudienceTile() {
  const { t, i18n } = useTranslation();
  const qc = useQueryClient();
  // The launch snapshot lands a few seconds after startup, usually on Home.
  useEffect(() => {
    const un = listen("pulse:snapshot-done", () => qc.invalidateQueries({ queryKey: ["pulse_kpis"] }));
    return () => {
      un.then((f) => f());
    };
  }, [qc]);
  // No artist given: the backend picks the first artist with a Spotify ID.
  const { data: k } = useQuery({ queryKey: ["pulse_kpis", "home"], queryFn: () => pulseKpis() });
  const hasData =
    !!k && (k.artist_popularity != null || k.best_track_popularity != null || k.spend_30d > 0);
  const num = (n?: number | null) =>
    n == null ? "·" : new Intl.NumberFormat(i18n.language).format(n);
  const delta = k?.artist_delta7;

  return (
    <div className="mt-8 border-t border-border pt-6">
      <div className="mb-4 flex items-baseline justify-between">
        <div className="kicker">{t("pulse.tile_title")}</div>
        <Link to="/audience" className="kicker hover:text-accent">
          {t("pulse.tile_open")} →
        </Link>
      </div>
      {!hasData ? (
        <p className="text-xs leading-relaxed text-fg-subtle">
          {t("pulse.tile_empty")}{" "}
          <Link to="/settings" className="text-accent hover:underline">
            {t("nav.settings")}
          </Link>
        </p>
      ) : (
        <div className="border border-border p-5">
          <div className="grid grid-cols-2 gap-4">
            <div>
              <div className="eyebrow">{t("pulse.tile_pop")}</div>
              <div className="mt-1 flex items-baseline gap-2">
                <span className="font-condensed text-[32px] leading-none tabular text-accent-2">
                  {k?.artist_popularity ?? "·"}
                </span>
                {delta != null && (
                  <span
                    className={`text-2xs font-bold tabular ${
                      delta > 0 ? "text-ok" : delta < 0 ? "text-accent" : "text-fg-faint"
                    }`}
                  >
                    {delta > 0 ? "+" : ""}
                    {delta} / {t("pulse.days_short", { count: 7 })}
                  </span>
                )}
              </div>
            </div>
            <div>
              <div className="eyebrow">{t("pulse.tile_followers")}</div>
              <div className="mt-1 font-condensed text-[32px] leading-none tabular">
                {num(k?.artist_followers)}
              </div>
            </div>
          </div>
          <div className="mt-4 space-y-1 eyebrow text-[10.5px]">
            {k?.best_track_name && (
              <div className="truncate">
                {t("pulse.tile_best")}:{" "}
                <span className="font-bold text-fg">
                  {k.best_track_name}
                  {k.best_track_popularity != null ? ` (${k.best_track_popularity})` : ""}
                </span>
              </div>
            )}
            <div>
              {t("pulse.tile_spend")}: <span className="font-bold text-fg">{euro(k?.spend_30d ?? 0)}</span>
            </div>
            {k?.last_snapshot && (
              <div>
                {t("pulse.last_snapshot")}: <span className="font-bold text-fg">{k.last_snapshot}</span>
              </div>
            )}
          </div>
        </div>
      )}
    </div>
  );
}
