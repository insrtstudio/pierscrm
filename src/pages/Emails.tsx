import { useEffect, useRef, useState } from "react";
import { useInfiniteQuery, useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { useNavigate, useSearchParams } from "react-router-dom";
import { NewResidencyModal } from "./Residency";
import { useVirtualizer } from "@tanstack/react-virtual";
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
  Clock,
  X,
  Inbox,
  Reply,
  ShieldBan,
  Users,
  RotateCcw,
} from "lucide-react";
import clsx from "clsx";
import {
  applyOpens,
  deleteCampaign,
  deleteTemplate,
  dismissFollowup,
  listDismissedFollowups,
  restoreFollowup,
  getContact,
  getSetting,
  listArtists,
  listCampaigns,
  listContacts,
  listEmailsPage,
  listFollowups,
  listTemplates,
  saveCampaign,
  saveTemplate,
  sendEmail,
  addSuppression,
  listSuppressions,
  removeSuppression,
  replySyncStatus,
  setSetting,
  listCampaignAudience,
  removeFromCampaign,
} from "../lib/api";
import type { Campaign, Contact, EmailFilter, Template } from "../lib/types";
import { CAMPAIGN_STATUSES, SUPPRESSION_REASONS } from "../lib/types";
import { PageHeader, EmptyState } from "../components/Layout";
import { Modal, Field, useToast, useConfirm, useSubmit, errorMessage } from "../components/ui";
import { useAppIntent, useDebounced } from "../lib/hooks";
import { runReplySync } from "../components/ReplySync";
import { BulkSend } from "./BulkSend";
import {
  ComposeModal,
  invalidateAfterSend,
  openCampaigns,
  QuotaControl,
  renderTemplate,
  resolveArtistName,
  sendErrorMessage,
  SuppressedWarning,
  unresolvedVars,
} from "../components/ComposeModal";
import { daysUntil, formatDateTime } from "../lib/format";

const LINK_FIELDS = ["mix", "listen", "onepager", "epk"] as const;

type Tab = "compose" | "bulk" | "followups" | "campaigns" | "templates" | "log" | "guardrails";

const CAMPAIGN_COLORS = [
  "#ff5a1f", "#f2b880", "#6f7782", "#3fb27f", "#d9430f", "#8b929c", "#4a4744", "#ffa66b",
];

const CAMPAIGN_STATUS_STYLE: Record<string, string> = {
  active: "bg-ok/10 text-ok",
  scheduled: "bg-chrome-dark/10 text-chrome-dark",
  done: "bg-fg-faint/10 text-fg-faint",
  archived: "bg-fg-faint/10 text-fg-faint",
};

/// The former Emails page is split in two sections of the nav:
/// "campaigns" (campaigns + bulk send) and "outbox" (follow-ups, compose,
/// history, templates). Both render this component with their own tabs.
const SECTIONS: Record<"campaigns" | "outbox", Tab[]> = {
  campaigns: ["campaigns", "bulk"],
  outbox: ["followups", "compose", "log", "templates", "guardrails"],
};

