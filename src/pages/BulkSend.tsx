import { useEffect, useMemo, useRef, useState, useSyncExternalStore } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { listen } from "@tauri-apps/api/event";
import { useVirtualizer } from "@tanstack/react-virtual";
import {
  Send,
  Users,
  Check,
  X,
  CheckCheck,
  Ban,
  Link as LinkIcon,
  ListPlus,
  ShieldBan,
  AlertTriangle,
} from "lucide-react";
import clsx from "clsx";
import {
  addToCampaign,
  listArtists,
  listCampaignAudience,
  listCampaignRecipients,
  listCampaigns,
  listContacts,
  listSuppressions,
  listTemplates,
  sendBulk,
  cancelBulkSend,
} from "../lib/api";
import { utcToLocal } from "../lib/deliverability";
import type { BulkProgress, BulkResult, Contact } from "../lib/types";
import { STATUSES } from "../lib/types";
import { CATEGORIES } from "../lib/constants";
import { Field, useToast, useConfirm, Modal, errorMessage } from "../components/ui";
import {
  invalidateAfterSend,
  openCampaigns,
  QuotaControl,
  quotaQuery,
  renderTemplate,
  unresolvedVars,
} from "../components/ComposeModal";
import { useDebounced } from "../lib/hooks";

/// Above this many matching contacts nothing is pre-selected: one click used to
/// select the whole pipeline (3 200 contacts) for a send.
const AUTO_SELECT_MAX = 50;
const ROW_H = 44;

const VARS = ["name", "venue", "event", "artist", "date", "promoter", "city", "target_date"] as const;
const LINK_FIELDS = ["mix", "listen", "onepager", "epk"] as const;
type LinkKey = (typeof LINK_FIELDS)[number];

