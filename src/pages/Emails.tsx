import { useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { Plus, Pencil, Trash2, Mail, Send, Eye, EyeOff, RefreshCw } from "lucide-react";
import clsx from "clsx";
import {
  applyOpens,
  deleteTemplate,
  getSetting,
  listEmails,
  listTemplates,
  saveTemplate,
  sendEmail,
} from "../lib/api";
import type { Template } from "../lib/types";
import { PageHeader } from "../components/Layout";
import { Modal, Field, useToast, useConfirm } from "../components/ui";

type Tab = "compose" | "templates" | "log";

export function Emails() {
  const { t } = useTranslation();
  const [tab, setTab] = useState<Tab>("compose");

  const tabs: { key: Tab; label: string }[] = [
    { key: "compose", label: t("emails.tab_compose") },
    { key: "templates", label: t("emails.tab_templates") },
    { key: "log", label: t("emails.tab_log") },
  ];

  return (
    <div>
      <PageHeader title={t("emails.title")} subtitle={t("emails.subtitle")} />
      <div className="px-8 pb-10">
        <div className="mb-5 inline-flex rounded-lg border border-border bg-surface p-1">
          {tabs.map((tb) => (
            <button
              key={tb.key}
              onClick={() => setTab(tb.key)}
              className={clsx(
                "rounded-md px-4 py-1.5 text-sm font-medium transition-colors",
                tab === tb.key
                  ? "bg-accent text-accent-fg"
                  : "text-fg-subtle hover:text-fg"
              )}
            >
              {tb.label}
            </button>
          ))}
        </div>

        {tab === "compose" && <ComposeTab />}
        {tab === "templates" && <TemplatesTab />}
        {tab === "log" && <LogTab />}
      </div>
    </div>
  );
}

function ComposeTab() {
  const { t } = useTranslation();
  const toast = useToast();
  const qc = useQueryClient();
  const { data: templates } = useQuery({
    queryKey: ["templates"],
    queryFn: listTemplates,
  });
  const [to, setTo] = useState("");
  const [subject, setSubject] = useState("");
  const [body, setBody] = useState("");
  const [sending, setSending] = useState(false);

  const doSend = async () => {
    setSending(true);
    try {
      const res = await sendEmail({ contact_id: null, to: to.trim(), subject, body });
      if (res.ok) {
        toast(t("emails.sent_ok"), "ok");
        setTo("");
        setSubject("");
        setBody("");
        qc.invalidateQueries({ queryKey: ["emails"] });
        qc.invalidateQueries({ queryKey: ["dashboard"] });
      } else {
        toast(`${t("emails.send_failed")}: ${res.error ?? ""}`, "error");
      }
    } catch (e: any) {
      toast(`${t("emails.send_failed")}: ${e?.toString?.() ?? e}`, "error");
    } finally {
      setSending(false);
    }
  };

  return (
    <div className="card max-w-2xl space-y-3.5 p-6">
      <Field label={t("emails.template")}>
        <select
          className="input"
          onChange={(e) => {
            const tpl = templates?.find((x) => x.id === Number(e.target.value));
            if (tpl) {
              setSubject(tpl.subject);
              setBody(tpl.body);
            }
          }}
        >
          <option value="">{t("emails.pick_template")}</option>
          {templates?.map((tpl) => (
            <option key={tpl.id} value={tpl.id}>
              {tpl.name}
            </option>
          ))}
        </select>
      </Field>
      <Field label={t("emails.to")}>
        <input className="input" value={to} onChange={(e) => setTo(e.target.value)} />
      </Field>
      <Field label={t("emails.subject")}>
        <input
          className="input"
          value={subject}
          onChange={(e) => setSubject(e.target.value)}
        />
      </Field>
      <Field label={t("emails.body")}>
        <textarea
          className="input min-h-[240px] font-mono text-[13px] leading-relaxed"
          value={body}
          onChange={(e) => setBody(e.target.value)}
        />
      </Field>
      <div className="flex justify-end">
        <button
          className="btn-primary"
          disabled={!to.trim() || !subject.trim() || !body.trim() || sending}
          onClick={doSend}
        >
          <Send size={15} />
          {sending ? t("emails.sending") : t("emails.send")}
        </button>
      </div>
    </div>
  );
}

function TemplatesTab() {
  const { t } = useTranslation();
  const qc = useQueryClient();
  const toast = useToast();
  const confirm = useConfirm();
  const { data: templates = [] } = useQuery({
    queryKey: ["templates"],
    queryFn: listTemplates,
  });
  const [editing, setEditing] = useState<Template | null>(null);

  const delMut = useMutation({
    mutationFn: (id: number) => deleteTemplate(id),
    onSuccess: () => qc.invalidateQueries({ queryKey: ["templates"] }),
  });

  return (
    <div className="space-y-4">
      <button
        className="btn-primary"
        onClick={() => setEditing({ name: "", subject: "", body: "" })}
      >
        <Plus size={16} />
        {t("emails.new_template")}
      </button>
      <div className="grid grid-cols-1 gap-3 md:grid-cols-2">
        {templates.map((tpl) => (
          <div key={tpl.id} className="card group p-4">
            <div className="flex items-start justify-between">
              <div className="min-w-0">
                <div className="text-sm font-semibold">{tpl.name}</div>
                <div className="mt-0.5 truncate text-xs text-fg-subtle">
                  {tpl.subject}
                </div>
              </div>
              <div className="flex gap-1 opacity-0 transition-opacity group-hover:opacity-100">
                <button
                  className="btn-ghost px-2 py-1.5"
                  onClick={() => setEditing(tpl)}
                >
                  <Pencil size={14} />
                </button>
                <button
                  className="btn-ghost px-2 py-1.5 text-rose-500"
                  onClick={() => {
                    if (confirm(t("common.confirm_delete"))) delMut.mutate(tpl.id!);
                  }}
                >
                  <Trash2 size={14} />
                </button>
              </div>
            </div>
            <p className="mt-2 line-clamp-4 whitespace-pre-wrap text-xs text-fg-subtle">
              {tpl.body}
            </p>
          </div>
        ))}
      </div>

      {editing && (
        <TemplateModal
          template={editing}
          onClose={() => setEditing(null)}
          onSaved={() => {
            setEditing(null);
            qc.invalidateQueries({ queryKey: ["templates"] });
            toast(t("common.save"), "ok");
          }}
        />
      )}
    </div>
  );
}

function TemplateModal({
  template,
  onClose,
  onSaved,
}: {
  template: Template;
  onClose: () => void;
  onSaved: () => void;
}) {
  const { t } = useTranslation();
  const [form, setForm] = useState<Template>({ ...template });
  const save = async () => {
    if (!form.name.trim()) return;
    await saveTemplate(form);
    onSaved();
  };
  return (
    <Modal open onClose={onClose} title={t("emails.new_template")} width="max-w-2xl">
      <div className="space-y-3.5">
        <Field label={t("emails.template_name")}>
          <input
            className="input"
            value={form.name}
            onChange={(e) => setForm({ ...form, name: e.target.value })}
          />
        </Field>
        <Field label={t("emails.subject")}>
          <input
            className="input"
            value={form.subject}
            onChange={(e) => setForm({ ...form, subject: e.target.value })}
          />
        </Field>
        <Field label={t("emails.body")}>
          <textarea
            className="input min-h-[240px] font-mono text-[13px] leading-relaxed"
            value={form.body}
            onChange={(e) => setForm({ ...form, body: e.target.value })}
          />
        </Field>
        <p className="text-xs text-fg-subtle">{t("emails.variables")}</p>
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

function LogTab() {
  const { t } = useTranslation();
  const toast = useToast();
  const qc = useQueryClient();
  const [syncing, setSyncing] = useState(false);
  const { data: emails = [] } = useQuery({
    queryKey: ["emails"],
    queryFn: () => listEmails(),
  });

  const syncOpens = async () => {
    const base = (await getSetting("tracking_base_url"))?.trim().replace(/\/$/, "");
    if (!base) {
      toast(t("emails.no_tracking"), "error");
      return;
    }
    setSyncing(true);
    try {
      const res = await fetch(`${base}/opens.json`, { cache: "no-store" });
      const data = await res.json();
      // Accept either an array of {token,opened_at,count} or a { token: opened_at } map.
      const opens = Array.isArray(data)
        ? data
        : Object.entries(data).map(([token, v]: any) =>
            typeof v === "string"
              ? { token, opened_at: v }
              : { token, opened_at: v?.opened_at, count: v?.count }
          );
      const n = await applyOpens(opens);
      qc.invalidateQueries({ queryKey: ["emails"] });
      qc.invalidateQueries({ queryKey: ["contacts"] });
      toast(t("emails.sync_done", { count: n }), "ok");
    } catch (e: any) {
      toast(`${t("emails.send_failed")}: ${e?.toString?.() ?? e}`, "error");
    } finally {
      setSyncing(false);
    }
  };

  return (
    <div className="space-y-3">
      <div className="flex justify-end">
        <button className="btn-outline" onClick={syncOpens} disabled={syncing}>
          <RefreshCw size={14} className={syncing ? "animate-spin" : ""} />
          {syncing ? t("emails.syncing") : t("emails.sync_opens")}
        </button>
      </div>
      {emails.length === 0 ? (
        <div className="card flex flex-col items-center gap-2 py-16 text-sm text-fg-subtle">
          <Mail size={28} className="opacity-40" />
          {t("emails.log_empty")}
        </div>
      ) : (
        <div className="card overflow-hidden">
          <table className="w-full text-sm">
            <thead>
              <tr className="border-b border-border text-left text-xs text-fg-subtle">
                <th className="px-4 py-2.5 font-medium">{t("emails.to")}</th>
                <th className="px-4 py-2.5 font-medium">{t("emails.subject")}</th>
                <th className="px-4 py-2.5 font-medium">{t("contacts.status")}</th>
                <th className="px-4 py-2.5 font-medium">{t("emails.open_col")}</th>
                <th className="px-4 py-2.5 font-medium">{t("contacts.date")}</th>
              </tr>
            </thead>
            <tbody>
              {emails.map((e) => (
                <tr key={e.id} className="border-b border-border/60">
                  <td className="px-4 py-2.5">{e.to_addr}</td>
                  <td className="max-w-[280px] truncate px-4 py-2.5 text-fg-subtle">
                    {e.subject}
                  </td>
                  <td className="px-4 py-2.5">
                    <span
                      className={clsx(
                        "rounded-full px-2 py-0.5 text-xs font-medium",
                        e.status === "sent"
                          ? "bg-emerald-500/10 text-emerald-500"
                          : "bg-rose-500/10 text-rose-500"
                      )}
                    >
                      {e.status === "sent"
                        ? t("emails.status_sent")
                        : t("emails.status_failed")}
                    </span>
                  </td>
                  <td className="px-4 py-2.5">
                    {e.opened_at ? (
                      <span
                        className="inline-flex items-center gap-1 text-xs font-medium text-emerald-500"
                        title={`${e.opened_at}${
                          e.open_count > 1 ? ` · ${e.open_count}×` : ""
                        }`}
                      >
                        <Eye size={13} />
                        {t("emails.opened")}
                        {e.open_count > 1 && ` ·${e.open_count}×`}
                      </span>
                    ) : (
                      <span className="inline-flex items-center gap-1 text-xs text-fg-subtle">
                        <EyeOff size={13} />
                        {t("emails.not_opened")}
                      </span>
                    )}
                  </td>
                  <td className="whitespace-nowrap px-4 py-2.5 text-xs text-fg-subtle">
                    {e.sent_at}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </div>
  );
}
