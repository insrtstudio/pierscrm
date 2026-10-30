import { useEffect, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { Plus, RefreshCw, Target, Wand2, X } from "lucide-react";
import { listArtists } from "../lib/api";
import { backendError } from "../lib/backendMessages";
import {
  fitAddComparable,
  fitDeleteComparable,
  fitListComparables,
  fitRecompute,
  fitSeedComparables,
  fitSuggestComparables,
} from "../lib/targets";
import { useToast } from "./ui";

/// Settings › General: the comparable artists per artist of the roster, the
/// basis of the "books artists like ours" fit score in Discovery. Seeded on
/// demand from shared lineups (RA, our own gigs) and the artist's genres;
/// names can be typed in. "Recalculer" rescores every venue and promoter.
export function ComparablesSettingsCard() {
  const { t } = useTranslation();
  const toast = useToast();
  const qc = useQueryClient();
  const { data: artists = [] } = useQuery({ queryKey: ["artists"], queryFn: listArtists });
  const [artistId, setArtistId] = useState<number | null>(null);
  useEffect(() => {
    if (artistId == null && artists.length) setArtistId(artists[0].id ?? null);
  }, [artists, artistId]);
  const key = ["fit_comparables", artistId];
  const { data: list = [] } = useQuery({
    queryKey: key,
    queryFn: () => fitListComparables(artistId!),
    enabled: artistId != null,
  });
  const { data: suggestions = [] } = useQuery({
    queryKey: ["fit_suggestions", artistId],
    queryFn: () => fitSuggestComparables(artistId!),
    enabled: artistId != null,
  });
  const [name, setName] = useState("");
  const invalidate = () => {
    qc.invalidateQueries({ queryKey: ["fit_comparables"] });
    qc.invalidateQueries({ queryKey: ["fit_suggestions"] });
  };
  const add = useMutation({
    mutationFn: (n: string) => fitAddComparable(artistId!, n),
    onSuccess: () => {
      setName("");
      invalidate();
    },
    onError: (e) => toast(backendError(e), "error"),
  });
  const del = useMutation({ mutationFn: fitDeleteComparable, onSuccess: invalidate });
  const seed = useMutation({
    mutationFn: () => fitSeedComparables(artistId!),
    onSuccess: (n) => {
      invalidate();
      toast(t("fit.seeded", { count: n }), "ok");
    },
    onError: (e) => toast(backendError(e), "error"),
  });
  const recompute = useMutation({
    mutationFn: () => fitRecompute(artistId ?? undefined),
    onSuccess: (s) => {
      qc.invalidateQueries({ queryKey: ["vi_venues"] });
      qc.invalidateQueries({ queryKey: ["vi_promoters"] });
      toast(t("fit.recomputed", { venues: s.venues_scored, promoters: s.promoters_scored }), "ok");
    },
    onError: (e) => toast(backendError(e), "error"),
  });
  const colineups = suggestions.filter((s) => s.source === "colineup");

  return (
    <div className="card p-6">
      <div className="mb-1 flex items-center gap-2 text-sm font-semibold">
        <Target size={15} /> {t("fit.comparables_title")}
      </div>
      <p className="mb-4 text-xs text-fg-subtle">{t("fit.comparables_help")}</p>
      <div className="mb-3 flex flex-wrap items-center gap-2">
        <select
          className="input w-auto"
          value={artistId ?? ""}
          onChange={(e) => setArtistId(e.target.value ? Number(e.target.value) : null)}
        >
          {artists.map((a) => (
            <option key={a.id} value={a.id}>
              {a.name}
            </option>
          ))}
        </select>
        <form
          className="flex flex-1 gap-2"
          onSubmit={(e) => {
            e.preventDefault();
            if (name.trim() && artistId != null) add.mutate(name.trim());
          }}
        >
          <input
            className="input flex-1"
            placeholder={t("fit.add_placeholder")}
            value={name}
            onChange={(e) => setName(e.target.value)}
          />
          <button className="btn-outline" type="submit" disabled={!name.trim() || artistId == null}>
            <Plus size={14} /> {t("fit.add")}
          </button>
        </form>
      </div>
      {list.length === 0 ? (
        <p className="text-xs text-fg-faint">{t("fit.comparables_empty")}</p>
      ) : (
        <div className="flex flex-wrap gap-1.5">
          {list.map((c) => (
            <span key={c.id} className="badge border border-border text-fg" title={t(`fit.source_${c.source}`, c.source)}>
              {c.nom}
              <button className="text-fg-faint hover:text-danger" onClick={() => del.mutate(c.id)} aria-label={t("common.delete")}>
                <X size={11} />
              </button>
            </span>
          ))}
        </div>
      )}
      {colineups.length > 0 && (
        <div className="mt-3 text-2xs text-fg-subtle">
          {t("fit.colineup_found", {
            names: colineups
              .slice(0, 8)
              .map((s) => `${s.nom} (${s.count})`)
              .join(", "),
          })}
        </div>
      )}
      <div className="mt-4 flex flex-wrap justify-end gap-2">
        <button
          className="btn-outline"
          onClick={() => seed.mutate()}
          disabled={seed.isPending || artistId == null || suggestions.length === 0}
          title={t("fit.seed_hint")}
        >
          <Wand2 size={14} /> {t("fit.seed", { count: suggestions.length })}
        </button>
        <button className="btn-primary" onClick={() => recompute.mutate()} disabled={recompute.isPending}>
          <RefreshCw size={14} className={recompute.isPending ? "animate-spin" : ""} /> {t("fit.recompute")}
        </button>
      </div>
    </div>
  );
}
