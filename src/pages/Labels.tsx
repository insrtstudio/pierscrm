import { useMemo, useState } from "react";
import { Link, useNavigate, useParams, useSearchParams } from "react-router-dom";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { openUrl } from "@tauri-apps/plugin-opener";
import clsx from "clsx";
import {
  AlertTriangle,
  Archive,
  ArchiveRestore,
  ArrowDown,
  ArrowLeft,
  ArrowUp,
  Check,
  Disc3,
  ExternalLink,
  Headphones,
  Pause,
  Pencil,
  Play,
  Plus,
  Send,
  Trash2,
  X,
} from "lucide-react";
import { EmptyState, ErrorState, Loader, PageHeader, Segmented, TabbedPage } from "../components/Layout";
import { Field, Modal, errorMessage, useConfirm, useSubmit, useToast } from "../components/ui";
import { deleteCampaign } from "../lib/api";
import { formatDate, formatDateTime } from "../lib/format";
import {
  pressKits,
  residencyMessages,
  saveMessage,
  setMessageStatus,
  validateAll,
  type MessageRow,
  type Step,
} from "../lib/residency";
import {
  CLOSED_STATES,
  addLabelsToDemo,
  createDemo,
  demosList,
  getDemo,
  importRadarLabels,
  labelsList,
  markSubmitted,
  openWave,
  redraftDemo,
  saveDemo,
  saveLabel,
  setDemoStatus,
  setDemoTargets,
  setLabelsArchived,
  setLabelsTier,
  setTargetOutcome,
  type DemoCard,
  type DemoView,
  type Label,
  type LabelInput,
  type Outcome,
  type TargetState,
} from "../lib/labels";

// Labels: one list of record labels, one campaign per demo. A demo page walks
// through four numbered steps (the demo, its labels, the emails to validate,
// the follow-up) and always opens on the one that needs the user.

const STATE_TONE: Record<TargetState, string> = {
  queued: "bg-fg/[0.06] text-fg-subtle",
  unreachable: "bg-warn/15 text-warn",
  to_submit: "bg-accent/15 text-accent-2",
  to_validate: "bg-accent/15 text-accent-2",
  scheduled: "bg-fg/[0.08] text-fg",
  failed: "bg-danger/15 text-danger",
  sent: "bg-fg/[0.08] text-fg",
  followed: "bg-fg/[0.08] text-fg",
  replied: "bg-ok/15 text-ok",
  declined: "bg-fg/[0.06] text-fg-subtle",
  no_answer: "bg-fg/[0.06] text-fg-subtle",
  bounced: "bg-danger/15 text-danger",
  withdrawn: "bg-fg/[0.06] text-fg-subtle",
  signed: "bg-ok text-white",
};

function StateBadge({ state }: { state: TargetState }) {
  const { t } = useTranslation();
  return <span className={clsx("badge", STATE_TONE[state])}>{t(`labels.state.${state}`)}</span>;
}

function TierDots({ tier }: { tier: number }) {
  const { t } = useTranslation();
  return (
    <span className="inline-flex items-center gap-0.5" title={t(`labels.tier_${tier}`)}>
      {[1, 2, 3].map((n) => (
        <span key={n} className={clsx("h-1.5 w-1.5 rounded-full", n <= 4 - tier ? "bg-accent" : "bg-fg/15")} />
      ))}
    </span>
  );
}

const isReachable = (l: Label) =>
  !l.archived && (l.demo_policy === "form" || (l.demo_policy === "email" && !!l.email && !l.suppressed));

// ---------------------------------------------------------------------------
// Section: demos + labels
// ---------------------------------------------------------------------------

export function LabelsSection() {
  const { t } = useTranslation();
  return (
    <TabbedPage
      title={t("nav.labels")}
      subtitle={t("labels.subtitle")}
      tabs={[
        { key: "demos", label: t("labels.tab_demos"), render: () => <DemosTab /> },
        { key: "list", label: t("labels.tab_list"), render: () => <LabelsTab /> },
      ]}
    />
  );
}

function DemosTab() {
  const { t } = useTranslation();
  const [creating, setCreating] = useState(false);
  const { data, isLoading, isError, refetch } = useQuery({ queryKey: ["label_demos"], queryFn: demosList });
  return (
    <div className="px-page py-5">
      <PageHeader
        title={t("nav.labels")}
        actions={
          <button className="btn-primary" onClick={() => setCreating(true)}>
            <Plus size={14} /> {t("labels.new_demo")}
          </button>
        }
      />
      {isLoading ? (
        <Loader />
      ) : isError ? (
        <ErrorState onRetry={refetch} />
      ) : !data?.length ? (
        <div className="mx-auto max-w-2xl">
          <EmptyState
            icon={Disc3}
            title={t("labels.empty_demos")}
            hint={t("labels.empty_demos_hint")}
            action={
              <button className="btn-primary" onClick={() => setCreating(true)}>
                <Plus size={14} /> {t("labels.new_demo")}
              </button>
            }
          />
          <ol className="card divide-y divide-border">
            {[1, 2, 3, 4].map((n) => (
              <li key={n} className="flex items-center gap-4 px-5 py-3.5">
                <span className="w-4 font-condensed text-2xl leading-none text-accent-2">{n}</span>
                <div>
                  <div className="text-sm font-medium">{t(`labels.step_${n}`)}</div>
                  <div className="text-[13px] text-fg-subtle">{t(`labels.step_${n}_hint`)}</div>
                </div>
              </li>
            ))}
          </ol>
        </div>
      ) : (
        <div className="grid gap-3 @lg:grid-cols-2 @2xl:grid-cols-3">
          {data.map((d) => (
            <DemoCardLink key={d.id} demo={d} />
          ))}
        </div>
      )}
      {creating && <NewDemoModal onClose={() => setCreating(false)} />}
    </div>
  );
}

function demoStatusKey(d: DemoCard): string {
  if (d.signed) return "signed";
  if (d.status === "done") return "done";
  if (d.status === "paused" || d.paused_reason) return "paused";
  if (d.current_wave === 0) return "draft";
  return "running";
}

