import { useEffect, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { useNavigate } from "react-router-dom";
import { Plus, Music2, MapPin } from "lucide-react";
import { listArtists } from "../lib/api";
import type { Artist } from "../lib/types";
import { PageHeader } from "../components/Layout";
import { ArtistModal } from "./ArtistDetail";

export function Artists() {
  const { t } = useTranslation();
  const qc = useQueryClient();
  const navigate = useNavigate();
  const { data: artists = [] } = useQuery({
    queryKey: ["artists"],
    queryFn: listArtists,
  });
  const [creating, setCreating] = useState(false);

  useEffect(() => {
    const h = (e: Event) => {
      if ((e as CustomEvent).detail === "new:artist") setCreating(true);
    };
    window.addEventListener("app:new", h);
    return () => window.removeEventListener("app:new", h);
  }, []);

  return (
    <div>
      <PageHeader
        title={t("artists.title")}
        subtitle={t("artists.subtitle")}
        actions={
          <button className="btn-primary" onClick={() => setCreating(true)}>
            <Plus size={16} />
            {t("artists.new")}
          </button>
        }
      />
      <div className="px-8 pb-10">
        {artists.length === 0 ? (
          <div className="card flex flex-col items-center gap-3 py-20 text-sm text-fg-subtle">
            <Music2 size={30} className="opacity-40" />
            {t("artists.no_artists")}
          </div>
        ) : (
          <div className="grid grid-cols-1 gap-4 sm:grid-cols-2 lg:grid-cols-3 xl:grid-cols-4">
            {artists.map((a) => (
              <button
                key={a.id}
                onClick={() => navigate(`/artists/${a.id}`)}
                className="card group overflow-hidden text-left transition-transform hover:-translate-y-0.5 hover:shadow-card"
              >
                <div className="relative aspect-[4/3] w-full overflow-hidden bg-gradient-to-br from-accent/20 to-surface-2">
                  {a.avatar ? (
                    <img
                      src={a.avatar}
                      alt={a.name}
                      className="h-full w-full object-cover"
                    />
                  ) : (
                    <div className="flex h-full items-center justify-center text-4xl font-bold text-accent/40">
                      {a.name.charAt(0).toUpperCase()}
                    </div>
                  )}
                </div>
                <div className="p-4">
                  <div className="text-sm font-semibold">{a.name}</div>
                  {a.tagline && (
                    <div className="mt-0.5 line-clamp-1 text-xs text-fg-subtle">
                      {a.tagline}
                    </div>
                  )}
                  <div className="mt-2 flex flex-wrap items-center gap-2 text-xs text-fg-subtle">
                    {a.genres && (
                      <span className="inline-flex items-center gap-1">
                        <Music2 size={12} /> {a.genres}
                      </span>
                    )}
                    {a.city && (
                      <span className="inline-flex items-center gap-1">
                        <MapPin size={12} /> {a.city}
                      </span>
                    )}
                  </div>
                </div>
              </button>
            ))}
          </div>
        )}
      </div>

      {creating && (
        <ArtistModal
          artist={{ name: "" } as Artist}
          onClose={() => setCreating(false)}
          onSaved={(id) => {
            setCreating(false);
            qc.invalidateQueries({ queryKey: ["artists"] });
            if (id) navigate(`/artists/${id}`);
          }}
        />
      )}
    </div>
  );
}
