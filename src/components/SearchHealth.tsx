import { useQuery } from "@tanstack/react-query";
import { invoke } from "@tauri-apps/api/core";
import { useTranslation } from "react-i18next";
import { Link } from "react-router-dom";
import { AlertTriangle } from "lucide-react";

export interface SerperHealth {
  key_set: boolean;
  ok: number;
  failed: number;
  last_error?: string | null;
  last_error_at?: string | null;
}

export const serperStatus = () => invoke<SerperHealth>("serper_status");

/// Collectives are found through web search: without a working Serper key
/// every enrichment run "finds nothing". Says so instead of failing silently.
export function SearchHealthBanner() {
  const { t } = useTranslation();
  const { data } = useQuery({ queryKey: ["serper_status"], queryFn: serperStatus, refetchInterval: 60_000 });
  if (!data || (data.key_set && !data.last_error)) return null;
  const text = !data.key_set
    ? t("search_health.no_key")
    : t("search_health.failing", { reason: data.last_error, failed: data.failed });
  return (
    <div className="flex items-start gap-2.5 rounded-xl border border-warn/40 bg-warn/10 px-3.5 py-2.5 text-sm">
      <AlertTriangle size={16} className="mt-0.5 shrink-0 text-warn" />
      <div className="min-w-0 flex-1">{text}</div>
      <Link to="/settings?tab=integrations" className="shrink-0 text-xs font-medium underline underline-offset-2">
        {t("search_health.settings")}
      </Link>
    </div>
  );
}
