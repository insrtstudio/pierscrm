// Offers (booking deals) and what follows them: documents (contract, rider,
// invoices), payments, advancing checklist. Every email here is a draft
// opened in ComposeModal: the user reviews and sends it, nothing goes out by
// itself. Backend: src-tauri/src/commands/offers.
import { useEffect, useState } from "react";
import { useQuery, useQueryClient, type QueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import type { TFunction } from "i18next";
import { Link } from "react-router-dom";
import clsx from "clsx";
import {
  AlertTriangle,
  Check,
  CircleSlash,
  FileText,
  Handshake,
  Mail,
  Plus,
  Send,
  Trash2,
  X,
} from "lucide-react";
import { Modal, Field, useToast, useConfirm, errorMessage } from "./ui";
import { ComposeModal } from "./ComposeModal";
import { getContact, getSetting, setSetting } from "../lib/api";
import type { Contact } from "../lib/types";
import { formatDate, formatDateTime } from "../lib/format";
import {
  DOC_KINDS,
  FEE_TYPES,
  OFFER_QUERY_KEYS,
  TRAVEL_BY,
  advancingEmailDraft,
  deleteOffer,
  generateOfferDocument,
  getEventAdvancing,
  listOfferDocuments,
  listOfferPayments,
  listOffers,
  money,
  newOfferForContact,
  offerEmailDraft,
  openOfferDocument,
  saveOffer,
  setAdvancingItem,
  setOfferPayment,
  todayIso,
  type AdvItem,
  type DocKind,
  type DraftPurpose,
  type EmailDraft,
  type GigAdvancing,
  type Offer,
  type Payment,
} from "../lib/offers";

export function invalidateOffers(qc: QueryClient) {
  for (const key of OFFER_QUERY_KEYS) qc.invalidateQueries({ queryKey: [key] });
}

/// Backend errors are English sentences; the known ones get the UI language.
function offerError(t: TFunction, e: unknown): string {
  const m = errorMessage(e);
  if (m.startsWith("An accepted offer needs a date")) return t("offers.err_needs_date");
  if (m.startsWith("Invoices are issued for accepted offers only")) return t("offers.err_invoice_accepted");
  if (m.startsWith("This offer has no deposit")) return t("offers.err_no_deposit");
  return m;
}

export const OFFER_STATUS_STYLE: Record<string, string> = {
  draft: "text-fg-subtle bg-muted",
  sent: "text-accent-2 bg-accent-soft",
  accepted: "text-accent-fg bg-accent",
  declined: "text-fg-faint bg-muted",
  expired: "text-warn bg-warn/15",
};

export function OfferStatusBadge({ status }: { status: string }) {
  const { t } = useTranslation();
  return (
    <span className={clsx("badge", OFFER_STATUS_STYLE[status] ?? "text-fg-subtle bg-muted")}>
      {t(`offers.st_${status}`, status)}
    </span>
  );
}

// ---------------- Email drafts ----------------

/// A backend draft opened in the usual compose window, attachments included.
export function DraftCompose({
  draft,
  onClose,
  onSent,
}: {
  draft: EmailDraft;
  onClose: () => void;
  onSent?: () => void;
}) {
  const { data: contact } = useQuery({
    queryKey: ["contact", draft.contact_id],
    queryFn: () => getContact(draft.contact_id!),
    enabled: draft.contact_id != null,
  });
  if (draft.contact_id != null && contact === undefined) return null;
  return (
    <ComposeModal
      open
      onClose={onClose}
      contact={(contact as Contact | null) ?? null}
      initialTo={draft.to}
      initialSubject={draft.subject}
      initialBody={draft.body}
      initialAttachments={draft.attachments}
      onSent={onSent}
    />
  );
}

// ---------------- Offer editor ----------------

type Num = number | null | undefined;

export function OfferEditor({ offer, onClose }: { offer: Offer; onClose: () => void }) {
  const { t } = useTranslation();
  const toast = useToast();
  const confirm = useConfirm();
  const qc = useQueryClient();
  const [form, setForm] = useState<Offer>({ ...offer });
  const [busy, setBusy] = useState(false);
  const [draft, setDraft] = useState<{ d: EmailDraft; purpose: DraftPurpose } | null>(null);
  useEffect(() => setForm({ ...offer }), [offer]);

  const set = <K extends keyof Offer>(k: K, v: Offer[K]) => setForm((f) => ({ ...f, [k]: v }));
  const str = (k: keyof Offer) => ({
    className: "input",
    value: (form[k] as string | null | undefined) ?? "",
    onChange: (e: { target: { value: string } }) => set(k, (e.target.value || null) as never),
  });
  const num = (k: keyof Offer) => ({
    className: "input",
    type: "number",
    value: (form[k] as Num) ?? "",
    onChange: (e: { target: { value: string } }) =>
      set(k, (e.target.value === "" ? null : Number(e.target.value)) as never),
  });

  const persist = async (status?: string) => {
    setBusy(true);
    try {
      const next = { ...form, status: status ?? form.status };
      const r = await saveOffer(next);
      setForm({ ...next, id: r.id, event_id: r.event_id });
      invalidateOffers(qc);
      toast(status ? t(`offers.toast_${status}`, t("offers.saved")) : t("offers.saved"), "ok");
      return r.id;
    } catch (e) {
      toast(offerError(t, e), "error");
      return null;
    } finally {
      setBusy(false);
    }
  };

  const openDraft = async (purpose: DraftPurpose) => {
    const id = form.id ?? (await persist());
    if (id == null) return;
    try {
      const d = await offerEmailDraft(id, purpose);
      qc.invalidateQueries({ queryKey: ["offer_docs", id] });
      setDraft({ d, purpose });
    } catch (e) {
      toast(offerError(t, e), "error");
    }
  };

  const remove = async () => {
    if (!form.id || !confirm(t("offers.confirm_delete"))) return;
    await deleteOffer(form.id);
    invalidateOffers(qc);
    onClose();
  };

  const st = form.status;
  const hasDeposit = (form.deposit_pct ?? 0) > 0;

  return (
    <Modal
      open
      onClose={onClose}
      width="max-w-3xl"
      title={
        <span className="flex items-center gap-2">
          <Handshake size={15} /> {form.id ? t("offers.edit") : t("offers.new")}
          <OfferStatusBadge status={st} />
        </span>
      }
    >
      <div className="space-y-4">
        {form.contact_name && (
          <p className="text-xs text-fg-subtle">
            {form.contact_name}
            {form.artist_name ? ` · ${form.artist_name}` : ""}
            {form.sent_at ? ` · ${t("offers.sent_on", { date: formatDate(form.sent_at) })}` : ""}
          </p>
        )}
        <div className="grid grid-cols-3 gap-3">
          <Field label={t("offers.date")}>
            <input {...str("date")} type="date" />
          </Field>
          <Field label={t("offers.venue")}>
            <input {...str("venue")} />
          </Field>
          <Field label={t("offers.city")}>
            <input {...str("city")} />
          </Field>
          <Field label={t("offers.set_time")}>
            <input {...str("set_time")} type="time" />
          </Field>
          <Field label={t("offers.set_length")}>
            <input {...num("set_length")} min={0} step={15} />
          </Field>
          <Field label={t("offers.answer_by")}>
            <input {...str("answer_by")} type="date" />
          </Field>
        </div>

        <div className="grid grid-cols-4 gap-3">
          <Field label={t("offers.fee")}>
            <input {...num("fee_amount")} min={0} step={50} />
          </Field>
          <Field label={t("offers.currency")}>
            <select className="input" value={form.currency} onChange={(e) => set("currency", e.target.value)}>
              {["EUR", "GBP", "USD", "CHF"].map((c) => (
                <option key={c}>{c}</option>
              ))}
            </select>
          </Field>
          <Field label={t("offers.fee_type")}>
            <select className="input" value={form.fee_type} onChange={(e) => set("fee_type", e.target.value)}>
              {FEE_TYPES.map((f) => (
                <option key={f} value={f}>
                  {t(`offers.fee_${f}`)}
                </option>
              ))}
            </select>
          </Field>
          <Field label={t("offers.deposit_pct")}>
            <input
              className="input"
              type="number"
              min={0}
              max={100}
              step={5}
              value={form.deposit_pct}
              onChange={(e) => set("deposit_pct", Number(e.target.value || 0))}
            />
          </Field>
          {form.fee_type !== "flat" && (
            <Field label={t("offers.deal_terms")} className="col-span-4">
              <input {...str("deal_terms")} placeholder={t("offers.deal_terms_ph")} />
            </Field>
          )}
        </div>

        <div className="grid grid-cols-3 gap-3">
          <Field label={t("offers.travel_by")}>
            <select
              className="input"
              value={form.travel_by ?? ""}
              onChange={(e) => set("travel_by", e.target.value || null)}
            >
              <option value="">·</option>
              {TRAVEL_BY.map((x) => (
                <option key={x} value={x}>
                  {t(`offers.travel_${x}`)}
                </option>
              ))}
            </select>
          </Field>
          <Field label={t("offers.travel_from")}>
            <input {...str("travel_from")} />
          </Field>
          <Field label={t("offers.travel_to")}>
            <input {...str("travel_to")} />
          </Field>
          <Field label={t("offers.hotel")}>
            <input {...str("hotel")} />
          </Field>
          <Field label={t("offers.meals")}>
            <input {...str("meals")} />
          </Field>
          <Field label={t("offers.guest_list")}>
            <input {...num("guest_list")} min={0} />
          </Field>
        </div>

        <Field label={t("offers.tech")}>
          <textarea {...str("tech_requirements")} className="input min-h-[70px]" />
        </Field>
        <div className="grid grid-cols-[1fr_8rem] gap-3">
          <Field label={t("offers.notes")}>
            <textarea {...str("notes")} className="input min-h-[50px]" />
          </Field>
          <Field label={t("offers.language")}>
            <select className="input" value={form.language} onChange={(e) => set("language", e.target.value)}>
              <option value="fr">Français</option>
              <option value="en">English</option>
            </select>
          </Field>
        </div>

        {st === "accepted" && (
          <div className="grid grid-cols-2 gap-3 border border-border p-3 text-xs">
            {hasDeposit && (
              <PaymentLine
                offerId={form.id!}
                part="deposit"
                due={form.deposit_due}
                paidAt={form.deposit_paid_at}
                onChange={(v) => set("deposit_paid_at", v)}
              />
            )}
            <PaymentLine
              offerId={form.id!}
              part="balance"
              due={form.balance_due}
              paidAt={form.balance_paid_at}
              onChange={(v) => set("balance_paid_at", v)}
            />
          </div>
        )}

        {form.id != null && <OfferDocuments offerId={form.id} accepted={st === "accepted"} hasDeposit={hasDeposit} />}

        {/* Emails: drafts only, reviewed and sent from the compose window. */}
        <div className="flex flex-wrap gap-2 border-t border-border pt-3">
          <button className="btn-outline py-1 text-xs" disabled={busy} onClick={() => openDraft("offer")}>
            <Mail size={13} /> {t("offers.mail_offer")}
          </button>
          {st === "accepted" && (
            <>
              <button className="btn-outline py-1 text-xs" disabled={busy} onClick={() => openDraft("contract")}>
                <Mail size={13} /> {t("offers.mail_contract")}
              </button>
              {hasDeposit && (
                <button className="btn-outline py-1 text-xs" disabled={busy} onClick={() => openDraft("invoice_deposit")}>
                  <Mail size={13} /> {t("offers.mail_invoice_deposit")}
                </button>
              )}
              <button className="btn-outline py-1 text-xs" disabled={busy} onClick={() => openDraft("invoice_balance")}>
                <Mail size={13} /> {t("offers.mail_invoice_balance")}
              </button>
            </>
          )}
        </div>

        <div className="flex items-center justify-between pt-1">
          <div>
            {form.id && (
              <button className="btn-danger" onClick={remove}>
                <Trash2 size={15} /> {t("common.delete")}
              </button>
            )}
          </div>
          <div className="flex flex-wrap justify-end gap-2">
            {(st === "draft" || st === "expired") && (
              <button className="btn-outline" disabled={busy} onClick={() => persist("sent")}>
                <Send size={14} /> {t("offers.mark_sent")}
              </button>
            )}
            {(st === "sent" || st === "draft" || st === "expired") && (
              <button className="btn-outline" disabled={busy} onClick={() => persist("accepted")}>
                <Check size={14} /> {t("offers.mark_accepted")}
              </button>
            )}
            {st !== "declined" && st !== "draft" && (
              <button className="btn-outline" disabled={busy} onClick={() => persist("declined")}>
                <X size={14} /> {t("offers.mark_declined")}
              </button>
            )}
            <button className="btn-primary" disabled={busy} onClick={() => persist()}>
              {t("common.save")}
            </button>
          </div>
        </div>
      </div>
      {draft && (
        <DraftCompose
          draft={draft.d}
          onClose={() => setDraft(null)}
          onSent={() => {
            // Sending the offer itself is what makes it "sent".
            if (draft.purpose === "offer" && (form.status === "draft" || form.status === "expired")) {
              void persist("sent");
            }
          }}
        />
      )}
    </Modal>
  );
}

function PaymentLine({
  offerId,
  part,
  due,
  paidAt,
  onChange,
}: {
  offerId: number;
  part: "deposit" | "balance";
  due?: string | null;
  paidAt?: string | null;
  onChange: (v: string | null) => void;
}) {
  const { t } = useTranslation();
  const qc = useQueryClient();
  const toast = useToast();
  const toggle = async () => {
    const v = paidAt ? null : todayIso();
    try {
      await setOfferPayment(offerId, part, v);
      onChange(v);
      invalidateOffers(qc);
    } catch (e) {
      toast(offerError(t, e), "error");
    }
  };
  const overdue = !paidAt && due != null && due < todayIso();
  return (
    <div className="flex items-center justify-between gap-2">
      <div>
        <div className="font-medium">{t(`offers.part_${part}`)}</div>
        <div className={clsx("text-2xs", overdue ? "text-danger" : "text-fg-subtle")}>
          {paidAt
            ? t("offers.paid_on", { date: formatDate(paidAt) })
            : due
              ? t("offers.due_on", { date: formatDate(due) })
              : "·"}
        </div>
      </div>
      <button className={clsx("py-1 text-xs", paidAt ? "btn-outline" : "btn-primary")} onClick={toggle}>
        {paidAt ? t("offers.mark_unpaid") : t("offers.mark_paid")}
      </button>
    </div>
  );
}

function OfferDocuments({ offerId, accepted, hasDeposit }: { offerId: number; accepted: boolean; hasDeposit: boolean }) {
  const { t } = useTranslation();
  const toast = useToast();
  const qc = useQueryClient();
  const { data: docs = [] } = useQuery({
    queryKey: ["offer_docs", offerId],
    queryFn: () => listOfferDocuments(offerId),
  });
  const kinds = DOC_KINDS.filter((k) =>
    k === "rider" ? true : k === "invoice_deposit" ? accepted && hasDeposit : accepted
  );
  const gen = async (kind: DocKind) => {
    try {
      const d = await generateOfferDocument(offerId, kind);
      qc.invalidateQueries({ queryKey: ["offer_docs", offerId] });
      await openOfferDocument(d.path);
    } catch (e) {
      toast(offerError(t, e), "error");
    }
  };
  return (
    <div className="border border-border p-3">
      <div className="mb-2 flex items-center gap-1.5 text-2xs font-semibold uppercase tracking-wide text-fg-subtle">
        <FileText size={12} /> {t("offers.documents")}
      </div>
      <div className="flex flex-wrap gap-2">
        {kinds.map((k) => (
          <button key={k} className="btn-outline py-1 text-xs" onClick={() => gen(k)}>
            <Plus size={12} /> {t(`offers.doc_${k}`)}
          </button>
        ))}
      </div>
      {!accepted && <p className="mt-2 text-2xs text-fg-subtle">{t("offers.docs_after_accept")}</p>}
      {docs.length > 0 && (
        <ul className="mt-2 space-y-1 text-xs">
          {docs.map((d) => (
            <li key={d.path}>
              <button className="text-accent hover:underline" onClick={() => openOfferDocument(d.path)}>
                {d.file_name}
              </button>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}

// ---------------- Contact: offers panel ----------------

export function ContactOffersPanel({ contactId }: { contactId: number }) {
  const { t, i18n } = useTranslation();
  const toast = useToast();
  const [editing, setEditing] = useState<Offer | null>(null);
  const { data: offers = [] } = useQuery({
    queryKey: ["offers", "contact", contactId],
    queryFn: () => listOffers({ contactId }),
  });
  const create = async () => {
    try {
      setEditing(await newOfferForContact(contactId));
    } catch (e) {
      toast(errorMessage(e), "error");
    }
  };
  return (
    <div className="mt-5 border-t border-border pt-4">
      <div className="mb-2 flex items-center justify-between">
        <div className="flex items-center gap-1.5 text-2xs font-semibold uppercase tracking-wide text-fg-subtle">
          <Handshake size={12} /> {t("offers.title")}
        </div>
        <button className="btn-outline py-1 text-xs" onClick={create}>
          <Plus size={13} /> {t("offers.new")}
        </button>
      </div>
      {offers.length === 0 ? (
        <p className="text-xs text-fg-subtle">{t("offers.none")}</p>
      ) : (
        <ul className="divide-y divide-border border border-border text-sm">
          {offers.map((o) => (
            <li key={o.id}>
              <button
                className="flex w-full items-center gap-3 px-3 py-2 text-left hover:bg-muted"
                onClick={() => setEditing(o)}
              >
                <OfferStatusBadge status={o.status} />
                <span className="flex-1 truncate">
                  {[o.date ? formatDate(o.date) : t("offers.no_date"), o.venue, o.city].filter(Boolean).join(" · ")}
                </span>
                <span className="font-medium tabular-nums">{money(o.fee_amount, o.currency, i18n.language)}</span>
              </button>
            </li>
          ))}
        </ul>
      )}
      {editing && <OfferEditor offer={editing} onClose={() => setEditing(null)} />}
    </div>
  );
}

// ---------------- Agenda event: offer, checklist, documents ----------------

export function EventOfferPanel({ eventId }: { eventId: number }) {
  const { t, i18n } = useTranslation();
  const [editing, setEditing] = useState<Offer | null>(null);
  const { data: offers = [] } = useQuery({
    queryKey: ["offers", "event", eventId],
    queryFn: () => listOffers({ eventId }),
  });
  const { data: adv } = useQuery({
    queryKey: ["advancing", "event", eventId],
    queryFn: () => getEventAdvancing(eventId),
  });
  if (offers.length === 0 && !adv) return null;
  return (
    <div className="space-y-3 border border-border px-3 py-2.5">
      {offers.map((o) => (
        <div key={o.id} className="flex items-center gap-2 text-sm">
          <Handshake size={14} className="text-fg-subtle" />
          <span className="font-medium">{t("offers.offer")}</span>
          <OfferStatusBadge status={o.status} />
          <span className="flex-1 text-xs text-fg-subtle">
            {money(o.fee_amount, o.currency, i18n.language)}
            {o.balance_paid_at ? ` · ${t("offers.paid")}` : o.balance_due ? ` · ${t("offers.due_on", { date: formatDate(o.balance_due) })}` : ""}
          </span>
          <button className="btn-outline py-1 text-xs" onClick={() => setEditing(o)}>
            <FileText size={13} /> {t("offers.open")}
          </button>
        </div>
      ))}
      {adv && <AdvancingChecklist adv={adv} />}
      {editing && <OfferEditor offer={editing} onClose={() => setEditing(null)} />}
    </div>
  );
}

export function AdvancingChecklist({ adv }: { adv: GigAdvancing }) {
  const { t } = useTranslation();
  const qc = useQueryClient();
  const toast = useToast();
  const [draft, setDraft] = useState<EmailDraft | null>(null);
  const today = todayIso();
  const setItem = async (item: string, value: string | null) => {
    try {
      await setAdvancingItem(adv.event_id, item, value);
      qc.invalidateQueries({ queryKey: ["advancing"] });
    } catch (e) {
      toast(errorMessage(e), "error");
    }
  };
  const openDraft = async () => {
    try {
      setDraft(await advancingEmailDraft(adv.event_id));
    } catch (e) {
      toast(errorMessage(e), "error");
    }
  };
  const open = adv.items.filter((i) => !i.state).length;
  return (
    <div>
      <div className="mb-1.5 flex items-center justify-between">
        <span className="text-2xs font-semibold uppercase tracking-wide text-fg-subtle">
          {t("offers.advancing")} · {t("offers.adv_open", { count: open })}
        </span>
        <button className="btn-outline py-1 text-xs" onClick={openDraft}>
          <Mail size={13} />
          {adv.email_sent_at
            ? t("offers.adv_email_again", { date: formatDateTime(adv.email_sent_at) })
            : t("offers.adv_email", { date: formatDate(adv.email_due) })}
        </button>
      </div>
      <ul className="space-y-0.5">
        {adv.items.map((i) => (
          <AdvRow key={i.key} item={i} today={today} onSet={(v) => setItem(i.key, v)} />
        ))}
      </ul>
      {draft && (
        <DraftCompose draft={draft} onClose={() => setDraft(null)} onSent={() => setItem("email", "done")} />
      )}
    </div>
  );
}

function AdvRow({ item, today, onSet }: { item: AdvItem; today: string; onSet: (v: string | null) => void }) {
  const { t } = useTranslation();
  const late = !item.state && item.due < today;
  return (
    <li className="flex items-center gap-2 text-xs">
      <input
        type="checkbox"
        className="accent-accent"
        checked={item.state === "done"}
        disabled={item.state === "na"}
        onChange={(e) => onSet(e.target.checked ? "done" : null)}
      />
      <span className={clsx("flex-1", item.state && "text-fg-faint line-through")}>{t(`offers.adv_${item.key}`)}</span>
      <span className={clsx("tabular-nums", late ? "font-medium text-danger" : "text-fg-subtle")}>
        {item.offset < 0 ? `J${item.offset}` : `J+${item.offset}`} · {formatDate(item.due)}
      </span>
      <button
        className="text-fg-faint hover:text-fg"
        title={item.state === "na" ? t("offers.adv_reopen") : t("offers.adv_na")}
        onClick={() => onSet(item.state === "na" ? null : "na")}
      >
        <CircleSlash size={12} />
      </button>
    </li>
  );
}

// ---------------- Budget: expected / received ----------------

export function PaymentsPanel() {
  const { t, i18n } = useTranslation();
  const qc = useQueryClient();
  const toast = useToast();
  const [showIssuer, setShowIssuer] = useState(false);
  const { data: payments = [] } = useQuery({ queryKey: ["offer_payments"], queryFn: listOfferPayments });
  const sum = (ps: Payment[]) => ps.reduce((s, p) => s + p.amount, 0);
  const paid = payments.filter((p) => p.paid_at);
  const pending = payments.filter((p) => !p.paid_at);
  const overdue = pending.filter((p) => p.overdue);
  const cur = payments[0]?.currency ?? "EUR";
  const toggle = async (p: Payment) => {
    try {
      await setOfferPayment(p.offer_id, p.part, p.paid_at ? null : todayIso());
      invalidateOffers(qc);
    } catch (e) {
      toast(offerError(t, e), "error");
    }
  };
  return (
    <section className="panel p-4">
      <div className="mb-3 flex items-center justify-between">
        <h2 className="eyebrow text-fg">{t("offers.payments")}</h2>
        <button className="btn-ghost py-1 text-xs" onClick={() => setShowIssuer((v) => !v)}>
          {t("offers.issuer_settings")}
        </button>
      </div>
      {showIssuer && <InvoiceSettings />}
      <div className="mb-3 grid grid-cols-3 gap-3 text-sm">
        <div>
          <div className="text-2xs uppercase text-fg-subtle">{t("offers.expected")}</div>
          <div className="font-semibold tabular-nums">{money(sum(pending), cur, i18n.language) || "·"}</div>
        </div>
        <div>
          <div className="text-2xs uppercase text-fg-subtle">{t("offers.received")}</div>
          <div className="font-semibold tabular-nums text-ok">{money(sum(paid), cur, i18n.language) || "·"}</div>
        </div>
        <div>
          <div className="text-2xs uppercase text-fg-subtle">{t("offers.overdue")}</div>
          <div className={clsx("font-semibold tabular-nums", overdue.length && "text-danger")}>
            {overdue.length ? money(sum(overdue), cur, i18n.language) : "·"}
          </div>
        </div>
      </div>
      {payments.length === 0 ? (
        <p className="text-xs text-fg-subtle">{t("offers.no_payments")}</p>
      ) : (
        <table className="w-full text-sm">
          <tbody className="divide-y divide-border">
            {payments.map((p) => (
              <tr key={`${p.offer_id}-${p.part}`}>
                <td className="py-1.5 pr-2">
                  <div className="font-medium">{p.title}</div>
                  <div className="text-2xs text-fg-subtle">
                    {t(`offers.part_${p.part}`)}
                    {p.invoice_number ? ` · ${p.invoice_number}` : ""}
                    {p.gig_date ? ` · ${formatDate(p.gig_date)}` : ""}
                  </div>
                </td>
                <td className={clsx("py-1.5 pr-2 text-xs", p.overdue ? "font-medium text-danger" : "text-fg-subtle")}>
                  {p.paid_at
                    ? t("offers.paid_on", { date: formatDate(p.paid_at) })
                    : p.due
                      ? t("offers.due_on", { date: formatDate(p.due) })
                      : "·"}
                </td>
                <td className="py-1.5 pr-2 text-right font-medium tabular-nums">
                  {money(p.amount, p.currency, i18n.language)}
                </td>
                <td className="py-1.5 text-right">
                  <button className={clsx("py-1 text-xs", p.paid_at ? "btn-ghost" : "btn-outline")} onClick={() => toggle(p)}>
                    {p.paid_at ? t("offers.mark_unpaid") : t("offers.mark_paid")}
                  </button>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </section>
  );
}

/// Seller details printed on invoices (legal name, address, SIRET, bank, VAT
/// note). Typed once; empty fields are simply left out of the PDF.
function InvoiceSettings() {
  const { t } = useTranslation();
  const toast = useToast();
  const keys = ["invoice_prefix", "invoice_issuer", "invoice_bank", "invoice_vat_note"] as const;
  const [vals, setVals] = useState<Record<string, string>>({});
  useEffect(() => {
    Promise.all(keys.map((k) => getSetting(k).then((v) => [k, v ?? ""] as const))).then((kv) =>
      setVals(Object.fromEntries(kv))
    );
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);
  const save = async () => {
    try {
      await Promise.all(keys.map((k) => setSetting(k, vals[k] ?? "")));
      toast(t("offers.saved"), "ok");
    } catch (e) {
      toast(errorMessage(e), "error");
    }
  };
  return (
    <div className="mb-4 space-y-2 border border-border p-3">
      <p className="text-2xs text-fg-subtle">{t("offers.issuer_hint")}</p>
      {keys.map((k) => (
        <Field key={k} label={t(`offers.${k}`)}>
          {k === "invoice_prefix" ? (
            <input
              className="input"
              placeholder="F"
              value={vals[k] ?? ""}
              onChange={(e) => setVals((v) => ({ ...v, [k]: e.target.value }))}
            />
          ) : (
            <textarea
              className="input min-h-[60px] text-xs"
              value={vals[k] ?? ""}
              onChange={(e) => setVals((v) => ({ ...v, [k]: e.target.value }))}
            />
          )}
        </Field>
      ))}
      <div className="flex justify-end">
        <button className="btn-primary py-1 text-xs" onClick={save}>
          {t("common.save")}
        </button>
      </div>
    </div>
  );
}

// ---------------- Home: overdue payments ----------------

export function OverduePaymentsAlert() {
  const { t, i18n } = useTranslation();
  const { data: payments = [] } = useQuery({ queryKey: ["offer_payments"], queryFn: listOfferPayments });
  const overdue = payments.filter((p) => p.overdue && !p.paid_at);
  if (overdue.length === 0) return null;
  const total = overdue.reduce((s, p) => s + p.amount, 0);
  return (
    <Link
      to="/budget"
      className="flex items-center gap-3 border-b border-border bg-danger/10 px-8 py-3 text-sm font-semibold text-danger"
    >
      <AlertTriangle size={16} />
      <span className="flex-1">
        {t("offers.overdue_alert", {
          count: overdue.length,
          amount: money(total, overdue[0].currency, i18n.language),
        })}
      </span>
      <span className="text-xs underline">{t("offers.see_payments")}</span>
    </Link>
  );
}
