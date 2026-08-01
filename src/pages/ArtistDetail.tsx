import { useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { useNavigate, useParams } from "react-router-dom";
import { invoke } from "@tauri-apps/api/core";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { openUrl } from "@tauri-apps/plugin-opener";
import {
  ArrowLeft,
  Pencil,
  Mail,
  FileDown,
  Plus,
  Trash2,
  MapPin,
  Music2,
  Globe,
  Instagram,
  ExternalLink,
  ImagePlus,
} from "lucide-react";
import {
  deleteArtist,
  deleteKpi,
  getArtist,
  imageToDataUrl,
  listContacts,
  listKpis,
  saveArtist,
  saveKpi,
} from "../lib/api";
import type { Artist, Kpi } from "../lib/types";
import { Modal, Field, useToast, useConfirm } from "../components/ui";
import { Loader, ErrorState } from "../components/Layout";
import { ComposeModal } from "../components/ComposeModal";
import { StatusBadge } from "../components/StatusBadge";

const LINK_FIELDS: { key: keyof Artist; label: string }[] = [
  { key: "website", label: "Website" },
  { key: "instagram", label: "Instagram" },
  { key: "soundcloud", label: "SoundCloud" },
  { key: "spotify", label: "Spotify" },
  { key: "apple_music", label: "Apple Music" },
  { key: "beatport", label: "Beatport" },
  { key: "youtube", label: "YouTube" },
];

function normUrl(u: string) {
  return u.startsWith("http") ? u : `https://${u}`;
}

export function ArtistDetail() {
  const { t } = useTranslation();
  const { id } = useParams();
  const artistId = Number(id);
  const navigate = useNavigate();
  const qc = useQueryClient();
  const confirm = useConfirm();
  const toast = useToast();

  const exportPdf = async () => {
    toast(t("artists.pdf_hint"), "ok");
    // small delay so the toast paints before the modal print dialog blocks the UI
    setTimeout(async () => {
      try {
        await invoke("print_page");
      } catch {
        try {
          window.print();
        } catch {
          /* ignore */
        }
      }
    }, 150);
  };

  const {
    data: artist,
    isLoading: artistLoading,
    isError: artistError,
    refetch: refetchArtist,
  } = useQuery({
    queryKey: ["artist", artistId],
    queryFn: () => getArtist(artistId),
    enabled: !!artistId,
  });
  const { data: kpis = [] } = useQuery({
    queryKey: ["kpis", artistId],
    queryFn: () => listKpis(artistId),
    enabled: !!artistId,
  });
  const { data: bookings = [] } = useQuery({
    queryKey: ["contacts", "artist", artistId],
    queryFn: () => listContacts({ artist_id: artistId }),
    enabled: !!artistId,
  });

  const [editing, setEditing] = useState(false);
  const [composing, setComposing] = useState(false);
  const [kpiEdit, setKpiEdit] = useState<Kpi | null>(null);

  const delArtist = useMutation({
    mutationFn: () => deleteArtist(artistId),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ["artists"] });
      navigate("/artists");
    },
  });
  const delKpiMut = useMutation({
    mutationFn: (kid: number) => deleteKpi(kid),
    onSuccess: () => qc.invalidateQueries({ queryKey: ["kpis", artistId] }),
  });

  if (artistLoading) return <div className="p-8"><Loader /></div>;
  if (artistError) return <div className="p-8"><ErrorState onRetry={() => refetchArtist()} /></div>;
  if (!artist) return null;

  const links = LINK_FIELDS.filter((l) => artist[l.key]);
  const bookingContact = artist.booking_email || artist.email;

  return (
    <div>
      <div className="flex items-center gap-3 px-8 pb-2 pt-3">
        <button className="btn-ghost px-2 py-1.5" onClick={() => navigate("/artists")}>
          <ArrowLeft size={18} />
        </button>
        <PageHeaderInline title={artist.name} tagline={artist.tagline} />
        <div className="ml-auto flex items-center gap-2">
          {bookingContact && (
            <button className="btn-outline" onClick={() => setComposing(true)}>
              <Mail size={15} />
              {t("artists.write_email")}
            </button>
          )}
          <button className="btn-outline" onClick={exportPdf}>
            <FileDown size={15} />
            {t("artists.export_pdf")}
          </button>
          <button className="btn-primary" onClick={() => setEditing(true)}>
            <Pencil size={15} />
            {t("common.edit")}
          </button>
        </div>
      </div>

      <div className="grid grid-cols-1 gap-6 px-8 pb-10 lg:grid-cols-3">
        {/* EPK — printable one-pager */}
        <div className="lg:col-span-2">
          <div id="epk-print" className="epk card overflow-hidden">
            <div className="flex gap-6 border-b border-border p-6">
              <div className="h-32 w-32 shrink-0 overflow-hidden rounded-2xl bg-gradient-to-br from-accent/30 to-surface-2">
                {artist.avatar ? (
                  <img src={artist.avatar} alt={artist.name} className="h-full w-full object-cover" />
                ) : (
                  <div className="flex h-full items-center justify-center text-4xl font-bold text-accent/50">
                    {artist.name.charAt(0).toUpperCase()}
                  </div>
                )}
              </div>
              <div className="min-w-0 flex-1">
                <h1 className="text-2xl font-bold tracking-tight">{artist.name}</h1>
                {artist.real_name && (
                  <div className="text-sm text-fg-subtle">{artist.real_name}</div>
                )}
                {artist.tagline && (
                  <p className="mt-2 text-sm italic text-fg-subtle">
                    “{artist.tagline}”
                  </p>
                )}
                <div className="mt-3 flex flex-wrap gap-3 text-xs text-fg-subtle">
                  {artist.genres && (
                    <span className="inline-flex items-center gap-1">
                      <Music2 size={13} /> {artist.genres}
                    </span>
                  )}
                  {(artist.city || artist.country) && (
                    <span className="inline-flex items-center gap-1">
                      <MapPin size={13} />{" "}
                      {[artist.city, artist.country].filter(Boolean).join(", ")}
                    </span>
                  )}
                </div>
              </div>
            </div>

            <div className="space-y-5 p-6">
              {artist.mix_url && (
                <Section title={t("artists.featured_mix")}>
                  <a
                    href="#"
                    onClick={(e) => {
                      e.preventDefault();
                      openUrl(normUrl(artist.mix_url as string)).catch(() => {});
                    }}
                    className="epk-link inline-flex items-center gap-2 border-2 border-accent px-4 py-2.5 text-sm font-semibold hover:bg-accent hover:text-accent-fg"
                  >
                    <Music2 size={16} /> {t("artists.listen_now")}
                  </a>
                </Section>
              )}
              {artist.stats && (
                <Section title={t("artists.audience")}>
                  <p className="whitespace-pre-wrap text-sm leading-relaxed">{artist.stats}</p>
                </Section>
              )}
              {artist.bio && (
                <Section title={t("artists.bio")}>
                  <p className="whitespace-pre-wrap text-sm leading-relaxed">
                    {artist.bio}
                  </p>
                </Section>
              )}
              {artist.achievements && (
                <Section title={t("artists.achievements")}>
                  <p className="whitespace-pre-wrap text-sm leading-relaxed">
                    {artist.achievements}
                  </p>
                </Section>
              )}
              {artist.press_quotes && (
                <Section title={t("artists.press")}>
                  <p className="whitespace-pre-wrap border-l-2 border-accent pl-3 text-sm italic text-fg-subtle">
                    {artist.press_quotes}
                  </p>
                </Section>
              )}
              {links.length > 0 && (
                <Section title={t("artists.listen")}>
                  <div className="flex flex-wrap gap-2">
                    {links.map((l) => (
                      <a
                        key={l.key as string}
                        onClick={(e) => {
                          e.preventDefault();
                          openUrl(normUrl(artist[l.key] as string)).catch(() => {});
                        }}
                        href="#"
                        className="epk-link inline-flex items-center gap-1.5 rounded-lg border border-border px-3 py-1.5 text-xs font-medium hover:bg-muted"
                      >
                        {l.key === "instagram" ? (
                          <Instagram size={13} />
                        ) : l.key === "website" ? (
                          <Globe size={13} />
                        ) : (
                          <ExternalLink size={13} />
                        )}
                        {l.label}
                      </a>
                    ))}
                  </div>
                </Section>
              )}
              {artist.tech_rider && (
                <Section title={t("artists.tech_rider")}>
                  <p className="whitespace-pre-wrap text-sm leading-relaxed text-fg-subtle">
                    {artist.tech_rider}
                  </p>
                </Section>
              )}
              {(bookingContact || artist.fee_range) && (
                <Section title={t("artists.contact_booking")}>
                  <div className="text-sm">
                    {artist.booking_email && <div>{artist.booking_email}</div>}
                    {artist.phone && <div className="text-fg-subtle">{artist.phone}</div>}
                    {artist.fee_range && (
                      <div className="mt-1 text-fg-subtle">
                        {t("artists.fee_range")}: {artist.fee_range}
                      </div>
                    )}
                  </div>
                </Section>
              )}
            </div>
            <div className="flex items-center justify-between border-t border-border px-6 py-3 text-[10px] text-fg-subtle">
              <span>Insrt · EPK · {artist.name}</span>
              {bookingContact && <span>{bookingContact}</span>}
            </div>
          </div>
        </div>

        {/* Side: KPIs + bookings */}
        <div className="space-y-6">
          <div className="card p-5">
            <div className="mb-3 flex items-center justify-between">
              <h3 className="text-sm font-semibold">{t("artists.kpis")}</h3>
              <button
                className="btn-ghost px-2 py-1"
                onClick={() => setKpiEdit({ artist_id: artistId, sort: kpis.length } as Kpi)}
              >
                <Plus size={15} />
              </button>
            </div>
            {kpis.length === 0 ? (
              <p className="text-xs text-fg-subtle">{t("common.empty")}</p>
            ) : (
              <div className="space-y-2">
                {kpis.map((k) => (
                  <div
                    key={k.id}
                    className="group flex items-center justify-between rounded-lg border border-border px-3 py-2"
                  >
                    <div className="min-w-0">
                      <div className="truncate text-xs font-medium">{k.kpi}</div>
                      <div className="text-[11px] text-fg-subtle">{k.goal}</div>
                    </div>
                    <div className="flex items-center gap-2">
                      <div className="text-right">
                        <span className="text-sm font-bold text-emerald-500">
                          {k.actual || "·"}
                        </span>
                        <span className="text-[11px] text-fg-subtle">
                          {" "}
                          / {k.target || "·"}
                        </span>
                      </div>
                      <div className="flex gap-0.5 opacity-0 transition-opacity group-hover:opacity-100 group-focus-within:opacity-100">
                        <button
                          className="btn-ghost px-1 py-1"
                          onClick={() => setKpiEdit(k)}
                        >
                          <Pencil size={12} />
                        </button>
                        <button
                          className="btn-ghost px-1 py-1 text-rose-500"
                          aria-label={t("common.delete")}
                          onClick={() => confirm(t("common.confirm_delete")) && delKpiMut.mutate(k.id!)}
                        >
                          <Trash2 size={12} />
                        </button>
                      </div>
                    </div>
                  </div>
                ))}
              </div>
            )}
          </div>

          <div className="card p-5">
            <h3 className="mb-3 text-sm font-semibold">{t("artists.bookings")}</h3>
            {bookings.length === 0 ? (
              <p className="text-xs text-fg-subtle">{t("common.empty")}</p>
            ) : (
              <div className="space-y-2">
                {bookings.map((c) => (
                  <div
                    key={c.id}
                    className="flex items-center justify-between rounded-lg border border-border px-3 py-2"
                  >
                    <div className="min-w-0">
                      <div className="truncate text-xs font-medium">{c.name}</div>
                      {c.date && (
                        <div className="text-[11px] text-fg-subtle">{c.date}</div>
                      )}
                    </div>
                    <StatusBadge status={c.status} />
                  </div>
                ))}
              </div>
            )}
          </div>

          <button
            className="btn-danger w-full"
            onClick={() => {
              if (confirm(t("common.confirm_delete"))) delArtist.mutate();
            }}
          >
            <Trash2 size={15} />
            {t("common.delete")}
          </button>
        </div>
      </div>

      {editing && (
        <ArtistModal
          artist={artist}
          onClose={() => setEditing(false)}
          onSaved={() => {
            setEditing(false);
            qc.invalidateQueries({ queryKey: ["artist", artistId] });
            qc.invalidateQueries({ queryKey: ["artists"] });
          }}
        />
      )}
      <ComposeModal
        open={composing}
        onClose={() => setComposing(false)}
        contact={
          {
            id: null as any,
            name: artist.name,
            email: bookingContact,
            status: "to_contact",
            category: "other",
          } as any
        }
      />
      {kpiEdit && (
        <ArtistKpiModal
          kpi={kpiEdit}
          onClose={() => setKpiEdit(null)}
          onSaved={() => {
            setKpiEdit(null);
            qc.invalidateQueries({ queryKey: ["kpis", artistId] });
          }}
        />
      )}
    </div>
  );
}

