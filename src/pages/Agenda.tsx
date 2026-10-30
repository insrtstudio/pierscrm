import { useMemo, useState } from "react";
import { createPortal } from "react-dom";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import {
  ChevronLeft,
  ChevronRight,
  Plus,
  Trash2,
  MapPin,
  Clock,
  Plane,
  AlertTriangle,
  ListChecks,
} from "lucide-react";
import { useNavigate } from "react-router-dom";
import clsx from "clsx";
import {
  deleteEvent,
  getContact,
  listArtists,
  listContacts,
  listEvents,
  listEventVisas,
  listTasks,
  saveEvent,
} from "../lib/api";
import type { Artist, Contact, Event, EventVisa, Task } from "../lib/types";
import { EVENT_STATUSES } from "../lib/types";
import { PageHeader } from "../components/Layout";
import { euro, parseDay } from "../lib/format";
import { useAppIntent, useDebounced } from "../lib/hooks";
import { Modal, Field, useToast, useConfirm, useSubmit } from "../components/ui";
import { InsrtGigFields, useInsrtGig } from "../components/InsrtGig";
import { TourRouting } from "../components/TourRouting";
import { GigRecap } from "../components/GigRecap";

// Warm, on-brand ramp (reds → warm neutrals) so artists stay distinguishable
// without breaking the Modernist red/grey system.
const ARTIST_COLORS = [
  // insrt.studio ramp: sunset, amber, chrome, smoke
  "#ff5a1f", "#f2b880", "#6f7782", "#d9430f", "#a9a49b",
  "#3fb27f", "#8b929c", "#8b5e3c", "#4a4744", "#ffa66b",
];

const EVENT_STATUS_COLOR: Record<string, string> = {
  confirmed: "#ff5a1f",
  hold: "#f2b880",
  cancelled: "#8b8680",
};

