import { useEffect, useMemo, useRef, useState } from "react";
import {
  keepPreviousData,
  useInfiniteQuery,
  useMutation,
  useQuery,
  useQueryClient,
} from "@tanstack/react-query";
import { useVirtualizer } from "@tanstack/react-virtual";
import { useTranslation } from "react-i18next";
import { useAppIntent, useDebounced } from "../lib/hooks";
import {
  Plus,
  Search,
  Mail,
  ExternalLink,
  Pencil,
  Trash2,
  Upload,
  Building2,
  X,
} from "lucide-react";
import { useNavigate, useSearchParams } from "react-router-dom";
import { openUrl } from "@tauri-apps/plugin-opener";
import {
  addToCampaign,
  assignContactsArtist,
  countContacts,
  createContact,
  deleteContact,
  deleteContacts,
  getContact,
  listArtists,
  listContacts,
  listEmails,
  listCampaigns,
  listEvents,
  listOrganisations,
  saveOrganisation,
  updateContact,
  updateContactStatus,
  updateContactsStatus,
} from "../lib/api";
import type { Contact, Status } from "../lib/types";
import { formatDate } from "../lib/format";
import { STATUSES } from "../lib/types";
import { CATEGORIES } from "../lib/constants";
import { PageHeader, Loader, ErrorState } from "../components/Layout";
import { StatusBadge, StatusSelect, PriorityBadge } from "../components/StatusBadge";
import { Modal, Field, useToast, useConfirm, useSubmit } from "../components/ui";
import { ComposeModal } from "../components/ComposeModal";

const PAGE_SIZE = 200;