function PageHeaderInline({
  title,
  tagline,
}: {
  title: string;
  tagline?: string | null;
}) {
  return (
    <div className="min-w-0">
      <h1 className="truncate text-xl font-semibold tracking-tight">{title}</h1>
      {tagline && <p className="truncate text-sm text-fg-subtle">{tagline}</p>}
    </div>
  );
}

function Section({
  title,
  children,
}: {
  title: string;
  children: React.ReactNode;
}) {
  return (
    <div>
      <div className="mb-1.5 text-xs font-semibold uppercase tracking-wide text-fg-subtle">
        {title}
      </div>
      {children}
    </div>
  );
}

// ---------------- Artist editor ----------------

export function ArtistModal({
  artist,
  onClose,
  onSaved,
}: {
  artist: Artist;
  onClose: () => void;
  onSaved: (id?: number) => void;
}) {
  const { t } = useTranslation();
  const toast = useToast();
  const [form, setForm] = useState<Artist>({ ...artist });
  const [uploading, setUploading] = useState(false);

  const set = (k: keyof Artist, v: any) => setForm((f) => ({ ...f, [k]: v }));
  const inp = (k: keyof Artist) => ({
    className: "input",
    value: (form[k] as string) ?? "",
    onChange: (e: any) => set(k, e.target.value),
  });

  const pickAvatar = async () => {
    const sel = await openDialog({
      multiple: false,
      filters: [{ name: "Image", extensions: ["png", "jpg", "jpeg", "webp", "gif"] }],
    });
    if (!sel || typeof sel !== "string") return;
    setUploading(true);
    try {
      const dataUrl = await imageToDataUrl(sel);
      set("avatar", dataUrl);
    } catch (e: any) {
      toast(e?.toString?.() ?? "error", "error");
    } finally {
      setUploading(false);
    }
  };

  const save = async () => {
    if (!form.name.trim()) {
      toast(t("artists.name") + " " + t("common.required"), "error");
      return;
    }
    try {
      const id = await saveArtist(form);
      toast(t("common.save"), "ok");
      onSaved(id);
    } catch (e: any) {
      toast(e?.toString?.() ?? "error", "error");
    }
  };

  return (
    <Modal
      open
      onClose={onClose}
      width="max-w-3xl"
      title={artist.id ? t("artists.edit") : t("artists.new")}
    >
      <div className="space-y-5">
        {/* Avatar + identity */}
        <div className="flex gap-5">
          <div className="shrink-0 text-center">
            <div className="h-28 w-28 overflow-hidden rounded-2xl bg-gradient-to-br from-accent/20 to-surface-2">
              {form.avatar ? (
                <img src={form.avatar} alt="" className="h-full w-full object-cover" />
              ) : (
                <div className="flex h-full items-center justify-center text-3xl font-bold text-accent/40">
                  {(form.name || "?").charAt(0).toUpperCase()}
                </div>
              )}
            </div>
            <button
              className="btn-ghost mt-2 w-full text-xs"
              onClick={pickAvatar}
              disabled={uploading}
            >
              <ImagePlus size={13} />
              {t("artists.upload_avatar")}
            </button>
          </div>
          <div className="grid flex-1 grid-cols-2 gap-x-4 gap-y-3">
            <Field label={t("artists.name")}>
              <input {...inp("name")} />
            </Field>
            <Field label={t("artists.real_name")}>
              <input {...inp("real_name")} />
            </Field>
            <Field label={t("artists.tagline")} className="col-span-2">
              <input {...inp("tagline")} />
            </Field>
            <Field label={t("artists.genres")}>
              <input {...inp("genres")} placeholder="Melodic techno, house…" />
            </Field>
            <Field label={t("artists.city")}>
              <input {...inp("city")} />
            </Field>
          </div>
        </div>

        <Field label={t("artists.bio")}>
          <textarea {...inp("bio")} className="input min-h-[90px]" />
        </Field>

        <div>
          <div className="mb-2 text-xs font-semibold uppercase tracking-wide text-fg-subtle">
            {t("artists.section_contact")}
          </div>
          <div className="grid grid-cols-2 gap-x-4 gap-y-3">
            <Field label={t("artists.booking_email")}>
              <input {...inp("booking_email")} />
            </Field>
            <Field label={t("artists.email")}>
              <input {...inp("email")} />
            </Field>
            <Field label={t("artists.phone")}>
              <input {...inp("phone")} />
            </Field>
            <Field label={t("artists.country")}>
              <input {...inp("country")} />
            </Field>
          </div>
        </div>

        <div>
          <div className="mb-2 text-xs font-semibold uppercase tracking-wide text-fg-subtle">
            {t("artists.section_links")}
          </div>
          <div className="grid grid-cols-2 gap-x-4 gap-y-3">
            {LINK_FIELDS.map((l) => (
              <Field key={l.key as string} label={l.label}>
                <input {...inp(l.key)} />
              </Field>
            ))}
          </div>
        </div>

        <div>
          <div className="mb-2 text-xs font-semibold uppercase tracking-wide text-fg-subtle">
            {t("artists.section_epk")}
          </div>
          <div className="grid grid-cols-2 gap-x-4 gap-y-3">
            <Field label={t("artists.featured_mix")} className="col-span-2">
              <input {...inp("mix_url")} placeholder="https://soundcloud.com/..." />
            </Field>
            <Field label={t("artists.audience")}>
              <input {...inp("stats")} placeholder="12k monthly, 8k IG..." />
            </Field>
            <Field label={t("artists.fee_range")}>
              <input {...inp("fee_range")} placeholder="300-600 EUR + all-in" />
            </Field>
            <Field label={t("artists.tech_rider")} className="col-span-2">
              <textarea {...inp("tech_rider")} className="input min-h-[60px]" placeholder="2x CDJ-3000, DJM-900..." />
            </Field>
            <Field label={t("artists.audience_cities")} className="col-span-2">
              <input {...inp("audience_cities")} placeholder="Paris, Amsterdam, Berlin, Lyon" />
            </Field>
            <Field label={t("artists.spotify_artist_id")} className="col-span-2">
              <input {...inp("spotify_artist_id")} placeholder="ID artiste Spotify (22 caractères) ou lien open.spotify.com/artist/..." />
            </Field>
          </div>
          <p className="mt-1.5 text-2xs text-fg-subtle">{t("artists.epk_hint")}</p>
        </div>

        <div>
          <div className="mb-2 text-xs font-semibold uppercase tracking-wide text-fg-subtle">
            {t("artists.section_press")}
          </div>
          <div className="grid grid-cols-1 gap-3">
            <Field label={t("artists.achievements")}>
              <textarea {...inp("achievements")} className="input min-h-[70px]" />
            </Field>
            <Field label={t("artists.press")}>
              <textarea {...inp("press_quotes")} className="input min-h-[70px]" />
            </Field>
          </div>
        </div>

        <div className="flex justify-end gap-2">
          <button className="btn-outline" onClick={onClose}>
            {t("common.cancel")}
          </button>
          <button className="btn-primary" onClick={save}>
            {t("common.save")}
          </button>
        </div>
      </div>
    </Modal>
  );
}

function ArtistKpiModal({
  kpi,
  onClose,
  onSaved,
}: {
  kpi: Kpi;
  onClose: () => void;
  onSaved: () => void;
}) {
  const { t } = useTranslation();
  const [form, setForm] = useState<Kpi>({ ...kpi });
  const inp = (k: keyof Kpi) => ({
    className: "input",
    value: (form[k] as string) ?? "",
    onChange: (e: any) => setForm({ ...form, [k]: e.target.value }),
  });
  const save = async () => {
    await saveKpi(form);
    onSaved();
  };
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
        <div className="flex justify-end gap-2">
          <button className="btn-outline" onClick={onClose}>
            {t("common.cancel")}
          </button>
          <button className="btn-primary" onClick={save}>
            {t("common.save")}
          </button>
        </div>
      </div>
    </Modal>
  );
}
