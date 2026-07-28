import { useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { openUrl } from "@tauri-apps/plugin-opener";
import {
  Plus,
  Pencil,
  Trash2,
  Plane,
  AlertTriangle,
  ExternalLink,
  Check,
  X,
  FileText,
} from "lucide-react";
import clsx from "clsx";
import {
  deleteDossier,
  deleteVisaCountry,
  listArtists,
  listDossiers,
  listVisaCountries,
  saveDossier,
  saveVisaCountry,
} from "../lib/api";
import type { ChecklistItem, VisaCountry, VisaDossier } from "../lib/types";
import { VISA_STATUSES } from "../lib/types";
import { PageHeader } from "../components/Layout";
import { Modal, Field, useToast, useConfirm } from "../components/ui";

type Tab = "dossiers" | "countries";

const STATUS_STYLE: Record<string, string> = {
  draft: "bg-slate-500/10 text-slate-500",
  preparing: "bg-amber-500/10 text-amber-500",
  submitted: "bg-blue-500/10 text-blue-500",
  approved: "bg-emerald-500/10 text-emerald-500",
  rejected: "bg-rose-500/10 text-rose-500",
};

function parseChecklist(s?: string | null): ChecklistItem[] {
  if (!s) return [];
  try {
    const v = JSON.parse(s);
    return Array.isArray(v) ? v : [];
  } catch {
    return [];
  }
}

export function Visa() {
  const { t } = useTranslation();
  const [tab, setTab] = useState<Tab>("dossiers");

  return (
    <div>
      <PageHeader title={t("visa.title")} subtitle={t("visa.subtitle")} />
      <div className="px-8 pb-10">
        <div className="mb-4 flex items-start gap-2.5 rounded-xl border border-amber-500/30 bg-amber-500/5 px-4 py-3 text-xs text-amber-600 dark:text-amber-400">
          <AlertTriangle size={15} className="mt-0.5 shrink-0" />
          <span>{t("visa.disclaimer")}</span>
        </div>

        <div className="mb-5 inline-flex rounded-lg border border-border bg-surface p-1">
          {(["dossiers", "countries"] as Tab[]).map((tb) => (
            <button
              key={tb}
              onClick={() => setTab(tb)}
              className={clsx(
                "rounded-md px-4 py-1.5 text-sm font-medium transition-colors",
                tab === tb ? "bg-accent text-accent-fg" : "text-fg-subtle hover:text-fg"
              )}
            >
              {t(`visa.tab_${tb}`)}
            </button>
          ))}
        </div>

        {tab === "dossiers" ? <DossiersTab /> : <CountriesTab />}
      </div>
    </div>
  );
}

// ---------------- Dossiers ----------------

function DossiersTab() {
  const { t } = useTranslation();
  const qc = useQueryClient();
  const confirm = useConfirm();
  const { data: dossiers = [] } = useQuery({
    queryKey: ["dossiers"],
    queryFn: listDossiers,
  });
  const { data: artists = [] } = useQuery({ queryKey: ["artists"], queryFn: listArtists });
  const [editing, setEditing] = useState<VisaDossier | null>(null);

  const delMut = useMutation({
    mutationFn: (id: number) => deleteDossier(id),
    onSuccess: () => qc.invalidateQueries({ queryKey: ["dossiers"] }),
  });

  const artistName = (id?: number | null) =>
    artists.find((a) => a.id === id)?.name ?? t("visa.no_artist");

  return (
    <div className="space-y-4">
      <button
        className="btn-primary"
        onClick={() =>
          setEditing({ title: "", status: "draft", checklist: "[]" } as VisaDossier)
        }
      >
        <Plus size={16} />
        {t("visa.new_dossier")}
      </button>

      {dossiers.length === 0 ? (
        <div className="card flex flex-col items-center gap-3 py-16 text-sm text-fg-subtle">
          <Plane size={28} className="opacity-40" />
          {t("visa.no_dossiers")}
        </div>
      ) : (
        <div className="grid grid-cols-1 gap-4 md:grid-cols-2 xl:grid-cols-3">
          {dossiers.map((d) => {
            const items = parseChecklist(d.checklist);
            const done = items.filter((i) => i.done).length;
            const pct = items.length ? Math.round((done / items.length) * 100) : 0;
            return (
              <div key={d.id} className="card group p-5">
                <div className="flex items-start justify-between gap-2">
                  <div className="min-w-0">
                    <div className="truncate text-sm font-semibold">{d.title}</div>
                    <div className="mt-0.5 text-xs text-fg-subtle">
                      {[d.country_name, artistName(d.artist_id)]
                        .filter(Boolean)
                        .join(" · ")}
                    </div>
                  </div>
                  <span
                    className={clsx(
                      "shrink-0 rounded-full px-2 py-0.5 text-xs font-medium",
                      STATUS_STYLE[d.status] ?? "bg-muted text-fg-subtle"
                    )}
                  >
                    {t(`visa.st_${d.status}`, d.status)}
                  </span>
                </div>

                {(d.event_date || d.entry_date) && (
                  <div className="mt-2 text-xs text-fg-subtle">
                    {[
                      d.event_date && `${t("visa.event_date")}: ${d.event_date}`,
                      d.entry_date && `${t("visa.entry_date")}: ${d.entry_date}`,
                    ]
                      .filter(Boolean)
                      .join(" · ")}
                  </div>
                )}

                <div className="mt-4">
                  <div className="mb-1 flex items-center justify-between text-xs text-fg-subtle">
                    <span>{t("visa.progress")}</span>
                    <span>
                      {done}/{items.length}
                    </span>
                  </div>
                  <div className="h-2 overflow-hidden rounded-full bg-muted">
                    <div
                      className="h-full rounded-full bg-accent transition-all"
                      style={{ width: `${pct}%` }}
                    />
                  </div>
                </div>

                <div className="mt-4 flex justify-end gap-1 opacity-0 transition-opacity group-hover:opacity-100">
                  <button className="btn-ghost px-2 py-1.5" onClick={() => setEditing(d)}>
                    <Pencil size={14} />
                  </button>
                  <button
                    className="btn-ghost px-2 py-1.5 text-rose-500"
                    onClick={() =>
                      confirm(t("common.confirm_delete")) && delMut.mutate(d.id!)
                    }
                  >
                    <Trash2 size={14} />
                  </button>
                </div>
              </div>
            );
          })}
        </div>
      )}

      {editing && (
        <DossierModal
          dossier={editing}
          onClose={() => setEditing(null)}
          onSaved={() => {
            setEditing(null);
            qc.invalidateQueries({ queryKey: ["dossiers"] });
          }}
        />
      )}
    </div>
  );
}

function DossierModal({
  dossier,
  onClose,
  onSaved,
}: {
  dossier: VisaDossier;
  onClose: () => void;
  onSaved: () => void;
}) {
  const { t } = useTranslation();
  const toast = useToast();
  const { data: artists = [] } = useQuery({ queryKey: ["artists"], queryFn: listArtists });
  const { data: countries = [] } = useQuery({
    queryKey: ["visa_countries"],
    queryFn: listVisaCountries,
  });
  const [form, setForm] = useState<VisaDossier>({ ...dossier });
  const [items, setItems] = useState<ChecklistItem[]>(parseChecklist(dossier.checklist));
  const [newDoc, setNewDoc] = useState("");

  const set = (k: keyof VisaDossier, v: any) => setForm((f) => ({ ...f, [k]: v }));

  const onCountry = (code: string) => {
    const c = countries.find((x) => x.code === code);
    set("country_code", code);
    set("country_name", c?.name ?? "");
  };

  const prefill = () => {
    const c = countries.find((x) => x.code === form.country_code);
    if (!c?.required_docs) return;
    const docs = c.required_docs
      .split("\n")
      .map((s) => s.trim())
      .filter(Boolean)
      .map((label) => ({ label, done: false }));
    // merge, avoiding duplicates by label
    const existing = new Set(items.map((i) => i.label));
    setItems([...items, ...docs.filter((d) => !existing.has(d.label))]);
  };

  const save = async () => {
    if (!form.title.trim()) {
      toast(t("visa.dossier_title") + " " + t("common.required"), "error");
      return;
    }
    await saveDossier({ ...form, checklist: JSON.stringify(items) });
    toast(t("common.save"), "ok");
    onSaved();
  };

  const doneCount = items.filter((i) => i.done).length;

  return (
    <Modal
      open
      onClose={onClose}
      width="max-w-2xl"
      title={dossier.id ? t("visa.edit_dossier") : t("visa.new_dossier")}
    >
      <div className="space-y-4">
        <Field label={t("visa.dossier_title")}>
          <input
            className="input"
            value={form.title}
            onChange={(e) => set("title", e.target.value)}
            placeholder="ADE 2026 · Piers"
          />
        </Field>
        <div className="grid grid-cols-2 gap-x-4 gap-y-3">
          <Field label={t("visa.artist")}>
            <select
              className="input"
              value={form.artist_id ?? ""}
              onChange={(e) =>
                set("artist_id", e.target.value ? Number(e.target.value) : null)
              }
            >
              <option value="">{t("visa.no_artist")}</option>
              {artists.map((a) => (
                <option key={a.id} value={a.id}>
                  {a.name}
                </option>
              ))}
            </select>
          </Field>
          <Field label={t("visa.country")}>
            <select
              className="input"
              value={form.country_code ?? ""}
              onChange={(e) => onCountry(e.target.value)}
            >
              <option value="">{t("visa.select_country")}</option>
              {countries.map((c) => (
                <option key={c.code} value={c.code}>
                  {c.name}
                </option>
              ))}
            </select>
          </Field>
          <Field label={t("visa.purpose")}>
            <input
              className="input"
              value={form.purpose ?? ""}
              onChange={(e) => set("purpose", e.target.value)}
              placeholder="Performance, promo…"
            />
          </Field>
          <Field label={t("visa.status")}>
            <select
              className="input"
              value={form.status}
              onChange={(e) => set("status", e.target.value)}
            >
              {VISA_STATUSES.map((s) => (
                <option key={s} value={s}>
                  {t(`visa.st_${s}`)}
                </option>
              ))}
            </select>
          </Field>
          <Field label={t("visa.event_date")}>
            <input
              className="input"
              value={form.event_date ?? ""}
              onChange={(e) => set("event_date", e.target.value)}
            />
          </Field>
          <Field label={t("visa.entry_date")}>
            <input
              className="input"
              value={form.entry_date ?? ""}
              onChange={(e) => set("entry_date", e.target.value)}
            />
          </Field>
        </div>

        {/* Checklist */}
        <div>
          <div className="mb-2 flex items-center justify-between">
            <span className="label mb-0">
              {t("visa.checklist")} · {doneCount}/{items.length}
            </span>
            {form.country_code && (
              <button className="btn-ghost px-2 py-1 text-xs" onClick={prefill}>
                <FileText size={13} />
                {t("visa.prefill_docs")}
              </button>
            )}
          </div>
          <div className="space-y-1.5">
            {items.map((it, idx) => (
              <div
                key={idx}
                className="flex items-center gap-2 rounded-lg border border-border px-3 py-1.5"
              >
                <button
                  onClick={() =>
                    setItems((arr) =>
                      arr.map((x, i) => (i === idx ? { ...x, done: !x.done } : x))
                    )
                  }
                  className={clsx(
                    "flex h-5 w-5 shrink-0 items-center justify-center rounded-md border",
                    it.done
                      ? "border-emerald-500 bg-emerald-500 text-white"
                      : "border-border"
                  )}
                >
                  {it.done && <Check size={13} />}
                </button>
                <span
                  className={clsx(
                    "flex-1 text-sm",
                    it.done && "text-fg-subtle line-through"
                  )}
                >
                  {it.label}
                </span>
                <button
                  className="btn-ghost px-1.5 py-1 text-rose-500"
                  onClick={() => setItems((arr) => arr.filter((_, i) => i !== idx))}
                >
                  <X size={13} />
                </button>
              </div>
            ))}
          </div>
          <div className="mt-2 flex gap-2">
            <input
              className="input flex-1"
              value={newDoc}
              placeholder={t("visa.add_doc")}
              onChange={(e) => setNewDoc(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter" && newDoc.trim()) {
                  setItems([...items, { label: newDoc.trim(), done: false }]);
                  setNewDoc("");
                }
              }}
            />
            <button
              className="btn-outline"
              onClick={() => {
                if (newDoc.trim()) {
                  setItems([...items, { label: newDoc.trim(), done: false }]);
                  setNewDoc("");
                }
              }}
            >
              <Plus size={15} />
            </button>
          </div>
        </div>

        <Field label={t("visa.notes")}>
          <textarea
            className="input min-h-[70px]"
            value={form.notes ?? ""}
            onChange={(e) => set("notes", e.target.value)}
          />
        </Field>

        <div className="flex justify-end gap-2">
          <button className="btn-outline" onClick={onClose}>
            {t("common.cancel")}
          </button>
          <button className="btn-primary" onClick={save}>
            {t("common.save")}
          </button>
        </div>
      </div>
    </Modal>
  );
}

