import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { Link, useNavigate } from "react-router-dom";
import { Plus, Mail } from "lucide-react";
import {
  dashboardStats,
  listArtists,
  listCampaigns,
  listEmails,
  listEvents,
} from "../lib/api";
import { PageHeader, EmptyState } from "../components/Layout";
import { STATUS_DOT } from "../lib/constants";
import { STATUSES, type Status } from "../lib/types";

function euro(n: number) {
  return new Intl.NumberFormat("fr-FR", {
    style: "currency",
    currency: "EUR",
    maximumFractionDigits: 0,
  }).format(n || 0);
}
function isoToday() {
  const d = new Date();
  return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-${String(
    d.getDate()
  ).padStart(2, "0")}`;
}

export function Dashboard() {
  const { t } = useTranslation();
  const navigate = useNavigate();

  const { data: stats } = useQuery({ queryKey: ["dashboard"], queryFn: dashboardStats });
  const { data: artists = [] } = useQuery({ queryKey: ["artists"], queryFn: listArtists });
  const { data: campaigns = [] } = useQuery({ queryKey: ["campaigns"], queryFn: listCampaigns });
  const { data: emails = [] } = useQuery({ queryKey: ["emails"], queryFn: () => listEmails() });
  const today = isoToday();
  const { data: upcoming = [] } = useQuery({
    queryKey: ["events", "upcoming", today],
    queryFn: () => listEvents({ from: today }),
  });

  const sent = emails.filter((e) => e.status === "sent").length;
  const opened = emails.filter((e) => e.opened_at).length;
  const openRate = sent ? Math.round((opened / sent) * 100) : 0;
  const activeCampaigns = campaigns.filter((c) => c.status === "active").length;

  const pipeline = STATUSES.map((s) => ({
    key: s,
    label: t(`status.${s}`),
    value: stats?.by_status[s] ?? 0,
  })).filter((d) => d.value > 0);
  const maxPipe = Math.max(1, ...pipeline.map((d) => d.value));

  const cats = Object.entries(stats?.by_category ?? {});
  const catTotal = cats.reduce((a, [, v]) => a + v, 0) || 1;
  const CAT_COLOR: Record<string, string> = {
    venue: "#ec3013",
    lineup: "#8a847e",
    major: "#3a3532",
    other: "#5c5651",
  };

  const months = t("agenda.months").split("_");
  const net = (stats?.revenue_actual ?? 0) - (stats?.budget_actual ?? 0);

  const stat = [
    { label: t("dashboard.artists"), value: artists.length },
    { label: t("dashboard.contacts"), value: stats?.total_contacts ?? 0 },
    { label: t("dashboard.confirmed"), value: stats?.by_status["confirmed"] ?? 0, hot: true },
    { label: t("dashboard.upcoming_events"), value: upcoming.length },
    { label: t("dashboard.active_campaigns"), value: activeCampaigns },
    { label: t("dashboard.open_rate"), value: `${openRate}`, unit: "%" },
  ];

  return (
    <div>
      <PageHeader
        kicker="PILOTAGE / VUE GÉNÉRALE"
        title={t("dashboard.title")}
        actions={
          <>
            <span className="mr-1 inline-flex items-center gap-2 border border-accent px-2.5 py-1.5 text-2xs font-bold uppercase tracking-widest text-accent-2">
              <span className="h-[7px] w-[7px] bg-accent animate-livepulse" />
              {t("agenda.months").split("_")[new Date().getMonth()].slice(0, 3)} 2026
            </span>
            <button className="btn-outline" onClick={() => navigate("/emails")}>
              <Mail size={15} />
              {t("artists.write_email")}
            </button>
            <button className="btn-primary" onClick={() => navigate("/agenda")}>
              <Plus size={15} />
              {t("agenda.new_event")}
            </button>
          </>
        }
      />

      {/* Stat band */}
      <div className="grid grid-cols-2 border-b-2 border-border sm:grid-cols-3 xl:grid-cols-6">
        {stat.map((s, i) => (
          <div
            key={i}
            className="border-b-2 border-r-2 border-border px-6 py-6 last:border-r-0 xl:border-b-0"
          >
            <div className="text-2xs font-bold uppercase tracking-widest text-fg-subtle">
              {s.label}
            </div>
            <div
              className={`mt-2 text-[52px] font-black leading-[0.9] tracking-tightest tabular ${
                s.hot ? "text-accent text-glow" : "text-fg"
              }`}
            >
              {s.value}
              {s.unit && <span className="text-[24px] text-fg-subtle">{s.unit}</span>}
            </div>
          </div>
        ))}
      </div>

      <div className="grid grid-cols-1 lg:grid-cols-[1fr_360px]">
        {/* Left: pipeline + upcoming */}
        <div className="border-b-2 border-border p-8 lg:border-b-0 lg:border-r-2">
          <div className="mb-5 flex items-baseline justify-between">
            <div className="kicker">{t("dashboard.by_status")}</div>
            <Link to="/contacts" className="text-2xs font-bold uppercase tracking-widest text-accent-2">
              {t("nav.contacts")} →
            </Link>
          </div>

          {pipeline.length === 0 ? (
            <EmptyState icon={Mail} title={t("common.empty")} />
          ) : (
            <div className="grid grid-cols-[130px_1fr_36px] items-center gap-x-4 gap-y-3 text-sm">
              {pipeline.map((d) => (
                <PipelineRow key={d.key} label={d.label} value={d.value} max={maxPipe} statusKey={d.key} />
              ))}
            </div>
          )}

          {/* Upcoming */}
          <div className="mt-8 border-t-2 border-border pt-6">
            <div className="mb-4 flex items-baseline justify-between">
              <div className="kicker">{t("dashboard.upcoming")}</div>
              <Link to="/agenda" className="text-2xs font-bold uppercase tracking-widest text-accent-2">
                {t("dashboard.view_agenda")} →
              </Link>
            </div>
            {upcoming.length === 0 ? (
              <p className="text-sm text-fg-subtle">{t("dashboard.no_upcoming")}</p>
            ) : (
              <div>
                {upcoming.slice(0, 6).map((ev) => {
                  const d = new Date(ev.date);
                  const artist = artists.find((a) => a.id === ev.artist_id);
                  return (
                    <div
                      key={ev.id}
                      className="grid grid-cols-[64px_1fr_auto] items-center gap-4 border-b border-border py-3 transition-colors hover:bg-muted/50"
                    >
                      <div className="tabular">
                        <span className="text-[26px] font-black leading-none">
                          {String(d.getDate()).padStart(2, "0")}
                        </span>
                        <span className="text-2xs text-fg-faint">
                          {" "}
                          {months[d.getMonth()]?.slice(0, 3).toUpperCase()}
                        </span>
                      </div>
                      <div className="min-w-0">
                        <div className="truncate text-sm font-bold">{ev.title}</div>
                        <div className="truncate text-2xs uppercase tracking-wide text-fg-subtle">
                          {[artist?.name, ev.venue].filter(Boolean).join(" · ")}
                        </div>
                      </div>
                      {ev.start_time && (
                        <div className="text-sm font-bold tabular text-accent-2">{ev.start_time}</div>
                      )}
                    </div>
                  );
                })}
              </div>
            )}
          </div>
        </div>

        {/* Right: category + budget */}
        <div className="p-8">
          <div className="kicker mb-4">{t("dashboard.by_category")}</div>
          {cats.length > 0 && (
            <>
              <div className="mb-4 flex h-4">
                {cats.map(([k, v]) => (
                  <div
                    key={k}
                    style={{ width: `${(v / catTotal) * 100}%`, background: CAT_COLOR[k] ?? "#5c5651" }}
                  />
                ))}
              </div>
              <div className="space-y-2.5">
                {cats.map(([k, v]) => (
                  <div key={k} className="flex items-center gap-2.5 text-sm">
                    <span className="h-2.5 w-2.5" style={{ background: CAT_COLOR[k] ?? "#5c5651" }} />
                    <span className="text-fg-subtle">{t(`category.${k}`, k)}</span>
                    <span className="ml-auto font-bold tabular">{v}</span>
                  </div>
                ))}
              </div>
            </>
          )}

          {/* Budget */}
          <div className="mt-8 border-t-2 border-border pt-6">
            <div className="kicker mb-4">{t("dashboard.budget")}</div>
            <div className="border border-accent p-5 shadow-[inset_0_0_24px_rgba(236,48,19,0.15)]">
              <div className="text-2xs font-bold uppercase tracking-widest text-fg-subtle">
                {t("dashboard.net")}
              </div>
              <div className="mt-1 text-[38px] font-black leading-none tracking-tightest tabular text-accent text-glow">
                {euro(net)}
              </div>
              <div className="mt-3 space-y-1 text-2xs uppercase tracking-wide text-fg-subtle">
                <div>
                  {t("budget.min")} – {t("budget.max")}:{" "}
                  <span className="font-bold text-fg">
                    {euro(stats?.budget_min ?? 0)} – {euro(stats?.budget_max ?? 0)}
                  </span>
                </div>
                <div>
                  {t("dashboard.revenue")}:{" "}
                  <span className="font-bold text-fg">{euro(stats?.revenue_actual ?? 0)}</span>
                </div>
              </div>
            </div>
          </div>
        </div>
      </div>
    </div>
  );
}

function PipelineRow({
  label,
  value,
  max,
  statusKey,
}: {
  label: string;
  value: number;
  max: number;
  statusKey: Status;
}) {
  const color = STATUS_DOT[statusKey];
  return (
    <>
      <div className="truncate text-2xs uppercase tracking-wide text-fg-subtle">{label}</div>
      <div className="h-2.5 bg-muted">
        <div
          className="h-full"
          style={{ width: `${(value / max) * 100}%`, background: color, minWidth: 3 }}
        />
      </div>
      <div className="text-right text-sm font-bold tabular" style={{ color }}>
        {value}
      </div>
    </>
  );
}
