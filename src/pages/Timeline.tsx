import { useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { Plus, Pencil, Trash2 } from "lucide-react";
import clsx from "clsx";
import { deleteTask, listTasks, saveTask } from "../lib/api";
import type { Task } from "../lib/types";
import { PageHeader } from "../components/Layout";
import { Modal, Field, useToast, useConfirm } from "../components/ui";

export function Timeline() {
  const { t } = useTranslation();
  const qc = useQueryClient();
  const confirm = useConfirm();
  const { data: tasks = [] } = useQuery({ queryKey: ["tasks"], queryFn: listTasks });
  const [editing, setEditing] = useState<Task | null>(null);

  const invalidate = () => {
    qc.invalidateQueries({ queryKey: ["tasks"] });
    qc.invalidateQueries({ queryKey: ["dashboard"] });
  };

  const toggleMut = useMutation({
    mutationFn: (task: Task) => saveTask({ ...task, done: !task.done }),
    onSuccess: invalidate,
  });
  const delMut = useMutation({
    mutationFn: (id: number) => deleteTask(id),
    onSuccess: invalidate,
  });

  // Group by period preserving order.
  const groups: { period: string; items: Task[] }[] = [];
  for (const task of tasks) {
    const p = task.period || "·";
    let g = groups.find((x) => x.period === p);
    if (!g) {
      g = { period: p, items: [] };
      groups.push(g);
    }
    g.items.push(task);
  }

  return (
    <div>
      <PageHeader
        title={t("timeline.title")}
        subtitle={t("timeline.subtitle")}
        actions={
          <button
            className="btn-primary"
            onClick={() =>
              setEditing({ title: "", done: false, sort: tasks.length } as Task)
            }
          >
            <Plus size={16} />
            {t("timeline.new_task")}
          </button>
        }
      />
      <div className="space-y-6 px-8 pb-10">
        {groups.length === 0 && (
          <div className="card py-16 text-center text-sm text-fg-subtle">
            {t("common.empty")}
          </div>
        )}
        {groups.map((g) => (
          <div key={g.period}>
            <div className="mb-2 flex items-center gap-3">
              <span className="rounded-md bg-accent/10 px-2.5 py-1 text-xs font-semibold text-accent">
                {g.period}
              </span>
              <div className="h-px flex-1 bg-border" />
            </div>
            <div className="card divide-y divide-border overflow-hidden">
              {g.items.map((task) => (
                <div
                  key={task.id}
                  className="group flex items-center gap-3 px-4 py-3"
                >
                  <input
                    type="checkbox"
                    checked={task.done}
                    onChange={() => toggleMut.mutate(task)}
                    className="h-4 w-4 accent-emerald-500"
                  />
                  <div className="min-w-0 flex-1">
                    <div
                      className={clsx(
                        "text-sm",
                        task.done && "text-fg-subtle line-through"
                      )}
                    >
                      {task.title}
                    </div>
                    {(task.owner || task.due_date) && (
                      <div className="text-xs text-fg-subtle">
                        {[task.owner, task.due_date].filter(Boolean).join(" · ")}
                      </div>
                    )}
                  </div>
                  <div className="flex gap-1 opacity-0 transition-opacity group-hover:opacity-100">
                    <button
                      className="btn-ghost px-2 py-1.5"
                      onClick={() => setEditing(task)}
                    >
                      <Pencil size={14} />
                    </button>
                    <button
                      className="btn-ghost px-2 py-1.5 text-rose-500"
                      onClick={() =>
                        confirm(t("common.confirm_delete")) && delMut.mutate(task.id!)
                      }
                    >
                      <Trash2 size={14} />
                    </button>
                  </div>
                </div>
              ))}
            </div>
          </div>
        ))}
      </div>

      {editing && (
        <TaskModal
          task={editing}
          onClose={() => setEditing(null)}
          onSaved={() => {
            setEditing(null);
            invalidate();
          }}
        />
      )}
    </div>
  );
}

function TaskModal({
  task,
  onClose,
  onSaved,
}: {
  task: Task;
  onClose: () => void;
  onSaved: () => void;
}) {
  const { t } = useTranslation();
  const toast = useToast();
  const [form, setForm] = useState<Task>({ ...task });
  const save = async () => {
    if (!form.title.trim()) return;
    await saveTask(form);
    toast(t("common.save"), "ok");
    onSaved();
  };
  return (
    <Modal open onClose={onClose} title={t("timeline.new_task")} width="max-w-lg">
      <div className="space-y-3.5">
        <Field label={t("timeline.task")}>
          <textarea
            className="input min-h-[70px]"
            value={form.title}
            onChange={(e) => setForm({ ...form, title: e.target.value })}
          />
        </Field>
        <div className="grid grid-cols-2 gap-4">
          <Field label={t("timeline.period")}>
            <input
              className="input"
              value={form.period ?? ""}
              onChange={(e) => setForm({ ...form, period: e.target.value })}
            />
          </Field>
          <Field label={t("timeline.owner")}>
            <input
              className="input"
              value={form.owner ?? ""}
              onChange={(e) => setForm({ ...form, owner: e.target.value })}
            />
          </Field>
          <Field label={t("timeline.due")}>
            <input
              className="input"
              value={form.due_date ?? ""}
              onChange={(e) => setForm({ ...form, due_date: e.target.value })}
            />
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
