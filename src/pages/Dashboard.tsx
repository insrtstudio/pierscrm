import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { Link } from "react-router-dom";
import {
  Users,
  Mail,
  CheckCircle2,
  CalendarClock,
  TrendingUp,
  Wallet,
} from "lucide-react";
import {
  ResponsiveContainer,
  PieChart,
  Pie,
  Cell,
  Tooltip,
} from "recharts";
import { dashboardStats, getSetting } from "../lib/api";
import { PageHeader } from "../components/Layout";
import { STATUS_DOT } from "../lib/constants";
import { STATUSES, type Status } from "../lib/types";
import { useTranslation as useT } from "react-i18next";

function StatCard({
  icon: Icon,
  label,
  value,
  sub,
  accent,
}: {
  icon: any;
  label: string;
  value: string | number;
  sub?: string;
  accent?: string;
}) {
  return (
    <div className="card p-5">
      <div className="flex items-center gap-2 text-fg-subtle">
        <Icon size={16} className={accent} />
        <span className="text-xs font-medium">{label}</span>
      </div>
      <div className="mt-2 text-2xl font-semibold tracking-tight">{value}</div>
      {sub && <div className="mt-0.5 text-xs text-fg-subtle">{sub}</div>}
    </div>
  );
}

function euro(n: number) {
  return new Intl.NumberFormat("fr-FR", {
    style: "currency",
    currency: "EUR",
    maximumFractionDigits: 0,
  }).format(n);
}

