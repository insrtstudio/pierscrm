import { useTranslation } from "react-i18next";
import clsx from "clsx";
import { CalendarClock } from "lucide-react";
import { formatDate } from "../lib/format";
import { parseFit, type FitDetail, type Timing } from "../lib/targets";

/// "a programmé X, Y ; 14 soirées house en 12 mois", in the UI language.
export function useFitExplain() {
  const { t } = useTranslation();
  return (d: FitDetail | null): string => {
    if (!d) return t("fit.none");
    const parts: string[] = [];
    if (d.matched.length) parts.push(t("fit.explain_matched", { names: d.matched.slice(0, 3).join(", ") }));
    parts.push(
      d.genre_nights > 0
        ? t("fit.explain_genre", { count: d.genre_nights, genre: d.genre_label })
        : t("fit.explain_nights", { count: d.nights })
    );
    return parts.join(" ; ");
  };
}

const tone = (s: number) => (s >= 60 ? "text-ok" : s >= 35 ? "text-accent-2" : "text-fg-faint");

/// Score 0-100 with its explanation and breakdown on hover. "·" when no score.
export function FitScore({ score, detail, className }: { score?: number | null; detail?: string | null; className?: string }) {
  const { t } = useTranslation();
  const explain = useFitExplain();
  const d = parseFit(detail);
  if (score == null) {
    return (
      <span className={clsx("text-fg-faint", className)} title={t("fit.none")}>
        ·
      </span>
    );
  }
  const title = d
    ? `${explain(d)}\n${t("fit.parts", { ...d.parts })}`
    : explain(null);
  return (
    <span className={clsx("font-condensed text-lg tabular", tone(score), className)} title={title}>
      {score}
    </span>
  );
}

/// One line explanation, for lists that have room for it.
export function FitLine({ detail }: { detail?: string | null }) {
  const explain = useFitExplain();
  const d = parseFit(detail);
  if (!d) return null;
  return <span className="text-2xs text-fg-subtle">{explain(d)}</span>;
}

const TIMING_STYLE: Record<string, string> = {
  now: "bg-ok/15 text-ok",
  soon: "bg-accent-2/15 text-accent-2",
  later: "bg-muted text-fg-subtle",
  unknown: "bg-muted text-fg-faint",
};

/// "Bon moment pour écrire" badge, with the lead time and window on hover.
export function TimingBadge({ timing, compact }: { timing?: Timing | null; compact?: boolean }) {
  const { t } = useTranslation();
  if (!timing) return null;
  const date = formatDate(timing.write_from);
  const label = t(`fit.timing_${timing.state}`, { date });
  const hint = [
    t("fit.timing_lead", { weeks: timing.lead_weeks }),
    timing.year_round ? t("fit.timing_year_round") : null,
    timing.state === "later" || timing.state === "soon" ? t("fit.timing_from", { date }) : null,
  ]
    .filter(Boolean)
    .join("\n");
  return (
    <span className={clsx("badge shrink-0 whitespace-nowrap", TIMING_STYLE[timing.state])} title={hint}>
      <CalendarClock size={11} />
      {!compact && label}
    </span>
  );
}
