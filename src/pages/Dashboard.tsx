import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { Link, useNavigate } from "react-router-dom";
import { Plus, Mail, AlertTriangle } from "lucide-react";
import {
  dashboardStats,
  listArtists,
  listCampaigns,
  listEvents,
  listEventVisas,
  listTasks,
} from "../lib/api";
import { PageHeader, EmptyState } from "../components/Layout";
import { AudienceTile } from "../components/AudienceTile";
import { daysUntil, euro, formatDate, parseDay, today as todayDay } from "../lib/format";
import { emitIntent } from "../lib/hooks";
import { STATUS_DOT } from "../lib/constants";
import { STATUSES, type Status } from "../lib/types";

export function Dashboard() {
  const { t } = useTranslation();
  const navigate = useNavigate();

  const { data: stats } = useQuery({ queryKey: ["dashboard"], queryFn: dashboardStats });
  const { data: artists = [] } = useQuery({ queryKey: ["artists"], queryFn: listArtists });
  const { data: campaigns = [] } = useQuery({ queryKey: ["campaigns"], queryFn: listCampaigns });
  const today = todayDay();
  const { data: upcomingAll = [] } = useQuery({
    queryKey: ["events", "upcoming", today],
    queryFn: () => listEvents({ from: today }),
  });
  // Cancelled gigs are not "upcoming".
  const upcoming = upcomingAll.filter((e) => e.status !== "cancelled");
  // Confirmed gigs abroad that need a visa / work permit and have no dossier.
  const { data: visas = [] } = useQuery({
    queryKey: ["event_visas", today, null],
    queryFn: () => listEventVisas({ from: today }),
  });
  const visaAlerts = visas
    .filter((v) => v.confirmed && v.needs_visa && v.dossier_id == null)
    .map((v) => ({ v, ev: upcoming.find((e) => e.id === v.event_id) }))
    .filter((x) => x.ev);

  // Counted in SQL: the email log endpoint is capped at 300 rows, so the rate
  // drifted once more mail went out, and it counted opens on failed sends.
  const sent = stats?.emails_sent ?? 0;
  const opened = stats?.emails_opened ?? 0;
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
    venue: "#ff5a1f",
    curator: "#f2b880",
    lineup: "#6f7782",
    major: "#4a4744",
    other: "#a9a49b",
  };

  const months = t("agenda.months").split("_");
  // Gig fees are budget revenue lines linked to their event (counted once,
  // server side); the net also takes the Meta Ads spend into account.
  const revenue = stats?.revenue_actual ?? 0;
  const net = stats?.net ?? 0;

  // Tasks: progress, plus the next open ones with a due date.
  const { data: tasks = [] } = useQuery({ queryKey: ["tasks"], queryFn: listTasks });
  const tasksTotal = stats?.tasks_total ?? 0;
  const tasksDone = stats?.tasks_done ?? 0;
  const nextTasks = tasks
    .filter((tk) => !tk.done && tk.due_date)
    .sort((a, b) => (a.due_date! < b.due_date! ? -1 : 1))
    .slice(0, 4);

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
        kicker={t("dashboard.kicker")}
        title={t("dashboard.title")}
        actions={
          <>
            <span className="badge mr-1 border border-border py-1.5 text-fg-subtle">
              <span className="h-1.5 w-1.5 rounded-full bg-accent animate-livepulse" />
              {months[new Date().getMonth()].slice(0, 3)} {new Date().getFullYear()}
            </span>
            <button className="btn-outline" onClick={() => navigate("/emails")}>
              <Mail size={15} />
              {t("artists.write_email")}
            </button>
            <button
              className="btn-primary"
              onClick={() => {
                navigate("/agenda");
                // Parked until Agenda mounts, then opens the new-event editor.
                emitIntent("new:event");
              }}
            >
              <Plus size={15} />
              {t("agenda.new_event")}
            </button>
          </>
        }
      />

      {visaAlerts.length > 0 && (
        <div className="space-y-1.5 border-b border-border bg-warn/10 px-8 py-3 text-sm">
          {visaAlerts.map(({ v, ev }) => (
            <div key={v.event_id} className="flex flex-wrap items-center gap-x-3 gap-y-1">
              <AlertTriangle size={14} className="shrink-0 text-warn" />
              <span className="font-semibold">
                {t("dashboard.visa_alert", {
                  title: ev!.title,
                  date: formatDate(ev!.date),
                  country: v.country_name ?? v.country_code,
                })}
              </span>
              <Link to={`/visa?event=${v.event_id}`} className="kicker text-accent hover:underline">
                {t("actions.visa_cta")} →
              </Link>
            </div>
          ))}
        </div>
      )}

      {/* Stat band */}
      <div className="grid grid-cols-2 border-b border-border sm:grid-cols-3 xl:grid-cols-6">
        {stat.map((s, i) => (
          <div
            key={i}
            className="border-b border-r border-border px-6 py-6 last:border-r-0 xl:border-b-0"
          >
            <div className="eyebrow">
              {s.label}
            </div>
            <div
              className={`mt-2 font-condensed text-[56px] tabular ${
                s.hot ? "text-accent-2" : "text-fg"
              }`}
            >
              {s.value}
              {s.unit && <span className="text-[24px] text-fg-subtle">{s.unit}</span>}
            </div>
          </div>
        ))}
      </div>

      <div className="grid grid-cols-1 lg:grid-cols-[1fr_360px] 2xl:grid-cols-[1fr_460px]">
        {/* Left: pipeline + upcoming */}
        <div className="border-b border-border p-8 lg:border-b-0 lg:border-r">
          <div className="mb-5 flex items-baseline justify-between">
            <div className="kicker">{t("dashboard.by_status")}</div>
            <Link to="/contacts" className="kicker hover:text-accent">
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
          <div className="mt-8 border-t border-border pt-6">
            <div className="mb-4 flex items-baseline justify-between">
              <div className="kicker">{t("dashboard.upcoming")}</div>
              <Link to="/agenda" className="kicker hover:text-accent">
                {t("dashboard.view_agenda")} →
              </Link>
            </div>
            {upcoming.length === 0 ? (
              <p className="text-sm text-fg-subtle">{t("dashboard.no_upcoming")}</p>
            ) : (
              <div>
                {upcoming.slice(0, 6).map((ev) => {
                  const d = parseDay(ev.date) ?? new Date();
                  const artist = artists.find((a) => a.id === ev.artist_id);
                  return (
                    <div
                      key={ev.id}
                      className="grid grid-cols-[64px_1fr_auto] items-center gap-4 border-b border-border py-3 transition-colors hover:bg-muted/50"
                    >
                      <div className="tabular">
                        <span className="font-condensed text-[26px]">
                          {String(d.getDate()).padStart(2, "0")}
                        </span>
                        <span className="text-2xs text-fg-faint">
                          {" "}
                          {months[d.getMonth()]?.slice(0, 3).toUpperCase()}
                        </span>
                      </div>
                      <div className="min-w-0">
                        <div className="truncate text-sm font-bold">{ev.title}</div>
                        <div className="truncate eyebrow text-[10.5px]">
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
                    style={{ width: `${(v / catTotal) * 100}%`, background: CAT_COLOR[k] ?? "#a9a49b" }}
                  />
                ))}
              </div>
              <div className="space-y-2.5">
                {cats.map(([k, v]) => (
                  <div key={k} className="flex items-center gap-2.5 text-sm">
                    <span className="h-2.5 w-2.5" style={{ background: CAT_COLOR[k] ?? "#a9a49b" }} />
                    <Link to={`/contacts?category=${k}`} className="text-fg-subtle hover:text-accent">
                      {t(`category.${k}`, k)}
                    </Link>
                    <span className="ml-auto font-bold tabular">{v}</span>
                  </div>
                ))}
              </div>
            </>
          )}

          {/* Budget */}
          <div className="mt-8 border-t border-border pt-6">
            <div className="kicker mb-4">{t("dashboard.budget")}</div>
            <div className="border border-accent p-5">
              <div className="eyebrow">
                {t("dashboard.net")}
              </div>
              <div className="mt-1 text-[38px] font-condensed leading-none tabular text-accent-2">
                {euro(net)}
              </div>
              <div className="mt-3 space-y-1 eyebrow text-[10.5px]">
                <div>
                  {t("budget.min")} – {t("budget.max")}:{" "}
                  <span className="font-bold text-fg">
                    {euro(stats?.budget_min ?? 0)} – {euro(stats?.budget_max ?? 0)}
                  </span>
                </div>
                <div>
                  {t("dashboard.revenue")}:{" "}
                  <span className="font-bold text-fg">{euro(revenue)}</span>
                </div>
                {(stats?.ad_spend ?? 0) > 0 && (
                  <div>
                    {t("budget.ad_spend")}:{" "}
                    <span className="font-bold text-fg">{euro(stats?.ad_spend ?? 0)}</span>
                  </div>
                )}
              </div>
            </div>
          </div>

          {/* Tasks */}
          <div className="mt-8 border-t border-border pt-6">
            <div className="mb-4 flex items-baseline justify-between">
              <div className="kicker">{t("dashboard.tasks")}</div>
              <Link to="/agenda?tab=planning" className="kicker hover:text-accent">
                {t("dashboard.view_planning")} →
              </Link>
            </div>
            {tasksTotal === 0 ? (
              <p className="text-sm text-fg-subtle">{t("dashboard.no_tasks")}</p>
            ) : (
              <>
                <div className="flex items-baseline justify-between text-sm">
                  <span className="font-condensed text-[32px] leading-none tabular">
                    {tasksDone}
                    <span className="text-[18px] text-fg-subtle">/{tasksTotal}</span>
                  </span>
                  {(stats?.tasks_overdue ?? 0) > 0 && (
                    <span className="eyebrow text-[10.5px] text-danger">
                      {t("dashboard.tasks_overdue", { count: stats?.tasks_overdue ?? 0 })}
                    </span>
                  )}
                </div>
                <div className="mt-2 h-2 bg-muted">
                  <div
                    className="h-full bg-accent"
                    style={{ width: `${(tasksDone / Math.max(1, tasksTotal)) * 100}%` }}
                  />
                </div>
                {nextTasks.length > 0 && (
                  <ul className="mt-3 space-y-1.5 text-xs">
                    {nextTasks.map((tk) => (
                      <li key={tk.id} className="flex items-baseline gap-2">
                        <span
                          className={`shrink-0 tabular ${
                            (daysUntil(tk.due_date) ?? 1) < 0 ? "text-danger" : "text-fg-subtle"
                          }`}
                        >
                          {formatDate(tk.due_date)}
                        </span>
                        <span className="truncate">{tk.title.split("\n")[0]}</span>
                      </li>
                    ))}
                  </ul>
                )}
              </>
            )}
          </div>
          <AudienceTile />
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
      <Link
        to={`/contacts?status=${statusKey}`}
        className="truncate eyebrow text-[10.5px] hover:text-accent"
      >
        {label}
      </Link>
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
