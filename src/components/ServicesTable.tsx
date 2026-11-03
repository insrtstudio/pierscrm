import { useEffect, useRef, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { invoke } from "@tauri-apps/api/core";
import { openUrl } from "@tauri-apps/plugin-opener";
import { useTranslation } from "react-i18next";
import clsx from "clsx";
import { Check, ExternalLink, KeyRound, Loader2 } from "lucide-react";
import { getSetting, setSetting } from "../lib/api";
import type { Credit } from "./Credits";
import { useToast, errorMessage } from "./ui";

interface KeyField {
  key: string;
  secret?: boolean;
  placeholder?: string;
  /** Label key under services.field.* when the row has several fields. */
  label?: string;
}

interface ServiceRow {
  id: string;
  fields: KeyField[];
  url: string;
  optional?: boolean;
  /** Service id in credits_overview, for the live balance. */
  credit?: Credit["service"];
}

/// Every API key of the app, one row per service. Fields save themselves on
/// blur or Enter: no Save button to hunt for.
const ROWS: ServiceRow[] = [
  { id: "claude", credit: "claude", url: "https://console.anthropic.com/settings/keys", fields: [{ key: "anthropic_api_key", secret: true, placeholder: "sk-ant-…" }] },
  { id: "insrt", url: "https://insrt.studio/admin", fields: [{ key: "insrt_api_token", secret: true }] },
  { id: "serper", credit: "serper", url: "https://serper.dev/api-key", fields: [{ key: "serper_api_key", secret: true }] },
  { id: "millionverifier", credit: "millionverifier", url: "https://app.millionverifier.com/api", fields: [{ key: "email_verifier_key", secret: true }] },
  {
    id: "pushover",
    credit: "pushover",
    url: "https://pushover.net/",
    fields: [
      { key: "pushover_user", secret: true, label: "user_key" },
      { key: "pushover_token", secret: true, label: "app_token" },
    ],
  },
  { id: "youtube", optional: true, url: "https://console.cloud.google.com/apis/credentials", fields: [{ key: "youtube_api_key", secret: true }] },
  {
    id: "spotify",
    optional: true,
    url: "https://developer.spotify.com/dashboard",
    fields: [
      { key: "spotify_client_id", label: "client_id" },
      { key: "spotify_client_secret", secret: true, label: "client_secret" },
    ],
  },
  {
    id: "meta",
    optional: true,
    url: "https://business.facebook.com/settings/system-users",
    fields: [
      { key: "meta_access_token", secret: true, label: "token" },
      { key: "meta_ad_account_id", label: "ad_account", placeholder: "act_1234567890" },
      { key: "meta_result_action_type", label: "action", placeholder: "link_click" },
    ],
  },
];

const ALL_KEYS = ROWS.flatMap((r) => r.fields.map((f) => f.key));

const tone: Record<string, string> = {
  ok: "bg-ok/15 text-ok",
  low: "bg-warn/15 text-warn",
  empty: "bg-danger/15 text-danger",
  error: "bg-danger/15 text-danger",
  missing: "bg-danger/10 text-danger",
  optional: "bg-fg-faint/10 text-fg-subtle",
  unknown: "bg-fg-faint/10 text-fg-subtle",
};

export function ServicesTable() {
  const { t } = useTranslation();
  const { data: values } = useQuery({
    queryKey: ["settings", "services"],
    queryFn: async () => {
      const out: Record<string, string> = {};
      for (const k of ALL_KEYS) out[k] = (await getSetting(k)) ?? "";
      return out;
    },
  });
  const { data: credits } = useQuery({
    queryKey: ["credits_overview"],
    queryFn: () => invoke<Credit[]>("credits_overview"),
    staleTime: 5 * 60_000,
  });
  if (!values) return null;
  return (
    <div className="card divide-y divide-border">
      {ROWS.map((row) => {
        const set = row.fields.filter((f) => f.secret).every((f) => (values[f.key] ?? "").trim() !== "");
        const credit = credits?.find((c) => c.service === row.credit);
        let status = set ? "ok" : row.optional ? "optional" : "missing";
        if (set && credit && credit.status !== "unset") status = credit.status;
        const balance =
          credit?.remaining != null
            ? credit.unit === "usd"
              ? `${credit.remaining.toFixed(2)} $`
              : Math.round(credit.remaining).toLocaleString("fr-FR")
            : null;
        return (
          <div key={row.id} className="grid items-center gap-x-5 gap-y-2 px-5 py-3.5 lg:grid-cols-[minmax(0,15rem)_minmax(0,1fr)_auto]">
            <div className="min-w-0">
              <div className="flex items-center gap-2 text-sm font-semibold">
                <KeyRound size={14} className="text-fg-subtle" /> {t(`services.${row.id}.name`)}
              </div>
              <div className="text-2xs text-fg-subtle">{t(`services.${row.id}.purpose`)}</div>
            </div>
            <div className={clsx("grid gap-2", row.fields.length > 1 && "sm:grid-cols-2 xl:grid-cols-3")}>
              {row.fields.map((f) => (
                <AutoField key={f.key} field={f} initial={values[f.key] ?? ""} label={f.label ? t(`services.field.${f.label}`) : undefined} />
              ))}
            </div>
            <div className="flex items-center gap-3 lg:justify-end">
              <span className={clsx("whitespace-nowrap rounded-full px-2 py-0.5 text-2xs font-medium", tone[status] ?? tone.unknown)}>
                {t(`services.status.${status}`)}
                {balance && status !== "missing" ? ` · ${balance}` : ""}
              </span>
              <button
                className="inline-flex items-center gap-1 whitespace-nowrap text-2xs font-medium text-fg-subtle underline underline-offset-2 hover:text-fg"
                onClick={() => openUrl(row.url).catch(() => {})}
              >
                {t(set ? "services.manage" : "services.get_key")} <ExternalLink size={11} />
              </button>
            </div>
          </div>
        );
      })}
    </div>
  );
}

/// One key: saved on blur or Enter when it changed. A secret comes back
/// masked; focusing selects it so pasting replaces it whole.
function AutoField({ field, initial, label }: { field: KeyField; initial: string; label?: string }) {
  const { t } = useTranslation();
  const qc = useQueryClient();
  const toast = useToast();
  const [val, setVal] = useState(initial);
  const [state, setState] = useState<"idle" | "saving" | "saved">("idle");
  const saved = useRef(initial);
  useEffect(() => {
    setVal(initial);
    saved.current = initial;
  }, [initial]);
  const commit = async () => {
    const v = val.trim();
    if (v === saved.current.trim()) return;
    setState("saving");
    try {
      await setSetting(field.key, v);
      saved.current = v;
      setState("saved");
      setTimeout(() => setState("idle"), 1800);
      qc.invalidateQueries({ queryKey: ["settings", "services"] });
      qc.invalidateQueries({ queryKey: ["setting", field.key] });
      qc.invalidateQueries({ queryKey: ["credits_overview"] });
      qc.invalidateQueries({ queryKey: ["serper_status"] });
      qc.invalidateQueries({ queryKey: ["email_check_stats"] });
      qc.invalidateQueries({ queryKey: ["push"] });
      qc.invalidateQueries({ queryKey: ["residency"] });
    } catch (e) {
      setState("idle");
      toast(errorMessage(e), "error");
    }
  };
  return (
    <label className="relative block min-w-0">
      {label && <span className="label">{label}</span>}
      <input
        className="input h-9 pr-8 text-sm"
        type={field.secret ? "password" : "text"}
        autoComplete="off"
        spellCheck={false}
        value={val}
        placeholder={field.placeholder ?? t("services.paste")}
        onFocus={(e) => field.secret && e.currentTarget.select()}
        onChange={(e) => setVal(e.target.value)}
        onBlur={commit}
        onKeyDown={(e) => e.key === "Enter" && (e.currentTarget as HTMLInputElement).blur()}
      />
      <span className="pointer-events-none absolute bottom-2.5 right-2.5 text-fg-subtle">
        {state === "saving" && <Loader2 size={14} className="animate-spin" />}
        {state === "saved" && <Check size={14} className="text-ok" />}
      </span>
    </label>
  );
}