export function Contacts() {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const qc = useQueryClient();
  const toast = useToast();
  const confirm = useConfirm();

  // Filters live in the URL so other pages (dashboard pipeline bars) can deep
  // link into a filtered Pipeline, and they survive a round trip to a contact.
  const [params, setParams] = useSearchParams();
  const [category, setCategory] = useState(() => params.get("category") || "all");
  const [status, setStatus] = useState(() => params.get("status") || "all");
  const [artist, setArtist] = useState(() => params.get("artist") || "all");
  const [sort, setSort] = useState(() => params.get("sort") || "priority");
  const [org, setOrg] = useState(() => params.get("org") || "all");
  const [search, setSearch] = useState("");
  useEffect(() => {
    const next: Record<string, string> = {};
    if (category !== "all") next.category = category;
    if (status !== "all") next.status = status;
    if (artist !== "all") next.artist = artist;
    if (org !== "all") next.org = org;
    if (sort !== "priority") next.sort = sort;
    setParams(next, { replace: true });
  }, [category, status, artist, org, sort]);
  const filtersActive =
    category !== "all" ||
    status !== "all" ||
    artist !== "all" ||
    org !== "all" ||
    search.trim() !== "";
  const resetFilters = () => {
    setCategory("all");
    setStatus("all");
    setArtist("all");
    setOrg("all");
    setSearch("");
  };
  const { data: artists = [] } = useQuery({ queryKey: ["artists"], queryFn: listArtists });
  const { data: organisations = [] } = useQuery({
    queryKey: ["organisations"],
    queryFn: listOrganisations,
  });
  const [selected, setSelected] = useState<Set<number>>(new Set());
  const [editing, setEditing] = useState<Contact | null>(null);
  const [composing, setComposing] = useState<Contact | null>(null);
  // Typing must not fire a query per keystroke.
  const dSearch = useDebounced(search, 250);

  // Rows are paged in on scroll (light rows, no notes), and the size of the
  // result comes from its own COUNT query: loading the 3 000 full rows on every
  // filter change was the slow part of this page.
  const filterArgs = {
    category,
    status,
    search: dSearch,
    artist_id: artist === "all" ? null : Number(artist),
    organisation_id: org === "all" ? null : Number(org),
  };
  const {
    data: pages,
    isLoading: contactsLoading,
    isError: contactsError,
    refetch: refetchContacts,
    fetchNextPage,
    hasNextPage,
    isFetchingNextPage,
  } = useInfiniteQuery({
    queryKey: ["contacts", "page", filterArgs, sort],
    queryFn: ({ pageParam }) =>
      listContacts({ ...filterArgs, sort, limit: PAGE_SIZE, offset: pageParam }),
    initialPageParam: 0,
    getNextPageParam: (last, all) =>
      last.length < PAGE_SIZE ? undefined : all.length * PAGE_SIZE,
    placeholderData: keepPreviousData,
  });
  const contacts = useMemo(() => pages?.pages.flat() ?? [], [pages]);
  const { data: total } = useQuery({
    queryKey: ["contacts", "count", filterArgs],
    queryFn: () => countContacts(filterArgs),
    placeholderData: keepPreviousData,
  });

  // Contact status feeds the follow-up queue, the dashboard and the Discovery
  // "contacted" flag: refresh all of them, not only this list.
  const invalidateAll = () => {
    qc.invalidateQueries({ queryKey: ["contacts"] });
    qc.invalidateQueries({ queryKey: ["dashboard"] });
    qc.invalidateQueries({ queryKey: ["followups"] });
    qc.invalidateQueries({ queryKey: ["vi_venues"] });
    qc.invalidateQueries({ queryKey: ["artist"] });
    qc.invalidateQueries({ queryKey: ["organisations"] });
  };

  // Render only the rows in view. The full pipeline is thousands of contacts and
  // each row carries an inline status <select>, so mounting them all made every
  // checkbox click re-render tens of thousands of nodes. Row height varies (the
  // promoter subline is optional), so rows are measured as they mount.
  const scrollRef = useRef<HTMLDivElement | null>(null);
  const rowVirtualizer = useVirtualizer({
    count: contacts.length,
    getScrollElement: () => scrollRef.current,
    estimateSize: () => 46,
    overscan: 12,
  });
  const virtualRows = rowVirtualizer.getVirtualItems();
  const padTop = virtualRows.length ? virtualRows[0].start : 0;
  const padBottom = virtualRows.length
    ? rowVirtualizer.getTotalSize() - virtualRows[virtualRows.length - 1].end
    : 0;

  // Pull the next page once the user scrolls near the end of what is loaded.
  const lastVisible = virtualRows.length ? virtualRows[virtualRows.length - 1].index : 0;
  useEffect(() => {
    if (
      contacts.length > 0 &&
      hasNextPage &&
      !isFetchingNextPage &&
      lastVisible >= contacts.length - 40
    )
      fetchNextPage();
  }, [lastVisible, contacts.length, hasNextPage, isFetchingNextPage, fetchNextPage]);
  useEffect(() => {
    if (scrollRef.current) scrollRef.current.scrollTop = 0;
  }, [category, status, dSearch, artist, org, sort]);

  // With a single artist, new contacts belong to it unless the user says otherwise.
  const soleArtist = artists.length === 1 ? artists[0].id ?? null : null;

  useAppIntent("new:contact", () =>
    setEditing({
      category: "venue",
      name: "",
      status: "to_contact",
      artist_id: soleArtist,
    } as Contact)
  );

  // A selection made under one filter must not survive into another: bulk
  // delete used to remove contacts that were no longer on screen.
  useEffect(() => setSelected(new Set()), [category, status, dSearch, artist, org]);
  const visibleSelected = contacts.filter((c) => selected.has(c.id!)).map((c) => c.id!);

  const statusMut = useMutation({
    mutationFn: ({ id, status }: { id: number; status: Status }) =>
      updateContactStatus(id, status),
    onSuccess: invalidateAll,
  });

  const delMut = useMutation({
    mutationFn: (id: number) => deleteContact(id),
    onSuccess: () => {
      invalidateAll();
      toast(t("common.delete"), "ok");
    },
  });

  const bulkDelMut = useMutation({
    mutationFn: (ids: number[]) => deleteContacts(ids),
    onSuccess: () => {
      invalidateAll();
      setSelected(new Set());
    },
  });
  const bulkStatusMut = useMutation({
    mutationFn: ({ ids, status }: { ids: number[]; status: string }) =>
      updateContactsStatus(ids, status),
    onSuccess: (n) => {
      invalidateAll();
      toast(t("contacts.bulk_updated", { count: n }), "ok");
    },
    onError: (e) => toast(String(e), "error"),
  });
  const bulkArtistMut = useMutation({
    mutationFn: ({ ids, artistId }: { ids: number[]; artistId: number | null }) =>
      assignContactsArtist(ids, artistId),
    onSuccess: (n) => {
      invalidateAll();
      toast(t("contacts.bulk_updated", { count: n }), "ok");
    },
    onError: (e) => toast(String(e), "error"),
  });

  const allChecked =
    contacts.length > 0 && contacts.every((c) => selected.has(c.id!));

  const toggleAll = () => {
    if (allChecked) setSelected(new Set());
    else setSelected(new Set(contacts.map((c) => c.id!)));
  };
  const toggleOne = (id: number) => {
    setSelected((s) => {
      const n = new Set(s);
      n.has(id) ? n.delete(id) : n.add(id);
      return n;
    });
  };

  return (
    <div>
      <PageHeader
        title={t("contacts.title")}
        subtitle={t("contacts.subtitle")}
        actions={
          <>
          <button className="btn-outline" onClick={() => navigate("/import")}>
            <Upload size={15} />
            {t("nav.import")}
          </button>
          <button
            className="btn-primary"
            onClick={() =>
              setEditing({
                category: category === "all" ? "venue" : category,
                name: "",
                status: status === "all" ? "to_contact" : status,
                artist_id: artist === "all" ? soleArtist : Number(artist),
                organisation_id: org === "all" ? null : Number(org),
              } as Contact)
            }
          >
            <Plus size={16} />
            {t("contacts.new")}
          </button>
          </>
        }
      />

      <div className="px-page pb-10 pt-6">
        {/* Filters */}
        <div className="mb-4 flex flex-wrap items-center gap-2">
          <div className="relative flex-1 min-w-[200px]">
            <Search
              size={15}
              className="pointer-events-none absolute left-3 top-1/2 -translate-y-1/2 text-fg-subtle"
            />
            <input
              className="input pl-9"
              placeholder={t("common.search")}
              value={search}
              onChange={(e) => setSearch(e.target.value)}
            />
          </div>
          <select
            className="input w-auto"
            value={category}
            onChange={(e) => setCategory(e.target.value)}
          >
            <option value="all">{t("category.all")}</option>
            {CATEGORIES.map((c) => (
              <option key={c} value={c}>
                {t(`category.${c}`)}
              </option>
            ))}
          </select>
          <select
            className="input w-auto"
            value={status}
            onChange={(e) => setStatus(e.target.value)}
          >
            <option value="all">{t("contacts.all_statuses")}</option>
            {STATUSES.map((s) => (
              <option key={s} value={s}>
                {t(`status.${s}`)}
              </option>
            ))}
          </select>
          {artists.length > 0 && (
            <select
              className="input w-auto"
              value={artist}
              onChange={(e) => setArtist(e.target.value)}
            >
              <option value="all">{t("contacts.all_artists")}</option>
              {artists.map((a) => (
                <option key={a.id} value={String(a.id)}>
                  {a.name}
                </option>
              ))}
            </select>
          )}
          {organisations.length > 0 && (
            <select
              className="input w-auto max-w-[220px]"
              value={org}
              onChange={(e) => setOrg(e.target.value)}
            >
              <option value="all">{t("contacts.all_organisations")}</option>
              {organisations.map((o) => (
                <option key={o.id} value={String(o.id)}>
                  {o.name} ({o.contact_count ?? 0})
                </option>
              ))}
            </select>
          )}
          <select
            className="input w-auto"
            value={sort}
            onChange={(e) => setSort(e.target.value)}
          >
            {["priority", "name", "recent", "created", "date", "area", "organisation"].map((k) => (
              <option key={k} value={k}>
                {t(`contacts.sort_${k}`)}
              </option>
            ))}
          </select>
        </div>

        {/* Bulk bar */}
        {visibleSelected.length > 0 && (
          <div className="mb-3 flex items-center gap-3 rounded-lg border border-border bg-surface px-4 py-2 text-sm">
            <span className="text-fg-subtle">
              {t("contacts.selected", { count: visibleSelected.length })}
            </span>
            <select
              className="input w-auto py-1.5 text-xs"
              value=""
              disabled={bulkStatusMut.isPending}
              aria-label={t("contacts.bulk_status")}
              onChange={(e) => {
                const s = e.target.value;
                if (s) bulkStatusMut.mutate({ ids: visibleSelected, status: s });
              }}
            >
              <option value="">{t("contacts.bulk_status")}</option>
              {STATUSES.map((s) => (
                <option key={s} value={s}>
                  {t(`status.${s}`)}
                </option>
              ))}
            </select>
            {artists.length > 0 && (
              <select
                className="input w-auto py-1.5 text-xs"
                value=""
                disabled={bulkArtistMut.isPending}
                aria-label={t("contacts.bulk_artist")}
                onChange={(e) => {
                  const v = e.target.value;
                  if (v)
                    bulkArtistMut.mutate({
                      ids: visibleSelected,
                      artistId: v === "none" ? null : Number(v),
                    });
                }}
              >
                <option value="">{t("contacts.bulk_artist")}</option>
                {artists.map((a) => (
                  <option key={a.id} value={String(a.id)}>
                    {a.name}
                  </option>
                ))}
                <option value="none">{t("contacts.no_artist")}</option>
              </select>
            )}
            <AddToCampaign ids={visibleSelected} onDone={invalidateAll} />
            <button
              className="btn-danger ml-auto py-1.5"
              disabled={bulkDelMut.isPending}
              onClick={() => {
                if (confirm(t("common.confirm_delete")))
                  bulkDelMut.mutate(visibleSelected);
              }}
            >
              <Trash2 size={14} />
              {t("contacts.delete_selected")}
            </button>
          </div>
        )}

        {/* Table */}
        <div className="card overflow-hidden">
          {/* Own scroll box so the row virtualiser has a viewport to measure. */}
          <div ref={scrollRef} className="max-h-[calc(100vh-330px)] overflow-auto">
            <table className="w-full text-sm">
              {/* WebKit ignores `position: sticky` on <tr>, so it goes on the cells. */}
              <thead className="[&_th]:sticky [&_th]:top-0 [&_th]:z-10 [&_th]:border-b [&_th]:border-border [&_th]:bg-surface [&_th]:font-mono [&_th]:text-[10.5px] [&_th]:font-normal [&_th]:uppercase [&_th]:tracking-[0.14em] [&_th]:text-fg-subtle">
                <tr className="text-left text-xs text-fg-subtle">
                  <th className="w-10 px-3 py-2.5">
                    <input
                      type="checkbox"
                      checked={allChecked}
                      onChange={toggleAll}
                      className="accent-accent"
                    />
                  </th>
                  <th className="px-3 py-2.5 ">{t("contacts.name")}</th>
                  <th className="px-3 py-2.5 ">{t("contacts.area")}</th>
                  <th className="px-3 py-2.5 ">{t("contacts.date")}</th>
                  <th className="px-3 py-2.5 ">{t("contacts.email")}</th>
                  <th className="px-3 py-2.5 ">{t("contacts.status")}</th>
                  <th className="px-3 py-2.5 text-right font-medium">
                    {t("common.actions")}
                  </th>
                </tr>
              </thead>
              <tbody>
                {contactsLoading && (
                  <tr>
                    <td colSpan={7}>
                      <Loader />
                    </td>
                  </tr>
                )}
                {contactsError && !contactsLoading && (
                  <tr>
                    <td colSpan={7}>
                      <ErrorState onRetry={() => refetchContacts()} />
                    </td>
                  </tr>
                )}
                {!contactsLoading && !contactsError && contacts.length === 0 && (
                  <tr>
                    <td
                      colSpan={7}
                      className="px-3 py-16 text-center text-sm text-fg-subtle"
                    >
                      {filtersActive ? (
                        <>
                          {t("contacts.no_match")}{" "}
                          <button className="text-accent hover:underline" onClick={resetFilters}>
                            {t("contacts.reset_filters")}
                          </button>
                        </>
                      ) : (
                        t("common.empty")
                      )}
                    </td>
                  </tr>
                )}
                {padTop > 0 && <tr style={{ height: padTop }} aria-hidden />}
                {virtualRows.map(({ index }) => {
                  const c = contacts[index];
                  // The window can briefly outrun a result set that just shrank.
                  if (!c) return null;
                  return (
                  <tr
                    key={c.id}
                    data-index={index}
                    ref={rowVirtualizer.measureElement}
                    className="group border-b border-border/60 transition-colors hover:bg-muted/50"
                  >
                    <td className="px-3 py-2.5">
                      <input
                        type="checkbox"
                        checked={selected.has(c.id!)}
                        onChange={() => toggleOne(c.id!)}
                        className="accent-accent"
                      />
                    </td>
                    <td className="px-3 py-2.5">
                      <button
                        className="flex items-center gap-2 text-left"
                        onClick={() => setEditing(c)}
                      >
                        <PriorityBadge priority={c.priority} />
                        <span className="font-medium">{c.name}</span>
                      </button>
                      {(c.promoter || c.organisation_name) && (
                        <div className="flex items-center gap-2 text-xs text-fg-subtle">
                          {c.promoter && <span className="truncate">{c.promoter}</span>}
                          {c.organisation_name && c.organisation_id != null && (
                            <button
                              className="badge shrink-0 border border-border py-0 hover:border-accent hover:text-accent"
                              title={t("contacts.filter_organisation")}
                              onClick={() => setOrg(String(c.organisation_id))}
                            >
                              <Building2 size={10} />
                              {c.organisation_name}
                            </button>
                          )}
                        </div>
                      )}
                    </td>
                    <td className="px-3 py-2.5 text-fg-subtle">
                      {c.area || c.venue || "·"}
                    </td>
                    <td className="whitespace-nowrap px-3 py-2.5 text-fg-subtle">
                      {formatDate(c.date)}
                    </td>
                    <td className="px-3 py-2.5">
                      {c.email ? (
                        <span className="text-xs">{c.email}</span>
                      ) : (
                        <span className="text-xs text-fg-subtle">·</span>
                      )}
                    </td>
                    <td className="px-3 py-2.5">
                      <StatusSelectInline
                        value={c.status}
                        onChange={(s) =>
                          statusMut.mutate({ id: c.id!, status: s })
                        }
                      />
                    </td>
                    <td className="px-3 py-2.5">
                      <div className="flex items-center justify-end gap-1 opacity-0 transition-opacity group-hover:opacity-100 group-focus-within:opacity-100">
                        {c.email && (
                          <button
                            title={t("contacts.compose")}
                            aria-label={t("contacts.compose")}
                            className="btn-ghost px-2 py-1.5"
                            onClick={() => setComposing(c)}
                          >
                            <Mail size={15} />
                          </button>
                        )}
                        {c.website && (
                          <button
                            title={t("contacts.open_site")}
                            aria-label={t("contacts.open_site")}
                            className="btn-ghost px-2 py-1.5"
                            onClick={() => {
                              const url = c.website!.startsWith("http")
                                ? c.website!
                                : `https://${c.website}`;
                              openUrl(url).catch(() => {});
                            }}
                          >
                            <ExternalLink size={15} />
                          </button>
                        )}
                        <button
                          title={t("common.edit")}
                            aria-label={t("common.edit")}
                          className="btn-ghost px-2 py-1.5"
                          onClick={() => setEditing(c)}
                        >
                          <Pencil size={15} />
                        </button>
                        <button
                          title={t("common.delete")}
                            aria-label={t("common.delete")}
                          className="btn-ghost px-2 py-1.5 text-danger"
                          onClick={() => {
                            if (confirm(t("common.confirm_delete")))
                              delMut.mutate(c.id!);
                          }}
                        >
                          <Trash2 size={15} />
                        </button>
                      </div>
                    </td>
                  </tr>
                  );
                })}
                {padBottom > 0 && <tr style={{ height: padBottom }} aria-hidden />}
              </tbody>
            </table>
          </div>
        </div>
        <div className="mt-3 text-xs text-fg-subtle">
          {total != null && total > contacts.length
            ? t("contacts.count_partial", { shown: contacts.length, count: total })
            : t("contacts.count", { count: total ?? contacts.length })}
        </div>
      </div>

      {editing && (
        <ContactEditor
          contact={editing}
          onClose={() => setEditing(null)}
          onSaved={() => {
            setEditing(null);
            invalidateAll();
          }}
        />
      )}
      <ComposeModal
        open={!!composing}
        contact={composing}
        onClose={() => setComposing(null)}
      />
    </div>
  );
}

