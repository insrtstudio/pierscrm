import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { useNavigate } from "react-router-dom";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import clsx from "clsx";
import {
  AlertTriangle,
  CheckCircle2,
  Copy,
  Gauge,
  HeartPulse,
  HelpCircle,
  Play,
  RefreshCw,
  ShieldAlert,
  ShieldCheck,
  XCircle,
} from "lucide-react";
import {
  checkDns,
  deliverabilityQuery,
  resumeSending,
  saveBudget,
  saveThresholds,
  utcToLocal,
  type BudgetConfig,
  type DnsCheck,
  type DnsStatus,
  type HealthThresholds,
} from "../lib/deliverability";
import { getSetting, setSetting } from "../lib/api";
import { errorMessage, Field, useConfirm, useToast } from "./ui";

const time = (s: string) =>
  utcToLocal(s).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });

// ---------------- Banner ----------------

/// Shown on every page while automated sending is paused by the health monitor.
export function DeliverabilityBanner() {
  const { t } = useTranslation();
  const nav = useNavigate();
  const qc = useQueryClient();
  const toast = useToast();
  const confirm = useConfirm();
  const { data } = useQuery(deliverabilityQuery);
  const pause = data?.pause;
  if (!pause) return null;
  const resume = async () => {
    if (!confirm(t("deliverability.resume_confirm"))) return;
    try {
      await resumeSending();
      qc.invalidateQueries({ queryKey: ["emails"] });
      toast(t("deliverability.resumed"), "ok");
    } catch (e) {
      toast(errorMessage(e), "error");
    }
  };
  return (
    <div className="flex items-start gap-3 border-b border-danger/30 bg-danger/10 px-4 py-2.5 text-sm">
      <ShieldAlert size={16} className="mt-0.5 shrink-0 text-danger" />
      <div className="min-w-0 flex-1">
        <div className="font-semibold text-fg">{t("deliverability.paused_title")}</div>
        <div className="text-xs text-fg-subtle">{pause.reason}</div>
        <div className="mt-0.5 text-2xs text-fg-subtle">{t("deliverability.paused_how")}</div>
      </div>
      <div className="flex shrink-0 items-center gap-2">
        <button className="btn-outline py-1.5" onClick={() => nav("/settings?tab=deliverability")}>
          {t("deliverability.see_detail")}
        </button>
        <button className="btn-primary py-1.5" onClick={resume}>
          <Play size={14} /> {t("deliverability.resume")}
        </button>
      </div>
    </div>
  );
}

// ---------------- Budget summary (Outbox) ----------------

function Meter({ value, max }: { value: number; max: number }) {
  const pct = Math.min(100, (value / Math.max(1, max)) * 100);
  return (
    <div className="h-1.5 overflow-hidden rounded-full bg-muted">
      <div
        className={clsx("h-full rounded-full", pct >= 100 ? "bg-danger" : "bg-accent")}
        style={{ width: `${pct}%` }}
      />
    </div>
  );
}

