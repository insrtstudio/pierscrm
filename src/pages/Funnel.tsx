import { useMemo, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import type { TFunction } from "i18next";
import { FlaskConical, Lightbulb, RefreshCw } from "lucide-react";
import clsx from "clsx";
import { getSetting, setSetting } from "../lib/api";
import {
  DIGEST_EMAIL_SETTING,
  STAGES,
  funnelDigest,
  funnelReport,
  keepVariant,
  type FunnelQuery,
  type FunnelReport,
  type FunnelRow,
  type Suggestion,
  type VariantGroup,
} from "../lib/funnel";
import { EmptyState, Loader } from "../components/Layout";
import { errorMessage, useConfirm, useToast } from "../components/ui";
import { euro } from "../lib/format";

const BREAKDOWNS = [
  "by_campaign",
  "by_city",
  "by_country",
  "by_category",
  "by_step",
  "by_template",
  "by_weekday",
  "by_hour",
] as const;
type Breakdown = (typeof BREAKDOWNS)[number];

const pct = (r: number | null | undefined) => (r == null ? "–" : `${Math.round(r * 100)} %`);

function daysAgo(n: number) {
  const d = new Date();
  d.setDate(d.getDate() - n);
  return d.toISOString().slice(0, 10);
}

// Prospect funnel: sent → opened → listened → replied → positive → offer →
// confirmed → collected, with breakdowns, subject tests and the weekly digest.
export function Funnel() {
  const { t } = useTranslation();
  const [period, setPeriod] = useState<"30" | "90" | "365" | "all">("90");
  const [q, setQ] = useState<FunnelQuery>({});
  const [breakdown, setBreakdown] = useState<Breakdown>("by_campaign");
  const query: FunnelQuery = useMemo(
    () => ({ ...q, from: period === "all" ? null : daysAgo(Number(period)) }),
    [q, period]
  );
  const { data: rep, isLoading } = useQuery({
    queryKey: ["funnel", query],
    queryFn: () => funnelReport(query),
  });
  const opts = rep?.options;
  const setF = (k: keyof FunnelQuery, v: string) =>
    setQ((x) => ({ ...x, [k]: v === "" ? null : k === "campaign_id" ? Number(v) : v }));

  return (
    <div className="px-page pb-10 pt-6 space-y-6">
      {/* Filters */}
      <div className="flex flex-wrap items-center gap-2">
        <select className="input w-auto py-1.5" value={period} onChange={(e) => setPeriod(e.target.value as typeof period)}>
          <option value="30">{t("funnel.period_30")}</option>
          <option value="90">{t("funnel.period_90")}</option>
          <option value="365">{t("funnel.period_365")}</option>
          <option value="all">{t("funnel.period_all")}</option>
        </select>
        <select className="input w-auto py-1.5" value={q.kind ?? ""} onChange={(e) => setF("kind", e.target.value)}>
          <option value="">{t("funnel.all_kinds")}</option>
          <option value="residency">{t("funnel.kind_residency")}</option>
          <option value="standard">{t("funnel.kind_standard")}</option>
        </select>
        <select className="input w-auto py-1.5" value={q.campaign_id ?? ""} onChange={(e) => setF("campaign_id", e.target.value)}>
          <option value="">{t("funnel.all_campaigns")}</option>
          {opts?.campaigns.map(([id, name]) => (
            <option key={id} value={id}>{name || `#${id}`}</option>
          ))}
        </select>
        <select className="input w-auto py-1.5" value={q.category ?? ""} onChange={(e) => setF("category", e.target.value)}>
          <option value="">{t("category.all")}</option>
          {opts?.categories.map((c) => (
            <option key={c} value={c}>{catLabel(t, c)}</option>
          ))}
        </select>
        <select className="input w-auto py-1.5" value={q.country ?? ""} onChange={(e) => setF("country", e.target.value)}>
          <option value="">{t("funnel.all_countries")}</option>
          {opts?.countries.map((c) => <option key={c} value={c}>{c}</option>)}
        </select>
        <select className="input w-auto py-1.5" value={q.city ?? ""} onChange={(e) => setF("city", e.target.value)}>
          <option value="">{t("funnel.all_cities")}</option>
          {opts?.cities.map((c) => <option key={c} value={c}>{c}</option>)}
        </select>
      </div>

      {isLoading || !rep ? (
        <Loader />
      ) : (
        <>
          <div className="grid gap-6 xl:grid-cols-[minmax(0,3fr)_minmax(0,2fr)]">
            <FunnelBars rep={rep} />
            <DigestCard />
          </div>
          <Variants groups={rep.variants} />
          <div className="card p-5">
            <div className="mb-4 flex flex-wrap items-center gap-3">
              <div className="kicker">{t("funnel.breakdown")}</div>
              <select className="input ml-auto w-auto py-1" value={breakdown} onChange={(e) => setBreakdown(e.target.value as Breakdown)}>
                {BREAKDOWNS.map((b) => (
                  <option key={b} value={b}>{t(`funnel.${b}`)}</option>
                ))}
              </select>
            </div>
            <BreakdownTable rows={rep[breakdown]} kind={breakdown} />
          </div>
        </>
      )}
    </div>
  );
}

function catLabel(t: TFunction, c: string) {
  return c ? t(`category.${c}`, { defaultValue: c }) : t("funnel.no_category");
}

function FunnelBars({ rep }: { rep: FunnelReport }) {
  const { t } = useTranslation();
  const max = Math.max(rep.total.sent, 1);
  const conv = Object.fromEntries(rep.conversions.map((c) => [c.to, c]));
  return (
    <div className="card p-5">
      <div className="kicker mb-4">{t("funnel.title")}</div>
      {rep.total.sent === 0 ? (
        <p className="text-sm text-fg-subtle">{t("funnel.empty")}</p>
      ) : (
        <div className="grid grid-cols-[110px_1fr_56px_64px] items-center gap-x-3 gap-y-2 text-sm">
          {STAGES.map((s) => {
            const v = rep.total[s];
            const c = conv[s];
            return (
              <div key={s} className="contents">
                <div className="text-fg-subtle">{t(`funnel.stage.${s}`)}</div>
                <div className="h-2.5 rounded-full bg-muted">
                  <div
                    className={clsx("h-2.5 rounded-full", s === "confirmed" || s === "collected" ? "bg-accent-2" : "bg-accent")}
                    style={{ width: `${Math.max((v / max) * 100, v > 0 ? 1.5 : 0)}%` }}
                  />
                </div>
                <div className="text-right font-semibold tabular">{v}</div>
                <div className="text-right text-2xs text-fg-faint tabular" title={c ? t("funnel.from_stage", { stage: t(`funnel.stage.${c.from}`) }) : undefined}>
                  {c ? pct(c.rate) : ""}
                </div>
              </div>
            );
          })}
        </div>
      )}
      <div className="mt-5 grid grid-cols-2 gap-4 border-t border-border pt-4 sm:grid-cols-4">
        <Mini label={t("funnel.median_reply")} value={rep.median_reply_hours == null ? "–" : t("funnel.hours", { n: Math.round(rep.median_reply_hours) })} />
        <Mini label={t("funnel.median_confirm")} value={rep.median_reply_to_confirmed_days == null ? "–" : t("funnel.days", { n: Math.round(rep.median_reply_to_confirmed_days) })} />
        <Mini label={t("funnel.fee_collected")} value={euro(rep.total.fee_collected)} />
        <Mini label={t("funnel.listened_no_reply")} value={String(rep.listened_no_reply)} />
      </div>
      <p className="mt-3 text-2xs text-fg-faint">{t("funnel.proxy_note")}</p>
    </div>
  );
}

function Mini({ label, value }: { label: string; value: string }) {
  return (
    <div>
      <div className="eyebrow text-[10.5px]">{label}</div>
      <div className="mt-1 font-condensed text-[26px] tabular">{value}</div>
    </div>
  );
}

function rowLabel(t: TFunction, kind: Breakdown, r: FunnelRow) {
  switch (kind) {
    case "by_weekday":
      return t(`funnel.weekday.${r.label}`);
    case "by_hour":
      return `${r.label.padStart(2, "0")}h`;
    case "by_step":
      return t("residency.step", { n: r.label });
    case "by_category":
      return catLabel(t, r.key);
    default:
      return r.label || "–";
  }
}

function BreakdownTable({ rows, kind }: { rows: FunnelRow[]; kind: Breakdown }) {
  const { t } = useTranslation();
  if (rows.length === 0) return <p className="text-sm text-fg-subtle">{t("funnel.empty")}</p>;
  const best = Math.max(...rows.map((r) => r.reply_rate ?? 0), 0.0001);
  return (
    <div className="overflow-x-auto">
      <table className="w-full text-sm">
        <thead>
          <tr className="border-b border-border text-left text-2xs uppercase tracking-wide text-fg-faint">
            <th className="py-2 pr-3 font-semibold" />
            {STAGES.map((s) => (
              <th key={s} className="px-2 py-2 text-right font-semibold">{t(`funnel.stage.${s}`)}</th>
            ))}
            <th className="py-2 pl-3 text-right font-semibold">{t("funnel.reply_rate")}</th>
          </tr>
        </thead>
        <tbody>
          {rows.map((r) => (
            <tr key={r.key} className="border-b border-border last:border-b-0">
              <td className="max-w-[220px] truncate py-2 pr-3">
                {rowLabel(t, kind, r)}
                {r.kind && <span className="ml-2 text-2xs text-fg-faint">{t(`funnel.kind_${r.kind}`, { defaultValue: r.kind })}</span>}
              </td>
              {STAGES.map((s) => (
                <td key={s} className="px-2 py-2 text-right tabular">{r.stages[s] || <span className="text-fg-faint">0</span>}</td>
              ))}
              <td className="py-2 pl-3">
                <div className="flex items-center justify-end gap-2">
                  <div className="h-1.5 w-16 rounded-full bg-muted">
                    <div className="h-1.5 rounded-full bg-accent" style={{ width: `${((r.reply_rate ?? 0) / best) * 100}%` }} />
                  </div>
                  <span className="w-10 text-right tabular">{pct(r.reply_rate)}</span>
                </div>
              </td>
            </tr>
          ))}
        </tbody>
      </table>
      {(kind === "by_weekday" || kind === "by_hour") && (
        <p className="mt-2 text-2xs text-fg-faint">{t("funnel.per_email_note")}</p>
      )}
    </div>
  );
}

function Variants({ groups }: { groups: VariantGroup[] }) {
  const { t } = useTranslation();
  const qc = useQueryClient();
  const toast = useToast();
  const confirm = useConfirm();
  const keep = useMutation({
    mutationFn: ({ g, label }: { g: VariantGroup; label: string }) => keepVariant(g, label),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ["funnel"] });
      qc.invalidateQueries({ queryKey: ["templates"] });
      qc.invalidateQueries({ queryKey: ["residency"] });
      toast(t("funnel.kept"), "ok");
    },
    onError: (e) => toast(errorMessage(e), "error"),
  });
  return (
    <div className="card p-5">
      <div className="kicker mb-4">{t("funnel.ab_title")}</div>
      {groups.length === 0 ? (
        <EmptyState icon={FlaskConical} title={t("funnel.ab_empty")} hint={t("funnel.ab_empty_hint")} />
      ) : (
        <div className="grid gap-4 lg:grid-cols-2">
          {groups.map((g) => (
            <div key={`${g.campaign_id}-${g.template_id ?? 0}-${g.step ?? 0}`} className="panel p-4">
              <div className="flex flex-wrap items-baseline gap-2">
                <div className="font-bold">{g.campaign_name || t("funnel.bulk_send")}</div>
                <div className="text-2xs text-fg-faint">
                  {g.step != null && t("residency.step", { n: g.step })}
                  {g.template_name && ` · ${g.template_name}`}
                </div>
                <span
                  className={clsx(
                    "ml-auto rounded-full px-2 py-0.5 text-2xs font-semibold",
                    g.verdict === "significant" ? "bg-accent text-white" : "bg-muted text-fg-subtle"
                  )}
                >
                  {t(`funnel.verdict.${g.verdict}`)}
                  {g.p_value != null && g.verdict !== "insufficient" && ` · p=${g.p_value.toFixed(3)}`}
                </span>
              </div>
              <div className="mt-3 space-y-2">
                {g.arms.map((a) => (
                  <div key={a.label} className={clsx("grid grid-cols-[20px_1fr_auto] items-center gap-3 text-sm", g.best === a.label && "font-semibold")}>
                    <span className="text-center text-2xs text-fg-subtle">{a.label}</span>
                    <span className="truncate" title={a.example}>{a.example}</span>
                    <span className="tabular text-right">
                      {pct(a.reply_rate)} <span className="text-2xs font-normal text-fg-faint">({a.replied}/{a.sent})</span>
                    </span>
                  </div>
                ))}
              </div>
              {g.best && g.can_keep && g.verdict !== "insufficient" && (
                <button
                  className="btn-ghost mt-3 text-xs"
                  disabled={keep.isPending}
                  onClick={() => {
                    if (confirm(t("funnel.keep_confirm", { label: g.best })))
                      keep.mutate({ g, label: g.best! });
                  }}
                >
                  {t("funnel.keep_best", { label: g.best })}
                </button>
              )}
            </div>
          ))}
        </div>
      )}
    </div>
  );
}