// ---------------- Countries ----------------

function CountriesTab() {
  const { t } = useTranslation();
  const qc = useQueryClient();
  const confirm = useConfirm();
  const { data: countries = [] } = useQuery({
    queryKey: ["visa_countries"],
    queryFn: listVisaCountries,
  });
  const [selected, setSelected] = useState<string | null>(null);
  const [editing, setEditing] = useState<VisaCountry | null>(null);

  const current = countries.find((c) => c.code === selected) ?? countries[0];

  const delMut = useMutation({
    mutationFn: (code: string) => deleteVisaCountry(code),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ["visa_countries"] });
      setSelected(null);
    },
  });

  return (
    <div className="grid grid-cols-1 gap-5 lg:grid-cols-[240px_1fr]">
      {/* Country list */}
      <div className="card h-fit overflow-hidden">
        <div className="flex items-center justify-between border-b border-border px-3 py-2">
          <span className="text-xs font-medium text-fg-subtle">
            {t("visa.country")}
          </span>
          <button
            className="btn-ghost px-1.5 py-1"
            onClick={() => setEditing({ code: "", name: "" } as VisaCountry)}
          >
            <Plus size={15} />
          </button>
        </div>
        <div className="max-h-[60vh] overflow-y-auto py-1">
          {countries.map((c) => (
            <button
              key={c.code}
              onClick={() => setSelected(c.code)}
              className={clsx(
                "flex w-full items-center gap-2 px-3 py-2 text-left text-sm transition-colors",
                current?.code === c.code
                  ? "bg-accent/10 text-accent"
                  : "hover:bg-muted"
              )}
            >
              <span className="inline-flex h-6 w-8 items-center justify-center rounded bg-muted text-[10px] font-bold">
                {c.code}
              </span>
              {c.name}
            </button>
          ))}
        </div>
      </div>

      {/* Country detail */}
      {current ? (
        <div className="card p-6">
          <div className="flex items-start justify-between">
            <div>
              <h2 className="text-lg font-semibold">{current.name}</h2>
              {current.official_link && (
                <button
                  className="mt-1 inline-flex items-center gap-1 text-xs text-accent hover:underline"
                  onClick={() => openUrl(current.official_link!).catch(() => {})}
                >
                  <ExternalLink size={12} />
                  {t("visa.open_official")}
                </button>
              )}
            </div>
            <div className="flex gap-1">
              <button className="btn-outline" onClick={() => setEditing(current)}>
                <Pencil size={14} />
                {t("common.edit")}
              </button>
              <button
                className="btn-ghost px-2 py-1.5 text-rose-500"
                onClick={() =>
                  confirm(t("common.confirm_delete")) && delMut.mutate(current.code)
                }
              >
                <Trash2 size={15} />
              </button>
            </div>
          </div>

          <div className="mt-5 space-y-5">
            <CountryField label={t("visa.work_rules")} value={current.work_rules} />
            <div className="grid grid-cols-1 gap-5 md:grid-cols-2">
              <CountryField label={t("visa.visa_types")} value={current.visa_types} />
              <CountryField
                label={t("visa.processing_time")}
                value={current.processing_time}
              />
            </div>
            <CountryField
              label={t("visa.required_docs")}
              value={current.required_docs}
              mono
            />
            <CountryField label={t("visa.notes")} value={current.notes} />
          </div>
        </div>
      ) : (
        <div className="card flex items-center justify-center py-16 text-sm text-fg-subtle">
          {t("common.empty")}
        </div>
      )}

      {editing && (
        <CountryModal
          country={editing}
          onClose={() => setEditing(null)}
          onSaved={() => {
            setEditing(null);
            qc.invalidateQueries({ queryKey: ["visa_countries"] });
          }}
        />
      )}
    </div>
  );
}

