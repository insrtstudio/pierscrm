import { useMemo } from "react";
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { Link, useNavigate } from "react-router-dom";
import {
  Users,
  Mail,
  CheckCircle2,
  CalendarDays,
  Music2,
  Megaphone,
  TrendingUp,
  Wallet,
  MapPin,
} from "lucide-react";
import { ResponsiveContainer, PieChart, Pie, Cell, Tooltip } from "recharts";
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

  const openRate = useMemo(() => {
    const sent = emails.filter((e) => e.status === "sent").length;
    const opened = emails.filter((e) => e.opened_at).length;
    return sent ? Math.round((opened / sent) * 100) : 0;
  }, [emails]);

  const activeCampaigns = campaigns.filter((c) => c.status === "active").length;

  const pipelineData = STATUSES.map((s) => ({
    key: s,
    label: t(`status.${s}`),
    value: stats?.by_status[s] ?? 0,
  })).filter((d) => d.value > 0);
  const maxPipeline = Math.max(1, ...pipelineData.map((d) => d.value));

  const categoryData = Object.entries(stats?.by_category ?? {}).map(([k, v]) => ({
    name: t(`category.${k}`, k),
    value: v,
  }));
  const CAT_COLORS = ["#5850ec", "#10b981", "#f59e0b", "#f43f5e", "#0ea5e9"];

  const months = t("agenda.months").split("_");

  return (
    <div>
      <PageHeader title={t("dashboard.title")} subtitle={t("dashboard.subtitle")} />
      <div className="space-y-6 px-8 py-6">
        {/* Stat row */}
        <div className="grid grid-cols-2 gap-4 lg:grid-cols-3 xl:grid-cols-6">
          <StatCard icon={Music2} label={t("dashboard.artists")} value={artists.length} tone="text-violet-500" />
          <StatCard icon={Users} label={t("dashboard.contacts")} value={stats?.total_contacts ?? 0} tone="text-blue-500" />
          <StatCard icon={CheckCircle2} label={t("dashboard.confirmed")} value={stats?.by_status["confirmed"] ?? 0} tone="text-emerald-500" />
          <StatCard icon={CalendarDays} label={t("dashboard.upcoming_events")} value={upcoming.length} tone="text-accent" />
          <StatCard icon={Megaphone} label={t("dashboard.active_campaigns")} value={activeCampaigns} tone="text-pink-500" />
          <StatCard icon={Mail} label={t("dashboard.open_rate")} value={`${openRate}%`} sub={`${stats?.emails_sent ?? 0} ${t("dashboard.emails_sent").toLowerCase()}`} tone="text-indigo-500" />
        </div>

        <div className="grid grid-cols-1 gap-6 lg:grid-cols-3">
          {/* Pipeline */}
          <div className="card p-5 lg:col-span-2">
            <div className="mb-4 flex items-center justify-between">
              <h3 className="text-sm font-semibold">{t("dashboard.by_status")}</h3>
              <Link to="/contacts" className="text-xs font-medium text-accent hover:underline">
                {t("nav.contacts")} →
              </Link>
            </div>
            {pipelineData.length === 0 ? (
              <EmptyState icon={Users} title={t("common.empty")} />
            ) : (
              <div className="space-y-2.5">
                {pipelineData.map((d) => (
                  <div key={d.key} className="flex items-center gap-3">
                    <div className="w-28 shrink-0 text-xs text-fg-subtle">{d.label}</div>
                    <div className="h-6 flex-1 overflow-hidden rounded-md bg-muted">
                      <div
                        className="flex h-full items-center justify-end rounded-md px-2 text-2xs font-semibold text-white"
                        style={{
                          width: `${(d.value / maxPipeline) * 100}%`,
                          backgroundColor: STATUS_DOT[d.key as Status],
                          minWidth: 26,
                        }}
                      >
                        {d.value}
                      </div>
                    </div>
                  </div>
                ))}
              </div>
            )}
          </div>

          {/* Category donut */}
          <div className="card p-5">
            <h3 className="mb-2 text-sm font-semibold">{t("dashboard.by_category")}</h3>
            {categoryData.length === 0 ? (
              <EmptyState icon={Users} title={t("common.empty")} />
            ) : (
              <>
                <div className="h-44">
                  <ResponsiveContainer width="100%" height="100%">
                    <PieChart>
                      <Pie data={categoryData} dataKey="value" nameKey="name" innerRadius={44} outerRadius={72} paddingAngle={2} stroke="none">
                        {categoryData.map((_, i) => (
                          <Cell key={i} fill={CAT_COLORS[i % CAT_COLORS.length]} />
                        ))}
                      </Pie>
                      <Tooltip
                        contentStyle={{
                          background: "rgb(var(--elevated))",
                          border: "1px solid rgb(var(--border))",
                          borderRadius: 10,
                          fontSize: 12,
                        }}
                      />
                    </PieChart>
                  </ResponsiveContainer>
                </div>
                <div className="mt-2 space-y-1">
                  {categoryData.map((d, i) => (
                    <div key={d.name} className="flex items-center gap-2 text-xs">
                      <span className="h-2.5 w-2.5 rounded-full" style={{ background: CAT_COLORS[i % CAT_COLORS.length] }} />
                      <span className="text-fg-subtle">{d.name}</span>
                      <span className="ml-auto font-medium tabular">{d.value}</span>
                    </div>
                  ))}
                </div>
              </>
            )}
          </div>
        </div>

        {/* Upcoming events + budget */}
        <div className="grid grid-cols-1 gap-6 lg:grid-cols-3">
          <div className="card p-5 lg:col-span-2">
            <div className="mb-4 flex items-center justify-between">
              <h3 className="text-sm font-semibold">{t("dashboard.upcoming")}</h3>
              <Link to="/agenda" className="text-xs font-medium text-accent hover:underline">
                {t("dashboard.view_agenda")} →
              </Link>
            </div>
            {upcoming.length === 0 ? (
              <EmptyState
                icon={CalendarDays}
                title={t("dashboard.no_upcoming")}
                action={
                  <button className="btn-outline" onClick={() => navigate("/agenda")}>
                    {t("nav.agenda")}
                  </button>
                }
              />
            ) : (
              <div className="divide-y divide-border">
                {upcoming.slice(0, 6).map((ev) => {
                  const artist = artists.find((a) => a.id === ev.artist_id);
                  return (
                    <div key={ev.id} className="flex items-center gap-3 py-2.5">
                      <div className="flex h-11 w-11 shrink-0 flex-col items-center justify-center rounded-lg bg-muted text-center">
                        <span className="text-sm font-semibold leading-none tabular">
                          {new Date(ev.date).getDate()}
                        </span>
                        <span className="text-2xs text-fg-subtle">
                          {months[new Date(ev.date).getMonth()]?.slice(0, 3)}
                        </span>
                      </div>
                      <div className="min-w-0 flex-1">
                        <div className="truncate text-sm font-medium">{ev.title}</div>
                        <div className="flex items-center gap-2 text-xs text-fg-subtle">
                          {artist && <span>{artist.name}</span>}
                          {ev.venue && (
                            <span className="inline-flex items-center gap-1">
                              <MapPin size={11} /> {ev.venue}
                            </span>
                          )}
                        </div>
                      </div>
                      {ev.start_time && (
                        <span className="text-xs tabular text-fg-subtle">{ev.start_time}</span>
                      )}
                    </div>
                  );
                })}
              </div>
            )}
          </div>

          <div className="card p-5">
            <div className="mb-4 flex items-center gap-2">
              <Wallet size={16} className="text-fg-subtle" />
              <h3 className="text-sm font-semibold">{t("dashboard.budget")}</h3>
            </div>
            <div className="space-y-3">
              <Row label={t("dashboard.budget")} value={`${euro(stats?.budget_min ?? 0)} – ${euro(stats?.budget_max ?? 0)}`} />
              <Row label={t("dashboard.revenue")} value={euro(stats?.revenue_actual ?? 0)} accent="text-emerald-500" />
              <div className="flex items-center gap-2 rounded-lg bg-muted px-3 py-2.5">
                <TrendingUp size={15} className="text-accent" />
                <span className="text-xs text-fg-subtle">{t("dashboard.net")}</span>
                <span className="ml-auto text-sm font-semibold tabular">
                  {euro((stats?.revenue_actual ?? 0) - (stats?.budget_actual ?? 0))}
                </span>
              </div>
            </div>
          </div>
        </div>
      </div>
    </div>
  );
}

function StatCard({
  icon: Icon,
  label,
  value,
  sub,
  tone,
}: {
  icon: any;
  label: string;
  value: string | number;
  sub?: string;
  tone?: string;
}) {
  return (
    <div className="card p-4 transition-shadow hover:shadow-card">
      <div className="flex items-center gap-2 text-fg-subtle">
        <Icon size={15} className={tone} />
        <span className="text-xs font-medium">{label}</span>
      </div>
      <div className="mt-2 text-[26px] font-semibold leading-none tracking-tight tabular">{value}</div>
      {sub && <div className="mt-1 text-2xs text-fg-subtle">{sub}</div>}
    </div>
  );
}

function Row({ label, value, accent }: { label: string; value: string; accent?: string }) {
  return (
    <div className="flex items-center justify-between rounded-lg border border-border px-3 py-2.5">
      <span className="text-xs text-fg-subtle">{label}</span>
      <span className={`text-sm font-semibold tabular ${accent ?? ""}`}>{value}</span>
    </div>
  );
}
