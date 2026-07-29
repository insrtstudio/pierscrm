import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { Download, RefreshCw, X } from "lucide-react";
import type { Update } from "@tauri-apps/plugin-updater";
import { checkForUpdate, installAndRelaunch } from "../lib/updater";

/** Silent auto-check on launch; shows a banner if an update is available. */
export function UpdateBanner() {
  const { t } = useTranslation();
  const [update, setUpdate] = useState<Update | null>(null);
  const [dismissed, setDismissed] = useState(false);
  const [busy, setBusy] = useState(false);
  const [pct, setPct] = useState<number | null>(null);

  useEffect(() => {
    // Slight delay so the app paints first.
    const id = setTimeout(async () => {
      const u = await checkForUpdate(false);
      if (u) setUpdate(u);
    }, 2500);
    return () => clearTimeout(id);
  }, []);

  if (!update || dismissed) return null;

  const install = async () => {
    setBusy(true);
    try {
      await installAndRelaunch(update, (p) => {
        if (p.phase === "downloading" && p.total)
          setPct(Math.round((p.downloaded / p.total) * 100));
      });
    } catch {
      setBusy(false);
    }
  };

  return (
    <div className="flex items-center gap-3 border-b border-accent/30 bg-accent/10 px-4 py-2 text-sm">
      <RefreshCw size={15} className="text-accent" />
      <span className="text-fg">
        {t("update.available", { version: update.version })}
      </span>
      <div className="ml-auto flex items-center gap-2">
        <button className="btn-primary py-1.5" onClick={install} disabled={busy}>
          <Download size={14} />
          {busy
            ? pct != null
              ? `${pct}%`
              : t("update.installing")
            : t("update.install")}
        </button>
        {!busy && (
          <button
            className="btn-ghost px-2 py-1.5"
            aria-label={t("common.close")}
            onClick={() => setDismissed(true)}
          >
            <X size={15} />
          </button>
        )}
      </div>
    </div>
  );
}
