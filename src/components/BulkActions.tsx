// Selection-bar actions shared by the Pipeline and Discovery (venues, curators):
// campaign audience, organisation, and the "assign every contact without an
// artist" action. Each control is self-contained (its own queries, mutation
// and toast) so a page only drops it into its bar.
import { useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import {
  addToCampaign,
  assignUnassignedContactsArtist,
  countContactsWithoutArtist,
  listArtists,
  listCampaigns,
  listOrganisations,
  saveOrganisation,
  setContactsOrganisation,
} from "../lib/api";
import { useToast, useConfirm, errorMessage } from "./ui";

/// Add contacts to a campaign's planned audience (campaign_contacts). Pass the
/// contact ids directly, or `resolve` when they must be computed on demand
/// (Discovery rows are promoted into the pipeline first).
export function AddToCampaign({
  ids,
  resolve,
  onDone,
}: {
  ids?: number[];
  resolve?: () => Promise<number[]>;
  onDone?: () => void;
}) {
  const { t } = useTranslation();
  const toast = useToast();
  const qc = useQueryClient();
  const { data: campaigns = [] } = useQuery({ queryKey: ["campaigns"], queryFn: listCampaigns });
  const mut = useMutation({
    mutationFn: async (campaignId: number) => {
      const contactIds = resolve ? await resolve() : ids ?? [];
      return addToCampaign(campaignId, contactIds);
    },
    onSuccess: (n) => {
      qc.invalidateQueries({ queryKey: ["campaigns"] });
      onDone?.();
      toast(t("contacts.bulk_added_campaign", { count: n }), "ok");
    },
    onError: (e) => toast(errorMessage(e), "error"),
  });
  // Residency campaigns build their target on their own page.
  const open = campaigns.filter(
    (c) => c.status !== "archived" && c.status !== "done" && c.kind !== "residency"
  );
  if (open.length === 0) return null;
  return (
    <select
      className="input w-auto py-1.5 text-xs"
      value=""
      disabled={mut.isPending}
      aria-label={t("contacts.bulk_campaign")}
      onChange={(e) => e.target.value && mut.mutate(Number(e.target.value))}
    >
      <option value="">{mut.isPending ? t("common.loading") : t("contacts.bulk_campaign")}</option>
      {open.map((c) => (
        <option key={c.id} value={String(c.id)}>
          {c.name}
        </option>
      ))}
    </select>
  );
}

/// Put the selected contacts in one organisation (new or existing), or take
/// them out of theirs.
export function SetOrganisation({ ids, onDone }: { ids: number[]; onDone?: () => void }) {
  const { t } = useTranslation();
  const toast = useToast();
  const qc = useQueryClient();
  const { data: organisations = [] } = useQuery({
    queryKey: ["organisations"],
    queryFn: listOrganisations,
  });
  const mut = useMutation({
    mutationFn: async (v: string) => {
      let orgId: number | null = null;
      if (v === "__new") {
        const name = window.prompt(t("contacts.new_organisation_name"))?.trim();
        if (!name) return null;
        orgId = await saveOrganisation({ name });
      } else if (v !== "none") orgId = Number(v);
      return setContactsOrganisation(ids, orgId);
    },
    onSuccess: (n) => {
      if (n == null) return;
      qc.invalidateQueries({ queryKey: ["organisations"] });
      qc.invalidateQueries({ queryKey: ["contacts"] });
      onDone?.();
      toast(t("contacts.bulk_updated", { count: n }), "ok");
    },
    onError: (e) => toast(errorMessage(e), "error"),
  });
  return (
    <select
      className="input w-auto max-w-[200px] py-1.5 text-xs"
      value=""
      disabled={mut.isPending}
      aria-label={t("orgs.bulk_set")}
      onChange={(e) => e.target.value && mut.mutate(e.target.value)}
    >
      <option value="">{t("orgs.bulk_set")}</option>
      {organisations.map((o) => (
        <option key={o.id} value={String(o.id)}>
          {o.name}
        </option>
      ))}
      <option value="__new">{t("contacts.new_organisation")}</option>
      <option value="none">{t("orgs.bulk_detach")}</option>
    </select>
  );
}

/// "Assign the N contacts without an artist to…": explicit, confirmed, never
/// run at startup. Hidden once every contact has an artist.
export function AssignUnassignedArtist({ onDone }: { onDone?: () => void }) {
  const { t } = useTranslation();
  const toast = useToast();
  const confirm = useConfirm();
  const qc = useQueryClient();
  const [key, setKey] = useState(0);
  const { data: artists = [] } = useQuery({ queryKey: ["artists"], queryFn: listArtists });
  const { data: count = 0 } = useQuery({
    // Under "contacts": every contact edit refreshes it.
    queryKey: ["contacts", "without_artist"],
    queryFn: countContactsWithoutArtist,
  });
  const mut = useMutation({
    mutationFn: (artistId: number) => assignUnassignedContactsArtist(artistId),
    onSuccess: (n) => {
      qc.invalidateQueries({ queryKey: ["contacts"] });
      qc.invalidateQueries({ queryKey: ["artist"] });
      qc.invalidateQueries({ queryKey: ["dashboard"] });
      onDone?.();
      toast(t("contacts.bulk_updated", { count: n }), "ok");
    },
    onError: (e) => toast(errorMessage(e), "error"),
  });
  if (artists.length === 0 || count === 0) return null;
  return (
    <select
      key={key}
      className="input w-auto py-1.5 text-xs"
      value=""
      disabled={mut.isPending}
      title={t("contacts.assign_unassigned_hint")}
      aria-label={t("contacts.assign_unassigned", { count })}
      onChange={(e) => {
        const a = artists.find((x) => String(x.id) === e.target.value);
        setKey((k) => k + 1);
        if (a && confirm(t("contacts.assign_unassigned_confirm", { count, name: a.name })))
          mut.mutate(a.id!);
      }}
    >
      <option value="">{t("contacts.assign_unassigned", { count })}</option>
      {artists.map((a) => (
        <option key={a.id} value={String(a.id)}>
          {a.name}
        </option>
      ))}
    </select>
  );
}