function StatusSelectInline({
  value,
  onChange,
}: {
  value: string;
  onChange: (s: Status) => void;
}) {
  const [editing, setEditing] = useState(false);
  if (editing)
    return (
      <StatusSelect
        value={value}
        className="w-40 py-1 text-xs"
        onChange={(s) => {
          onChange(s);
          setEditing(false);
        }}
      />
    );
  return (
    <button onClick={() => setEditing(true)}>
      <StatusBadge status={value} />
    </button>
  );
}

// ---------------- Bulk: add to campaign ----------------

// Planned audience of a campaign (campaign_contacts).
function AddToCampaign({ ids, onDone }: { ids: number[]; onDone: () => void }) {
  const { t } = useTranslation();
  const toast = useToast();
  const { data: campaigns = [] } = useQuery({ queryKey: ["campaigns"], queryFn: listCampaigns });
  const qc = useQueryClient();
  const mut = useMutation({
    mutationFn: (campaignId: number) => addToCampaign(campaignId, ids),
    onSuccess: (n) => {
      qc.invalidateQueries({ queryKey: ["campaigns"] });
      onDone();
      toast(t("contacts.bulk_added_campaign", { count: typeof n === "number" ? n : ids.length }), "ok");
    },
    onError: (e) => toast(String(e), "error"),
  });
  const open = campaigns.filter((c) => c.status !== "archived" && c.status !== "done");
  if (open.length === 0) return null;
  return (
    <select
      className="input w-auto py-1.5 text-xs"
      value=""
      disabled={mut.isPending}
      aria-label={t("contacts.bulk_campaign")}
      onChange={(e) => e.target.value && mut.mutate(Number(e.target.value))}
    >
      <option value="">{t("contacts.bulk_campaign")}</option>
      {open.map((c) => (
        <option key={c.id} value={String(c.id)}>
          {c.name}
        </option>
      ))}
    </select>
  );
}

