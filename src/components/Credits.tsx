import { useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { invoke } from "@tauri-apps/api/core";
import { openUrl } from "@tauri-apps/plugin-opener";
import { useTranslation } from "react-i18next";
import { Link } from "react-router-dom";
import clsx from "clsx";
import { AlertTriangle, ExternalLink, RefreshCw, Wallet } from "lucide-react";
import { useSubmit, useToast } from "./ui";

export interface Credit {
  service: "claude" | "serper" | "millionverifier" | "pushover";
  key_set: boolean;
  remaining?: number | null;
  unit: "usd" | "searches" | "checks" | "messages";
  status: "ok" | "low" | "empty" | "error" | "unset" | "unknown";
  detail?: string | null;
  per_day?: number | null;
  days_left?: number | null;
  spent_month?: number | null;
  manage_url: string;
}

const fetchCredits = () => invoke<Credit[]>("credits_overview");
const QK = ["credits_overview"];

const tone: Record<Credit["status"], string> = {
  ok: "bg-ok/15 text-ok",
  low: "bg-warn/15 text-warn",
  empty: "bg-danger/15 text-danger",
  error: "bg-danger/15 text-danger",
  unset: "bg-fg-faint/10 text-fg-subtle",
  unknown: "bg-fg-faint/10 text-fg-subtle",
};

function amount(c: Credit, v: number) {
  return c.unit === "usd" ? `${v.toFixed(2)} $` : Math.round(v).toLocaleString("fr-FR");
}

/// Admin, Crédits: what is left on every paid service the autopilot uses.
export function CreditsPanel() {
  const { t } = useTranslation();
  const qc = useQueryClient();
  const { data, isFetching, refetch } = useQuery({ queryKey: QK, queryFn: fetchCredits, staleTime: 5 * 60_000 });
  return (
    <div className="space-y-4">
      <div className="flex items-center justify-between">
        <p className="text-sm text-fg-subtle">{t("credits.help")}</p>
        <button className="btn btn-ghost" onClick={() => refetch()} disabled={isFetching}>
          <RefreshCw size={14} className={clsx(isFetching && "animate-spin")} /> {t("credits.refresh")}
        </button>
      </div>
      <div className="grid gap-4 sm:grid-cols-2">
        {(data ?? []).map((c) => (
          <CreditCard key={c.service} c={c} onChange={(list) => qc.setQueryData(QK, list)} />
        ))}
      </div>
    </div>
  );
}

function CreditCard({ c, onChange }: { c: Credit; onChange: (l: Credit[]) => void }) {
  const { t } = useTranslation();
  const toast = useToast();
  const [val, setVal] = useState("");
  const [save, busy] = useSubmit(async () => {
    const usd = parseFloat(val.replace(",", "."));
    const list = await invoke<Credit[]>("credits_set_claude_balance", { usd });
    onChange(list);
    setVal("");
    toast(t("settings.saved"), "ok");
  });
  return (
    <div className="card p-5">
      <div className="mb-3 flex items-center justify-between gap-2">
        <div className="flex items-center gap-2 text-sm font-semibold">
          <Wallet size={15} /> {t(`credits.name.${c.service}`)}
        </div>
        <span className={clsx("rounded-full px-2 py-0.5 text-2xs font-medium", tone[c.status])}>
          {t(`credits.status.${c.status}`)}
        </span>
      </div>
      <div className="text-2xl font-semibold tabular-nums">
        {c.remaining != null ? amount(c, c.remaining) : "–"}
        <span className="ml-1.5 text-xs font-normal text-fg-subtle">{t(`credits.unit.${c.unit}`)}</span>
      </div>
      <div className="mt-1 space-y-0.5 text-xs text-fg-subtle">
        {c.per_day != null && (
          <div>
            {t("credits.per_day", { value: amount(c, c.per_day) })}
            {c.days_left != null && <> · {t("credits.days_left", { count: Math.floor(c.days_left) })}</>}
          </div>
        )}
        {c.spent_month != null && <div>{t("credits.spent_month", { value: amount(c, c.spent_month) })}</div>}
        {!c.key_set && <div>{t("credits.no_key")}</div>}
        {c.detail && <div>{c.detail}</div>}
        {c.service === "claude" && c.key_set && c.remaining == null && <div>{t("credits.claude_help")}</div>}
      </div>
      {c.service === "claude" && c.key_set && (
        <div className="mt-3 flex items-center gap-2">
          <input
            className="input h-8 flex-1 text-sm"
            inputMode="decimal"
            placeholder={t("credits.claude_placeholder")}
            value={val}
            onChange={(e) => setVal(e.target.value)}
          />
          <button className="btn btn-primary" onClick={save} disabled={busy || !val.trim()}>
            {t("credits.claude_save")}
          </button>
        </div>
      )}
      <button
        className="mt-3 inline-flex items-center gap-1 text-xs font-medium underline underline-offset-2"
        onClick={() => openUrl(c.manage_url).catch(() => {})}
      >
        {t("credits.top_up")} <ExternalLink size={12} />
      </button>
    </div>
  );
}

/// On Aujourd'hui: shown only when a service runs low or is empty.
export function CreditsBanner() {
  const { t } = useTranslation();
  const { data } = useQuery({ queryKey: QK, queryFn: fetchCredits, staleTime: 30 * 60_000, refetchInterval: 60 * 60_000 });
  const low = (data ?? []).filter((c) => c.key_set && (c.status === "low" || c.status === "empty"));
  if (!low.length) return null;
  return (
    <div className="flex items-start gap-2.5 rounded-xl border border-warn/40 bg-warn/10 px-3.5 py-2.5 text-sm">
      <AlertTriangle size={16} className="mt-0.5 shrink-0 text-warn" />
      <div className="min-w-0 flex-1">
        {t("credits.banner", { list: low.map((c) => t(`credits.name.${c.service}`)).join(", ") })}
      </div>
      <Link to="/admin?tab=credits" className="shrink-0 text-xs font-medium underline underline-offset-2">
        {t("credits.see")}
      </Link>
    </div>
  );
}