export function Dashboard() {
  const { t } = useTranslation();
  const { data: stats } = useQuery({
    queryKey: ["dashboard"],
    queryFn: dashboardStats,
  });
  const { data: targetDate } = useQuery({
    queryKey: ["setting", "target_date"],
    queryFn: () => getSetting("target_date"),
  });

  const daysUntil = (() => {
    if (!targetDate) return null;
    const d = new Date(targetDate);
    if (isNaN(d.getTime())) return null;
    const diff = Math.ceil((d.getTime() - Date.now()) / 86400000);
    return diff;
  })();

  const pipelineData = STATUSES.map((s) => ({
    key: s,
    label: t(`status.${s}`),
    value: stats?.by_status[s] ?? 0,
  })).filter((d) => d.value > 0);

  const maxPipeline = Math.max(1, ...pipelineData.map((d) => d.value));

  const categoryData = Object.entries(stats?.by_category ?? {}).map(
    ([k, v]) => ({ name: t(`category.${k}`, k), value: v })
  );
  const CAT_COLORS = ["#635bff", "#10b981", "#f59e0b", "#f43f5e", "#0ea5e9"];

  return (
    <div>
      <PageHeader title={t("dashboard.title")} subtitle={t("dashboard.subtitle")} />
      <div className="space-y-6 px-8 pb-10">
        {/* Stat row */}
        <div className="grid grid-cols-2 gap-4 lg:grid-cols-4">
          <StatCard
            icon={CalendarClock}
            label={t("dashboard.days_until")}
            value={daysUntil !== null ? Math.max(0, daysUntil) : "—"}
            sub={
              targetDate
                ? new Date(targetDate).toLocaleDateString()
                : t("dashboard.set_date")
            }
            accent="text-accent"
          />
          <StatCard
            icon={Users}
            label={t("dashboard.contacts")}
            value={stats?.total_contacts ?? 0}
            accent="text-blue-500"
          />
          <StatCard
            icon={Mail}
            label={t("dashboard.emails_sent")}
            value={stats?.emails_sent ?? 0}
            accent="text-indigo-500"
          />
          <StatCard
            icon={CheckCircle2}
            label={t("dashboard.confirmed")}
            value={stats?.by_status["confirmed"] ?? 0}
            accent="text-emerald-500"
          />
        </div>

        <div className="grid grid-cols-1 gap-6 lg:grid-cols-3">
          {/* Pipeline */}
          <div className="card p-5 lg:col-span-2">
            <div className="mb-4 flex items-center justify-between">
              <h3 className="text-sm font-semibold">{t("dashboard.by_status")}</h3>
              <Link to="/contacts" className="text-xs text-accent hover:underline">
                {t("nav.contacts")} →
              </Link>
            </div>
            {pipelineData.length === 0 ? (
              <EmptyHint />
            ) : (
              <div className="space-y-2.5">
                {pipelineData.map((d) => (
                  <div key={d.key} className="flex items-center gap-3">
                    <div className="w-28 shrink-0 text-xs text-fg-subtle">
                      {d.label}
                    </div>
                    <div className="h-6 flex-1 overflow-hidden rounded-md bg-muted">
                      <div
                        className="flex h-full items-center justify-end rounded-md px-2 text-[11px] font-medium text-white"
                        style={{
                          width: `${(d.value / maxPipeline) * 100}%`,
                          backgroundColor: STATUS_DOT[d.key as Status],
                          minWidth: 24,
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
            <h3 className="mb-2 text-sm font-semibold">
              {t("dashboard.by_category")}
            </h3>
            {categoryData.length === 0 ? (
              <EmptyHint />
            ) : (
              <div className="h-52">
                <ResponsiveContainer width="100%" height="100%">
                  <PieChart>
                    <Pie
                      data={categoryData}
                      dataKey="value"
                      nameKey="name"
                      innerRadius={48}
                      outerRadius={78}
                      paddingAngle={2}
                      stroke="none"
                    >
                      {categoryData.map((_, i) => (
                        <Cell key={i} fill={CAT_COLORS[i % CAT_COLORS.length]} />
                      ))}
                    </Pie>
                    <Tooltip
                      contentStyle={{
                        background: "rgb(var(--surface))",
                        border: "1px solid rgb(var(--border))",
                        borderRadius: 10,
                        fontSize: 12,
                      }}
                    />
                  </PieChart>
                </ResponsiveContainer>
              </div>
            )}
            <div className="mt-2 space-y-1">
              {categoryData.map((d, i) => (
                <div key={d.name} className="flex items-center gap-2 text-xs">
                  <span
                    className="h-2.5 w-2.5 rounded-full"
                    style={{ background: CAT_COLORS[i % CAT_COLORS.length] }}
                  />
                  <span className="text-fg-subtle">{d.name}</span>
                  <span className="ml-auto font-medium">{d.value}</span>
                </div>
              ))}
            </div>
          </div>
        </div>

        {/* Budget + tasks */}
        <div className="grid grid-cols-1 gap-6 lg:grid-cols-3">
          <div className="card p-5 lg:col-span-2">
            <div className="mb-4 flex items-center gap-2">
              <Wallet size={16} className="text-fg-subtle" />
              <h3 className="text-sm font-semibold">{t("dashboard.budget")}</h3>
            </div>
            <div className="grid grid-cols-3 gap-4">
              <MiniStat
                label={t("budget.min")}
                value={euro(stats?.budget_min ?? 0)}
              />
              <MiniStat
                label={t("budget.max")}
                value={euro(stats?.budget_max ?? 0)}
              />
              <MiniStat
                label={t("dashboard.revenue")}
                value={euro(stats?.revenue_actual ?? 0)}
                accent="text-emerald-500"
              />
            </div>
            <div className="mt-4 flex items-center gap-2 rounded-lg bg-muted px-4 py-3">
              <TrendingUp size={16} className="text-accent" />
              <span className="text-xs text-fg-subtle">{t("dashboard.net")}</span>
              <span className="ml-auto text-sm font-semibold">
                {euro(
                  (stats?.revenue_actual ?? 0) - (stats?.budget_actual ?? 0)
                )}
              </span>
            </div>
          </div>

          <div className="card p-5">
            <h3 className="mb-3 text-sm font-semibold">{t("dashboard.tasks")}</h3>
            <TasksProgress
              done={stats?.tasks_done ?? 0}
              total={stats?.tasks_total ?? 0}
            />
          </div>
        </div>
      </div>
    </div>
  );
}

function MiniStat({
  label,
  value,
  accent,
}: {
  label: string;
  value: string;
  accent?: string;
}) {
  return (
    <div className="rounded-lg border border-border p-3">
      <div className="text-xs text-fg-subtle">{label}</div>
      <div className={`mt-1 text-base font-semibold ${accent ?? ""}`}>{value}</div>
    </div>
  );
}

function TasksProgress({ done, total }: { done: number; total: number }) {
  const pct = total ? Math.round((done / total) * 100) : 0;
  return (
    <div>
      <div className="flex items-end justify-between">
        <span className="text-2xl font-semibold">{pct}%</span>
        <span className="text-xs text-fg-subtle">
          {done}/{total}
        </span>
      </div>
      <div className="mt-2 h-2.5 overflow-hidden rounded-full bg-muted">
        <div
          className="h-full rounded-full bg-emerald-500 transition-all"
          style={{ width: `${pct}%` }}
        />
      </div>
    </div>
  );
}

function EmptyHint() {
  const { t } = useT();
  return (
    <div className="flex h-40 items-center justify-center text-sm text-fg-subtle">
      {t("common.empty")}
    </div>
  );
}