// ---------------- Contact editor ----------------

// List rows are light (no notes / reason): the editor works on the full row.
function ContactEditor(props: {
  contact: Contact;
  onClose: () => void;
  onSaved: () => void;
}) {
  const id = props.contact.id;
  const { data: full, isLoading } = useQuery({
    queryKey: ["contact", id],
    queryFn: () => getContact(id!),
    enabled: id != null,
    staleTime: 0,
  });
  if (id == null) return <ContactModal {...props} />;
  if (isLoading) return null;
  return <ContactModal {...props} contact={full ?? props.contact} />;
}

const isIsoDay = (v?: string | null) => !!v && /^\d{4}-\d{2}-\d{2}$/.test(v);

function ContactModal({
  contact,
  onClose,
  onSaved,
}: {
  contact: Contact;
  onClose: () => void;
  onSaved: () => void;
}) {
  const { t } = useTranslation();
  const toast = useToast();
  const [form, setForm] = useState<Contact>({ ...contact });
  const isNew = !contact.id;
  const { data: artists = [] } = useQuery({ queryKey: ["artists"], queryFn: listArtists });

  const set = (k: keyof Contact, v: any) =>
    setForm((f) => ({ ...f, [k]: v }));
  // Dates are ISO days edited with a date picker. A value that is still free
  // text (not migrated) stays editable as text instead of showing blank.
  const dateInp = (k: "date" | "first_contact" | "follow_up") => {
    const v = (form[k] as string | null | undefined) ?? "";
    return v && !isIsoDay(v)
      ? { className: "input", value: v, onChange: (e: any) => set(k, e.target.value) }
      : {
          type: "date",
          className: "input",
          value: v,
          onChange: (e: any) => set(k, e.target.value || null),
        };
  };
  const { data: organisations = [] } = useQuery({
    queryKey: ["organisations"],
    queryFn: listOrganisations,
  });
  const [newOrg, setNewOrg] = useState<string | null>(null);

  const [save, saving] = useSubmit(async () => {
    if (!form.name.trim()) {
      toast(t("contacts.name") + " " + t("common.required"), "error");
      return;
    }
    let payload = form;
    if (newOrg != null && newOrg.trim()) {
      const domain = form.email?.split("@")[1]?.trim().toLowerCase() || null;
      const orgId = await saveOrganisation({ name: newOrg.trim(), email_domain: domain });
      payload = { ...form, organisation_id: orgId };
    }
    if (isNew) await createContact(payload);
    else await updateContact(payload);
    toast(t("common.save"), "ok");
    onSaved();
  });

  const inp = (k: keyof Contact) => ({
    className: "input",
    value: (form[k] as string) ?? "",
    onChange: (e: any) => set(k, e.target.value),
  });

  return (
    <Modal
      open
      onClose={onClose}
      width="max-w-3xl"
      title={isNew ? t("contacts.new") : form.name || t("common.edit")}
    >
      <div className="grid grid-cols-2 gap-x-4 gap-y-3">
        <Field label={t("contacts.name")} className="col-span-2">
          <input {...inp("name")} />
        </Field>
        <Field label={t("contacts.category")}>
          <select
            className="input"
            value={form.category}
            onChange={(e) => set("category", e.target.value)}
          >
            {CATEGORIES.map((c) => (
              <option key={c} value={c}>
                {t(`category.${c}`)}
              </option>
            ))}
          </select>
        </Field>
        <Field label={t("contacts.priority")}>
          <input {...inp("priority")} placeholder="P1 / A / …" />
        </Field>
        {artists.length > 0 && (
          <Field label={t("contacts.artist")} className="col-span-2">
            <select
              className="input"
              value={form.artist_id ?? ""}
              onChange={(e) => set("artist_id", e.target.value ? Number(e.target.value) : null)}
            >
              <option value="">{t("contacts.no_artist")}</option>
              {artists.map((a) => (
                <option key={a.id} value={a.id}>
                  {a.name}
                </option>
              ))}
            </select>
          </Field>
        )}
        <Field label={t("contacts.organisation")} className="col-span-2">
          {newOrg != null ? (
            <div className="flex gap-2">
              <input
                className="input flex-1"
                autoFocus
                placeholder={t("contacts.new_organisation_name")}
                value={newOrg}
                onChange={(e) => setNewOrg(e.target.value)}
              />
              <button className="btn-ghost px-2" onClick={() => setNewOrg(null)} aria-label={t("common.cancel")}>
                <X size={15} />
              </button>
            </div>
          ) : (
            <select
              className="input"
              value={form.organisation_id ?? ""}
              onChange={(e) => {
                const v = e.target.value;
                if (v === "__new") setNewOrg(form.promoter ?? "");
                else set("organisation_id", v ? Number(v) : null);
              }}
            >
              <option value="">{t("contacts.no_organisation")}</option>
              {organisations.map((o) => (
                <option key={o.id} value={o.id}>
                  {o.name}
                </option>
              ))}
              <option value="__new">{t("contacts.new_organisation")}</option>
            </select>
          )}
        </Field>
        <Field label={t("contacts.promoter")}>
          <input {...inp("promoter")} />
        </Field>
        <Field label={t("contacts.venue")}>
          <input {...inp("venue")} />
        </Field>
        <Field label={t("contacts.type")}>
          <input {...inp("type")} />
        </Field>
        <Field label={t("contacts.area")}>
          <input {...inp("area")} />
        </Field>
        <Field label={t("contacts.date")}>
          <input {...dateInp("date")} />
        </Field>
        <Field label={t("contacts.time")}>
          <input {...inp("time")} />
        </Field>
        <Field label={t("contacts.format")}>
          <input {...inp("format")} />
        </Field>
        <Field label={t("contacts.scale")}>
          <input {...inp("scale")} />
        </Field>
        <Field label={t("contacts.email")}>
          <input {...inp("email")} />
        </Field>
        <Field label={t("contacts.email_status")}>
          <input {...inp("email_status")} />
        </Field>
        <Field label={t("contacts.status")}>
          <StatusSelect
            value={form.status}
            onChange={(s) => set("status", s)}
          />
        </Field>
        <Field label={t("contacts.website")}>
          <input {...inp("website")} />
        </Field>
        <Field label={t("contacts.first_contact")}>
          <input {...dateInp("first_contact")} />
        </Field>
        <Field label={t("contacts.follow_up")}>
          <input {...dateInp("follow_up")} />
        </Field>
        <Field label={t("contacts.contact_channel")} className="col-span-2">
          <input {...inp("contact_channel")} />
        </Field>
        <Field label={t("contacts.reason")} className="col-span-2">
          <textarea {...inp("reason")} className="input min-h-[70px]" />
        </Field>
        <Field label={t("contacts.notes")} className="col-span-2">
          <textarea {...inp("notes")} className="input min-h-[70px]" />
        </Field>
        <Field label={t("contacts.tags")} className="col-span-2">
          <input {...inp("tags")} placeholder="fit-a, noord, boat…" />
        </Field>
      </div>
      {form.organisation_id != null && newOrg == null && (
        <OrganisationSiblings orgId={form.organisation_id} selfId={contact.id} />
      )}
      {!isNew && <ContactHistory contactId={contact.id!} />}
      <div className="mt-5 flex justify-end gap-2">
        <button className="btn-outline" onClick={onClose}>
          {t("common.cancel")}
        </button>
        <button className="btn-primary" onClick={() => save()} disabled={saving}>
          {t("common.save")}
        </button>
      </div>
    </Modal>
  );
}