function CountryField({
  label,
  value,
  mono,
}: {
  label: string;
  value?: string | null;
  mono?: boolean;
}) {
  if (!value) return null;
  return (
    <div>
      <div className="mb-1 text-xs font-semibold uppercase tracking-wide text-fg-subtle">
        {label}
      </div>
      <p
        className={clsx(
          "whitespace-pre-wrap text-sm leading-relaxed",
          mono && "font-mono text-[13px]"
        )}
      >
        {value}
      </p>
    </div>
  );
}

function CountryModal({
  country,
  onClose,
  onSaved,
}: {
  country: VisaCountry;
  onClose: () => void;
  onSaved: () => void;
}) {
  const { t } = useTranslation();
  const toast = useToast();
  const [form, setForm] = useState<VisaCountry>({ ...country });
  const isNew = !country.code;
  const inp = (k: keyof VisaCountry) => ({
    className: "input",
    value: (form[k] as string) ?? "",
    onChange: (e: any) => setForm({ ...form, [k]: e.target.value }),
  });
  const save = async () => {
    if (!form.code.trim() || !form.name.trim()) {
      toast(t("common.required"), "error");
      return;
    }
    await saveVisaCountry(form);
    toast(t("common.save"), "ok");
    onSaved();
  };
  return (
    <Modal
      open
      onClose={onClose}
      width="max-w-2xl"
      title={isNew ? t("visa.add_country") : t("visa.edit_country")}
    >
      <div className="space-y-3.5">
        <div className="grid grid-cols-[120px_1fr] gap-4">
          <Field label={t("visa.country_code")}>
            <input {...inp("code")} disabled={!isNew} placeholder="NL" />
          </Field>
          <Field label={t("visa.country_name")}>
            <input {...inp("name")} />
          </Field>
        </div>
        <Field label={t("visa.work_rules")}>
          <textarea {...inp("work_rules")} className="input min-h-[90px]" />
        </Field>
        <div className="grid grid-cols-2 gap-4">
          <Field label={t("visa.visa_types")}>
            <textarea {...inp("visa_types")} className="input min-h-[60px]" />
          </Field>
          <Field label={t("visa.processing_time")}>
            <textarea {...inp("processing_time")} className="input min-h-[60px]" />
          </Field>
        </div>
        <Field label={t("visa.required_docs")}>
          <textarea
            {...inp("required_docs")}
            className="input min-h-[110px] font-mono text-[13px]"
          />
        </Field>
        <Field label={t("visa.notes")}>
          <textarea {...inp("notes")} className="input min-h-[60px]" />
        </Field>
        <Field label={t("visa.official_link")}>
          <input {...inp("official_link")} />
        </Field>
        <div className="flex justify-end gap-2">
          <button className="btn-outline" onClick={onClose}>
            {t("common.cancel")}
          </button>
          <button className="btn-primary" onClick={save}>
            {t("common.save")}
          </button>
        </div>
      </div>
    </Modal>
  );
}
