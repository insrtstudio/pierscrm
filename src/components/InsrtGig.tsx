import { useEffect, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { Globe } from "lucide-react";
import clsx from "clsx";
import { getInsrtGig, setInsrtGig, type GigState } from "../lib/insrt";
import { formatDateTime } from "../lib/format";

export interface GigForm {
  publish: boolean;
  country: string;
  ticket_url: string;
}

/// The "publier sur insrt" part of an Agenda event: loaded state, local form
/// and whether it changed. Off by default: nothing goes public unless the user
/// ticks it. `defaultCountry` (the gig's inferred country) prefills the field.
export function useInsrtGig(eventId: number | null | undefined, defaultCountry?: string | null) {
  const { data: remote } = useQuery({
    queryKey: ["insrt_gig", eventId],
    queryFn: () => getInsrtGig(eventId!),
    enabled: eventId != null,
  });
  const initial = (r?: GigState): GigForm => ({
    publish: r?.publish ?? false,
    country: r?.country ?? defaultCountry ?? "",
    ticket_url: r?.ticket_url ?? "",
  });
  const [form, setForm] = useState<GigForm>(initial());
  const [base, setBase] = useState<GigForm>(initial());
  useEffect(() => {
    if (!remote) return;
    const v = initial(remote);
    setForm(v);
    setBase(v);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [remote]);
  // A gig never touched (no row) stays without a row while the switch is off.
  const dirty =
    form.publish !== base.publish ||
    (form.publish || remote?.publish || remote?.pushed_at != null
      ? form.country !== base.country || form.ticket_url !== base.ticket_url
      : false);
  /** Save after the event itself was saved (its id is known by then). */
  const commit = async (id: number): Promise<GigState | null> => {
    if (!dirty && !(remote && !remote.in_sync)) return null;
    return setInsrtGig(id, form);
  };
  return { form, setForm, remote, dirty, commit };
}

export function InsrtGigFields({
  gig,
  status,
}: {
  gig: ReturnType<typeof useInsrtGig>;
  status: string;
}) {
  const { t } = useTranslation();
  const { form, setForm, remote } = gig;
  const set = (k: keyof GigForm, v: string | boolean) => setForm((f) => ({ ...f, [k]: v }));
  return (
    <div className={clsx("border px-3 py-2.5", form.publish ? "border-accent/50 bg-accent-soft/30" : "border-border")}>
      <label className="flex cursor-pointer items-center gap-2 text-sm">
        <input
          type="checkbox"
          className="accent-accent"
          checked={form.publish}
          onChange={(e) => set("publish", e.target.checked)}
        />
        <Globe size={14} className="text-fg-subtle" />
        <span className="font-medium">{t("insrt.publish")}</span>
        <GigBadge remote={remote} />
      </label>
      <p className="mt-1 text-2xs text-fg-subtle">
        {form.publish && status !== "confirmed" ? t("insrt.publish_needs_confirmed") : t("insrt.publish_hint")}
      </p>
      {form.publish && (
        <div className="mt-2 grid grid-cols-[6rem_1fr] gap-2">
          <input
            className="input py-1.5 text-xs"
            placeholder={t("insrt.country")}
            value={form.country}
            maxLength={40}
            onChange={(e) => set("country", e.target.value)}
          />
          <input
            className="input py-1.5 text-xs"
            placeholder={t("insrt.ticket_url")}
            value={form.ticket_url}
            onChange={(e) => set("ticket_url", e.target.value)}
          />
        </div>
      )}
      {remote?.last_error && <p className="mt-1.5 text-2xs text-danger">{remote.last_error}</p>}
    </div>
  );
}

function GigBadge({ remote }: { remote?: GigState }) {
  const { t } = useTranslation();
  if (!remote || (!remote.live && !remote.pushed_at)) return null;
  if (!remote.in_sync)
    return <span className="badge ml-auto bg-warn/10 text-warn">{t("insrt.gig_pending")}</span>;
  return remote.live ? (
    <span className="badge ml-auto bg-accent-soft text-accent-ink" title={formatDateTime(remote.pushed_at)}>
      {t("insrt.gig_live")}
    </span>
  ) : (
    <span className="badge ml-auto bg-muted text-fg-subtle">{t("insrt.gig_private")}</span>
  );
}
