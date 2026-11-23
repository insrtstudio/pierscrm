import { useEffect, useMemo, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { openUrl } from "@tauri-apps/plugin-opener";
import { listen } from "@tauri-apps/api/event";
import clsx from "clsx";
import {
  AlertTriangle,
  AtSign,
  Check,
  ChevronLeft,
  ChevronRight,
  ClipboardCopy,
  Disc3,
  ExternalLink,
  Eye,
  Mail,
  Pause,
  PenLine,
  Play,
  Plus,
  RefreshCw,
  Search,
  Send,
  Trash2,
} from "lucide-react";
import { EmptyState, ErrorState, Loader, PageHeader, TabbedPage } from "../components/Layout";
import { Field, Modal, errorMessage, useConfirm, useToast } from "../components/ui";
import { formatDate, toDay, today } from "../lib/format";
import {
  CAMPAIGN_KINDS,
  CARD_KINDS,
  COLS,
  META_FIELDS,
  cardAdd,
  cardDelete,
  cardDetail,
  cardFind,
  cardMove,
  cardSend,
  cardSetData,
  cardWrite,
  instaLogin,
  instaQueue,
  instaTest,
  metaMissing,
  releaseDelete,
  releaseSave,
  releaseValidate,
  releasesBoard,
  targetSave,
  targetsMark,
  type Board,
  type Card,
  type CardData,
  type CardKind,
  type Col,
  type Release,
  type ReleaseProgress,
  type Target,
} from "../lib/releases";

// Releases of the label, run as a board: one card per step of a release, on a
// calendar day and in a column, worked from its pop-up.

type T = (k: string, o?: Record<string, unknown>) => string;

const isCampaign = (k: CardKind) => CAMPAIGN_KINDS.includes(k);
const openExt = (url?: string | null) => url && openUrl(url).catch(() => {});

export function ReleasesSection() {
  const { t } = useTranslation();
  return (
    <TabbedPage
      title={t("nav.releases")}
      subtitle={t("releases.subtitle")}
      tabs={[
        { key: "board", label: t("releases.tab_board"), render: () => <Desk view="board" /> },
        { key: "agenda", label: t("releases.tab_agenda"), render: () => <Desk view="agenda" /> },
      ]}
    />
  );
}

// Which release the board is narrowed to, kept across the two tabs.
let rememberedRelease: number | null = null;

function useProgress() {
  const qc = useQueryClient();
  const toast = useToast();
  const [progress, setProgress] = useState<ReleaseProgress | null>(null);
  useEffect(() => {
    const un = listen<ReleaseProgress>("releases:progress", (e) => {
      const p = e.payload;
      setProgress(p.finished ? null : p);
      if (p.error) toast(p.error, "error");
      if (p.info) toast(p.info, "ok");
      qc.invalidateQueries({ queryKey: ["releases_board"] });
      qc.invalidateQueries({ queryKey: ["release_card"] });
    });
    return () => {
      un.then((f) => f());
    };
  }, [qc, toast]);
  return progress;
}

function Desk({ view }: { view: "board" | "agenda" }) {
  const { t } = useTranslation();
  const qc = useQueryClient();
  const toast = useToast();
  const confirm = useConfirm();
  const q = useQuery({ queryKey: ["releases_board"], queryFn: releasesBoard });
  const progress = useProgress();
  const [releaseId, setReleaseIdRaw] = useState<number | null>(rememberedRelease);
  const [editing, setEditing] = useState<Release | "new" | null>(null);
  const [openCard, setOpenCard] = useState<number | null>(null);
  const [adding, setAdding] = useState<string | null>(null);
  const setReleaseId = (id: number | null) => {
    rememberedRelease = id;
    setReleaseIdRaw(id);
  };
  const refresh = () => qc.invalidateQueries({ queryKey: ["releases_board"] });
  const run = (p: Promise<unknown>) => p.then(refresh).catch((e) => toast(errorMessage(e), "error"));

  if (q.isLoading) return <Loader />;
  if (q.isError || !q.data) return <ErrorState onRetry={() => q.refetch()} />;
  const b = q.data;
  const release = b.releases.find((r) => r.id === releaseId) ?? null;
  const cards = b.cards.filter((c) => !release || c.release_id === release.id);

  return (
    <div className="px-page py-5">
      <PageHeader
        title={t("nav.releases")}
        actions={
          <button className="btn-primary" onClick={() => setEditing("new")}>
            <Plus size={14} /> {t("releases.new")}
          </button>
        }
      />
      <Setup board={b} t={t} />

      {b.releases.length === 0 ? (
        <EmptyState
          icon={Disc3}
          title={t("releases.empty")}
          hint={t("releases.empty_hint")}
          action={
            <button className="btn-primary" onClick={() => setEditing("new")}>
              <Plus size={14} /> {t("releases.new")}
            </button>
          }
        />
      ) : (
        <>
          <div className="mb-3 flex flex-wrap items-center gap-2">
            <Chip active={!release} onClick={() => setReleaseId(null)} label={t("releases.all")} />
            {b.releases.map((r) => (
              <Chip
                key={r.id}
                active={release?.id === r.id}
                onClick={() => setReleaseId(r.id)}
                label={r.name}
                hint={r.release_date ? formatDate(r.release_date) : t("releases.to_validate")}
                warn={!r.release_date}
              />
            ))}
          </div>

          {b.releases
            .filter((r) => !r.release_date && (!release || release.id === r.id))
            .map((r) => (
              <ValidateBar key={r.id} release={r} t={t} onValidate={(d) => run(releaseValidate(r.id, d))} onEdit={() => setEditing(r)} />
            ))}

          {release?.release_date && (
            <div className="mb-4 flex flex-wrap items-center gap-x-4 gap-y-1 text-[12.5px] text-fg-subtle">
              <span className="text-fg">{release.artist}</span>
              <span>{release.genre}</span>
              <span>{t(`releases.master.${release.master_state}`)}</span>
              <span>
                {t("releases.out_on")} <span className="text-fg">{formatDate(release.release_date)}</span>
              </span>
              {!release.listen_url && <span className="text-accent-2">{t("releases.no_link")}</span>}
              <button className="btn-ghost h-7 px-3" onClick={() => setEditing(release)}>
                <PenLine size={12} /> {t("releases.edit")}
              </button>
              <button
                className="btn-ghost h-7 px-3"
                onClick={() => confirm(t("releases.delete_confirm", { name: release.name })) && run(releaseDelete(release.id).then(() => setReleaseId(null)))}
              >
                <Trash2 size={12} /> {t("releases.delete")}
              </button>
            </div>
          )}

          <InstaBar board={b} t={t} run={run} />
          {progress && progress.stage !== "insta" && (
            <div className="mb-3 flex items-center gap-2 text-[12.5px] text-fg-subtle">
              <RefreshCw size={13} className="animate-spin" />
              {t(`releases.stage.${progress.stage}`)} {progress.label}
              {progress.total > 0 && ` (${progress.done + 1}/${progress.total})`}
            </div>
          )}

          {view === "board" ? (
            <BoardView cards={cards} single={!!release} t={t} onOpen={setOpenCard} onMove={(id, col) => run(cardMove(id, { col }))} />
          ) : (
            <AgendaView
              cards={cards}
              single={!!release}
              t={t}
              onOpen={setOpenCard}
              onMove={(id, dueDate) => run(cardMove(id, { dueDate }))}
              onAdd={(day) => setAdding(day)}
            />
          )}
        </>
      )}

      {editing && (
        <ReleaseModal
          release={editing === "new" ? null : editing}
          t={t}
          onClose={() => setEditing(null)}
          onSaved={(id) => {
            setEditing(null);
            setReleaseId(id);
            refresh();
          }}
        />
      )}
      {adding && (
        <AddCardModal
          day={adding}
          releases={b.releases.filter((r) => r.release_date)}
          preset={release?.release_date ? release.id : null}
          t={t}
          onClose={() => setAdding(null)}
          onAdded={(id) => {
            setAdding(null);
            refresh();
            setOpenCard(id);
          }}
        />
      )}
      {openCard != null && <CardModal id={openCard} busy={progress?.card_id === openCard ? progress : null} t={t} onClose={() => setOpenCard(null)} />}
    </div>
  );
}

// ---------------- Small pieces ----------------

function Chip({ active, onClick, label, hint, warn }: { active: boolean; onClick: () => void; label: string; hint?: string; warn?: boolean }) {
  return (
    <button
      onClick={onClick}
      className={clsx(
        "flex items-center gap-2 rounded-full border px-3.5 py-1.5 text-[12.5px] transition-colors",
        active ? "border-fg bg-fg text-bg" : "border-border bg-surface text-fg hover:border-fg"
      )}
    >
      <span className="font-medium">{label}</span>
      {hint && <span className={clsx("text-[11.5px]", active ? "opacity-70" : warn ? "text-accent-2" : "text-fg-subtle")}>{hint}</span>}
    </button>
  );
}

function Setup({ board: b, t }: { board: Board; t: T }) {
  const missing = [!b.sender && t("releases.setup.sender"), !b.ai_ready && t("releases.setup.ai"), !b.search_ready && t("releases.setup.search")].filter(
    Boolean
  ) as string[];
  if (!missing.length) return null;
  return (
    <div className="mb-4 rounded-xl border border-accent-2/40 bg-accent-2/[0.06] px-4 py-3 text-[13px]">
      <div className="mb-1 flex items-center gap-2 font-medium">
        <AlertTriangle size={14} className="text-accent-2" /> {t("releases.setup.title")}
      </div>
      <ul className="list-disc space-y-0.5 pl-5 text-fg-subtle">
        {missing.map((m) => (
          <li key={m}>{m}</li>
        ))}
      </ul>
    </div>
  );
}

function ValidateBar({ release: r, t, onValidate, onEdit }: { release: Release; t: T; onValidate: (d: string) => void; onEdit: () => void }) {
  const [date, setDate] = useState("");
  return (
    <div className="card mb-4 flex flex-wrap items-center gap-3 px-4 py-3">
      <div className="min-w-0 flex-1">
        <div className="text-sm font-medium">{r.name}</div>
        <div className="text-[12.5px] text-fg-subtle">
          {[r.artist, r.genre, t(`releases.master.${r.master_state}`)].filter(Boolean).join(" · ")}. {t("releases.validate_hint")}
        </div>
      </div>
      <button className="btn-ghost" onClick={onEdit}>
        <PenLine size={13} /> {t("releases.edit")}
      </button>
      <input type="date" className="input w-44" min={today()} value={date} onChange={(e) => setDate(e.target.value)} />
      <button className="btn-primary" disabled={!date} onClick={() => onValidate(date)}>
        <Check size={14} /> {t("releases.validate")}
      </button>
    </div>
  );
}

function InstaBar({ board: b, t, run }: { board: Board; t: T; run: (p: Promise<unknown>) => void }) {
  const i = b.insta;
  const [handle, setHandle] = useState("");
  return (
    <div className={clsx("mb-4 flex flex-wrap items-center gap-3 rounded-xl border px-4 py-2.5 text-[12.5px]", i.need_login ? "border-accent-2/50 bg-accent-2/[0.06]" : "border-border bg-surface")}>
      <AtSign size={14} className="text-fg-subtle" />
      <span>
        {t("releases.insta.line", { queued: i.queued, sent: i.sent_today, cap: i.cap })}
        {i.busy && <span className="text-fg-subtle"> · {t("releases.insta.busy")}</span>}
        {i.need_login && <span className="text-accent-2"> · {t("releases.insta.need_login")}</span>}
      </span>
      <span className="flex-1" />
      <input className="input h-7 w-40 text-[12.5px]" placeholder={t("releases.insta.test_handle")} value={handle} onChange={(e) => setHandle(e.target.value)} />
      <button className="btn-ghost h-7 px-3" disabled={!handle.trim() || i.busy} title={t("releases.insta.test_hint")} onClick={() => run(instaTest(handle))}>
        {t("releases.insta.test")}
      </button>
      <button className="btn-ghost h-7 px-3" onClick={() => run(instaLogin())}>
        {t("releases.insta.login")}
      </button>
      {i.queued > 0 &&
        (i.busy ? (
          <button className="btn-outline h-7 px-3" onClick={() => run(instaQueue(false))}>
            <Pause size={12} /> {t("releases.insta.pause")}
          </button>
        ) : (
          <button className="btn-outline h-7 px-3" onClick={() => run(instaQueue(true))}>
            <Play size={12} /> {t("releases.insta.resume")}
          </button>
        ))}
    </div>
  );
}

function cardNote(c: Card, t: T): { text: string; tone: string } | null {
  if (isCampaign(c.kind)) {
    if (c.drafts > 0) return { text: t("releases.note.drafts", { count: c.drafts }), tone: "bg-accent-2/15 text-accent-2" };
    if (c.followups_due > 0) return { text: t("releases.note.followups", { count: c.followups_due }), tone: "bg-accent-2/15 text-accent-2" };
    if (c.replies > 0) return { text: t("releases.note.replies", { count: c.replies, sent: c.sent }), tone: "bg-ok/15 text-ok" };
    if (c.sent > 0) return { text: t("releases.note.sent", { count: c.sent }), tone: "bg-fg/[0.07] text-fg-subtle" };
    if (c.targets > 0) return { text: t("releases.note.found", { count: c.targets }), tone: "bg-fg/[0.07] text-fg-subtle" };
    return { text: t("releases.note.searching"), tone: "bg-fg/[0.07] text-fg-subtle" };
  }
  if (c.kind === "meta") {
    const n = metaMissing(c.data);
    return n > 0 ? { text: t("releases.note.missing", { count: n }), tone: "bg-accent-2/15 text-accent-2" } : { text: t("releases.note.complete"), tone: "bg-ok/15 text-ok" };
  }
  const steps = c.data.checklist ?? [];
  if (!steps.length) return null;
  const done = steps.filter((s) => s.done).length;
  return { text: `${done}/${steps.length}`, tone: done === steps.length ? "bg-ok/15 text-ok" : "bg-fg/[0.07] text-fg-subtle" };
}

function CardTile({ card: c, single, compact, t, onOpen }: { card: Card; single: boolean; compact?: boolean; t: T; onOpen: () => void }) {
  const late = c.col !== "done" && c.due_date < today();
  const note = cardNote(c, t);
  return (
    <button
      draggable
      onDragStart={(e) => {
        e.dataTransfer.setData("text/plain", String(c.id));
        e.dataTransfer.effectAllowed = "move";
      }}
      onClick={onOpen}
      className={clsx(
        "card block w-full cursor-grab text-left transition-colors hover:border-fg active:cursor-grabbing",
        compact ? "px-2 py-1" : "px-3 py-2.5",
        c.col === "done" && "opacity-60"
      )}
    >
      {!single && <div className="eyebrow truncate">{c.release_name}</div>}
      <div className={clsx("truncate font-medium", compact ? "text-[12px]" : "text-sm")}>{c.title}</div>
      {!compact && (
        <div className="mt-1.5 flex items-center gap-2">
          <span className={clsx("text-[11.5px]", late ? "text-danger" : "text-fg-subtle")}>{formatDate(c.due_date)}</span>
          <span className="flex-1" />
          {note && <span className={clsx("badge", note.tone)}>{note.text}</span>}
        </div>
      )}
      {compact && note && <div className="truncate text-[10.5px] text-fg-subtle">{note.text}</div>}
    </button>
  );
}

// ---------------- Board ----------------

function BoardView({ cards, single, t, onOpen, onMove }: { cards: Card[]; single: boolean; t: T; onOpen: (id: number) => void; onMove: (id: number, col: Col) => void }) {
  const [over, setOver] = useState<Col | null>(null);
  return (
    <div className="grid grid-cols-4 gap-3">
      {COLS.map((col) => {
        const list = cards.filter((c) => c.col === col);
        return (
          <div
            key={col}
            onDragOver={(e) => {
              e.preventDefault();
              setOver(col);
            }}
            onDragLeave={() => setOver((o) => (o === col ? null : o))}
            onDrop={(e) => {
              e.preventDefault();
              setOver(null);
              const id = Number(e.dataTransfer.getData("text/plain"));
              if (id) onMove(id, col);
            }}
            className={clsx("panel min-h-[320px] p-2.5 transition-colors", over === col && "border-accent")}
          >
            <div className="mb-2 flex items-center justify-between px-1">
              <span className="eyebrow">{t(`releases.col.${col}`)}</span>
              <span className="text-[11.5px] text-fg-faint">{list.length}</span>
            </div>
            <div className="space-y-2">
              {list.map((c) => (
                <CardTile key={c.id} card={c} single={single} t={t} onOpen={() => onOpen(c.id)} />
              ))}
            </div>
          </div>
        );
      })}
    </div>
  );
}

// ---------------- Agenda ----------------

function AgendaView({
  cards,
  single,
  t,
  onOpen,
  onMove,
  onAdd,
}: {
  cards: Card[];
  single: boolean;
  t: T;
  onOpen: (id: number) => void;
  onMove: (id: number, day: string) => void;
  onAdd: (day: string) => void;
}) {
  const { i18n } = useTranslation();
  const [month, setMonth] = useState(() => {
    const next = cards.filter((c) => c.col !== "done").map((c) => c.due_date).sort()[0];
    const d = next && next > today() ? new Date(next + "T12:00:00") : new Date();
    return new Date(d.getFullYear(), d.getMonth(), 1);
  });
  const [over, setOver] = useState<string | null>(null);
  const days = useMemo(() => {
    const first = new Date(month);
    const lead = (first.getDay() + 6) % 7; // Monday first
    const start = new Date(first.getFullYear(), first.getMonth(), 1 - lead);
    const total = Math.ceil((lead + new Date(first.getFullYear(), first.getMonth() + 1, 0).getDate()) / 7) * 7;
    return Array.from({ length: total }, (_, i) => new Date(start.getFullYear(), start.getMonth(), start.getDate() + i));
  }, [month]);
  const byDay = useMemo(() => {
    const m = new Map<string, Card[]>();
    for (const c of cards) m.set(c.due_date, [...(m.get(c.due_date) ?? []), c]);
    return m;
  }, [cards]);
  const shift = (n: number) => setMonth(new Date(month.getFullYear(), month.getMonth() + n, 1));
  const weekdays = Array.from({ length: 7 }, (_, i) => new Date(2024, 0, 1 + i).toLocaleDateString(i18n.language, { weekday: "short" }));

  return (
    <div>
      <div className="mb-3 flex items-center gap-2">
        <button className="btn-icon" onClick={() => shift(-1)} aria-label={t("releases.prev_month")}>
          <ChevronLeft size={16} />
        </button>
        <div className="w-60 whitespace-nowrap text-center font-display text-base capitalize">{month.toLocaleDateString(i18n.language, { month: "long", year: "numeric" })}</div>
        <button className="btn-icon" onClick={() => shift(1)} aria-label={t("releases.next_month")}>
          <ChevronRight size={16} />
        </button>
        <button className="btn-ghost h-7 px-3" onClick={() => setMonth(new Date(new Date().getFullYear(), new Date().getMonth(), 1))}>
          {t("releases.this_month")}
        </button>
        <span className="flex-1" />
        <span className="text-[12px] text-fg-faint">{t("releases.agenda_hint")}</span>
      </div>
      <div className="grid grid-cols-7 gap-px overflow-hidden rounded-xl border border-border bg-border">
        {weekdays.map((w) => (
          <div key={w} className="bg-surface-2 px-2 py-1.5 text-center font-mono text-[10.5px] uppercase tracking-widest text-fg-subtle">
            {w}
          </div>
        ))}
        {days.map((d) => {
          const key = toDay(d);
          const inMonth = d.getMonth() === month.getMonth();
          return (
            <div
              key={key}
              onDragOver={(e) => {
                e.preventDefault();
                setOver(key);
              }}
              onDragLeave={() => setOver((o) => (o === key ? null : o))}
              onDrop={(e) => {
                e.preventDefault();
                setOver(null);
                const id = Number(e.dataTransfer.getData("text/plain"));
                if (id) onMove(id, key);
              }}
              className={clsx("group min-h-[104px] p-1.5", inMonth ? "bg-surface" : "bg-surface-2/60", over === key && "ring-2 ring-inset ring-accent")}
            >
              <div className="mb-1 flex items-center justify-between">
                <span
                  className={clsx(
                    "flex h-5 min-w-5 items-center justify-center rounded-full px-1 text-[11.5px]",
                    key === today() ? "bg-accent text-accent-fg" : inMonth ? "text-fg-subtle" : "text-fg-faint"
                  )}
                >
                  {d.getDate()}
                </span>
                <button className="rounded-full p-0.5 text-fg-faint opacity-0 hover:text-fg group-hover:opacity-100" onClick={() => onAdd(key)} aria-label={t("releases.add_card")}>
                  <Plus size={13} />
                </button>
              </div>
              <div className="space-y-1">
                {(byDay.get(key) ?? []).map((c) => (
                  <CardTile key={c.id} card={c} single={single} compact t={t} onOpen={() => onOpen(c.id)} />
                ))}
              </div>
            </div>
          );
        })}
      </div>
    </div>
  );
}

// ---------------- Release profile ----------------

function ReleaseModal({ release, t, onClose, onSaved }: { release: Release | null; t: T; onClose: () => void; onSaved: (id: number) => void }) {
  const toast = useToast();
  const [f, setF] = useState({
    name: release?.name ?? "",
    genre: release?.genre ?? "",
    master_state: release?.master_state ?? ("unmastered" as Release["master_state"]),
    artist: release?.artist ?? "",
    listen_url: release?.listen_url ?? "",
    notes: release?.notes ?? "",
  });
  const [date, setDate] = useState(release?.release_date ?? "");
  const [busy, setBusy] = useState(false);
  const save = async () => {
    setBusy(true);
    try {
      const id = await releaseSave({ ...f, id: release?.id ?? 0 });
      if (date && date !== release?.release_date) await releaseValidate(id, date);
      onSaved(id);
    } catch (e) {
      toast(errorMessage(e), "error");
      setBusy(false);
    }
  };
  return (
    <Modal open onClose={onClose} title={release ? t("releases.edit_title") : t("releases.new")}>
      <div className="space-y-3">
        <Field label={t("releases.f.name")}>
          <input className="input" autoFocus value={f.name} onChange={(e) => setF({ ...f, name: e.target.value })} />
        </Field>
        <div className="grid grid-cols-2 gap-3">
          <Field label={t("releases.f.genre")}>
            <input className="input" placeholder="Tech house" value={f.genre} onChange={(e) => setF({ ...f, genre: e.target.value })} />
          </Field>
          <Field label={t("releases.f.state")}>
            <select className="input" value={f.master_state} onChange={(e) => setF({ ...f, master_state: e.target.value as Release["master_state"] })}>
              <option value="unmastered">{t("releases.master.unmastered")}</option>
              <option value="master">{t("releases.master.master")}</option>
            </select>
          </Field>
        </div>
        <div className="grid grid-cols-2 gap-3">
          <Field label={t("releases.f.artist")}>
            <input className="input" placeholder="Piers" value={f.artist} onChange={(e) => setF({ ...f, artist: e.target.value })} />
          </Field>
          <Field label={release?.release_date ? t("releases.f.date") : t("releases.f.date_later")}>
            <input type="date" className="input" value={date} onChange={(e) => setDate(e.target.value)} />
          </Field>
        </div>
        <Field label={t("releases.f.link")}>
          <input className="input" placeholder="https://" value={f.listen_url} onChange={(e) => setF({ ...f, listen_url: e.target.value })} />
        </Field>
        <Field label={t("releases.f.notes")}>
          <textarea className="input min-h-[70px]" value={f.notes} onChange={(e) => setF({ ...f, notes: e.target.value })} />
        </Field>
        {release?.release_date && date && date !== release.release_date && <div className="text-[12.5px] text-accent-2">{t("releases.shift_hint")}</div>}
        <div className="flex justify-end gap-2 pt-1">
          <button className="btn-ghost" onClick={onClose}>
            {t("common.cancel")}
          </button>
          <button className="btn-primary" disabled={busy || !f.name.trim()} onClick={save}>
            {t("common.save")}
          </button>
        </div>
      </div>
    </Modal>
  );
}

function AddCardModal({
  day,
  releases,
  preset,
  t,
  onClose,
  onAdded,
}: {
  day: string;
  releases: Release[];
  preset: number | null;
  t: T;
  onClose: () => void;
  onAdded: (id: number) => void;
}) {
  const toast = useToast();
  const [releaseId, setReleaseId] = useState<number>(preset ?? releases[0]?.id ?? 0);
  const [kind, setKind] = useState<CardKind>("dj");
  const [title, setTitle] = useState("");
  return (
    <Modal open onClose={onClose} title={`${t("releases.add_card")} · ${formatDate(day)}`}>
      {releases.length === 0 ? (
        <div className="text-sm text-fg-subtle">{t("releases.add_needs_release")}</div>
      ) : (
        <div className="space-y-3">
          <Field label={t("releases.f.release")}>
            <select className="input" value={releaseId} onChange={(e) => setReleaseId(Number(e.target.value))}>
              {releases.map((r) => (
                <option key={r.id} value={r.id}>
                  {r.name}
                </option>
              ))}
            </select>
          </Field>
          <div>
            <span className="label">{t("releases.f.kind")}</span>
            <div className="flex flex-wrap gap-1.5">
              {CARD_KINDS.map((k) => (
                <button
                  key={k}
                  onClick={() => setKind(k)}
                  className={clsx("rounded-full border px-3 py-1 text-[12.5px]", kind === k ? "border-fg bg-fg text-bg" : "border-border hover:border-fg")}
                >
                  {t(`releases.kind.${k}`)}
                </button>
              ))}
            </div>
          </div>
          <Field label={t("releases.f.title")}>
            <input className="input" placeholder={t(`releases.kind.${kind}`)} value={title} onChange={(e) => setTitle(e.target.value)} />
          </Field>
          {isCampaign(kind) && <div className="text-[12.5px] text-fg-subtle">{t("releases.add_campaign_hint")}</div>}
          <div className="flex justify-end gap-2">
            <button className="btn-ghost" onClick={onClose}>
              {t("common.cancel")}
            </button>
            <button
              className="btn-primary"
              onClick={() =>
                cardAdd(releaseId, kind, day, title.trim() || (kind === "custom" ? t("releases.kind.custom") : undefined))
                  .then(onAdded)
                  .catch((e) => toast(errorMessage(e), "error"))
              }
            >
              <Plus size={14} /> {t("releases.add")}
            </button>
          </div>
        </div>
      )}
    </Modal>
  );
}

// ---------------- The card pop-up ----------------

function CardModal({ id, busy, t, onClose }: { id: number; busy: ReleaseProgress | null; t: T; onClose: () => void }) {
  const qc = useQueryClient();
  const toast = useToast();
  const confirm = useConfirm();
  const q = useQuery({ queryKey: ["release_card", id], queryFn: () => cardDetail(id) });
  const refresh = () => {
    qc.invalidateQueries({ queryKey: ["release_card", id] });
    qc.invalidateQueries({ queryKey: ["releases_board"] });
  };
  const run = (p: Promise<unknown>) => p.then(refresh).catch((e) => toast(errorMessage(e), "error"));
  const d = q.data;
  return (
    <Modal
      open
      onClose={onClose}
      width={d && isCampaign(d.card.kind) ? "max-w-4xl" : "max-w-2xl"}
      title={d ? <span>{d.card.title} <span className="text-fg-subtle">· {d.release.name}</span></span> : undefined}
    >
      {!d ? (
        q.isError ? <ErrorState onRetry={() => q.refetch()} /> : <Loader />
      ) : (
        <div>
          <div className="mb-4 flex flex-wrap items-center gap-2">
            <input type="date" className="input h-8 w-40" value={d.card.due_date} onChange={(e) => e.target.value && run(cardMove(id, { dueDate: e.target.value }))} />
            <select className="input h-8 w-44" value={d.card.col} onChange={(e) => run(cardMove(id, { col: e.target.value as Col }))}>
              {COLS.map((c) => (
                <option key={c} value={c}>
                  {t(`releases.col.${c}`)}
                </option>
              ))}
            </select>
            <span className="flex-1" />
            <button
              className="btn-ghost h-8 px-3"
              onClick={() =>
                confirm(t("releases.card_delete_confirm")) &&
                cardDelete(id)
                  .then(() => {
                    qc.invalidateQueries({ queryKey: ["releases_board"] });
                    onClose();
                  })
                  .catch((e) => toast(errorMessage(e), "error"))
              }
            >
              <Trash2 size={13} /> {t("releases.delete")}
            </button>
          </div>
          {d.card.kind === "meta" ? (
            <MetaBody card={d.card} t={t} run={run} />
          ) : isCampaign(d.card.kind) ? (
            <CampaignBody card={d.card} release={d.release} targets={d.targets} busy={busy} t={t} run={run} />
          ) : (
            <ChecklistBody card={d.card} t={t} run={run} />
          )}
        </div>
      )}
    </Modal>
  );
}

type Run = (p: Promise<unknown>) => void;

function MetaBody({ card, t, run }: { card: Card; t: T; run: Run }) {
  const toast = useToast();
  const [fields, setFields] = useState<Record<string, string>>(card.data.fields ?? {});
  const save = (next: Record<string, string>) => {
    const data: CardData = { ...card.data, fields: next };
    const missing = metaMissing(data);
    run(cardSetData(card.id, data, missing === 0 ? "done" : card.col === "todo" ? "doing" : card.col === "done" ? "doing" : undefined));
  };
  const missing = metaMissing({ fields });
  const sheet = META_FIELDS.map((f) => `${t(`releases.meta.${f.key}`)} : ${fields[f.key] ?? ""}`).join("\n");
  return (
    <div>
      <div className="mb-3 flex items-center gap-2 text-[12.5px]">
        {missing > 0 ? (
          <span className="badge bg-accent-2/15 text-accent-2">{t("releases.note.missing", { count: missing })}</span>
        ) : (
          <span className="badge bg-ok/15 text-ok">{t("releases.note.complete")}</span>
        )}
        <span className="text-fg-subtle">{t("releases.meta_hint")}</span>
        <span className="flex-1" />
        <button className="btn-outline h-8 px-3" onClick={() => navigator.clipboard.writeText(sheet).then(() => toast(t("releases.copied"), "ok"))}>
          <ClipboardCopy size={13} /> {t("releases.copy_sheet")}
        </button>
      </div>
      <div className="grid grid-cols-2 gap-3">
        {META_FIELDS.map((f) => {
          const empty = f.required && !(fields[f.key] ?? "").trim();
          return (
            <label key={f.key} className={clsx("block", f.wide && "col-span-2")}>
              <span className={clsx("label", empty && "text-accent-2")}>
                {t(`releases.meta.${f.key}`)}
                {f.required ? "" : ` (${t("releases.optional")})`}
              </span>
              <input
                className={clsx("input", empty && "border-accent-2/60")}
                type={f.key === "release_date" ? "date" : "text"}
                value={fields[f.key] ?? ""}
                onChange={(e) => setFields({ ...fields, [f.key]: e.target.value })}
                onBlur={() => fields[f.key] !== (card.data.fields?.[f.key] ?? "") && save(fields)}
              />
            </label>
          );
        })}
      </div>
    </div>
  );
}

function ChecklistBody({ card, t, run }: { card: Card; t: T; run: Run }) {
  const toast = useToast();
  const [data, setData] = useState<CardData>(card.data);
  const [step, setStep] = useState("");
  const [writing, setWriting] = useState(false);
  useEffect(() => setData(card.data), [card.data]);
  const steps = data.checklist ?? [];
  const save = (next: CardData) => {
    setData(next);
    const list = next.checklist ?? [];
    const done = list.filter((s) => s.done).length;
    const col: Col | undefined = list.length === 0 ? undefined : done === list.length ? "done" : done > 0 ? (card.col === "todo" || card.col === "done" ? "doing" : undefined) : card.col === "done" ? "todo" : undefined;
    run(cardSetData(card.id, next, col));
  };
  const canWrite = ["pitch", "groover", "release"].includes(card.kind);
  return (
    <div className="space-y-4">
      <div className="space-y-1">
        {steps.map((s, i) => (
          <label key={i} className="group flex cursor-pointer items-center gap-2.5 rounded-lg px-2 py-1.5 hover:bg-fg/[0.03]">
            <input
              type="checkbox"
              className="h-4 w-4 accent-[rgb(var(--accent))]"
              checked={s.done}
              onChange={() => save({ ...data, checklist: steps.map((x, j) => (j === i ? { ...x, done: !x.done } : x)) })}
            />
            <span className={clsx("flex-1 text-sm", s.done && "text-fg-subtle line-through")}>{s.label}</span>
            <button
              className="text-fg-faint opacity-0 hover:text-danger group-hover:opacity-100"
              onClick={(e) => {
                e.preventDefault();
                save({ ...data, checklist: steps.filter((_, j) => j !== i) });
              }}
              aria-label={t("releases.delete")}
            >
              <Trash2 size={13} />
            </button>
          </label>
        ))}
        <form
          className="flex gap-2 pt-1"
          onSubmit={(e) => {
            e.preventDefault();
            if (!step.trim()) return;
            save({ ...data, checklist: [...steps, { label: step.trim(), done: false }] });
            setStep("");
          }}
        >
          <input className="input h-8" placeholder={t("releases.add_step")} value={step} onChange={(e) => setStep(e.target.value)} />
        </form>
      </div>

      {(canWrite || data.text) && (
        <div>
          <div className="mb-1 flex items-center gap-2">
            <span className="label mb-0">{t(`releases.text_label.${canWrite ? card.kind : "custom"}`)}</span>
            <span className="flex-1" />
            {canWrite && (
              <button
                className="btn-outline h-7 px-3"
                disabled={writing}
                onClick={() => {
                  setWriting(true);
                  cardWrite(card.id)
                    .then((text) => setData((d) => ({ ...d, text })))
                    .catch((e) => toast(errorMessage(e), "error"))
                    .finally(() => setWriting(false));
                }}
              >
                <PenLine size={12} className={writing ? "animate-pulse" : undefined} /> {data.text ? t("releases.rewrite") : t("releases.write")}
              </button>
            )}
            {data.text && (
              <button className="btn-ghost h-7 px-3" onClick={() => navigator.clipboard.writeText(data.text ?? "").then(() => toast(t("releases.copied"), "ok"))}>
                <ClipboardCopy size={12} /> {t("releases.copy")}
              </button>
            )}
          </div>
          <textarea
            className="input min-h-[110px]"
            value={data.text ?? ""}
            onChange={(e) => setData({ ...data, text: e.target.value })}
            onBlur={() => data.text !== card.data.text && save(data)}
          />
          {data.text && <div className="mt-1 text-right text-[11.5px] text-fg-faint">{t("releases.chars", { count: data.text.length })}</div>}
        </div>
      )}

      <Field label={t("releases.f.notes")}>
        <textarea
          className="input min-h-[70px]"
          value={data.notes ?? ""}
          onChange={(e) => setData({ ...data, notes: e.target.value })}
          onBlur={() => data.notes !== card.data.notes && save(data)}
        />
      </Field>
      {data.url && (
        <button className="btn-outline" onClick={() => openExt(data.url)}>
          <ExternalLink size={13} /> {data.url.replace(/^https?:\/\//, "")}
        </button>
      )}
    </div>
  );
}

// ---------------- Campaign card ----------------

function TargetStatus({ target: x, t }: { target: Target; t: T }) {
  if (x.replied) return <span className="badge bg-ok/15 text-ok">{t("releases.st.replied")}</span>;
  if (x.status === "sent" && x.fu_status === "sent") return <span className="badge bg-fg/[0.07] text-fg-subtle">{t("releases.st.followed")}</span>;
  if (x.status === "sent" && x.fu_status === "queued") return <span className="badge bg-accent/15 text-accent">{t("releases.st.queued")}</span>;
  if (x.status === "sent" && x.opened_at)
    return (
      <span className="badge bg-accent/15 text-accent">
        <Eye size={11} /> {t("releases.st.opened", { count: x.open_count })}
      </span>
    );
  if (x.status === "sent") return <span className="badge bg-fg/[0.07] text-fg-subtle">{t("releases.st.sent", { date: formatDate(x.sent_at) })}</span>;
  if (x.status === "queued" || x.status === "sending") return <span className="badge bg-accent/15 text-accent">{t(`releases.st.${x.status}`)}</span>;
  if (x.status === "failed") return <span className="badge bg-danger/15 text-danger" title={x.error ?? undefined}>{t("releases.st.failed")}</span>;
  if (x.status === "draft") return <span className="badge bg-accent-2/15 text-accent-2">{t("releases.st.draft")}</span>;
  if (x.status === "skipped") return <span className="badge bg-fg/[0.07] text-fg-faint">{t("releases.st.skipped")}</span>;
  return <span className="badge bg-fg/[0.07] text-fg-subtle">{x.channel === "none" ? t("releases.st.no_contact") : t("releases.st.found")}</span>;
}

function CampaignBody({ card, release, targets, busy, t, run }: { card: Card; release: Release; targets: Target[]; busy: ReleaseProgress | null; t: T; run: Run }) {
  const toast = useToast();
  const [open, setOpen] = useState<number | null>(null);
  const [names, setNames] = useState("");
  const sendable = targets.filter((x) => (x.status === "draft" || x.status === "failed") && x.channel !== "none");
  const due = targets.filter((x) => x.followup_due);
  const [off, setOff] = useState<Set<number>>(new Set());
  const picked = sendable.filter((x) => !off.has(x.id));
  const pickedDue = due.filter((x) => !off.has(x.id));
  const toggle = (id: number) =>
    setOff((s) => {
      const n = new Set(s);
      if (n.has(id)) n.delete(id);
      else n.add(id);
      return n;
    });
  const working = !!busy && busy.stage !== "insta";
  const find = () => {
    const list = names.split(/[,\n]/).map((n) => n.trim()).filter(Boolean);
    setNames("");
    run(cardFind(card.id, list));
  };
  const shown = targets.filter((x) => x.status !== "skipped");
  const skipped = targets.length - shown.length;

  return (
    <div>
      {!release.listen_url && (
        <div className="mb-3 rounded-lg border border-accent-2/40 bg-accent-2/[0.06] px-3 py-2 text-[12.5px] text-accent-2">{t("releases.no_link_long")}</div>
      )}
      <div className="mb-3 flex flex-wrap items-center gap-2">
        <input
          className="input h-8 min-w-0 flex-1"
          placeholder={t(card.kind === "dj" ? "releases.names_dj" : "releases.names_pl")}
          value={names}
          onChange={(e) => setNames(e.target.value)}
          onKeyDown={(e) => e.key === "Enter" && find()}
        />
        <button className="btn-outline h-8 px-3" disabled={working} onClick={find}>
          {working ? <RefreshCw size={13} className="animate-spin" /> : <Search size={13} />}
          {working ? `${t(`releases.stage.${busy!.stage}`)} ${busy!.total ? `${busy!.done + 1}/${busy!.total}` : ""}` : names.trim() ? t("releases.find_these") : t("releases.find_more")}
        </button>
        <select
          className="input h-8 w-28"
          value={card.data.lang ?? "en"}
          title={t("releases.lang_hint")}
          onChange={(e) => run(cardSetData(card.id, { ...card.data, lang: e.target.value as "en" | "fr" }))}
        >
          <option value="en">English</option>
          <option value="fr">Français</option>
        </select>
      </div>

      {shown.length === 0 ? (
        <div className="py-10 text-center text-sm text-fg-subtle">{working ? t("releases.agent_working") : t("releases.no_targets")}</div>
      ) : (
        <div className="max-h-[52vh] divide-y divide-border overflow-y-auto rounded-xl border border-border">
          {shown.map((x) => {
            const selectable = sendable.includes(x) || due.includes(x);
            return (
              <div key={x.id}>
                <div className="flex items-center gap-3 px-3 py-2">
                  <input
                    type="checkbox"
                    className="h-4 w-4 accent-[rgb(var(--accent))] disabled:opacity-30"
                    disabled={!selectable}
                    checked={selectable && !off.has(x.id)}
                    onChange={() => toggle(x.id)}
                  />
                  <button className="min-w-0 flex-1 text-left" onClick={() => setOpen(open === x.id ? null : x.id)}>
                    <div className="truncate text-sm font-medium">{x.name}</div>
                    <div className="truncate text-[12px] text-fg-subtle">{x.why}</div>
                  </button>
                  <div className="w-56 shrink-0 truncate text-left text-[12.5px]">
                    {x.channel === "email" ? (
                      <span className="inline-flex items-center gap-1.5">
                        <Mail size={12} className="text-fg-subtle" /> {x.email}
                      </span>
                    ) : x.channel === "instagram" ? (
                      <button className="inline-flex items-center gap-1.5 text-left hover:underline" onClick={() => openExt(`https://www.instagram.com/${x.instagram}/`)}>
                        <AtSign size={12} className="text-fg-subtle" /> {x.instagram}
                      </button>
                    ) : (
                      <span className="text-fg-faint">{t("releases.st.no_contact")}</span>
                    )}
                  </div>
                  {x.trackstack ? (
                    <button className="btn-ghost h-7 shrink-0 px-2.5" title={t("releases.trackstack_hint")} onClick={() => openExt(x.trackstack)}>
                      Trackstack <ExternalLink size={11} />
                    </button>
                  ) : (
                    <span className="w-[92px] shrink-0" />
                  )}
                  <div className="flex w-32 shrink-0 justify-end">
                    <TargetStatus target={x} t={t} />
                  </div>
                </div>
                {open === x.id && <TargetEditor target={x} t={t} run={run} onClose={() => setOpen(null)} />}
              </div>
            );
          })}
        </div>
      )}

      <div className="mt-4 flex flex-wrap items-center gap-2">
        <span className="text-[12px] text-fg-faint">
          {t("releases.sender_line")}
          {skipped > 0 && ` · ${t("releases.skipped_count", { count: skipped })}`}
        </span>
        <span className="flex-1" />
        {due.length > 0 && (
          <button
            className="btn-outline"
            disabled={!pickedDue.length}
            onClick={() =>
              cardSend(card.id, pickedDue.map((x) => x.id), true)
                .then((n) => toast(t("releases.sending", { count: n }), "ok"))
                .then(() => run(Promise.resolve()))
                .catch((e) => toast(errorMessage(e), "error"))
            }
          >
            <Send size={13} /> {t("releases.send_followups", { count: pickedDue.length })}
          </button>
        )}
        <button
          className="btn-primary"
          disabled={!picked.length || working}
          onClick={() =>
            cardSend(card.id, picked.map((x) => x.id), false)
              .then((n) => toast(t("releases.sending", { count: n }), "ok"))
              .then(() => run(Promise.resolve()))
              .catch((e) => toast(errorMessage(e), "error"))
          }
        >
          <Send size={13} /> {t("releases.send", { count: picked.length })}
        </button>
      </div>
    </div>
  );
}

function TargetEditor({ target: x, t, run, onClose }: { target: Target; t: T; run: Run; onClose: () => void }) {
  const [f, setF] = useState({
    channel: x.channel,
    email: x.email ?? "",
    instagram: x.instagram ?? "",
    subject: x.subject ?? "",
    body: x.body ?? "",
    dm: x.dm ?? "",
    followup: x.followup ?? "",
  });
  const locked = x.status === "sent" || x.status === "queued" || x.status === "sending";
  const save = () => run(targetSave({ id: x.id, ...f }).then(onClose));
  return (
    <div className="space-y-3 bg-fg/[0.025] px-4 py-3">
      {x.error && <div className="text-[12.5px] text-danger">{x.error}</div>}
      <div className="grid grid-cols-3 gap-3">
        <Field label={t("releases.e.channel")}>
          <select className="input h-8" disabled={locked} value={f.channel} onChange={(e) => setF({ ...f, channel: e.target.value as Target["channel"] })}>
            <option value="email">Email</option>
            <option value="instagram">Instagram</option>
            <option value="none">{t("releases.e.none")}</option>
          </select>
        </Field>
        <Field label="Email">
          <input className="input h-8" disabled={locked} value={f.email} onChange={(e) => setF({ ...f, email: e.target.value })} />
        </Field>
        <Field label="Instagram">
          <input className="input h-8" disabled={locked} value={f.instagram} onChange={(e) => setF({ ...f, instagram: e.target.value })} />
        </Field>
      </div>
      {x.source_url && (
        <button className="text-[12px] text-fg-subtle hover:underline" onClick={() => openExt(x.source_url)}>
          {t("releases.e.source")} {x.source_url.replace(/^https?:\/\//, "").slice(0, 70)}
        </button>
      )}
      {f.channel === "instagram" ? (
        <Field label={t("releases.e.dm")}>
          <textarea className="input min-h-[80px]" disabled={locked} value={f.dm} onChange={(e) => setF({ ...f, dm: e.target.value })} />
        </Field>
      ) : (
        <>
          <Field label={t("releases.e.subject")}>
            <input className="input h-8" disabled={locked} value={f.subject} onChange={(e) => setF({ ...f, subject: e.target.value })} />
          </Field>
          <Field label={t("releases.e.body")}>
            <textarea className="input min-h-[150px]" disabled={locked} value={f.body} onChange={(e) => setF({ ...f, body: e.target.value })} />
          </Field>
        </>
      )}
      <Field label={t("releases.e.followup")}>
        <textarea className="input min-h-[60px]" disabled={x.fu_status != null} value={f.followup} onChange={(e) => setF({ ...f, followup: e.target.value })} />
      </Field>
      <div className="flex flex-wrap items-center gap-2">
        {x.status === "sent" && (
          <button className="btn-ghost h-8 px-3" onClick={() => run(targetsMark([x.id], x.replied ? "not_replied" : "replied"))}>
            <Check size={13} /> {x.replied ? t("releases.e.unmark_replied") : t("releases.e.mark_replied")}
          </button>
        )}
        {x.status === "sent" && x.fu_status == null && (
          <button className="btn-ghost h-8 px-3" onClick={() => run(targetsMark([x.id], "no_followup"))}>
            {t("releases.e.no_followup")}
          </button>
        )}
        {x.status !== "sent" && x.status !== "sending" && (
          <button className="btn-ghost h-8 px-3" onClick={() => run(targetsMark([x.id], x.status === "queued" ? "restore" : "skip").then(onClose))}>
            {x.status === "queued" ? t("releases.e.unqueue") : t("releases.e.skip")}
          </button>
        )}
        <span className="flex-1" />
        <button className="btn-ghost h-8 px-3" onClick={onClose}>
          {t("common.cancel")}
        </button>
        <button className="btn-primary h-8 px-3" onClick={save}>
          {t("common.save")}
        </button>
      </div>
    </div>
  );
}
