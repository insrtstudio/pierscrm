import { useEffect, useMemo, useState } from "react";
import { useQuery, useQueryClient, type QueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import type { TFunction } from "i18next";
import { Send, Link as LinkIcon, ShieldAlert, Gauge } from "lucide-react";
import { Modal, Field, useToast, useConfirm, errorMessage } from "./ui";
import {
  listArtists,
  listCampaigns,
  listEmails,
  listSuppressions,
  listTemplates,
  sendEmail,
  sendQuota,
  setSetting,
} from "../lib/api";
import type { Artist, Campaign, Contact, SendResult } from "../lib/types";

const LINK_FIELDS = ["mix", "listen", "onepager", "epk"] as const;
type LinkKey = (typeof LINK_FIELDS)[number];

/// Campaign / artist context for {{event}}, {{artist}}, {{target_date}}.
export type TemplateCtx = { campaign?: Campaign | null; artistName?: string };

/// Same variables, same fallbacks as the backend's bulk renderer (email.rs), so
/// what the preview shows is what gets sent. Without a contact, contact fields
/// stay as placeholders (they used to become "" and mail "something at  during").
export function renderTemplate(
  str: string,
  c?: Contact | null,
  links?: Record<string, string>,
  ctx?: TemplateCtx
): string {
  const map: Record<string, string> = {};
  if (c) {
    map.name = c.promoter?.trim() || c.name || "";
    map.venue = c.venue?.trim() || c.name || "";
    map.event = c.name || "";
    map.promoter = c.promoter || "";
    map.city = c.area || "";
    map.date = humanDate(c.date?.trim() || ctx?.campaign?.target_date || "");
  }
  if (ctx?.campaign?.event_name?.trim()) map.event = ctx.campaign.event_name;
  if (ctx?.campaign?.target_date?.trim()) map.target_date = humanDate(ctx.campaign.target_date);
  if (ctx?.artistName) map.artist = ctx.artistName;
  Object.assign(map, links || {});
  return str.replace(/\{\{\s*(\w+)\s*\}\}/g, (_, k) =>
    map[k] !== undefined ? map[k] : `{{${k}}}`
  );
}

const MONTHS = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

/// {{date}} / {{target_date}} for humans, exactly like the backend's
/// human_date (email.rs): ISO "2026-10-21" -> "21 Oct" (year added when not the
/// current one); free text is kept as typed.
export function humanDate(raw: string): string {
  const t = raw.trim();
  const m = /^(\d{4})-(\d{2})-(\d{2})(?:[T ].*)?$/.exec(t);
  if (!m) return t;
  const [y, mo, d] = [Number(m[1]), Number(m[2]), Number(m[3])];
  if (mo < 1 || mo > 12 || d < 1 || d > 31) return t;
  const base = `${d} ${MONTHS[mo - 1]}`;
  return y === new Date().getFullYear() ? base : `${base} ${y}`;
}

/// Placeholders still left after rendering (unknown variable, empty link field).
export function unresolvedVars(rendered: string): string[] {
  return [...new Set([...rendered.matchAll(/\{\{\s*(\w+)\s*\}\}/g)].map((m) => m[1]))];
}

/// Artist behind {{artist}}: the campaign's, else the contact's, else the only
/// artist of the roster (mirrors the backend fallback).
export function resolveArtistName(
  artists: Artist[],
  campaign?: Campaign | null,
  contact?: Contact | null
): string {
  const byId = (id?: number | null) => (id ? artists.find((a) => a.id === id)?.name : undefined);
  return (
    byId(campaign?.artist_id) ||
    byId(contact?.artist_id) ||
    (artists.length === 1 ? artists[0].name : "") ||
    ""
  );
}

/// Campaigns still worth attributing a send to.
export const openCampaigns = (cs: Campaign[]) =>
  cs.filter((c) => c.status !== "done" && c.status !== "archived");

/// Why a single send did not go out, in the UI language: the guardrails
/// (suppression list, daily cap, hard bounce) report a code, SMTP errors a text.
export async function sendErrorMessage(t: TFunction, res: SendResult): Promise<string> {
  if (res.code === "suppressed") return t("emails.err_suppressed");
  if (res.code === "bounced") return t("emails.err_bounced");
  if (res.code === "quota") {
    const q = await sendQuota().catch(() => null);
    return t("emails.err_quota", { sent: q?.sent_today ?? "?", cap: q?.cap ?? "?" });
  }
  return `${t("emails.send_failed")}: ${res.error ?? ""}`;
}

/// Suppression reason for an address, if it is on the list (for a warning
/// before sending; the backend blocks it anyway).
export function useSuppressionOf(addr: string): string | null {
  const { data: list = [] } = useQuery({ queryKey: ["suppressions"], queryFn: listSuppressions });
  const a = addr.trim().toLowerCase();
  if (!a) return null;
  return list.find((s) => s.address === a)?.reason ?? null;
}

/// Today's send quota (shared by Outbox and bulk send), under the "emails" key
/// so every send refreshes it.
export const quotaQuery = { queryKey: ["emails", "quota"], queryFn: sendQuota };

/// Daily cap: usage of the day, and an inline control to change the cap (the
/// `daily_send_cap` setting, enforced by the backend for every send path).
export function QuotaControl({ compact }: { compact?: boolean }) {
  const { t } = useTranslation();
  const qc = useQueryClient();
  const toast = useToast();
  const { data: q } = useQuery(quotaQuery);
  const [cap, setCap] = useState("");
  useEffect(() => {
    if (q) setCap(String(q.cap));
  }, [q?.cap]);
  const save = async () => {
    const n = Math.round(Number(cap));
    if (!Number.isFinite(n) || n < 1 || n > 5000) return;
    try {
      await setSetting("daily_send_cap", String(n));
      qc.invalidateQueries({ queryKey: ["emails", "quota"] });
      toast(t("emails.cap_saved"), "ok");
    } catch (e) {
      toast(errorMessage(e), "error");
    }
  };
  if (!q) return null;
  return (
    <div className={compact ? "flex items-center gap-2 text-2xs text-fg-subtle" : "space-y-2"}>
      <span className={compact ? "inline-flex items-center gap-1" : "flex items-center gap-1.5 text-xs"}>
        <Gauge size={13} className={q.remaining === 0 ? "text-danger" : undefined} />
        {compact
          ? t("bulk.quota", { remaining: q.remaining, cap: q.cap })
          : t("emails.quota_today", { sent: q.sent_today, cap: q.cap, remaining: q.remaining })}
      </span>
      <label className="inline-flex items-center gap-1.5">
        <span className={compact ? "" : "text-xs text-fg-subtle"}>
          {compact ? t("bulk.cap_label") : t("emails.cap_label")}
        </span>
        <input
          type="number"
          min={1}
          max={5000}
          className="input w-20 py-1 text-xs tabular"
          value={cap}
          onChange={(e) => setCap(e.target.value)}
          onBlur={() => String(q.cap) !== cap && save()}
          onKeyDown={(e) => e.key === "Enter" && save()}
        />
      </label>
    </div>
  );
}

/// Inline warning under a recipient field when the address is suppressed.
export function SuppressedWarning({ addr }: { addr: string }) {
  const { t } = useTranslation();
  const reason = useSuppressionOf(addr);
  if (!reason) return null;
  return (
    <p className="mt-1 inline-flex items-center gap-1 text-2xs font-medium text-danger">
      <ShieldAlert size={12} />
      {t("emails.suppressed_warning", { reason: t(`emails.reason_${reason}`) })}
    </p>
  );
}

/// Everything a send can change: the log, campaign stats, follow-ups, and the
/// contact pipeline as seen by Contacts, Discovery (contacted venues) and Home.
export function invalidateAfterSend(qc: QueryClient) {
  for (const key of [
    "emails",
    "suppressions",
    "campaigns",
    "followups",
    "contacts",
    "dashboard",
    "kpis",
    "vi_venues",
    "vi_venue_stats",
    "vi_venue_detail",
    "radar_list",
    "radar_stats",
  ])
    qc.invalidateQueries({ queryKey: [key] });
}

export function ComposeModal({
  open,
  onClose,
  contact,
  initialSubject,
  initialBody,
  initialCampaignId,
  onSent,
}: {
  open: boolean;
  onClose: () => void;
  contact?: Contact | null;
  /// Pre-filled draft (e.g. the reply the residency agent suggests). The
  /// user always reviews and sends it: nothing goes out by itself.
  initialSubject?: string;
  initialBody?: string;
  initialCampaignId?: number | null;
  onSent?: () => void;
}) {
  const { t } = useTranslation();
  const toast = useToast();
  const confirm = useConfirm();
  const qc = useQueryClient();
  const { data: templates } = useQuery({ queryKey: ["templates"], queryFn: listTemplates });
  const { data: campaigns = [] } = useQuery({ queryKey: ["campaigns"], queryFn: listCampaigns });
  const { data: artists = [] } = useQuery({ queryKey: ["artists"], queryFn: listArtists });
  // The contact's history: a follow-up defaults to the campaign of the last
  // email, so relances count in that campaign's stats.
  const { data: history } = useQuery({
    queryKey: ["emails", "contact", contact?.id],
    queryFn: () => listEmails(contact!.id!),
    enabled: open && !!contact?.id,
  });
  const [campaignId, setCampaignId] = useState<number | "">("");

  const [to, setTo] = useState("");
  const [subject, setSubject] = useState("");
  const [body, setBody] = useState("");
  const [tplId, setTplId] = useState<number | "">("");
  const [links, setLinks] = useState<Record<LinkKey, string>>({
    mix: "",
    listen: "",
    onepager: "",
    epk: "",
  });
  const [sending, setSending] = useState(false);

  useEffect(() => {
    if (open) {
      setTo(contact?.email ?? "");
      setSubject(initialSubject ?? "");
      setBody(initialBody ?? "");
      setTplId("");
      setLinks({ mix: "", listen: "", onepager: "", epk: "" });
      setCampaignId(initialCampaignId ?? "");
    }
    // Keyed on identity, not the object: callers pass inline literals, and a
    // parent re-render (refetch on focus) used to wipe the draft being typed.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open, contact?.id, contact?.email]);

  const lastCampaignId = history?.find((e) => e.status === "sent" && e.campaign_id)?.campaign_id;
  useEffect(() => {
    if (open && lastCampaignId && !initialCampaignId) setCampaignId(lastCampaignId);
  }, [open, lastCampaignId, initialCampaignId]);

  const applyTemplate = (id: number | "") => {
    setTplId(id);
    const tpl = templates?.find((x) => x.id === id);
    if (tpl) {
      // keep raw placeholders — they resolve live in the preview and at send
      setSubject(tpl.subject);
      setBody(tpl.body);
    }
  };

  const linkVars = useMemo(
    () => Object.fromEntries(Object.entries(links).filter(([, v]) => v.trim())),
    [links]
  );
  const campaign = campaigns.find((c) => c.id === campaignId) ?? null;
  const ctx = { campaign, artistName: resolveArtistName(artists, campaign, contact) };
  const renderedSubject = renderTemplate(subject, contact, linkVars, ctx);
  const renderedBody = renderTemplate(body, contact, linkVars, ctx);
  const missing = unresolvedVars(renderedSubject + "\n" + renderedBody);

  const canSend = useMemo(
    () => to.trim() && renderedSubject.trim() && renderedBody.trim() && !sending,
    [to, renderedSubject, renderedBody, sending]
  );

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
        invalidateAfterSend(qc);
        onSent?.();
        onClose();
      } else {
        // A hard bounce changed the suppression list and the log.
        if (res.code === "bounced") invalidateAfterSend(qc);
        toast(await sendErrorMessage(t, res), "error");
      }
    } catch (e: any) {
      toast(`${t("emails.send_failed")}: ${e?.toString?.() ?? e}`, "error");
    } finally {
      setSending(false);
    }
  };

  const hasPlaceholders = /\{\{\s*\w+\s*\}\}/.test(subject + body);

  return (
    <Modal open={open} onClose={onClose} title={t("contacts.compose")} width="max-w-2xl">
      <div className="space-y-3.5">
        <div className="grid grid-cols-2 gap-4">
          <Field label={t("emails.template")}>
            <select
              className="input"
              value={tplId}
              onChange={(e) => applyTemplate(e.target.value ? Number(e.target.value) : "")}
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
              {campaigns
                .filter((c) => c.id === campaignId || openCampaigns([c]).length)
                .map((c) => (
                  <option key={c.id} value={c.id}>
                    {c.name}
                  </option>
                ))}
            </select>
          </Field>
        </div>
        <Field label={t("emails.to")}>
          <input
            className="input"
            value={to}
            onChange={(e) => setTo(e.target.value)}
            placeholder="name@venue.com"
          />
          <SuppressedWarning addr={to} />
        </Field>
        <Field label={t("emails.subject")}>
          <input className="input" value={subject} onChange={(e) => setSubject(e.target.value)} />
        </Field>
        <Field label={t("emails.body")}>
          <textarea
            className="input min-h-[200px] font-mono text-[13px] leading-relaxed"
            value={body}
            onChange={(e) => setBody(e.target.value)}
          />
        </Field>

        {/* Case-by-case links */}
        <div className="panel p-3">
          <div className="flex items-center gap-1.5 text-2xs font-semibold uppercase tracking-wide text-fg-subtle">
            <LinkIcon size={12} /> {t("bulk.links")}
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
                  value={links[k]}
                  onChange={(e) => setLinks((l) => ({ ...l, [k]: e.target.value }))}
                />
              </label>
            ))}
          </div>
        </div>

        {/* Live preview */}
        {hasPlaceholders && (
          <div className="panel p-3">
            <div className="mb-1 text-2xs font-semibold uppercase tracking-wide text-fg-subtle">
              {t("bulk.preview")}
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

        <div className="flex justify-end gap-2 pt-1">
          <button className="btn-outline" onClick={onClose}>
            {t("common.cancel")}
          </button>
          <button className="btn-primary" disabled={!canSend} onClick={doSend}>
            <Send size={15} />
            {sending ? t("emails.sending") : t("emails.send")}
          </button>
        </div>
      </div>
    </Modal>
  );
}
