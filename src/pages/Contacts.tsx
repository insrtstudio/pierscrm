import { useEffect, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import {
  Plus,
  Search,
  Mail,
  ExternalLink,
  Pencil,
  Trash2,
} from "lucide-react";
import { openUrl } from "@tauri-apps/plugin-opener";
import {
  createContact,
  deleteContact,
  deleteContacts,
  listContacts,
  updateContact,
  updateContactStatus,
} from "../lib/api";
import type { Contact, Status } from "../lib/types";
import { STATUSES } from "../lib/types";
import { CATEGORIES } from "../lib/constants";
import { PageHeader } from "../components/Layout";
import { StatusBadge, StatusSelect, PriorityBadge } from "../components/StatusBadge";
import { Modal, Field, useToast, useConfirm } from "../components/ui";
import { ComposeModal } from "../components/ComposeModal";

export function Contacts() {
  const { t } = useTranslation();
  const qc = useQueryClient();
  const toast = useToast();
  const confirm = useConfirm();

  const [category, setCategory] = useState("all");
  const [status, setStatus] = useState("all");
  const [search, setSearch] = useState("");
  const [selected, setSelected] = useState<Set<number>>(new Set());
  const [editing, setEditing] = useState<Contact | null>(null);
  const [composing, setComposing] = useState<Contact | null>(null);

  const { data: contacts = [] } = useQuery({
    queryKey: ["contacts", category, status, search],
    queryFn: () => listContacts({ category, status, search }),
  });

  useEffect(() => {
    const h = (e: Event) => {
      if ((e as CustomEvent).detail === "new:contact")
        setEditing({ category: "venue", name: "", status: "to_contact" } as Contact);
    };
    window.addEventListener("app:new", h);
    return () => window.removeEventListener("app:new", h);
  }, []);

  const statusMut = useMutation({
    mutationFn: ({ id, status }: { id: number; status: Status }) =>
      updateContactStatus(id, status),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ["contacts"] });
      qc.invalidateQueries({ queryKey: ["dashboard"] });
    },
  });

  const delMut = useMutation({
    mutationFn: (id: number) => deleteContact(id),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ["contacts"] });
      qc.invalidateQueries({ queryKey: ["dashboard"] });
      toast(t("common.delete"), "ok");
    },
  });

  const bulkDelMut = useMutation({
    mutationFn: (ids: number[]) => deleteContacts(ids),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ["contacts"] });
      qc.invalidateQueries({ queryKey: ["dashboard"] });
      setSelected(new Set());
    },
  });

  const allChecked =
    contacts.length > 0 && contacts.every((c) => selected.has(c.id!));

  const toggleAll = () => {
    if (allChecked) setSelected(new Set());
    else setSelected(new Set(contacts.map((c) => c.id!)));
  };
  const toggleOne = (id: number) => {
    setSelected((s) => {
      const n = new Set(s);
      n.has(id) ? n.delete(id) : n.add(id);
      return n;
    });
  };

  return (
    <div>
      <PageHeader
        title={t("contacts.title")}
        subtitle={t("contacts.subtitle")}
        actions={
          <button
            className="btn-primary"
            onClick={() =>
              setEditing({
                category: category === "all" ? "venue" : category,
                name: "",
                status: "to_contact",
              } as Contact)
            }
          >
            <Plus size={16} />
            {t("contacts.new")}
          </button>
        }
      />

      <div className="px-8 pb-10">
        {/* Filters */}
        <div className="mb-4 flex flex-wrap items-center gap-2">
          <div className="relative flex-1 min-w-[200px]">
            <Search
              size={15}
              className="pointer-events-none absolute left-3 top-1/2 -translate-y-1/2 text-fg-subtle"
            />
            <input
              className="input pl-9"
              placeholder={t("common.search")}
              value={search}
              onChange={(e) => setSearch(e.target.value)}
            />
          </div>
          <select
            className="input w-auto"
            value={category}
            onChange={(e) => setCategory(e.target.value)}
          >
            <option value="all">{t("category.all")}</option>
            {CATEGORIES.map((c) => (
              <option key={c} value={c}>
                {t(`category.${c}`)}
              </option>
            ))}
          </select>
          <select
            className="input w-auto"
            value={status}
            onChange={(e) => setStatus(e.target.value)}
          >
            <option value="all">{t("common.all")}</option>
            {STATUSES.map((s) => (
              <option key={s} value={s}>
                {t(`status.${s}`)}
              </option>
            ))}
          </select>
        </div>

        {/* Bulk bar */}
        {selected.size > 0 && (
          <div className="mb-3 flex items-center gap-3 rounded-lg border border-border bg-surface px-4 py-2 text-sm">
            <span className="text-fg-subtle">
              {t("contacts.selected", { count: selected.size })}
            </span>
            <button
              className="btn-danger ml-auto py-1.5"
              onClick={() => {
                if (confirm(t("common.confirm_delete")))
                  bulkDelMut.mutate([...selected]);
              }}
            >
              <Trash2 size={14} />
              {t("contacts.delete_selected")}
            </button>
          </div>
        )}

        {/* Table */}
        <div className="card overflow-hidden">
          <div className="overflow-x-auto">
            <table className="w-full text-sm">
              <thead>
                <tr className="border-b border-border text-left text-xs text-fg-subtle">
                  <th className="w-10 px-3 py-2.5">
                    <input
                      type="checkbox"
                      checked={allChecked}
                      onChange={toggleAll}
                      className="accent-accent"
                    />
                  </th>
                  <th className="px-3 py-2.5 font-medium">{t("contacts.name")}</th>
                  <th className="px-3 py-2.5 font-medium">{t("contacts.area")}</th>
                  <th className="px-3 py-2.5 font-medium">{t("contacts.date")}</th>
                  <th className="px-3 py-2.5 font-medium">{t("contacts.email")}</th>
                  <th className="px-3 py-2.5 font-medium">{t("contacts.status")}</th>
                  <th className="px-3 py-2.5 text-right font-medium">
                    {t("common.actions")}
                  </th>
                </tr>
              </thead>
              <tbody>
                {contacts.length === 0 && (
                  <tr>
                    <td
                      colSpan={7}
                      className="px-3 py-16 text-center text-sm text-fg-subtle"
                    >
                      {t("common.empty")}
                    </td>
                  </tr>
                )}
                {contacts.map((c) => (
                  <tr
                    key={c.id}
                    className="group border-b border-border/60 transition-colors hover:bg-muted/50"
                  >
                    <td className="px-3 py-2.5">
                      <input
                        type="checkbox"
                        checked={selected.has(c.id!)}
                        onChange={() => toggleOne(c.id!)}
                        className="accent-accent"
                      />
                    </td>
                    <td className="px-3 py-2.5">
                      <button
                        className="flex items-center gap-2 text-left"
                        onClick={() => setEditing(c)}
                      >
                        <PriorityBadge priority={c.priority} />
                        <span className="font-medium">{c.name}</span>
                      </button>
                      {c.promoter && (
                        <div className="text-xs text-fg-subtle">{c.promoter}</div>
                      )}
                    </td>
                    <td className="px-3 py-2.5 text-fg-subtle">
                      {c.area || c.venue || "·"}
                    </td>
                    <td className="whitespace-nowrap px-3 py-2.5 text-fg-subtle">
                      {c.date || "·"}
                    </td>
                    <td className="px-3 py-2.5">
                      {c.email ? (
                        <span className="text-xs">{c.email}</span>
                      ) : (
                        <span className="text-xs text-fg-subtle">·</span>
                      )}
                    </td>
                    <td className="px-3 py-2.5">
                      <StatusSelectInline
                        value={c.status}
                        onChange={(s) =>
                          statusMut.mutate({ id: c.id!, status: s })
                        }
                      />
                    </td>
                    <td className="px-3 py-2.5">
                      <div className="flex items-center justify-end gap-1 opacity-0 transition-opacity group-hover:opacity-100 group-focus-within:opacity-100">
                        {c.email && (
                          <button
                            title={t("contacts.compose")}
                            className="btn-ghost px-2 py-1.5"
                            onClick={() => setComposing(c)}
                          >
                            <Mail size={15} />
                          </button>
                        )}
                        {c.website && (
                          <button
                            title={t("contacts.open_site")}
                            className="btn-ghost px-2 py-1.5"
                            onClick={() => {
                              const url = c.website!.startsWith("http")
                                ? c.website!
                                : `https://${c.website}`;
                              openUrl(url).catch(() => {});
                            }}
                          >
                            <ExternalLink size={15} />
                          </button>
                        )}
                        <button
                          title={t("common.edit")}
                          className="btn-ghost px-2 py-1.5"
                          onClick={() => setEditing(c)}
                        >
                          <Pencil size={15} />
                        </button>
                        <button
                          title={t("common.delete")}
                          className="btn-ghost px-2 py-1.5 text-rose-500"
                          onClick={() => {
                            if (confirm(t("common.confirm_delete")))
                              delMut.mutate(c.id!);
                          }}
                        >
                          <Trash2 size={15} />
                        </button>
                      </div>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </div>
        <div className="mt-3 text-xs text-fg-subtle">
          {t("contacts.count", { count: contacts.length })}
        </div>
      </div>

      {editing && (
        <ContactModal
          contact={editing}
          onClose={() => setEditing(null)}
          onSaved={() => {
            setEditing(null);
            qc.invalidateQueries({ queryKey: ["contacts"] });
            qc.invalidateQueries({ queryKey: ["dashboard"] });
          }}
        />
      )}
      <ComposeModal
        open={!!composing}
        contact={composing}
        onClose={() => setComposing(null)}
      />
    </div>
  );
}

function StatusSelectInline({
  value,
  onChange,
}: {
  value: string;
  onChange: (s: Status) => void;
}) {
  const [editing, setEditing] = useState(false);
  if (editing)
    return (
      <StatusSelect
        value={value}
        className="w-40 py-1 text-xs"
        onChange={(s) => {
          onChange(s);
          setEditing(false);
        }}
      />
    );
  return (
    <button onClick={() => setEditing(true)}>
      <StatusBadge status={value} />
    </button>
  );
}

// ---------------- Contact editor ----------------

function ContactModal({
  contact,
  onClose,
  onSaved,
}: {
  contact: Contact;
  onClose: () => void;
  onSaved: () => void;
}) {
  const { t } = useTranslation();
  const toast = useToast();
  const [form, setForm] = useState<Contact>({ ...contact });
  const isNew = !contact.id;

  const set = (k: keyof Contact, v: any) =>
    setForm((f) => ({ ...f, [k]: v }));

  const save = async () => {
    if (!form.name.trim()) {
      toast(t("contacts.name") + " " + t("common.required"), "error");
      return;
    }
    try {
      if (isNew) await createContact(form);
      else await updateContact(form);
      toast(t("common.save"), "ok");
      onSaved();
    } catch (e: any) {
      toast(e?.toString?.() ?? "error", "error");
    }
  };

  const inp = (k: keyof Contact) => ({
    className: "input",
    value: (form[k] as string) ?? "",
    onChange: (e: any) => set(k, e.target.value),
  });

  return (
    <Modal
      open
      onClose={onClose}
      width="max-w-3xl"
      title={isNew ? t("contacts.new") : form.name || t("common.edit")}
    >
      <div className="grid grid-cols-2 gap-x-4 gap-y-3">
        <Field label={t("contacts.name")} className="col-span-2">
          <input {...inp("name")} />
        </Field>
        <Field label={t("contacts.category")}>
          <select
            className="input"
            value={form.category}
            onChange={(e) => set("category", e.target.value)}
          >
            {CATEGORIES.map((c) => (
              <option key={c} value={c}>
                {t(`category.${c}`)}
              </option>
            ))}
          </select>
        </Field>
        <Field label={t("contacts.priority")}>
          <input {...inp("priority")} placeholder="P1 / A / …" />
        </Field>
        <Field label={t("contacts.promoter")}>
          <input {...inp("promoter")} />
        </Field>
        <Field label={t("contacts.venue")}>
          <input {...inp("venue")} />
        </Field>
        <Field label={t("contacts.type")}>
          <input {...inp("type")} />
        </Field>
        <Field label={t("contacts.area")}>
          <input {...inp("area")} />
        </Field>
        <Field label={t("contacts.date")}>
          <input {...inp("date")} />
        </Field>
        <Field label={t("contacts.time")}>
          <input {...inp("time")} />
        </Field>
        <Field label={t("contacts.format")}>
          <input {...inp("format")} />
        </Field>
        <Field label={t("contacts.scale")}>
          <input {...inp("scale")} />
        </Field>
        <Field label={t("contacts.email")}>
          <input {...inp("email")} />
        </Field>
        <Field label={t("contacts.email_status")}>
          <input {...inp("email_status")} />
        </Field>
        <Field label={t("contacts.status")}>
          <StatusSelect
            value={form.status}
            onChange={(s) => set("status", s)}
          />
        </Field>
        <Field label={t("contacts.website")}>
          <input {...inp("website")} />
        </Field>
        <Field label={t("contacts.first_contact")}>
          <input {...inp("first_contact")} />
        </Field>
        <Field label={t("contacts.follow_up")}>
          <input {...inp("follow_up")} />
        </Field>
        <Field label={t("contacts.contact_channel")} className="col-span-2">
          <input {...inp("contact_channel")} />
        </Field>
        <Field label={t("contacts.reason")} className="col-span-2">
          <textarea {...inp("reason")} className="input min-h-[70px]" />
        </Field>
        <Field label={t("contacts.notes")} className="col-span-2">
          <textarea {...inp("notes")} className="input min-h-[70px]" />
        </Field>
        <Field label={t("contacts.tags")} className="col-span-2">
          <input {...inp("tags")} placeholder="fit-a, noord, boat…" />
        </Field>
      </div>
      <div className="mt-5 flex justify-end gap-2">
        <button className="btn-outline" onClick={onClose}>
          {t("common.cancel")}
        </button>
        <button className="btn-primary" onClick={save}>
          {t("common.save")}
        </button>
      </div>
    </Modal>
  );
}
