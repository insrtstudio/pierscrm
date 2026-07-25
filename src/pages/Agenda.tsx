import { useEffect, useMemo, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { ChevronLeft, ChevronRight, Plus, Trash2 } from "lucide-react";
import clsx from "clsx";
import {
  deleteEvent,
  listArtists,
  listContacts,
  listEvents,
  saveEvent,
} from "../lib/api";
import type { Artist, Event } from "../lib/types";
import { EVENT_STATUSES } from "../lib/types";
import { PageHeader } from "../components/Layout";
import { Modal, Field, useToast, useConfirm } from "../components/ui";

const ARTIST_COLORS = [
  "#6366f1", "#10b981", "#f59e0b", "#ec4899", "#0ea5e9",
  "#8b5cf6", "#ef4444", "#14b8a6", "#f97316", "#a855f7",
];

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

  const colorFor = (artistId?: number | null) => {
    if (!artistId) return "#94a3b8";
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
    },
  });

  // Native menu "New event"
  useEffect(() => {
    const h = () => setEditing({ title: "", date: todayIso, status: "confirmed" } as Event);
    window.addEventListener("app:new", ((e: CustomEvent) => {
      if (e.detail === "new:event") h();
    }) as EventListener);
    return () => window.removeEventListener("app:new", h as EventListener);
  }, [todayIso]);

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

      <div className="px-8 py-6">
        {/* Month nav */}
        <div className="mb-4 flex items-center gap-3">
          <div className="flex items-center gap-1">
            <button className="btn-outline px-2 py-2" onClick={() => setCursor(addMonths(cursor, -1))}>
              <ChevronLeft size={16} />
            </button>
            <button className="btn-outline px-2 py-2" onClick={() => setCursor(addMonths(cursor, 1))}>
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
                        className={clsx(
                          "flex items-center gap-1 rounded-md px-1.5 py-0.5 text-2xs font-medium leading-tight",
                          ev.status === "cancelled" && "line-through opacity-50"
                        )}
                        style={{
                          background: `${colorFor(ev.artist_id)}22`,
                          color: colorFor(ev.artist_id),
                        }}
                      >
                        {ev.start_time && (
                          <span className="tabular opacity-80">{ev.start_time}</span>
                        )}
                        <span className="truncate">{ev.title}</span>
                      </div>
                    ))}
                    {dayEvents.length > 3 && (
                      <div className="px-1.5 text-2xs text-fg-subtle">
                        +{dayEvents.length - 3}
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
          onClose={() => setEditing(null)}
          onSaved={() => {
            setEditing(null);
            qc.invalidateQueries({ queryKey: ["events"] });
            qc.invalidateQueries({ queryKey: ["dashboard"] });
          }}
          onDelete={(id) => {
            if (confirm(t("common.confirm_delete"))) {
              delMut.mutate(id);
              setEditing(null);
            }
          }}
        />
      )}
    </div>
  );
}

function EventModal({
  event,
  artists,
  onClose,
  onSaved,
  onDelete,
}: {
  event: Event;
  artists: Artist[];
  onClose: () => void;
  onSaved: () => void;
  onDelete: (id: number) => void;
}) {
  const { t } = useTranslation();
  const toast = useToast();
  const [form, setForm] = useState<Event>({ ...event });
  const { data: bookings = [] } = useQuery({
    queryKey: ["contacts", "confirmed-list"],
    queryFn: () => listContacts({}),
  });
  const set = (k: keyof Event, v: any) => setForm((f) => ({ ...f, [k]: v }));
  const save = async () => {
    if (!form.title.trim() || !form.date) {
      toast(t("common.required"), "error");
      return;
    }
    await saveEvent(form);
    toast(t("common.save"), "ok");
    onSaved();
  };
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
            <select
              className="input"
              value={form.contact_id ?? ""}
              onChange={(e) => set("contact_id", e.target.value ? Number(e.target.value) : null)}
            >
              <option value="">—</option>
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
            <button className="btn-primary" onClick={save}>
              {t("common.save")}
            </button>
          </div>
        </div>
      </div>
    </Modal>
  );
}
