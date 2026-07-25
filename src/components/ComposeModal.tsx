import { useEffect, useMemo, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { Send } from "lucide-react";
import { Modal, Field, useToast } from "./ui";
import { listTemplates, sendEmail } from "../lib/api";
import type { Contact } from "../lib/types";

export function renderTemplate(str: string, c?: Contact | null): string {
  if (!c) return str;
  const map: Record<string, string> = {
    name: c.promoter || c.name || "",
    event: c.name || "",
    venue: c.venue || c.name || "",
    date: c.date || "",
    promoter: c.promoter || "",
  };
  return str.replace(/\{\{\s*(\w+)\s*\}\}/g, (_, k) => map[k] ?? `{{${k}}}`);
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
  const { data: templates } = useQuery({
    queryKey: ["templates"],
    queryFn: listTemplates,
  });

  const [to, setTo] = useState("");
  const [subject, setSubject] = useState("");
  const [body, setBody] = useState("");
  const [tplId, setTplId] = useState<number | "">("");
  const [sending, setSending] = useState(false);

  useEffect(() => {
    if (open) {
      setTo(contact?.email ?? "");
      setSubject("");
      setBody("");
      setTplId("");
    }
  }, [open, contact]);

  const applyTemplate = (id: number | "") => {
    setTplId(id);
    const tpl = templates?.find((x) => x.id === id);
    if (tpl) {
      setSubject(renderTemplate(tpl.subject, contact));
      setBody(renderTemplate(tpl.body, contact));
    }
  };

  const canSend = useMemo(
    () => to.trim() && subject.trim() && body.trim() && !sending,
    [to, subject, body, sending]
  );

  const doSend = async () => {
    setSending(true);
    try {
      const res = await sendEmail({
        contact_id: contact?.id ?? null,
        to: to.trim(),
        subject,
        body,
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

  return (
    <Modal
      open={open}
      onClose={onClose}
      title={t("contacts.compose")}
      width="max-w-2xl"
    >
      <div className="space-y-3.5">
        <Field label={t("emails.template")}>
          <select
            className="input"
            value={tplId}
            onChange={(e) =>
              applyTemplate(e.target.value ? Number(e.target.value) : "")
            }
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
          <input
            className="input"
            value={subject}
            onChange={(e) => setSubject(e.target.value)}
          />
        </Field>
        <Field label={t("emails.body")}>
          <textarea
            className="input min-h-[220px] font-mono text-[13px] leading-relaxed"
            value={body}
            onChange={(e) => setBody(e.target.value)}
          />
        </Field>
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
