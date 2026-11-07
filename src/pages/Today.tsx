import { useTranslation } from "react-i18next";
import { useQuery } from "@tanstack/react-query";
import { Link } from "react-router-dom";
import clsx from "clsx";
import { ArrowUpRight, CalendarDays, Inbox, Megaphone, type LucideIcon } from "lucide-react";
import { PageHeader } from "../components/Layout";
import { Actions } from "./Actions";
import { DmQueue } from "../components/DmQueue";
import { CreditsBanner } from "../components/Credits";
import { deskCount } from "../lib/replyDesk";
import { listArtists, listCampaigns, listEvents } from "../lib/api";
import { parseDay, today } from "../lib/format";

/// The landing screen: what needs Thibault now. Three glanceable tiles (the
/// replies waiting, the next date, the campaigns running), then the week of
/// actions the app has lined up. The figures live in Résultats.
export function Today() {
  const { t, i18n } = useTranslation();
  const lang = i18n.language;
  const day = today();
  const { data: desk } = useQuery({ queryKey: ["reply_desk_count"], queryFn: deskCount, refetchInterval: 5 * 60_000 });
  const { data: events = [] } = useQuery({ queryKey: ["events", "ticker", day], queryFn: () => listEvents({ from: day }) });
  const { data: artists = [] } = useQuery({ queryKey: ["artists"], queryFn: listArtists });
  const { data: campaigns = [] } = useQuery({ queryKey: ["campaigns"], queryFn: listCampaigns });

  const next = events.find((e) => e.status !== "cancelled");
  const nextDate = next ? parseDay(next.date) : null;
  const active = campaigns.filter((c) => c.status === "active").length;
  const dateLabel = parseDay(day)!.toLocaleDateString(lang, { weekday: "long", day: "numeric", month: "long" });

  return (
    <div>
      <PageHeader title={t("nav.today")} subtitle={dateLabel.charAt(0).toUpperCase() + dateLabel.slice(1)} />
      <div className="grid gap-3 px-page pt-6 [grid-template-columns:repeat(auto-fit,minmax(min(100%,205px),1fr))]">
        <Tile
          to="/replies"
          icon={Inbox}
          label={t("today.replies")}
          value={desk?.open ?? 0}
          hint={
            desk && desk.overdue > 0
              ? t("today.replies_overdue", { count: desk.overdue })
              : desk?.open
                ? t("today.replies_hint")
                : t("today.replies_none")
          }
          hot={!!desk && desk.open > 0}
          danger={!!desk && desk.overdue > 0}
        />
        <Tile
          to="/agenda"
          icon={CalendarDays}
          label={t("today.next_gig")}
          value={
            nextDate
              ? nextDate.toLocaleDateString(lang, { day: "numeric", month: "short" })
              : "·"
          }
          hint={
            next
              ? [artists.find((a) => a.id === next.artist_id)?.name || next.title, next.venue, next.city]
                  .filter(Boolean)
                  .join(" · ")
              : t("today.next_gig_none")
          }
        />
        <Tile
          to="/campaigns"
          icon={Megaphone}
          label={t("today.campaigns")}
          value={active}
          hint={t("today.campaigns_hint", { count: active })}
        />
      </div>
      <div className="px-page pt-4 empty:hidden">
        <CreditsBanner />
      </div>
      <DmQueue />
      <Actions />
    </div>
  );
}

function Tile({
  to,
  icon: Icon,
  label,
  value,
  hint,
  hot,
  danger,
}: {
  to: string;
  icon: LucideIcon;
  label: string;
  value: React.ReactNode;
  hint?: string;
  hot?: boolean;
  danger?: boolean;
}) {
  return (
    <Link
      to={to}
      className={clsx(
        "card group relative flex min-w-0 items-start gap-3 px-3.5 py-3.5 transition-colors hover:border-fg/30",
        hot && "border-accent/40 bg-accent-soft/30"
      )}
    >
      <div
        className={clsx(
          "flex h-9 w-9 shrink-0 items-center justify-center rounded-full",
          danger ? "bg-danger/10 text-danger" : hot ? "bg-accent/15 text-accent-2" : "bg-fg/[0.05] text-fg-subtle"
        )}
      >
        <Icon size={17} strokeWidth={1.75} />
      </div>
      <div className="min-w-0 flex-1">
        <div className="eyebrow truncate pr-2.5 text-[10px] tracking-[0.08em]">{label}</div>
        <div className="mt-1 truncate font-condensed text-[26px] leading-none tabular">{value}</div>
        {hint && <div className="mt-1 line-clamp-2 text-xs leading-snug text-fg-subtle">{hint}</div>}
      </div>
      <ArrowUpRight size={14} className="absolute right-2.5 top-2.5 text-fg-faint transition-colors group-hover:text-fg" />
    </Link>
  );
}
