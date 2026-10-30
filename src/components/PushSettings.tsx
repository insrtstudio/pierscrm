import { useEffect, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { Smartphone } from "lucide-react";
import { pushGet, pushSave, pushTest, type PushConfig } from "../lib/push";
import { formatDateTime } from "../lib/format";
import { Field, errorMessage, useSubmit, useToast } from "./ui";

/// Event switches, in display order (keys of PushConfig).
const EVENTS = ["replies", "out_of_office", "epk", "opens", "problems", "summary"] as const;

/// Settings › Integrations: "Notifications iPhone" (Pushover). The two keys go
/// to the Keychain (masked like the other secrets); per-event switches, quiet
/// hours, a test button and the last error.
export function PushSettingsCard() {
  const { t } = useTranslation();
  const toast = useToast();
  const qc = useQueryClient();
  const { data } = useQuery({ queryKey: ["push"], queryFn: pushGet });
  const [cfg, setCfg] = useState<PushConfig | null>(null);
  const [user, setUser] = useState("");
  const [token, setToken] = useState("");
  useEffect(() => {
    if (!data) return;
    setCfg(data.config);
    setUser(data.user);
    setToken(data.token);
  }, [data]);
  const set = <K extends keyof PushConfig>(k: K, v: PushConfig[K]) => setCfg((c) => (c ? { ...c, [k]: v } : c));
  const [save, saving] = useSubmit(async () => {
    if (!cfg) return;
    const v = await pushSave(cfg, user.trim(), token.trim());
    qc.setQueryData(["push"], v);
    toast(t("settings.saved"), "ok");
  });
  const [testing, setTesting] = useState(false);
  const test = async () => {
    setTesting(true);
    try {
      // Test what is on screen: save first (an untouched mask keeps the key).
      if (cfg) await pushSave(cfg, user.trim(), token.trim());
      const v = await pushTest();
      qc.setQueryData(["push"], v);
      toast(t("push.test_ok"), "ok");
    } catch (e) {
      toast(errorMessage(e), "error");
      qc.invalidateQueries({ queryKey: ["push"] });
    } finally {
      setTesting(false);
    }
  };
  if (!cfg || !data) return null;
  const check = (k: keyof PushConfig, label: string, disabled = false) => (
    <label className="flex items-center gap-2 text-sm">
      <input type="checkbox" checked={Boolean(cfg[k])} disabled={disabled} onChange={(e) => set(k, e.target.checked)} />
      {label}
    </label>
  );
  return (
    <div className="card p-6">
      <div className="mb-1 flex items-center gap-2 text-sm font-semibold">
        <Smartphone size={15} /> {t("push.section")}
      </div>
      <p className="mb-4 text-xs text-fg-subtle">{t("push.help")}</p>
      <div className="grid grid-cols-2 gap-x-4 gap-y-3">
        <Field label={t("push.user_key")}>
          <input className="input" type="password" autoComplete="off" value={user} onChange={(e) => setUser(e.target.value)} />
        </Field>
        <Field label={t("push.app_token")}>
          <input className="input" type="password" autoComplete="off" value={token} onChange={(e) => setToken(e.target.value)} />
        </Field>
      </div>
      <p className="mt-2 text-2xs text-fg-faint">{t("push.keys_hint")}</p>

      <div className="mt-4">{check("enabled", t("push.enabled"))}</div>
      <div className="mt-3 grid grid-cols-2 gap-x-4 gap-y-2">
        {EVENTS.map((k) => (
          <div key={k}>
            {check(k, t(`push.ev_${k}`), !cfg.enabled)}
            <p className="ml-6 text-2xs text-fg-faint">{t(`push.ev_${k}_hint`)}</p>
            {k === "opens" && cfg.opens && (
              <div className="ml-6 mt-1">{check("opens_in_summary", t("push.opens_in_summary"), !cfg.enabled)}</div>
            )}
          </div>
        ))}
      </div>

      <div className="mt-4 flex flex-wrap items-center gap-3 text-sm">
        {check("quiet_enabled", t("push.quiet"), !cfg.enabled)}
        <input
          type="time"
          className="input w-28"
          value={cfg.quiet_start}
          disabled={!cfg.enabled || !cfg.quiet_enabled}
          onChange={(e) => set("quiet_start", e.target.value)}
        />
        <span className="text-fg-subtle">{t("push.quiet_to")}</span>
        <input
          type="time"
          className="input w-28"
          value={cfg.quiet_end}
          disabled={!cfg.enabled || !cfg.quiet_enabled}
          onChange={(e) => set("quiet_end", e.target.value)}
        />
      </div>
      <p className="mt-1 text-2xs text-fg-faint">{t("push.quiet_hint")}</p>

      <div className="mt-3 space-y-1 text-2xs text-fg-subtle tabular">
        {data.last_sent_at && (
          <p>{t("push.last_sent", { at: formatDateTime(data.last_sent_at), count: data.sent_today })}</p>
        )}
        {data.held > 0 && <p>{t("push.held", { count: data.held })}</p>}
        {data.last_error && (
          <p className="text-danger">
            {t(data.blocked ? "push.last_error_blocked" : "push.last_error", {
              at: formatDateTime(data.last_error_at),
              error: data.last_error,
            })}
          </p>
        )}
      </div>

      <div className="mt-4 flex justify-end gap-2">
        <button className="btn-outline" onClick={test} disabled={testing || saving}>
          {t("push.test")}
        </button>
        <button className="btn-primary" onClick={save} disabled={saving}>
          {t("common.save")}
        </button>
      </div>
    </div>
  );
}
