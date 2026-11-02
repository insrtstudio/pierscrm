import { useMemo, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { useNavigate } from "react-router-dom";
import clsx from "clsx";
import {
  AlarmClock,
  Bot,
  Banknote,
  CalendarClock,
  ClipboardCheck,
  Flame,
  UserX,
  CalendarCheck,
  Check,
  CheckCheck,
  ChevronLeft,
  ChevronRight,
  CircleCheck,
  Disc3,
  FileText,
  Handshake,
  Heart,
  ListChecks,
  Mail,
  Megaphone,
  MessageSquareReply,
  MapPinned,
  MapPinPlus,
  Plane,
  Repeat,
  Send,
  Target,
  Undo2,
  type LucideIcon,
  Instagram,
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
import { formatDate, parseDay, toDay, today as todayDay } from "../lib/format";
import { EmptyState } from "../components/Layout";
import { ComposeModal, quotaQuery } from "../components/ComposeModal";
import { errorMessage, useToast } from "../components/ui";
import {
  agendaWeek,
  createExpansion,
  dismissExpansion,
  markHandled,
  utcDate,
  type DoneItem,
  type ExpandSuggestion,
  type ResidencyAction,
} from "../lib/residency";
import { IntentBadge } from "./Residency";
import { followupDone, followupDraft, gigFollowups, tourActions, type FollowupKind } from "../lib/routing";
import { TourRoutingModal } from "../components/TourRouting";
import { DraftCompose } from "../components/Offers";
import {
  advancingEmailDraft,
  listAdvancing,
  listOfferPayments,
  money,
  offerEmailDraft,
  setAdvancingItem,
  type EmailDraft,
} from "../lib/offers";

/// How far ahead "soon" reaches for things placed on today (visa dossiers,
/// advancing): further out, nothing is actionable yet.
const HORIZON_DAYS = 14;
/// Replies older than this are assumed handled.
const REPLY_DAYS = 10;
/// Dots drawn in a day header, at most.
const MAX_DOTS = 10;

type Kind =
  | "followup"
  | "reply"
  | "campaign"
  | "deadline"
  | "task"
  | "gig"
  | "visa"
  | "agent"
  | "scheduled"
  | "snoozed"
  | "expand"
  | "hot"
  | "wrong"
  | "rebook"
  | "routing"
  | "payment"
  | "advancing";

type Action = {
  key: string;
  /// YYYY-MM-DD: the due day (placement in the week comes after).
  day: string;
  kind: Kind;
  title: string;
  detail?: string;
  cta?: { label: string; run: () => void };
  /// Reply classification (residency agent).
  intent?: string | null;
  dismiss?: () => void;
  /// Tooltip of the dismiss button ("Traité" by default).
  dismissLabel?: string;
};

/// An action on its column: late ones sit on today, with their own date.
type Placed = Action & { due: string; overdue: boolean };

/// What was done, as one checked line of a day.
type Done = { key: string; kind: DoneItem["kind"] | "gig"; label: string; detail?: string };

/// What ComposeModal opens with: the contact, plus the reply the residency
/// agent suggests (the user reviews and sends it, never the agent).
type ComposeState = {
  contact: Contact;
  subject?: string;
  body?: string;
  campaignId?: number | null;
  insightId?: number | null;
  /// Thank-you / rebook draft: marked handled once sent (routing.rs).
  gigFollowup?: { eventId: number; kind: FollowupKind };
};

const KIND_ICON: Record<Kind, LucideIcon> = {
  followup: Mail,
  reply: MessageSquareReply,
  campaign: Megaphone,
  deadline: Target,
  task: ListChecks,
  gig: Disc3,
  visa: Plane,
  agent: Bot,
  scheduled: CalendarClock,
  snoozed: AlarmClock,
  expand: MapPinPlus,
  hot: Flame,
  wrong: UserX,
  rebook: Repeat,
  routing: MapPinned,
  payment: Banknote,
  advancing: ClipboardCheck,
};

const DONE_ICON: Record<Done["kind"], LucideIcon> = {
  first_emails: Send,
  followups: Repeat,
  emails: Mail,
  reply: MessageSquareReply,
  auto_reply: Bot,
  dismissed: Undo2,
  handled: CheckCheck,
  expand_later: AlarmClock,
  campaign: Megaphone,
  thanks: Heart,
  rebook: Repeat,
  routing: MapPinned,
  offer_sent: FileText,
  offer_accepted: Handshake,
  paid: Banknote,
  gig: Disc3,
  dm: Instagram,
};

/// Someone is waiting on these: the icon takes the accent.
const HOT_KINDS = new Set<Kind>(["reply", "hot", "wrong", "payment", "followup", "snoozed"]);

// Most urgent kind first within a day: a reply waiting beats a task.
const KIND_ORDER: Kind[] = [
  "reply",
  "hot",
  "wrong",
  "payment",
  "followup",
  "snoozed",
  "rebook",
  "agent",
  "expand",
  "visa",
  "advancing",
  "campaign",
  "scheduled",
  "routing",
  "gig",
  "deadline",
  "task",
];

function addDays(day: string, n: number) {
  const d = parseDay(day)!;
  d.setDate(d.getDate() + n);
  return toDay(d);
}

function mondayOf(day: string) {
  const d = parseDay(day)!;
  return addDays(day, -((d.getDay() + 6) % 7));
}

/// "Résidence Paris" reads "Paris" in a list of campaigns.
const shortName = (n: string) => n.replace(/^Résidence\s+/i, "");

/// Agenda › Actions: the week, Monday to Sunday. Each day holds what is left
/// to do (follow-ups due, replies to answer, planned sends, tasks, gigs, visa
/// dossiers...), each with the button that does it, and what was done that
/// day as checked lines. Late items sit on today.
export function Actions() {
  const { t, i18n } = useTranslation();
  const toast = useToast();
  const qc = useQueryClient();
  const navigate = useNavigate();
  const [compose, setCompose] = useState<ComposeState | null>(null);
  const [routing, setRouting] = useState<{ eventId: number; title: string } | null>(null);
  // Booking drafts (invoice reminder, advancing email), attachments included.
  const [draft, setDraft] = useState<{ d: EmailDraft; eventId?: number } | null>(null);
  const [open, setOpen] = useState<string | null>(null);

  const today = todayDay();
  const [weekStart, setWeekStart] = useState(() => mondayOf(todayDay()));
  const weekEnd = addDays(weekStart, 6);
  const horizon = toDay(new Date(Date.now() + HORIZON_DAYS * 86_400_000));
  const replySince = toDay(new Date(Date.now() - REPLY_DAYS * 86_400_000));
  // Client lists cover today's horizon and the visible week.
  const rangeFrom = weekStart < today ? weekStart : today;
  const rangeTo = weekEnd > horizon ? weekEnd : horizon;
  const isThisWeek = today >= weekStart && today <= weekEnd;

  const { data: followups = [] } = useQuery({ queryKey: ["followups"], queryFn: listFollowups });
  const { data: replies } = useQuery({
    queryKey: ["emails", "page", "actions-replies"],
    queryFn: () => listEmailsPage({ replied: true, limit: 50 }),
  });
  const { data: campaigns = [] } = useQuery({ queryKey: ["campaigns"], queryFn: listCampaigns });
  const { data: tasks = [] } = useQuery({ queryKey: ["tasks"], queryFn: listTasks });
  const { data: events = [] } = useQuery({
    queryKey: ["events", "actions", rangeFrom, rangeTo],
    queryFn: () => listEvents({ from: rangeFrom, to: rangeTo }),
  });
  const { data: visas = [] } = useQuery({
    queryKey: ["event_visas", today, horizon],
    queryFn: () => listEventVisas({ from: today, to: horizon }),
  });
  const { data: dossiers = [] } = useQuery({ queryKey: ["dossiers"], queryFn: listDossiers });
  const { data: payments = [] } = useQuery({ queryKey: ["offer_payments"], queryFn: listOfferPayments });
  const { data: advancing = [] } = useQuery({
    queryKey: ["advancing", "actions", rangeTo],
    queryFn: () => listAdvancing(rangeTo),
  });
  const { data: quota } = useQuery(quotaQuery);
  // Residency actions placed on the week, and what was done each day
  // (commands/residency/agenda.rs). Under "residency_actions" so every
  // refresh of the agent refreshes it.
  const { data: week } = useQuery({
    queryKey: ["residency_actions", "week", weekStart],
    queryFn: () => agendaWeek(weekStart),
    refetchInterval: 120_000,
  });
  // Chaining dates and bringing clubs back (commands/routing.rs).
  const { data: gigFollow = [] } = useQuery({ queryKey: ["routing_followups"], queryFn: gigFollowups });
  const { data: tours = [] } = useQuery({ queryKey: ["routing_tours"], queryFn: tourActions });
  const refreshWeek = () => qc.invalidateQueries({ queryKey: ["residency_actions"] });
  const gigDone = useMutation({
    mutationFn: ({ eventId, kind }: { eventId: number; kind: FollowupKind | "routing" }) => followupDone(eventId, kind),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ["routing_followups"] });
      qc.invalidateQueries({ queryKey: ["routing_tours"] });
      refreshWeek();
    },
  });
  const draftFollowup = async (contactId: number, eventId: number, kind: FollowupKind) => {
    try {
      const [d, c] = await Promise.all([followupDraft(eventId, kind), getContact(contactId)]);
      if (!c) return;
      if (d.note) toast(d.note, "error");
      setCompose({ contact: c, subject: d.subject, body: d.body, gigFollowup: { eventId, kind } });
    } catch (e) {
      toast(String(e), "error");
    }
  };

  const handled = useMutation({
    mutationFn: (id: number) => markHandled(id),
    onSuccess: refreshWeek,
  });

  // A residency campaign running dry: one click creates the next one (target
  // and drafts, autopilot off), then opens its drafts for review.
  const expandCreate = useMutation({
    mutationFn: ({ id, s }: { id: number; s: ExpandSuggestion }) => createExpansion(id, s.zone, s.name),
    onSuccess: (r) => {
      refreshWeek();
      qc.invalidateQueries({ queryKey: ["campaigns"] });
      toast(t("actions.expand_created", { name: r.name, targets: r.targets, drafts: r.drafts }), "ok");
      if (r.errors.length) toast(r.errors[0], "error");
      navigate(`/campaigns/${r.campaign_id}?tab=sequence`);
    },
    onError: (e) => toast(errorMessage(e), "error"),
  });
  // "Plus tard" on a zone card puts it off for every campaign it follows on from.
  const expandLater = useMutation({
    mutationFn: (ids: number[]) => Promise.all(ids.map(dismissExpansion)),
    onSuccess: refreshWeek,
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

  const openDraft = async (load: () => Promise<EmailDraft>, eventId?: number) => {
    try {
      setDraft({ d: await load(), eventId });
    } catch (e) {
      toast(String(e), "error");
    }
  };

  const lang = i18n.language;
  // "Paris, Belgique et France (régions)".
  const list = (names: string[]) =>
    names.length < 2 ? names.join("") : `${names.slice(0, -1).join(", ")} ${t("actions.and")} ${names[names.length - 1]}`;

  // Residency agent: today's autopilot plan, classified replies, hot EPK
  // signals, "later" follow-ups, wrong contacts, next zones, planned sends,
  // replies put aside. A reply the agent read replaces the generic "reply"
  // line for the same contact.
  const fromAgent = (a: ResidencyAction): Action | null => {
    const day = a.day.slice(0, 10);
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
        return {
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
                ? t("actions.agent_next", { when: next.toLocaleString(lang, { weekday: "short", hour: "2-digit", minute: "2-digit" }) })
                : undefined,
          cta: camp ? { label: t("actions.agent_cta"), run: () => navigate(`/campaigns/${camp}`) } : undefined,
        };
      }
      case "scheduled":
        return {
          key: a.key,
          day,
          kind: "scheduled",
          title: t("actions.scheduled", { name: a.name, count: a.count ?? 0 }),
          detail: [
            a.followups ? t("actions.scheduled_followups", { count: a.followups }) : null,
            t("actions.scheduled_hint"),
          ]
            .filter(Boolean)
            .join(" · "),
          cta: camp ? { label: t("actions.agent_cta"), run: () => navigate(`/campaigns/${camp}`) } : undefined,
        };
      case "snoozed":
        return {
          key: a.key,
          day,
          kind: "snoozed",
          title: t("actions.snoozed", { name: a.name }),
          detail: a.suggested_subject ?? undefined,
          cta: { label: t("actions.agent_cta"), run: () => navigate(`/replies?item=${a.desk_id}`) },
        };
      case "reply":
      case "unread":
        return {
          key: a.key,
          day,
          kind: "reply",
          title: t("actions.reply", { to: a.name }),
          detail: a.summary ?? (a.kind === "unread" ? t("actions.agent_unread") : undefined),
          intent: a.intent ?? (a.kind === "unread" ? "unread" : null),
          cta: reply(a.suggested_body ? t("actions.agent_reply_cta") : t("actions.reply_cta")),
          dismiss,
        };
      case "later":
        return {
          key: a.key,
          day,
          kind: "followup",
          title: t("actions.agent_later", { name: a.name }),
          detail: a.summary ?? undefined,
          intent: "later",
          cta: reply(t("actions.followup_cta")),
          dismiss,
        };
      case "wrong":
        return {
          key: a.key,
          day,
          kind: "wrong",
          title: t("actions.agent_wrong", { name: a.name }),
          detail: a.mentioned_email ? t("actions.agent_wrong_hint", { email: a.mentioned_email }) : a.summary ?? undefined,
          intent: "wrong_person",
          cta: a.contact_id ? { label: t("actions.agent_wrong_cta"), run: () => navigate("/contacts") } : undefined,
          dismiss,
        };
      case "expand": {
        const s = a.expand;
        if (!s || !camp) return null;
        const busy = expandCreate.isPending && expandCreate.variables?.id === camp;
        const sources = a.sources?.length ? a.sources : [{ id: camp, name: a.name, remaining: a.remaining ?? 0 }];
        const place = lang.startsWith("fr") ? s.place_fr : s.place_en;
        return {
          key: a.key,
          day,
          kind: "expand",
          title:
            sources.length > 1
              ? t("actions.expand_many", { names: list(sources.map((x) => shortName(x.name))), count: s.eligible, place })
              : t("actions.expand", { name: a.name, count: s.eligible, place }),
          detail: [
            sources.length > 1
              ? t("actions.expand_from", { name: a.name, count: a.remaining ?? 0 })
              : t("actions.expand_left", { count: a.remaining ?? 0 }),
            s.no_email > 0 ? t("actions.expand_no_email", { count: s.no_email }) : null,
            s.top.join(", "),
          ]
            .filter(Boolean)
            .join(" · "),
          cta: {
            label: busy ? t("actions.expand_creating") : t("actions.expand_cta"),
            run: () => !expandCreate.isPending && expandCreate.mutate({ id: camp, s }),
          },
          dismiss: () => expandLater.mutate(sources.map((x) => x.id)),
          dismissLabel: t("actions.expand_later"),
        };
      }
      case "expand_review":
        return {
          key: a.key,
          day,
          kind: "expand",
          title: t("actions.expand_review", { name: a.campaign, count: a.count ?? 0 }),
          detail: t("actions.expand_review_hint", { source: a.name }),
          cta: camp ? { label: t("actions.expand_review_cta"), run: () => navigate(`/campaigns/${camp}?tab=sequence`) } : undefined,
        };
      case "hot":
        return {
          key: a.key,
          day,
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
        };
    }
    return null;
  };

  // Everything the other modules ask for, on its due day.
  const clientActions = useMemo(() => {
    const out: Action[] = [];
    const clampDay = (d?: string | null) => (d && /^\d{4}-\d{2}-\d{2}/.test(d) ? d.slice(0, 10) : today);

    for (const f of followups) {
      out.push({
        key: `f${f.contact_id}`,
        day: clampDay(f.due),
        kind: "followup",
        title: t("actions.followup", { name: f.name }),
        // No email from the app yet: a follow-up date typed on the contact.
        detail:
          f.email_count > 0
            ? t("actions.followup_detail", { count: f.email_count, days: f.days_since })
            : t("actions.followup_manual", { date: formatDate(f.due) }),
        cta: { label: t("actions.followup_cta"), run: () => openCompose(f.contact_id) },
      });
    }

    const agentReplied = new Set(week?.reply_contacts ?? []);
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
      if (c.target_date && c.target_date >= today) {
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
      out.push({
        key: `t${tk.id}`,
        day: clampDay(tk.due_date),
        kind: "task",
        title: tk.title,
        detail: tk.owner ?? undefined,
        cta: { label: t("actions.task_cta"), run: () => doneTask.mutate(tk.id!) },
      });
    }

    const visaFor = new Map(visas.map((v) => [v.event_id, v]));
    for (const ev of events) {
      // Gigs already played are in the "done" lines.
      if (ev.status === "cancelled" || ev.date.slice(0, 10) < today) continue;
      out.push({
        key: `g${ev.id}`,
        day: ev.date.slice(0, 10),
        kind: "gig",
        title: ev.title,
        detail: [ev.venue, ev.city, ev.start_time].filter(Boolean).join(" · "),
        cta: { label: t("actions.gig_cta"), run: () => navigate("/agenda") },
      });
      const v = visaFor.get(ev.id!);
      if (v?.confirmed && v.needs_visa && !v.dossier_id) {
        out.push({
          key: `v${ev.id}`,
          day: today,
          kind: "visa",
          title: t("actions.visa_missing", { title: ev.title, country: v.country_name ?? v.country_code }),
          cta: { label: t("actions.visa_cta"), run: () => navigate(`/admin?tab=visa&event=${ev.id}`) },
        });
      }
    }

    // After a confirmed gig: the thank-you (J+2) and, with nothing new booked,
    // the rebook reminder (J+60). Drafted on click, sent by the user.
    for (const f of gigFollow) {
      out.push({
        key: `gf${f.kind}${f.event_id}`,
        day: clampDay(f.due),
        kind: "rebook",
        title: t(f.kind === "thanks" ? "routing.action_thanks" : "routing.action_rebook", { name: f.venue || f.contact_name }),
        detail: [formatDate(f.gig_date), f.city].filter(Boolean).join(" · "),
        cta: { label: t("routing.action_draft"), run: () => draftFollowup(f.contact_id, f.event_id, f.kind) },
        dismiss: () => gigDone.mutate({ eventId: f.event_id, kind: f.kind }),
      });
    }

    // Confirmed gigs with free days around them and clubs nearby to pitch.
    for (const r of tours) {
      out.push({
        key: `rt${r.event_id}`,
        day: today,
        kind: "routing",
        title: t("routing.action_route", { title: r.title, count: r.count }),
        detail: [formatDate(r.date), r.city, r.warm ? t("routing.action_warm", { count: r.warm }) : null, r.top.join(", ")]
          .filter(Boolean)
          .join(" · "),
        cta: { label: t("routing.action_route_cta"), run: () => setRouting({ eventId: r.event_id, title: r.title }) },
        dismiss: () => gigDone.mutate({ eventId: r.event_id, kind: "routing" }),
      });
    }

    // Booking money: deposits / balances overdue (late, on today) or due
    // later; the CTA drafts the invoice email again.
    for (const p of payments) {
      if (p.paid_at || !p.due) continue;
      out.push({
        key: `p${p.offer_id}${p.part}`,
        day: clampDay(p.due),
        kind: "payment",
        title: t(p.overdue ? "offers.action_overdue" : "offers.action_due", {
          part: t(`offers.part_${p.part}`),
          title: p.title,
          amount: money(p.amount, p.currency, lang),
        }),
        detail: [p.invoice_number, t("offers.due_on", { date: formatDate(p.due) })].filter(Boolean).join(" · "),
        cta: {
          label: t("offers.action_invoice_cta"),
          run: () => openDraft(() => offerEmailDraft(p.offer_id, p.part === "deposit" ? "invoice_deposit" : "invoice_balance")),
        },
      });
    }

    // Advancing: the J-14 email to the venue, then what is still open and due.
    for (const g of advancing) {
      if (!g.email_sent_at && g.email_due <= rangeTo) {
        out.push({
          key: `ae${g.event_id}`,
          day: g.email_due,
          kind: "advancing",
          title: t("offers.action_adv_email", { title: g.title }),
          detail: [g.venue, g.city, formatDate(g.date)].filter(Boolean).join(" · "),
          cta: { label: t("offers.action_adv_email_cta"), run: () => openDraft(() => advancingEmailDraft(g.event_id), g.event_id) },
        });
      }
      const due = g.items.filter((i) => !i.state && i.due <= rangeTo);
      if (due.length) {
        const first = due.reduce((a, b) => (a.due < b.due ? a : b));
        out.push({
          key: `ai${g.event_id}`,
          day: first.due,
          kind: "advancing",
          title: t("offers.action_adv_items", { title: g.title, count: due.length }),
          detail: due.map((i) => t(`offers.adv_${i.key}`)).join(", "),
          cta: { label: t("actions.gig_cta"), run: () => navigate("/agenda") },
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
        cta: { label: t("actions.visa_open_cta"), run: () => navigate(`/admin?tab=visa&dossier=${d.id}`) },
      });
    }
    return out;
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [followups, replies, campaigns, tasks, events, visas, dossiers, payments, advancing, quota, week, gigFollow, tours, today, horizon, rangeTo, lang]);

  const doneLine = (d: DoneItem): Done => {
    const who = d.name ?? "";
    const names = d.names.length ? d.names.join(", ") + (d.count > d.names.length ? "…" : "") : undefined;
    const camp = d.campaign ?? undefined;
    switch (d.kind) {
      case "first_emails":
      case "followups":
      case "emails":
        return {
          key: d.key,
          kind: d.kind,
          label: [t(`actions.done_${d.kind}`, { count: d.count }), camp].filter(Boolean).join(" · "),
          detail: names,
        };
      case "paid":
        return { key: d.key, kind: d.kind, label: t(camp === "deposit" ? "actions.done_paid_deposit" : "actions.done_paid_balance", { name: who }) };
      case "campaign":
      case "expand_later":
        return { key: d.key, kind: d.kind, label: t(`actions.done_${d.kind}`, { name: camp ?? who }) };
      default:
        return { key: d.key, kind: d.kind, label: t(`actions.done_${d.kind}`, { name: who }), detail: camp };
    }
  };

  // The seven columns: placed to-dos (late ones on today) and done lines.
  const days = useMemo(() => {
    const cols = Array.from({ length: 7 }, (_, i) => ({ day: addDays(weekStart, i), todo: [] as Placed[], done: [] as Done[] }));
    const col = (d: string) => cols.find((c) => c.day === d);
    const put = (a: Action, due: string) => {
      if (due < today) {
        if (isThisWeek) col(today)?.todo.push({ ...a, due, overdue: true });
      } else col(due)?.todo.push({ ...a, due, overdue: false });
    };
    for (const a of clientActions) put(a, a.day);
    for (const wd of week?.days ?? []) {
      const c = col(wd.day);
      if (!c) continue;
      for (const a of wd.todo) {
        const x = fromAgent(a);
        if (x) c.todo.push({ ...x, due: x.day, overdue: !!a.overdue });
      }
      c.done.push(...wd.done.map(doneLine));
    }
    for (const ev of events) {
      const d = ev.date.slice(0, 10);
      if (ev.status === "cancelled" || d >= today) continue;
      col(d)?.done.push({
        key: `g${ev.id}`,
        kind: "gig",
        label: t("actions.done_gig", { name: ev.title }),
        detail: [ev.venue, ev.city].filter(Boolean).join(" · ") || undefined,
      });
    }
    for (const c of cols)
      c.todo.sort(
        (a, b) =>
          Number(b.overdue) - Number(a.overdue) ||
          KIND_ORDER.indexOf(a.kind) - KIND_ORDER.indexOf(b.kind) ||
          a.due.localeCompare(b.due)
      );
    return cols;
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [clientActions, week, events, weekStart, today, isThisWeek, lang, expandCreate.isPending]);

  const totals = useMemo(
    () => ({
      todo: days.reduce((n, d) => n + d.todo.length, 0),
      done: days.reduce((n, d) => n + d.done.length, 0),
      late: days.reduce((n, d) => n + d.todo.filter((a) => a.overdue).length, 0),
    }),
    [days]
  );

  const fmtDay = (d: string, o: Intl.DateTimeFormatOptions) => parseDay(d)!.toLocaleDateString(lang, o);
  const weekLabel = t("actions.week_range", {
    from: fmtDay(weekStart, { day: "numeric", month: "short" }),
    to: fmtDay(weekEnd, { day: "numeric", month: "short", year: "numeric" }),
  });

  return (
    <div className="px-page pb-10 pt-6">
      {/* Week header: range, tally, navigation */}
      <div className="mb-5 flex flex-wrap items-end gap-x-6 gap-y-3">
        <div>
          <div className="kicker">{isThisWeek ? t("actions.this_week") : t("actions.week")}</div>
          <div className="font-condensed text-[34px] leading-none tracking-tight">{weekLabel}</div>
        </div>
        <div className="flex flex-wrap items-center gap-2 pb-1 text-2xs">
          <span className="badge bg-ok/10 text-ok">
            <Check size={11} />
            {t("actions.done_count", { count: totals.done })}
          </span>
          <span className="badge bg-accent-soft text-accent-2">{t("actions.todo_count", { count: totals.todo })}</span>
          {totals.late > 0 && <span className="badge bg-danger/10 text-danger">{t("actions.late_count", { count: totals.late })}</span>}
          {quota && (
            <span className="text-fg-subtle">{t("actions.quota", { remaining: quota.remaining, cap: quota.cap })}</span>
          )}
        </div>
        <div className="ml-auto flex items-center gap-1">
          <button className="btn-icon" aria-label={t("actions.prev_week")} title={t("actions.prev_week")} onClick={() => setWeekStart(addDays(weekStart, -7))}>
            <ChevronLeft size={16} />
          </button>
          <button className="btn-outline h-8" disabled={isThisWeek} onClick={() => setWeekStart(mondayOf(today))}>
            {t("actions.today")}
          </button>
          <button className="btn-icon" aria-label={t("actions.next_week")} title={t("actions.next_week")} onClick={() => setWeekStart(addDays(weekStart, 7))}>
            <ChevronRight size={16} />
          </button>
        </div>
      </div>

      {totals.todo + totals.done === 0 ? (
        <div className="card">
          <EmptyState icon={CalendarCheck} title={t("actions.empty")} hint={t("actions.empty_hint")} />
        </div>
      ) : (
        // Seven columns on a wide window (scrolling sideways rather than
        // squeezing a card under ~160px), one day under the other below.
        <div className="grid grid-cols-1 gap-3 xl:grid-cols-[repeat(7,minmax(160px,1fr))] xl:gap-2 xl:overflow-x-auto xl:pb-2">
          {days.map((d) => {
            const isToday = d.day === today;
            const past = d.day < today;
            const dots = d.done.length + d.todo.length;
            return (
              <section
                key={d.day}
                className={clsx(
                  "flex min-w-0 flex-col rounded-md border p-2 xl:min-h-[420px]",
                  isToday ? "border-accent bg-accent-soft/30 shadow-soft" : "border-border bg-surface",
                  past && !isToday && "bg-surface/60"
                )}
              >
                <header className="mb-2 flex items-start justify-between gap-2 px-1">
                  <div className="flex items-baseline gap-1.5">
                    <span className={clsx("font-condensed text-[26px] leading-none tabular", isToday ? "text-accent" : past ? "text-fg-faint" : "text-fg")}>
                      {parseDay(d.day)!.getDate()}
                    </span>
                    <span className={clsx("eyebrow", isToday && "text-accent-2")}>
                      {isToday ? t("actions.today") : fmtDay(d.day, { weekday: "short" })}
                    </span>
                  </div>
                  {dots > 0 && (
                    <div className="pt-1 text-right">
                      <div className="flex flex-wrap justify-end gap-[3px]" aria-hidden>
                        {d.done.slice(0, MAX_DOTS).map((x) => (
                          <span key={x.key} className="h-1.5 w-1.5 rounded-full bg-ok" />
                        ))}
                        {d.todo.slice(0, Math.max(0, MAX_DOTS - d.done.length)).map((x) => (
                          <span
                            key={x.key}
                            className={clsx("h-1.5 w-1.5 rounded-full border", x.overdue ? "border-danger" : "border-accent")}
                          />
                        ))}
                      </div>
                      <div className="mt-1 whitespace-nowrap font-mono text-[10px] text-fg-faint">
                        {t("actions.day_tally", { done: d.done.length, todo: d.todo.length })}
                      </div>
                    </div>
                  )}
                </header>

                {d.todo.length === 0 && d.done.length === 0 && (
                  <div className="px-1 py-3 text-2xs text-fg-faint">{past ? t("actions.nothing_done") : t("actions.nothing_planned")}</div>
                )}

                <ul className="flex flex-col gap-1.5">
                  {d.todo.map((a) => (
                    <ActionCard
                      key={a.key}
                      a={a}
                      open={open === a.key}
                      onToggle={() => setOpen(open === a.key ? null : a.key)}
                      lateLabel={a.overdue ? fmtDay(a.due, { day: "numeric", month: "short" }) : null}
                    />
                  ))}
                </ul>

                {d.done.length > 0 && (
                  <ul className={clsx("flex flex-col gap-1 px-1", d.todo.length > 0 && "mt-3 border-t border-dashed border-border pt-2")}>
                    {d.done.map((x) => {
                      const Icon = DONE_ICON[x.kind];
                      return (
                        <li key={x.key} className="flex items-start gap-1.5 text-2xs text-fg-subtle" title={x.detail}>
                          <CircleCheck size={13} className="mt-px shrink-0 text-ok" />
                          <div className="min-w-0 flex-1">
                            <div className="leading-snug">{x.label}</div>
                            {x.detail && <div className="truncate text-fg-faint">{x.detail}</div>}
                          </div>
                          <Icon size={12} className="mt-0.5 shrink-0 text-fg-faint" />
                        </li>
                      );
                    })}
                  </ul>
                )}
              </section>
            );
          })}
        </div>
      )}

      {draft && (
        <DraftCompose
          draft={draft.d}
          onClose={() => setDraft(null)}
          onSent={() => {
            refreshWeek();
            if (draft.eventId != null)
              setAdvancingItem(draft.eventId, "email", "done").then(() =>
                qc.invalidateQueries({ queryKey: ["advancing"] })
              );
          }}
        />
      )}

      {compose && (
        <ComposeModal
          open
          onClose={() => setCompose(null)}
          contact={compose.contact}
          initialSubject={compose.subject}
          initialBody={compose.body}
          initialCampaignId={compose.campaignId}
          onSent={() => {
            refreshWeek();
            if (compose.insightId) handled.mutate(compose.insightId);
            if (compose.gigFollowup) gigDone.mutate(compose.gigFollowup);
          }}
        />
      )}
      {routing && <TourRoutingModal eventId={routing.eventId} title={routing.title} onClose={() => setRouting(null)} />}
    </div>
  );
}

/// One to-do: icon, short title, one line of context, its button. A click
/// opens the full text.
function ActionCard({
  a,
  open,
  onToggle,
  lateLabel,
}: {
  a: Placed;
  open: boolean;
  onToggle: () => void;
  lateLabel: string | null;
}) {
  const { t } = useTranslation();
  const Icon = KIND_ICON[a.kind];
  return (
    <li
      className={clsx(
        "rounded-md border bg-elevated transition-shadow hover:shadow-soft",
        a.overdue ? "border-danger/40 border-l-2 border-l-danger" : "border-border"
      )}
    >
      <button className="flex w-full items-start gap-2 px-2 pb-1 pt-2 text-left" onClick={onToggle} aria-expanded={open}>
        <span
          className={clsx(
            "mt-px flex h-6 w-6 shrink-0 items-center justify-center rounded-full",
            HOT_KINDS.has(a.kind) ? "bg-accent-soft text-accent-2" : "bg-surface-2 text-fg-subtle"
          )}
        >
          <Icon size={13} />
        </span>
        <span className="min-w-0 flex-1">
          <span className={clsx("block text-[13px] font-medium leading-snug", !open && "line-clamp-2")}>{a.title}</span>
          {a.detail && (
            <span className={clsx("mt-0.5 block text-2xs text-fg-subtle", open ? "whitespace-pre-line" : "truncate")}>{a.detail}</span>
          )}
        </span>
      </button>
      {(open || lateLabel) && (
        <div className="flex flex-wrap items-center gap-1.5 px-2 pb-1">
          {lateLabel && <span className="badge bg-danger/10 text-danger">{t("actions.late_since", { date: lateLabel })}</span>}
          {open &&
            a.intent &&
            (a.intent === "unread" ? (
              <span className="badge bg-muted text-fg-subtle">{t("actions.agent_unread_badge")}</span>
            ) : (
              <IntentBadge intent={a.intent} />
            ))}
        </div>
      )}
      {(a.cta || a.dismiss) && (
        <div className="flex items-center justify-end gap-1 px-1.5 pb-1.5">
          {a.dismiss && (
            <button className="btn-icon h-7 w-7" onClick={a.dismiss} title={a.dismissLabel ?? t("actions.agent_done")} aria-label={a.dismissLabel ?? t("actions.agent_done")}>
              <Check size={13} />
            </button>
          )}
          {a.cta && (
            <button className="btn-outline h-auto min-h-7 whitespace-normal px-3 py-1 text-center text-[10px] leading-tight" onClick={a.cta.run}>
              {a.kind === "task" && <Check size={12} />}
              {a.cta.label}
            </button>
          )}
        </div>
      )}
    </li>
  );
}
