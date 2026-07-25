import { useEffect, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { Globe, Moon, Sun, Mail, Plug, Eye, Download, RefreshCw } from "lucide-react";
import { getVersion } from "@tauri-apps/api/app";
import i18n from "../i18n";
import { checkForUpdate, installAndRelaunch } from "../lib/updater";
import {
  getSetting,
  getSmtpConfig,
  saveSmtpConfig,
  setSetting,
  testSmtp,
} from "../lib/api";
import type { SmtpConfig } from "../lib/types";
import { PageHeader } from "../components/Layout";
import { Field, useToast } from "../components/ui";

export function Settings() {
  const { t } = useTranslation();
  const toast = useToast();
  const qc = useQueryClient();

  const [lang, setLang] = useState(i18n.language);
  const [theme, setTheme] = useState(
    localStorage.getItem("theme") || "dark"
  );

  const { data: trackingUrl } = useQuery({
    queryKey: ["setting", "tracking_base_url"],
    queryFn: () => getSetting("tracking_base_url"),
  });
  const [trackVal, setTrackVal] = useState("");
  useEffect(() => {
    if (trackingUrl != null) setTrackVal(trackingUrl);
  }, [trackingUrl]);
  const saveTracking = async () => {
    await setSetting("tracking_base_url", trackVal.trim());
    qc.invalidateQueries({ queryKey: ["setting", "tracking_base_url"] });
    toast(t("settings.saved"), "ok");
  };

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
  const [installing, setInstalling] = useState(false);
  useEffect(() => {
    getVersion().then(setVersion).catch(() => {});
  }, []);

  const checkUpdate = async () => {
    setChecking(true);
    try {
      const update = await checkForUpdate(true);
      if (!update) {
        toast(t("update.uptodate"), "ok");
        return;
      }
      toast(t("update.available", { version: update.version }), "ok");
      setInstalling(true);
      await installAndRelaunch(update);
    } catch (e: any) {
      toast(`${t("update.error")}: ${e?.toString?.() ?? e}`, "error");
    } finally {
      setChecking(false);
      setInstalling(false);
    }
  };

  useEffect(() => {
    const h = () => checkUpdate();
    window.addEventListener("app:check-updates", h);
    return () => window.removeEventListener("app:check-updates", h);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const changeLang = (l: string) => {
    setLang(l);
    i18n.changeLanguage(l);
    localStorage.setItem("lang", l);
  };
  const changeTheme = (th: string) => {
    setTheme(th);
    localStorage.setItem("theme", th);
    document.documentElement.classList.toggle("dark", th === "dark");
  };

  const saveSmtp = async () => {
    await saveSmtpConfig(cfg);
    qc.invalidateQueries({ queryKey: ["smtp"] });
    toast(t("settings.saved"), "ok");
  };

  const runTest = async () => {
    await saveSmtpConfig(cfg);
    setTesting(true);
    try {
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
    value: (cfg[k] as any) ?? "",
    onChange: (e: any) =>
      setCfg({
        ...cfg,
        [k]: k === "port" ? Number(e.target.value) : e.target.value,
      }),
  });

  return (
    <div>
      <PageHeader title={t("settings.title")} subtitle={t("settings.subtitle")} />
      <div className="max-w-3xl space-y-6 px-8 pb-10">
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

        {/* SMTP */}
        <div className="card p-6">
          <div className="mb-1 flex items-center gap-2 text-sm font-semibold">
            <Mail size={15} /> {t("settings.email_section")}
          </div>
          <p className="mb-4 text-xs text-fg-subtle">{t("settings.email_help")}</p>
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
              <input type="password" {...cinp("password")} />
            </Field>
            <Field label={t("settings.from_name")}>
              <input {...cinp("from_name")} placeholder="Piers — Insrt.Studio" />
            </Field>
            <Field label={t("settings.from_email")}>
              <input {...cinp("from_email")} placeholder="piers@insrt.fr" />
            </Field>
          </div>
          <div className="mt-5 flex justify-end gap-2">
            <button className="btn-outline" onClick={runTest} disabled={testing}>
              <Plug size={15} />
              {testing ? t("settings.testing") : t("settings.test")}
            </button>
            <button className="btn-primary" onClick={saveSmtp}>
              {t("common.save")}
            </button>
          </div>
        </div>

        {/* Open tracking */}
        <div className="card p-6">
          <div className="mb-1 flex items-center gap-2 text-sm font-semibold">
            <Eye size={15} /> {t("settings.tracking_section")}
          </div>
          <p className="mb-4 text-xs text-fg-subtle">{t("settings.tracking_help")}</p>
          <div className="flex items-end gap-3">
            <Field label={t("settings.tracking_url")} className="flex-1">
              <input
                className="input"
                value={trackVal}
                onChange={(e) => setTrackVal(e.target.value)}
                placeholder="https://track.insrt.fr"
              />
            </Field>
            <button className="btn-primary" onClick={saveTracking}>
              {t("common.save")}
            </button>
          </div>
        </div>

        {/* Updates */}
        <div className="card p-6">
          <div className="mb-1 flex items-center gap-2 text-sm font-semibold">
            <RefreshCw size={15} /> {t("update.section")}
          </div>
          <p className="mb-4 text-xs text-fg-subtle">{t("update.help")}</p>
          <div className="flex items-center justify-between">
            <div className="text-sm">
              <span className="text-fg-subtle">{t("update.current")} </span>
              <span className="font-mono font-medium">v{version || "—"}</span>
            </div>
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
          </div>
        </div>
      </div>
    </div>
  );
}
