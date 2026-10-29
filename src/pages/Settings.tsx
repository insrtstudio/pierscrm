import { useEffect, useState, useSyncExternalStore, type FocusEvent } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { Globe, Moon, Sun, Mail, Plug, Eye, Download, RefreshCw, Info, PenLine, Search, Activity } from "lucide-react";
import { getVersion } from "@tauri-apps/api/app";
import i18n from "../i18n";
import type { Update } from "@tauri-apps/plugin-updater";
import { checkForUpdate, installAndRelaunch, isInstalling, subscribeInstalling } from "../lib/updater";
import { useAppIntent } from "../lib/hooks";
import { applyTheme, getTheme, type Theme } from "../lib/theme";
import {
  getSetting,
  getSmtpConfig,
  saveSmtpConfig,
  setSetting,
  testSmtp,
} from "../lib/api";
import type { SmtpConfig } from "../lib/types";
import { PageHeader } from "../components/Layout";
import clsx from "clsx";
import { Field, errorMessage, useSubmit, useToast } from "../components/ui";

type SignatureFields = {
  name: string;
  role: string;
  label: string;
  phone: string;
  booking_email: string;
  website: string;
  instagram: string;
  soundcloud: string;
};
const EMPTY_SIG: SignatureFields = {
  name: "",
  role: "",
  label: "",
  phone: "",
  booking_email: "",
  website: "",
  instagram: "",
  soundcloud: "",
};

const SETTINGS_TABS = ["general", "email", "integrations"] as const;

/// Secret fields (password, API keys) come back from the backend as a masked
/// placeholder, never the real value; saving the untouched mask keeps the
/// stored secret. Focusing selects it so typing replaces it whole.
const selectOnFocus = { onFocus: (e: FocusEvent<HTMLInputElement>) => e.currentTarget.select() };
type SettingsTab = (typeof SETTINGS_TABS)[number];