function DemoCardLink({ demo: d }: { demo: DemoCard }) {
  const { t } = useTranslation();
  const key = demoStatusKey(d);
  return (
    <Link to={`/labels/${d.id}`} className="card block p-5 transition-colors hover:border-fg">
      <div className="flex items-start justify-between gap-3">
        <div className="min-w-0">
          <div className="eyebrow">{t("labels.demo_kicker")}</div>
          <div className="mt-1 truncate font-display text-xl leading-tight">{d.track}</div>
        </div>
        <span
          className={clsx(
            "badge shrink-0",
            key === "signed" ? "bg-ok text-white" : key === "running" ? "bg-ok/15 text-ok" : key === "paused" ? "bg-warn/15 text-warn" : "bg-fg/[0.06] text-fg-subtle"
          )}
        >
          {t(`labels.demo_status.${key}`)}
        </span>
      </div>
      <div className="mt-4 grid grid-cols-4 gap-2 text-center">
        {(
          [
            ["todo", d.todo],
            ["waiting", d.waiting],
            ["replied", d.replied],
            ["closed", d.closed],
          ] as const
        ).map(([k, n]) => (
          <div key={k}>
            <div className={clsx("font-condensed text-2xl leading-none tabular", k === "todo" && n > 0 && "text-accent-2")}>{n}</div>
            <div className="mt-1 font-mono text-[9.5px] uppercase tracking-wider text-fg-subtle">{t(`labels.count.${k}`)}</div>
          </div>
        ))}
      </div>
      <div className="mt-4 text-[13px] text-fg-subtle">
        {d.current_wave === 0
          ? t("labels.card_not_started", { count: d.total })
          : t("labels.card_wave", { wave: d.current_wave, queued: d.queued })}
        {d.listened > 0 && <span className="text-fg"> · {t("labels.card_listened", { count: d.listened })}</span>}
      </div>
    </Link>
  );
}

function NewDemoModal({ onClose }: { onClose: () => void }) {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const qc = useQueryClient();
  const [track, setTrack] = useState("");
  const [kit, setKit] = useState("");
  const [listenUrl, setListenUrl] = useState("");
  const { data: kits } = useQuery({ queryKey: ["press_kits"], queryFn: pressKits, retry: false });
  const [submit, busy] = useSubmit(async () => {
    const k = kits?.find((x) => x.id === kit);
    const id = await createDemo(track, k?.id, k?.title, listenUrl);
    qc.invalidateQueries({ queryKey: ["label_demos"] });
    navigate(`/labels/${id}?step=labels`);
  });
  return (
    <Modal open onClose={onClose} title={t("labels.new_demo")}>
      <div className="space-y-4">
        <Field label={t("labels.track")}>
          <input className="input" autoFocus value={track} onChange={(e) => setTrack(e.target.value)} placeholder={t("labels.track_placeholder")} />
        </Field>
        <KitFields kits={kits} kit={kit} setKit={setKit} listenUrl={listenUrl} setListenUrl={setListenUrl} />
        <div className="flex justify-end gap-2 pt-1">
          <button className="btn-ghost" onClick={onClose}>
            {t("common.cancel")}
          </button>
          <button className="btn-primary" disabled={busy || !track.trim()} onClick={() => submit()}>
            {t("labels.create_demo")}
          </button>
        </div>
      </div>
    </Modal>
  );
}

/// Where the label listens: a personal insrt page (tracked), with a plain
/// link as a fallback.
function KitFields({
  kits,
  kit,
  setKit,
  listenUrl,
  setListenUrl,
}: {
  kits?: { id: string; title: string }[];
  kit: string;
  setKit: (v: string) => void;
  listenUrl: string;
  setListenUrl: (v: string) => void;
}) {
  const { t } = useTranslation();
  return (
    <>
      <Field label={t("labels.kit")}>
        <select className="input" value={kit} onChange={(e) => setKit(e.target.value)}>
          <option value="">{t("labels.kit_none")}</option>
          {kits?.map((k) => (
            <option key={k.id} value={k.id}>
              {k.title}
            </option>
          ))}
        </select>
        <p className="mt-1.5 text-[12.5px] text-fg-subtle">{kits?.length ? t("labels.kit_hint") : t("labels.kit_missing")}</p>
      </Field>
      <Field label={`${t("labels.listen_url")} (${t("common.optional")})`}>
        <input className="input" value={listenUrl} onChange={(e) => setListenUrl(e.target.value)} placeholder="https://soundcloud.com/…/s-…" />
        <p className="mt-1.5 text-[12.5px] text-fg-subtle">{t("labels.listen_url_hint")}</p>
      </Field>
    </>
  );
}

// ---------------------------------------------------------------------------
// The list of labels
// ---------------------------------------------------------------------------

type ListFilter = "reachable" | "all" | "no_email" | "form" | "archived";