/// Today's usage of the shared budget and the warm-up ramp.
export function BudgetSummary({ withLink }: { withLink?: boolean }) {
  const { t } = useTranslation();
  const nav = useNavigate();
  const { data } = useQuery(deliverabilityQuery);
  if (!data) return null;
  const b = data.budget;
  const week = Math.floor(b.warmup_day / 7);
  return (
    <div className="space-y-3">
      <div>
        <div className="mb-1 flex justify-between text-xs">
          <span className="inline-flex items-center gap-1.5">
            <Gauge size={13} /> {t("deliverability.today")}
          </span>
          <span className="tabular">
            {b.sent_today} / {b.daily_limit}
          </span>
        </div>
        <Meter value={b.sent_today} max={b.daily_limit} />
      </div>
      <div>
        <div className="mb-1 flex justify-between text-xs">
          <span>{t("deliverability.this_hour")}</span>
          <span className="tabular">
            {b.sent_hour} / {b.config.hourly_cap}
          </span>
        </div>
        <Meter value={b.sent_hour} max={b.config.hourly_cap} />
      </div>
      <div className="text-2xs text-fg-subtle">
        {t("deliverability.ramp_line", { week: week + 1, day: b.warmup_day + 1 })}
        {b.next_at && <> · {t("deliverability.next_at", { time: time(b.next_at) })}</>}
      </div>
      <div className="flex flex-wrap gap-1">
        {b.ramp.map((v, i) => (
          <span
            key={i}
            title={t("deliverability.week_n", { n: i + 1 })}
            className={clsx(
              "rounded px-1.5 py-0.5 font-mono text-[10px] tabular",
              i === Math.min(week, b.ramp.length - 1)
                ? "bg-accent text-white"
                : i < week
                  ? "bg-muted text-fg-subtle"
                  : "border border-border text-fg-subtle"
            )}
          >
            {v}
          </span>
        ))}
      </div>
      {b.capped_domains.length > 0 && (
        <p className="text-2xs text-fg-subtle">
          {t("deliverability.capped_domains", {
            n: b.config.per_domain_daily,
            list: b.capped_domains.join(", "),
          })}
        </p>
      )}
      {withLink && (
        <button className="btn-ghost px-0 text-2xs" onClick={() => nav("/settings?tab=deliverability")}>
          {t("deliverability.open_settings")}
        </button>
      )}
    </div>
  );
}

// ---------------- Settings panel ----------------

const STATUS_ICON: Record<DnsStatus, JSX.Element> = {
  ok: <CheckCircle2 size={15} className="text-ok" />,
  warn: <AlertTriangle size={15} className="text-warn" />,
  fail: <XCircle size={15} className="text-danger" />,
  unknown: <HelpCircle size={15} className="text-fg-subtle" />,
};

function CheckRow({ c }: { c: DnsCheck }) {
  const { t } = useTranslation();
  const toast = useToast();
  const copy = (v: string) =>
    navigator.clipboard.writeText(v).then(() => toast(t("deliverability.copied"), "ok"));
  return (
    <div className="border-t border-border py-3 first:border-t-0">
      <div className="flex items-start gap-2">
        <span className="mt-0.5 shrink-0">{STATUS_ICON[c.status]}</span>
        <div className="min-w-0 flex-1">
          <div className="flex items-baseline gap-2 text-sm">
            <span className="font-mono text-[11px] uppercase tracking-wider text-fg-subtle">
              {c.key}
            </span>
            <span className="font-medium">{c.summary}</span>
            <span
              className={clsx(
                "ml-auto shrink-0 text-2xs font-semibold",
                c.status === "ok" ? "text-ok" : c.status === "unknown" ? "text-fg-subtle" : "text-danger"
              )}
            >
              {t(`deliverability.st_${c.status}`)}
            </span>
          </div>
          {c.detail && <p className="mt-1 text-xs text-fg-subtle">{c.detail}</p>}
          {c.found && (
            <pre className="mt-1.5 whitespace-pre-wrap break-all rounded bg-muted px-2 py-1 font-mono text-[11px] text-fg-subtle">
              {c.found}
            </pre>
          )}
          {c.fix && (
            <div className="mt-2 rounded-lg border border-accent/30 bg-accent/5 p-2.5 text-xs">
              <div className="mb-1 font-semibold">{t("deliverability.record_to_add")}</div>
              <div className="grid grid-cols-[auto_1fr_auto] items-center gap-x-3 gap-y-1">
                <span className="text-fg-subtle">{t("deliverability.rec_type")}</span>
                <span className="font-mono">{c.fix.rtype}</span>
                <span />
                <span className="text-fg-subtle">{t("deliverability.rec_host")}</span>
                <span className="break-all font-mono">{c.fix.host}</span>
                <button className="btn-ghost p-1" onClick={() => copy(c.fix!.host)} title={t("deliverability.copy")}>
                  <Copy size={12} />
                </button>
                <span className="text-fg-subtle">{t("deliverability.rec_value")}</span>
                <span className="break-all font-mono">{c.fix.value}</span>
                <button className="btn-ghost p-1" onClick={() => copy(c.fix!.value)} title={t("deliverability.copy")}>
                  <Copy size={12} />
                </button>
              </div>
            </div>
          )}
        </div>
      </div>
    </div>
  );
}

