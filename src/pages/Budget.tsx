import { useMemo, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { Plus, Pencil, Trash2, TrendingDown, TrendingUp } from "lucide-react";
import {
  deleteBudgetItem,
  listBudget,
  saveBudgetItem,
} from "../lib/api";
import type { BudgetItem } from "../lib/types";
import { PageHeader } from "../components/Layout";
import { Modal, Field, useToast, useConfirm } from "../components/ui";

const euro = (n: number) =>
  new Intl.NumberFormat("fr-FR", {
    style: "currency",
    currency: "EUR",
    maximumFractionDigits: 0,
  }).format(n || 0);

export function Budget() {
  const { t } = useTranslation();
  const qc = useQueryClient();
  const confirm = useConfirm();
  const { data: items = [] } = useQuery({
    queryKey: ["budget"],
    queryFn: listBudget,
  });
  const [editing, setEditing] = useState<BudgetItem | null>(null);

  const delMut = useMutation({
    mutationFn: (id: number) => deleteBudgetItem(id),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ["budget"] });
      qc.invalidateQueries({ queryKey: ["dashboard"] });
    },
  });

  const expenses = items.filter((i) => i.kind === "expense");
  const revenues = items.filter((i) => i.kind === "revenue");

  const totals = useMemo(() => {
    const min = expenses.reduce((a, i) => a + (i.min_cost || 0), 0);
    const max = expenses.reduce((a, i) => a + (i.max_cost || 0), 0);
    const spent = expenses.reduce((a, i) => a + (i.actual || 0), 0);
    const rev = revenues.reduce((a, i) => a + (i.actual || 0), 0);
    return { min, max, spent, rev, net: rev - spent };
  }, [items]);

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
      <div className="space-y-6 px-8 pb-10">
        {/* Summary */}
        <div className="grid grid-cols-2 gap-4 lg:grid-cols-4">
          <Summary
            icon={TrendingDown}
            label={t("budget.total_expense")}
            value={`${euro(totals.min)} – ${euro(totals.max)}`}
            tone="text-rose-500"
          />
          <Summary
            icon={TrendingDown}
            label={t("budget.actual")}
            value={euro(totals.spent)}
            tone="text-rose-500"
          />
          <Summary
            icon={TrendingUp}
            label={t("budget.total_revenue")}
            value={euro(totals.rev)}
            tone="text-emerald-500"
          />
          <Summary
            icon={TrendingUp}
            label={t("budget.net_estimate")}
            value={euro(totals.net)}
            tone={totals.net >= 0 ? "text-emerald-500" : "text-rose-500"}
          />
        </div>

        <BudgetTable
          title={t("budget.expense")}
          rows={expenses}
          onEdit={setEditing}
          onDelete={(id) => confirm(t("common.confirm_delete")) && delMut.mutate(id)}
        />
        <BudgetTable
          title={t("budget.revenue")}
          rows={revenues}
          onEdit={setEditing}
          onDelete={(id) => confirm(t("common.confirm_delete")) && delMut.mutate(id)}
          revenue
        />
      </div>

      {editing && (
        <BudgetModal
          item={editing}
          onClose={() => setEditing(null)}
          onSaved={() => {
            setEditing(null);
            qc.invalidateQueries({ queryKey: ["budget"] });
            qc.invalidateQueries({ queryKey: ["dashboard"] });
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
}: {
  title: string;
  rows: BudgetItem[];
  onEdit: (i: BudgetItem) => void;
  onDelete: (id: number) => void;
  revenue?: boolean;
}) {
  const { t } = useTranslation();
  if (rows.length === 0) return null;
  return (
    <div className="card overflow-hidden">
      <div className="border-b border-border px-5 py-3 text-sm font-semibold">
        {title}
      </div>
      <table className="w-full text-sm">
        <thead>
          <tr className="border-b border-border text-left text-xs text-fg-subtle">
            <th className="px-5 py-2 font-medium">{t("budget.item")}</th>
            {!revenue && <th className="px-3 py-2 text-right font-medium">{t("budget.min")}</th>}
            {!revenue && <th className="px-3 py-2 text-right font-medium">{t("budget.max")}</th>}
            <th className="px-3 py-2 text-right font-medium">{t("budget.actual")}</th>
            <th className="px-5 py-2 font-medium">{t("budget.notes")}</th>
            <th className="w-20 px-3 py-2"></th>
          </tr>
        </thead>
        <tbody>
          {rows.map((i) => (
            <tr key={i.id} className="group border-b border-border/60 hover:bg-muted/40">
              <td className="px-5 py-2.5">
                <div className="font-medium">{i.item}</div>
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
                <div className="flex justify-end gap-1 opacity-0 transition-opacity group-hover:opacity-100">
                  <button className="btn-ghost px-2 py-1.5" onClick={() => onEdit(i)}>
                    <Pencil size={14} />
                  </button>
                  <button
                    className="btn-ghost px-2 py-1.5 text-rose-500"
                    onClick={() => onDelete(i.id!)}
                  >
                    <Trash2 size={14} />
                  </button>
                </div>
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

function BudgetModal({
  item,
  onClose,
  onSaved,
}: {
  item: BudgetItem;
  onClose: () => void;
  onSaved: () => void;
}) {
  const { t } = useTranslation();
  const toast = useToast();
  const [form, setForm] = useState<BudgetItem>({ ...item });
  const num = (v: string) => (v === "" ? 0 : parseFloat(v));
  const save = async () => {
    if (!form.item.trim()) return;
    await saveBudgetItem(form);
    toast(t("common.save"), "ok");
    onSaved();
  };
  return (
    <Modal open onClose={onClose} title={t("budget.new_item")} width="max-w-lg">
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
            onChange={(e) =>
              setForm({
                ...form,
                actual: e.target.value === "" ? null : num(e.target.value),
              })
            }
          />
        </Field>
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
        <button className="btn-primary" onClick={save}>
          {t("common.save")}
        </button>
      </div>
    </Modal>
  );
}
