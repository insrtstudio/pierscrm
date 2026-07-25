import { useEffect, useMemo, useRef, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { listen } from "@tauri-apps/api/event";
import { Send, Users, Check, X, CheckCheck, Ban, Link as LinkIcon } from "lucide-react";
import clsx from "clsx";
import {
  listArtists,
  listCampaigns,
  listContacts,
  listTemplates,
  sendBulk,
} from "../lib/api";
import type { BulkProgress, BulkResult, Campaign, Contact } from "../lib/types";
import { STATUSES } from "../lib/types";
import { CATEGORIES } from "../lib/constants";
import { Field, useToast, useConfirm, Modal } from "../components/ui";

const VARS = ["name", "venue", "event", "artist", "date", "promoter", "city", "target_date"] as const;
const LINK_FIELDS = ["mix", "listen", "onepager", "epk"] as const;
type LinkKey = (typeof LINK_FIELDS)[number];

function renderPreview(
  str: string,
  c: Contact | undefined,
  campaign: Campaign | undefined,
  artistName: string,
  links: Record<string, string>
): string {
  if (!c) return str;
  const map: Record<string, string> = {
    name: c.promoter || c.name || "",
    venue: c.venue || c.name || "",
    event: campaign?.event_name || c.name || "",
    artist: artistName || "",
    date: c.date || campaign?.target_date || "",
    promoter: c.promoter || "",
    city: c.area || "",
    target_date: campaign?.target_date || "",
    ...links,
  };
  return str.replace(/\{\{\s*(\w+)\s*\}\}/g, (_, k) => map[k] ?? `{{${k}}}`);
}

export function BulkSend() {
  const { t } = useTranslation();
  const toast = useToast();
  const confirm = useConfirm();
  const qc = useQueryClient();

  const { data: campaigns = [] } = useQuery({ queryKey: ["campaigns"], queryFn: listCampaigns });
  const { data: artists = [] } = useQuery({ queryKey: ["artists"], queryFn: listArtists });
  const { data: templates = [] } = useQuery({ queryKey: ["templates"], queryFn: listTemplates });

  // Audience filters
  const [campaignId, setCampaignId] = useState<number | "">("");
  const [category, setCategory] = useState("all");
  const [status, setStatus] = useState("all");
  const [priority, setPriority] = useState("all");
  const [search, setSearch] = useState("");
  const [onlyEmail, setOnlyEmail] = useState(true);

  const { data: rawContacts = [] } = useQuery({
    queryKey: ["contacts", category, status, search],
    queryFn: () => listContacts({ category, status, search }),
  });

  const contacts = useMemo(
    () =>
      rawContacts.filter((c) => {
        if (priority !== "all") {
          const p = (c.priority || "").toUpperCase();
          if (priority === "high" && !["P1", "A"].includes(p)) return false;
          if (priority === "mid" && !["P2", "B"].includes(p)) return false;
          if (priority === "low" && !["P3", "C"].includes(p)) return false;
        }
        if (onlyEmail && !c.email?.trim()) return false;
        return true;
      }),
    [rawContacts, priority, onlyEmail]
  );

  const withoutEmail = rawContacts.filter((c) => !c.email?.trim()).length;

  // Selection — auto-select all matching (with email) whenever the list changes.
  const [selected, setSelected] = useState<Set<number>>(new Set());
  const listKey = contacts.map((c) => c.id).join(",");
  useEffect(() => {
    setSelected(new Set(contacts.filter((c) => c.email?.trim()).map((c) => c.id!)));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [listKey]);

  const toggle = (id: number) =>
    setSelected((s) => {
      const n = new Set(s);
      n.has(id) ? n.delete(id) : n.add(id);
      return n;
    });

  // Message
  const [subject, setSubject] = useState("");
  const [body, setBody] = useState("");
  const [links, setLinks] = useState<Record<LinkKey, string>>({
    mix: "",
    listen: "",
    onepager: "",
    epk: "",
  });
  const focused = useRef<"subject" | "body">("body");
  const subjectRef = useRef<HTMLInputElement>(null);
  const bodyRef = useRef<HTMLTextAreaElement>(null);

  const insertVar = (v: string) => {
    const token = `{{${v}}}`;
    if (focused.current === "subject") setSubject((s) => s + token);
    else setBody((b) => b + token);
  };

  const applyTemplate = (id: string) => {
    const tpl = templates.find((x) => x.id === Number(id));
    if (tpl) {
      setSubject(tpl.subject);
      setBody(tpl.body);
    }
  };

  const campaign = campaigns.find((c) => c.id === campaignId);
  const artist = artists.find((a) => a.id === campaign?.artist_id);
  const artistName = artist?.name ?? "";
  const firstRecipient = contacts.find((c) => selected.has(c.id!));

  // Prefill empty link fields from the campaign's artist profile.
  useEffect(() => {
    if (!artist) return;
    setLinks((l) => ({
      mix: l.mix || artist.soundcloud || "",
      listen: l.listen || artist.spotify || "",
      onepager: l.onepager || artist.website || "",
      epk: l.epk,
    }));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [campaignId]);

  // Sending
  const [sending, setSending] = useState(false);
  const [progress, setProgress] = useState<BulkProgress | null>(null);
  const [result, setResult] = useState<BulkResult | null>(null);

  useEffect(() => {
    const un = listen<BulkProgress>("bulk-progress", (e) => setProgress(e.payload));
    return () => {
      un.then((f) => f());
    };
  }, []);

  const doSend = async () => {
    const ids = contacts.filter((c) => selected.has(c.id!) && c.email?.trim()).map((c) => c.id!);
    if (ids.length === 0) return toast(t("bulk.need_recipients"), "error");
    if (!subject.trim() || !body.trim()) return toast(t("bulk.need_message"), "error");
    if (!confirm(t("bulk.confirm", { count: ids.length }))) return;

    setSending(true);
    setProgress({ done: 0, total: ids.length, sent: 0, failed: 0, skipped: 0 });
    try {
      const extra_vars = Object.fromEntries(
        Object.entries(links).filter(([, v]) => v.trim())
      );
      const res = await sendBulk({
        campaign_id: campaignId === "" ? null : campaignId,
        contact_ids: ids,
        subject,
        body,
        extra_vars,
      });
      setResult(res);
      qc.invalidateQueries({ queryKey: ["contacts"] });
      qc.invalidateQueries({ queryKey: ["emails"] });
      qc.invalidateQueries({ queryKey: ["campaigns"] });
      qc.invalidateQueries({ queryKey: ["dashboard"] });
    } catch (e: any) {
      toast(e?.toString?.() ?? "error", "error");
    } finally {
      setSending(false);
      setProgress(null);
    }
  };

  const selectedCount = contacts.filter((c) => selected.has(c.id!) && c.email?.trim()).length;

  return (
    <div className="grid grid-cols-1 gap-5 lg:grid-cols-[minmax(0,380px)_1fr]">
      {/* Audience */}
      <div className="space-y-4">
        <div className="card p-4">
          <div className="mb-2 text-xs font-semibold uppercase tracking-wide text-fg-subtle">
            {t("bulk.step_campaign")}
          </div>
          <select
            className="input"
            value={campaignId}
            onChange={(e) => setCampaignId(e.target.value ? Number(e.target.value) : "")}
          >
            <option value="">{t("bulk.no_campaign")}</option>
            {campaigns.map((c) => (
              <option key={c.id} value={c.id}>
                {c.name}
                {c.event_name ? ` · ${c.event_name}` : ""}
              </option>
            ))}
          </select>
          {campaign && (
            <p className="mt-2 text-2xs text-fg-subtle">{t("bulk.campaign_hint")}</p>
          )}
        </div>

        <div className="card overflow-hidden">
          <div className="border-b border-border p-4">
            <div className="mb-2 text-xs font-semibold uppercase tracking-wide text-fg-subtle">
              {t("bulk.step_audience")}
            </div>
            <input
              className="input mb-2"
              placeholder={t("bulk.search")}
              value={search}
              onChange={(e) => setSearch(e.target.value)}
            />
            <div className="grid grid-cols-3 gap-2">
              <select className="input py-1.5 text-xs" value={category} onChange={(e) => setCategory(e.target.value)}>
                <option value="all">{t("category.all")}</option>
                {CATEGORIES.map((c) => (
                  <option key={c} value={c}>{t(`category.${c}`)}</option>
                ))}
              </select>
              <select className="input py-1.5 text-xs" value={status} onChange={(e) => setStatus(e.target.value)}>
                <option value="all">{t("common.all")}</option>
                {STATUSES.map((s) => (
                  <option key={s} value={s}>{t(`status.${s}`)}</option>
                ))}
              </select>
              <select className="input py-1.5 text-xs" value={priority} onChange={(e) => setPriority(e.target.value)}>
                <option value="all">{t("bulk.priority")}</option>
                <option value="high">P1 / A</option>
                <option value="mid">P2 / B</option>
                <option value="low">P3 / C</option>
              </select>
            </div>
            <label className="mt-2 flex items-center gap-2 text-xs text-fg-subtle">
              <input type="checkbox" className="accent-accent" checked={onlyEmail} onChange={(e) => setOnlyEmail(e.target.checked)} />
              {t("bulk.only_with_email")}
            </label>
          </div>

          <div className="flex items-center justify-between border-b border-border px-4 py-2 text-xs">
            <span className="font-medium text-accent">
              {t("bulk.selected", { count: selectedCount })}
            </span>
            <div className="flex gap-2">
              <button className="btn-ghost px-2 py-1" onClick={() => setSelected(new Set(contacts.filter((c) => c.email?.trim()).map((c) => c.id!)))}>
                <CheckCheck size={13} /> {t("bulk.select_all")}
              </button>
              <button className="btn-ghost px-2 py-1" onClick={() => setSelected(new Set())}>
                <Ban size={13} /> {t("bulk.clear")}
              </button>
            </div>
          </div>

          <div className="max-h-[42vh] overflow-y-auto">
            {contacts.map((c) => {
              const on = selected.has(c.id!);
              const noEmail = !c.email?.trim();
              return (
                <button
                  key={c.id}
                  disabled={noEmail}
                  onClick={() => toggle(c.id!)}
                  className={clsx(
                    "flex w-full items-center gap-2.5 border-b border-border/50 px-4 py-2 text-left transition-colors",
                    noEmail ? "opacity-40" : "hover:bg-muted/50"
                  )}
                >
                  <span
                    className={clsx(
                      "flex h-4 w-4 shrink-0 items-center justify-center rounded border",
                      on ? "border-accent bg-accent text-accent-fg" : "border-border-strong"
                    )}
                  >
                    {on && <Check size={11} />}
                  </span>
                  <div className="min-w-0 flex-1">
                    <div className="truncate text-xs font-medium">{c.name}</div>
                    <div className="truncate text-2xs text-fg-subtle">{c.email || "—"}</div>
                  </div>
                </button>
              );
            })}
            {contacts.length === 0 && (
              <div className="px-4 py-8 text-center text-xs text-fg-subtle">{t("common.empty")}</div>
            )}
          </div>
          <div className="border-t border-border px-4 py-2 text-2xs text-fg-subtle">
            {t("bulk.matching", { count: rawContacts.length })}
            {withoutEmail > 0 && ` · ${t("bulk.no_email", { count: withoutEmail })}`}
          </div>
        </div>
      </div>

      {/* Message */}
      <div className="card flex flex-col p-5">
        <div className="mb-3 text-xs font-semibold uppercase tracking-wide text-fg-subtle">
          {t("bulk.step_message")}
        </div>
        <div className="grid grid-cols-2 gap-4">
          <Field label={t("bulk.template")}>
            <select className="input" onChange={(e) => applyTemplate(e.target.value)}>
              <option value="">{t("bulk.pick_template")}</option>
              {templates.map((tpl) => (
                <option key={tpl.id} value={tpl.id}>{tpl.name}</option>
              ))}
            </select>
          </Field>
          <div className="flex items-end">
            <div className="flex flex-wrap gap-1">
              {[...VARS, ...LINK_FIELDS].map((v) => (
                <button
                  key={v}
                  onClick={() => insertVar(v)}
                  className="badge bg-muted text-fg-subtle hover:bg-accent-soft hover:text-accent"
                  title={t(`bulk.var_${v}`)}
                >
                  {`{{${v}}}`}
                </button>
              ))}
            </div>
          </div>
        </div>

        {/* Links (case-by-case) */}
        <div className="mt-4 panel p-4">
          <div className="flex items-center gap-2 text-2xs font-semibold uppercase tracking-wide text-fg-subtle">
            <LinkIcon size={12} /> {t("bulk.links")}
          </div>
          <p className="mt-0.5 text-2xs text-fg-subtle">{t("bulk.links_hint")}</p>
          <div className="mt-3 grid grid-cols-2 gap-3">
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

        <div className="mt-3">
          <span className="label">{t("bulk.subject")}</span>
          <input
            ref={subjectRef}
            className="input"
            value={subject}
            onFocus={() => (focused.current = "subject")}
            onChange={(e) => setSubject(e.target.value)}
          />
        </div>
        <div className="mt-3 flex-1">
          <span className="label">{t("bulk.body")}</span>
          <textarea
            ref={bodyRef}
            className="input min-h-[200px] font-mono text-[13px] leading-relaxed"
            value={body}
            onFocus={() => (focused.current = "body")}
            onChange={(e) => setBody(e.target.value)}
          />
        </div>

        {/* Preview */}
        {firstRecipient && (subject || body) && (
          <div className="mt-4 panel p-4">
            <div className="mb-2 flex items-center gap-2 text-2xs font-semibold uppercase tracking-wide text-fg-subtle">
              {t("bulk.preview")} · {firstRecipient.name}
            </div>
            <div className="text-sm font-semibold">
              {renderPreview(subject, firstRecipient, campaign, artistName, links)}
            </div>
            <div className="mt-1 whitespace-pre-wrap text-xs text-fg-subtle">
              {renderPreview(body, firstRecipient, campaign, artistName, links)}
            </div>
          </div>
        )}

        {/* Send bar */}
        <div className="mt-5 flex items-center gap-4 border-t border-border pt-4">
          {sending && progress ? (
            <div className="flex-1">
              <div className="mb-1 flex justify-between text-xs text-fg-subtle">
                <span>{t("bulk.sending", { done: progress.done, total: progress.total })}</span>
                <span className="tabular">
                  ✓{progress.sent} ✕{progress.failed} ⊘{progress.skipped}
                </span>
              </div>
              <div className="h-2 overflow-hidden rounded-full bg-muted">
                <div
                  className="h-full rounded-full bg-accent transition-all"
                  style={{ width: `${(progress.done / Math.max(1, progress.total)) * 100}%` }}
                />
              </div>
            </div>
          ) : (
            <div className="flex-1 text-xs text-fg-subtle">
              <span className="inline-flex items-center gap-1">
                <Users size={13} /> {selectedCount}
              </span>
            </div>
          )}
          <button
            className="btn-primary"
            disabled={sending || selectedCount === 0 || !subject.trim() || !body.trim()}
            onClick={doSend}
          >
            <Send size={15} />
            {t("bulk.send_to", { count: selectedCount })}
          </button>
        </div>
      </div>

      {/* Result modal */}
      {result && (
        <Modal open onClose={() => setResult(null)} title={t("bulk.done_title")} width="max-w-md">
          <div className="grid grid-cols-3 gap-3">
            <ResultTile label={t("bulk.r_sent")} value={result.sent} tone="text-emerald-500" />
            <ResultTile label={t("bulk.r_failed")} value={result.failed} tone="text-rose-500" />
            <ResultTile label={t("bulk.r_skipped")} value={result.skipped} tone="text-fg-subtle" />
          </div>
          {result.errors.length > 0 && (
            <div className="mt-4 max-h-40 overflow-y-auto rounded-lg bg-muted p-3 text-2xs text-fg-subtle">
              {result.errors.slice(0, 30).map((e, i) => (
                <div key={i} className="flex items-start gap-1.5">
                  <X size={11} className="mt-0.5 shrink-0 text-rose-500" />
                  {e}
                </div>
              ))}
            </div>
          )}
          <div className="mt-5 flex justify-end">
            <button className="btn-primary" onClick={() => setResult(null)}>
              {t("bulk.close")}
            </button>
          </div>
        </Modal>
      )}
    </div>
  );
}

function ResultTile({ label, value, tone }: { label: string; value: number; tone: string }) {
  return (
    <div className="rounded-xl border border-border p-4 text-center">
      <div className={`text-2xl font-semibold tabular ${tone}`}>{value}</div>
      <div className="mt-0.5 text-2xs text-fg-subtle">{label}</div>
    </div>
  );
}