export function Emails({ section = "outbox" }: { section?: "campaigns" | "outbox" }) {
  const { t } = useTranslation();
  const tabs = SECTIONS[section];
  // `?tab=` lets other pages deep-link (Agenda › Actions opens the bulk send).
  const [params] = useSearchParams();
  const [selTab, setTab] = useState<Tab>(() => (params.get("tab") as Tab) || tabs[0]);
  // /campaigns and /outbox render this same component at the same spot, so React
  // keeps the state across them: a tab of the other section rendered a blank page.
  const tab = tabs.includes(selTab) ? selTab : tabs[0];
  const tabLabel = (tb: Tab) => (tb === "bulk" ? t("bulk.tab") : t(`emails.tab_${tb}`));

  return (
    <div>
      <PageHeader title={t(`emails.title_${section}`)} subtitle={t(`emails.subtitle_${section}`)} />
      <div className="px-page py-6">
        <div className="segmented mb-6">
          {tabs.map((tb) => (
            <button
              key={tb}
              data-active={tab === tb}
              onClick={() => setTab(tb)}
              className="segmented-item"
            >
              {tabLabel(tb)}
            </button>
          ))}
        </div>

        {tab === "compose" && <ComposeTab />}
        {tab === "bulk" && <BulkSend />}
        {tab === "followups" && <FollowupsTab />}
        {tab === "campaigns" && <CampaignsTab />}
        {tab === "templates" && <TemplatesTab />}
        {tab === "log" && <LogTab />}
        {tab === "guardrails" && <GuardrailsTab />}
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
  const confirm = useConfirm();
  const { data: campaigns = [] } = useQuery({ queryKey: ["campaigns"], queryFn: listCampaigns });
  const { data: artists = [] } = useQuery({ queryKey: ["artists"], queryFn: listArtists });
  // Same cache entry as Bulk send's unfiltered audience.
  const { data: allContacts = [] } = useQuery({
    queryKey: ["contacts", "all", "all", ""],
    queryFn: () => listContacts({ category: "all", status: "all", search: "" }),
  });
  const [to, setTo] = useState("");
  const [subject, setSubject] = useState("");
  const [body, setBody] = useState("");
  const [campaignId, setCampaignId] = useState<number | "">("");
  const [links, setLinks] = useState<Record<string, string>>({});
  const [sending, setSending] = useState(false);
  const [templateId, setTemplateId] = useState("");

  const linkVars = Object.fromEntries(
    Object.entries(links).filter(([, v]) => v.trim())
  );

  // A typed address that belongs to a pipeline contact fills its variables and
  // links the email to it (the backend also matches by address).
  const addr = to.trim().toLowerCase();
  const contact: Contact | null =
    (addr && allContacts.find((c) => c.email?.trim().toLowerCase() === addr)) || null;
  const campaign = campaigns.find((c) => c.id === campaignId) ?? null;
  const ctx = { campaign, artistName: resolveArtistName(artists, campaign, contact) };
  const renderedSubject = renderTemplate(subject, contact, linkVars, ctx);
  const renderedBody = renderTemplate(body, contact, linkVars, ctx);
  const missing = unresolvedVars(renderedSubject + "\n" + renderedBody);

  const doSend = async () => {
    if (missing.length && !confirm(t("emails.unresolved_confirm", { vars: missing.join(", ") })))
      return;
    setSending(true);
    try {
      const res = await sendEmail({
        contact_id: contact?.id ?? null,
        campaign_id: campaignId === "" ? null : campaignId,
        to: to.trim(),
        subject: renderedSubject,
        body: renderedBody,
      });
      if (res.ok) {
        toast(t("emails.sent_ok"), "ok");
        setTo("");
        setSubject("");
        setBody("");
        setLinks({});
        setTemplateId("");
        invalidateAfterSend(qc);
      } else {
        if (res.code === "bounced") invalidateAfterSend(qc);
        toast(await sendErrorMessage(t, res), "error");
      }
    } catch (e: any) {
      toast(`${t("emails.send_failed")}: ${e?.toString?.() ?? e}`, "error");
    } finally {
      setSending(false);
    }
  };

  return (
    <div className="card max-w-4xl space-y-3.5 p-6">
      <div className="grid grid-cols-2 gap-4">
        <Field label={t("emails.template")}>
          <select
            className="input"
            value={templateId}
            onChange={(e) => {
              setTemplateId(e.target.value);
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
            {openCampaigns(campaigns).map((c) => (
              <option key={c.id} value={c.id}>
                {c.name}
              </option>
            ))}
          </select>
        </Field>
      </div>
      <Field label={t("emails.to")}>
        <input className="input" value={to} onChange={(e) => setTo(e.target.value)} />
        <SuppressedWarning addr={to} />
        {contact && (
          <p className="mt-1 text-2xs text-fg-subtle">
            {t("emails.linked_contact", { name: contact.name })}
          </p>
        )}
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
      <div className="panel p-3">
        <div className="text-2xs font-semibold uppercase tracking-wide text-fg-subtle">
          {t("bulk.links")}
        </div>
        <div className="mt-2 grid grid-cols-2 gap-2">
          {LINK_FIELDS.map((k) => (
            <label key={k} className="block">
              <span className="mb-1 flex items-center justify-between text-2xs text-fg-subtle">
                {t(`bulk.link_${k}`)}
                <code className="text-fg-faint">{`{{${k}}}`}</code>
              </span>
              <input
                className="input py-1.5 text-xs"
                placeholder="https://…"
                value={links[k] ?? ""}
                onChange={(e) => setLinks((l) => ({ ...l, [k]: e.target.value }))}
              />
            </label>
          ))}
        </div>
      </div>
      {/\{\{\s*\w+\s*\}\}/.test(subject + body) && (
        <div className="panel p-3">
          <div className="mb-1 text-2xs font-semibold uppercase tracking-wide text-fg-subtle">
            {t("emails.sample_contact")} {contact?.name ?? "·"}
          </div>
          <div className="text-sm font-semibold">{renderedSubject}</div>
          <div className="mt-1 whitespace-pre-wrap text-xs text-fg-subtle">{renderedBody}</div>
          {missing.length > 0 && (
            <p className="mt-2 text-2xs font-medium text-danger">
              {t("emails.unresolved", { vars: missing.map((v) => `{{${v}}}`).join(", ") })}
            </p>
          )}
        </div>
      )}
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

// ---------------- Follow-ups (booking cadence) ----------------

function FollowupsTab() {
  const { t } = useTranslation();
  const qc = useQueryClient();
  const { data: followups = [] } = useQuery({ queryKey: ["followups"], queryFn: listFollowups });
  const toast = useToast();
  // The full contact, so {{name}} (promoter), {{date}}, category... render as in
  // a normal send (a hand-built stub used to drop them).
  const [relance, setRelance] = useState<Contact | null>(null);
  const openRelance = async (id: number) => {
    try {
      const c = await getContact(id);
      if (c) setRelance(c);
    } catch (e) {
      toast(errorMessage(e), "error");
    }
  };
  const dismiss = useMutation({
    mutationFn: (id: number) => dismissFollowup(id),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ["followups"] });
      qc.invalidateQueries({ queryKey: ["followups-dismissed"] });
      qc.invalidateQueries({ queryKey: ["contacts"] });
    },
  });
  // "Stop following up" is reversible: dismissed contacts are listed below.
  const { data: dismissed = [] } = useQuery({
    queryKey: ["followups-dismissed"],
    queryFn: listDismissedFollowups,
  });
  const [showDismissed, setShowDismissed] = useState(false);
  const restore = useMutation({
    mutationFn: (id: number) => restoreFollowup(id),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ["followups"] });
      qc.invalidateQueries({ queryKey: ["followups-dismissed"] });
      qc.invalidateQueries({ queryKey: ["contacts"] });
      toast(t("emails.followup_restored"), "ok");
    },
    onError: (e) => toast(errorMessage(e), "error"),
  });

  return (
    <div className="space-y-4">
      <p className="text-2xs text-fg-subtle">{t("emails.followups_hint")}</p>
      {followups.length === 0 ? (
        <div className="card">
          <EmptyState icon={Clock} title={t("emails.followups_empty")} />
        </div>
      ) : (
        <div className="card max-h-[calc(100vh-340px)] overflow-auto">
          <table className="tbl">
            <thead>
              <tr>
                <th>{t("contacts.name")}</th>
                <th>{t("contacts.venue")}</th>
                <th className="text-right">{t("emails.days_since")}</th>
                <th>{t("emails.opened")}</th>
                <th></th>
              </tr>
            </thead>
            <tbody>
              {followups.map((f) => (
                <tr key={f.contact_id}>
                  <td className="font-semibold">{f.name}</td>
                  <td className="text-fg-subtle">{f.venue || f.area || "·"}</td>
                  <td className="text-right tabular">
                    <span className={clsx("font-bold", f.days_since >= 14 && "text-accent")}>
                      {t("emails.days_ago", { count: f.days_since })}
                    </span>
                  </td>
                  <td>
                    {f.opened ? (
                      <span className="badge bg-ok/15 text-ok">{t("emails.opened_yes")}</span>
                    ) : (
                      <span className="badge bg-muted text-fg-subtle">{t("emails.opened_no")}</span>
                    )}
                  </td>
                  <td className="text-right">
                    <div className="flex justify-end gap-1">
                      {f.email && (
                        <button className="btn-outline py-1.5" onClick={() => openRelance(f.contact_id)}>
                          <Mail size={13} />
                          {t("emails.followup_do")}
                        </button>
                      )}
                      <button
                        className="btn-ghost px-2 py-1.5"
                        aria-label={t("emails.followup_stop")}
                        title={t("emails.followup_stop")}
                        onClick={() => dismiss.mutate(f.contact_id)}
                      >
                        <X size={15} />
                      </button>
                    </div>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
      {dismissed.length > 0 && (
        <div className="card p-3">
          <button
            className="btn-ghost px-2 py-1 text-2xs"
            aria-expanded={showDismissed}
            onClick={() => setShowDismissed((v) => !v)}
          >
            {t("emails.followups_dismissed", { count: dismissed.length })}
          </button>
          {showDismissed && (
            <ul className="mt-2 divide-y divide-border">
              {dismissed.map((d) => (
                <li key={d.contact_id} className="flex items-center justify-between gap-2 py-1.5">
                  <span className="min-w-0 truncate text-xs">
                    <span className="font-semibold">{d.name}</span>
                    <span className="text-fg-subtle"> · {d.venue || d.area || "·"}</span>
                  </span>
                  <button
                    className="btn-outline shrink-0 py-1 text-2xs"
                    disabled={restore.isPending}
                    onClick={() => restore.mutate(d.contact_id)}
                  >
                    <RotateCcw size={12} />
                    {t("emails.followup_restore")}
                  </button>
                </li>
              ))}
            </ul>
          )}
        </div>
      )}
      <ComposeModal
        open={!!relance}
        onClose={() => {
          setRelance(null);
          qc.invalidateQueries({ queryKey: ["followups"] });
        }}
        contact={relance}
      />
    </div>
  );
}

function CampaignsTab() {
  const { t } = useTranslation();
  const qc = useQueryClient();
  const confirm = useConfirm();
  const { data: campaigns = [] } = useQuery({ queryKey: ["campaigns"], queryFn: listCampaigns });
  const { data: artists = [] } = useQuery({ queryKey: ["artists"], queryFn: listArtists });
  const [editing, setEditing] = useState<Campaign | null>(null);
  const [audienceOf, setAudienceOf] = useState<Campaign | null>(null);
  const [newResidency, setNewResidency] = useState(false);
  const navigate = useNavigate();
  // Menu "Nouvelle campagne" lands here (Campaigns is the section's first tab).
  useAppIntent("new:campaign", () =>
    setEditing({ name: "", status: "active", color: CAMPAIGN_COLORS[0] } as Campaign)
  );

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
      <button className="btn-outline ml-2" onClick={() => setNewResidency(true)}>
        <Plus size={16} />
        {t("residency.new")}
      </button>

      {campaigns.length === 0 ? (
        <div className="card">
          <EmptyState icon={Megaphone} title={t("campaigns.no_campaigns")} />
        </div>
      ) : (
        <div className="grid grid-cols-[repeat(auto-fill,minmax(340px,1fr))] gap-4">
          {campaigns.map((c) => {
            const rate = c.sent_count ? Math.round((c.opened_count / c.sent_count) * 100) : 0;
            // Calendar days: the old ms diff rounded "tomorrow, before noon" to 0.
            const dd = daysUntil(c.target_date);
            return (
              <div
                key={c.id}
                className={clsx("card group overflow-hidden p-5", c.kind === "residency" && "cursor-pointer hover:border-fg-subtle")}
                // A residency campaign lives on its own page.
                onClick={c.kind === "residency" ? () => navigate(`/campaigns/${c.id}`) : undefined}
              >
                <div className="flex items-start justify-between gap-2">
                  <div className="flex items-center gap-2 min-w-0">
                    <span
                      className="h-3 w-3 shrink-0 rounded-full"
                      style={{ background: c.color ?? "#ff5a1f" }}
                    />
                    <span className="truncate font-semibold">{c.name}</span>
                    {c.kind === "residency" && (
                      <span className="badge shrink-0 bg-accent-soft text-accent-ink">{t("residency.badge")}</span>
                    )}
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

                <div className="mt-4 grid grid-cols-4 gap-2 border-t border-border pt-3">
                  <Stat label={t("campaigns.planned")} value={c.planned_count ?? 0} />
                  <Stat label={t("campaigns.sent")} value={c.sent_count} />
                  <Stat label={t("campaigns.open_rate")} value={`${rate}%`} accent />
                  <div title={t("campaigns.replied_hint")}>
                    <Stat label={t("campaigns.replied")} value={c.replied_count ?? 0} />
                  </div>
                </div>

                <div className="mt-3 flex items-center justify-end gap-1" onClick={(e) => e.stopPropagation()}>
                  <button
                    className="btn-ghost mr-auto px-2 py-1.5 text-xs"
                    onClick={() => setAudienceOf(c)}
                  >
                    <Users size={14} />
                    {t("campaigns.audience")} · {c.audience_count ?? 0}
                  </button>
                  <div className="flex gap-1 opacity-0 transition-opacity group-hover:opacity-100 group-focus-within:opacity-100">
                  <button className="btn-ghost px-2 py-1.5" onClick={() => setEditing(c)}>
                    <Pencil size={14} />
                  </button>
                  <button
                    className="btn-ghost px-2 py-1.5 text-danger"
                    onClick={() => confirm(t("common.confirm_delete")) && delMut.mutate(c.id!)}
                  >
                    <Trash2 size={14} />
                  </button>
                  </div>
                </div>
              </div>
            );
          })}
        </div>
      )}

      {audienceOf && <AudienceModal campaign={audienceOf} onClose={() => setAudienceOf(null)} />}
      {newResidency && <NewResidencyModal onClose={() => setNewResidency(false)} />}

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
      <div className={clsx("font-condensed text-2xl tabular", accent && "text-accent-2")}>
        {value ?? 0}
      </div>
      <div className="eyebrow mt-1 text-[10px]">{label}</div>
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
  const [save, saving] = useSubmit(async () => {
    if (!form.name.trim()) {
      toast(t("campaigns.name") + " " + t("common.required"), "error");
      return;
    }
    await saveCampaign(form);
    toast(t("common.save"), "ok");
    onSaved();
  });
  return (
    <Modal
      open
      onClose={onClose}
      width="max-w-lg"
      title={campaign.id ? t("campaigns.edit") : t("campaigns.new")}
    >
      <div className="space-y-3.5">
        <div className="grid grid-cols-2 gap-4">
          <Field label={t("campaigns.name")}>
            <input
              className="input"
              value={form.name}
              onChange={(e) => set("name", e.target.value)}
            />
          </Field>
          <Field label={t("campaigns.event_name")}>
            <input
              className="input"
              value={form.event_name ?? ""}
              placeholder={t("campaigns.event_name_ph")}
              onChange={(e) => set("event_name", e.target.value)}
            />
          </Field>
        </div>
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
              <option value="">·</option>
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
          <button className="btn-primary" onClick={() => save()} disabled={saving}>
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
      <div className="grid grid-cols-[repeat(auto-fill,minmax(360px,1fr))] gap-3">
        {templates.map((tpl) => (
          <div key={tpl.id} className="card group p-4">
            <div className="flex items-start justify-between">
              <div className="min-w-0">
                <div className="text-sm font-semibold">{tpl.name}</div>
                <div className="mt-0.5 truncate text-xs text-fg-subtle">{tpl.subject}</div>
              </div>
              <div className="flex gap-1 opacity-0 transition-opacity group-hover:opacity-100 group-focus-within:opacity-100">
                <button className="btn-ghost px-2 py-1.5" onClick={() => setEditing(tpl)}>
                  <Pencil size={14} />
                </button>
                <button
                  className="btn-ghost px-2 py-1.5 text-danger"
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
  const toast = useToast();
  const [form, setForm] = useState<Template>({ ...template });
  const [save, saving] = useSubmit(async () => {
    if (!form.name.trim()) {
      toast(t("emails.template_name") + " " + t("common.required"), "error");
      return;
    }
    await saveTemplate(form);
    onSaved();
  });
  return (
    <Modal
      open
      onClose={onClose}
      title={template.id ? t("emails.edit_template") : t("emails.new_template")}
      width="max-w-2xl"
    >
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
          <button className="btn-primary" onClick={() => save()} disabled={saving}>
            {t("common.save")}
          </button>
        </div>
      </div>
    </Modal>
  );
}


// ---------------- Campaign audience ----------------

const AUDIENCE_STYLE: Record<string, string> = {
  planned: "bg-chrome-dark/10 text-chrome-dark",
  sent: "bg-accent-soft text-accent",
  replied: "bg-ok/10 text-ok",
  bounced: "bg-danger/10 text-danger",
};

/// A campaign's recipients (campaign_contacts) with their state. Rows are
/// virtualised: an audience can hold the whole pipeline.
function AudienceModal({ campaign, onClose }: { campaign: Campaign; onClose: () => void }) {
  const { t } = useTranslation();
  const qc = useQueryClient();
  const { data: rows = [], isLoading } = useQuery({
    queryKey: ["campaigns", "audience", campaign.id],
    queryFn: () => listCampaignAudience(campaign.id!),
  });
  const remove = useMutation({
    mutationFn: (contactId: number) => removeFromCampaign(campaign.id!, [contactId]),
    onSuccess: () => qc.invalidateQueries({ queryKey: ["campaigns"] }),
  });
  const counts = rows.reduce<Record<string, number>>((acc, r) => {
    acc[r.state] = (acc[r.state] ?? 0) + 1;
    return acc;
  }, {});
  const scrollRef = useRef<HTMLDivElement | null>(null);
  const virt = useVirtualizer({
    count: rows.length,
    getScrollElement: () => scrollRef.current,
    estimateSize: () => 44,
    overscan: 10,
  });

  return (
    <Modal
      open
      onClose={onClose}
      width="max-w-2xl"
      title={t("campaigns.audience_title", { name: campaign.name })}
    >
      <div className="mb-3 flex flex-wrap gap-2 text-2xs">
        {(["planned", "sent", "replied", "bounced"] as const).map((st) => (
          <span key={st} className={clsx("badge", AUDIENCE_STYLE[st])}>
            {t(`campaigns.aud_${st}`)} · {counts[st] ?? 0}
          </span>
        ))}
      </div>
      {!isLoading && rows.length === 0 ? (
        <EmptyState icon={Users} title={t("campaigns.audience_empty")} />
      ) : (
        <div ref={scrollRef} className="max-h-[55vh] overflow-y-auto rounded-lg border border-border">
          <div style={{ height: virt.getTotalSize(), position: "relative" }}>
            {virt.getVirtualItems().map((v) => {
              const r = rows[v.index];
              return (
                <div
                  key={r.id}
                  className="absolute left-0 right-0 flex items-center gap-3 border-b border-border/50 px-3"
                  style={{ top: v.start, height: v.size }}
                >
                  <div className="min-w-0 flex-1">
                    <div className="truncate text-xs font-medium">{r.name}</div>
                    <div className="truncate text-2xs text-fg-subtle">{r.email || "·"}</div>
                  </div>
                  {r.suppressed && (
                    <span className="badge bg-danger/10 text-danger">{t("campaigns.aud_suppressed")}</span>
                  )}
                  <span className={clsx("badge", AUDIENCE_STYLE[r.state] ?? "bg-muted text-fg-subtle")}>
                    {t(`campaigns.aud_${r.state}`)}
                  </span>
                  <button
                    className="btn-ghost px-1.5 py-1"
                    title={t("campaigns.remove")}
                    aria-label={t("campaigns.remove")}
                    onClick={() => remove.mutate(r.id!)}
                  >
                    <X size={13} />
                  </button>
                </div>
              );
            })}
          </div>
        </div>
      )}
    </Modal>
  );
}

// ---------------- Log ----------------

/// The manual "sync replies" action with its toasts (Log tab and Guardrails).
function useReplySyncAction(): [() => Promise<void>, boolean] {
  const { t } = useTranslation();
  const toast = useToast();
  const qc = useQueryClient();
  const [busy, setBusy] = useState(false);
  const run = async () => {
    setBusy(true);
    try {
      const r = await runReplySync(qc);
      if (r.skipped) toast(t("emails.replies_busy"), "ok");
      else toast(t("emails.replies_done", { ...r }), "ok");
    } catch (e) {
      qc.invalidateQueries({ queryKey: ["reply_sync_status"] });
      toast(`${t("emails.replies_failed")}: ${errorMessage(e)}`, "error");
    } finally {
      setBusy(false);
    }
  };
  return [run, busy];
}

const LOG_PAGE = 100;

function LogTab() {
  const { t } = useTranslation();
  const toast = useToast();
  const qc = useQueryClient();
  const [syncing, setSyncing] = useState(false);
  const [syncRepliesNow, syncingReplies] = useReplySyncAction();
  const { data: campaigns = [] } = useQuery({ queryKey: ["campaigns"], queryFn: listCampaigns });
  const campaignOf = (id?: number | null) => (id ? campaigns.find((c) => c.id === id) : undefined);

  // Filters (search debounced so typing does not fire one query per key).
  const [campaignId, setCampaignId] = useState<number | "">("");
  const [status, setStatus] = useState<"" | "sent" | "failed">("");
  const [opened, setOpened] = useState<"" | "yes" | "no">("");
  const [replied, setReplied] = useState<"" | "yes" | "no">("");
  const [search, setSearch] = useState("");
  const dSearch = useDebounced(search, 300);
  const filter: EmailFilter = {
    campaign_id: campaignId === "" ? null : campaignId,
    status: status || null,
    opened: opened === "" ? null : opened === "yes",
    replied: replied === "" ? null : replied === "yes",
    search: dSearch.trim() || null,
  };

  // Under the "emails" key, so every send and sync refreshes it.
  const pages = useInfiniteQuery({
    queryKey: ["emails", "page", filter],
    queryFn: ({ pageParam }) => listEmailsPage({ ...filter, limit: LOG_PAGE, offset: pageParam }),
    initialPageParam: 0,
    getNextPageParam: (last, all) => {
      const loaded = all.reduce((n, p) => n + p.rows.length, 0);
      return loaded < last.total ? loaded : undefined;
    },
  });
  const emails = pages.data?.pages.flatMap((p) => p.rows) ?? [];
  const total = pages.data?.pages[0]?.total ?? 0;

  const syncOpens = async () => {
    setSyncing(true);
    try {
      const base = (await getSetting("tracking_base_url"))?.trim().replace(/\/$/, "");
      if (!base) {
        toast(t("emails.no_tracking"), "error");
        return;
      }
      const res = await fetch(`${base}/opens.json`, { cache: "no-store" });
      // A 404/500 page is not JSON: report the status rather than a parse error.
      if (!res.ok) throw new Error(`HTTP ${res.status}`);
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
      toast(`${t("emails.sync_failed")}: ${errorMessage(e)}`, "error");
    } finally {
      setSyncing(false);
    }
  };


  const sel = "input w-auto py-1.5 text-xs";
  return (
    <div className="space-y-3">
      <div className="flex flex-wrap items-center gap-2">
        <input
          className="input w-56 py-1.5 text-xs"
          placeholder={t("emails.search_log")}
          value={search}
          onChange={(e) => setSearch(e.target.value)}
        />
        <select
          className={sel}
          value={campaignId}
          onChange={(e) => setCampaignId(e.target.value ? Number(e.target.value) : "")}
        >
          <option value="">{t("campaigns.all_campaigns")}</option>
          {campaigns.map((c) => (
            <option key={c.id} value={c.id}>
              {c.name}
            </option>
          ))}
        </select>
        <select className={sel} value={status} onChange={(e) => setStatus(e.target.value as any)}>
          <option value="">{t("emails.filter_status_all")}</option>
          <option value="sent">{t("emails.status_sent")}</option>
          <option value="failed">{t("emails.status_failed")}</option>
        </select>
        <select className={sel} value={opened} onChange={(e) => setOpened(e.target.value as any)}>
          <option value="">{t("emails.filter_opened_all")}</option>
          <option value="yes">{t("emails.filter_opened_yes")}</option>
          <option value="no">{t("emails.filter_opened_no")}</option>
        </select>
        <select className={sel} value={replied} onChange={(e) => setReplied(e.target.value as any)}>
          <option value="">{t("emails.filter_replied_all")}</option>
          <option value="yes">{t("emails.filter_replied_yes")}</option>
          <option value="no">{t("emails.filter_replied_no")}</option>
        </select>
        <div className="ml-auto flex gap-2">
          <button className="btn-outline" onClick={syncRepliesNow} disabled={syncingReplies}>
            <Inbox size={14} className={syncingReplies ? "animate-pulse" : ""} />
            {syncingReplies ? t("emails.syncing_replies") : t("emails.sync_replies")}
          </button>
          <button className="btn-outline" onClick={syncOpens} disabled={syncing}>
            <RefreshCw size={14} className={syncing ? "animate-spin" : ""} />
            {syncing ? t("emails.syncing") : t("emails.sync_opens")}
          </button>
        </div>
      </div>
      {!pages.isLoading && emails.length === 0 ? (
        <div className="card">
          <EmptyState icon={Mail} title={t("emails.log_empty")} />
        </div>
      ) : (
        <div className="card max-h-[calc(100vh-300px)] overflow-auto">
          <table className="tbl">
            <thead>
              <tr>
                <th>{t("emails.to")}</th>
                <th>{t("emails.subject")}</th>
                <th>{t("emails.campaign")}</th>
                <th>{t("contacts.status")}</th>
                <th>{t("emails.open_col")}</th>
                <th>{t("emails.reply_col")}</th>
                <th>{t("contacts.date")}</th>
              </tr>
            </thead>
            <tbody>
              {emails.map((e) => (
                <tr key={e.id}>
                  <td>{e.to_addr}</td>
                  <td className="max-w-[280px] truncate text-fg-subtle">{e.subject}</td>
                  <td className="max-w-[160px] truncate text-xs text-fg-subtle">
                    {campaignOf(e.campaign_id) ? (
                      <span className="inline-flex items-center gap-1.5">
                        <span
                          className="h-2 w-2 shrink-0 rounded-full"
                          style={{ background: campaignOf(e.campaign_id)!.color ?? "#ff5a1f" }}
                        />
                        {campaignOf(e.campaign_id)!.name}
                      </span>
                    ) : (
                      "·"
                    )}
                  </td>
                  <td>
                    <span
                      title={e.error ?? undefined}
                      className={clsx(
                        "badge",
                        e.status === "sent"
                          ? "bg-ok/10 text-ok"
                          : "bg-danger/10 text-danger"
                      )}
                    >
                      {e.status === "sent" ? t("emails.status_sent") : t("emails.status_failed")}
                    </span>
                  </td>
                  <td>
                    {e.opened_at ? (
                      <span
                        className="inline-flex items-center gap-1 text-xs font-medium text-ok"
                        title={`${formatDateTime(e.opened_at)}${e.open_count > 1 ? ` · ${e.open_count}×` : ""}`}
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
                  <td>
                    {e.replied_at ? (
                      <span
                        className="inline-flex items-center gap-1 text-xs font-medium text-ok"
                        title={formatDateTime(e.replied_at)}
                      >
                        <Reply size={13} />
                        {t("emails.replied")}
                      </span>
                    ) : (
                      <span className="text-xs text-fg-faint">·</span>
                    )}
                  </td>
                  <td className="whitespace-nowrap text-xs text-fg-subtle">{formatDateTime(e.sent_at)}</td>
                </tr>
              ))}
            </tbody>
          </table>
          <div className="flex items-center justify-between border-t border-border px-4 py-2 text-2xs text-fg-subtle">
            <span>{t("emails.log_count", { shown: emails.length, total })}</span>
            {pages.hasNextPage && (
              <button
                className="btn-ghost px-2 py-1 text-xs"
                disabled={pages.isFetchingNextPage}
                onClick={() => pages.fetchNextPage()}
              >
                {t("emails.load_more")}
              </button>
            )}
          </div>
        </div>
      )}
    </div>
  );
}

// ---------------- Guardrails ----------------

/// Daily cap, reply detection (IMAP) and the suppression list.
function GuardrailsTab() {
  const { t } = useTranslation();
  return (
    <div className="grid grid-cols-1 gap-5 xl:grid-cols-[minmax(0,420px)_1fr]">
      <div className="space-y-5">
        <div className="card space-y-2 p-5">
          <SectionTitle>{t("emails.quota_title")}</SectionTitle>
          <p className="text-2xs text-fg-subtle">{t("emails.quota_hint")}</p>
          <QuotaControl />
        </div>
        <ReplyDetectionCard />
      </div>
      <SuppressionsCard />
    </div>
  );
}

function SectionTitle({ children }: { children: React.ReactNode }) {
  return <div className="text-xs font-semibold uppercase tracking-wide text-fg-subtle">{children}</div>;
}

function ReplyDetectionCard() {
  const { t } = useTranslation();
  const toast = useToast();
  const qc = useQueryClient();
  const { data: st } = useQuery({ queryKey: ["reply_sync_status"], queryFn: replySyncStatus });
  const { data: auto } = useQuery({
    queryKey: ["settings", "imap_auto"],
    queryFn: () => getSetting("imap_auto"),
  });
  const [host, setHost] = useState("");
  useEffect(() => {
    if (st) setHost(st.derived ? "" : st.host);
  }, [st?.host, st?.derived]);
  const [syncNow, busy] = useReplySyncAction();

  const saveHost = async () => {
    try {
      await setSetting("imap_host", host.trim());
      qc.invalidateQueries({ queryKey: ["reply_sync_status"] });
      toast(t("emails.imap_saved"), "ok");
    } catch (e) {
      toast(errorMessage(e), "error");
    }
  };
  const toggleAuto = async (on: boolean) => {
    await setSetting("imap_auto", on ? "1" : "0").catch(() => {});
    qc.invalidateQueries({ queryKey: ["settings", "imap_auto"] });
  };

  return (
    <div className="card space-y-3 p-5">
      <SectionTitle>{t("emails.imap_title")}</SectionTitle>
      <p className="text-2xs text-fg-subtle">{t("emails.imap_hint")}</p>
      <Field label={t("emails.imap_host")}>
        <input
          className="input"
          value={host}
          placeholder={st?.derived ? st.host : "imap.example.com"}
          onChange={(e) => setHost(e.target.value)}
          onBlur={() => st && host.trim() !== (st.derived ? "" : st.host) && saveHost()}
          onKeyDown={(e) => e.key === "Enter" && saveHost()}
        />
        {st?.derived && (
          <p className="mt-1 text-2xs text-fg-subtle">
            {t("emails.imap_host_derived", { host: st.host || "·" })}
          </p>
        )}
      </Field>
      <label className="flex items-center gap-2 text-xs text-fg-subtle">
        <input
          type="checkbox"
          className="accent-accent"
          checked={auto !== "0"}
          onChange={(e) => toggleAuto(e.target.checked)}
        />
        {t("emails.imap_auto")}
      </label>
      <div className="text-2xs text-fg-subtle">
        {st?.last_sync
          ? t("emails.imap_last", { date: formatDateTime(st.last_sync) })
          : t("emails.imap_never")}
        {st?.last_result && !st.last_error && ` · ${t("emails.replies_done", { ...st.last_result })}`}
      </div>
      {st?.last_error && (
        <p className="text-2xs font-medium text-danger">{t("emails.imap_error", { error: st.last_error })}</p>
      )}
      <p className="text-2xs text-fg-faint">{t("emails.list_unsub_note")}</p>
      <div className="flex justify-end">
        <button className="btn-outline" onClick={syncNow} disabled={busy}>
          <Inbox size={14} className={busy ? "animate-pulse" : ""} />
          {busy ? t("emails.syncing_replies") : t("emails.sync_replies")}
        </button>
      </div>
    </div>
  );
}

function SuppressionsCard() {
  const { t } = useTranslation();
  const toast = useToast();
  const qc = useQueryClient();
  const confirm = useConfirm();
  const { data: list = [] } = useQuery({ queryKey: ["suppressions"], queryFn: listSuppressions });
  const [address, setAddress] = useState("");
  const [reason, setReason] = useState<string>("manual");
  const [filter, setFilter] = useState("");
  const refresh = () => {
    qc.invalidateQueries({ queryKey: ["suppressions"] });
    qc.invalidateQueries({ queryKey: ["campaigns"] });
    qc.invalidateQueries({ queryKey: ["contacts"] });
    qc.invalidateQueries({ queryKey: ["followups"] });
  };
  const [add, adding] = useSubmit(async () => {
    if (!address.includes("@")) return;
    const n = await addSuppression(address, reason);
    toast(t("emails.supp_added", { count: n }), "ok");
    setAddress("");
    refresh();
  });
  const del = useMutation({
    mutationFn: (id: number) => removeSuppression(id),
    onSuccess: refresh,
  });
  const f = filter.trim().toLowerCase();
  const shown = f ? list.filter((s) => s.address.includes(f) || s.reason.includes(f)) : list;

  return (
    <div className="card flex flex-col overflow-hidden">
      <div className="space-y-3 border-b border-border p-5">
        <SectionTitle>
          <span className="inline-flex items-center gap-1.5">
            <ShieldBan size={13} /> {t("emails.supp_title")} · {list.length}
          </span>
        </SectionTitle>
        <p className="text-2xs text-fg-subtle">{t("emails.supp_hint")}</p>
        <div className="flex flex-wrap gap-2">
          <input
            className="input min-w-[220px] flex-1"
            placeholder={t("emails.supp_address_ph")}
            value={address}
            onChange={(e) => setAddress(e.target.value)}
            onKeyDown={(e) => e.key === "Enter" && add()}
          />
          <select
            className="input w-auto"
            aria-label={t("emails.supp_reason")}
            value={reason}
            onChange={(e) => setReason(e.target.value)}
          >
            {SUPPRESSION_REASONS.map((r) => (
              <option key={r} value={r}>
                {t(`emails.reason_${r}`)}
              </option>
            ))}
          </select>
          <button className="btn-primary" disabled={adding || !address.includes("@")} onClick={() => add()}>
            <Plus size={15} />
            {t("emails.supp_add")}
          </button>
        </div>
        {list.length > 10 && (
          <input
            className="input py-1.5 text-xs"
            placeholder={t("emails.supp_search")}
            value={filter}
            onChange={(e) => setFilter(e.target.value)}
          />
        )}
      </div>
      {shown.length === 0 ? (
        <EmptyState icon={ShieldBan} title={t("emails.supp_empty")} />
      ) : (
        <div className="max-h-[calc(100vh-420px)] overflow-auto">
          <table className="tbl">
            <tbody>
              {shown.map((s) => (
                <tr key={s.id}>
                  <td className="font-medium">{s.address}</td>
                  <td>
                    <span
                      className={clsx(
                        "badge",
                        s.reason === "bounce" ? "bg-danger/10 text-danger" : "bg-muted text-fg-subtle"
                      )}
                    >
                      {t(`emails.reason_${s.reason}`)}
                    </span>
                  </td>
                  <td className="max-w-[220px] truncate text-2xs text-fg-subtle" title={s.note ?? undefined}>
                    {s.note || ""}
                  </td>
                  <td className="whitespace-nowrap text-2xs text-fg-subtle">
                    {formatDateTime(s.created_at)}
                  </td>
                  <td className="text-right">
                    <button
                      className="btn-ghost px-2 py-1"
                      title={t("emails.supp_remove")}
                      aria-label={t("emails.supp_remove")}
                      onClick={() => confirm(t("emails.supp_remove") + " ?") && del.mutate(s.id)}
                    >
                      <Trash2 size={13} />
                    </button>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </div>
  );
}