function DnsCard() {
  const { t } = useTranslation();
  const qc = useQueryClient();
  const toast = useToast();
  const { data } = useQuery(deliverabilityQuery);
  const run = useMutation({
    mutationFn: checkDns,
    onSuccess: () => qc.invalidateQueries({ queryKey: ["emails", "deliverability"] }),
    onError: (e) => toast(errorMessage(e), "error"),
  });
  const r = run.data ?? data?.dns ?? null;
  return (
    <div className="card p-6 xl:col-span-2">
      <div className="mb-1 flex items-center gap-2 text-sm font-semibold">
        {r?.ready ? <ShieldCheck size={15} className="text-ok" /> : <ShieldAlert size={15} />}
        {t("deliverability.dns_title")}
        <button className="btn-outline ml-auto py-1.5" onClick={() => run.mutate()} disabled={run.isPending}>
          <RefreshCw size={14} className={run.isPending ? "animate-spin" : undefined} />
          {run.isPending ? t("deliverability.checking") : t("deliverability.check")}
        </button>
      </div>
      <p className="mb-3 text-xs text-fg-subtle">{t("deliverability.dns_help")}</p>
      {r ? (
        <>
          <div className="mb-2 flex flex-wrap gap-x-4 text-xs text-fg-subtle">
            <span>
              {t("deliverability.domain")} <span className="font-mono text-fg">{r.domain}</span>
            </span>
            <span>
              {t("deliverability.provider")} <span className="text-fg">{r.provider ?? t("deliverability.provider_unknown")}</span>
            </span>
            <span>{t("deliverability.checked_at", { when: utcToLocal(r.checked_at).toLocaleString() })}</span>
          </div>
          {!r.ready && (
            <p className="mb-2 flex items-start gap-1.5 text-xs font-medium text-danger">
              <AlertTriangle size={13} className="mt-0.5 shrink-0" />
              {t("deliverability.autopilot_blocked")}
            </p>
          )}
          <div>
            {r.checks.map((c) => (
              <CheckRow key={c.key} c={c} />
            ))}
          </div>
        </>
      ) : (
        <p className="text-xs text-fg-subtle">{t("deliverability.never_checked")}</p>
      )}
    </div>
  );
}

function NumField({
  label,
  value,
  onChange,
  min = 0,
  max,
  step,
}: {
  label: string;
  value: number;
  onChange: (n: number) => void;
  min?: number;
  max?: number;
  step?: number;
}) {
  return (
    <Field label={label}>
      <input
        type="number"
        className="input tabular"
        min={min}
        max={max}
        step={step}
        value={Number.isFinite(value) ? value : ""}
        onChange={(e) => onChange(Number(e.target.value))}
      />
    </Field>
  );
}

