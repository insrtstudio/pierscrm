import { useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { useNavigate } from "react-router-dom";
import { Plus, Music2, MapPin } from "lucide-react";
import { listArtists } from "../lib/api";
import type { Artist } from "../lib/types";
import { PageHeader, EmptyState } from "../components/Layout";
import { ArtistModal } from "./ArtistDetail";
import { useAppIntent, useArtistAvatar } from "../lib/hooks";

export function Artists() {
  const { t } = useTranslation();
  const qc = useQueryClient();
  const navigate = useNavigate();
  const { data: artists = [] } = useQuery({
    queryKey: ["artists"],
    queryFn: listArtists,
  });
  const [creating, setCreating] = useState(false);

  useAppIntent("new:artist", () => setCreating(true));

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
      <div className="px-page pb-10 pt-6">
        {artists.length === 0 ? (
          <div className="card">
            <EmptyState
              icon={Music2}
              title={t("artists.no_artists")}
              action={
                <button className="btn-primary" onClick={() => setCreating(true)}>
                  <Plus size={16} />
                  {t("artists.new")}
                </button>
              }
            />
          </div>
        ) : (
          <div className="grid grid-cols-[repeat(auto-fill,minmax(260px,1fr))] gap-4">
            {artists.map((a) => (
              <button
                key={a.id}
                onClick={() => navigate(`/artists/${a.id}`)}
                className="card group overflow-hidden text-left transition-transform hover:-translate-y-0.5 hover:shadow-card"
              >
                <div className="relative aspect-[4/3] w-full overflow-hidden bg-gradient-to-br from-accent/20 to-surface-2">
                  <ArtistCover artist={a} />
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

/// Card cover: the avatar is fetched per artist (not shipped by listArtists).
function ArtistCover({ artist }: { artist: Artist }) {
  const avatar = useArtistAvatar(artist.id);
  return avatar ? (
    <img src={avatar} alt={artist.name} className="h-full w-full object-cover" />
  ) : (
    <div className="flex h-full items-center justify-center text-4xl font-bold text-accent/40">
      {artist.name.charAt(0).toUpperCase()}
    </div>
  );
}
