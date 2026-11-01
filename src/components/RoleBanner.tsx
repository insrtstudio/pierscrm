import { useState } from "react";
import { useNavigate } from "react-router-dom";
import { useTranslation } from "react-i18next";
import { ArrowRight, Info, X } from "lucide-react";

/// One line at the top of Scouting and Outreach saying what the page is for,
/// with a link to the other one: the two were easy to mix up. Hidden for good
/// once dismissed.
export function RoleBanner({ id, text, link, to }: { id: string; text: string; link: string; to: string }) {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const key = `banner_dismissed:${id}`;
  const [hidden, setHidden] = useState(() => {
    try {
      return localStorage.getItem(key) === "1";
    } catch {
      return false;
    }
  });
  if (hidden) return null;
  const dismiss = () => {
    setHidden(true);
    try {
      localStorage.setItem(key, "1");
    } catch {
      // Private window: it just comes back next launch.
    }
  };
  return (
    <div className="flex items-center gap-3 rounded-lg border border-border bg-surface px-4 py-2 text-xs text-fg-subtle">
      <Info size={14} className="shrink-0 text-accent" />
      <span className="min-w-0 flex-1">
        {text}{" "}
        <button className="inline-flex items-center gap-1 text-accent hover:underline" onClick={() => navigate(to)}>
          {link}
          <ArrowRight size={12} />
        </button>
      </span>
      <button className="btn-ghost px-1.5 py-1" onClick={dismiss} title={t("scouting.dismiss")} aria-label={t("scouting.dismiss")}>
        <X size={13} />
      </button>
    </div>
  );
}
