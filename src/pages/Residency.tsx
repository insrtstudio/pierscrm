import { useEffect, useMemo, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { Link, Navigate, useNavigate, useParams } from "react-router-dom";
import clsx from "clsx";
import {
  AlertTriangle,
  ArrowLeft,
  Bot,
  CheckCheck,
  Headphones,
  Eye,
  Mail,
  MessageSquare,
  Pause,
  Play,
  Plane,
  Plus,
  RefreshCw,
  Sparkles,
  Trash2,
  Wand2,
} from "lucide-react";
import { TabbedPage, Loader, ErrorState, EmptyState } from "../components/Layout";
import { Field, Modal, errorMessage, useConfirm, useToast } from "../components/ui";
import { InsrtActivityPanel } from "../components/InsrtActivity";
import {
  ASSET_TYPES,
  BOARD_COLUMNS,
  addTargets,
  buildTarget,
  createResidency,
  generateDrafts,
  getResidency,
  pressKits,
  removeTarget,
  reportNow,
  residencyBoard,
  residencyLog,
  residencyMessages,
  residencyPresets,
  residencyReports,
  saveMessage,
  saveResidency,
  setAutopilot,
  setMessageStatus,
  utcDate,
  validateAll,
  type BoardRow,
  type BuildOptions,
  type BuildResult,
  type MessageRow,
  type PressKit,
  type ResidencyConfig,
  type ResidencyView,
  type Zone,
} from "../lib/residency";

const VARS = ["name", "venue", "city", "artist", "ask", "press_link", "first_name"];

// ---------------------------------------------------------------------------
// Page
// ---------------------------------------------------------------------------

/// /campaigns/:id. Residency campaigns get the full page (pitch, target,
/// sequence, follow-up board, reports); a standard campaign goes back to the
/// campaign list, where it is managed.
export function ResidencyPage() {
  const { t } = useTranslation();
  const id = Number(useParams().id);
  const q = useQuery({
    queryKey: ["residency", id],
    queryFn: () => getResidency(id),
    enabled: Number.isFinite(id),
    refetchInterval: 60_000,
    retry: false,
  });
  if (!Number.isFinite(id)) return <Navigate to="/campaigns" replace />;
  if (q.isLoading) return <Loader />;
  if (q.error) {
    // Not a residency campaign (or gone): its home is the list.
    if (String(q.error).includes("introuvable")) return <Navigate to="/campaigns" replace />;
    return <ErrorState onRetry={() => q.refetch()} />;
  }
  const v = q.data!;
  const wrap = (el: React.ReactNode) => (
    <div className="px-page pb-12 pt-5">
      <div className="mb-3">
        <BackToCampaigns />
      </div>
      <StatusBar view={v} />
      {el}
    </div>
  );
  return (
    <TabbedPage
      kicker={t("residency.kicker")}
      title={v.campaign.name}
      subtitle={t("residency.subtitle")}
      tabs={[
        { key: "pitch", label: t("residency.tab_pitch"), render: () => wrap(<PitchTab view={v} />) },
        { key: "target", label: t("residency.tab_target"), render: () => wrap(<TargetTab view={v} />) },
        { key: "sequence", label: t("residency.tab_sequence"), render: () => wrap(<SequenceTab view={v} />) },
        { key: "follow", label: t("residency.tab_follow"), render: () => wrap(<FollowTab view={v} />) },
        { key: "reports", label: t("residency.tab_reports"), render: () => wrap(<ReportsTab view={v} />) },
      ]}
    />
  );
}

function useInvalidate(id: number) {
  const qc = useQueryClient();
  return () => {
    for (const k of [["residency", id], ["residency_board", id], ["residency_messages", id], ["residency_reports", id], ["residency_actions"], ["campaigns"]])
      qc.invalidateQueries({ queryKey: k });
  };
}

function fmtWhen(s: string | null | undefined, lang: string, t: (k: string) => string): string {
  if (s === "asap") return t("residency.asap");
  const d = utcDate(s);
  if (!d) return "";
  const today = new Date();
  const tomorrow = new Date(Date.now() + 86_400_000);
  const time = d.toLocaleTimeString(lang, { hour: "2-digit", minute: "2-digit" });
  if (d.toDateString() === today.toDateString()) return `${t("residency.today")} ${time}`;
  if (d.toDateString() === tomorrow.toDateString()) return `${t("residency.tomorrow")} ${time}`;
  return `${d.toLocaleDateString(lang, { weekday: "short", day: "numeric", month: "short" })} ${time}`;
}

/// Header strip of every tab: autopilot switch, status line, visa, AI cost.
function StatusBar({ view }: { view: ResidencyView }) {
  const { t, i18n } = useTranslation();
  const toast = useToast();
  const invalidate = useInvalidate(view.campaign.id!);
  const [confirmOpen, setConfirmOpen] = useState(false);
  const s = view.status;
  const cfg = view.residency.config;
  const toggle = useMutation({
    mutationFn: (on: boolean) => setAutopilot(view.campaign.id!, on),
    onSuccess: (_, on) => {
      invalidate();
      toast(on ? t("residency.autopilot_on_ok") : t("residency.autopilot_off_ok"), "ok");
    },
    onError: (e) => toast(errorMessage(e), "error"),
  });
  const line = s.paused_reason
    ? t("residency.status_paused", { reason: s.paused_reason })
    : s.autopilot
      ? [
          t("residency.status_on"),
          s.next_send_at
            ? t("residency.status_next", { when: fmtWhen(s.next_send_at, i18n.language, t) })
            : t("residency.status_nothing"),
          t("residency.status_today", { sent: s.sent_today, max: s.daily_max }),
        ].join(" · ")
      : t("residency.status_off");
  const windowLabel = `${cfg.window.days.map((d) => t(`residency.day_${d}`)).join(" ")} ${cfg.window.start}-${cfg.window.end}`;
  return (
    <div className="mb-6 space-y-3">
      <div className="card flex flex-wrap items-center gap-4 p-4">
        <Bot size={18} className={clsx(s.autopilot && !s.paused_reason ? "text-ok" : "text-fg-faint")} />
        <div className="min-w-0 flex-1">
          <div className="eyebrow text-fg">{t("residency.autopilot")}</div>
          <div className={clsx("mt-0.5 text-sm", s.paused_reason ? "text-danger" : "text-fg-subtle")}>{line}</div>
        </div>
        <div className="text-2xs text-fg-subtle tabular">
          {t("residency.ai_cost", { eur: view.cost.cost_eur.toFixed(2), calls: view.cost.calls })}
        </div>
        {s.autopilot && !s.paused_reason ? (
          <button className="btn-danger" onClick={() => toggle.mutate(false)} disabled={toggle.isPending}>
            <Pause size={15} /> {t("residency.pause")}
          </button>
        ) : (
          <button
            className="btn-primary"
            disabled={toggle.isPending}
            onClick={() => (s.confirmed && !s.paused_reason ? toggle.mutate(true) : setConfirmOpen(true))}
          >
            <Play size={15} /> {s.paused_reason || s.autopilot ? t("residency.resume") : t("residency.start")}
          </button>
        )}
      </div>
      {view.visas.length > 0 && (
        <div className="panel flex items-center gap-3 p-3 text-sm">
          <Plane size={15} className="text-accent-2" />
          <span className="flex-1">
            {t("residency.visa_needed", { countries: view.visas.map((v) => v.country_name).join(", ") })}
          </span>
          <Link className="btn-outline py-1.5" to="/visa">
            {t("residency.visa_cta")}
          </Link>
        </div>
      )}
      {!view.ai_configured && (
        <div className="panel p-3 text-xs text-fg-subtle">
          {t("residency.no_ai")} <Link className="underline" to="/settings?tab=integrations">{t("residency.settings_link")}</Link>
        </div>
      )}
      <Modal open={confirmOpen} onClose={() => setConfirmOpen(false)} title={t("residency.confirm_title")}>
        <div className="space-y-3 text-sm">
          <p>{t("residency.confirm_intro")}</p>
          <ul className="list-disc space-y-1 pl-5 text-fg-subtle">
            <li>{t("residency.confirm_targets", { count: view.campaign.audience_count ?? 0 })}</li>
            <li>{t("residency.confirm_steps", { count: cfg.steps.length, offsets: cfg.steps.map((x) => `J+${x.offset_days}`).join(", ") })}</li>
            <li>{t("residency.confirm_rate", { max: cfg.daily_max })}</li>
            <li>{t("residency.confirm_window", { window: windowLabel })}</li>
            <li>{t("residency.confirm_stop")}</li>
            <li>{t("residency.confirm_scheduled", { count: s.scheduled })}</li>
          </ul>
          {!s.smtp_ok && <p className="text-danger">{t("residency.confirm_no_smtp")}</p>}
          <div className="flex justify-end gap-2 pt-2">
            <button className="btn-outline" onClick={() => setConfirmOpen(false)}>
              {t("common.cancel")}
            </button>
            <button
              className="btn-primary"
              onClick={() => {
                setConfirmOpen(false);
                toggle.mutate(true);
              }}
            >
              <Play size={15} /> {t("residency.confirm_go")}
            </button>
          </div>
        </div>
      </Modal>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Pitch & assets
// ---------------------------------------------------------------------------

function useConfigForm(view: ResidencyView) {
  const toast = useToast();
  const { t } = useTranslation();
  const invalidate = useInvalidate(view.campaign.id!);
  const [cfg, setCfg] = useState<ResidencyConfig>(view.residency.config);
  const [dirty, setDirty] = useState(false);
  useEffect(() => {
    if (!dirty) setCfg(view.residency.config);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [view.residency.config]);
  const set = <K extends keyof ResidencyConfig>(k: K, v: ResidencyConfig[K]) => {
    setCfg((c) => ({ ...c, [k]: v }));
    setDirty(true);
  };
  const save = useMutation({
    mutationFn: () => saveResidency(view.campaign.id!, cfg),
    onSuccess: () => {
      setDirty(false);
      invalidate();
      toast(t("settings.saved"), "ok");
    },
    onError: (e) => toast(errorMessage(e), "error"),
  });
  return { cfg, set, setCfg: (c: ResidencyConfig) => (setCfg(c), setDirty(true)), dirty, save };
}

function SaveBar({ dirty, onSave, busy }: { dirty: boolean; onSave: () => void; busy: boolean }) {
  const { t } = useTranslation();
  return (
    <div className="sticky bottom-0 z-10 -mx-1 flex justify-end gap-2 bg-bg/90 px-1 py-3 backdrop-blur">
      {dirty && <span className="self-center text-2xs text-fg-subtle">{t("residency.unsaved")}</span>}
      <button className="btn-primary" disabled={!dirty || busy} onClick={onSave}>
        {t("common.save")}
      </button>
    </div>
  );
}

function PitchTab({ view }: { view: ResidencyView }) {
  const { t } = useTranslation();
  const toast = useToast();
  const { cfg, set, dirty, save } = useConfigForm(view);
  const [kits, setKits] = useState<PressKit[] | null>(null);
  const [loadingKits, setLoadingKits] = useState(false);
  const loadKits = async () => {
    setLoadingKits(true);
    try {
      setKits(await pressKits());
    } catch (e) {
      toast(errorMessage(e), "error");
    } finally {
      setLoadingKits(false);
    }
  };
  const setAsset = (i: number, k: string, v: string) =>
    set("assets", cfg.assets.map((a, j) => (j === i ? { ...a, [k]: v } : a)));
  return (
    <div className="grid gap-6 xl:grid-cols-[minmax(0,3fr)_minmax(0,2fr)]">
      <div className="space-y-5">
        <div className="card space-y-4 p-5">
          <div className="eyebrow text-fg">{t("residency.pitch")}</div>
          <Field label={t("residency.pitch_fr")}>
            <textarea className="input min-h-[180px]" value={cfg.pitch_fr} onChange={(e) => set("pitch_fr", e.target.value)} />
          </Field>
          <Field label={t("residency.pitch_en")}>
            <textarea className="input min-h-[150px]" value={cfg.pitch_en} onChange={(e) => set("pitch_en", e.target.value)} />
          </Field>
          <Field label={t("residency.concept")}>
            <textarea className="input" value={cfg.concept} onChange={(e) => set("concept", e.target.value)} />
          </Field>
          <div className="grid grid-cols-2 gap-4">
            <Field label={t("residency.ask")}>
              <select className="input" value={cfg.ask} onChange={(e) => set("ask", e.target.value)}>
                <option value="monthly">{t("residency.ask_monthly")}</option>
                <option value="bimonthly">{t("residency.ask_bimonthly")}</option>
              </select>
            </Field>
            <Field label={t("residency.lang")}>
              <select className="input" value={cfg.lang} onChange={(e) => set("lang", e.target.value)}>
                <option value="auto">{t("residency.lang_auto")}</option>
                <option value="fr">Français</option>
                <option value="en">English</option>
              </select>
            </Field>
          </div>
        </div>
        <ZoneCard zone={cfg.zone} onChange={(z) => set("zone", z)} />
      </div>

      <div className="space-y-5">
        <div className="card space-y-3 p-5">
          <div className="flex items-center justify-between">
            <div className="eyebrow text-fg">{t("residency.press_kit")}</div>
            <button className="btn-ghost py-1 text-xs" onClick={loadKits} disabled={loadingKits}>
              <RefreshCw size={13} className={clsx(loadingKits && "animate-spin")} /> {t("residency.load_kits")}
            </button>
          </div>
          <p className="text-xs text-fg-subtle">
            {view.insrt_configured ? t("residency.press_kit_help") : t("residency.press_kit_off")}
          </p>
          <select
            className="input"
            value={cfg.press_kit_id ?? ""}
            onChange={(e) => {
              const k = kits?.find((x) => x.id === e.target.value);
              set("press_kit_id", e.target.value || null);
              set("press_kit_title", k?.title ?? null);
            }}
          >
            <option value="">{t("residency.no_kit")}</option>
            {cfg.press_kit_id && !kits?.some((k) => k.id === cfg.press_kit_id) && (
              <option value={cfg.press_kit_id}>{cfg.press_kit_title ?? cfg.press_kit_id}</option>
            )}
            {kits?.map((k) => (
              <option key={k.id} value={k.id}>
                {k.title}
              </option>
            ))}
          </select>
        </div>

        <div className="card space-y-3 p-5">
          <div className="flex items-center justify-between">
            <div className="eyebrow text-fg">{t("residency.assets")}</div>
            <button
              className="btn-ghost py-1 text-xs"
              onClick={() => set("assets", [...cfg.assets, { type: "link", title: "", url: "", note: "" }])}
            >
              <Plus size={13} /> {t("residency.add_asset")}
            </button>
          </div>
          <p className="text-xs text-fg-subtle">{t("residency.assets_help")}</p>
          {cfg.assets.length === 0 && <p className="text-xs text-fg-faint">{t("residency.no_assets")}</p>}
          {cfg.assets.map((a, i) => (
            <div key={i} className="panel space-y-2 p-3">
              <div className="flex gap-2">
                <select className="input w-28" value={a.type} onChange={(e) => setAsset(i, "type", e.target.value)}>
                  {ASSET_TYPES.map((k) => (
                    <option key={k} value={k}>
                      {t(`residency.asset_${k}`)}
                    </option>
                  ))}
                </select>
                <input className="input" placeholder={t("residency.asset_title")} value={a.title} onChange={(e) => setAsset(i, "title", e.target.value)} />
                <button className="btn-icon text-danger" onClick={() => set("assets", cfg.assets.filter((_, j) => j !== i))}>
                  <Trash2 size={14} />
                </button>
              </div>
              <input className="input" placeholder="https://…" value={a.url} onChange={(e) => setAsset(i, "url", e.target.value)} />
              <input className="input" placeholder={t("residency.asset_note")} value={a.note} onChange={(e) => setAsset(i, "note", e.target.value)} />
            </div>
          ))}
        </div>
      </div>
      <div className="xl:col-span-2">
        <SaveBar dirty={dirty} busy={save.isPending} onSave={() => save.mutate()} />
      </div>
    </div>
  );
}

function ZoneCard({ zone, onChange }: { zone: Zone; onChange: (z: Zone) => void }) {
  const { t } = useTranslation();
  const { data: presets = [] } = useQuery({ queryKey: ["residency_presets"], queryFn: residencyPresets });
  const toggle = (k: string) =>
    onChange({ ...zone, presets: zone.presets.includes(k) ? zone.presets.filter((p) => p !== k) : [...zone.presets, k] });
  return (
    <div className="card space-y-3 p-5">
      <div className="eyebrow text-fg">{t("residency.zone")}</div>
      <div className="flex flex-wrap gap-2">
        {presets.map((p) => (
          <button
            key={p.key}
            className={clsx("badge border py-1", zone.presets.includes(p.key) ? "border-accent bg-accent text-accent-fg" : "border-border text-fg-subtle")}
            onClick={() => toggle(p.key)}
          >
            {p.label}
          </button>
        ))}
      </div>
      <div className="grid grid-cols-2 gap-4">
        <Field label={t("residency.zone_cities")}>
          <input
            className="input"
            placeholder="Lyon, Lille"
            value={zone.cities.join(", ")}
            onChange={(e) => onChange({ ...zone, cities: e.target.value.split(",").map((s) => s.trim()).filter(Boolean) })}
          />
        </Field>
        <Field label={t("residency.zone_countries")}>
          <input
            className="input"
            placeholder="NL, DE"
            value={zone.countries.join(", ")}
            onChange={(e) =>
              onChange({ ...zone, countries: e.target.value.split(",").map((s) => s.trim().toUpperCase()).filter(Boolean) })
            }
          />
        </Field>
      </div>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Target
// ---------------------------------------------------------------------------

function TargetTab({ view }: { view: ResidencyView }) {
  const { t } = useTranslation();
  const toast = useToast();
  const invalidate = useInvalidate(view.campaign.id!);
  const cfg = view.residency.config;
  const [opts, setOpts] = useState<BuildOptions>({
    capacity_min: cfg.capacity_min,
    capacity_max: cfg.capacity_max,
    include_unknown_capacity: true,
    min_events: cfg.min_events,
  });
  const [result, setResult] = useState<BuildResult | null>(null);
  const [picked, setPicked] = useState<Set<string>>(new Set());
  const build = useMutation({
    mutationFn: () => buildTarget(view.campaign.id!, opts),
    onSuccess: (r) => {
      setResult(r);
      setPicked(new Set(r.candidates.filter((c) => c.email_rank < 4).map((c) => c.key)));
    },
    onError: (e) => toast(errorMessage(e), "error"),
  });
  const add = useMutation({
    mutationFn: () =>
      addTargets(
        view.campaign.id!,
        (result?.candidates ?? [])
          .filter((c) => picked.has(c.key))
          .map((c) => ({ venue_id: c.venue_id, contact_id: c.contact_id, email: c.email, first_name: c.first_name }))
      ),
    onSuccess: (n) => {
      toast(t("residency.targets_added", { count: n }), "ok");
      setResult(null);
      invalidate();
    },
    onError: (e) => toast(errorMessage(e), "error"),
  });
  const num = (k: keyof BuildOptions) => (e: React.ChangeEvent<HTMLInputElement>) =>
    setOpts((o) => ({ ...o, [k]: e.target.value === "" ? undefined : Number(e.target.value) }));

  return (
    <div className="space-y-6">
      <div className="card p-5">
        <div className="mb-3 eyebrow text-fg">{t("residency.build_title")}</div>
        <div className="flex flex-wrap items-end gap-4">
          <Field label={t("residency.cap_min")}>
            <input type="number" className="input w-28" value={opts.capacity_min ?? ""} onChange={num("capacity_min")} />
          </Field>
          <Field label={t("residency.cap_max")}>
            <input type="number" className="input w-28" value={opts.capacity_max ?? ""} onChange={num("capacity_max")} />
          </Field>
          <Field label={t("residency.min_events")}>
            <input type="number" className="input w-28" value={opts.min_events ?? ""} onChange={num("min_events")} />
          </Field>
          <label className="flex items-center gap-2 pb-2 text-sm">
            <input
              type="checkbox"
              checked={opts.include_unknown_capacity !== false}
              onChange={(e) => setOpts((o) => ({ ...o, include_unknown_capacity: e.target.checked }))}
            />
            {t("residency.include_unknown")}
          </label>
          <button className="btn-primary" onClick={() => build.mutate()} disabled={build.isPending}>
            <Wand2 size={15} /> {build.isPending ? t("common.loading") : t("residency.build")}
          </button>
        </div>
        <p className="mt-3 text-2xs text-fg-faint">{t("residency.build_help")}</p>
      </div>

      {result && (
        <div className="card overflow-hidden">
          <div className="flex flex-wrap items-center gap-3 border-b border-border p-4">
            <div className="eyebrow text-fg">{t("residency.found", { count: result.candidates.length })}</div>
            <div className="text-2xs text-fg-subtle">
              {t("residency.excluded", {
                suppressed: result.excluded_suppressed,
                recent: result.excluded_recent,
                declined: result.excluded_declined,
                audience: result.excluded_in_audience,
                noemail: result.excluded_no_email,
              })}
            </div>
            <button className="btn-primary ml-auto" disabled={picked.size === 0 || add.isPending} onClick={() => add.mutate()}>
              <Plus size={15} /> {t("residency.add_targets", { count: picked.size })}
            </button>
          </div>
          <div className="max-h-[560px] overflow-auto">
            <table className="tbl">
              <thead>
                <tr>
                  <th>
                    <input
                      type="checkbox"
                      checked={picked.size === result.candidates.length && picked.size > 0}
                      onChange={(e) => setPicked(e.target.checked ? new Set(result.candidates.map((c) => c.key)) : new Set())}
                    />
                  </th>
                  <th>{t("residency.col_club")}</th>
                  <th>{t("residency.col_reasons")}</th>
                  <th>{t("residency.col_email")}</th>
                  <th>{t("residency.col_lang")}</th>
                </tr>
              </thead>
              <tbody>
                {result.candidates.map((c) => (
                  <tr key={c.key}>
                    <td>
                      <input
                        type="checkbox"
                        checked={picked.has(c.key)}
                        onChange={(e) => {
                          const n = new Set(picked);
                          e.target.checked ? n.add(c.key) : n.delete(c.key);
                          setPicked(n);
                        }}
                      />
                    </td>
                    <td>
                      <div className="font-medium">{c.name}</div>
                      <div className="text-2xs text-fg-subtle">
                        {c.city} · {c.country}
                        {c.source === "pipeline" && ` · ${t("residency.from_pipeline")}`}
                      </div>
                    </td>
                    <td className="text-xs">
                      <div className="flex flex-wrap gap-1">
                        <span className={clsx("badge", c.capacity_unknown ? "bg-warn/15 text-warn" : "bg-muted text-fg-subtle")}>
                          {c.capacity_unknown ? t("residency.cap_unknown") : t("residency.cap", { n: c.capacity })}
                        </span>
                        {c.events > 0 && <span className="badge bg-muted text-fg-subtle">{t("residency.events", { n: c.events })}</span>}
                      </div>
                      {c.artists.length > 0 && (
                        <div className="mt-1 text-2xs text-fg-subtle">
                          {t("residency.played", { artists: c.artists.join(", ") })}
                        </div>
                      )}
                    </td>
                    <td className="text-xs">
                      {c.email}
                      {c.email_rank >= 3 && (
                        <span className="badge ml-1 bg-warn/15 text-warn">{t(`residency.email_rank_${c.email_rank}`)}</span>
                      )}
                    </td>
                    <td className="text-xs uppercase">{c.lang}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </div>
      )}

      <AudienceList view={view} />
    </div>
  );
}

function AudienceList({ view }: { view: ResidencyView }) {
  const { t } = useTranslation();
  const confirm = useConfirm();
  const invalidate = useInvalidate(view.campaign.id!);
  const { data: rows = [], isLoading } = useQuery({
    queryKey: ["residency_board", view.campaign.id],
    queryFn: () => residencyBoard(view.campaign.id!),
  });
  const remove = useMutation({
    mutationFn: (cid: number) => removeTarget(view.campaign.id!, cid),
    onSuccess: invalidate,
  });
  return (
    <div className="card overflow-hidden">
      <div className="border-b border-border p-4 eyebrow text-fg">{t("residency.audience", { count: rows.length })}</div>
      {isLoading ? (
        <Loader />
      ) : rows.length === 0 ? (
        <EmptyState icon={Mail} title={t("residency.audience_empty")} hint={t("residency.audience_empty_hint")} />
      ) : (
        <table className="tbl">
          <thead>
            <tr>
              <th>{t("residency.col_club")}</th>
              <th>{t("residency.col_email")}</th>
              <th>{t("residency.col_state")}</th>
              <th />
            </tr>
          </thead>
          <tbody>
            {rows.map((r) => (
              <tr key={r.contact_id}>
                <td>
                  <span className="font-medium">{r.name}</span>
                  <div className="text-2xs text-fg-subtle">{r.city}</div>
                </td>
                <td className="text-xs">
                  {r.email}
                  {r.suppressed && <span className="badge ml-1 bg-danger/10 text-danger">{t("residency.suppressed")}</span>}
                </td>
                <td>
                  <span className="badge bg-muted text-fg-subtle">{t(`residency.col_${r.column}`)}</span>
                </td>
                <td className="text-right">
                  {r.steps_sent === 0 && (
                    <button
                      className="btn-icon text-danger"
                      onClick={() => confirm(t("residency.remove_confirm", { name: r.name })) && remove.mutate(r.contact_id)}
                    >
                      <Trash2 size={14} />
                    </button>
                  )}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </div>
  );
}

// ---------------------------------------------------------------------------
// Sequence + drafts
// ---------------------------------------------------------------------------

const STATUS_STYLE: Record<string, string> = {
  draft: "bg-warn/15 text-warn",
  scheduled: "bg-chrome-dark/10 text-chrome-dark",
  sent: "bg-ok/10 text-ok",
  skipped: "bg-fg-faint/10 text-fg-faint",
  failed: "bg-danger/10 text-danger",
};

function SequenceTab({ view }: { view: ResidencyView }) {
  const { t, i18n } = useTranslation();
  const toast = useToast();
  const invalidate = useInvalidate(view.campaign.id!);
  const { cfg, set, dirty, save } = useConfigForm(view);
  const [enOpen, setEnOpen] = useState<number | null>(null);
  const [editing, setEditing] = useState<MessageRow | null>(null);
  const [filter, setFilter] = useState<string>("all");
  const { data: messages = [] } = useQuery({
    queryKey: ["residency_messages", view.campaign.id],
    queryFn: () => residencyMessages(view.campaign.id!),
  });
  const gen = useMutation({
    mutationFn: () => generateDrafts(view.campaign.id!),
    onSuccess: (r) => {
      invalidate();
      toast(t("residency.generated", { count: r.created, ai: r.with_ai, links: r.press_links }), r.errors.length ? "error" : "ok");
      if (r.errors.length) console.warn(r.errors);
    },
    onError: (e) => toast(errorMessage(e), "error"),
  });
  const valAll = useMutation({
    mutationFn: () => validateAll(view.campaign.id!),
    onSuccess: (r) => {
      invalidate();
      toast(t("residency.validated", { count: r.scheduled, left: r.unresolved }), "ok");
    },
  });
  const setStep = (i: number, k: string, v: string | number | string[] | null) =>
    set("steps", cfg.steps.map((s, j) => (j === i ? { ...s, [k]: v } : s)));
  const toggleDay = (d: number) => {
    const days = cfg.window.days.includes(d) ? cfg.window.days.filter((x) => x !== d) : [...cfg.window.days, d].sort();
    set("window", { ...cfg.window, days });
  };
  const shown = messages.filter((m) => filter === "all" || m.status === filter);
  const counts = useMemo(() => {
    const c: Record<string, number> = {};
    for (const m of messages) c[m.status] = (c[m.status] ?? 0) + 1;
    return c;
  }, [messages]);

  return (
    <div className="space-y-6">
      <div className="grid gap-6 xl:grid-cols-[minmax(0,3fr)_minmax(0,2fr)]">
        <div className="space-y-4">
          {cfg.steps.map((s, i) => (
            <div key={i} className="card space-y-3 p-5">
              <div className="flex items-center gap-3">
                <div className="eyebrow text-fg">{t("residency.step", { n: i + 1 })}</div>
                <label className="flex items-center gap-2 text-xs text-fg-subtle">
                  J+
                  <input
                    type="number"
                    className="input w-20 py-1"
                    value={s.offset_days}
                    disabled={i === 0}
                    onChange={(e) => setStep(i, "offset_days", Number(e.target.value))}
                  />
                </label>
                {i > 0 && (
                  <button className="btn-icon ml-auto text-danger" onClick={() => set("steps", cfg.steps.filter((_, j) => j !== i))}>
                    <Trash2 size={14} />
                  </button>
                )}
              </div>
              {i === 0 && <p className="text-2xs text-fg-faint">{t("residency.step1_help")}</p>}
              <Field label={t("residency.subject_fr")}>
                <input className="input" value={s.subject} onChange={(e) => setStep(i, "subject", e.target.value)} />
                <SubjectVariants values={s.subject_variants ?? []} onChange={(v) => setStep(i, "subject_variants", v)} />
              </Field>
              <Field label={t("residency.body_fr")}>
                <textarea className="input min-h-[150px] font-mono text-[13px]" value={s.body} onChange={(e) => setStep(i, "body", e.target.value)} />
              </Field>
              <button className="btn-ghost py-1 text-xs" onClick={() => setEnOpen(enOpen === i ? null : i)}>
                {t("residency.english_version")}
              </button>
              {enOpen === i && (
                <>
                  <Field label={t("residency.subject_en")}>
                    <input className="input" value={s.subject_en} onChange={(e) => setStep(i, "subject_en", e.target.value)} />
                    <SubjectVariants values={s.subject_variants_en ?? []} onChange={(v) => setStep(i, "subject_variants_en", v)} />
                  </Field>
                  <Field label={t("residency.body_en")}>
                    <textarea className="input min-h-[150px] font-mono text-[13px]" value={s.body_en} onChange={(e) => setStep(i, "body_en", e.target.value)} />
                  </Field>
                </>
              )}
              <Field label={t("residency.extra_asset")}>
                <select className="input" value={s.extra_asset ?? ""} onChange={(e) => setStep(i, "extra_asset", e.target.value || null)}>
                  <option value="">{t("residency.none")}</option>
                  {cfg.assets.map((a) => (
                    <option key={a.title} value={a.title}>
                      {a.title}
                    </option>
                  ))}
                </select>
              </Field>
            </div>
          ))}
          <button
            className="btn-outline"
            onClick={() =>
              set("steps", [
                ...cfg.steps,
                { offset_days: (cfg.steps[cfg.steps.length - 1]?.offset_days ?? 0) + 7, subject: "", body: "", subject_en: "", body_en: "" },
              ])
            }
          >
            <Plus size={15} /> {t("residency.add_step")}
          </button>
          <p className="text-2xs text-fg-faint">{t("residency.vars", { vars: VARS.map((v) => `{{${v}}}`).join(" ") })}</p>
        </div>
        <div className="space-y-4">
          <div className="card space-y-4 p-5">
            <div className="eyebrow text-fg">{t("residency.window")}</div>
            <div className="flex flex-wrap gap-1.5">
              {[1, 2, 3, 4, 5, 6, 7].map((d) => (
                <button
                  key={d}
                  className={clsx("badge border py-1", cfg.window.days.includes(d) ? "border-fg bg-fg text-bg" : "border-border text-fg-subtle")}
                  onClick={() => toggleDay(d)}
                >
                  {t(`residency.day_${d}`)}
                </button>
              ))}
            </div>
            <div className="grid grid-cols-2 gap-4">
              <Field label={t("residency.start")}>
                <input type="time" className="input" value={cfg.window.start} onChange={(e) => set("window", { ...cfg.window, start: e.target.value })} />
              </Field>
              <Field label={t("residency.end")}>
                <input type="time" className="input" value={cfg.window.end} onChange={(e) => set("window", { ...cfg.window, end: e.target.value })} />
              </Field>
            </div>
            <p className="text-2xs text-fg-faint">{t("residency.window_help")}</p>
            <Field label={t("residency.daily_max")}>
              <input type="number" className="input w-28" min={1} max={50} value={cfg.daily_max} onChange={(e) => set("daily_max", Number(e.target.value))} />
            </Field>
            <p className="text-2xs text-fg-faint">{t("residency.daily_max_help", { quota: view.status.quota_remaining })}</p>
          </div>
        </div>
      </div>
      <SaveBar dirty={dirty} busy={save.isPending} onSave={() => save.mutate()} />

      <div className="card overflow-hidden">
        <div className="flex flex-wrap items-center gap-2 border-b border-border p-4">
          <div className="eyebrow mr-2 text-fg">{t("residency.drafts")}</div>
          <div className="segmented">
            {["all", "draft", "scheduled", "sent", "skipped", "failed"].map((f) => (
              <button key={f} className="segmented-item" data-active={filter === f} onClick={() => setFilter(f)}>
                {f === "all" ? t("residency.all") : t(`residency.msg_${f}`)} {f === "all" ? messages.length : counts[f] ?? 0}
              </button>
            ))}
          </div>
          <div className="ml-auto flex gap-2">
            <button className="btn-outline" onClick={() => gen.mutate()} disabled={gen.isPending}>
              <Sparkles size={15} /> {gen.isPending ? t("residency.generating") : t("residency.generate")}
            </button>
            <button className="btn-primary" onClick={() => valAll.mutate()} disabled={valAll.isPending || !counts.draft}>
              <CheckCheck size={15} /> {t("residency.validate_all")}
            </button>
          </div>
        </div>
        {shown.length === 0 ? (
          <EmptyState icon={Mail} title={t("residency.no_drafts")} hint={t("residency.no_drafts_hint")} />
        ) : (
          <table className="tbl">
            <thead>
              <tr>
                <th>{t("residency.col_club")}</th>
                <th>{t("residency.col_step")}</th>
                <th>{t("residency.col_subject")}</th>
                <th>{t("residency.col_status")}</th>
                <th>{t("residency.col_when")}</th>
              </tr>
            </thead>
            <tbody>
              {shown.map((m) => (
                <tr key={m.id} className="cursor-pointer hover:bg-muted/40" onClick={() => setEditing(m)}>
                  <td>
                    <div className="font-medium">{m.name}</div>
                    <div className="text-2xs text-fg-subtle">{m.email}</div>
                  </td>
                  <td className="text-xs tabular">
                    {m.step} · {m.lang.toUpperCase()}
                  </td>
                  <td className="max-w-[360px] truncate text-xs">
                    {m.subject}
                    {m.unresolved.length > 0 && (
                      <div className="text-2xs text-danger">{t("emails.unresolved", { vars: m.unresolved.map((v) => `{{${v}}}`).join(", ") })}</div>
                    )}
                    {m.note && m.unresolved.length === 0 && <div className="truncate text-2xs text-fg-faint">{m.note}</div>}
                  </td>
                  <td>
                    <span className={clsx("badge", STATUS_STYLE[m.status])}>{t(`residency.msg_${m.status}`)}</span>
                  </td>
                  <td className="text-xs text-fg-subtle">
                    {m.status === "sent"
                      ? fmtWhen(m.sent_at, i18n.language, t)
                      : m.status === "scheduled"
                        ? fmtWhen(m.scheduled_for ?? "asap", i18n.language, t)
                        : ""}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </div>
      {editing && <MessageModal msg={editing} campaignId={view.campaign.id!} onClose={() => setEditing(null)} />}
    </div>
  );
}

function MessageModal({ msg, campaignId, onClose }: { msg: MessageRow; campaignId: number; onClose: () => void }) {
  const { t } = useTranslation();
  const toast = useToast();
  const invalidate = useInvalidate(campaignId);
  const [subject, setSubject] = useState(msg.subject);
  const [body, setBody] = useState(msg.body);
  const [busy, setBusy] = useState(false);
  const missing = [...new Set([...(subject + "\n" + body).matchAll(/\{\{\s*(\w+)\s*\}\}/g)].map((m) => m[1]))];
  const sent = msg.status === "sent";
  const run = async (fn: () => Promise<unknown>, ok?: string) => {
    setBusy(true);
    try {
      await fn();
      invalidate();
      if (ok) toast(ok, "ok");
      onClose();
    } catch (e) {
      toast(errorMessage(e), "error");
    } finally {
      setBusy(false);
    }
  };
  return (
    <Modal open onClose={onClose} title={`${msg.name} · ${t("residency.step", { n: msg.step })}`} width="max-w-2xl">
      <div className="space-y-3">
        <div className="flex flex-wrap gap-2 text-2xs text-fg-subtle">
          <span>{msg.email}</span>
          {msg.generated_by && <span>· {msg.generated_by}</span>}
          {msg.press_url && (
            <a className="underline" href={msg.press_url} target="_blank" rel="noreferrer">
              · {t("residency.press_link")}
            </a>
          )}
        </div>
        <Field label={t("emails.subject")}>
          <input className="input" value={subject} disabled={sent} onChange={(e) => setSubject(e.target.value)} />
        </Field>
        <Field label={t("emails.body")}>
          <textarea className="input min-h-[260px] font-mono text-[13px]" value={body} disabled={sent} onChange={(e) => setBody(e.target.value)} />
        </Field>
        {missing.length > 0 && (
          <p className="text-2xs font-medium text-danger">{t("emails.unresolved", { vars: missing.map((v) => `{{${v}}}`).join(", ") })}</p>
        )}
        {!sent && (
          <div className="flex flex-wrap justify-end gap-2 pt-1">
            {msg.step === 1 && (
              <button
                className="btn-ghost mr-auto"
                disabled={busy}
                onClick={() => run(() => generateDrafts(campaignId, [msg.contact_id], true), t("residency.regenerated"))}
              >
                <RefreshCw size={14} /> {t("residency.regenerate")}
              </button>
            )}
            {msg.status !== "skipped" && (
              <button className="btn-ghost" disabled={busy} onClick={() => run(() => setMessageStatus(msg.id, "skipped"))}>
                {t("residency.skip")}
              </button>
            )}
            <button className="btn-outline" disabled={busy} onClick={() => run(() => saveMessage(msg.id, subject, body), t("settings.saved"))}>
              {t("common.save")}
            </button>
            {msg.status === "scheduled" ? (
              <button className="btn-outline" disabled={busy} onClick={() => run(() => setMessageStatus(msg.id, "draft"))}>
                {t("residency.unvalidate")}
              </button>
            ) : (
              <button
                className="btn-primary"
                disabled={busy || missing.length > 0}
                onClick={() =>
                  run(async () => {
                    await saveMessage(msg.id, subject, body);
                    await setMessageStatus(msg.id, "scheduled");
                  }, t("residency.validated_one"))
                }
              >
                <CheckCheck size={15} /> {t("residency.validate")}
              </button>
            )}
          </div>
        )}
      </div>
    </Modal>
  );
}

// ---------------------------------------------------------------------------
// Suivi (board)
// ---------------------------------------------------------------------------

function FollowTab({ view }: { view: ResidencyView }) {
  const { t, i18n } = useTranslation();
  const { data: rows = [], isLoading } = useQuery({
    queryKey: ["residency_board", view.campaign.id],
    queryFn: () => residencyBoard(view.campaign.id!),
    refetchInterval: 60_000,
  });
  if (isLoading) return <Loader />;
  const by = (c: string) => rows.filter((r) => r.column === c);
  return (
    <div className="-mx-1 flex gap-3 overflow-x-auto px-1 pb-4">
      {BOARD_COLUMNS.map((c) => (
        <div key={c} className="w-[250px] shrink-0">
          <div className="eyebrow mb-2 flex items-center gap-2 border-b border-border pb-2 text-fg">
            {t(`residency.col_${c}`)}
            <span className="tabular text-fg-faint">{by(c).length}</span>
          </div>
          <div className="space-y-2">
            {by(c).map((r) => (
              <BoardCard key={r.contact_id} r={r} lang={i18n.language} />
            ))}
          </div>
        </div>
      ))}
    </div>
  );
}

function BoardCard({ r, lang }: { r: BoardRow; lang: string }) {
  const { t } = useTranslation();
  // insrt EPK activity in detail (tracks, time listened, feedback), on demand.
  const [open, setOpen] = useState(false);
  const active = r.views > 0 || r.listen_seconds > 0 || r.feedback > 0;
  return (
    <div className="card p-3 text-xs">
      <div className="truncate text-sm font-medium">{r.name}</div>
      <div className="truncate text-2xs text-fg-subtle">
        {r.city} {r.lang && `· ${r.lang.toUpperCase()}`}
      </div>
      <div className="mt-2 flex flex-wrap gap-1">
        {r.steps_sent > 0 && <span className="badge bg-muted text-fg-subtle"><Mail size={10} /> {r.steps_sent}</span>}
        {r.opened && <span className="badge bg-muted text-fg-subtle"><Eye size={10} /> {t("residency.opened")}</span>}
        {r.views > 0 && <span className="badge bg-accent-soft text-accent-ink">EPK {r.views}</span>}
        {r.listen_seconds > 0 && (
          <span className="badge bg-accent-soft text-accent-ink">
            <Headphones size={10} /> {Math.round(r.listen_seconds / 60)} min
          </span>
        )}
        {r.feedback > 0 && <span className="badge bg-accent-soft text-accent-ink"><MessageSquare size={10} /> {r.feedback}</span>}
        {r.intent && <IntentBadge intent={r.intent} />}
      </div>
      {active && (
        <button className="mt-1.5 text-2xs text-fg-subtle underline" onClick={() => setOpen((v) => !v)}>
          {open ? t("insrt.hide_detail") : t("insrt.show_detail")}
        </button>
      )}
      {open && <InsrtActivityPanel contactId={r.contact_id} compact />}
      {r.summary && <p className="mt-2 line-clamp-3 text-2xs text-fg-subtle">{r.summary}</p>}
      {r.next_at && (
        <p className="mt-2 text-2xs text-fg-faint">
          {t("residency.next_step", { n: r.next_step ?? "", when: fmtWhen(r.next_at, lang, t) })}
        </p>
      )}
    </div>
  );
}

export function IntentBadge({ intent }: { intent: string }) {
  const { t } = useTranslation();
  const style =
    intent === "interested" || intent === "question"
      ? "bg-ok/10 text-ok"
      : intent === "not_interested" || intent === "unsubscribe"
        ? "bg-danger/10 text-danger"
        : "bg-warn/15 text-warn";
  return <span className={clsx("badge", style)}>{t(`residency.intent_${intent}`, intent)}</span>;
}

// ---------------------------------------------------------------------------
// Reports
// ---------------------------------------------------------------------------

function ReportsTab({ view }: { view: ResidencyView }) {
  const { t, i18n } = useTranslation();
  const toast = useToast();
  const invalidate = useInvalidate(view.campaign.id!);
  const { data: reports = [] } = useQuery({
    queryKey: ["residency_reports", view.campaign.id],
    queryFn: () => residencyReports(view.campaign.id!),
  });
  const { data: log = [] } = useQuery({
    queryKey: ["residency_log", view.campaign.id],
    queryFn: () => residencyLog(view.campaign.id!, 80),
    refetchInterval: 60_000,
  });
  const [day, setDay] = useState<string | null>(null);
  const now = useMutation({
    mutationFn: () => reportNow(view.campaign.id!),
    onSuccess: (ok) => {
      invalidate();
      toast(ok ? t("residency.report_built") : t("residency.report_empty"), ok ? "ok" : "error");
    },
    onError: (e) => toast(errorMessage(e), "error"),
  });
  const r = reports.find((x) => x.day === day) ?? reports[0];
  const d = r?.data;
  const stat = (label: string, y: number | undefined, tot: number | undefined) => (
    <div className="panel p-3">
      <div className="font-condensed text-2xl tabular">{tot ?? 0}</div>
      <div className="eyebrow mt-1 text-[10px]">{label}</div>
      <div className="mt-1 text-2xs text-fg-subtle">{t("residency.yesterday_n", { n: y ?? 0 })}</div>
    </div>
  );
  return (
    <div className="grid gap-6 xl:grid-cols-[minmax(0,3fr)_minmax(0,2fr)]">
      <div className="space-y-4">
        <div className="flex flex-wrap items-center gap-2">
          <select className="input w-56" value={r?.day ?? ""} onChange={(e) => setDay(e.target.value)}>
            {reports.length === 0 && <option value="">{t("residency.no_reports")}</option>}
            {reports.map((x) => (
              <option key={x.day} value={x.day}>
                {new Date(x.day + "T12:00:00").toLocaleDateString(i18n.language, { weekday: "long", day: "numeric", month: "long" })}
              </option>
            ))}
          </select>
          <button className="btn-outline" onClick={() => now.mutate()} disabled={now.isPending}>
            <RefreshCw size={14} className={clsx(now.isPending && "animate-spin")} /> {t("residency.report_now")}
          </button>
        </div>
        {!r || !d ? (
          <div className="card">
            <EmptyState icon={Bot} title={t("residency.no_reports")} hint={t("residency.no_reports_hint")} />
          </div>
        ) : (
          <>
            {r.synthesis && (
              <div className="card whitespace-pre-line p-5 text-sm leading-relaxed">{r.synthesis}</div>
            )}
            <div className="grid grid-cols-2 gap-3 md:grid-cols-4">
              {stat(t("residency.r_sent"), d.yesterday.sent, d.total.sent)}
              {stat(t("residency.r_bounced"), d.yesterday.bounced, d.total.bounced)}
              {stat(t("residency.r_opened"), d.yesterday.opened, d.total.opened)}
              {stat(t("residency.r_replies"), d.yesterday.replies, d.total.replies)}
              {stat(t("residency.r_views"), d.yesterday.views, d.total.views)}
              {stat(t("residency.r_listens"), d.yesterday.listens, d.total.listens)}
              {stat(t("residency.r_feedback"), d.yesterday.feedback, d.total.feedback)}
              {stat(t("residency.r_downloads"), d.yesterday.downloads, d.total.downloads)}
            </div>
            <div className="card p-5">
              <div className="eyebrow mb-2 text-fg">{t("residency.r_by_intent")}</div>
              <div className="flex flex-wrap gap-2">
                {Object.entries(d.by_intent).map(([k, n]) => (
                  <span key={k} className="inline-flex items-center gap-1">
                    <IntentBadge intent={k} /> <span className="text-xs tabular">{n}</span>
                  </span>
                ))}
                {Object.keys(d.by_intent).length === 0 && <span className="text-xs text-fg-faint">{t("residency.none")}</span>}
              </div>
              {d.replies_yesterday.length > 0 && (
                <ul className="mt-4 space-y-1.5 text-sm">
                  {d.replies_yesterday.map((x, i) => (
                    <li key={i} className="flex gap-2">
                      <IntentBadge intent={x.intent} />
                      <span className="font-medium">{x.name}</span>
                      <span className="text-fg-subtle">{x.summary}</span>
                    </li>
                  ))}
                </ul>
              )}
            </div>
            <div className="grid gap-4 md:grid-cols-2">
              <div className="card p-5 text-sm">
                <div className="eyebrow mb-2 text-fg">{t("residency.r_next")}</div>
                {d.next_actions.length === 0 ? (
                  <p className="text-xs text-fg-faint">{t("residency.none")}</p>
                ) : (
                  <ul className="list-disc space-y-1 pl-4">{d.next_actions.map((a, i) => <li key={i}>{a}</li>)}</ul>
                )}
              </div>
              <div className="card p-5 text-sm">
                <div className="eyebrow mb-2 text-fg">{t("residency.r_followups")}</div>
                {d.followups_today.length === 0 ? (
                  <p className="text-xs text-fg-faint">{t("residency.none")}</p>
                ) : (
                  <p>{d.followups_today.join(", ")}</p>
                )}
                <p className="mt-3 text-2xs text-fg-subtle">
                  {d.paused_reason
                    ? t("residency.status_paused", { reason: d.paused_reason })
                    : d.autopilot
                      ? t("residency.status_on")
                      : t("residency.status_off")}
                </p>
              </div>
            </div>
          </>
        )}
      </div>
      <div className="card overflow-hidden">
        <div className="border-b border-border p-4 eyebrow text-fg">{t("residency.log")}</div>
        <ul className="max-h-[640px] overflow-auto text-xs">
          {log.map((l, i) => (
            <li key={i} className="flex gap-3 border-b border-border/60 px-4 py-2">
              <span className="shrink-0 tabular text-fg-faint">{fmtWhen(l.at, i18n.language, t)}</span>
              <span className={clsx(l.level === "warn" || l.level === "pause" ? "text-danger" : l.level === "send" ? "text-ok" : "text-fg-subtle")}>
                {l.level === "pause" && <AlertTriangle size={11} className="mr-1 inline" />}
                {l.message}
              </span>
            </li>
          ))}
          {log.length === 0 && <li className="p-4 text-fg-faint">{t("residency.none")}</li>}
        </ul>
      </div>
    </div>
  );
}

// ---------------------------------------------------------------------------
// "Nouvelle campagne résidence"
// ---------------------------------------------------------------------------

export function NewResidencyModal({ onClose }: { onClose: () => void }) {
  const { t } = useTranslation();
  const toast = useToast();
  const navigate = useNavigate();
  const qc = useQueryClient();
  const { data: presets = [] } = useQuery({ queryKey: ["residency_presets"], queryFn: residencyPresets });
  const [zone, setZone] = useState<Zone>({ presets: ["paris"], cities: [], countries: [] });
  const [name, setName] = useState("");
  const [busy, setBusy] = useState(false);
  const label = presets.find((p) => p.key === zone.presets[0])?.label.split(" (")[0].split(" /")[0] ?? zone.cities[0] ?? "";
  const go = async () => {
    setBusy(true);
    try {
      const id = await createResidency(name.trim() || t("residency.default_name", { zone: label }), zone);
      qc.invalidateQueries({ queryKey: ["campaigns"] });
      onClose();
      navigate(`/campaigns/${id}`);
    } catch (e) {
      toast(errorMessage(e), "error");
    } finally {
      setBusy(false);
    }
  };
  return (
    <Modal open onClose={onClose} title={t("residency.new")}>
      <div className="space-y-4">
        <p className="text-sm text-fg-subtle">{t("residency.new_help")}</p>
        <div>
          <span className="label">{t("residency.zone")}</span>
          <div className="space-y-1.5">
            {presets.map((p) => (
              <label key={p.key} className="flex items-center gap-2 text-sm">
                <input
                  type="radio"
                  name="zone"
                  checked={zone.presets[0] === p.key}
                  onChange={() => setZone({ presets: [p.key], cities: [], countries: [] })}
                />
                {p.label}
              </label>
            ))}
            <label className="flex items-center gap-2 text-sm">
              <input
                type="radio"
                name="zone"
                checked={zone.presets.length === 0}
                onChange={() => setZone({ presets: [], cities: [], countries: [] })}
              />
              {t("residency.zone_other")}
            </label>
          </div>
        </div>
        {zone.presets.length === 0 && (
          <div className="grid grid-cols-2 gap-4">
            <Field label={t("residency.zone_cities")}>
              <input
                className="input"
                placeholder="Lyon, Lille"
                onChange={(e) => setZone((z) => ({ ...z, cities: e.target.value.split(",").map((s) => s.trim()).filter(Boolean) }))}
              />
            </Field>
            <Field label={t("residency.zone_countries")}>
              <input
                className="input"
                placeholder="NL, DE"
                onChange={(e) =>
                  setZone((z) => ({ ...z, countries: e.target.value.split(",").map((s) => s.trim().toUpperCase()).filter(Boolean) }))
                }
              />
            </Field>
          </div>
        )}
        <Field label={t("residency.name")}>
          <input className="input" placeholder={t("residency.default_name", { zone: label })} value={name} onChange={(e) => setName(e.target.value)} />
        </Field>
        <div className="flex justify-end gap-2">
          <button className="btn-outline" onClick={onClose}>
            {t("common.cancel")}
          </button>
          <button
            className="btn-primary"
            disabled={busy || (zone.presets.length === 0 && zone.cities.length === 0 && zone.countries.length === 0)}
            onClick={go}
          >
            <Plus size={15} /> {t("residency.create")}
          </button>
        </div>
      </div>
    </Modal>
  );
}

export function BackToCampaigns() {
  const { t } = useTranslation();
  return (
    <Link to="/campaigns" className="btn-ghost">
      <ArrowLeft size={14} /> {t("residency.back")}
    </Link>
  );
}

// Subject A/B test for a step: lines B and C next to the main subject (A).
// Each prospect gets one line from a hash of its id (commands/funnel.rs).
function SubjectVariants({ values, onChange }: { values: string[]; onChange: (v: string[]) => void }) {
  const { t } = useTranslation();
  return (
    <div className="mt-2 space-y-2">
      {values.map((v, i) => (
        <div key={i} className="flex items-center gap-2">
          <span className="w-5 text-center text-2xs font-semibold text-fg-subtle">{String.fromCharCode(66 + i)}</span>
          <input
            className="input flex-1"
            value={v}
            placeholder={t("funnel.variant_placeholder")}
            onChange={(e) => onChange(values.map((x, j) => (j === i ? e.target.value : x)))}
          />
          <button type="button" className="btn-ghost text-xs" onClick={() => onChange(values.filter((_, j) => j !== i))}>
            {t("funnel.variant_remove")}
          </button>
        </div>
      ))}
      {values.length < 2 && (
        <button type="button" className="btn-ghost py-1 text-xs" onClick={() => onChange([...values, ""])}>
          {t("funnel.variant_add", { label: String.fromCharCode(66 + values.length) })}
        </button>
      )}
    </div>
  );
}