function iso(d: Date) {
  return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-${String(
    d.getDate()
  ).padStart(2, "0")}`;
}
function startOfMonth(d: Date) {
  return new Date(d.getFullYear(), d.getMonth(), 1);
}
function addMonths(d: Date, n: number) {
  return new Date(d.getFullYear(), d.getMonth() + n, 1);
}

export function Agenda() {
  const { t } = useTranslation();
  const qc = useQueryClient();
  const confirm = useConfirm();

  const [cursor, setCursor] = useState(() => startOfMonth(new Date()));
  const [artistFilter, setArtistFilter] = useState<number | "all">("all");
  const [editing, setEditing] = useState<Event | null>(null);
  const [hover, setHover] = useState<{ ev: Event; rect: DOMRect } | null>(null);
  const todayIso = iso(new Date());

  // Build the 6x7 grid (Monday-first).
  const grid = useMemo(() => {
    const first = startOfMonth(cursor);
    const offset = (first.getDay() + 6) % 7; // Mon=0
    const start = new Date(first);
    start.setDate(first.getDate() - offset);
    return Array.from({ length: 42 }, (_, i) => {
      const d = new Date(start);
      d.setDate(start.getDate() + i);
      return d;
    });
  }, [cursor]);

  const rangeFrom = iso(grid[0]);
  const rangeTo = iso(grid[41]);

  const { data: artists = [] } = useQuery({ queryKey: ["artists"], queryFn: listArtists });
  const { data: events = [] } = useQuery({
    queryKey: ["events", rangeFrom, rangeTo, artistFilter],
    queryFn: () =>
      listEvents({
        from: rangeFrom,
        to: rangeTo,
        artist_id: artistFilter === "all" ? null : artistFilter,
      }),
  });

  // Light layer: open tasks with a due date in the visible range.
  const { data: tasks = [] } = useQuery({ queryKey: ["tasks"], queryFn: listTasks });
  const tasksByDay = useMemo(() => {
    const map: Record<string, Task[]> = {};
    for (const tk of tasks)
      if (tk.due_date && !tk.done && tk.due_date >= rangeFrom && tk.due_date <= rangeTo)
        (map[tk.due_date] ??= []).push(tk);
    return map;
  }, [tasks, rangeFrom, rangeTo]);
  // Visa situation per gig: linked dossier, or a gig abroad that needs one.
  const { data: visas = [] } = useQuery({
    queryKey: ["event_visas", rangeFrom, rangeTo],
    queryFn: () => listEventVisas({ from: rangeFrom, to: rangeTo }),
  });
  const visaFor = useMemo(() => {
    const m = new Map<number, EventVisa>();
    for (const v of visas) m.set(v.event_id, v);
    return m;
  }, [visas]);
  const navigate = useNavigate();

  const colorFor = (artistId?: number | null) => {
    if (!artistId) return "#8b929c";
    const idx = artists.findIndex((a) => a.id === artistId);
    return ARTIST_COLORS[(idx < 0 ? 0 : idx) % ARTIST_COLORS.length];
  };

  const eventsByDay = useMemo(() => {
    const map: Record<string, Event[]> = {};
    for (const e of events) (map[e.date] ??= []).push(e);
    return map;
  }, [events]);

  const delMut = useMutation({
    mutationFn: (id: number) => deleteEvent(id),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ["events"] });
      qc.invalidateQueries({ queryKey: ["dashboard"] });
      qc.invalidateQueries({ queryKey: ["budget"] });
      qc.invalidateQueries({ queryKey: ["event_visas"] });
    },
  });

  // Native menu / Dashboard "New event"
  useAppIntent("new:event", () =>
    setEditing({ title: "", date: todayIso, status: "confirmed" } as Event)
  );

  const months = t("agenda.months").split("_");
  const dayLabels = t("agenda.days").split("_");
  const monthLabel = `${months[cursor.getMonth()]} ${cursor.getFullYear()}`;

  return (
    <div>
      <PageHeader
        title={t("agenda.title")}
        subtitle={t("agenda.subtitle")}
        actions={
          <>
            <select
              className="input w-auto py-1.5"
              value={artistFilter}
              onChange={(e) =>
                setArtistFilter(e.target.value === "all" ? "all" : Number(e.target.value))
              }
            >
              <option value="all">{t("agenda.all_artists")}</option>
              {artists.map((a) => (
                <option key={a.id} value={a.id}>
                  {a.name}
                </option>
              ))}
            </select>
            <button
              className="btn-primary"
              onClick={() =>
                setEditing({ title: "", date: todayIso, status: "confirmed" } as Event)
              }
            >
              <Plus size={16} />
              {t("agenda.new_event")}
            </button>
          </>
        }
      />

      <div className="px-page py-6">
        {/* Month nav */}
        <div className="mb-4 flex items-center gap-3">
          <div className="flex items-center gap-1">
            <button
              className="btn-outline px-2 py-2"
              aria-label={t("agenda.prev_month")}
              onClick={() => setCursor(addMonths(cursor, -1))}
            >
              <ChevronLeft size={16} />
            </button>
            <button
              className="btn-outline px-2 py-2"
              aria-label={t("agenda.next_month")}
              onClick={() => setCursor(addMonths(cursor, 1))}
            >
              <ChevronRight size={16} />
            </button>
          </div>
          <button className="btn-ghost" onClick={() => setCursor(startOfMonth(new Date()))}>
            {t("agenda.today")}
          </button>
          <h2 className="text-lg font-semibold capitalize">{monthLabel}</h2>
        </div>

        {/* Calendar */}
        <div className="card overflow-hidden">
          <div className="grid grid-cols-7 border-b border-border">
            {dayLabels.map((d, i) => (
              <div
                key={i}
                className="px-3 py-2 text-center text-xs font-semibold text-fg-subtle"
              >
                {d}
              </div>
            ))}
          </div>
          <div className="grid grid-cols-7">
            {grid.map((d, i) => {
              const dISO = iso(d);
              const inMonth = d.getMonth() === cursor.getMonth();
              const isToday = dISO === todayIso;
              const dayEvents = eventsByDay[dISO] ?? [];
              return (
                <button
                  key={i}
                  onClick={() =>
                    setEditing({ title: "", date: dISO, status: "confirmed" } as Event)
                  }
                  className={clsx(
                    "min-h-[104px] border-b border-r border-border p-1.5 text-left align-top transition-colors hover:bg-muted/40",
                    (i + 1) % 7 === 0 && "border-r-0",
                    i >= 35 && "border-b-0",
                    !inMonth && "bg-surface-2/40"
                  )}
                >
                  <div className="mb-1 flex justify-end">
                    <span
                      className={clsx(
                        "flex h-6 w-6 items-center justify-center rounded-full text-xs tabular",
                        isToday
                          ? "bg-accent font-semibold text-accent-fg"
                          : inMonth
                          ? "text-fg"
                          : "text-fg-faint"
                      )}
                    >
                      {d.getDate()}
                    </span>
                  </div>
                  <div className="space-y-1">
                    {dayEvents.slice(0, 3).map((ev) => (
                      <div
                        key={ev.id}
                        onClick={(e) => {
                          e.stopPropagation();
                          setEditing(ev);
                        }}
                        onMouseEnter={(e) =>
                          setHover({ ev, rect: e.currentTarget.getBoundingClientRect() })
                        }
                        onMouseLeave={() => setHover(null)}
                        className={clsx(
                          "flex items-center gap-1 px-1.5 py-0.5 text-2xs font-semibold leading-tight",
                          ev.status === "cancelled" && "line-through opacity-50"
                        )}
                        style={{
                          borderLeft: `2px solid ${colorFor(ev.artist_id)}`,
                          background: `${colorFor(ev.artist_id)}1f`,
                          color: colorFor(ev.artist_id),
                        }}
                      >
                        {ev.start_time && (
                          <span className="tabular opacity-80">{ev.start_time}</span>
                        )}
                        <span className="truncate">{ev.title}</span>
                        <VisaMark v={visaFor.get(ev.id!)} />
                      </div>
                    ))}
                    {dayEvents.length > 3 && (
                      <div className="px-1.5 text-2xs text-fg-subtle">
                        +{dayEvents.length - 3}
                      </div>
                    )}
                    {(tasksByDay[dISO] ?? []).slice(0, 2).map((tk) => (
                      <div
                        key={`t${tk.id}`}
                        title={tk.title}
                        onClick={(e) => {
                          e.stopPropagation();
                          navigate("/agenda?tab=planning");
                        }}
                        className="flex items-center gap-1 border border-dashed border-border px-1.5 py-0.5 text-2xs leading-tight text-fg-subtle hover:text-fg"
                      >
                        <ListChecks size={10} className="shrink-0" />
                        <span className="truncate">{tk.title.split("\n")[0]}</span>
                      </div>
                    ))}
                    {(tasksByDay[dISO]?.length ?? 0) > 2 && (
                      <div className="px-1.5 text-2xs text-fg-faint">
                        +{tasksByDay[dISO].length - 2}
                      </div>
                    )}
                  </div>
                </button>
              );
            })}
          </div>
        </div>

        {/* Legend */}
        {artists.length > 0 && (
          <div className="mt-4 flex flex-wrap gap-3">
            {artists.map((a) => (
              <div key={a.id} className="flex items-center gap-1.5 text-xs text-fg-subtle">
                <span
                  className="h-2.5 w-2.5 rounded-full"
                  style={{ background: colorFor(a.id) }}
                />
                {a.name}
              </div>
            ))}
          </div>
        )}
      </div>

      {editing && (
        <EventModal
          event={editing}
          artists={artists}
          visa={editing.id != null ? visaFor.get(editing.id) : undefined}
          onClose={() => setEditing(null)}
          onSaved={() => {
            setEditing(null);
            qc.invalidateQueries({ queryKey: ["events"] });
            qc.invalidateQueries({ queryKey: ["dashboard"] });
            // A confirmed gig flips its linked contact to "confirmed" server-side.
            qc.invalidateQueries({ queryKey: ["contacts"] });
            qc.invalidateQueries({ queryKey: ["followups"] });
            qc.invalidateQueries({ queryKey: ["vi_venues"] });
            // A confirmed gig with a fee owns a budget revenue line.
            qc.invalidateQueries({ queryKey: ["budget"] });
            qc.invalidateQueries({ queryKey: ["event_visas"] });
          }}
          onDelete={(id) => {
            if (confirm(t("common.confirm_delete"))) {
              delMut.mutate(id);
              setEditing(null);
            }
          }}
        />
      )}

      {hover && (
        <EventHoverCard
          ev={hover.ev}
          rect={hover.rect}
          artist={artists.find((a) => a.id === hover.ev.artist_id)}
          color={colorFor(hover.ev.artist_id)}
          months={months}
          visa={visaFor.get(hover.ev.id!)}
        />
      )}
    </div>
  );
}

/// Small mark on an event chip: a dossier exists, or the gig needs one.
function VisaMark({ v }: { v?: EventVisa }) {
  const { t } = useTranslation();
  if (!v) return null;
  if (v.dossier_id != null)
    return (
      <span title={t("agenda.visa_dossier", { status: t(`visa.st_${v.dossier_status}`) })} className="ml-auto shrink-0">
        <Plane size={10} />
      </span>
    );
  if (v.needs_visa)
    return (
      <span title={t("agenda.visa_missing", { country: v.country_name ?? v.country_code })} className="ml-auto shrink-0 text-warn">
        <AlertTriangle size={10} />
      </span>
    );
  return null;
}

function VisaLine({ v }: { v?: EventVisa }) {
  const { t } = useTranslation();
  if (!v || (v.dossier_id == null && !v.needs_visa)) return null;
  return v.dossier_id != null ? (
    <div className="flex items-center gap-2">
      <Plane size={12} className="shrink-0 text-fg-faint" />
      {t("agenda.visa_dossier", { status: t(`visa.st_${v.dossier_status}`) })}
    </div>
  ) : (
    <div className="flex items-center gap-2 font-semibold text-warn">
      <AlertTriangle size={12} className="shrink-0" />
      {t("agenda.visa_missing", { country: v.country_name ?? v.country_code })}
    </div>
  );
}

function EventHoverCard({
  ev,
  rect,
  artist,
  color,
  months,
  visa,
}: {
  ev: Event;
  rect: DOMRect;
  artist?: Artist;
  color: string;
  months: string[];
  visa?: EventVisa;
}) {
  const { t } = useTranslation();
  const W = 268;
  const H = 172;
  // Prefer below the chip; flip above if it would overflow the viewport.
  const below = rect.bottom + H + 8 < window.innerHeight;
  const top = below ? rect.bottom + 6 : rect.top - H - 6;
  const left = Math.min(Math.max(8, rect.left), window.innerWidth - W - 8);

  const d = parseDay(ev.date);
  const fullDate = d ? `${d.getDate()} ${months[d.getMonth()]} ${d.getFullYear()}` : ev.date;
  const time = [ev.start_time, ev.end_time].filter(Boolean).join(" – ");

  return createPortal(
    <div
      className="pointer-events-none fixed z-[70] animate-fade-in border border-border bg-elevated p-4 shadow-pop"
      style={{ top, left, width: W, borderTopColor: color }}
    >
      <div className="flex items-start justify-between gap-2">
        <div className="text-sm font-bold leading-tight text-fg">{ev.title}</div>
        <span
          className="badge shrink-0"
          style={{ background: `${EVENT_STATUS_COLOR[ev.status]}22`, color: EVENT_STATUS_COLOR[ev.status] }}
        >
          {t(`agenda.st_${ev.status}`)}
        </span>
      </div>
      {artist && (
        <div className="mt-1 flex items-center gap-1.5 text-2xs font-bold uppercase tracking-wide" style={{ color }}>
          <span className="h-2 w-2" style={{ background: color }} />
          {artist.name}
        </div>
      )}
      <div className="mt-3 space-y-1.5 text-2xs text-fg-subtle">
        <div className="flex items-center gap-2">
          <Clock size={12} className="shrink-0 text-fg-faint" />
          <span className="tabular">
            {fullDate}
            {time && ` · ${time}`}
          </span>
        </div>
        {(ev.venue || ev.city) && (
          <div className="flex items-center gap-2">
            <MapPin size={12} className="shrink-0 text-fg-faint" />
            {[ev.venue, ev.city].filter(Boolean).join(" · ")}
          </div>
        )}
        {ev.fee != null && <div className="font-bold text-fg">{euro(ev.fee)}</div>}
        <VisaLine v={visa} />
        {ev.notes && <div className="line-clamp-3 pt-1 leading-relaxed">{ev.notes}</div>}
      </div>
    </div>,
    document.body
  );
}

function EventModal({
  event,
  artists,
  visa,
  onClose,
  onSaved,
  onDelete,
}: {
  event: Event;
  artists: Artist[];
  visa?: EventVisa;
  onClose: () => void;
  onSaved: () => void;
  onDelete: (id: number) => void;
}) {
  const { t } = useTranslation();
  const toast = useToast();
  const navigate = useNavigate();
  const [form, setForm] = useState<Event>({ ...event });
  const qc = useQueryClient();
  // "Publier sur insrt" (off by default), prefilled with the inferred country.
  const gig = useInsrtGig(event.id, visa?.country_code);
  // Contacts that can plausibly be a booking (confirmed / in discussion /
  // contacted), plus the one currently linked, plus search hits over the whole
  // pipeline: the full list runs into the thousands, and on a fresh pipeline
  // nobody is "in discussion" yet, which left this picker empty.
  const [bookingSearch, setBookingSearch] = useState("");
  const dBookingSearch = useDebounced(bookingSearch.trim(), 250);
  const { data: bookings = [] } = useQuery({
    queryKey: ["contacts", "booking-options", form.contact_id ?? null, dBookingSearch],
    queryFn: async () => {
      const [confirmed, discussing, linked, hits] = await Promise.all([
        listContacts({ status: "confirmed" }),
        listContacts({ status: "in_discussion" }),
        form.contact_id ? getContact(form.contact_id) : Promise.resolve(null),
        dBookingSearch.length >= 2
          ? listContacts({ search: dBookingSearch, sort: "name", limit: 50 })
          : Promise.resolve([] as Contact[]),
      ]);
      const byId = new Map<number, Contact>();
      for (const c of [
        ...(linked ? [linked] : []),
        ...hits.slice(0, 50),
        ...confirmed,
        ...discussing,
      ])
        if (c.id != null) byId.set(c.id, c);
      return [...byId.values()].sort((a, b) => a.name.localeCompare(b.name));
    },
    placeholderData: (prev) => prev,
  });
  const pickContact = (id: number | null) => {
    const c = bookings.find((b) => b.id === id);
    setForm((f) => ({
      ...f,
      contact_id: id,
      // Prefill the empty fields from the booking so the gig is not retyped.
      venue: f.venue || (c ? c.venue || c.name : f.venue),
      city: f.city || (c?.area ?? f.city),
      title: f.title || (c ? c.venue || c.name : f.title),
    }));
  };
  const set = (k: keyof Event, v: any) => setForm((f) => ({ ...f, [k]: v }));
  const [save, busy] = useSubmit(async () => {
    if (!form.title.trim() || !form.date) {
      toast(t("common.required"), "error");
      return;
    }
    const id = await saveEvent(form);
    const pushed = await gig.commit(id);
    qc.invalidateQueries({ queryKey: ["insrt_gig", id] });
    if (pushed?.last_error) toast(pushed.last_error, "error");
    else toast(pushed?.live ? t("insrt.gig_saved_live") : t("common.save"), "ok");
    onSaved();
  });
  return (
    <Modal
      open
      onClose={onClose}
      width="max-w-xl"
      title={event.id ? t("agenda.edit_event") : t("agenda.new_event")}
    >
      <div className="space-y-3.5">
        <Field label={t("agenda.event_title")}>
          <input
            className="input"
            value={form.title}
            onChange={(e) => set("title", e.target.value)}
            placeholder="Piers @ SkateCafe"
          />
        </Field>
        <div className="grid grid-cols-2 gap-x-4 gap-y-3">
          <Field label={t("agenda.artist")}>
            <select
              className="input"
              value={form.artist_id ?? ""}
              onChange={(e) => set("artist_id", e.target.value ? Number(e.target.value) : null)}
            >
              <option value="">{t("agenda.no_artist")}</option>
              {artists.map((a) => (
                <option key={a.id} value={a.id}>
                  {a.name}
                </option>
              ))}
            </select>
          </Field>
          <Field label={t("agenda.status")}>
            <select
              className="input"
              value={form.status}
              onChange={(e) => set("status", e.target.value)}
            >
              {EVENT_STATUSES.map((s) => (
                <option key={s} value={s}>
                  {t(`agenda.st_${s}`)}
                </option>
              ))}
            </select>
          </Field>
          <Field label={t("agenda.date")}>
            <input
              type="date"
              className="input"
              value={form.date}
              onChange={(e) => set("date", e.target.value)}
            />
          </Field>
          <div className="grid grid-cols-2 gap-2">
            <Field label={t("agenda.start")}>
              <input
                type="time"
                className="input"
                value={form.start_time ?? ""}
                onChange={(e) => set("start_time", e.target.value)}
              />
            </Field>
            <Field label={t("agenda.end")}>
              <input
                type="time"
                className="input"
                value={form.end_time ?? ""}
                onChange={(e) => set("end_time", e.target.value)}
              />
            </Field>
          </div>
          <Field label={t("agenda.venue")}>
            <input
              className="input"
              value={form.venue ?? ""}
              onChange={(e) => set("venue", e.target.value)}
            />
          </Field>
          <Field label={t("agenda.city")}>
            <input
              className="input"
              value={form.city ?? ""}
              onChange={(e) => set("city", e.target.value)}
            />
          </Field>
          <Field label={t("agenda.fee")}>
            <input
              type="number"
              className="input"
              value={form.fee ?? ""}
              onChange={(e) => set("fee", e.target.value === "" ? null : parseFloat(e.target.value))}
            />
          </Field>
          <Field label={t("agenda.link_contact")}>
            <input
              className="input mb-1.5 py-1.5 text-xs"
              placeholder={t("agenda.search_contact")}
              value={bookingSearch}
              onChange={(e) => setBookingSearch(e.target.value)}
            />
            <select
              className="input"
              value={form.contact_id ?? ""}
              onChange={(e) => pickContact(e.target.value ? Number(e.target.value) : null)}
            >
              <option value="">·</option>
              {bookings.map((c) => (
                <option key={c.id} value={c.id}>
                  {c.name}
                </option>
              ))}
            </select>
          </Field>
        </div>
        <Field label={t("agenda.notes")}>
          <textarea
            className="input min-h-[70px]"
            value={form.notes ?? ""}
            onChange={(e) => set("notes", e.target.value)}
          />
        </Field>
        <InsrtGigFields gig={gig} status={form.status} />
        <TourRouting eventId={event.id} status={form.status} />
        <GigRecap eventId={event.id} artistId={form.artist_id} date={form.date} status={form.status} />
        {event.id != null && visa && (visa.dossier_id != null || visa.needs_visa) && (
          <div
            className={clsx(
              "flex items-center gap-3 border px-3 py-2 text-xs",
              visa.dossier_id != null ? "border-border text-fg-subtle" : "border-warn/40 bg-warn/5 text-warn"
            )}
          >
            <div className="flex-1">
              <VisaLine v={visa} />
            </div>
            <button
              className="btn-outline py-1 text-xs"
              onClick={() =>
                navigate(
                  visa.dossier_id != null
                    ? `/visa?dossier=${visa.dossier_id}`
                    : `/visa?event=${event.id}`
                )
              }
            >
              {visa.dossier_id != null ? t("agenda.open_dossier") : t("agenda.create_dossier")}
            </button>
          </div>
        )}
        <div className="flex items-center justify-between pt-1">
          <div>
            {event.id && (
              <button className="btn-danger" onClick={() => onDelete(event.id!)}>
                <Trash2 size={15} />
                {t("common.delete")}
              </button>
            )}
          </div>
          <div className="flex gap-2">
            <button className="btn-outline" onClick={onClose}>
              {t("common.cancel")}
            </button>
            <button className="btn-primary" onClick={save} disabled={busy}>
              {t("common.save")}
            </button>
          </div>
        </div>
      </div>
    </Modal>
  );
}