function BudgetCard() {
  const { t } = useTranslation();
  const qc = useQueryClient();
  const toast = useToast();
  const { data } = useQuery(deliverabilityQuery);
  const [cfg, setCfg] = useState<BudgetConfig | null>(null);
  useEffect(() => {
    if (data && !cfg) setCfg(data.budget.config);
  }, [data, cfg]);
  if (!data || !cfg) return null;
  const set = (k: keyof BudgetConfig) => (n: number) => setCfg({ ...cfg, [k]: n });
  const save = async (next: BudgetConfig) => {
    try {
      const saved = await saveBudget(next);
      setCfg(saved);
      qc.invalidateQueries({ queryKey: ["emails", "deliverability"] });
      toast(t("deliverability.saved"), "ok");
    } catch (e) {
      toast(errorMessage(e), "error");
    }
  };
  const today = new Date().toISOString().slice(0, 10);
  return (
    <div className="card p-6">
      <div className="mb-1 flex items-center gap-2 text-sm font-semibold">
        <Gauge size={15} /> {t("deliverability.budget_title")}
      </div>
      <p className="mb-4 text-xs text-fg-subtle">{t("deliverability.budget_help")}</p>
      <BudgetSummary />
      <div className="mt-5 grid grid-cols-2 gap-x-4 gap-y-3">
        <NumField label={t("deliverability.f_start")} value={cfg.start_per_day} min={1} onChange={set("start_per_day")} />
        <NumField label={t("deliverability.f_growth")} value={cfg.weekly_growth_pct} onChange={set("weekly_growth_pct")} />
        <NumField label={t("deliverability.f_ceiling")} value={cfg.ceiling} min={1} onChange={set("ceiling")} />
        <NumField label={t("deliverability.f_hourly")} value={cfg.hourly_cap} min={1} onChange={set("hourly_cap")} />
        <NumField label={t("deliverability.f_domain")} value={cfg.per_domain_daily} min={1} onChange={set("per_domain_daily")} />
        <div />
        <NumField label={t("deliverability.f_spacing_min")} value={cfg.spacing_min_s} onChange={set("spacing_min_s")} />
        <NumField label={t("deliverability.f_spacing_max")} value={cfg.spacing_max_s} onChange={set("spacing_max_s")} />
        <NumField label={t("deliverability.f_bulk_spacing_min")} value={cfg.bulk_spacing_min_s} onChange={set("bulk_spacing_min_s")} />
        <NumField label={t("deliverability.f_bulk_spacing_max")} value={cfg.bulk_spacing_max_s} onChange={set("bulk_spacing_max_s")} />
        <Field label={t("deliverability.f_warmup_start")}>
          <input
            type="date"
            className="input"
            value={cfg.warmup_start ?? data.budget.warmup_start}
            onChange={(e) => setCfg({ ...cfg, warmup_start: e.target.value || null })}
          />
        </Field>
        <div className="flex items-end">
          <button
            className="btn-outline w-full"
            onClick={() => save({ ...cfg, warmup_start: today })}
            title={t("deliverability.restart_hint")}
          >
            {t("deliverability.restart_ramp")}
          </button>
        </div>
      </div>
      <p className="mt-2 text-2xs text-fg-subtle">{t("deliverability.ramp_hint")}</p>
      <div className="mt-4 flex justify-end">
        <button className="btn-primary" onClick={() => save(cfg)}>
          {t("deliverability.save")}
        </button>
      </div>
    </div>
  );
}

