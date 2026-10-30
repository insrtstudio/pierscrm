import { useEffect, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { ChevronDown, ChevronRight, MapPinned, PenLine } from "lucide-react";
import clsx from "clsx";
import { getContact, viPromoteVenue } from "../lib/api";
import type { Contact } from "../lib/types";
import { parseDay } from "../lib/format";
import { pitchDraft, tourPlan, type RouteSuggestion, type Warmth } from "../lib/routing";
import { ComposeModal } from "./ComposeModal";
import { Modal, errorMessage, useConfirm, useToast } from "./ui";

const RADII = [100, 300, 600];

const WARMTH_CLASS: Record<Warmth, string> = {
  discussion: "bg-accent-soft text-accent-ink",
  replied: "bg-accent-soft text-accent-ink",
  booked: "bg-ok/10 text-ok",
  contacted: "bg-muted text-fg",
  cold: "bg-muted text-fg-subtle",
  silent: "bg-muted text-fg-faint",
  new: "bg-warn/10 text-warn",
};

type Compose = { contact: Contact; subject: string; body: string };

/// "Enchaîner des dates": around a gig (D-2..D+2), the free days and the
/// clubs nearby, warm contacts first. One click drafts the pitch in the
/// compose window; nothing is sent by itself.
export function TourRouting({ eventId, status }: { eventId?: number | null; status: string }) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(status === "confirmed");
  useEffect(() => {
    if (status === "confirmed") setOpen(true);
  }, [status]);
  if (eventId == null || status === "cancelled") return null;
  return (
    <div className="border border-border px-3 py-2.5">
      <button type="button" className="flex w-full items-center gap-2 text-left text-sm" onClick={() => setOpen((o) => !o)}>
        {open ? <ChevronDown size={14} /> : <ChevronRight size={14} />}
        <MapPinned size={14} className="text-fg-subtle" />
        <span className="font-medium">{t("routing.title")}</span>
      </button>
      {open && (
        <div className="mt-2">
          <TourRoutingPanel eventId={eventId} />
        </div>
      )}
    </div>
  );
}

/// The same panel in its own window (Agenda › Actions).
export function TourRoutingModal({ eventId, title, onClose }: { eventId: number; title: string; onClose: () => void }) {
  const { t } = useTranslation();
  return (
    <Modal open onClose={onClose} width="max-w-xl" title={`${t("routing.title")} · ${title}`}>
      <TourRoutingPanel eventId={eventId} />
    </Modal>
  );
}

function TourRoutingPanel({ eventId }: { eventId: number }) {
  const { t, i18n } = useTranslation();
  const toast = useToast();
  const confirm = useConfirm();
  const qc = useQueryClient();
  const [radius, setRadius] = useState(300);
  const [day, setDay] = useState<string | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [compose, setCompose] = useState<Compose | null>(null);
  const { data: plan, isLoading, error } = useQuery({
    queryKey: ["routing_plan", eventId, radius],
    queryFn: () => tourPlan(eventId, radius),
    placeholderData: (prev) => prev,
  });
  const chosen = day && plan?.free_days.includes(day) ? day : plan?.best_day ?? null;

  const dayLabel = (d: string) =>
    parseDay(d)?.toLocaleDateString(i18n.language, { weekday: "short", day: "numeric", month: "short" }) ?? d;

  const draft = async (s: RouteSuggestion) => {
    if (!chosen) return;
    const key = `${s.kind}${s.contact_id ?? s.venue_id}`;
    setBusy(key);
    try {
      let contactId = s.contact_id ?? null;
      // A Discovery venue joins the pipeline first, so the email has a recipient.
      if (contactId == null && s.venue_id != null) {
        if (!confirm(t("routing.promote_confirm", { name: s.name }))) return;
        contactId = await viPromoteVenue(s.venue_id);
        qc.invalidateQueries({ queryKey: ["contacts"] });
        qc.invalidateQueries({ queryKey: ["routing_plan", eventId] });
      }
      if (contactId == null) return;
      const [d, contact] = await Promise.all([pitchDraft(eventId, s, chosen), getContact(contactId)]);
      if (!contact) return;
      if (d.note) toast(d.note, "error");
      setCompose({ contact, subject: d.subject, body: d.body });
    } catch (e) {
      toast(errorMessage(e), "error");
    } finally {
      setBusy(null);
    }
  };

  if (isLoading && !plan) return <p className="text-2xs text-fg-subtle">{t("common.loading")}</p>;
  if (error) return <p className="text-2xs text-danger">{errorMessage(error)}</p>;
  if (!plan) return null;

  return (
    <div className="space-y-2.5">
      <div className="flex flex-wrap items-center gap-1.5">
        <span className="text-2xs text-fg-subtle">{t("routing.free_days")}</span>
        {plan.free_days.length === 0 && <span className="text-2xs text-fg-faint">{t("routing.no_free_day")}</span>}
        {plan.free_days.map((d) => (
          <button
            key={d}
            type="button"
            className={clsx("badge cursor-pointer", d === chosen ? "bg-accent text-accent-fg" : "bg-muted text-fg-subtle")}
            onClick={() => setDay(d)}
          >
            {dayLabel(d)}
          </button>
        ))}
        <select
          className="input ml-auto w-auto py-1 text-2xs"
          value={radius}
          onChange={(e) => setRadius(Number(e.target.value))}
          aria-label={t("routing.radius")}
        >
          {RADII.map((r) => (
            <option key={r} value={r}>
              {t("routing.radius_km", { km: r })}
            </option>
          ))}
        </select>
      </div>
      {!plan.located && <p className="text-2xs text-warn">{t("routing.not_located", { city: plan.city || "·" })}</p>}
      {plan.free_days.length > 0 && plan.suggestions.length === 0 && (
        <p className="text-2xs text-fg-subtle">{t("routing.empty")}</p>
      )}
      {plan.free_days.length > 0 && plan.suggestions.length > 0 && (
        <ul className="max-h-64 overflow-y-auto">
          {plan.suggestions.map((s) => {
            const key = `${s.kind}${s.contact_id ?? s.venue_id}`;
            return (
              <li key={key} className="flex items-center gap-2 border-b border-border/60 py-1.5 text-xs">
                <span className={clsx("badge shrink-0", WARMTH_CLASS[s.warmth_label])}>{t(`routing.warmth_${s.warmth_label}`)}</span>
                <div className="min-w-0 flex-1">
                  <div className="truncate font-medium">{s.name}</div>
                  <div className="truncate text-2xs text-fg-subtle">
                    {[s.city, s.distance_km > 0 ? t("routing.km", { km: Math.round(s.distance_km) }) : t("routing.same_city"), s.score > 0 ? t("routing.score", { score: s.score }) : null]
                      .filter(Boolean)
                      .join(" · ")}
                  </div>
                </div>
                <button
                  type="button"
                  className="btn-outline shrink-0 py-1 text-2xs"
                  disabled={!chosen || busy != null}
                  onClick={() => draft(s)}
                  title={t("routing.draft_hint")}
                >
                  <PenLine size={12} />
                  {busy === key ? "…" : t("routing.draft")}
                </button>
              </li>
            );
          })}
        </ul>
      )}
      {compose && (
        <ComposeModal
          open
          onClose={() => setCompose(null)}
          contact={compose.contact}
          initialSubject={compose.subject}
          initialBody={compose.body}
        />
      )}
    </div>
  );
}
