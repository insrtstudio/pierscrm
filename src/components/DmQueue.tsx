import { useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { invoke } from "@tauri-apps/api/core";
import { useTranslation } from "react-i18next";
import clsx from "clsx";
import { Instagram } from "lucide-react";
import { DmPanel, IgHandle } from "./InstagramDm";

interface DmQueueItem {
  promoter_id: number;
  name: string;
  city?: string | null;
  country?: string | null;
  handle: string;
  fit_score?: number | null;
  matched: string[];
}

/// Aujourd'hui: the collectives worth a DM today (local crews that booked a
/// reference artist, with an Instagram but no booking address). One at a time
/// opens the assisted DM; "DM envoyé" takes it off the list.
export function DmQueue() {
  const { t } = useTranslation();
  const qc = useQueryClient();
  const [open, setOpen] = useState<number | null>(null);
  const { data: items = [] } = useQuery({
    queryKey: ["social_dm_queue"],
    queryFn: () => invoke<DmQueueItem[]>("social_dm_queue", { limit: 5 }),
  });
  if (!items.length) return null;
  return (
    <section className="px-page pt-6">
      <div className="mb-2 flex items-baseline gap-2">
        <h2 className="eyebrow flex items-center gap-1.5 text-fg">
          <Instagram size={12} /> {t("dm_queue.title")}
        </h2>
        <span className="text-xs text-fg-subtle">{t("dm_queue.hint")}</span>
      </div>
      <ul className="card divide-y divide-border">
        {items.map((it) => (
          <li key={it.promoter_id} className="px-4 py-3">
            <button
              className="flex w-full items-center gap-3 text-left"
              onClick={() => setOpen(open === it.promoter_id ? null : it.promoter_id)}
            >
              <div className="min-w-0 flex-1">
                <div className="truncate text-sm font-medium">{it.name}</div>
                <div className="truncate text-xs text-fg-subtle">
                  {[it.city, it.country].filter(Boolean).join(", ")}
                  {it.matched.length > 0 && ` · ${t("dm_queue.booked", { names: it.matched.join(", ") })}`}
                </div>
              </div>
              <IgHandle handle={it.handle} compact />
              <span className={clsx("text-xs", open === it.promoter_id ? "text-fg" : "text-accent-2")}>
                {open === it.promoter_id ? t("dm_queue.close") : t("dm_queue.write")}
              </span>
            </button>
            {open === it.promoter_id && (
              <DmPanel
                className="mt-3"
                target={{ promoterId: it.promoter_id }}
                handle={it.handle}
                onRecorded={() => {
                  setOpen(null);
                  qc.invalidateQueries({ queryKey: ["social_dm_queue"] });
                }}
              />
            )}
          </li>
        ))}
      </ul>
    </section>
  );
}