/// In-flight send state lives outside the component: the send keeps running in
/// the backend when the user switches tab, and coming back must show its progress
/// instead of an idle form inviting a second send.
type BulkState = { sending: boolean; progress: BulkProgress | null; result: BulkResult | null };
let bulkState: BulkState = { sending: false, progress: null, result: null };
const bulkSubs = new Set<() => void>();
function setBulk(patch: Partial<BulkState>) {
  bulkState = { ...bulkState, ...patch };
  bulkSubs.forEach((f) => f());
}
let bulkListening = false;
function subscribeBulk(f: () => void) {
  if (!bulkListening) {
    bulkListening = true;
    listen<BulkProgress>("bulk-progress", (e) => setBulk({ progress: e.payload })).catch(() => {
      bulkListening = false;
    });
  }
  bulkSubs.add(f);
  return () => {
    bulkSubs.delete(f);
  };
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
  const [skipReached, setSkipReached] = useState(true);
  // Who to send to: the filtered pipeline, or the campaign's planned recipients.
  const [source, setSource] = useState<"filters" | "planned">("filters");
  const dSearch = useDebounced(search, 300);

  const { data: rawContacts = [] } = useQuery({
    queryKey: ["contacts", category, status, dSearch],
    queryFn: () => listContacts({ category, status, search: dSearch }),
    enabled: source === "filters",
  });

  // The campaign's audience (campaign_contacts), for the "planned" source.
  const { data: audience = [] } = useQuery({
    queryKey: ["campaigns", "audience", campaignId],
    queryFn: () => listCampaignAudience(campaignId as number),
    enabled: campaignId !== "",
  });
  const planned = useMemo(() => audience.filter((r) => r.state === "planned"), [audience]);
  useEffect(() => {
    if (campaignId === "") setSource("filters");
  }, [campaignId]);

  // Suppressed addresses are never mailed (the backend skips them too).
  const { data: suppressions = [] } = useQuery({ queryKey: ["suppressions"], queryFn: listSuppressions });
  const suppressed = useMemo(() => new Set(suppressions.map((s) => s.address)), [suppressions]);
  const isSuppressed = (c: Contact) => !!c.email && suppressed.has(c.email.trim().toLowerCase());
  const { data: quota } = useQuery(quotaQuery);

  // Contacts already mailed in the chosen campaign: re-running a campaign after
  // adding contacts must not mail the first wave a second time.
  const { data: reachedIds = [] } = useQuery({
    queryKey: ["campaigns", "recipients", campaignId],
    queryFn: () => listCampaignRecipients(campaignId as number),
    enabled: campaignId !== "",
  });
  const reached = useMemo(
    () => new Set(campaignId !== "" && skipReached ? reachedIds : []),
    [reachedIds, campaignId, skipReached]
  );

  // Priority applies before the counts below, which used to ignore it.
  const prioFiltered = useMemo(
    () =>
      (source === "planned" ? (planned as Contact[]) : rawContacts).filter((c) => {
        if (priority === "all" || source === "planned") return true;
        const p = (c.priority || "").trim().toUpperCase();
        if (priority === "high") return ["P1", "A"].includes(p);
        if (priority === "mid") return ["P2", "B"].includes(p);
        if (priority === "low") return ["P3", "C"].includes(p);
        return true;
      }),
    [rawContacts, planned, source, priority]
  );
  const contacts = useMemo(
    () =>
      prioFiltered.filter((c) => {
        if (onlyEmail && !c.email?.trim()) return false;
        if (reached.has(c.id!)) return false;
        return true;
      }),
    [prioFiltered, onlyEmail, reached]
  );

  const withoutEmail = prioFiltered.filter((c) => !c.email?.trim()).length;
  const alreadyReached = prioFiltered.filter((c) => reached.has(c.id!)).length;
  const suppressedCount = contacts.filter(isSuppressed).length;
  const selectable = (c: Contact) => !!c.email?.trim() && !isSuppressed(c);
  const selectableIds = () => contacts.filter(selectable).map((c) => c.id!);

  // Selection: a short list (<= 50) is selected whole when it changes; a longer
  // one starts empty and needs an explicit, confirmed "select all N".
  const [selected, setSelected] = useState<Set<number>>(new Set());
  const listKey = contacts.map((c) => c.id).join(",");
  useEffect(() => {
    const ids = selectableIds();
    setSelected(new Set(ids.length <= AUTO_SELECT_MAX ? ids : []));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [listKey, suppressed]);
  const selectAll = () => {
    const ids = selectableIds();
    if (ids.length > AUTO_SELECT_MAX && !confirm(t("bulk.select_all_confirm", { count: ids.length })))
      return;
    setSelected(new Set(ids));
  };

  // Virtualised list: 3 000 rows of buttons froze the tab.
  const listRef = useRef<HTMLDivElement | null>(null);
  const virt = useVirtualizer({
    count: contacts.length,
    getScrollElement: () => listRef.current,
    estimateSize: () => ROW_H,
    overscan: 12,
  });
  useEffect(() => {
    if (listRef.current) listRef.current.scrollTop = 0;
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
  // Campaign artist, else the only artist of the roster (the backend does the same).
  const artist =
    artists.find((a) => a.id === campaign?.artist_id) ??
    (artists.length === 1 ? artists[0] : undefined);
  const artistName = artist?.name ?? "";
  const firstRecipient = contacts.find((c) => selected.has(c.id!));
  const ctxFor = (c: (typeof contacts)[number]) => ({
    campaign,
    artistName:
      (campaign?.artist_id ? artistName : "") ||
      artists.find((a) => a.id === c.artist_id)?.name ||
      artistName,
  });
  // Empty link fields are not sent (the backend keeps "{{mix}}" as is): same here.
  const filledLinks = Object.fromEntries(Object.entries(links).filter(([, v]) => v.trim()));
  const preview = (str: string) =>
    firstRecipient ? renderTemplate(str, firstRecipient, filledLinks, ctxFor(firstRecipient)) : str;
  const missing = firstRecipient ? unresolvedVars(preview(subject) + "\n" + preview(body)) : [];

  // Prefill link fields from the campaign's artist profile. What was filled in
  // automatically is remembered, so switching campaign replaces the previous
  // artist's links (they used to stick) while anything typed by hand stays.
  const autoLinks = useRef<Partial<Record<LinkKey, string>>>({});
  useEffect(() => {
    const next: Partial<Record<LinkKey, string>> = {
      mix: artist?.mix_url || artist?.soundcloud || "",
      listen: artist?.spotify || "",
      onepager: artist?.website || "",
    };
    setLinks((l) => {
      const out = { ...l };
      for (const k of Object.keys(next) as LinkKey[]) {
        if (!l[k] || l[k] === autoLinks.current[k]) out[k] = next[k] ?? "";
      }
      return out;
    });
    autoLinks.current = next;
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [campaignId, artist?.id]);

  // Sending
  const { sending, progress: liveProgress, result } = useSyncExternalStore(subscribeBulk, () => bulkState);
  // A send started elsewhere (or before a reload) still streams progress events.
  const progress =
    sending || (liveProgress && liveProgress.done < liveProgress.total) ? liveProgress : null;

  const doSend = async () => {
    const ids = contacts.filter((c) => selected.has(c.id!) && selectable(c)).map((c) => c.id!);
    if (ids.length === 0) return toast(t("bulk.need_recipients"), "error");
    if (!subject.trim() || !body.trim()) return toast(t("bulk.need_message"), "error");
    if (quota && quota.remaining <= 0)
      return toast(t("bulk.quota_reached", { cap: quota.cap }), "error");
    if (missing.length && !confirm(t("bulk.unresolved_confirm", { vars: missing.join(", ") })))
      return;
    const overQuota =
      quota && ids.length > quota.remaining
        ? "\n\n" + t("bulk.over_quota", { remaining: quota.remaining, cap: quota.cap })
        : "";
    if (!confirm(t("bulk.confirm", { count: ids.length }) + overQuota)) return;

    setBulk({
      sending: true,
      result: null,
      progress: { done: 0, total: ids.length, sent: 0, failed: 0, skipped: 0 },
    });
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
      setBulk({ result: res });
      invalidateAfterSend(qc);
    } catch (e: any) {
      // Includes the backend refusing a second concurrent send.
      toast(errorMessage(e), "error");
    } finally {
      setBulk({ sending: false, progress: null });
    }
  };

  const selectedCount = contacts.filter((c) => selected.has(c.id!) && selectable(c)).length;
  // One email per organisation: the other selected contacts of an organisation
  // are skipped by the backend (and marked as reached through it).
  const sameOrg = useMemo(() => {
    const seen = new Set<number>();
    let n = 0;
    for (const c of contacts) {
      if (!selected.has(c.id!) || !selectable(c) || c.organisation_id == null) continue;
      if (seen.has(c.organisation_id)) n++;
      else seen.add(c.organisation_id);
    }
    return n;
  }, [contacts, selected]);

  // Put the current selection in the campaign's audience (planned recipients).
  const [adding, setAdding] = useState(false);
  const addSelection = async () => {
    if (campaignId === "" || selectedCount === 0) return;
    setAdding(true);
    try {
      const ids = contacts.filter((c) => selected.has(c.id!)).map((c) => c.id!);
      const n = await addToCampaign(campaignId, ids);
      toast(t("bulk.added_to_campaign", { count: n }), "ok");
      qc.invalidateQueries({ queryKey: ["campaigns"] });
    } catch (e) {
      toast(errorMessage(e), "error");
    } finally {
      setAdding(false);
    }
  };

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
            {openCampaigns(campaigns).map((c) => (
              <option key={c.id} value={c.id}>
                {c.name}
                {c.event_name ? ` · ${c.event_name}` : ""}
              </option>
            ))}
          </select>
          {campaign && (
            <>
              <p className="mt-2 text-2xs text-fg-subtle">{t("bulk.campaign_hint")}</p>
              <div className="segmented mt-3">
                <button
                  className="segmented-item"
                  data-active={source === "filters"}
                  onClick={() => setSource("filters")}
                >
                  {t("bulk.source_filters")}
                </button>
                <button
                  className="segmented-item"
                  data-active={source === "planned"}
                  onClick={() => setSource("planned")}
                >
                  {t("bulk.source_planned", { count: planned.length })}
                </button>
              </div>
              <label className="mt-2 flex items-center gap-2 text-xs text-fg-subtle">
                <input
                  type="checkbox"
                  className="accent-accent"
                  checked={skipReached}
                  onChange={(e) => setSkipReached(e.target.checked)}
                />
                {t("bulk.skip_campaign")}
              </label>
            </>
          )}
        </div>

        <div className="card overflow-hidden">
          <div className="border-b border-border p-4">
            <div className="mb-2 text-xs font-semibold uppercase tracking-wide text-fg-subtle">
              {t("bulk.step_audience")}
            </div>
            {source === "planned" ? (
              <p className="text-2xs text-fg-subtle">{t("bulk.planned_hint")}</p>
            ) : (
            <>
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
            </>
            )}
          </div>

          <div className="flex items-center justify-between border-b border-border px-4 py-2 text-xs">
            <span className="font-medium text-accent">
              {t("bulk.selected", { count: selectedCount })}
            </span>
            <div className="flex gap-2">
              <button className="btn-ghost px-2 py-1" onClick={selectAll}>
                <CheckCheck size={13} /> {t("bulk.select_all_n", { count: selectableIds().length })}
              </button>
              <button className="btn-ghost px-2 py-1" onClick={() => setSelected(new Set())}>
                <Ban size={13} /> {t("bulk.clear")}
              </button>
            </div>
          </div>
          {selectableIds().length > AUTO_SELECT_MAX && selectedCount === 0 && (
            <p className="border-b border-border bg-muted/40 px-4 py-2 text-2xs text-fg-subtle">
              {t("bulk.big_list_hint")}
            </p>
          )}

          <div ref={listRef} className="max-h-[42vh] overflow-y-auto">
            <div style={{ height: virt.getTotalSize(), position: "relative" }}>
              {virt.getVirtualItems().map((v) => {
                const c = contacts[v.index];
                const on = selected.has(c.id!);
                const blocked = isSuppressed(c);
                const disabled = !c.email?.trim() || blocked;
                return (
                  <button
                    key={c.id}
                    disabled={disabled}
                    onClick={() => toggle(c.id!)}
                    style={{ top: v.start, height: v.size }}
                    className={clsx(
                      "absolute left-0 right-0 flex items-center gap-2.5 border-b border-border/50 px-4 text-left transition-colors",
                      disabled ? "opacity-40" : "hover:bg-muted/50"
                    )}
                  >
                    <span
                      className={clsx(
                        "flex h-4 w-4 shrink-0 items-center justify-center rounded border",
                        on && !disabled ? "border-accent bg-accent text-accent-fg" : "border-border-strong"
                      )}
                    >
                      {on && !disabled && <Check size={11} />}
                    </span>
                    <div className="min-w-0 flex-1">
                      <div className="truncate text-xs font-medium">{c.name}</div>
                      <div className="truncate text-2xs text-fg-subtle">{c.email || "·"}</div>
                    </div>
                    {blocked && <ShieldBan size={13} className="shrink-0 text-danger" />}
                  </button>
                );
              })}
            </div>
            {contacts.length === 0 && (
              <div className="px-4 py-8 text-center text-xs text-fg-subtle">
                {source === "planned" ? t("bulk.planned_empty") : t("common.empty")}
              </div>
            )}
          </div>
          <div className="border-t border-border px-4 py-2 text-2xs text-fg-subtle">
            {t("bulk.matching", { count: prioFiltered.length })}
            {withoutEmail > 0 && ` · ${t("bulk.no_email", { count: withoutEmail })}`}
            {alreadyReached > 0 && ` · ${t("bulk.already_in_campaign", { count: alreadyReached })}`}
            {suppressedCount > 0 && ` · ${t("bulk.suppressed_in_list", { count: suppressedCount })}`}
            {sameOrg > 0 && ` · ${t("bulk.same_organisation", { count: sameOrg })}`}
          </div>
          {campaign && source === "filters" && (
            <div className="border-t border-border px-4 py-2">
              <button
                className="btn-ghost w-full justify-center px-2 py-1.5 text-xs"
                disabled={adding || selectedCount === 0}
                onClick={addSelection}
              >
                <ListPlus size={13} /> {t("bulk.add_to_campaign")} ({selectedCount})
              </button>
            </div>
          )}
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
              {preview(subject)}
            </div>
            <div className="mt-1 whitespace-pre-wrap text-xs text-fg-subtle">{preview(body)}</div>
            {missing.length > 0 && (
              <p className="mt-2 text-2xs font-medium text-danger">
                {t("bulk.unresolved", { vars: missing.map((v) => `{{${v}}}`).join(", ") })}
              </p>
            )}
          </div>
        )}

        {/* Send bar */}
        <div className="mt-5 flex items-center gap-4 border-t border-border pt-4">
          {progress ? (
            <div className="flex-1">
              <div className="mb-1 flex justify-between text-xs text-fg-subtle">
                <span>
                  {t("bulk.sending", { done: progress.done, total: progress.total })}
                  {progress.waiting_until && (
                    <>
                      {" · "}
                      {t("bulk.waiting_until", {
                        time: utcToLocal(progress.waiting_until).toLocaleTimeString([], {
                          hour: "2-digit",
                          minute: "2-digit",
                          second: "2-digit",
                        }),
                      })}
                    </>
                  )}
                </span>
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
            <div className="flex flex-1 flex-wrap items-center gap-x-4 gap-y-1 text-xs text-fg-subtle">
              <span className="inline-flex items-center gap-1">
                <Users size={13} /> {selectedCount}
              </span>
              <QuotaControl compact />
              {quota && selectedCount > quota.remaining && (
                <span className="inline-flex items-center gap-1 text-2xs font-medium text-danger">
                  <AlertTriangle size={12} />
                  {t("bulk.over_quota", { remaining: quota.remaining, cap: quota.cap })}
                </span>
              )}
            </div>
          )}
          {progress && (
            <button
              className="btn-outline"
              onClick={() =>
                cancelBulkSend()
                  .then(() => toast(t("bulk.cancelling"), "ok"))
                  .catch((e) => toast(errorMessage(e), "error"))
              }
            >
              <X size={15} />
              {t("bulk.cancel")}
            </button>
          )}
          <button
            className="btn-primary"
            disabled={sending || !!progress || selectedCount === 0 || !subject.trim() || !body.trim()}
            onClick={doSend}
          >
            <Send size={15} />
            {t("bulk.send_to", { count: selectedCount })}
          </button>
        </div>
      </div>

      {/* Result modal */}
      {result && (
        <Modal open onClose={() => setBulk({ result: null })} title={t("bulk.done_title")} width="max-w-md">
          <div className="grid grid-cols-3 gap-3">
            <ResultTile label={t("bulk.r_sent")} value={result.sent} tone="text-ok" />
            <ResultTile label={t("bulk.r_failed")} value={result.failed} tone="text-danger" />
            <ResultTile label={t("bulk.r_skipped")} value={result.skipped} tone="text-fg-subtle" />
          </div>
          {result.deferred > 0 && (
            <p className="mt-4 flex items-start gap-1.5 text-xs font-medium text-danger">
              <AlertTriangle size={13} className="mt-0.5 shrink-0" />
              {result.stop_reason
                ? t("bulk.r_stopped", { count: result.deferred, reason: result.stop_reason })
                : t("bulk.r_deferred", { count: result.deferred, cap: result.cap })}
            </p>
          )}
          {(result.throttled?.length ?? 0) > 0 && (
            <p className="mt-3 text-2xs text-fg-subtle">
              {t("bulk.r_throttled", { list: result.throttled!.slice(0, 20).join(", ") })}
              {result.throttled!.length > 20 && " …"}
            </p>
          )}
          {(result.grouped ?? 0) > 0 && (
            <p className="mt-3 text-2xs text-fg-subtle">
              {t("bulk.r_grouped", { count: result.grouped })}
            </p>
          )}
          {result.suppressed.length > 0 && (
            <p className="mt-3 text-2xs text-fg-subtle">
              {t("bulk.r_suppressed", { list: result.suppressed.slice(0, 20).join(", ") })}
              {result.suppressed.length > 20 && " …"}
            </p>
          )}
          {result.bounced.length > 0 && (
            <p className="mt-3 text-2xs text-danger">
              {t("bulk.r_bounced", { list: result.bounced.slice(0, 20).join(", ") })}
              {result.bounced.length > 20 && " …"}
            </p>
          )}
          {result.errors.length > 0 && (
            <div className="mt-4 max-h-40 overflow-y-auto rounded-lg bg-muted p-3 text-2xs text-fg-subtle">
              {result.errors.slice(0, 30).map((e, i) => (
                <div key={i} className="flex items-start gap-1.5">
                  <X size={11} className="mt-0.5 shrink-0 text-danger" />
                  {e}
                </div>
              ))}
            </div>
          )}
          <div className="mt-5 flex justify-end">
            <button className="btn-primary" onClick={() => setBulk({ result: null })}>
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
