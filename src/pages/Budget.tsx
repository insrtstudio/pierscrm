import { useMemo, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { Link } from "react-router-dom";
import { Plus, Pencil, Trash2, TrendingDown, TrendingUp, CalendarDays, Megaphone, Link2 } from "lucide-react";
import {
  dashboardStats,
  deleteBudgetItem,
  linkBudgetItem,
  listBudget,
  listEvents,
  saveBudgetItem,
} from "../lib/api";
import type { BudgetItem, Event } from "../lib/types";
import { euro } from "../lib/format";
import { PageHeader } from "../components/Layout";
import { Modal, Field, useToast, useConfirm, useSubmit } from "../components/ui";

export function Budget() {
  const { t } = useTranslation();
  const qc = useQueryClient();
  const confirm = useConfirm();
  const { data: items = [] } = useQuery({
    queryKey: ["budget"],
    queryFn: listBudget,
  });
  // A confirmed Agenda gig with a fee owns one revenue line here (event_id
  // set, kept in step by the backend), so each fee is counted exactly once.
  // Meta Ads spend recorded by Audience is shown as a read-only expense.
  const { data: stats } = useQuery({ queryKey: ["dashboard"], queryFn: dashboardStats });
  const adSpend = stats?.ad_spend ?? 0;
  const [editing, setEditing] = useState<BudgetItem | null>(null);
  const toast = useToast();
  // Gigs that own a fee line: the targets a manual revenue line can be linked to.
  const { data: allEvents = [] } = useQuery({
    queryKey: ["events", "budget"],
    queryFn: () => listEvents({}),
  });
  const paidGigs = allEvents.filter((e) => e.status === "confirmed" && (e.fee ?? 0) > 0);
  const refresh = () => {
    qc.invalidateQueries({ queryKey: ["budget"] });
    qc.invalidateQueries({ queryKey: ["dashboard"] });
  };

  const delMut = useMutation({
    mutationFn: (id: number) => deleteBudgetItem(id),
    onSuccess: refresh,
  });
  const linkMut = useMutation({
    mutationFn: ({ id, eventId }: { id: number; eventId: number }) => linkBudgetItem(id, eventId),
    onSuccess: () => {
      refresh();
      toast(t("budget.linked_ok"), "ok");
    },
    onError: (e: any) => toast(e?.toString?.() ?? "error", "error"),
  });

  const expenses = items.filter((i) => i.kind === "expense");
  const revenues = items.filter((i) => i.kind === "revenue");
  // A manual revenue line with the amount or the name of a gig's fee line is
  // probably that fee typed twice: flagged, with a one-click link.
  const duplicates = useMemo(() => {
    const m = new Map<number, Event>();
    const norm = (s?: string | null) => (s ?? "").trim().toLowerCase();
    for (const i of revenues) {
      if (i.event_id != null || i.id == null) continue;
      for (const l of revenues) {
        const ev = l.event_id != null ? paidGigs.find((e) => e.id === l.event_id) : undefined;
        if (!ev) continue;
        const sameAmount = i.actual != null && i.actual > 0 && i.actual === l.actual;
        const name = norm(i.item);
        const sameName = name !== "" && (name === norm(l.item) || name === norm(ev.title));
        if (sameAmount || sameName) {
          m.set(i.id, ev);
          break;
        }
      }
    }
    return m;
  }, [items, allEvents]);

  const totals = useMemo(() => {
    const min = expenses.reduce((a, i) => a + (i.min_cost || 0), 0);
    const max = expenses.reduce((a, i) => a + (i.max_cost || 0), 0);
    const spent = expenses.reduce((a, i) => a + (i.actual || 0), 0) + adSpend;
    const rev = revenues.reduce((a, i) => a + (i.actual || 0), 0);
    return { min, max, spent, rev, net: rev - spent };
  }, [items, adSpend]);

  return (
    <div>
      <PageHeader
        title={t("budget.title")}
        subtitle={t("budget.subtitle")}
        actions={
          <button
            className="btn-primary"
            onClick={() =>
              setEditing({
                item: "",
                min_cost: 0,
                max_cost: 0,
                kind: "expense",
                sort: items.length,
              } as BudgetItem)
            }
          >
            <Plus size={16} />
            {t("budget.new_item")}
          </button>
        }
      />
      <div className="space-y-6 px-page pb-10 pt-6">
        {/* Summary */}
        <div className="grid grid-cols-2 gap-4 lg:grid-cols-4">
          <Summary
            icon={TrendingDown}
            label={t("budget.total_expense")}
            value={`${euro(totals.min)} – ${euro(totals.max)}`}
            tone="text-danger"
          />
          <Summary
            icon={TrendingDown}
            label={t("budget.actual")}
            value={euro(totals.spent)}
            tone="text-danger"
          />
          <Summary
            icon={TrendingUp}
            label={t("budget.total_revenue")}
            value={euro(totals.rev)}
            tone="text-ok"
          />
          <Summary
            icon={TrendingUp}
            label={t("budget.net_estimate")}
            value={euro(totals.net)}
            tone={totals.net >= 0 ? "text-ok" : "text-danger"}
          />
        </div>

        <BudgetTable
          title={t("budget.expense")}
          rows={expenses}
          onEdit={setEditing}
          onDelete={(id) => confirm(t("common.confirm_delete")) && delMut.mutate(id)}
          extra={
            adSpend > 0 ? (
              <tr className="border-b border-border/60">
                <td className="px-5 py-2.5">
                  <div className="flex items-center gap-2 font-medium">
                    <Megaphone size={13} className="text-fg-subtle" />
                    {t("budget.ad_spend")}
                  </div>
                  <div className="text-xs text-fg-subtle">{t("budget.ad_spend_hint")}</div>
                </td>
                <td className="px-3 py-2.5 text-right tabular-nums text-fg-subtle">·</td>
                <td className="px-3 py-2.5 text-right tabular-nums text-fg-subtle">·</td>
                <td className="px-3 py-2.5 text-right tabular-nums font-medium">{euro(adSpend)}</td>
                <td className="px-5 py-2.5 text-xs">
                  <Link to="/audience" className="text-fg-subtle hover:text-accent">
                    {t("budget.from_audience")} →
                  </Link>
                </td>
                <td />
              </tr>
            ) : null
          }
        />
        <BudgetTable
          title={t("budget.revenue")}
          rows={revenues}
          onEdit={setEditing}
          onDelete={(id) => confirm(t("common.confirm_delete")) && delMut.mutate(id)}
          revenue
          duplicates={duplicates}
          onLink={(id, eventId) => linkMut.mutate({ id, eventId })}
        />
      </div>

      {editing && (
        <BudgetModal
          item={editing}
          gigs={paidGigs}
          suggested={editing.id != null ? duplicates.get(editing.id)?.id : undefined}
          onClose={() => setEditing(null)}
          onSaved={() => {
            setEditing(null);
            refresh();
          }}
        />
      )}
    </div>
  );
}

function Summary({
  icon: Icon,
  label,
  value,
  tone,
}: {
  icon: any;
  label: string;
  value: string;
  tone: string;
}) {
  return (
    <div className="card p-4">
      <div className="flex items-center gap-2 text-xs text-fg-subtle">
        <Icon size={14} className={tone} />
        {label}
      </div>
      <div className="mt-1.5 text-lg font-semibold">{value}</div>
    </div>
  );
}

function BudgetTable({
  title,
  rows,
  onEdit,
  onDelete,
  revenue,
  extra,
  duplicates,
  onLink,
}: {
  title: string;
  rows: BudgetItem[];
  onEdit: (i: BudgetItem) => void;
  onDelete: (id: number) => void;
  revenue?: boolean;
  /// Read-only row appended to the table (ad spend).
  extra?: React.ReactNode;
  /// Manual lines that look like a gig's fee typed again, by line id.
  duplicates?: Map<number, Event>;
  onLink?: (id: number, eventId: number) => void;
}) {
  const { t } = useTranslation();
  if (rows.length === 0 && !extra) return null;
  return (
    <div className="card overflow-hidden">
      <div className="eyebrow border-b border-border px-5 py-3 text-fg">
        {title}
      </div>
      <div className="overflow-x-auto">
      <table className="w-full text-sm">
        <thead>
          <tr className="border-b border-border text-left font-mono text-[10.5px] uppercase tracking-[0.14em] text-fg-subtle">
            <th className="px-5 py-2 font-normal">{t("budget.item")}</th>
            {!revenue && <th className="px-3 py-2 text-right font-normal">{t("budget.min")}</th>}
            {!revenue && <th className="px-3 py-2 text-right font-normal">{t("budget.max")}</th>}
            <th className="px-3 py-2 text-right font-normal">{t("budget.actual")}</th>
            <th className="px-5 py-2 font-normal">{t("budget.notes")}</th>
            <th className="w-20 px-3 py-2"></th>
          </tr>
        </thead>
        <tbody>
          {rows.map((i) => {
            const dup = i.id != null ? duplicates?.get(i.id) : undefined;
            return (
            <tr key={i.id} className="group border-b border-border/60 hover:bg-muted/40">
              <td className="px-5 py-2.5">
                <div className="flex items-center gap-2 font-medium">
                  {i.event_id != null && <CalendarDays size={13} className="text-fg-subtle" />}
                  {i.item}
                  {i.event_id != null && (
                    <Link
                      to="/agenda"
                      className="badge border border-border py-0 text-fg-subtle hover:text-accent"
                      title={t("budget.linked_event_hint")}
                    >
                      {t("budget.linked_event")}
                    </Link>
                  )}
                  {dup && (
                    <span
                      className="badge bg-warn/15 py-0 text-warn"
                      title={t("budget.duplicate_hint", { title: dup.title })}
                    >
                      {t("budget.duplicate")}
                    </span>
                  )}
                  {dup && onLink && (
                    <button
                      className="btn-ghost px-1.5 py-0.5 text-xs"
                      title={t("budget.duplicate_hint", { title: dup.title })}
                      onClick={() => onLink(i.id!, dup.id!)}
                    >
                      <Link2 size={12} />
                      {t("budget.link_now")}
                    </button>
                  )}
                </div>
                {i.category && (
                  <div className="text-xs text-fg-subtle">{i.category}</div>
                )}
              </td>
              {!revenue && (
                <td className="px-3 py-2.5 text-right tabular-nums">
                  {euro(i.min_cost)}
                </td>
              )}
              {!revenue && (
                <td className="px-3 py-2.5 text-right tabular-nums">
                  {euro(i.max_cost)}
                </td>
              )}
              <td className="px-3 py-2.5 text-right tabular-nums font-medium">
                {i.actual != null ? euro(i.actual) : "·"}
              </td>
              <td className="max-w-[280px] truncate px-5 py-2.5 text-xs text-fg-subtle">
                {i.notes}
              </td>
              <td className="px-3 py-2.5">
                <div className="flex justify-end gap-1 opacity-0 transition-opacity group-hover:opacity-100 group-focus-within:opacity-100">
                  <button className="btn-ghost px-2 py-1.5" onClick={() => onEdit(i)}>
                    <Pencil size={14} />
                  </button>
                  {i.event_id == null && (
                    <button
                      className="btn-ghost px-2 py-1.5 text-danger"
                      onClick={() => onDelete(i.id!)}
                    >
                      <Trash2 size={14} />
                    </button>
                  )}
                </div>
              </td>
            </tr>
            );
          })}
          {extra}
        </tbody>
      </table>
      </div>
    </div>
  );
}

function BudgetModal({
  item,
  gigs,
  suggested,
  onClose,
  onSaved,
}: {
  item: BudgetItem;
  /// Confirmed gigs with a fee (link targets).
  gigs: Event[];
  /// Gig this line seems to duplicate, preselected.
  suggested?: number;
  onClose: () => void;
  onSaved: () => void;
}) {
  const { t } = useTranslation();
  const toast = useToast();
  const [form, setForm] = useState<BudgetItem>({ ...item });
  // A gig's line follows its Agenda event: kind and amount are not editable here.
  const linked = item.event_id != null;
  const [linkTo, setLinkTo] = useState<number | "">(suggested ?? "");
  const willLink = !linked && form.kind === "revenue" && linkTo !== "";
  const num = (v: string) => (v === "" ? 0 : parseFloat(v));
  const [save, busy] = useSubmit(async () => {
    if (!form.item.trim()) return;
    const id = await saveBudgetItem(form);
    if (willLink) await linkBudgetItem(id, linkTo as number);
    toast(t(willLink ? "budget.linked_ok" : "common.save"), "ok");
    onSaved();
  });
  return (
    <Modal open onClose={onClose} title={item.id ? t("budget.edit_item") : t("budget.new_item")} width="max-w-lg">
      <div className="grid grid-cols-2 gap-x-4 gap-y-3">
        <Field label={t("budget.item")} className="col-span-2">
          <input
            className="input"
            value={form.item}
            onChange={(e) => setForm({ ...form, item: e.target.value })}
          />
        </Field>
        <Field label={t("budget.kind")}>
          <select
            className="input"
            value={form.kind}
            disabled={linked}
            onChange={(e) =>
              setForm({ ...form, kind: e.target.value as "expense" | "revenue" })
            }
          >
            <option value="expense">{t("budget.expense")}</option>
            <option value="revenue">{t("budget.revenue")}</option>
          </select>
        </Field>
        <Field label={t("budget.category")}>
          <input
            className="input"
            value={form.category ?? ""}
            onChange={(e) => setForm({ ...form, category: e.target.value })}
          />
        </Field>
        {form.kind === "expense" && (
          <>
            <Field label={t("budget.min")}>
              <input
                type="number"
                className="input"
                value={form.min_cost}
                onChange={(e) => setForm({ ...form, min_cost: num(e.target.value) })}
              />
            </Field>
            <Field label={t("budget.max")}>
              <input
                type="number"
                className="input"
                value={form.max_cost}
                onChange={(e) => setForm({ ...form, max_cost: num(e.target.value) })}
              />
            </Field>
          </>
        )}
        <Field label={t("budget.actual")} className={form.kind === "revenue" ? "col-span-2" : ""}>
          <input
            type="number"
            className="input"
            value={form.actual ?? ""}
            disabled={linked}
            onChange={(e) =>
              setForm({
                ...form,
                actual: e.target.value === "" ? null : num(e.target.value),
              })
            }
          />
        </Field>
        {linked && (
          <p className="col-span-2 text-xs text-fg-subtle">{t("budget.linked_event_hint")}</p>
        )}
        {!linked && form.kind === "revenue" && gigs.length > 0 && (
          <Field label={t("budget.link_event")} className="col-span-2">
            <select
              className="input"
              value={linkTo}
              onChange={(e) => setLinkTo(e.target.value === "" ? "" : Number(e.target.value))}
            >
              <option value="">{t("budget.link_event_none")}</option>
              {gigs.map((g) => (
                <option key={g.id} value={g.id!}>
                  {[g.date, g.title, g.city].filter(Boolean).join(" · ")} ({euro(g.fee ?? 0)})
                </option>
              ))}
            </select>
            <p className="mt-1 text-xs text-fg-subtle">{t("budget.link_event_hint")}</p>
          </Field>
        )}
        <Field label={t("budget.notes")} className="col-span-2">
          <textarea
            className="input min-h-[70px]"
            value={form.notes ?? ""}
            onChange={(e) => setForm({ ...form, notes: e.target.value })}
          />
        </Field>
      </div>
      <div className="mt-5 flex justify-end gap-2">
        <button className="btn-outline" onClick={onClose}>
          {t("common.cancel")}
        </button>
        <button className="btn-primary" onClick={save} disabled={busy}>
          {t("common.save")}
        </button>
      </div>
    </Modal>
  );
}
