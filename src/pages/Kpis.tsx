import { useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { Plus, Pencil, Trash2, Target } from "lucide-react";
import { deleteKpi, listKpis, saveKpi } from "../lib/api";
import type { Kpi } from "../lib/types";
import { PageHeader } from "../components/Layout";
import { Modal, Field, useToast, useConfirm } from "../components/ui";

export function Kpis() {
  const { t } = useTranslation();
  const qc = useQueryClient();
  const confirm = useConfirm();
  const { data: kpis = [] } = useQuery({
    queryKey: ["kpis"],
    queryFn: () => listKpis(),
  });
  const [editing, setEditing] = useState<Kpi | null>(null);

  const delMut = useMutation({
    mutationFn: (id: number) => deleteKpi(id),
    onSuccess: () => qc.invalidateQueries({ queryKey: ["kpis"] }),
  });

  return (
    <div>
      <PageHeader
        title={t("kpis.title")}
        subtitle={t("kpis.subtitle")}
        actions={
          <button
            className="btn-primary"
            onClick={() => setEditing({ sort: kpis.length } as Kpi)}
          >
            <Plus size={16} />
            {t("kpis.new_kpi")}
          </button>
        }
      />
      <div className="px-8 pb-10">
        {kpis.length === 0 ? (
          <div className="card flex flex-col items-center gap-2 py-16 text-sm text-fg-subtle">
            <Target size={28} className="opacity-40" />
            {t("common.empty")}
          </div>
        ) : (
          <div className="grid grid-cols-1 gap-4 md:grid-cols-2 lg:grid-cols-3">
            {kpis.map((k) => (
              <div key={k.id} className="card group p-5">
                <div className="flex items-start justify-between">
                  <div className="text-xs font-medium uppercase tracking-wide text-accent">
                    {k.goal}
                  </div>
                  <div className="flex gap-1 opacity-0 transition-opacity group-hover:opacity-100">
                    <button
                      className="btn-ghost px-1.5 py-1"
                      onClick={() => setEditing(k)}
                    >
                      <Pencil size={13} />
                    </button>
                    <button
                      className="btn-ghost px-1.5 py-1 text-rose-500"
                      onClick={() =>
                        confirm(t("common.confirm_delete")) && delMut.mutate(k.id!)
                      }
                    >
                      <Trash2 size={13} />
                    </button>
                  </div>
                </div>
                <div className="mt-1.5 text-sm font-medium">{k.kpi}</div>
                <div className="mt-4 flex items-end justify-between">
                  <div>
                    <div className="text-xs text-fg-subtle">{t("kpis.target")}</div>
                    <div className="text-sm font-semibold">{k.target || "·"}</div>
                  </div>
                  <div className="text-right">
                    <div className="text-xs text-fg-subtle">{t("kpis.actual")}</div>
                    <div className="text-lg font-bold text-emerald-500">
                      {k.actual || "·"}
                    </div>
                  </div>
                </div>
              </div>
            ))}
          </div>
        )}
      </div>

      {editing && (
        <KpiModal
          kpi={editing}
          onClose={() => setEditing(null)}
          onSaved={() => {
            setEditing(null);
            qc.invalidateQueries({ queryKey: ["kpis"] });
          }}
        />
      )}
    </div>
  );
}

function KpiModal({
  kpi,
  onClose,
  onSaved,
}: {
  kpi: Kpi;
  onClose: () => void;
  onSaved: () => void;
}) {
  const { t } = useTranslation();
  const toast = useToast();
  const [form, setForm] = useState<Kpi>({ ...kpi });
  const save = async () => {
    await saveKpi(form);
    toast(t("common.save"), "ok");
    onSaved();
  };
  const inp = (k: keyof Kpi) => ({
    className: "input",
    value: (form[k] as string) ?? "",
    onChange: (e: any) => setForm({ ...form, [k]: e.target.value }),
  });
  return (
    <Modal open onClose={onClose} title={t("kpis.new_kpi")} width="max-w-lg">
      <div className="space-y-3.5">
        <Field label={t("kpis.goal")}>
          <input {...inp("goal")} />
        </Field>
        <Field label={t("kpis.kpi")}>
          <input {...inp("kpi")} />
        </Field>
        <div className="grid grid-cols-2 gap-4">
          <Field label={t("kpis.target")}>
            <input {...inp("target")} />
          </Field>
          <Field label={t("kpis.actual")}>
            <input {...inp("actual")} />
          </Field>
        </div>
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