function LabelsTab() {
  const { t } = useTranslation();
  const qc = useQueryClient();
  const toast = useToast();
  const [search, setSearch] = useState("");
  const [filter, setFilter] = useState<ListFilter>("reachable");
  const [tier, setTier] = useState(0);
  const [selected, setSelected] = useState<Set<number>>(new Set());
  const [editing, setEditing] = useState<Label | "new" | null>(null);
  const [adding, setAdding] = useState(false);
  const { data, isLoading, isError, refetch } = useQuery({ queryKey: ["labels"], queryFn: labelsList });
  const refresh = () => {
    qc.invalidateQueries({ queryKey: ["labels"] });
    qc.invalidateQueries({ queryKey: ["label_demo"] });
  };
  const rows = useMemo(() => {
    const q = search.trim().toLowerCase();
    return (data ?? []).filter((l) => {
      if (filter === "archived" ? !l.archived : l.archived) return false;
      if (filter === "reachable" && !isReachable(l)) return false;
      if (filter === "no_email" && (l.demo_policy !== "email" || (!!l.email && !l.suppressed))) return false;
      if (filter === "form" && l.demo_policy !== "form") return false;
      if (tier && l.tier !== tier) return false;
      if (!q) return true;
      return [l.name, l.country, l.genres, l.email, l.notes].some((v) => v?.toLowerCase().includes(q));
    });
  }, [data, search, filter, tier]);
  const counts = useMemo(() => {
    const all = (data ?? []).filter((l) => !l.archived);
    return { all: all.length, reachable: all.filter(isReachable).length };
  }, [data]);
  const ids = [...selected].filter((id) => rows.some((r) => r.id === id));
  const bulk = useMutation({
    mutationFn: (fn: () => Promise<unknown>) => fn(),
    onSuccess: () => {
      setSelected(new Set());
      refresh();
    },
    onError: (e) => toast(errorMessage(e), "error"),
  });
  const importMut = useMutation({
    mutationFn: importRadarLabels,
    onSuccess: (n) => {
      toast(t("labels.imported", { count: n }));
      refresh();
    },
    onError: (e) => toast(errorMessage(e), "error"),
  });
  const toggle = (id: number) =>
    setSelected((s) => {
      const n = new Set(s);
      if (n.has(id)) n.delete(id);
      else n.add(id);
      return n;
    });
  return (
    <div className="py-5">
      <PageHeader
        title={t("nav.labels")}
        actions={
          <>
            <button className="btn-outline" disabled={importMut.isPending} onClick={() => importMut.mutate()} title={t("labels.import_radar_hint")}>
              {t("labels.import_radar")}
            </button>
            <button className="btn-primary" onClick={() => setEditing("new")}>
              <Plus size={14} /> {t("labels.add_label")}
            </button>
          </>
        }
      />
      <div className="flex flex-wrap items-center gap-2 px-page pb-4">
        <input className="input max-w-xs" value={search} onChange={(e) => setSearch(e.target.value)} placeholder={t("common.search")} />
        <Segmented
          items={(["reachable", "all", "no_email", "form", "archived"] as ListFilter[]).map((k) => ({
            key: k,
            label: t(`labels.filter.${k}`) + (k === "reachable" ? ` ${counts.reachable}` : k === "all" ? ` ${counts.all}` : ""),
          }))}
          value={filter}
          onChange={setFilter}
        />
        <select className="input w-auto" value={tier} onChange={(e) => setTier(Number(e.target.value))} aria-label={t("labels.tier")}>
          <option value={0}>{t("labels.tier_all")}</option>
          {[1, 2, 3].map((n) => (
            <option key={n} value={n}>
              {t(`labels.tier_${n}`)}
            </option>
          ))}
        </select>
      </div>
      {ids.length > 0 && (
        <div className="mx-page mb-3 flex flex-wrap items-center gap-2 rounded-xl border border-border bg-surface-2 px-4 py-2.5">
          <span className="text-sm">{t("labels.selected", { count: ids.length })}</span>
          <div className="flex-1" />
          <button className="btn-primary" onClick={() => setAdding(true)}>
            <Send size={13} /> {t("labels.add_to_demo")}
          </button>
          {[1, 2, 3].map((n) => (
            <button key={n} className="btn-outline" onClick={() => bulk.mutate(() => setLabelsTier(ids, n))}>
              {t(`labels.tier_${n}`)}
            </button>
          ))}
          <button className="btn-ghost" onClick={() => bulk.mutate(() => setLabelsArchived(ids, filter !== "archived"))}>
            {filter === "archived" ? <ArchiveRestore size={13} /> : <Archive size={13} />}
            {filter === "archived" ? t("labels.unarchive") : t("labels.archive")}
          </button>
        </div>
      )}
      {isLoading ? (
        <Loader />
      ) : isError ? (
        <ErrorState onRetry={refetch} />
      ) : !rows.length ? (
        <EmptyState icon={Disc3} title={t("labels.empty_list")} hint={t("labels.empty_list_hint")} />
      ) : (
        <table className="tbl">
          <thead>
            <tr>
              <th className="w-10">
                <input
                  type="checkbox"
                  aria-label={t("labels.select_all")}
                  checked={ids.length === rows.length}
                  onChange={(e) => setSelected(e.target.checked ? new Set(rows.map((r) => r.id)) : new Set())}
                />
              </th>
              <th>{t("labels.col_label")}</th>
              <th>{t("labels.col_country")}</th>
              <th>{t("labels.col_channel")}</th>
              <th>{t("labels.tier")}</th>
              <th>{t("labels.col_demos")}</th>
              <th className="w-12" />
            </tr>
          </thead>
          <tbody>
            {rows.map((l) => (
              <tr key={l.id}>
                <td>
                  <input type="checkbox" aria-label={l.name} checked={selected.has(l.id)} onChange={() => toggle(l.id)} />
                </td>
                <td>
                  <div className="flex items-center gap-1.5">
                    <span className="font-medium">{l.name}</span>
                    {l.website && (
                      <button className="text-fg-faint hover:text-fg" title={l.website} onClick={() => openUrl(l.website!).catch(() => {})}>
                        <ExternalLink size={12} />
                      </button>
                    )}
                  </div>
                  {l.genres && <div className="text-[12.5px] text-fg-subtle">{l.genres}</div>}
                </td>
                <td className="text-fg-subtle">{l.country ?? "·"}</td>
                <td>
                  <Channel label={l} />
                </td>
                <td>
                  <TierDots tier={l.tier} />
                </td>
                <td>
                  <div className="flex flex-wrap gap-1">
                    {l.demos.map((d) => (
                      <Link key={d.campaign_id} to={`/labels/${d.campaign_id}`} className={clsx("badge", STATE_TONE[d.state])} title={t(`labels.state.${d.state}`)}>
                        {d.track}
                      </Link>
                    ))}
                  </div>
                </td>
                <td>
                  <button className="btn-icon" aria-label={t("common.edit")} onClick={() => setEditing(l)}>
                    <Pencil size={14} />
                  </button>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
      {editing && <LabelModal label={editing === "new" ? null : editing} onClose={() => setEditing(null)} onSaved={refresh} />}
      {adding && (
        <AddToDemoModal
          labelIds={ids}
          onClose={() => setAdding(false)}
          onDone={() => {
            setSelected(new Set());
            refresh();
          }}
        />
      )}
    </div>
  );
}

function Channel({ label: l }: { label: Pick<Label, "demo_policy" | "email" | "demo_url"> & { suppressed?: boolean } }) {
  const { t } = useTranslation();
  if (l.demo_policy === "closed") return <span className="badge bg-fg/[0.06] text-fg-subtle">{t("labels.policy.closed")}</span>;
  if (l.demo_policy === "form")
    return (
      <button className="badge bg-fg/[0.08] text-fg hover:bg-fg/15" onClick={() => l.demo_url && openUrl(l.demo_url).catch(() => {})}>
        {t("labels.policy.form")} <ExternalLink size={10} />
      </button>
    );
  if (!l.email) return <span className="badge bg-warn/15 text-warn">{t("labels.no_email")}</span>;
  if (l.suppressed) return <span className="badge bg-danger/15 text-danger" title={l.email}>{t("labels.suppressed")}</span>;
  return <span className="font-mono text-[12px]">{l.email}</span>;
}

function LabelModal({ label, onClose, onSaved }: { label: Label | null; onClose: () => void; onSaved: () => void }) {
  const { t } = useTranslation();
  const [f, setF] = useState<LabelInput>(() => ({
    id: label?.id ?? null,
    name: label?.name ?? "",
    country: label?.country ?? "",
    genres: label?.genres ?? "",
    website: label?.website ?? "",
    email: label?.email ?? "",
    contact_name: label?.contact_name ?? "",
    demo_policy: label?.demo_policy ?? "email",
    demo_url: label?.demo_url ?? "",
    tier: label?.tier ?? 2,
    notes: label?.notes ?? "",
  }));
  const set = <K extends keyof LabelInput>(k: K, v: LabelInput[K]) => setF((p) => ({ ...p, [k]: v }));
  const [submit, busy] = useSubmit(async () => {
    await saveLabel(f);
    onSaved();
    onClose();
  });
  return (
    <Modal open onClose={onClose} title={label ? label.name : t("labels.add_label")} width="max-w-2xl">
      <div className="grid gap-4 sm:grid-cols-2">
        <Field label={t("labels.col_label")}>
          <input className="input" autoFocus={!label} value={f.name} onChange={(e) => set("name", e.target.value)} />
        </Field>
        <Field label={t("labels.col_country")}>
          <input className="input" value={f.country ?? ""} onChange={(e) => set("country", e.target.value)} placeholder="France, United Kingdom…" />
        </Field>
        <Field label={t("labels.genres")}>
          <input className="input" value={f.genres ?? ""} onChange={(e) => set("genres", e.target.value)} placeholder="house, tech house" />
        </Field>
        <Field label={t("labels.website")}>
          <input className="input" value={f.website ?? ""} onChange={(e) => set("website", e.target.value)} placeholder="https://" />
        </Field>
        <Field label={t("labels.col_channel")} className="sm:col-span-2">
          <Segmented
            items={(["email", "form", "closed"] as const).map((k) => ({ key: k, label: t(`labels.policy.${k}`) }))}
            value={f.demo_policy}
            onChange={(k) => set("demo_policy", k)}
          />
          <p className="mt-1.5 text-[12.5px] text-fg-subtle">{t(`labels.policy_hint.${f.demo_policy}`)}</p>
        </Field>
        {f.demo_policy === "form" ? (
          <Field label={t("labels.demo_url")} className="sm:col-span-2">
            <input className="input" value={f.demo_url ?? ""} onChange={(e) => set("demo_url", e.target.value)} placeholder="https://label.com/demos" />
          </Field>
        ) : (
          <>
            <Field label={t("labels.demo_email")}>
              <input className="input" value={f.email ?? ""} onChange={(e) => set("email", e.target.value)} placeholder="demos@label.com" />
            </Field>
            <Field label={`${t("labels.contact_name")} (${t("common.optional")})`}>
              <input className="input" value={f.contact_name ?? ""} onChange={(e) => set("contact_name", e.target.value)} />
            </Field>
          </>
        )}
        <Field label={t("labels.tier")} className="sm:col-span-2">
          <Segmented
            items={[1, 2, 3].map((n) => ({ key: String(n), label: t(`labels.tier_${n}`) }))}
            value={String(f.tier)}
            onChange={(k) => set("tier", Number(k))}
          />
        </Field>
        <Field label={t("labels.notes")} className="sm:col-span-2">
          <textarea className="input" rows={3} value={f.notes ?? ""} onChange={(e) => set("notes", e.target.value)} />
        </Field>
      </div>
      <div className="mt-5 flex justify-end gap-2">
        <button className="btn-ghost" onClick={onClose}>
          {t("common.cancel")}
        </button>
        <button className="btn-primary" disabled={busy || !f.name.trim()} onClick={() => submit()}>
          {t("common.save")}
        </button>
      </div>
    </Modal>
  );
}

function AddToDemoModal({ labelIds, onClose, onDone }: { labelIds: number[]; onClose: () => void; onDone: () => void }) {
  const { t } = useTranslation();
  const toast = useToast();
  const navigate = useNavigate();
  const { data } = useQuery({ queryKey: ["label_demos"], queryFn: demosList });
  const open = (data ?? []).filter((d) => d.status !== "done");
  const [submit, busy] = useSubmit(async (id: number) => {
    const n = await addLabelsToDemo(id, labelIds);
    toast(t("labels.added_to_demo", { count: n }));
    onDone();
    navigate(`/labels/${id}?step=labels`);
  });
  return (
    <Modal open onClose={onClose} title={t("labels.add_to_demo")}>
      {!open.length ? (
        <p className="text-sm text-fg-subtle">{t("labels.no_open_demo")}</p>
      ) : (
        <div className="space-y-2">
          {open.map((d) => (
            <button key={d.id} disabled={busy} className="card flex w-full items-center justify-between px-4 py-3 text-left hover:border-fg" onClick={() => submit(d.id)}>
              <span className="font-medium">{d.track}</span>
              <span className="text-[13px] text-fg-subtle">{t("labels.card_not_started", { count: d.total })}</span>
            </button>
          ))}
        </div>
      )}
    </Modal>
  );
}

// ---------------------------------------------------------------------------
// One demo, four steps
// ---------------------------------------------------------------------------

type StepKey = "demo" | "labels" | "validate" | "follow";
const STEPS: StepKey[] = ["demo", "labels", "validate", "follow"];

const TODO_STATES: TargetState[] = ["to_validate", "to_submit", "failed"];

/// The step that needs the user right now.
function currentStep(v: DemoView): StepKey {
  if (!v.targets.length || v.card.current_wave === 0) return "labels";
  if (v.targets.some((x) => TODO_STATES.includes(x.state))) return "validate";
  return "follow";
}

export function LabelDemoPage() {
  const { t } = useTranslation();
  const { id: raw } = useParams();
  const id = Number(raw);
  const qc = useQueryClient();
  const toast = useToast();
  const confirm = useConfirm();
  const navigate = useNavigate();
  const [params, setParams] = useSearchParams();
  const { data: v, isLoading, isError, refetch } = useQuery({
    queryKey: ["label_demo", id],
    queryFn: () => getDemo(id),
    refetchInterval: 60_000,
  });
  const refresh = () => {
    qc.invalidateQueries({ queryKey: ["label_demo", id] });
    qc.invalidateQueries({ queryKey: ["label_demos"] });
    qc.invalidateQueries({ queryKey: ["labels"] });
    qc.invalidateQueries({ queryKey: ["label_messages", id] });
  };
  const status = useMutation({
    mutationFn: (s: "active" | "paused" | "done") => setDemoStatus(id, s),
    onSuccess: refresh,
    onError: (e) => toast(errorMessage(e), "error"),
  });
  if (isLoading) return <Loader />;
  if (isError || !v) return <ErrorState onRetry={refetch} />;
  const step = (STEPS.find((s) => s === params.get("step")) ?? currentStep(v)) as StepKey;
  const go = (s: StepKey) => setParams({ step: s }, { replace: true });
  const todo = v.targets.filter((x) => TODO_STATES.includes(x.state)).length;
  const paused = v.card.status === "paused" || !!v.card.paused_reason;
  const done: Record<StepKey, boolean> = {
    demo: !!(v.press_kit_id || v.listen_url),
    labels: v.card.current_wave > 0,
    validate: v.card.current_wave > 0 && todo === 0,
    follow: v.card.status === "done",
  };
  const remove = async () => {
    if (!confirm(t("labels.confirm_delete", { track: v.card.track }))) return;
    await deleteCampaign(id);
    qc.invalidateQueries({ queryKey: ["label_demos"] });
    qc.invalidateQueries({ queryKey: ["labels"] });
    navigate("/labels");
  };
  return (
    <div>
      <PageHeader
        kicker={t("labels.demo_kicker")}
        title={v.card.track}
        actions={
          <>
            {v.card.status !== "done" &&
              (paused ? (
                <button className="btn-primary" disabled={status.isPending} onClick={() => status.mutate("active")}>
                  <Play size={13} /> {t("labels.resume")}
                </button>
              ) : (
                <button className="btn-outline" disabled={status.isPending} onClick={() => status.mutate("paused")}>
                  <Pause size={13} /> {t("labels.pause")}
                </button>
              ))}
            {v.card.status === "done" ? (
              <button className="btn-outline" onClick={() => status.mutate("active")}>
                {t("labels.reopen_demo")}
              </button>
            ) : (
              <button className="btn-ghost" onClick={() => confirm(t("labels.confirm_finish")) && status.mutate("done")}>
                {t("labels.finish")}
              </button>
            )}
            <button className="btn-icon" aria-label={t("common.delete")} onClick={remove}>
              <Trash2 size={14} />
            </button>
          </>
        }
      />
      <div className="px-page py-5">
        <Link to="/labels" className="btn-ghost -ml-3 mb-3">
          <ArrowLeft size={14} /> {t("labels.back")}
        </Link>
        {(v.card.paused_reason || !v.smtp_ready) && (
          <div className="mb-4 flex items-start gap-2.5 rounded-xl border border-warn/40 bg-warn/10 px-4 py-3 text-sm">
            <AlertTriangle size={16} className="mt-0.5 shrink-0 text-warn" />
            <span>{v.card.paused_reason ?? t("labels.smtp_missing")}</span>
          </div>
        )}
        <nav className="mb-6 grid grid-cols-2 gap-2 @lg:grid-cols-4" aria-label={t("labels.steps")}>
          {STEPS.map((s, i) => (
            <button
              key={s}
              onClick={() => go(s)}
              aria-current={s === step ? "step" : undefined}
              className={clsx(
                "flex items-center gap-3 rounded-xl border px-4 py-3 text-left transition-colors",
                s === step ? "border-fg bg-surface" : "border-border bg-surface/50 hover:border-fg-subtle"
              )}
            >
              <span
                className={clsx(
                  "flex h-8 w-8 shrink-0 items-center justify-center rounded-full font-condensed text-lg",
                  done[s] ? "bg-ok/15 text-ok" : s === step ? "bg-accent text-accent-fg" : "bg-fg/[0.06] text-fg-subtle"
                )}
              >
                {done[s] ? <Check size={15} /> : i + 1}
              </span>
              <span className="min-w-0">
                <span className="block text-sm font-medium">{t(`labels.step_${i + 1}`)}</span>
                <span className="block truncate text-[12.5px] text-fg-subtle">{stepStatus(t, s, v, todo)}</span>
              </span>
            </button>
          ))}
        </nav>
        {step === "demo" && <DemoStep view={v} onSaved={refresh} />}
        {step === "labels" && <LabelsStep view={v} refresh={refresh} onLaunched={() => go("validate")} />}
        {step === "validate" && <ValidateStep view={v} refresh={refresh} goFollow={() => go("follow")} />}
        {step === "follow" && <FollowStep view={v} refresh={refresh} />}
      </div>
    </div>
  );
}

type T = (k: string, o?: Record<string, unknown>) => string;

function stepStatus(t: T, s: StepKey, v: DemoView, todo: number): string {
  switch (s) {
    case "demo":
      return v.press_kit_title ?? (v.listen_url ? t("labels.status_link") : t("labels.status_no_link"));
    case "labels":
      return t("labels.status_labels", { count: v.targets.length });
    case "validate":
      return v.card.current_wave === 0 ? t("labels.status_not_started") : todo ? t("labels.status_todo", { count: todo }) : t("labels.status_nothing");
    case "follow":
      return v.card.current_wave === 0
        ? t("labels.status_not_started")
        : t("labels.status_follow", { waiting: v.card.waiting, replied: v.card.replied });
  }
}

// ---------------- Step 1: the demo ----------------

function DemoStep({ view: v, onSaved }: { view: DemoView; onSaved: () => void }) {
  const { t } = useTranslation();
  const toast = useToast();
  const [track, setTrack] = useState(v.card.track);
  const [kit, setKit] = useState(v.press_kit_id ?? "");
  const [listenUrl, setListenUrl] = useState(v.listen_url ?? "");
  const [steps, setSteps] = useState<Step[]>(v.steps);
  const [waveSize, setWaveSize] = useState(v.wave_size);
  const [closeDays, setCloseDays] = useState(v.close_days);
  const [lang, setLang] = useState<"fr" | "en">("en");
  const { data: kits } = useQuery({ queryKey: ["press_kits"], queryFn: pressKits, retry: false });
  const drafts = v.targets.filter((x) => x.state === "to_validate").length;
  const setStep = (i: number, patch: Partial<Step>) => setSteps((s) => s.map((x, j) => (j === i ? { ...x, ...patch } : x)));
  const [submit, busy] = useSubmit(async () => {
    const k = kits?.find((x) => x.id === kit);
    await saveDemo(v.card.id, {
      track,
      press_kit_id: kit || null,
      press_kit_title: k?.title ?? (kit ? v.press_kit_title : null),
      listen_url: listenUrl,
      steps,
      wave_size: waveSize,
      close_days: closeDays,
    });
    if (drafts) await redraftDemo(v.card.id);
    toast(drafts ? t("labels.saved_redrafted", { count: drafts }) : t("labels.saved"));
    onSaved();
  });
  const subjKey = lang === "en" ? "subject_en" : "subject";
  const bodyKey = lang === "en" ? "body_en" : "body";
  return (
    <div className="grid gap-5 @xl:grid-cols-[minmax(0,22rem)_minmax(0,1fr)]">
      <div className="card space-y-4 p-5">
        <div className="eyebrow">{t("labels.section_track")}</div>
        <Field label={t("labels.track")}>
          <input className="input" value={track} onChange={(e) => setTrack(e.target.value)} />
        </Field>
        <KitFields kits={kits} kit={kit} setKit={setKit} listenUrl={listenUrl} setListenUrl={setListenUrl} />
        {kit && !v.insrt_ready && <p className="text-[12.5px] text-warn">{t("labels.insrt_missing")}</p>}
        <div className="eyebrow pt-2">{t("labels.section_rhythm")}</div>
        <div className="grid grid-cols-3 gap-3">
          <Field label={t("labels.wave_size")}>
            <input className="input" type="number" min={1} max={20} value={waveSize} onChange={(e) => setWaveSize(Number(e.target.value))} />
          </Field>
          <Field label={t("labels.followup_day")}>
            <input
              className="input"
              type="number"
              min={1}
              value={steps[1]?.offset_days ?? 0}
              disabled={steps.length < 2}
              onChange={(e) => setStep(1, { offset_days: Number(e.target.value) })}
            />
          </Field>
          <Field label={t("labels.close_day")}>
            <input className="input" type="number" min={2} value={closeDays} onChange={(e) => setCloseDays(Number(e.target.value))} />
          </Field>
        </div>
        <p className="text-[12.5px] text-fg-subtle">
          {t("labels.rhythm_hint", { size: waveSize, followup: steps[1]?.offset_days ?? "·", close: closeDays })}
        </p>
      </div>
      <div className="card space-y-4 p-5">
        <div className="flex items-center justify-between gap-3">
          <div className="eyebrow">{t("labels.section_emails")}</div>
          <Segmented
            items={[
              { key: "en", label: "English" },
              { key: "fr", label: "Français" },
            ]}
            value={lang}
            onChange={setLang}
          />
        </div>
        <p className="text-[12.5px] text-fg-subtle">{t("labels.emails_hint")}</p>
        {steps.map((s, i) => (
          <div key={i} className="space-y-2 rounded-lg border border-border p-4">
            <div className="text-sm font-medium">{i === 0 ? t("labels.email_first") : t("labels.email_followup", { day: s.offset_days })}</div>
            <input className="input" value={s[subjKey]} onChange={(e) => setStep(i, { [subjKey]: e.target.value })} aria-label={t("labels.subject")} />
            <textarea className="input font-mono text-[12.5px]" rows={i === 0 ? 14 : 10} value={s[bodyKey]} onChange={(e) => setStep(i, { [bodyKey]: e.target.value })} aria-label={t("labels.body")} />
          </div>
        ))}
        <div className="flex items-center justify-between gap-3">
          <span className="font-mono text-[11px] text-fg-subtle">{"{{label}} {{track}} {{artist}} {{press_link}} {{first_name}}"}</span>
          <button className="btn-primary" disabled={busy || !track.trim()} onClick={() => submit()}>
            {t("common.save")}
          </button>
        </div>
      </div>
    </div>
  );
}

// ---------------- Step 2: the labels ----------------

function LabelsStep({ view: v, refresh, onLaunched }: { view: DemoView; refresh: () => void; onLaunched: () => void }) {
  const { t } = useTranslation();
  const toast = useToast();
  const confirm = useConfirm();
  const [search, setSearch] = useState("");
  const [onlyReachable, setOnlyReachable] = useState(true);
  const { data: all } = useQuery({ queryKey: ["labels"], queryFn: labelsList });
  const waiting = v.targets.filter((x) => x.wave == null);
  const inWaves = v.targets.filter((x) => x.wave != null);
  const taken = new Set(v.targets.map((x) => x.label_id));
  const order = useMutation({
    mutationFn: (ids: number[]) => setDemoTargets(v.card.id, ids),
    onSuccess: refresh,
    onError: (e) => toast(errorMessage(e), "error"),
  });
  const ids = waiting.map((x) => x.label_id);
  const move = (i: number, d: -1 | 1) => {
    const n = [...ids];
    [n[i], n[i + d]] = [n[i + d], n[i]];
    order.mutate(n);
  };
  const candidates = useMemo(() => {
    const q = search.trim().toLowerCase();
    return (all ?? [])
      .filter((l) => !l.archived && !taken.has(l.id) && (!onlyReachable || isReachable(l)))
      .filter((l) => !q || [l.name, l.country, l.genres].some((x) => x?.toLowerCase().includes(q)))
      .slice(0, 200);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [all, search, onlyReachable, v.targets]);
  const queued = waiting.filter((x) => x.state === "queued");
  const nextWave = v.card.current_wave + 1;
  const waveOpen = inWaves.some((x) => x.wave === v.card.current_wave && !CLOSED_STATES.includes(x.state));
  const [launch, launching] = useSubmit(async () => {
    if (v.card.current_wave > 0 && waveOpen && !confirm(t("labels.confirm_next_wave", { wave: nextWave }))) return;
    const n = await openWave(v.card.id);
    toast(t("labels.wave_opened", { wave: nextWave, count: n }));
    refresh();
    onLaunched();
  });
  // Wave each waiting label will leave in: unreachable ones are passed over.
  let reach = 0;
  return (
    <div className="grid gap-5 @xl:grid-cols-2">
      <div className="card p-5">
        <div className="flex flex-wrap items-center justify-between gap-3">
          <div>
            <div className="eyebrow">{t("labels.section_order")}</div>
            <p className="mt-1 text-[12.5px] text-fg-subtle">{t("labels.order_hint", { size: v.wave_size })}</p>
          </div>
          <button
            className={v.card.current_wave > 0 && waveOpen ? "btn-outline" : "btn-primary"}
            disabled={launching || !queued.length || v.card.status === "done"}
            onClick={() => launch()}
          >
            <Send size={13} />
            {v.card.current_wave === 0
              ? t("labels.launch_first", { count: Math.min(queued.length, v.wave_size) })
              : t("labels.launch_next", { wave: nextWave })}
          </button>
        </div>
        {inWaves.length > 0 && (
          <div className="mt-4 rounded-lg bg-fg/[0.03] px-4 py-3 text-[13px] text-fg-subtle">
            {t("labels.already_sent", { count: inWaves.length, wave: v.card.current_wave })}
          </div>
        )}
        {!waiting.length ? (
          <EmptyState icon={Disc3} title={t("labels.no_waiting")} hint={t("labels.no_waiting_hint")} />
        ) : (
          <ol className="mt-3">
            {waiting.map((x, i) => {
              const reachable = x.state === "queued";
              const wave = reachable ? v.card.current_wave + Math.floor(reach / v.wave_size) + 1 : null;
              const first = reachable && reach % v.wave_size === 0;
              if (reachable) reach += 1;
              return (
                <li key={x.id}>
                  {first && <div className="kicker mb-1 mt-4">{t("labels.wave_n", { wave })}</div>}
                  <div className="flex items-center gap-3 border-b border-border py-2">
                    <span className="w-6 text-right font-mono text-[11px] text-fg-faint">{i + 1}</span>
                    <TierDots tier={x.tier} />
                    <div className="min-w-0 flex-1">
                      <div className="truncate text-sm font-medium">{x.name}</div>
                      <div className="truncate text-[12.5px] text-fg-subtle">
                        {x.demo_policy === "form" ? t("labels.policy.form") : x.email ?? t("labels.no_email")}
                        {x.country ? ` · ${x.country}` : ""}
                      </div>
                    </div>
                    {!reachable && <StateBadge state="unreachable" />}
                    <button className="btn-icon" disabled={i === 0 || order.isPending} aria-label={t("labels.move_up")} onClick={() => move(i, -1)}>
                      <ArrowUp size={14} />
                    </button>
                    <button className="btn-icon" disabled={i === waiting.length - 1 || order.isPending} aria-label={t("labels.move_down")} onClick={() => move(i, 1)}>
                      <ArrowDown size={14} />
                    </button>
                    <button className="btn-icon" aria-label={t("labels.remove")} onClick={() => order.mutate(ids.filter((l) => l !== x.label_id))}>
                      <X size={14} />
                    </button>
                  </div>
                </li>
              );
            })}
          </ol>
        )}
      </div>
      <div className="card p-5">
        <div className="eyebrow">{t("labels.section_pick")}</div>
        <div className="mt-3 flex items-center gap-3">
          <input className="input" value={search} onChange={(e) => setSearch(e.target.value)} placeholder={t("common.search")} />
          <label className="flex shrink-0 items-center gap-2 text-[13px] text-fg-subtle">
            <input type="checkbox" checked={onlyReachable} onChange={(e) => setOnlyReachable(e.target.checked)} />
            {t("labels.filter.reachable")}
          </label>
        </div>
        <ul className="mt-3 max-h-[32rem] overflow-y-auto">
          {candidates.map((l) => (
            <li key={l.id} className="flex items-center gap-3 border-b border-border py-2">
              <TierDots tier={l.tier} />
              <div className="min-w-0 flex-1">
                <div className="truncate text-sm font-medium">{l.name}</div>
                <div className="truncate text-[12.5px] text-fg-subtle">{[l.genres, l.country].filter(Boolean).join(" · ") || "·"}</div>
              </div>
              {l.demos.length > 0 && <span className="badge bg-fg/[0.06] text-fg-subtle">{t("labels.already_demoed", { count: l.demos.length })}</span>}
              <button className="btn-outline h-8 px-3" disabled={order.isPending} onClick={() => order.mutate([...ids, l.id])}>
                <Plus size={13} /> {t("common.add")}
              </button>
            </li>
          ))}
          {!candidates.length && <li className="py-8 text-center text-sm text-fg-subtle">{t("labels.empty_list")}</li>}
        </ul>
        <p className="mt-3 text-[12.5px] text-fg-subtle">
          {t("labels.pick_hint")}{" "}
          <Link to="/labels?tab=list" className="underline hover:text-fg">
            {t("labels.tab_list")}
          </Link>
        </p>
      </div>
    </div>
  );
}

// ---------------- Step 3: validate ----------------

function ValidateStep({ view: v, refresh, goFollow }: { view: DemoView; refresh: () => void; goFollow: () => void }) {
  const { t } = useTranslation();
  const toast = useToast();
  const [editing, setEditing] = useState<MessageRow | null>(null);
  const { data: messages } = useQuery({ queryKey: ["label_messages", v.card.id], queryFn: () => residencyMessages(v.card.id) });
  const byId = useMemo(() => new Map((messages ?? []).map((m) => [m.id, m])), [messages]);
  const act = useMutation({
    mutationFn: (fn: () => Promise<unknown>) => fn(),
    onSuccess: refresh,
    onError: (e) => toast(errorMessage(e), "error"),
  });
  const items = v.targets.filter((x) => [...TODO_STATES, "scheduled"].includes(x.state));
  const drafts = items.filter((x) => x.state === "to_validate");
  if (v.card.current_wave === 0)
    return <EmptyState icon={Send} title={t("labels.status_not_started")} hint={t("labels.validate_not_started")} />;
  if (!items.length)
    return (
      <EmptyState
        icon={Check}
        title={t("labels.status_nothing")}
        hint={t("labels.validate_done")}
        action={
          <button className="btn-outline" onClick={goFollow}>
            {t("labels.step_4")}
          </button>
        }
      />
    );
  return (
    <div className="mx-auto max-w-3xl space-y-3">
      <div className="flex flex-wrap items-center justify-between gap-3">
        <p className="text-sm text-fg-subtle">{t("labels.validate_hint", { days: v.steps[1]?.offset_days ?? 10 })}</p>
        {drafts.length > 1 && (
          <button
            className="btn-primary"
            disabled={act.isPending}
            onClick={() =>
              act.mutate(async () => {
                const r = await validateAll(v.card.id);
                toast(t("labels.validated", { count: r.scheduled }));
              })
            }
          >
            <Check size={13} /> {t("labels.validate_all", { count: drafts.length })}
          </button>
        )}
      </div>
      {items.map((x) => {
        const m = x.draft_id ? byId.get(x.draft_id) : undefined;
        return (
          <div key={x.id} className="card p-5">
            <div className="flex flex-wrap items-center gap-2">
              <span className="font-medium">{x.name}</span>
              <span className="font-mono text-[12px] text-fg-subtle">{x.demo_policy === "form" ? x.demo_url : x.email}</span>
              <div className="flex-1" />
              <StateBadge state={x.state} />
            </div>
            {x.note && (
              <div className="mt-2 flex items-center gap-2 text-[13px] text-warn">
                <AlertTriangle size={13} /> {x.note}
              </div>
            )}
            {x.demo_policy === "form" ? (
              <>
                <p className="mt-3 text-sm text-fg-subtle">{t("labels.form_hint")}</p>
                <div className="mt-3 flex flex-wrap justify-end gap-2">
                  <button className="btn-outline" onClick={() => x.demo_url && openUrl(x.demo_url).catch(() => {})}>
                    <ExternalLink size={13} /> {t("labels.open_form")}
                  </button>
                  <button className="btn-primary" disabled={act.isPending} onClick={() => act.mutate(() => markSubmitted(x.id, true))}>
                    <Check size={13} /> {t("labels.mark_submitted")}
                  </button>
                </div>
              </>
            ) : (
              <>
                {m && (
                  <div className="mt-3 rounded-lg bg-fg/[0.03] p-4">
                    <div className="text-sm font-medium">{m.subject}</div>
                    <div className="mt-2 whitespace-pre-wrap text-[13px] leading-relaxed text-fg-subtle">{m.body}</div>
                  </div>
                )}
                <div className="mt-3 flex flex-wrap justify-end gap-2">
                  {x.state === "scheduled" ? (
                    <button className="btn-ghost" disabled={act.isPending} onClick={() => x.draft_id && act.mutate(() => setMessageStatus(x.draft_id!, "draft"))}>
                      {t("labels.unvalidate")}
                    </button>
                  ) : (
                    <>
                      <button className="btn-outline" disabled={!m} onClick={() => m && setEditing(m)}>
                        <Pencil size={13} /> {t("common.edit")}
                      </button>
                      <button
                        className="btn-primary"
                        disabled={act.isPending || !x.draft_id}
                        onClick={() => act.mutate(() => setMessageStatus(x.draft_id!, x.state === "failed" ? "draft" : "scheduled"))}
                      >
                        <Check size={13} /> {x.state === "failed" ? t("labels.retry") : t("labels.validate")}
                      </button>
                    </>
                  )}
                </div>
              </>
            )}
          </div>
        );
      })}
      {editing && <MessageModal message={editing} onClose={() => setEditing(null)} onSaved={refresh} />}
    </div>
  );
}

function MessageModal({ message, onClose, onSaved }: { message: MessageRow; onClose: () => void; onSaved: () => void }) {
  const { t } = useTranslation();
  const [subject, setSubject] = useState(message.subject);
  const [body, setBody] = useState(message.body);
  const [submit, busy] = useSubmit(async () => {
    await saveMessage(message.id, subject, body);
    onSaved();
    onClose();
  });
  return (
    <Modal open onClose={onClose} title={message.name} width="max-w-2xl">
      <div className="space-y-3">
        <Field label={t("labels.subject")}>
          <input className="input" value={subject} onChange={(e) => setSubject(e.target.value)} />
        </Field>
        <Field label={t("labels.body")}>
          <textarea className="input" rows={14} value={body} onChange={(e) => setBody(e.target.value)} />
        </Field>
        <div className="flex justify-end gap-2">
          <button className="btn-ghost" onClick={onClose}>
            {t("common.cancel")}
          </button>
          <button className="btn-primary" disabled={busy} onClick={() => submit()}>
            {t("common.save")}
          </button>
        </div>
      </div>
    </Modal>
  );
}

// ---------------- Step 4: follow-up ----------------

const DECISIONS: Outcome[] = ["replied", "declined", "withdrawn", "signed"];

function FollowStep({ view: v, refresh }: { view: DemoView; refresh: () => void }) {
  const { t } = useTranslation();
  const toast = useToast();
  const confirm = useConfirm();
  const rows = v.targets.filter((x) => x.wave != null);
  const waves = [...new Set(rows.map((x) => x.wave as number))].sort((a, b) => b - a);
  const decide = useMutation({
    mutationFn: ({ id, outcome }: { id: number; outcome: Outcome | null }) => setTargetOutcome(id, outcome),
    onSuccess: refresh,
    onError: (e) => toast(errorMessage(e), "error"),
  });
  if (!rows.length) return <EmptyState icon={Send} title={t("labels.status_not_started")} hint={t("labels.validate_not_started")} />;
  const queued = v.targets.filter((x) => x.state === "queued").length;
  return (
    <div className="space-y-5">
      <div className="grid grid-cols-2 gap-3 @lg:grid-cols-5">
        {(
          [
            ["sent", rows.filter((x) => x.sent_at).length],
            ["listened", v.card.listened],
            ["replied", v.card.replied],
            ["closed", v.card.closed],
            ["queued", queued],
          ] as const
        ).map(([k, n]) => (
          <div key={k} className="card px-4 py-3">
            <div className="font-condensed text-3xl leading-none tabular">{n}</div>
            <div className="mt-1.5 eyebrow">{t(`labels.count.${k}`)}</div>
          </div>
        ))}
      </div>
      <p className="text-[13px] text-fg-subtle">
        {v.next_send ? t("labels.next_send", { when: formatDateTime(v.next_send) }) : t("labels.no_next_send")}{" "}
        {t("labels.follow_hint", { close: v.close_days })}
      </p>
      {waves.map((w) => (
        <div key={w} className="card overflow-x-auto">
          <div className="kicker px-4 pt-4">{t("labels.wave_n", { wave: w })}</div>
          <table className="tbl mt-2 min-w-[52rem]">
            <thead>
              <tr>
                <th>{t("labels.col_label")}</th>
                <th>{t("labels.col_state")}</th>
                <th>{t("labels.col_sent")}</th>
                <th>{t("labels.col_followup")}</th>
                <th>{t("labels.col_closes")}</th>
                <th>{t("labels.col_listens")}</th>
                <th>{t("labels.col_reply")}</th>
                <th>{t("labels.col_decision")}</th>
              </tr>
            </thead>
            <tbody>
              {rows
                .filter((x) => x.wave === w)
                .map((x) => (
                  <tr key={x.id}>
                    <td>
                      <div className="font-medium">{x.name}</div>
                      <div className="font-mono text-[11.5px] text-fg-subtle">{x.demo_policy === "form" ? t("labels.policy.form") : x.email}</div>
                    </td>
                    <td>
                      <StateBadge state={x.state} />
                    </td>
                    <td className="whitespace-nowrap text-fg-subtle">{formatDate(x.sent_at)}</td>
                    <td className="whitespace-nowrap text-fg-subtle">
                      {x.followup_at ? (
                        <span className={clsx(x.followup_sent && "text-fg")}>
                          {formatDate(x.followup_at)}
                          {x.followup_sent && <Check size={12} className="ml-1 inline" />}
                        </span>
                      ) : (
                        "·"
                      )}
                    </td>
                    <td className="whitespace-nowrap text-fg-subtle">{CLOSED_STATES.includes(x.state) || x.state === "signed" ? "·" : formatDate(x.closes_at)}</td>
                    <td className="whitespace-nowrap">
                      {x.listens > 0 ? (
                        <span className="inline-flex items-center gap-1 text-ok">
                          <Headphones size={13} /> {x.listens}
                          {x.completion != null && <span className="text-fg-subtle">· {Math.round(x.completion * 100)} %</span>}
                        </span>
                      ) : x.views > 0 || x.opens > 0 ? (
                        <span className="text-fg-subtle">{t("labels.opened")}</span>
                      ) : (
                        <span className="text-fg-faint">·</span>
                      )}
                    </td>
                    <td className="max-w-[18rem]">
                      {x.reply_summary ? (
                        <Link to="/replies?tab=inbox" className="line-clamp-2 text-[13px] hover:underline" title={x.reply_summary}>
                          {x.reply_summary}
                        </Link>
                      ) : x.state === "replied" ? (
                        <Link to="/replies?tab=inbox" className="text-[13px] underline">
                          {t("labels.see_reply")}
                        </Link>
                      ) : (
                        <span className="text-fg-faint">·</span>
                      )}
                    </td>
                    <td>
                      <select
                        className="input h-8 min-h-0 w-auto py-0 text-[12.5px]"
                        aria-label={t("labels.col_decision")}
                        disabled={decide.isPending}
                        value={x.outcome_by === "user" ? x.state : ""}
                        onChange={(e) => {
                          const o = (e.target.value || null) as Outcome | null;
                          if (o === "signed" && !confirm(t("labels.confirm_signed", { label: x.name }))) return;
                          decide.mutate({ id: x.id, outcome: o });
                        }}
                      >
                        <option value="">{t("labels.decision_auto")}</option>
                        {DECISIONS.map((o) => (
                          <option key={o} value={o}>
                            {t(`labels.decision.${o}`)}
                          </option>
                        ))}
                      </select>
                    </td>
                  </tr>
                ))}
            </tbody>
          </table>
        </div>
      ))}
    </div>
  );
}
