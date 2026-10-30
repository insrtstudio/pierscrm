import { useEffect, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import clsx from "clsx";
import { BookOpen, Check, Plus, PowerOff, ShieldCheck, Trash2, Undo2 } from "lucide-react";
import {
  AUTONOMY_CATEGORIES,
  autonomyOverview,
  autonomySave,
  autonomyStopAll,
  playbookDelete,
  playbookList,
  playbookSave,
  playbookSetStatus,
  type AutonomySettings,
  type EntryKind,
  type PlaybookEntry,
} from "../lib/playbook";
import { Field, useConfirm, useSubmit, useToast } from "./ui";

const KINDS: EntryKind[] = ["fact", "answer", "objection"];

function invalidate(qc: ReturnType<typeof useQueryClient>) {
  for (const k of ["playbook", "autonomy", "reply_desk"]) qc.invalidateQueries({ queryKey: [k] });
}

/// Settings › Agent: the agent's internal name, its autonomy per category and
/// the library of approved answers it writes from.
export function AgentLibraryPanel() {
  const { t } = useTranslation();
  const { data } = useQuery({ queryKey: ["playbook"], queryFn: playbookList });
  const approved = data?.entries.filter((e) => e.status === "approved").length ?? 0;
  return (
    <div className="space-y-6">
      <AutonomyCard />
      <div className="card p-6">
        <div className="mb-1 flex items-center gap-2 text-sm font-semibold">
          <BookOpen size={15} /> {t("playbook.title")}
        </div>
        <p className="text-xs text-fg-subtle">{t("playbook.help")}</p>
        {data && (
          <p className="mt-2 text-xs text-fg-muted">
            {t("playbook.approved_count", { count: approved, total: data.entries.length })}
          </p>
        )}
      </div>
      {data && <ArtistFactsCard artist={data.artist} />}
      {data &&
        KINDS.map((k) => (
          <KindSection
            key={k}
            kind={k}
            entries={data.entries.filter((e) => e.kind === k)}
            topics={data.topics.filter((x) => x.kind === k).map((x) => x.topic)}
          />
        ))}
    </div>
  );
}

function AutonomyCard() {
  const { t } = useTranslation();
  const toast = useToast();
  const qc = useQueryClient();
  const { data } = useQuery({ queryKey: ["autonomy"], queryFn: autonomyOverview });
  const [s, setS] = useState<AutonomySettings | null>(null);
  useEffect(() => {
    if (data) setS(data.settings);
  }, [data]);
  const [save, saving] = useSubmit(async (next: AutonomySettings) => {
    const r = await autonomySave(next);
    setS(r);
    invalidate(qc);
    toast(t("settings.saved"), "ok");
  });
  const [stop, stopping] = useSubmit(async () => {
    const n = await autonomyStopAll();
    invalidate(qc);
    toast(t("autonomy.stopped", { count: n }), "ok");
  });
  if (!s) return null;
  const level = (c: string) => s.levels.find((l) => l.category === c)?.level ?? 0;
  const setLevel = (c: string, v: number) =>
    setS({
      ...s,
      levels: AUTONOMY_CATEGORIES.map((k) => ({ category: k, level: k === c ? v : level(k) })),
    });
  const anyOn = s.enabled && s.levels.some((l) => l.level > 0);
  return (
    <div className="card p-6">
      <div className="mb-1 flex items-center gap-2 text-sm font-semibold">
        <ShieldCheck size={15} /> {t("autonomy.title")}
      </div>
      <p className="mb-4 text-xs text-fg-subtle">{t("autonomy.help")}</p>

      <div className="grid grid-cols-2 gap-x-4 gap-y-3">
        <Field label={t("autonomy.agent_name")}>
          <input
            className="input"
            value={s.agent_name}
            maxLength={40}
            onChange={(e) => setS({ ...s, agent_name: e.target.value })}
          />
        </Field>
        <Field label={t("autonomy.delay")}>
          <input
            type="number"
            min={5}
            max={240}
            className="input"
            value={s.delay_min}
            onChange={(e) => setS({ ...s, delay_min: Math.round(Number(e.target.value)) || 15 })}
          />
        </Field>
      </div>
      <p className="mt-1 text-2xs text-fg-faint">{t("autonomy.agent_name_hint")}</p>

      <label className="mt-4 flex items-center gap-2 text-sm">
        <input type="checkbox" checked={s.enabled} onChange={(e) => setS({ ...s, enabled: e.target.checked })} />
        {t("autonomy.enabled")}
      </label>

      <div className="mt-4 space-y-3">
        {AUTONOMY_CATEGORIES.map((c) => (
          <div key={c} className="flex flex-wrap items-start gap-3 rounded-md border border-border p-3">
            <div className="min-w-[14rem] flex-1">
              <p className="text-sm font-medium">{t(`autonomy.cat_${c}`)}</p>
              <p className="text-xs text-fg-subtle">{t(`autonomy.cat_${c}_help`)}</p>
            </div>
            <select
              className="input w-56"
              value={level(c)}
              disabled={!s.enabled}
              onChange={(e) => setLevel(c, Number(e.target.value))}
            >
              <option value={0}>{t("autonomy.level_0")}</option>
              <option value={1}>{t("autonomy.level_1")}</option>
            </select>
          </div>
        ))}
      </div>

      <div className="mt-4 rounded-md bg-muted/40 p-3 text-xs text-fg-muted">
        <p className="font-medium text-fg">{t("autonomy.never_title")}</p>
        <p className="mt-1">{t("autonomy.never_body", { pct: Math.round(s.min_confidence * 100) })}</p>
      </div>

      <div className="mt-4 flex flex-wrap items-center justify-end gap-2">
        {anyOn && (
          <button className="btn-outline text-danger" onClick={() => stop()} disabled={stopping}>
            <PowerOff size={14} />
            {t("autonomy.stop_all")}
          </button>
        )}
        <button className="btn-primary" onClick={() => save(s)} disabled={saving}>
          {t("common.save")}
        </button>
      </div>
    </div>
  );
}

function ArtistFactsCard({ artist }: { artist: import("../lib/playbook").ArtistFacts }) {
  const { t } = useTranslation();
  const rows: [string, string | null][] = [
    [t("playbook.af_fee"), artist.fee_range],
    [t("playbook.af_rider"), artist.tech_rider],
    [t("playbook.af_mix"), artist.mix_url],
    [t("playbook.af_links"), artist.links],
  ];
  return (
    <div className="card p-6">
      <p className="text-sm font-semibold">{t("playbook.artist_title", { name: artist.name || "·" })}</p>
      <p className="mb-3 text-xs text-fg-subtle">{t("playbook.artist_help")}</p>
      <dl className="grid grid-cols-[10rem_1fr] gap-x-4 gap-y-1 text-sm">
        {rows.map(([k, v]) => (
          <div key={k} className="contents">
            <dt className="text-fg-muted">{k}</dt>
            <dd className="whitespace-pre-wrap break-words">{v?.trim() || <span className="text-fg-faint">{t("playbook.af_empty")}</span>}</dd>
          </div>
        ))}
        <dt className="text-fg-muted">{t("playbook.af_played")}</dt>
        <dd>
          {artist.played_at.length === 0 ? (
            <span className="text-fg-faint">{t("playbook.af_played_empty")}</span>
          ) : (
            <ul className="list-inside list-disc">
              {artist.played_at.map((p) => (
                <li key={p}>{p}</li>
              ))}
            </ul>
          )}
        </dd>
      </dl>
    </div>
  );
}

function KindSection({ kind, entries, topics }: { kind: EntryKind; entries: PlaybookEntry[]; topics: string[] }) {
  const { t } = useTranslation();
  const qc = useQueryClient();
  const [topic, setTopic] = useState(topics[topics.length - 1] ?? "");
  const [add, adding] = useSubmit(async () => {
    await playbookSave({ kind, topic, title: "", text_fr: "", text_en: "", status: "draft" });
    invalidate(qc);
  });
  return (
    <section className="space-y-3">
      <div className="flex flex-wrap items-end gap-3">
        <div className="flex-1">
          <h3 className="text-sm font-semibold">{t(`playbook.kind_${kind}`)}</h3>
          <p className="text-xs text-fg-subtle">{t(`playbook.kind_${kind}_help`)}</p>
        </div>
        <select className="input w-64" value={topic} onChange={(e) => setTopic(e.target.value)}>
          {topics.map((x) => (
            <option key={x} value={x}>
              {t(`playbook.topic_${x}`)}
            </option>
          ))}
        </select>
        <button className="btn-outline" onClick={() => add()} disabled={adding || !topic}>
          <Plus size={14} />
          {t("playbook.add")}
        </button>
      </div>
      {entries.map((e) => (
        <EntryEditor key={e.id} entry={e} />
      ))}
    </section>
  );
}

function EntryEditor({ entry }: { entry: PlaybookEntry }) {
  const { t } = useTranslation();
  const qc = useQueryClient();
  const toast = useToast();
  const confirm = useConfirm();
  const [title, setTitle] = useState(entry.title);
  const [fr, setFr] = useState(entry.text_fr);
  const [en, setEn] = useState(entry.text_en);
  useEffect(() => {
    setTitle(entry.title);
    setFr(entry.text_fr);
    setEn(entry.text_en);
  }, [entry.title, entry.text_fr, entry.text_en]);
  const dirty = title !== entry.title || fr !== entry.text_fr || en !== entry.text_en;
  const approved = entry.status === "approved";
  const empty = !fr.trim() && !en.trim();

  const [save, saving] = useSubmit(async (status: "draft" | "approved") => {
    await playbookSave({ ...entry, title, text_fr: fr, text_en: en, status });
    invalidate(qc);
    toast(status === "approved" ? t("playbook.approved_toast") : t("settings.saved"), "ok");
  });
  const [toggle, toggling] = useSubmit(async () => {
    if (dirty) return save(approved ? "draft" : "approved");
    await playbookSetStatus(entry.id, approved ? "draft" : "approved");
    invalidate(qc);
  });
  const [remove, removing] = useSubmit(async () => {
    if (!confirm(t("playbook.delete_confirm"))) return;
    await playbookDelete(entry.id);
    invalidate(qc);
  });
  const busy = saving || toggling || removing;
  const needsTitle = entry.kind === "objection" || entry.topic.startsWith("other") || entry.topic === "obj_other";

  return (
    <div className={clsx("card space-y-3 p-4", approved && "border-accent/40")}>
      <div className="flex flex-wrap items-center gap-2">
        <span className="text-sm font-medium">{t(`playbook.topic_${entry.topic}`)}</span>
        <span
          className={clsx(
            "rounded-full px-2 py-0.5 text-[11px]",
            approved ? "bg-accent/15 text-accent-2" : "bg-muted text-fg-muted"
          )}
        >
          {approved ? t("playbook.status_approved") : t("playbook.status_draft")}
        </span>
        <span className="ml-auto text-2xs text-fg-faint">#{entry.id}</span>
      </div>
      {needsTitle && (
        <input
          className="input w-full"
          placeholder={t(entry.kind === "objection" ? "playbook.objection_placeholder" : "playbook.title_placeholder")}
          value={title}
          onChange={(e) => setTitle(e.target.value)}
        />
      )}
      <div className="grid grid-cols-2 gap-3">
        <Field label={t("playbook.text_fr")}>
          <textarea className="input min-h-[6rem] w-full font-[inherit]" value={fr} onChange={(e) => setFr(e.target.value)} />
        </Field>
        <Field label={t("playbook.text_en")}>
          <textarea className="input min-h-[6rem] w-full font-[inherit]" value={en} onChange={(e) => setEn(e.target.value)} />
        </Field>
      </div>
      <div className="flex flex-wrap items-center gap-2">
        <button
          className={approved ? "btn-outline" : "btn-primary"}
          disabled={busy || (!approved && empty)}
          onClick={() => toggle()}
          title={!approved && empty ? t("playbook.empty_hint") : undefined}
        >
          {approved ? <Undo2 size={14} /> : <Check size={14} />}
          {approved ? t("playbook.unapprove") : t("playbook.approve")}
        </button>
        {dirty && (
          <button className="btn-outline" disabled={busy} onClick={() => save(entry.status)}>
            {t("common.save")}
          </button>
        )}
        <button className="btn-ghost ml-auto" disabled={busy} onClick={() => remove()}>
          <Trash2 size={14} />
          {t("playbook.delete")}
        </button>
      </div>
    </div>
  );
}