export function Settings() {
  const [tab, setTab] = useState<SettingsTab>("general");
  const { t } = useTranslation();
  const toast = useToast();
  const qc = useQueryClient();

  const [lang, setLang] = useState(i18n.language);
  const [theme, setTheme] = useState<Theme>(getTheme());
  useEffect(() => {
    const h = (e: Event) => setTheme((e as CustomEvent).detail as Theme);
    window.addEventListener("app:theme", h);
    return () => window.removeEventListener("app:theme", h);
  }, []);

  const { data: trackingUrl } = useQuery({
    queryKey: ["setting", "tracking_base_url"],
    queryFn: () => getSetting("tracking_base_url"),
  });
  const [trackVal, setTrackVal] = useState("");
  useEffect(() => {
    if (trackingUrl != null) setTrackVal(trackingUrl);
  }, [trackingUrl]);
  const [saveTracking, saveTrackingBusy] = useSubmit(async () => {
    await setSetting("tracking_base_url", trackVal.trim());
    qc.invalidateQueries({ queryKey: ["setting", "tracking_base_url"] });
    toast(t("settings.saved"), "ok");
  });

  // ---- Serper (venue website resolution) ----
  const { data: serperKey } = useQuery({
    queryKey: ["setting", "serper_api_key"],
    queryFn: () => getSetting("serper_api_key"),
  });
  const [serperVal, setSerperVal] = useState("");
  useEffect(() => {
    if (serperKey != null) setSerperVal(serperKey);
  }, [serperKey]);
  const [saveSerper, saveSerperBusy] = useSubmit(async () => {
    await setSetting("serper_api_key", serperVal.trim());
    qc.invalidateQueries({ queryKey: ["setting", "serper_api_key"] });
    toast(t("settings.saved"), "ok");
  });

  // ---- Radar: YouTube Data API key ----
  const { data: ytKey } = useQuery({
    queryKey: ["setting", "youtube_api_key"],
    queryFn: () => getSetting("youtube_api_key"),
  });
  const [ytVal, setYtVal] = useState("");
  useEffect(() => {
    if (ytKey != null) setYtVal(ytKey);
  }, [ytKey]);
  const [saveYt, saveYtBusy] = useSubmit(async () => {
    await setSetting("youtube_api_key", ytVal.trim());
    qc.invalidateQueries({ queryKey: ["setting", "youtube_api_key"] });
    toast(t("settings.saved"), "ok");
  });

  // ---- Pulse credentials (Spotify + Meta) ----
  const PULSE_KEYS = [
    "spotify_client_id",
    "spotify_client_secret",
    "meta_access_token",
    "meta_ad_account_id",
    "meta_result_action_type",
  ] as const;
  const { data: pulseRaw } = useQuery({
    queryKey: ["settings", "pulse"],
    queryFn: async () => {
      const out: Record<string, string> = {};
      for (const k of PULSE_KEYS) out[k] = (await getSetting(k)) ?? "";
      return out;
    },
  });
  const [pulseCfg, setPulseCfg] = useState<Record<string, string>>({});
  useEffect(() => {
    if (pulseRaw) setPulseCfg(pulseRaw);
  }, [pulseRaw]);
  const pinp = (k: string, type = "text") => ({
    className: "input",
    type,
    autoComplete: "off",
    value: pulseCfg[k] ?? "",
    onChange: (e: any) => setPulseCfg((c) => ({ ...c, [k]: e.target.value })),
    ...(type === "password" ? selectOnFocus : {}),
  });
  const [savePulse, savePulseBusy] = useSubmit(async () => {
    for (const k of PULSE_KEYS) await setSetting(k, (pulseCfg[k] ?? "").trim());
    qc.invalidateQueries({ queryKey: ["settings", "pulse"] });
    toast(t("settings.saved"), "ok");
  });

  // ---- Harvest performance (concurrent workers) ----
  const { data: workersRaw } = useQuery({
    queryKey: ["setting", "vi_workers"],
    queryFn: () => getSetting("vi_workers"),
  });
  const [workers, setWorkers] = useState("3");
  useEffect(() => {
    if (workersRaw) setWorkers(workersRaw);
  }, [workersRaw]);
  const [saveWorkers, saveWorkersBusy] = useSubmit(async (v: string) => {
    setWorkers(v);
    await setSetting("vi_workers", v);
    qc.invalidateQueries({ queryKey: ["setting", "vi_workers"] });
    toast(t("settings.saved"), "ok");
  });

  // ---- Email signature ----
  const { data: sigRaw } = useQuery({
    queryKey: ["setting", "email_signature"],
    queryFn: () => getSetting("email_signature"),
  });
  const [sig, setSig] = useState<SignatureFields>(EMPTY_SIG);
  useEffect(() => {
    if (sigRaw) {
      try {
        setSig({ ...EMPTY_SIG, ...JSON.parse(sigRaw) });
      } catch {
        /* ignore malformed */
      }
    }
  }, [sigRaw]);
  const sinp = (k: keyof SignatureFields) => ({
    className: "input",
    value: sig[k],
    onChange: (e: any) => setSig({ ...sig, [k]: e.target.value }),
  });
  const [saveSig, saveSigBusy] = useSubmit(async () => {
    await setSetting("email_signature", JSON.stringify(sig));
    qc.invalidateQueries({ queryKey: ["setting", "email_signature"] });
    toast(t("settings.saved"), "ok");
  });
  const st = (v: string) => v.trim();
  const sigPreview: string[] = [];
  {
    const title = [sig.name, sig.role].map(st).filter(Boolean).join(" · ");
    if (title) sigPreview.push(title);
    if (st(sig.label)) sigPreview.push(st(sig.label));
    const contact = [sig.booking_email, sig.phone].map(st).filter(Boolean).join(" · ");
    if (contact) sigPreview.push(contact);
    const links = [st(sig.website), st(sig.instagram) && "Instagram", st(sig.soundcloud) && "SoundCloud"]
      .filter(Boolean)
      .join(" · ");
    if (links) sigPreview.push(links);
  }

  const { data: smtp } = useQuery({
    queryKey: ["smtp"],
    queryFn: getSmtpConfig,
  });
  const [cfg, setCfg] = useState<SmtpConfig>({
    host: "",
    port: 587,
    username: "",
    password: "",
    from_name: "",
    from_email: "",
    encryption: "starttls",
  });
  useEffect(() => {
    if (smtp) setCfg(smtp);
  }, [smtp]);

  const [testing, setTesting] = useState(false);

  // ---- App version + updates ----
  const [version, setVersion] = useState("");
  const [checking, setChecking] = useState(false);
  // Found update awaiting an explicit "Install and restart" click.
  const [pending, setPending] = useState<Update | null>(null);
  const installing = useSyncExternalStore(subscribeInstalling, isInstalling);
  useEffect(() => {
    getVersion().then(setVersion).catch(() => {});
  }, []);

  const checkUpdate = async () => {
    if (checking || isInstalling()) return;
    setChecking(true);
    try {
      const update = await checkForUpdate(true);
      setPending(update);
      if (!update) toast(t("update.uptodate"), "ok");
    } catch (e) {
      toast(`${t("update.error")}: ${errorMessage(e)}`, "error");
    } finally {
      setChecking(false);
    }
  };

  const installUpdate = async () => {
    if (!pending) return;
    try {
      await installAndRelaunch(pending);
    } catch (e) {
      toast(`${t("update.error")}: ${errorMessage(e)}`, "error");
    }
  };

  useAppIntent("check-updates", () => {
    setTab("general");
    checkUpdate();
  });

  const changeLang = (l: string) => {
    setLang(l);
    i18n.changeLanguage(l);
    localStorage.setItem("lang", l);
  };
  const changeTheme = (th: Theme) => {
    setTheme(th);
    applyTheme(th);
  };

  // An emptied port field must not be saved as 0: fall back to the stored one, else 587.
  const normCfg = (): SmtpConfig => ({
    ...cfg,
    port: cfg.port > 0 ? cfg.port : smtp?.port && smtp.port > 0 ? smtp.port : 587,
  });

  const [saveSmtp, saveSmtpBusy] = useSubmit(async () => {
    const c = normCfg();
    await saveSmtpConfig(c);
    setCfg(c);
    qc.invalidateQueries({ queryKey: ["smtp"] });
    toast(t("settings.saved"), "ok");
  });

  const runTest = async () => {
    setTesting(true);
    try {
      const c = normCfg();
      await saveSmtpConfig(c);
      setCfg(c);
      const ok = await testSmtp();
      toast(ok ? t("settings.test_ok") : t("settings.test_failed"), ok ? "ok" : "error");
    } catch (e: any) {
      toast(`${t("settings.test_failed")}: ${e?.toString?.() ?? e}`, "error");
    } finally {
      setTesting(false);
    }
  };

  const cinp = (k: keyof SmtpConfig) => ({
    className: "input",
    value: k === "port" && !cfg.port ? "" : ((cfg[k] as any) ?? ""),
    onChange: (e: any) =>
      setCfg({
        ...cfg,
        [k]: k === "port" ? Number(e.target.value) : e.target.value,
      }),
  });

  return (
    <div>
      <PageHeader title={t("settings.title")} subtitle={t("settings.subtitle")}>
        {/* Grouped like the old single column, one concern per tab */}
        <div role="tablist" className="-mb-px flex gap-6">
          {SETTINGS_TABS.map((k) => (
            <button
              key={k}
              role="tab"
              aria-selected={tab === k}
              onClick={() => setTab(k)}
              className={clsx(
                "border-b-2 pb-2.5 font-mono text-[11px] uppercase tracking-[0.14em] transition-colors",
                tab === k ? "border-accent text-fg" : "border-transparent text-fg-subtle hover:text-fg"
              )}
            >
              {t(`settings.tab_${k}`)}
            </button>
          ))}
        </div>
      </PageHeader>
      {/* Cards flow into columns on a wide window instead of one 768px strip. */}
      <div className="grid items-start gap-6 px-page pb-10 pt-6 xl:grid-cols-2">
        {tab === "general" && (
        <>
        {/* Appearance */}
        <div className="card p-6">
          <div className="grid grid-cols-2 gap-6">
            <div>
              <div className="mb-2 flex items-center gap-2 text-sm font-semibold">
                <Globe size={15} /> {t("settings.language")}
              </div>
              <div className="flex gap-2">
                {["fr", "en"].map((l) => (
                  <button
                    key={l}
                    onClick={() => changeLang(l)}
                    className={
                      lang === l ? "btn-primary flex-1" : "btn-outline flex-1"
                    }
                  >
                    {l.toUpperCase()}
                  </button>
                ))}
              </div>
            </div>
            <div>
              <div className="mb-2 flex items-center gap-2 text-sm font-semibold">
                {theme === "dark" ? <Moon size={15} /> : <Sun size={15} />}{" "}
                {t("settings.theme")}
              </div>
              <div className="flex gap-2">
                <button
                  onClick={() => changeTheme("light")}
                  className={
                    theme === "light" ? "btn-primary flex-1" : "btn-outline flex-1"
                  }
                >
                  <Sun size={15} /> {t("settings.theme_light")}
                </button>
                <button
                  onClick={() => changeTheme("dark")}
                  className={
                    theme === "dark" ? "btn-primary flex-1" : "btn-outline flex-1"
                  }
                >
                  <Moon size={15} /> {t("settings.theme_dark")}
                </button>
              </div>
            </div>
          </div>
        </div>

        </>
        )}
        {tab === "email" && (
        <>
        {/* SMTP */}
        <div className="card p-6">
          <div className="mb-1 flex items-center gap-2 text-sm font-semibold">
            <Mail size={15} /> {t("settings.email_section")}
          </div>
          <p className="mb-2 text-xs text-fg-subtle">{t("settings.email_help")}</p>
          <div className="mb-4 panel flex gap-2.5 p-3 text-2xs leading-relaxed text-fg-subtle">
            <Info size={14} className="mt-0.5 shrink-0 text-accent-2" />
            <span>{t("settings.email_hint")}</span>
          </div>
          <div className="grid grid-cols-2 gap-x-4 gap-y-3">
            <Field label={t("settings.host")} className="col-span-2">
              <input {...cinp("host")} placeholder="ssl0.ovh.net / smtp.gmail.com" />
            </Field>
            <Field label={t("settings.encryption")}>
              <select
                className="input"
                value={cfg.encryption}
                onChange={(e) =>
                  setCfg({ ...cfg, encryption: e.target.value as any })
                }
              >
                <option value="starttls">{t("settings.enc_starttls")}</option>
                <option value="tls">{t("settings.enc_tls")}</option>
                <option value="none">{t("settings.enc_none")}</option>
              </select>
            </Field>
            <Field label={t("settings.port")}>
              <input type="number" {...cinp("port")} />
            </Field>
            <Field label={t("settings.username")}>
              <input {...cinp("username")} />
            </Field>
            <Field label={t("settings.password")}>
              <input type="password" autoComplete="off" {...cinp("password")} {...selectOnFocus} />
            </Field>
            <Field label={t("settings.from_name")}>
              <input {...cinp("from_name")} placeholder="Piers | Insrt" />
            </Field>
            <Field label={t("settings.from_email")}>
              <input {...cinp("from_email")} placeholder="piers@insrt.fr" />
            </Field>
          </div>
          <p className="mt-3 text-2xs text-fg-faint">{t("settings.secrets_note")}</p>
          <div className="mt-5 flex justify-end gap-2">
            <button className="btn-outline" onClick={runTest} disabled={testing || saveSmtpBusy}>
              <Plug size={15} />
              {testing ? t("settings.testing") : t("settings.test")}
            </button>
            <button className="btn-primary" onClick={saveSmtp} disabled={saveSmtpBusy}>
              {t("common.save")}
            </button>
          </div>
        </div>

        </>
        )}
        {tab === "email" && (
        <>
        {/* Open tracking */}
        <div className="card p-6">
          <div className="mb-1 flex items-center gap-2 text-sm font-semibold">
            <Eye size={15} /> {t("settings.tracking_section")}
          </div>
          <p className="mb-4 text-xs text-fg-subtle">{t("settings.tracking_help")}</p>

          <div className="mb-4 panel p-4">
            <div className="kicker mb-3">{t("settings.tracking_steps_title")}</div>
            <ol className="space-y-2.5">
              {["tracking_s1", "tracking_s2", "tracking_s3", "tracking_s4"].map((k, i) => (
                <li key={k} className="flex gap-3 text-xs leading-relaxed text-fg-subtle">
                  <span className="flex h-5 w-5 shrink-0 items-center justify-center bg-accent text-2xs font-bold text-accent-fg tabular">
                    {i + 1}
                  </span>
                  <span>{t(`settings.${k}`)}</span>
                </li>
              ))}
            </ol>
          </div>

          <div className="flex items-end gap-3">
            <Field label={t("settings.tracking_url")} className="flex-1">
              <input
                className="input"
                value={trackVal}
                onChange={(e) => setTrackVal(e.target.value)}
                placeholder="https://track.insrt.fr"
              />
            </Field>
            <button className="btn-primary" onClick={saveTracking} disabled={saveTrackingBusy}>
              {t("common.save")}
            </button>
          </div>
          <p className="mt-3 text-2xs text-fg-faint">{t("settings.tracking_note")}</p>
        </div>

        </>
        )}
        {tab === "email" && (
        <>
        {/* Email signature */}
        <div className="card p-6">
          <div className="mb-1 flex items-center gap-2 text-sm font-semibold">
            <PenLine size={15} /> {t("settings.sig_section")}
          </div>
          <p className="mb-4 text-xs text-fg-subtle">{t("settings.sig_help")}</p>
          <div className="grid grid-cols-2 gap-x-4 gap-y-3">
            <Field label={t("settings.sig_name")}>
              <input {...sinp("name")} placeholder="Piers" />
            </Field>
            <Field label={t("settings.sig_role")}>
              <input {...sinp("role")} placeholder="DJ & Producer · House / Tech House" />
            </Field>
            <Field label={t("settings.sig_label")}>
              <input {...sinp("label")} placeholder="Insrt" />
            </Field>
            <Field label={t("settings.sig_phone")}>
              <input {...sinp("phone")} placeholder="+33 6 12 34 56 78" />
            </Field>
            <Field label={t("settings.sig_booking_email")}>
              <input {...sinp("booking_email")} placeholder="booking@insrt.fr" />
            </Field>
            <Field label={t("settings.sig_website")}>
              <input {...sinp("website")} placeholder="insrt.fr" />
            </Field>
            <Field label={t("settings.sig_instagram")}>
              <input {...sinp("instagram")} placeholder="@piers" />
            </Field>
            <Field label={t("settings.sig_soundcloud")}>
              <input {...sinp("soundcloud")} placeholder="soundcloud.com/piers" />
            </Field>
          </div>

          <div className="mt-4">
            <div className="kicker mb-2">{t("settings.sig_preview")}</div>
            <div className="panel p-4 text-xs leading-relaxed text-fg-subtle">
              {sigPreview.length ? (
                sigPreview.map((l, i) => (
                  <div key={i} className={i === 0 ? "font-semibold text-fg" : ""}>
                    {l}
                  </div>
                ))
              ) : (
                <span className="text-fg-faint">{t("settings.sig_empty")}</span>
              )}
            </div>
          </div>

          <div className="mt-5 flex justify-end">
            <button className="btn-primary" onClick={saveSig} disabled={saveSigBusy}>
              {t("common.save")}
            </button>
          </div>
        </div>

        </>
        )}
        {tab === "integrations" && (
        <>
        {/* Venue website resolution (Serper) */}
        <div className="card p-6">
          <div className="mb-1 flex items-center gap-2 text-sm font-semibold">
            <Search size={15} /> {t("settings.enrich_section")}
          </div>
          <p className="mb-4 text-xs text-fg-subtle">{t("settings.enrich_help")}</p>
          <div className="flex items-end gap-3">
            <Field label={t("settings.serper_key")} className="flex-1">
              <input
                type="password"
                className="input"
                value={serperVal}
                onChange={(e) => setSerperVal(e.target.value)}
                placeholder="serper.dev API key"
                autoComplete="off"
                {...selectOnFocus}
              />
            </Field>
            <button className="btn-primary" onClick={saveSerper} disabled={saveSerperBusy}>
              {t("common.save")}
            </button>
          </div>
          <p className="mt-3 text-2xs text-fg-faint">{t("settings.serper_hint")}</p>
        </div>

        </>
        )}
        {tab === "integrations" && (
        <>
        {/* Harvest performance */}
        <div className="card p-6">
          <div className="mb-1 flex items-center gap-2 text-sm font-semibold">
            <RefreshCw size={15} /> {t("settings.perf_section")}
          </div>
          <p className="mb-4 text-xs text-fg-subtle">{t("settings.perf_help")}</p>
          <div className="flex items-end gap-3">
            <Field label={t("settings.perf_workers")}>
              <select
                className="input w-28"
                value={workers}
                disabled={saveWorkersBusy}
                onChange={(e) => saveWorkers(e.target.value)}
              >
                {[1, 2, 3, 4, 5, 6].map((n) => (
                  <option key={n} value={String(n)}>
                    {n}
                  </option>
                ))}
              </select>
            </Field>
            <p className="pb-2 text-2xs text-fg-faint">{t("settings.perf_hint")}</p>
          </div>
        </div>

        </>
        )}
        {tab === "integrations" && (
        <>
        {/* Radar (YouTube) */}
        <div className="card p-6">
          <div className="mb-1 flex items-center gap-2 text-sm font-semibold">
            <Search size={15} /> {t("settings.radar_section")}
          </div>
          <p className="mb-4 text-xs text-fg-subtle">{t("settings.radar_help")}</p>
          <div className="flex items-end gap-3">
            <Field label={t("settings.youtube_key")} className="flex-1">
              <input
                type="password"
                className="input"
                value={ytVal}
                onChange={(e) => setYtVal(e.target.value)}
                placeholder="YouTube Data API v3 key"
                autoComplete="off"
                {...selectOnFocus}
              />
            </Field>
            <button className="btn-primary" onClick={saveYt} disabled={saveYtBusy}>
              {t("common.save")}
            </button>
          </div>
          <p className="mt-3 text-2xs text-fg-faint">{t("settings.youtube_hint")}</p>
        </div>

        </>
        )}
        {tab === "integrations" && (
        <>
        {/* Pulse (Spotify + Meta) */}
        <div className="card p-6">
          <div className="mb-1 flex items-center gap-2 text-sm font-semibold">
            <Activity size={15} /> {t("settings.pulse_section")}
          </div>
          <p className="mb-4 text-xs text-fg-subtle">{t("settings.pulse_help")}</p>
          <div className="grid grid-cols-2 gap-x-4 gap-y-3">
            <Field label="Spotify Client ID">
              <input {...pinp("spotify_client_id")} />
            </Field>
            <Field label="Spotify Client Secret">
              <input {...pinp("spotify_client_secret", "password")} />
            </Field>
            <Field label={t("settings.pulse_meta_token")}>
              <input {...pinp("meta_access_token", "password")} />
            </Field>
            <Field label={t("settings.pulse_meta_account")}>
              <input {...pinp("meta_ad_account_id")} placeholder="act_1234567890 ou 1234567890" />
            </Field>
            <Field label={t("settings.pulse_action_type")}>
              <input {...pinp("meta_result_action_type")} placeholder="link_click" />
            </Field>
          </div>
          <p className="mt-3 text-2xs text-fg-faint">{t("settings.pulse_hint")}</p>
          <p className="mt-1 text-2xs text-fg-faint">{t("settings.secrets_note")}</p>
          <div className="mt-4 flex justify-end">
            <button className="btn-primary" onClick={savePulse} disabled={savePulseBusy}>
              {t("common.save")}
            </button>
          </div>
        </div>

        </>
        )}
        {tab === "general" && (
        <>
        {/* Updates */}
        <div className="card p-6">
          <div className="mb-1 flex items-center gap-2 text-sm font-semibold">
            <RefreshCw size={15} /> {t("update.section")}
          </div>
          <p className="mb-4 text-xs text-fg-subtle">{t("update.help")}</p>
          <div className="flex items-center justify-between">
            <div className="text-sm">
              <span className="text-fg-subtle">{t("update.current")} </span>
              <span className="font-mono font-medium">v{version || "·"}</span>
            </div>
            {pending ? (
              <div className="flex items-center gap-3">
                <span className="text-sm text-fg">
                  {t("update.available", { version: pending.version })}
                </span>
                <button className="btn-primary" onClick={installUpdate} disabled={installing}>
                  <Download size={15} />
                  {installing ? t("update.installing") : t("update.install")}
                </button>
              </div>
            ) : (
              <button
                className="btn-primary"
                onClick={checkUpdate}
                disabled={checking || installing}
              >
                {installing ? (
                  <Download size={15} />
                ) : (
                  <RefreshCw size={15} className={checking ? "animate-spin" : ""} />
                )}
                {installing
                  ? t("update.installing")
                  : checking
                  ? t("update.checking")
                  : t("update.check")}
              </button>
            )}
          </div>
        </div>
        </>
        )}
      </div>
    </div>
  );
}