// Other contacts of the same organisation (one programmer, several venues).
function OrganisationSiblings({ orgId, selfId }: { orgId: number; selfId?: number }) {
  const { t } = useTranslation();
  const { data: rows = [] } = useQuery({
    queryKey: ["contacts", "organisation", orgId],
    queryFn: () => listContacts({ organisation_id: orgId, sort: "name", limit: 30 }),
  });
  const siblings = rows.filter((c) => c.id !== selfId);
  return (
    <div className="mt-5 border-t border-border pt-4">
      <div className="eyebrow mb-2 flex items-center gap-1.5 text-fg">
        <Building2 size={12} />
        {t("contacts.same_organisation")} ({siblings.length})
      </div>
      {siblings.length === 0 ? (
        <p className="text-xs text-fg-subtle">{t("contacts.same_organisation_none")}</p>
      ) : (
        <ul className="space-y-1 text-xs">
          {siblings.map((c) => (
            <li key={c.id} className="flex items-center gap-2">
              <span className="truncate font-medium">{c.name}</span>
              <span className="truncate text-fg-subtle">{c.area || c.venue}</span>
              <span className="ml-auto shrink-0">
                <StatusBadge status={c.status} />
              </span>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}

// What happened with this contact elsewhere in the app: emails sent to it
// (Outbox / campaigns) and Agenda events linked to it as a booking.
function ContactHistory({ contactId }: { contactId: number }) {
  const { t } = useTranslation();
  const { data: emails = [] } = useQuery({
    queryKey: ["emails", "contact", contactId],
    queryFn: () => listEmails(contactId),
  });
  const { data: events = [] } = useQuery({
    queryKey: ["events", "all"],
    queryFn: () => listEvents({}),
  });
  const linked = events.filter((e) => e.contact_id === contactId);
  return (
    <div className="mt-5 border-t border-border pt-4">
      <div className="eyebrow mb-2 text-fg">{t("contacts.history")}</div>
      {emails.length === 0 && linked.length === 0 ? (
        <p className="text-xs text-fg-subtle">{t("contacts.history_empty")}</p>
      ) : (
        <div className="grid grid-cols-1 gap-4 md:grid-cols-2">
          <div>
            <div className="mb-1 text-2xs uppercase tracking-wide text-fg-subtle">
              {t("contacts.history_emails")} ({emails.length})
            </div>
            <ul className="space-y-1 text-xs">
              {emails.slice(0, 8).map((e) => (
                <li key={e.id} className="flex items-baseline gap-2">
                  <span className="shrink-0 tabular text-fg-subtle">
                    {e.sent_at?.slice(0, 10)}
                  </span>
                  <span className="truncate">{e.subject}</span>
                  {e.status !== "sent" ? (
                    <span className="shrink-0 text-danger">{t("contacts.failed")}</span>
                  ) : e.opened_at ? (
                    <span className="shrink-0 text-ok">{t("contacts.opened")}</span>
                  ) : null}
                </li>
              ))}
            </ul>
          </div>
          <div>
            <div className="mb-1 text-2xs uppercase tracking-wide text-fg-subtle">
              {t("contacts.history_events")} ({linked.length})
            </div>
            <ul className="space-y-1 text-xs">
              {linked.map((e) => (
                <li key={e.id} className="flex items-baseline gap-2">
                  <span className="shrink-0 tabular text-fg-subtle">{formatDate(e.date)}</span>
                  <span className="truncate">{e.title}</span>
                  <span className="shrink-0 text-fg-subtle">{t(`agenda.st_${e.status}`)}</span>
                </li>
              ))}
            </ul>
          </div>
        </div>
      )}
    </div>
  );
}