export function suggestionText(t: TFunction, s: Suggestion) {
  const v = { ...s.values };
  if (v.weekday) v.weekday = t(`funnel.weekday.${v.weekday}`);
  if (v.part) v.part = t(`funnel.part.${v.part}`);
  if (s.code === "category_gap") {
    v.best = catLabel(t, v.best ?? "");
    v.worst = catLabel(t, v.worst ?? "");
    return t(v.ratio ? "funnel.sugg.category_gap" : "funnel.sugg.category_gap_zero", { ...v, defaultValue: s.text });
  }
  return t(`funnel.sugg.${s.code}`, { ...v, defaultValue: s.text });
}

function DigestCard() {
  const { t } = useTranslation();
  const qc = useQueryClient();
  const toast = useToast();
  const { data: d } = useQuery({ queryKey: ["funnel-digest"], queryFn: () => funnelDigest() });
  const { data: emailOn = false } = useQuery({
    queryKey: ["setting", DIGEST_EMAIL_SETTING],
    queryFn: async () => (await getSetting(DIGEST_EMAIL_SETTING)) === "1",
  });
  const refresh = useMutation({
    mutationFn: () => funnelDigest(true),
    onSuccess: (x) => qc.setQueryData(["funnel-digest"], x),
    onError: (e) => toast(errorMessage(e), "error"),
  });
  const toggle = useMutation({
    mutationFn: (on: boolean) => setSetting(DIGEST_EMAIL_SETTING, on ? "1" : "0"),
    onSuccess: () => qc.invalidateQueries({ queryKey: ["setting", DIGEST_EMAIL_SETTING] }),
  });
  return (
    <div className="card p-5">
      <div className="mb-3 flex items-center gap-2">
        <div className="kicker">{t("funnel.digest_title")}</div>
        <button className="btn-icon ml-auto" title={t("funnel.digest_refresh")} onClick={() => refresh.mutate()} disabled={refresh.isPending}>
          <RefreshCw size={14} className={clsx(refresh.isPending && "animate-spin")} />
        </button>
      </div>
      {!d ? (
        <Loader />
      ) : (
        <>
          <div className="text-2xs text-fg-faint">{t("funnel.digest_week", { from: d.week_start, to: d.week_end })}</div>
          <div className="mt-3 grid grid-cols-5 gap-2 text-center">
            {(["sent", "replies", "positive", "offers", "confirmed"] as const).map((k) => (
              <div key={k}>
                <div className="font-condensed text-[24px] tabular">{d.activity[k]}</div>
                <div className="text-[10px] uppercase tracking-wide text-fg-faint">{t(`funnel.activity.${k}`)}</div>
              </div>
            ))}
          </div>
          {(d.best_campaign || d.worst_campaign) && (
            <div className="mt-4 space-y-1 text-sm">
              {d.best_campaign && (
                <div>
                  <span className="text-fg-subtle">{t("funnel.best_campaign")} </span>
                  {d.best_campaign.label} ({pct(d.best_campaign.reply_rate)})
                </div>
              )}
              {d.worst_campaign && d.worst_campaign.key !== d.best_campaign?.key && (
                <div>
                  <span className="text-fg-subtle">{t("funnel.worst_campaign")} </span>
                  {d.worst_campaign.label} ({pct(d.worst_campaign.reply_rate)})
                </div>
              )}
            </div>
          )}
          <ul className="mt-4 space-y-2">
            {d.suggestions.map((s, i) => (
              <li key={i} className="flex gap-2 text-sm">
                <Lightbulb size={14} className="mt-0.5 shrink-0 text-accent" />
                <span>{suggestionText(t, s)}</span>
              </li>
            ))}
          </ul>
        </>
      )}
      <label className="mt-4 flex items-center gap-2 border-t border-border pt-3 text-xs text-fg-subtle">
        <input type="checkbox" checked={emailOn} onChange={(e) => toggle.mutate(e.target.checked)} />
        {t("funnel.digest_email")}
      </label>
      {d?.emailed_at && <div className="mt-1 text-2xs text-fg-faint">{t("funnel.digest_emailed", { at: d.emailed_at })}</div>}
    </div>
  );
}
