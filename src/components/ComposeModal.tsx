import { useEffect, useMemo, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { Send, Link as LinkIcon } from "lucide-react";
import { Modal, Field, useToast } from "./ui";
import { listTemplates, sendEmail } from "../lib/api";
import type { Contact } from "../lib/types";

const LINK_FIELDS = ["mix", "listen", "onepager", "epk"] as const;
type LinkKey = (typeof LINK_FIELDS)[number];

export function renderTemplate(
  str: string,
  c?: Contact | null,
  links?: Record<string, string>
): string {
  const map: Record<string, string> = {
    name: c?.promoter || c?.name || "",
    event: c?.name || "",
    venue: c?.venue || c?.name || "",
    date: c?.date || "",
    promoter: c?.promoter || "",
    city: c?.area || "",
    ...(links || {}),
  };
  return str.replace(/\{\{\s*(\w+)\s*\}\}/g, (_, k) =>
    map[k] !== undefined ? map[k] : `{{${k}}}`
  );
}

export function ComposeModal({
  open,
  onClose,
  contact,
}: {
  open: boolean;
  onClose: () => void;
  contact?: Contact | null;
}) {
  const { t } = useTranslation();
  const toast = useToast();
  const qc = useQueryClient();
  const { data: templates } = useQuery({ queryKey: ["templates"], queryFn: listTemplates });

  const [to, setTo] = useState("");
  const [subject, setSubject] = useState("");
  const [body, setBody] = useState("");
  const [tplId, setTplId] = useState<number | "">("");
  const [links, setLinks] = useState<Record<LinkKey, string>>({
    mix: "",
    listen: "",
    onepager: "",
    epk: "",
  });
  const [sending, setSending] = useState(false);

  useEffect(() => {
    if (open) {
      setTo(contact?.email ?? "");
      setSubject("");
      setBody("");
      setTplId("");
      setLinks({ mix: "", listen: "", onepager: "", epk: "" });
    }
  }, [open, contact]);

  const applyTemplate = (id: number | "") => {
    setTplId(id);
    const tpl = templates?.find((x) => x.id === id);
    if (tpl) {
      // keep raw placeholders — they resolve live in the preview and at send
      setSubject(tpl.subject);
      setBody(tpl.body);
    }
  };

  const linkVars = useMemo(
    () => Object.fromEntries(Object.entries(links).filter(([, v]) => v.trim())),
    [links]
  );
  const renderedSubject = renderTemplate(subject, contact, linkVars);
  const renderedBody = renderTemplate(body, contact, linkVars);

  const canSend = useMemo(
    () => to.trim() && renderedSubject.trim() && renderedBody.trim() && !sending,
    [to, renderedSubject, renderedBody, sending]
  );

  const doSend = async () => {
    setSending(true);
    try {
      const res = await sendEmail({
        contact_id: contact?.id ?? null,
        to: to.trim(),
        subject: renderedSubject,
        body: renderedBody,
      });
      if (res.ok) {
        toast(t("emails.sent_ok"), "ok");
        qc.invalidateQueries({ queryKey: ["contacts"] });
        qc.invalidateQueries({ queryKey: ["emails"] });
        qc.invalidateQueries({ queryKey: ["dashboard"] });
        onClose();
      } else {
        toast(`${t("emails.send_failed")}: ${res.error ?? ""}`, "error");
      }
    } catch (e: any) {
      toast(`${t("emails.send_failed")}: ${e?.toString?.() ?? e}`, "error");
    } finally {
      setSending(false);
    }
  };

  const hasPlaceholders = /\{\{\s*\w+\s*\}\}/.test(subject + body);

  return (
    <Modal open={open} onClose={onClose} title={t("contacts.compose")} width="max-w-2xl">
      <div className="space-y-3.5">
        <Field label={t("emails.template")}>
          <select
            className="input"
            value={tplId}
            onChange={(e) => applyTemplate(e.target.value ? Number(e.target.value) : "")}
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
          <input
            className="input"
            value={to}
            onChange={(e) => setTo(e.target.value)}
            placeholder="name@venue.com"
          />
        </Field>
        <Field label={t("emails.subject")}>
          <input className="input" value={subject} onChange={(e) => setSubject(e.target.value)} />
        </Field>
        <Field label={t("emails.body")}>
          <textarea
            className="input min-h-[200px] font-mono text-[13px] leading-relaxed"
            value={body}
            onChange={(e) => setBody(e.target.value)}
          />
        </Field>

        {/* Case-by-case links */}
        <div className="panel p-3">
          <div className="flex items-center gap-1.5 text-2xs font-semibold uppercase tracking-wide text-fg-subtle">
            <LinkIcon size={12} /> {t("bulk.links")}
          </div>
          <div className="mt-2 grid grid-cols-2 gap-2">
            {LINK_FIELDS.map((k) => (
              <label key={k} className="block">
                <span className="mb-1 flex items-center justify-between text-2xs text-fg-subtle">
                  {t(`bulk.link_${k}`)}
                  <code className="text-fg-faint">{`{{${k}}}`}</code>
                </span>
                <input
                  className="input py-1.5 text-xs"
                  placeholder="https://…"
                  value={links[k]}
                  onChange={(e) => setLinks((l) => ({ ...l, [k]: e.target.value }))}
                />
              </label>
            ))}
          </div>
        </div>

        {/* Live preview */}
        {hasPlaceholders && (
          <div className="panel p-3">
            <div className="mb-1 text-2xs font-semibold uppercase tracking-wide text-fg-subtle">
              {t("bulk.preview")}
            </div>
            <div className="text-sm font-semibold">{renderedSubject}</div>
            <div className="mt-1 whitespace-pre-wrap text-xs text-fg-subtle">{renderedBody}</div>
          </div>
        )}

        <div className="flex justify-end gap-2 pt-1">
          <button className="btn-outline" onClick={onClose}>
            {t("common.cancel")}
          </button>
          <button className="btn-primary" disabled={!canSend} onClick={doSend}>
            <Send size={15} />
            {sending ? t("emails.sending") : t("emails.send")}
          </button>
        </div>
      </div>
    </Modal>
  );
}
