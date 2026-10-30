import { useEffect, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { ChevronDown, ChevronRight, Sparkles, UploadCloud } from "lucide-react";
import { today as todayDay } from "../lib/format";
import { getGigRecap, playedAtList, pushPlayedAt, saveGigRecap, type GigRecap as Recap } from "../lib/routing";
import { useConfirm, useSubmit, useToast } from "./ui";

/// "Après la date": the night's assets on a confirmed gig already played
/// (photo / video links, attendance note, a quote from the programmer), the
/// "a joué à" list it feeds, and the press kit update on insrt (only on click).
export function GigRecap({
  eventId,
  artistId,
  date,
  status,
}: {
  eventId?: number | null;
  artistId?: number | null;
  date: string;
  status: string;
}) {
  const { t, i18n } = useTranslation();
  const toast = useToast();
  const confirm = useConfirm();
  const qc = useQueryClient();
  const played = eventId != null && status === "confirmed" && !!date && date.slice(0, 10) < todayDay();
  const [open, setOpen] = useState(false);
  const { data: remote } = useQuery({
    queryKey: ["gig_recap", eventId],
    queryFn: () => getGigRecap(eventId!),
    enabled: played,
  });
  const { data: list = [] } = useQuery({
    queryKey: ["played_at", artistId ?? null],
    queryFn: () => playedAtList(artistId),
    enabled: played && open,
  });
  const [form, setForm] = useState<Recap | null>(null);
  useEffect(() => {
    if (remote) setForm(remote);
  }, [remote]);

  const [save, saving] = useSubmit(async () => {
    if (!form) return;
    const r = await saveGigRecap(form);
    setForm(r);
    qc.setQueryData(["gig_recap", eventId], r);
    qc.invalidateQueries({ queryKey: ["played_at"] });
    toast(t("routing.recap_saved"), "ok");
  });
  const [push, pushing] = useSubmit(async () => {
    if (!confirm(t("routing.push_confirm", { count: list.length }))) return;
    const r = await pushPlayedAt(artistId);
    toast(t(r.created ? "routing.push_created" : "routing.push_updated", { title: r.title, count: r.count }), "ok");
  });

  if (!played || !form) return null;
  const set = (k: keyof Recap, v: string | boolean) => setForm((f) => (f ? { ...f, [k]: v } : f));
  const fr = i18n.language.startsWith("fr");
  return (
    <div className="border border-border px-3 py-2.5">
      <button type="button" className="flex w-full items-center gap-2 text-left text-sm" onClick={() => setOpen((o) => !o)}>
        {open ? <ChevronDown size={14} /> : <ChevronRight size={14} />}
        <Sparkles size={14} className="text-fg-subtle" />
        <span className="font-medium">{t("routing.recap_title")}</span>
      </button>
      {open && (
        <div className="mt-2 space-y-2">
          <textarea
            className="input min-h-[52px] py-1.5 text-xs"
            placeholder={t("routing.recap_photos")}
            value={form.photos}
            onChange={(e) => set("photos", e.target.value)}
          />
          <textarea
            className="input min-h-[52px] py-1.5 text-xs"
            placeholder={t("routing.recap_videos")}
            value={form.videos}
            onChange={(e) => set("videos", e.target.value)}
          />
          <input
            className="input py-1.5 text-xs"
            placeholder={t("routing.recap_attendance")}
            value={form.attendance}
            onChange={(e) => set("attendance", e.target.value)}
          />
          <div className="grid grid-cols-[1fr_12rem] gap-2">
            <input
              className="input py-1.5 text-xs"
              placeholder={t("routing.recap_quote")}
              value={form.quote}
              onChange={(e) => set("quote", e.target.value)}
            />
            <input
              className="input py-1.5 text-xs"
              placeholder={t("routing.recap_quote_author")}
              value={form.quote_author}
              onChange={(e) => set("quote_author", e.target.value)}
            />
          </div>
          <label className="flex cursor-pointer items-center gap-2 text-xs">
            <input
              type="checkbox"
              className="accent-accent"
              checked={form.played_at}
              onChange={(e) => set("played_at", e.target.checked)}
            />
            {t("routing.recap_in_list")}
          </label>
          <div className="flex justify-end">
            <button type="button" className="btn-outline py-1 text-xs" onClick={save} disabled={saving}>
              {t("routing.recap_save")}
            </button>
          </div>
          <div className="border-t border-border pt-2">
            <div className="mb-1 text-2xs text-fg-subtle">{t("routing.played_at_title", { count: list.length })}</div>
            <ul className="max-h-28 overflow-y-auto text-2xs">
              {list.map((p) => (
                <li key={p.event_id} className="truncate">
                  {fr ? p.line_fr : p.line_en}
                </li>
              ))}
            </ul>
            <div className="mt-2 flex items-center gap-2">
              <p className="flex-1 text-2xs text-fg-faint">{t("routing.push_hint")}</p>
              <button
                type="button"
                className="btn-outline shrink-0 py-1 text-xs"
                onClick={push}
                disabled={pushing || list.length === 0}
              >
                <UploadCloud size={13} />
                {t("routing.push")}
              </button>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}
