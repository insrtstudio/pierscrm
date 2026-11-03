import { useEffect, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { Bot } from "lucide-react";
import { getSetting, setSetting } from "../lib/api";
import { aiCostMonth, pressKits } from "../lib/residency";
import { Field, errorMessage, useToast } from "./ui";

/// Models offered for drafts and the daily note. Haiku by default (cheap and
/// good enough); reply classification always runs on Haiku.
const MODELS = ["claude-haiku-5-5", "claude-sonnet-5-5"];
const KEYS = ["ai_model", "insrt_api_base"] as const;

/// Settings › Integrations: the residency agent (Claude API key, model,
/// insrt.studio API) and this month's AI cost. Secrets go to the Keychain
/// (the backend masks them; an untouched mask keeps the stored value).
export function ResidencySettingsCard() {
  const { t } = useTranslation();
  const toast = useToast();
  const qc = useQueryClient();
  const { data: raw } = useQuery({
    queryKey: ["settings", "residency"],
    queryFn: async () => {
      const out: Record<string, string> = {};
      for (const k of KEYS) out[k] = (await getSetting(k)) ?? "";
      return out;
    },
  });
  const { data: cost } = useQuery({ queryKey: ["ai_cost"], queryFn: () => aiCostMonth(null) });
  const [cfg, setCfg] = useState<Record<string, string>>({});
  useEffect(() => {
    if (raw) setCfg(raw);
  }, [raw]);
  const inp = (k: string, type = "text") => ({
    className: "input",
    type,
    autoComplete: "off",
    value: cfg[k] ?? "",
    onChange: (e: React.ChangeEvent<HTMLInputElement>) => setCfg((c) => ({ ...c, [k]: e.target.value })),
  });
  // Saved as soon as it changes (model) or on leaving the field (URL).
  const persist = async (k: string, v: string) => {
    if ((raw?.[k] ?? "") === v.trim()) return;
    await setSetting(k, v.trim());
    qc.invalidateQueries({ queryKey: ["settings", "residency"] });
    qc.invalidateQueries({ queryKey: ["residency"] });
    toast(t("settings.saved"), "ok");
  };
  const [testing, setTesting] = useState(false);
  const test = async () => {
    setTesting(true);
    try {
      const kits = await pressKits();
      toast(t("residency.insrt_ok", { count: kits.length }), "ok");
    } catch (e) {
      toast(errorMessage(e), "error");
    } finally {
      setTesting(false);
    }
  };
  return (
    <div className="card p-6">
      <div className="mb-1 flex items-center gap-2 text-sm font-semibold">
        <Bot size={15} /> {t("residency.settings_section")}
      </div>
      <p className="mb-4 text-xs text-fg-subtle">{t("residency.settings_help")}</p>
      <div className="grid grid-cols-2 gap-x-4 gap-y-3">
        <Field label={t("residency.ai_model")}>
          <select className="input" value={cfg.ai_model || MODELS[0]} onChange={(e) => {
              setCfg((c) => ({ ...c, ai_model: e.target.value }));
              persist("ai_model", e.target.value);
            }}>
            {MODELS.map((m) => (
              <option key={m} value={m}>
                {t(`residency.model_${m.replace(/-/g, "_")}`, m)}
              </option>
            ))}
          </select>
        </Field>
        <Field label={t("residency.insrt_base")}>
          <input {...inp("insrt_api_base")} placeholder="https://insrt.studio" onBlur={(e) => persist("insrt_api_base", e.target.value)} />
        </Field>
      </div>
      <p className="mt-3 text-2xs text-fg-faint">{t("settings.keys_in_services")}</p>
      {cost && (
        <p className="mt-1 text-2xs text-fg-subtle tabular">
          {t("residency.ai_cost", { eur: cost.cost_eur.toFixed(2), calls: cost.calls })}
        </p>
      )}
      <div className="mt-4 flex justify-end gap-2">
        <button className="btn-outline" onClick={test} disabled={testing}>
          {t("residency.insrt_test")}
        </button>
      </div>
    </div>
  );
}
