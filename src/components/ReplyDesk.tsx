import { useEffect, useRef, useState } from "react";
import { useQuery, useQueryClient, type QueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { useNavigate, useSearchParams } from "react-router-dom";
import { listen } from "@tauri-apps/api/event";
import clsx from "clsx";
import { BookOpen, Bot, ChevronDown, ChevronRight, Clock, EyeOff, PowerOff, RefreshCw, Send, Sparkles, X } from "lucide-react";
import { setSetting } from "../lib/api";
import {
  DESK_NOTIFY_KEY,
  DESK_NUDGE_DAYS_KEY,
  deskCount,
  deskIgnore,
  deskList,
  deskRedraft,
  deskRefresh,
  deskSave,
  deskSend,
  deskSnooze,
  type DeskItem,
} from "../lib/replyDesk";
import {
  autonomyOverview,
  autonomyStopAll,
  autosendCancel,
  localTime,
  type AutoRow,
  type DeskAuto,
} from "../lib/playbook";
import { errorMessage, useSubmit, useToast } from "./ui";

/// Opening the app within this delay of a notification lands on its conversation
/// (the macOS notification has no click callback on desktop).
const NOTIFIED_OPEN_MS = 2 * 3600_000;

function invalidateDesk(qc: QueryClient) {
  for (const key of ["reply_desk", "reply_desk_count", "emails", "campaigns", "followups", "contacts", "autonomy"])
    qc.invalidateQueries({ queryKey: [key] });
}

/// Global listener, mounted once in App: refreshes the desk when the backend
/// pass changes it, and opens the notified conversation when the window regains
/// focus shortly after a notification.
export function ReplyDeskWatcher() {
  const qc = useQueryClient();
  const navigate = useNavigate();
  const pending = useRef<{ id: number; at: number } | null>(null);
  useEffect(() => {
    const a = listen("reply-desk", () => invalidateDesk(qc));
    const b = listen<number>("reply-desk-notified", (e) => {
      pending.current = { id: e.payload, at: Date.now() };
    });
    const onFocus = () => {
      const p = pending.current;
      if (!p) return;
      pending.current = null;
      if (Date.now() - p.at < NOTIFIED_OPEN_MS) navigate(`/outbox?tab=replies&item=${p.id}`);
    };
    window.addEventListener("focus", onFocus);
    return () => {
      a.then((un) => un());
      b.then((un) => un());
      window.removeEventListener("focus", onFocus);
    };
  }, [qc, navigate]);
  return null;
}

/// Count next to "Envois" in the sidebar, red when someone waits for over 24h.
export function DeskBadge({ collapsed }: { collapsed?: boolean }) {
  const { t } = useTranslation();
  const { data } = useQuery({
    queryKey: ["reply_desk_count"],
    queryFn: deskCount,
    refetchInterval: 5 * 60_000,
  });
  if (!data || data.open === 0) return null;
  return (
    <span
      title={t("desk.badge_title", { count: data.open })}
      className={clsx(
        "tabular rounded-full text-[10px] font-medium leading-none",
        collapsed ? "absolute right-1 top-1 h-2 w-2" : "ml-auto px-1.5 py-0.5",
        data.overdue > 0 ? "bg-danger text-white" : "bg-accent/15 text-accent-2",
        collapsed && data.overdue === 0 && "bg-accent"
      )}
    >
      {collapsed ? null : data.open}
    </span>
  );
}

function fmtDate(iso: string | null, lang: string) {
  if (!iso) return "";
  const d = new Date(iso.includes("T") ? iso : iso.replace(" ", "T") + "Z");
  if (Number.isNaN(d.getTime())) return iso;
  return d.toLocaleString(lang.startsWith("fr") ? "fr-FR" : "en-GB", {
    day: "numeric",
    month: "short",
    hour: "2-digit",
    minute: "2-digit",
  });
}

function waiting(hours: number, t: (k: string, o?: Record<string, unknown>) => string) {
  if (hours < 1) return t("desk.waiting_now");
  if (hours < 48) return t("desk.waiting_hours", { count: Math.floor(hours) });
  return t("desk.waiting_days", { count: Math.floor(hours / 24) });
}

/// Outbox › "Réponses à traiter": one card per reply (or stalled thread) with
/// the incoming message, our last one folded away and an editable draft.
export function ReplyDeskTab() {
  const { t } = useTranslation();
  const toast = useToast();
  const qc = useQueryClient();
  const [params] = useSearchParams();
  const focusId = Number(params.get("item")) || null;
  const { data, isLoading } = useQuery({ queryKey: ["reply_desk"], queryFn: deskList });
  // Autonomy (scheduled sends, why a draft waits, playbook entries used).
  const { data: auto } = useQuery({ queryKey: ["autonomy"], queryFn: autonomyOverview, refetchInterval: 60_000 });
  const autoFor = (id: number) => auto?.items.find((a) => a.desk_id === id);
  const autoOn = !!auto && auto.settings.enabled && auto.settings.levels.some((l) => l.level > 0);
  const [stopAll, stopping] = useSubmit(async () => {
    const n = await autonomyStopAll();
    invalidateDesk(qc);
    toast(t("autonomy.stopped", { count: n }));
  });
  const [refresh, refreshing] = useSubmit(async () => {
    const r = await deskRefresh();
    invalidateDesk(qc);
    toast(t("desk.refreshed", { created: r.created, closed: r.closed }));
  });

  return (
    <div className="space-y-4">
      <div className="flex flex-wrap items-center gap-3">
        <p className="text-sm text-fg-muted flex-1 min-w-[16rem]">{t("desk.hint")}</p>
        <button className="btn-outline" onClick={() => refresh()} disabled={refreshing}>
          <RefreshCw size={14} className={clsx(refreshing && "animate-spin")} />
          {t("desk.refresh")}
        </button>
      </div>
      {data && <DeskSettings notify={data.notify} nudgeDays={data.nudge_days} />}
      {autoOn && auto && (
        <div className="flex flex-wrap items-center gap-3 rounded-md border border-accent/30 bg-accent/5 px-3 py-2 text-xs">
          <Bot size={14} className="text-accent-2" />
          <span className="flex-1">{t("autonomy.desk_on", { name: auto.settings.agent_name, delay: auto.settings.delay_min })}</span>
          <button className="btn-ghost text-danger" onClick={() => stopAll()} disabled={stopping}>
            <PowerOff size={13} />
            {t("autonomy.stop_all")}
          </button>
        </div>
      )}
      {isLoading && <p className="text-sm text-fg-muted">{t("common.loading")}</p>}
      {data && data.items.length === 0 && (
        <div className="card p-8 text-center text-sm text-fg-muted">{t("desk.empty")}</div>
      )}
      {data?.items.map((it) => (
        <DeskCard key={it.id} item={it} focused={it.id === focusId} auto={autoFor(it.id)} />
      ))}
      {data && data.snoozed > 0 && (
        <p className="text-xs text-fg-faint">{t("desk.snoozed_count", { count: data.snoozed })}</p>
      )}
      {auto && auto.history.length > 0 && <AutosendHistory rows={auto.history} />}
    </div>
  );
}

/// Every autonomous send (and the ones cancelled or held back), with its reason.
function AutosendHistory({ rows }: { rows: AutoRow[] }) {
  const { t, i18n } = useTranslation();
  const [open, setOpen] = useState(false);
  return (
    <div className="pt-2">
      <button className="flex items-center gap-1 text-xs text-fg-muted hover:text-fg" onClick={() => setOpen((v) => !v)}>
        {open ? <ChevronDown size={12} /> : <ChevronRight size={12} />}
        {t("autonomy.history", { count: rows.length })}
      </button>
      {open && (
        <ul className="mt-2 divide-y divide-border rounded-md border border-border text-xs">
          {rows.map((r) => (
            <li key={r.id} className="flex flex-wrap items-baseline gap-x-3 gap-y-1 px-3 py-2">
              <span className="tabular text-fg-faint">{fmtDate(r.done_at ?? r.due_at, i18n.language)}</span>
              <span className="font-medium">{r.contact}</span>
              <span className="text-fg-muted">{t(`autonomy.cat_${r.category}`)}</span>
              <span
                className={clsx(
                  "rounded-full px-2 py-0.5",
                  r.state === "sent" ? "bg-accent/15 text-accent-2" : r.state === "failed" ? "bg-danger/10 text-danger" : "bg-muted text-fg-muted"
                )}
              >
                {t(`autonomy.state_${r.state}`)}
              </span>
              <span className="w-full text-fg-subtle">
                {r.reason}
                {r.note && ` · ${r.note}`}
              </span>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}

function DeskSettings({ notify, nudgeDays }: { notify: boolean; nudgeDays: number }) {
  const { t } = useTranslation();
  const qc = useQueryClient();
  const toast = useToast();
  const [days, setDays] = useState(String(nudgeDays));
  useEffect(() => setDays(String(nudgeDays)), [nudgeDays]);
  const save = async (key: string, value: string) => {
    try {
      await setSetting(key, value);
      qc.invalidateQueries({ queryKey: ["reply_desk"] });
    } catch (e) {
      toast(errorMessage(e), "error");
    }
  };
  return (
    <div className="flex flex-wrap items-center gap-x-6 gap-y-2 text-sm">
      <label className="flex items-center gap-2">
        <input
          type="checkbox"
          checked={notify}
          onChange={(e) => save(DESK_NOTIFY_KEY, e.target.checked ? "1" : "0")}
        />
        {t("desk.notify")}
      </label>
      <label className="flex items-center gap-2">
        {t("desk.nudge_after")}
        <input
          type="number"
          min={1}
          max={60}
          className="input w-16"
          value={days}
          onChange={(e) => setDays(e.target.value)}
          onBlur={() => {
            const n = Math.round(Number(days));
            if (n >= 1 && n !== nudgeDays) save(DESK_NUDGE_DAYS_KEY, String(n));
            else setDays(String(nudgeDays));
          }}
        />
        {t("desk.days")}
      </label>
    </div>
  );
}

function DeskCard({ item, focused, auto }: { item: DeskItem; focused: boolean; auto?: DeskAuto }) {
  const { t, i18n } = useTranslation();
  const toast = useToast();
  const qc = useQueryClient();
  const ref = useRef<HTMLDivElement>(null);
  const [subject, setSubject] = useState(item.subject);
  const [body, setBody] = useState(item.body);
  const [showOurs, setShowOurs] = useState(false);
  // A redraft (or a pass) replaces the draft: follow it unless being edited.
  useEffect(() => setSubject(item.subject), [item.subject]);
  useEffect(() => setBody(item.body), [item.body]);
  useEffect(() => {
    if (focused) ref.current?.scrollIntoView({ behavior: "smooth", block: "center" });
  }, [focused]);

  const dirty = subject !== item.subject || body !== item.body;
  const persist = async () => {
    if (!dirty) return;
    try {
      await deskSave(item.id, subject, body);
      qc.invalidateQueries({ queryKey: ["reply_desk"] });
    } catch (e) {
      toast(errorMessage(e), "error");
    }
  };

  const [send, sending] = useSubmit(async () => {
    const r = await deskSend(item.id, subject, body);
    if (!r.ok) throw new Error(r.error || t("desk.send_failed"));
    invalidateDesk(qc);
    toast(t("desk.sent", { name: item.contact }));
  });
  const [ignore, ignoring] = useSubmit(async () => {
    await deskIgnore(item.id);
    invalidateDesk(qc);
  });
  const [snooze, snoozing] = useSubmit(async () => {
    await deskSnooze(item.id);
    invalidateDesk(qc);
  });
  const [redraft, redrafting] = useSubmit(async () => {
    await deskRedraft(item.id);
    qc.invalidateQueries({ queryKey: ["reply_desk"] });
  });
  const [cancelAuto, cancelling] = useSubmit(async () => {
    if (auto?.autosend_id) await autosendCancel(auto.autosend_id);
    invalidateDesk(qc);
    toast(t("autonomy.cancelled"));
  });
  const busy = sending || ignoring || snoozing || redrafting || cancelling;

  const intentLabel =
    item.kind === "nudge"
      ? t("desk.kind_nudge")
      : item.intent === "interested"
        ? t("desk.intent_interested")
        : item.intent === "question"
          ? t("desk.intent_question")
          : t("desk.intent_unknown");

  return (
    <div
      ref={ref}
      className={clsx(
        "card space-y-4 p-5",
        focused && "ring-2 ring-accent",
        item.overdue && "border-danger/40"
      )}
    >
      <div className="flex flex-wrap items-baseline gap-x-3 gap-y-1">
        <span className="font-medium">{item.contact}</span>
        {item.venue && <span className="text-sm text-fg-muted">{item.venue}</span>}
        <span className="rounded-full bg-accent/10 px-2 py-0.5 text-[11px] text-accent-2">{intentLabel}</span>
        {item.campaign && <span className="text-xs text-fg-faint">{item.campaign}</span>}
        <span
          className={clsx(
            "ml-auto flex items-center gap-1 text-xs",
            item.overdue ? "font-medium text-danger" : "text-fg-muted"
          )}
        >
          <Clock size={12} />
          {waiting(item.hours_waiting, t)}
        </span>
      </div>

      {auto?.autosend_id && auto.due_at && (
        <div className="flex flex-wrap items-center gap-2 rounded-md border border-accent/40 bg-accent/10 px-3 py-2 text-sm">
          <Bot size={14} className="text-accent-2" />
          <span>{t("autonomy.badge", { time: localTime(auto.due_at) })}</span>
          <button className="btn-ghost ml-auto" disabled={busy} onClick={() => cancelAuto()}>
            <X size={13} />
            {t("autonomy.cancel")}
          </button>
        </div>
      )}
      {!auto?.autosend_id && auto?.note && (
        <p className="text-xs text-fg-muted">
          <Bot size={12} className="mr-1 inline" />
          {t("autonomy.waits", { reason: auto.note })}
        </p>
      )}

      {item.summary && <p className="text-sm italic text-fg-muted">{item.summary}</p>}

      {item.their_text && (
        <div className="rounded-md border border-border bg-muted/40 p-3">
          <p className="eyebrow mb-1 text-[10px]">
            {t("desk.their_message")} · {fmtDate(item.their_at, i18n.language)}
          </p>
          {item.their_subject && <p className="mb-1 text-sm font-medium">{item.their_subject}</p>}
          <p className="whitespace-pre-wrap text-sm">{item.their_text}</p>
        </div>
      )}

      {item.our_text && (
        <div>
          <button
            className="flex items-center gap-1 text-xs text-fg-muted hover:text-fg"
            onClick={() => setShowOurs((v) => !v)}
          >
            {showOurs ? <ChevronDown size={12} /> : <ChevronRight size={12} />}
            {t("desk.our_message")} · {fmtDate(item.our_at, i18n.language)}
          </button>
          {showOurs && (
            <div className="mt-2 rounded-md border border-border p-3">
              {item.our_subject && <p className="mb-1 text-sm font-medium">{item.our_subject}</p>}
              <p className="whitespace-pre-wrap text-sm text-fg-muted">{item.our_text}</p>
            </div>
          )}
        </div>
      )}

      <div className="space-y-2">
        <p className="eyebrow text-[10px]">
          {t("desk.draft")} · {t("desk.to")} {item.to_addr}
          {item.draft_by && (
            <span className="ml-2 normal-case text-fg-faint">
              {item.draft_by === "template" ? t("desk.by_template") : t("desk.by_ai")}
              {item.edited && ` · ${t("desk.edited")}`}
            </span>
          )}
        </p>
        {auto && auto.playbook.length > 0 && (
          <p className="flex flex-wrap items-center gap-1 text-2xs text-fg-faint">
            <BookOpen size={11} />
            {t("autonomy.used")}
            {auto.playbook.map((p) => (
              <span key={p.id} className="rounded bg-muted px-1.5 py-0.5 text-fg-muted">
                {p.title || t(`playbook.topic_${p.topic}`)}
              </span>
            ))}
          </p>
        )}
        <input
          className="input w-full"
          value={subject}
          onChange={(e) => setSubject(e.target.value)}
          onBlur={persist}
          aria-label={t("desk.subject")}
        />
        <textarea
          className="input min-h-[12rem] w-full font-[inherit]"
          value={body}
          onChange={(e) => setBody(e.target.value)}
          onBlur={persist}
          aria-label={t("desk.body")}
        />
      </div>

      <div className="flex flex-wrap items-center gap-2">
        <button
          className="btn-primary"
          disabled={busy || !body.trim() || !subject.trim()}
          onClick={() => send()}
        >
          <Send size={14} />
          {t("desk.send")}
        </button>
        <button className="btn-outline" disabled={busy} onClick={() => snooze()}>
          <Clock size={14} />
          {t("desk.later")}
        </button>
        <button className="btn-ghost" disabled={busy} onClick={() => ignore()}>
          <EyeOff size={14} />
          {t("desk.ignore")}
        </button>
        <button
          className="btn-ghost ml-auto"
          disabled={busy}
          onClick={() => redraft()}
          title={item.edited ? t("desk.redraft_warn") : undefined}
        >
          <Sparkles size={14} className={clsx(redrafting && "animate-pulse")} />
          {t("desk.redraft")}
        </button>
      </div>
    </div>
  );
}