function HealthCard() {
  const { t } = useTranslation();
  const qc = useQueryClient();
  const toast = useToast();
  const confirm = useConfirm();
  const { data } = useQuery(deliverabilityQuery);
  const [th, setTh] = useState<HealthThresholds | null>(null);
  const [unsub, setUnsub] = useState<string | null>(null);
  useEffect(() => {
    if (data && !th) setTh(data.thresholds);
  }, [data, th]);
  useEffect(() => {
    getSetting("unsubscribe_base_url").then((v) => setUnsub(v ?? ""), () => setUnsub(""));
  }, []);
  if (!data || !th) return null;
  const refresh = () => qc.invalidateQueries({ queryKey: ["emails", "deliverability"] });
  const saveTh = async () => {
    try {
      setTh(await saveThresholds(th));
      refresh();
      toast(t("deliverability.saved"), "ok");
    } catch (e) {
      toast(errorMessage(e), "error");
    }
  };
  const saveUnsub = async () => {
    const v = (unsub ?? "").trim();
    if (v && !v.startsWith("https://")) {
      toast(t("deliverability.unsub_https"), "error");
      return;
    }
    await setSetting("unsubscribe_base_url", v).catch((e) => toast(errorMessage(e), "error"));
    refresh();
  };
  const resume = async () => {
    if (!confirm(t("deliverability.resume_confirm"))) return;
    await resumeSending().catch((e) => toast(errorMessage(e), "error"));
    qc.invalidateQueries({ queryKey: ["emails"] });
  };
  const rate = (r: number, n: number, limit?: number) => (
    <span className={clsx("tabular", limit != null && r > limit && "font-semibold text-danger")}>
      {r.toLocaleString(undefined, { maximumFractionDigits: 1 })} %
      <span className="text-fg-subtle"> ({n})</span>
    </span>
  );
  return (
    <div className="card p-6">
      <div className="mb-1 flex items-center gap-2 text-sm font-semibold">
        <HeartPulse size={15} /> {t("deliverability.health_title")}
      </div>
      <p className="mb-3 text-xs text-fg-subtle">{t("deliverability.health_help")}</p>
      {data.pause ? (
        <div className="mb-3 rounded-lg border border-danger/30 bg-danger/10 p-3 text-xs">
          <div className="font-semibold text-danger">{t("deliverability.paused_title")}</div>
          <p className="mt-1 text-fg-subtle">{data.pause.reason}</p>
          <button className="btn-primary mt-2 py-1.5" onClick={resume}>
            <Play size={14} /> {t("deliverability.resume")}
          </button>
        </div>
      ) : (
        <p className="mb-3 flex items-center gap-1.5 text-xs text-ok">
          <CheckCircle2 size={13} /> {t("deliverability.running")}
        </p>
      )}
      {data.health.length === 0 ? (
        <p className="text-xs text-fg-subtle">{t("deliverability.no_sends")}</p>
      ) : (
        <table className="w-full text-xs">
          <thead className="text-left text-2xs uppercase tracking-wide text-fg-subtle">
            <tr>
              <th className="py-1 font-medium">{t("deliverability.h_sender")}</th>
              <th className="py-1 font-medium">{t("deliverability.h_sent")}</th>
              <th className="py-1 font-medium">{t("deliverability.h_bounces")}</th>
              <th className="py-1 font-medium">{t("deliverability.h_complaints")}</th>
              <th className="py-1 font-medium">{t("deliverability.h_unsubs")}</th>
              <th className="py-1 font-medium">{t("deliverability.h_replies")}</th>
            </tr>
          </thead>
          <tbody>
            {data.health.map((s) => (
              <tr key={s.sender} className="border-t border-border">
                <td className="max-w-[10rem] truncate py-1.5 font-mono text-[11px]" title={s.sender}>
                  {s.sender || "?"}
                </td>
                <td className="tabular">{s.sent}</td>
                <td>{rate(s.bounce_rate, s.bounces, th.bounce_pct)}</td>
                <td>{rate(s.complaint_rate, s.complaints, th.complaint_pct)}</td>
                <td>{rate(s.unsubscribe_rate, s.unsubscribes, th.unsubscribe_pct)}</td>
                <td>{rate(s.reply_rate, s.replies)}</td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
      <div className="mt-5 grid grid-cols-2 gap-x-4 gap-y-3">
        <NumField label={t("deliverability.t_bounce")} value={th.bounce_pct} step={0.5} onChange={(n) => setTh({ ...th, bounce_pct: n })} />
        <NumField label={t("deliverability.t_complaint")} value={th.complaint_pct} step={0.1} onChange={(n) => setTh({ ...th, complaint_pct: n })} />
        <NumField label={t("deliverability.t_unsub")} value={th.unsubscribe_pct} step={0.5} onChange={(n) => setTh({ ...th, unsubscribe_pct: n })} />
        <NumField label={t("deliverability.t_sample")} value={th.min_sample} min={5} onChange={(n) => setTh({ ...th, min_sample: n })} />
      </div>
      <p className="mt-2 text-2xs text-fg-subtle">{t("deliverability.complaints_hint")}</p>
      <div className="mt-3 flex justify-end">
        <button className="btn-primary" onClick={saveTh}>
          {t("deliverability.save")}
        </button>
      </div>
      <div className="mt-5 border-t border-border pt-4">
        <div className="mb-1 text-xs font-semibold">{t("deliverability.unsub_title")}</div>
        <p className="mb-2 text-2xs text-fg-subtle">
          {data.one_click_unsubscribe ? t("deliverability.unsub_on") : t("deliverability.unsub_off")}
        </p>
        <input
          className="input"
          placeholder="https://t.insrt.studio"
          value={unsub ?? ""}
          onChange={(e) => setUnsub(e.target.value)}
          onBlur={saveUnsub}
        />
      </div>
    </div>
  );
}

/// Settings, "Délivrabilité" tab.
export function DeliverabilityPanel() {
  return (
    <>
      <DnsCard />
      <BudgetCard />
      <HealthCard />
    </>
  );
}
