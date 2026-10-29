import { useMemo, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { useNavigate } from "react-router-dom";
import clsx from "clsx";
import {
  Bot,
  Flame,
  UserX,
  CalendarCheck,
  Check,
  Disc3,
  ListChecks,
  Mail,
  Megaphone,
  MessageSquareReply,
  Plane,
  Target,
  type LucideIcon,
} from "lucide-react";
import {
  getContact,
  listCampaigns,
  listDossiers,
  listEmailsPage,
  listEvents,
  listEventVisas,
  listFollowups,
  listTasks,
  saveTask,
} from "../lib/api";
import type { Contact } from "../lib/types";
import { parseDay, toDay, today as todayDay } from "../lib/format";
import { EmptyState } from "../components/Layout";
import { ComposeModal, quotaQuery } from "../components/ComposeModal";
import { useToast } from "../components/ui";
import { markHandled, residencyActions, utcDate } from "../lib/residency";
import { IntentBadge } from "./Residency";

/// How far ahead the list looks. Further out, nothing is actionable yet.
const HORIZON_DAYS = 14;
/// Replies older than this are assumed handled.
const REPLY_DAYS = 10;

type Kind = "followup" | "reply" | "campaign" | "deadline" | "task" | "gig" | "visa" | "agent" | "hot" | "wrong";

type Action = {
  key: string;
  day: string; // YYYY-MM-DD; overdue items keep their own day
  kind: Kind;
  title: string;
  detail?: string;
  cta?: { label: string; run: () => void };
  /// Reply classification (residency agent).
  intent?: string | null;
  dismiss?: () => void;
};

/// What ComposeModal opens with: the contact, plus the reply the residency
/// agent suggests (the user reviews and sends it, never the agent).
type ComposeState = { contact: Contact; subject?: string; body?: string; campaignId?: number | null; insightId?: number | null };

const KIND_ICON: Record<Kind, LucideIcon> = {
  followup: Mail,
  reply: MessageSquareReply,
  campaign: Megaphone,
  deadline: Target,
  task: ListChecks,
  gig: Disc3,
  visa: Plane,
  agent: Bot,
  hot: Flame,
  wrong: UserX,
};

// Most urgent kind first within a day: a reply waiting beats a task.
const KIND_ORDER: Kind[] = ["reply", "hot", "wrong", "followup", "agent", "visa", "campaign", "gig", "deadline", "task"];

/// Agenda › Actions: what to do, day by day, gathered from every module
/// (follow-ups due, replies to answer, planned campaign sends, tasks, gigs,
/// visa dossiers), each with the button that does it.
export function Actions() {
  const { t, i18n } = useTranslation();
  const toast = useToast();
  const qc = useQueryClient();
  const navigate = useNavigate();
  const [compose, setCompose] = useState<ComposeState | null>(null);

  const today = todayDay();
  const horizon = toDay(new Date(Date.now() + HORIZON_DAYS * 86_400_000));
  const replySince = toDay(new Date(Date.now() - REPLY_DAYS * 86_400_000));

  const { data: followups = [] } = useQuery({ queryKey: ["followups"], queryFn: listFollowups });
  const { data: replies } = useQuery({
    queryKey: ["emails", "page", "actions-replies"],
    queryFn: () => listEmailsPage({ replied: true, limit: 50 }),
  });
  const { data: campaigns = [] } = useQuery({ queryKey: ["campaigns"], queryFn: listCampaigns });
  const { data: tasks = [] } = useQuery({ queryKey: ["tasks"], queryFn: listTasks });
  const { data: events = [] } = useQuery({
    queryKey: ["events", "actions", today, horizon],
    queryFn: () => listEvents({ from: today, to: horizon }),
  });
  const { data: visas = [] } = useQuery({
    queryKey: ["event_visas", today, horizon],
    queryFn: () => listEventVisas({ from: today, to: horizon }),
  });
  const { data: dossiers = [] } = useQuery({ queryKey: ["dossiers"], queryFn: listDossiers });
  const { data: quota } = useQuery(quotaQuery);
  const { data: agent = [] } = useQuery({
    queryKey: ["residency_actions"],
    queryFn: residencyActions,
    refetchInterval: 120_000,
  });
  const handled = useMutation({
    mutationFn: (id: number) => markHandled(id),
    onSuccess: () => qc.invalidateQueries({ queryKey: ["residency_actions"] }),
  });

  const doneTask = useMutation({
    mutationFn: (id: number) => {
      const tk = tasks.find((x) => x.id === id)!;
      return saveTask({ ...tk, done: true });
    },
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ["tasks"] });
      qc.invalidateQueries({ queryKey: ["dashboard"] });
      toast(t("actions.task_done"), "ok");
    },
  });

  const openCompose = async (contactId: number, prefill?: Omit<ComposeState, "contact">) => {
    const c = await getContact(contactId);
    if (c) setCompose({ contact: c, ...prefill });
  };

  const actions = useMemo(() => {
    const out: Action[] = [];
    const clampDay = (d?: string | null) => (d && /^\d{4}-\d{2}-\d{2}/.test(d) ? d.slice(0, 10) : today);

    for (const f of followups) {
      const day = clampDay(f.due);
      if (day > horizon) continue;
      out.push({
        key: `f${f.contact_id}`,
        day,
        kind: "followup",
        title: t("actions.followup", { name: f.name }),
        detail: t("actions.followup_detail", { count: f.email_count, days: f.days_since }),
        cta: { label: t("actions.followup_cta"), run: () => openCompose(f.contact_id) },
      });
    }

    const agentReplied = new Set(
      agent.filter((a) => a.kind === "reply" || a.kind === "unread" || a.kind === "later" || a.kind === "wrong").map((a) => a.contact_id)
    );
    for (const e of replies?.rows ?? []) {
      const day = clampDay(e.replied_at);
      if (day < replySince || !e.contact_id) continue;
      // The residency agent already lists (and classified) this reply.
      if (agentReplied.has(e.contact_id)) continue;
      out.push({
        key: `r${e.id}`,
        day,
        kind: "reply",
        title: t("actions.reply", { to: e.to_addr }),
        detail: e.subject,
        cta: { label: t("actions.reply_cta"), run: () => openCompose(e.contact_id!) },
      });
    }

    for (const c of campaigns) {
      if (c.status !== "active") continue;
      // A residency audience is sent by its autopilot (listed below), not by bulk send.
      if ((c.planned_count ?? 0) > 0 && c.kind !== "residency") {
        out.push({
          key: `c${c.id}`,
          day: today,
          kind: "campaign",
          title: t("actions.campaign", { name: c.name, count: c.planned_count }),
          detail:
            quota != null ? t("actions.quota", { remaining: quota.remaining, cap: quota.cap }) : undefined,
          cta: { label: t("actions.campaign_cta"), run: () => navigate("/campaigns?tab=bulk") },
        });
      }
      if (c.target_date && c.target_date >= today && c.target_date <= horizon) {
        out.push({
          key: `d${c.id}`,
          day: c.target_date.slice(0, 10),
          kind: "deadline",
          title: t("actions.deadline", { name: c.name }),
        });
      }
    }

    for (const tk of tasks) {
      if (tk.done || !tk.due_date) continue;
      const day = clampDay(tk.due_date);
      if (day > horizon) continue;
      out.push({
        key: `t${tk.id}`,
        day,
        kind: "task",
        title: tk.title,
        detail: tk.owner ?? undefined,
        cta: { label: t("actions.task_cta"), run: () => doneTask.mutate(tk.id!) },
      });
    }

    const visaFor = new Map(visas.map((v) => [v.event_id, v]));
    for (const ev of events) {
      if (ev.status === "cancelled") continue;
      out.push({
        key: `g${ev.id}`,
        day: ev.date.slice(0, 10),
        kind: "gig",
        title: ev.title,
        detail: [ev.venue, ev.city, ev.start_time].filter(Boolean).join(" · "),
        cta: { label: t("actions.gig_cta"), run: () => navigate("/agenda") },
      });
      const v = visaFor.get(ev.id!);
      if (v?.needs_visa && !v.dossier_id) {
        out.push({
          key: `v${ev.id}`,
          day: today,
          kind: "visa",
          title: t("actions.visa_missing", { title: ev.title, country: v.country_name ?? v.country_code }),
          cta: { label: t("actions.visa_cta"), run: () => navigate(`/visa?event=${ev.id}`) },
        });
      }
    }

    // Dossiers still open whose trip falls inside the horizon.
    for (const d of dossiers) {
      if (["approved", "rejected"].includes(d.status)) continue;
      const trip = d.entry_date || d.event_date;
      if (!trip || trip > horizon) continue;
      out.push({
        key: `vd${d.id}`,
        day: today,
        kind: "visa",
        title: t("actions.visa_open", { title: d.title }),
        detail: t(`visa.st_${d.status}`, d.status),
        cta: { label: t("actions.visa_open_cta"), run: () => navigate(`/visa?dossier=${d.id}`) },
      });
    }

    // Residency agent: today's autopilot plan, classified replies, hot EPK
    // signals, "later" follow-ups, wrong contacts. A reply the agent read
    // replaces the generic "reply" line for the same contact.
    for (const a of agent) {
      const day = clampDay(a.day);
      if (day > horizon) continue;
      const camp = a.campaign_id;
      const reply = (label: string) =>
        a.contact_id
          ? {
              label,
              run: () =>
                openCompose(a.contact_id!, {
                  subject: a.suggested_subject ?? undefined,
                  body: a.suggested_body ?? undefined,
                  campaignId: camp,
                  insightId: a.insight_id,
                }),
            }
          : undefined;
      const dismiss = a.insight_id ? () => handled.mutate(a.insight_id!) : undefined;
      switch (a.kind) {
        case "plan": {
          const next = utcDate(a.next_at);
          out.push({
            key: a.key,
            day,
            kind: "agent",
            title:
              a.summary === "off"
                ? t("actions.agent_off", { name: a.name })
                : a.summary
                  ? t("actions.agent_paused", { name: a.name })
                  : t("actions.agent_plan", { name: a.name, count: a.count ?? 0 }),
            detail:
              a.summary && a.summary !== "off"
                ? a.summary
                : next
                  ? t("actions.agent_next", { when: next.toLocaleString(i18n.language, { weekday: "short", hour: "2-digit", minute: "2-digit" }) })
                  : undefined,
            cta: camp ? { label: t("actions.agent_cta"), run: () => navigate(`/campaigns/${camp}`) } : undefined,
          });
          break;
        }
        case "reply":
        case "unread":
          out.push({
            key: a.key,
            day,
            kind: "reply",
            title: t("actions.reply", { to: a.name }),
            detail: a.summary ?? (a.kind === "unread" ? t("actions.agent_unread") : undefined),
            intent: a.intent ?? (a.kind === "unread" ? "unread" : null),
            cta: reply(a.suggested_body ? t("actions.agent_reply_cta") : t("actions.reply_cta")),
            dismiss,
          });
          break;
        case "later":
          out.push({
            key: a.key,
            day,
            kind: "followup",
            title: t("actions.agent_later", { name: a.name }),
            detail: a.summary ?? undefined,
            intent: "later",
            cta: reply(t("actions.followup_cta")),
            dismiss,
          });
          break;
        case "wrong":
          out.push({
            key: a.key,
            day,
            kind: "wrong",
            title: t("actions.agent_wrong", { name: a.name }),
            detail: a.mentioned_email ? t("actions.agent_wrong_hint", { email: a.mentioned_email }) : a.summary ?? undefined,
            intent: "wrong_person",
            cta: a.contact_id ? { label: t("actions.agent_wrong_cta"), run: () => navigate("/contacts") } : undefined,
            dismiss,
          });
          break;
        case "hot":
          out.push({
            key: a.key,
            day: today,
            kind: "hot",
            title:
              a.intent === "feedback"
                ? t("actions.agent_hot_feedback", { name: a.name })
                : t("actions.agent_hot_listen", {
                    name: a.name,
                    track: a.track_title || t("actions.agent_the_mix"),
                    min: Math.max(1, Math.round((a.seconds ?? 0) / 60)),
                  }),
            detail: a.summary ?? undefined,
            cta: reply(t("actions.agent_hot_cta")),
          });
          break;
      }
    }

    return out;
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [followups, replies, campaigns, tasks, events, visas, dossiers, quota, agent, today, horizon, i18n.language]);

  // Overdue first (one bucket), then today, then each upcoming day that has something.
  const groups = useMemo(() => {
    const byDay = new Map<string, Action[]>();
    for (const a of actions) {
      const k = a.day < today ? "overdue" : a.day;
      (byDay.get(k) ?? byDay.set(k, []).get(k)!).push(a);
    }
    for (const list of byDay.values())
      list.sort((a, b) => KIND_ORDER.indexOf(a.kind) - KIND_ORDER.indexOf(b.kind) || a.day.localeCompare(b.day));
    return [...byDay.entries()].sort(([a], [b]) => (a === "overdue" ? -1 : b === "overdue" ? 1 : a.localeCompare(b)));
  }, [actions, today]);

  const dayLabel = (k: string) => {
    if (k === "overdue") return t("actions.overdue");
    if (k === today) return t("actions.today");
    const d = parseDay(k)!;
    const diff = Math.round((d.getTime() - parseDay(today)!.getTime()) / 86_400_000);
    if (diff === 1) return t("actions.tomorrow");
    return d.toLocaleDateString(i18n.language, { weekday: "long", day: "numeric", month: "long" });
  };

  const todayCount = actions.filter((a) => a.day <= today).length;

  return (
    <div className="px-page pb-10 pt-6">
      <div className="mb-6 flex flex-wrap items-baseline gap-x-6 gap-y-1">
        <div className="font-condensed text-[44px] leading-none tabular">{todayCount}</div>
        <div className="kicker">{t("actions.summary", { count: todayCount })}</div>
        {quota && (
          <div className="ml-auto text-2xs text-fg-subtle">
            {t("actions.quota", { remaining: quota.remaining, cap: quota.cap })}
          </div>
        )}
      </div>

      {groups.length === 0 ? (
        <div className="card">
          <EmptyState icon={CalendarCheck} title={t("actions.empty")} hint={t("actions.empty_hint")} />
        </div>
      ) : (
        <div className="grid gap-8 2xl:grid-cols-2 2xl:gap-x-10">
          {groups.map(([k, list]) => (
            <section key={k}>
              <div
                className={clsx(
                  "eyebrow mb-2 flex items-center gap-2 border-b border-border pb-2",
                  k === "overdue" && "text-danger",
                  k === today && "text-accent"
                )}
              >
                {dayLabel(k)}
                <span className="tabular text-fg-faint">{list.length}</span>
              </div>
              <ul>
                {list.map((a) => {
                  const Icon = KIND_ICON[a.kind];
                  return (
                    <li
                      key={a.key}
                      className="flex items-center gap-3 border-b border-border/60 py-2.5 hover:bg-muted/40"
                    >
                      <Icon size={15} className="shrink-0 text-fg-subtle" />
                      <div className="min-w-0 flex-1">
                        <div className="truncate text-sm font-medium">{a.title}</div>
                        {a.detail && <div className="truncate text-2xs text-fg-subtle">{a.detail}</div>}
                      </div>
                      {a.intent && (a.intent === "unread" ? (
                        <span className="badge shrink-0 bg-muted text-fg-subtle">{t("actions.agent_unread_badge")}</span>
                      ) : (
                        <IntentBadge intent={a.intent} />
                      ))}
                      {k === "overdue" && (
                        <span className="shrink-0 text-2xs tabular text-danger">
                          {parseDay(a.day)?.toLocaleDateString(i18n.language, { day: "2-digit", month: "short" })}
                        </span>
                      )}
                      {a.dismiss && (
                        <button className="btn-ghost shrink-0 px-2 py-1.5 text-2xs" onClick={a.dismiss} title={t("actions.agent_done")}>
                          <Check size={13} />
                        </button>
                      )}
                      {a.cta && (
                        <button className="btn-outline shrink-0 py-1.5" onClick={a.cta.run}>
                          {a.kind === "task" && <Check size={13} />}
                          {a.cta.label}
                        </button>
                      )}
                    </li>
                  );
                })}
              </ul>
            </section>
          ))}
        </div>
      )}

      {compose && (
        <ComposeModal
          open
          onClose={() => setCompose(null)}
          contact={compose.contact}
          initialSubject={compose.subject}
          initialBody={compose.body}
          initialCampaignId={compose.campaignId}
          onSent={() => compose.insightId && handled.mutate(compose.insightId)}
        />
      )}
    </div>
  );
}
