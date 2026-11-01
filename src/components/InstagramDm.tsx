import { useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { openUrl } from "@tauri-apps/plugin-opener";
import clsx from "clsx";
import { Check, Copy, Instagram, MessageCircle, RefreshCw, Send } from "lucide-react";
import { backendError } from "../lib/backendMessages";
import { formatDate } from "../lib/format";
import {
  instagramDmUrl,
  instagramProfileUrl,
  listSocialTouches,
  socialDmDraft,
  socialRecordDm,
  cleanHandle,
  socialSetInstagram,
  viStartPromoterInstagram,
  type DmDraft,
  type DmTargetRef,
} from "../lib/social";
import { useToast } from "./ui";

/// "@handle" opening the profile in the default browser. Clicks do not reach
/// the row underneath (lists open a fiche on row click). `compact`: the icon
/// only, for fixed-height rows.
export function IgHandle({ handle, className, compact }: { handle?: string | null; className?: string; compact?: boolean }) {
  if (!handle) return null;
  return (
    <button
      type="button"
      className={clsx("inline-flex max-w-full items-center gap-1 truncate text-2xs text-fg-subtle hover:text-accent", className)}
      title={`@${handle}`}
      aria-label={`@${handle}`}
      onClick={(e) => {
        e.stopPropagation();
        openUrl(instagramProfileUrl(handle)).catch(() => {});
      }}
    >
      <Instagram size={compact ? 13 : 12} className="shrink-0" />
      {!compact && <span className="truncate">@{handle}</span>}
    </button>
  );
}

/// Collectifs toolbar: queue the Instagram search (the selection, else every
/// enriched collective without a handle). Progress shows in the runs tab.
export function InstagramSearchButton({ ids }: { ids: number[] }) {
  const { t } = useTranslation();
  const toast = useToast();
  const run = useMutation({
    mutationFn: () => viStartPromoterInstagram(ids.length ? { ids } : {}),
    onSuccess: () => toast(t("social.find_started"), "ok"),
    onError: (e) => toast(String(e) === "social.instagram_none" ? t("social.instagram_none") : backendError(e), "error"),
  });
  return (
    <button className="btn-outline" onClick={() => run.mutate()} disabled={run.isPending} title={t("social.find_hint")}>
      <Instagram size={15} />
      {ids.length ? t("social.find_selected", { count: ids.length }) : t("social.find")}
    </button>
  );
}

/// The assisted DM: a short draft in Thibault's voice (Claude, else a
/// template), editable, copied to the clipboard while Instagram opens on the
/// conversation; "DM envoyé" then records the touch (status, follow-up date).
/// Nothing is sent from the app. `handle` is the known one; a venue or
/// collective without one can be given one here.
export function DmPanel({
  target,
  handle,
  onRecorded,
  className,
}: {
  target: DmTargetRef;
  handle?: string | null;
  onRecorded?: () => void;
  className?: string;
}) {
  const { t } = useTranslation();
  const toast = useToast();
  const qc = useQueryClient();
  const [draft, setDraft] = useState<DmDraft | null>(null);
  const [body, setBody] = useState("");
  const [copied, setCopied] = useState(false);
  const [typed, setTyped] = useState("");
  const [ownHandle, setOwnHandle] = useState<string | null>(null);
  const h = cleanHandle(ownHandle ?? handle ?? draft?.handle);
  const key = ["social_touches", target.contactId ?? null, target.venueId ?? null, target.promoterId ?? null];
  const { data: touches = [] } = useQuery({ queryKey: key, queryFn: () => listSocialTouches(target) });

  const prepare = useMutation({
    mutationFn: () => socialDmDraft(target),
    onSuccess: (d) => {
      setDraft(d);
      setBody(d.body);
      setCopied(false);
    },
    onError: (e) => toast(backendError(e), "error"),
  });
  const saveHandle = useMutation({
    mutationFn: () =>
      socialSetInstagram({ venueId: target.venueId ?? undefined, promoterId: target.promoterId ?? undefined }, typed),
    onSuccess: (saved) => {
      setOwnHandle(saved);
      setTyped("");
      qc.invalidateQueries({ queryKey: ["vi_promoters"] });
      qc.invalidateQueries({ queryKey: ["vi_promoter"] });
      qc.invalidateQueries({ queryKey: ["vi_venues"] });
      qc.invalidateQueries({ queryKey: ["vi_venue_detail"] });
    },
    onError: (e) => toast(backendError(e), "error"),
  });
  const record = useMutation({
    mutationFn: () => socialRecordDm(target, h, body),
    onSuccess: () => {
      toast(t("social.recorded"), "ok");
      setDraft(null);
      setCopied(false);
      qc.invalidateQueries({ queryKey: ["social_touches"] });
      qc.invalidateQueries({ queryKey: ["contacts"] });
      qc.invalidateQueries({ queryKey: ["contact"] });
      qc.invalidateQueries({ queryKey: ["followups"] });
      qc.invalidateQueries({ queryKey: ["vi_promoters"] });
      qc.invalidateQueries({ queryKey: ["vi_promoter"] });
      qc.invalidateQueries({ queryKey: ["vi_venues"] });
      qc.invalidateQueries({ queryKey: ["vi_venue_detail"] });
      onRecorded?.();
    },
    onError: (e) => toast(backendError(e), "error"),
  });

  const copyAndOpen = async () => {
    if (!h) return;
    try {
      await navigator.clipboard.writeText(body);
      setCopied(true);
      toast(t("social.copied"), "ok");
    } catch {
      toast(t("social.copy_failed"), "error");
    }
    openUrl(instagramDmUrl(h)).catch(() => openUrl(instagramProfileUrl(h)).catch(() => {}));
  };

  const canSetHandle = !h && (target.venueId != null || target.promoterId != null);

  return (
    <section className={clsx("space-y-2", className)}>
      <div className="eyebrow flex items-center gap-1.5 text-fg">
        <MessageCircle size={12} />
        {t("social.dm_title")}
        {h && <IgHandle handle={h} className="ml-1 normal-case tracking-normal" />}
      </div>

      {touches.length > 0 && (
        <ul className="space-y-0.5 text-xs">
          {touches.slice(0, 5).map((s) => (
            <li key={s.id} className="flex items-baseline gap-2" title={s.body ?? undefined}>
              <Check size={12} className="shrink-0 text-ok" />
              <span>{t("social.sent_on", { date: formatDate(s.sent_at.slice(0, 10)) })}</span>
              {s.body && <span className="truncate text-fg-faint">{s.body}</span>}
            </li>
          ))}
        </ul>
      )}

      {!h ? (
        <div className="space-y-1.5">
          <p className="text-xs text-fg-faint">{t("social.no_handle")}</p>
          {canSetHandle && (
            <div className="flex gap-2">
              <input
                className="input max-w-[220px] py-1.5 text-xs"
                placeholder={t("social.set_handle")}
                value={typed}
                onChange={(e) => setTyped(e.target.value)}
              />
              <button
                className="btn-outline py-1.5 text-xs"
                onClick={() => saveHandle.mutate()}
                disabled={!typed.trim() || saveHandle.isPending}
              >
                {t("social.save_handle")}
              </button>
            </div>
          )}
        </div>
      ) : !draft ? (
        <div className="space-y-1">
          <button className="btn-outline py-1.5 text-xs" onClick={() => prepare.mutate()} disabled={prepare.isPending}>
            {prepare.isPending ? <RefreshCw size={13} className="animate-spin" /> : <Instagram size={13} />}
            {t("social.prepare")}
          </button>
          <p className="text-2xs text-fg-faint">{t("social.hint")}</p>
        </div>
      ) : (
        <div className="space-y-2">
          <textarea
            className="input min-h-[110px] text-sm"
            value={body}
            onChange={(e) => {
              setBody(e.target.value);
              setCopied(false);
            }}
          />
          <div className="flex flex-wrap items-center gap-2 text-2xs text-fg-faint">
            <span>
              {draft.generated_by === "template"
                ? draft.note
                  ? t("social.ai_failed", { note: draft.note })
                  : t("social.by_template")
                : t("social.by_ai")}
            </span>
            <span className="ml-auto tabular">{t("social.chars", { count: body.length })}</span>
          </div>
          <div className="flex flex-wrap items-center gap-2">
            <button className={copied ? "btn-outline py-1.5 text-xs" : "btn-primary py-1.5 text-xs"} onClick={copyAndOpen} disabled={!body.trim()}>
              <Copy size={13} />
              {t("social.copy_open")}
            </button>
            <button
              className={copied ? "btn-primary py-1.5 text-xs" : "btn-outline py-1.5 text-xs"}
              onClick={() => record.mutate()}
              disabled={!body.trim() || record.isPending}
            >
              <Send size={13} />
              {t("social.sent")}
            </button>
            <button className="btn-ghost py-1.5 text-xs" onClick={() => prepare.mutate()} disabled={prepare.isPending}>
              <RefreshCw size={13} className={prepare.isPending ? "animate-spin" : ""} />
              {t("social.regenerate")}
            </button>
            <button
              className="btn-ghost ml-auto py-1.5 text-xs"
              onClick={() => openUrl(instagramProfileUrl(h)).catch(() => {})}
            >
              {t("social.open_profile")}
            </button>
          </div>
        </div>
      )}
    </section>
  );
}
