import { useEffect, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import {
  Plus,
  Pencil,
  Trash2,
  Mail,
  Send,
  Eye,
  EyeOff,
  RefreshCw,
  Megaphone,
  Target,
} from "lucide-react";
import clsx from "clsx";
import {
  applyOpens,
  deleteCampaign,
  deleteTemplate,
  getSetting,
  listArtists,
  listCampaigns,
  listEmails,
  listTemplates,
  saveCampaign,
  saveTemplate,
  sendEmail,
} from "../lib/api";
import type { Campaign, Template } from "../lib/types";
import { CAMPAIGN_STATUSES } from "../lib/types";
import { PageHeader, EmptyState } from "../components/Layout";
import { Modal, Field, useToast, useConfirm } from "../components/ui";

type Tab = "compose" | "campaigns" | "templates" | "log";

const CAMPAIGN_COLORS = [
  "#6366f1", "#10b981", "#f59e0b", "#ec4899", "#0ea5e9", "#8b5cf6", "#ef4444", "#14b8a6",
];

const CAMPAIGN_STATUS_STYLE: Record<string, string> = {
  active: "bg-emerald-500/10 text-emerald-500",
  scheduled: "bg-blue-500/10 text-blue-500",
  done: "bg-slate-500/10 text-slate-500",
  archived: "bg-zinc-500/10 text-zinc-400",
};

export function Emails() {
  const { t } = useTranslation();
  const [tab, setTab] = useState<Tab>("compose");
  const tabs: Tab[] = ["compose", "campaigns", "templates", "log"];

  useEffect(() => {
    const h = (e: Event) => {
      if ((e as CustomEvent).detail === "new:campaign") setTab("campaigns");
    };
    window.addEventListener("app:new", h);
    return () => window.removeEventListener("app:new", h);
  }, []);

  return (
    <div>
      <PageHeader title={t("emails.title")} subtitle={t("emails.subtitle")} />
      <div className="px-8 py-6">
        <div className="segmented mb-6">
          {tabs.map((tb) => (
            <button
              key={tb}
              data-active={tab === tb}
              onClick={() => setTab(tb)}
              className="segmented-item"
            >
              {t(`emails.tab_${tb}`)}
            </button>
          ))}
        </div>

        {tab === "compose" && <ComposeTab />}
        {tab === "campaigns" && <CampaignsTab />}
        {tab === "templates" && <TemplatesTab />}
        {tab === "log" && <LogTab />}
      </div>
    </div>
  );
}

// ---------------- Compose ----------------

function ComposeTab() {
  const { t } = useTranslation();
  const toast = useToast();
  const qc = useQueryClient();
  const { data: templates } = useQuery({ queryKey: ["templates"], queryFn: listTemplates });
  const { data: campaigns = [] } = useQuery({ queryKey: ["campaigns"], queryFn: listCampaigns });
  const [to, setTo] = useState("");
  const [subject, setSubject] = useState("");
  const [body, setBody] = useState("");
  const [campaignId, setCampaignId] = useState<number | "">("");
  const [sending, setSending] = useState(false);

  const doSend = async () => {
    setSending(true);
    try {
      const res = await sendEmail({
        contact_id: null,
        campaign_id: campaignId === "" ? null : campaignId,
        to: to.trim(),
        subject,
        body,
      });
      if (res.ok) {
        toast(t("emails.sent_ok"), "ok");
        setTo("");
        setSubject("");
        setBody("");
        qc.invalidateQueries({ queryKey: ["emails"] });
        qc.invalidateQueries({ queryKey: ["campaigns"] });
        qc.invalidateQueries({ queryKey: ["dashboard"] });
      } else {
        toast(`${t("emails.send_failed")}: ${res.error ?? ""}`, "error");
      }
    } catch (e: any) {
      toast(`${t("emails.send_failed")}: ${e?.toString?.() ?? e}`, "error");
    } finally {
      setSending(false);
    }
  };

  return (
    <div className="card max-w-2xl space-y-3.5 p-6">
      <div className="grid grid-cols-2 gap-4">
        <Field label={t("emails.template")}>
          <select
            className="input"
            onChange={(e) => {
              const tpl = templates?.find((x) => x.id === Number(e.target.value));
              if (tpl) {
                setSubject(tpl.subject);
                setBody(tpl.body);
              }
            }}
          >
            <option value="">{t("emails.pick_template")}</option>
            {templates?.map((tpl) => (
              <option key={tpl.id} value={tpl.id}>
                {tpl.name}
              </option>
            ))}
          </select>
        </Field>
        <Field label={t("emails.campaign")}>
          <select
            className="input"
            value={campaignId}
            onChange={(e) => setCampaignId(e.target.value ? Number(e.target.value) : "")}
          >
            <option value="">{t("emails.no_campaign")}</option>
            {campaigns.map((c) => (
              <option key={c.id} value={c.id}>
                {c.name}
              </option>
            ))}
          </select>
        </Field>
      </div>
      <Field label={t("emails.to")}>
        <input className="input" value={to} onChange={(e) => setTo(e.target.value)} />
      </Field>
      <Field label={t("emails.subject")}>
        <input className="input" value={subject} onChange={(e) => setSubject(e.target.value)} />
      </Field>
      <Field label={t("emails.body")}>
        <textarea
          className="input min-h-[240px] font-mono text-[13px] leading-relaxed"
          value={body}
          onChange={(e) => setBody(e.target.value)}
        />
      </Field>
      <div className="flex justify-end">
        <button
          className="btn-primary"
          disabled={!to.trim() || !subject.trim() || !body.trim() || sending}
          onClick={doSend}
        >
          <Send size={15} />
          {sending ? t("emails.sending") : t("emails.send")}
        </button>
      </div>
    </div>
  );
}

// ---------------- Campaigns ----------------

function daysDiff(dateStr: string): number {
  const d = new Date(dateStr);
  return Math.round((d.getTime() - Date.now()) / 86400000);
}

function CampaignsTab() {
  const { t } = useTranslation();
  const qc = useQueryClient();
  const confirm = useConfirm();
  const { data: campaigns = [] } = useQuery({ queryKey: ["campaigns"], queryFn: listCampaigns });
  const { data: artists = [] } = useQuery({ queryKey: ["artists"], queryFn: listArtists });
  const [editing, setEditing] = useState<Campaign | null>(null);

  const delMut = useMutation({
    mutationFn: (id: number) => deleteCampaign(id),
    onSuccess: () => qc.invalidateQueries({ queryKey: ["campaigns"] }),
  });

  const artistName = (id?: number | null) => artists.find((a) => a.id === id)?.name;

  return (
    <div className="space-y-4">
      <button
        className="btn-primary"
        onClick={() =>
          setEditing({ name: "", status: "active", color: CAMPAIGN_COLORS[0] } as Campaign)
        }
      >
        <Plus size={16} />
        {t("campaigns.new")}
      </button>

      {campaigns.length === 0 ? (
        <div className="card">
          <EmptyState icon={Megaphone} title={t("campaigns.no_campaigns")} />
        </div>
      ) : (
        <div className="grid grid-cols-1 gap-4 md:grid-cols-2 xl:grid-cols-3">
          {campaigns.map((c) => {
            const rate = c.sent_count ? Math.round((c.opened_count / c.sent_count) * 100) : 0;
            const dd = c.target_date ? daysDiff(c.target_date) : null;
            return (
              <div key={c.id} className="card group overflow-hidden p-5">
                <div className="flex items-start justify-between gap-2">
                  <div className="flex items-center gap-2 min-w-0">
                    <span
                      className="h-3 w-3 shrink-0 rounded-full"
                      style={{ background: c.color ?? "#6366f1" }}
                    />
                    <span className="truncate font-semibold">{c.name}</span>
                  </div>
                  <span
                    className={clsx(
                      "badge shrink-0",
                      CAMPAIGN_STATUS_STYLE[c.status] ?? "bg-muted text-fg-subtle"
                    )}
                  >
                    {t(`campaigns.st_${c.status}`)}
                  </span>
                </div>

                {c.purpose && (
                  <p className="mt-2 line-clamp-2 text-xs text-fg-subtle">{c.purpose}</p>
                )}

                <div className="mt-3 flex items-center gap-3 text-xs text-fg-subtle">
                  {artistName(c.artist_id) && (
                    <span className="truncate">{artistName(c.artist_id)}</span>
                  )}
                  {dd !== null && (
                    <span className="inline-flex items-center gap-1">
                      <Target size={12} />
                      {dd >= 0
                        ? t("campaigns.days_left", { count: dd })
                        : t("campaigns.days_past", { count: -dd })}
                    </span>
                  )}
                </div>

                <div className="mt-4 grid grid-cols-3 gap-2 border-t border-border pt-3">
                  <Stat label={t("campaigns.sent")} value={c.sent_count} />
                  <Stat label={t("campaigns.opened")} value={c.opened_count} />
                  <Stat label={t("campaigns.open_rate")} value={`${rate}%`} accent />
                </div>

                <div className="mt-3 flex justify-end gap-1 opacity-0 transition-opacity group-hover:opacity-100">
                  <button className="btn-ghost px-2 py-1.5" onClick={() => setEditing(c)}>
                    <Pencil size={14} />
                  </button>
                  <button
                    className="btn-ghost px-2 py-1.5 text-rose-500"
                    onClick={() => confirm(t("common.confirm_delete")) && delMut.mutate(c.id!)}
                  >
                    <Trash2 size={14} />
                  </button>
                </div>
              </div>
            );
          })}
        </div>
      )}

      {editing && (
        <CampaignModal
          campaign={editing}
          artists={artists}
          onClose={() => setEditing(null)}
          onSaved={() => {
            setEditing(null);
            qc.invalidateQueries({ queryKey: ["campaigns"] });
          }}
        />
      )}
    </div>
  );
}

function Stat({ label, value, accent }: { label: string; value: any; accent?: boolean }) {
  return (
    <div>
      <div className={clsx("text-base font-semibold tabular", accent && "text-accent")}>
        {value}
      </div>
      <div className="text-2xs text-fg-subtle">{label}</div>
    </div>
  );
}

function CampaignModal({
  campaign,
  artists,
  onClose,
  onSaved,
}: {
  campaign: Campaign;
  artists: { id?: number; name: string }[];
  onClose: () => void;
  onSaved: () => void;
}) {
  const { t } = useTranslation();
  const toast = useToast();
  const [form, setForm] = useState<Campaign>({ ...campaign });
  const set = (k: keyof Campaign, v: any) => setForm((f) => ({ ...f, [k]: v }));
  const save = async () => {
    if (!form.name.trim()) {
      toast(t("common.required"), "error");
      return;
    }
    await saveCampaign(form);
    toast(t("common.save"), "ok");
    onSaved();
  };
  return (
    <Modal
      open
      onClose={onClose}
      width="max-w-lg"
      title={campaign.id ? t("campaigns.edit") : t("campaigns.new")}
    >
      <div className="space-y-3.5">
        <Field label={t("campaigns.name")}>
          <input
            className="input"
            value={form.name}
            onChange={(e) => set("name", e.target.value)}
          />
        </Field>
        <Field label={t("campaigns.purpose")}>
          <textarea
            className="input min-h-[70px]"
            value={form.purpose ?? ""}
            placeholder={t("campaigns.purpose_ph")}
            onChange={(e) => set("purpose", e.target.value)}
          />
        </Field>
        <div className="grid grid-cols-2 gap-x-4 gap-y-3">
          <Field label={t("campaigns.artist")}>
            <select
              className="input"
              value={form.artist_id ?? ""}
              onChange={(e) => set("artist_id", e.target.value ? Number(e.target.value) : null)}
            >
              <option value="">—</option>
              {artists.map((a) => (
                <option key={a.id} value={a.id}>
                  {a.name}
                </option>
              ))}
            </select>
          </Field>
          <Field label={t("campaigns.target_date")}>
            <input
              type="date"
              className="input"
              value={form.target_date ?? ""}
              onChange={(e) => set("target_date", e.target.value)}
            />
          </Field>
          <Field label={t("campaigns.status")}>
            <select
              className="input"
              value={form.status}
              onChange={(e) => set("status", e.target.value)}
            >
              {CAMPAIGN_STATUSES.map((s) => (
                <option key={s} value={s}>
                  {t(`campaigns.st_${s}`)}
                </option>
              ))}
            </select>
          </Field>
          <Field label={t("campaigns.color")}>
            <div className="flex flex-wrap items-center gap-1.5 pt-1.5">
              {CAMPAIGN_COLORS.map((col) => (
                <button
                  key={col}
                  onClick={() => set("color", col)}
                  className={clsx(
                    "h-6 w-6 rounded-full transition-transform",
                    form.color === col ? "ring-2 ring-offset-2 ring-offset-surface scale-110" : ""
                  )}
                  style={{ background: col, boxShadow: form.color === col ? `0 0 0 2px ${col}` : "" }}
                />
              ))}
            </div>
          </Field>
        </div>
        <div className="flex justify-end gap-2">
          <button className="btn-outline" onClick={onClose}>
            {t("common.cancel")}
          </button>
          <button className="btn-primary" onClick={save}>
            {t("common.save")}
          </button>
        </div>
      </div>
    </Modal>
  );
}

// ---------------- Templates ----------------

function TemplatesTab() {
  const { t } = useTranslation();
  const qc = useQueryClient();
  const toast = useToast();
  const confirm = useConfirm();
  const { data: templates = [] } = useQuery({ queryKey: ["templates"], queryFn: listTemplates });
  const [editing, setEditing] = useState<Template | null>(null);

  const delMut = useMutation({
    mutationFn: (id: number) => deleteTemplate(id),
    onSuccess: () => qc.invalidateQueries({ queryKey: ["templates"] }),
  });

  return (
    <div className="space-y-4">
      <button className="btn-primary" onClick={() => setEditing({ name: "", subject: "", body: "" })}>
        <Plus size={16} />
        {t("emails.new_template")}
      </button>
      <div className="grid grid-cols-1 gap-3 md:grid-cols-2">
        {templates.map((tpl) => (
          <div key={tpl.id} className="card group p-4">
            <div className="flex items-start justify-between">
              <div className="min-w-0">
                <div className="text-sm font-semibold">{tpl.name}</div>
                <div className="mt-0.5 truncate text-xs text-fg-subtle">{tpl.subject}</div>
              </div>
              <div className="flex gap-1 opacity-0 transition-opacity group-hover:opacity-100">
                <button className="btn-ghost px-2 py-1.5" onClick={() => setEditing(tpl)}>
                  <Pencil size={14} />
                </button>
                <button
                  className="btn-ghost px-2 py-1.5 text-rose-500"
                  onClick={() => confirm(t("common.confirm_delete")) && delMut.mutate(tpl.id!)}
                >
                  <Trash2 size={14} />
                </button>
              </div>
            </div>
            <p className="mt-2 line-clamp-4 whitespace-pre-wrap text-xs text-fg-subtle">
              {tpl.body}
            </p>
          </div>
        ))}
      </div>

      {editing && (
        <TemplateModal
          template={editing}
          onClose={() => setEditing(null)}
          onSaved={() => {
            setEditing(null);
            qc.invalidateQueries({ queryKey: ["templates"] });
            toast(t("common.save"), "ok");
          }}
        />
      )}
    </div>
  );
}

function TemplateModal({
  template,
  onClose,
  onSaved,
}: {
  template: Template;
  onClose: () => void;
  onSaved: () => void;
}) {
  const { t } = useTranslation();
  const [form, setForm] = useState<Template>({ ...template });
  const save = async () => {
    if (!form.name.trim()) return;
    await saveTemplate(form);
    onSaved();
  };
  return (
    <Modal open onClose={onClose} title={t("emails.new_template")} width="max-w-2xl">
      <div className="space-y-3.5">
        <Field label={t("emails.template_name")}>
          <input
            className="input"
            value={form.name}
            onChange={(e) => setForm({ ...form, name: e.target.value })}
          />
        </Field>
        <Field label={t("emails.subject")}>
          <input
            className="input"
            value={form.subject}
            onChange={(e) => setForm({ ...form, subject: e.target.value })}
          />
        </Field>
        <Field label={t("emails.body")}>
          <textarea
            className="input min-h-[240px] font-mono text-[13px] leading-relaxed"
            value={form.body}
            onChange={(e) => setForm({ ...form, body: e.target.value })}
          />
        </Field>
        <p className="text-xs text-fg-subtle">{t("emails.variables")}</p>
        <div className="flex justify-end gap-2">
          <button className="btn-outline" onClick={onClose}>
            {t("common.cancel")}
          </button>
          <button className="btn-primary" onClick={save}>
            {t("common.save")}
          </button>
        </div>
      </div>
    </Modal>
  );
}

// ---------------- Log ----------------

function LogTab() {
  const { t } = useTranslation();
  const toast = useToast();
  const qc = useQueryClient();
  const [syncing, setSyncing] = useState(false);
  const { data: emails = [] } = useQuery({ queryKey: ["emails"], queryFn: () => listEmails() });

  const syncOpens = async () => {
    const base = (await getSetting("tracking_base_url"))?.trim().replace(/\/$/, "");
    if (!base) {
      toast(t("emails.no_tracking"), "error");
      return;
    }
    setSyncing(true);
    try {
      const res = await fetch(`${base}/opens.json`, { cache: "no-store" });
      const data = await res.json();
      const opens = Array.isArray(data)
        ? data
        : Object.entries(data).map(([token, v]: any) =>
            typeof v === "string"
              ? { token, opened_at: v }
              : { token, opened_at: v?.opened_at, count: v?.count }
          );
      const n = await applyOpens(opens);
      qc.invalidateQueries({ queryKey: ["emails"] });
      qc.invalidateQueries({ queryKey: ["campaigns"] });
      toast(t("emails.sync_done", { count: n }), "ok");
    } catch (e: any) {
      toast(`${t("emails.send_failed")}: ${e?.toString?.() ?? e}`, "error");
    } finally {
      setSyncing(false);
    }
  };

  return (
    <div className="space-y-3">
      <div className="flex justify-end">
        <button className="btn-outline" onClick={syncOpens} disabled={syncing}>
          <RefreshCw size={14} className={syncing ? "animate-spin" : ""} />
          {syncing ? t("emails.syncing") : t("emails.sync_opens")}
        </button>
      </div>
      {emails.length === 0 ? (
        <div className="card">
          <EmptyState icon={Mail} title={t("emails.log_empty")} />
        </div>
      ) : (
        <div className="card max-h-[calc(100vh-260px)] overflow-auto">
          <table className="tbl">
            <thead>
              <tr>
                <th>{t("emails.to")}</th>
                <th>{t("emails.subject")}</th>
                <th>{t("contacts.status")}</th>
                <th>{t("emails.open_col")}</th>
                <th>{t("contacts.date")}</th>
              </tr>
            </thead>
            <tbody>
              {emails.map((e) => (
                <tr key={e.id}>
                  <td>{e.to_addr}</td>
                  <td className="max-w-[280px] truncate text-fg-subtle">{e.subject}</td>
                  <td>
                    <span
                      className={clsx(
                        "badge",
                        e.status === "sent"
                          ? "bg-emerald-500/10 text-emerald-500"
                          : "bg-rose-500/10 text-rose-500"
                      )}
                    >
                      {e.status === "sent" ? t("emails.status_sent") : t("emails.status_failed")}
                    </span>
                  </td>
                  <td>
                    {e.opened_at ? (
                      <span
                        className="inline-flex items-center gap-1 text-xs font-medium text-emerald-500"
                        title={`${e.opened_at}${e.open_count > 1 ? ` · ${e.open_count}×` : ""}`}
                      >
                        <Eye size={13} />
                        {t("emails.opened")}
                        {e.open_count > 1 && ` ·${e.open_count}×`}
                      </span>
                    ) : (
                      <span className="inline-flex items-center gap-1 text-xs text-fg-subtle">
                        <EyeOff size={13} />
                        {t("emails.not_opened")}
                      </span>
                    )}
                  </td>
                  <td className="whitespace-nowrap text-xs text-fg-subtle">{e.sent_at}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </div>
  );
}
