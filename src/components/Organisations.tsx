// Organisations panel (audit item 9): the organisations above contacts, each
// with its contacts and Discovery venues, and the grouping proposals computed
// from shared addresses. A proposal changes nothing until it is accepted.
import { useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import clsx from "clsx";
import { Building2, Check, ChevronLeft, Sparkles, Trash2, X } from "lucide-react";
import {
  acceptOrganisationProposal,
  deleteOrganisation,
  dismissOrganisationProposal,
  getOrganisationDetail,
  listOrganisationProposals,
  listOrganisations,
  saveOrganisation,
  setContactsOrganisation,
} from "../lib/api";
import type { Organisation, OrgProposal } from "../lib/types";
import { Modal, useToast, useConfirm, errorMessage } from "./ui";
import { EmptyState } from "./Layout";
import { StatusBadge } from "./StatusBadge";

/// Everything that shows an organisation (Pipeline rows, filter, counts).
function useInvalidateOrgs() {
  const qc = useQueryClient();
  return () => {
    qc.invalidateQueries({ queryKey: ["organisations"] });
    qc.invalidateQueries({ queryKey: ["organisation"] });
    qc.invalidateQueries({ queryKey: ["organisation_proposals"] });
    qc.invalidateQueries({ queryKey: ["contacts"] });
  };
}

export function OrganisationsModal({
  onClose,
  onShowContacts,
}: {
  onClose: () => void;
  /** Filter the Pipeline on this organisation. */
  onShowContacts?: (id: number) => void;
}) {
  const { t } = useTranslation();
  const [tab, setTab] = useState<"list" | "proposals">("list");
  const [openId, setOpenId] = useState<number | null>(null);
  const { data: organisations = [] } = useQuery({
    queryKey: ["organisations"],
    queryFn: listOrganisations,
  });
  const { data: proposals = [], isLoading: proposalsLoading } = useQuery({
    queryKey: ["organisation_proposals"],
    queryFn: listOrganisationProposals,
  });
  const opened = organisations.find((o) => o.id === openId);

  return (
    <Modal open onClose={onClose} width="max-w-2xl" title={t("orgs.title")}>
      {opened ? (
        <OrganisationDetail
          org={opened}
          onBack={() => setOpenId(null)}
          onShowContacts={onShowContacts}
        />
      ) : (
        <>
          <div className="mb-4 flex gap-1 border-b border-border">
            {(["list", "proposals"] as const).map((k) => (
              <button
                key={k}
                className={clsx(
                  "-mb-px border-b-2 px-3 py-2 text-xs font-medium",
                  tab === k ? "border-accent text-fg" : "border-transparent text-fg-subtle hover:text-fg"
                )}
                onClick={() => setTab(k)}
              >
                {k === "list"
                  ? `${t("orgs.tab_list")} (${organisations.length})`
                  : `${t("orgs.tab_proposals")} (${proposals.length})`}
              </button>
            ))}
          </div>
          {tab === "list" ? (
            organisations.length === 0 ? (
              <EmptyState icon={Building2} title={t("orgs.empty")} hint={t("orgs.empty_hint")} />
            ) : (
              <ul className="max-h-[55vh] divide-y divide-border/60 overflow-y-auto rounded-lg border border-border">
                {organisations.map((o) => (
                  <li key={o.id}>
                    <button
                      className="flex w-full items-center gap-3 px-3 py-2 text-left hover:bg-muted"
                      onClick={() => setOpenId(o.id!)}
                    >
                      <Building2 size={13} className="shrink-0 text-fg-subtle" />
                      <span className="truncate text-sm font-medium">{o.name}</span>
                      {o.email_domain && (
                        <span className="truncate text-2xs text-fg-subtle">{o.email_domain}</span>
                      )}
                      <span className="ml-auto shrink-0 text-2xs tabular text-fg-subtle">
                        {t("contacts.count", { count: o.contact_count ?? 0 })}
                      </span>
                    </button>
                  </li>
                ))}
              </ul>
            )
          ) : (
            <ProposalList proposals={proposals} loading={proposalsLoading} />
          )}
        </>
      )}
    </Modal>
  );
}

function ProposalList({ proposals, loading }: { proposals: OrgProposal[]; loading: boolean }) {
  const { t } = useTranslation();
  if (!loading && proposals.length === 0)
    return <EmptyState icon={Sparkles} title={t("orgs.no_proposals")} hint={t("orgs.proposals_hint")} />;
  return (
    <div>
      <p className="mb-3 text-2xs text-fg-subtle">{t("orgs.proposals_hint")}</p>
      <ul className="max-h-[55vh] space-y-2 overflow-y-auto">
        {proposals.map((p) => (
          <ProposalRow key={p.key} p={p} />
        ))}
      </ul>
    </div>
  );
}

function ProposalRow({ p }: { p: OrgProposal }) {
  const { t } = useTranslation();
  const toast = useToast();
  const invalidate = useInvalidateOrgs();
  const [name, setName] = useState(p.name);
  const accept = useMutation({
    mutationFn: () =>
      acceptOrganisationProposal({
        name,
        email_domain: p.kind === "domain" ? p.email_domain : null,
        contact_ids: p.contact_ids,
        organisation_id: p.organisation_id ?? null,
      }),
    onSuccess: () => {
      invalidate();
      toast(t("orgs.accepted", { count: p.contact_ids.length }), "ok");
    },
    onError: (e) => toast(errorMessage(e), "error"),
  });
  const dismiss = useMutation({
    mutationFn: () => dismissOrganisationProposal(p.key),
    onSuccess: invalidate,
    onError: (e) => toast(errorMessage(e), "error"),
  });
  const busy = accept.isPending || dismiss.isPending;
  return (
    <li className="rounded-lg border border-border p-3">
      <div className="flex flex-wrap items-center gap-2">
        {p.organisation_id != null ? (
          <span className="text-sm font-medium">
            {t("orgs.join", { name: p.organisation_name ?? "" })}
          </span>
        ) : (
          <input
            className="input max-w-[220px] py-1 text-sm"
            value={name}
            aria-label={t("contacts.new_organisation_name")}
            onChange={(e) => setName(e.target.value)}
          />
        )}
        <span className="badge bg-muted text-fg-subtle">
          {p.kind === "domain" ? `@${p.email_domain}` : p.shared_email}
        </span>
        <div className="ml-auto flex gap-1">
          <button
            className="btn-primary px-2 py-1 text-xs"
            disabled={busy || (p.organisation_id == null && !name.trim())}
            onClick={() => accept.mutate()}
          >
            <Check size={13} />
            {t("orgs.accept")}
          </button>
          <button
            className="btn-ghost px-2 py-1 text-xs"
            disabled={busy}
            title={t("orgs.dismiss_hint")}
            onClick={() => dismiss.mutate()}
          >
            <X size={13} />
            {t("orgs.dismiss")}
          </button>
        </div>
      </div>
      <p className="mt-1.5 line-clamp-2 text-2xs text-fg-subtle">
        {t("contacts.count", { count: p.contact_ids.length })} : {p.contact_names.join(", ")}
      </p>
    </li>
  );
}

function OrganisationDetail({
  org,
  onBack,
  onShowContacts,
}: {
  org: Organisation;
  onBack: () => void;
  onShowContacts?: (id: number) => void;
}) {
  const { t } = useTranslation();
  const toast = useToast();
  const confirm = useConfirm();
  const invalidate = useInvalidateOrgs();
  const [form, setForm] = useState({
    name: org.name,
    email_domain: org.email_domain ?? "",
    notes: org.notes ?? "",
  });
  const { data } = useQuery({
    queryKey: ["organisation", org.id],
    queryFn: () => getOrganisationDetail(org.id!),
  });
  const save = useMutation({
    mutationFn: () =>
      saveOrganisation({
        id: org.id,
        name: form.name,
        email_domain: form.email_domain.trim() || null,
        notes: form.notes.trim() || null,
      }),
    onSuccess: () => {
      invalidate();
      toast(t("common.save"), "ok");
    },
    onError: (e) => toast(errorMessage(e), "error"),
  });
  const detach = useMutation({
    mutationFn: (contactId: number) => setContactsOrganisation([contactId], null),
    onSuccess: invalidate,
    onError: (e) => toast(errorMessage(e), "error"),
  });
  const del = useMutation({
    mutationFn: () => deleteOrganisation(org.id!),
    onSuccess: () => {
      invalidate();
      onBack();
    },
    onError: (e) => toast(errorMessage(e), "error"),
  });
  const contacts = data?.contacts ?? [];
  const venues = data?.venues ?? [];

  return (
    <div className="space-y-4">
      <button className="btn-ghost -ml-2 px-2 py-1 text-xs" onClick={onBack}>
        <ChevronLeft size={14} />
        {t("orgs.back")}
      </button>
      <div className="grid grid-cols-2 gap-3">
        <label className="block">
          <span className="label">{t("contacts.new_organisation_name")}</span>
          <input
            className="input"
            value={form.name}
            onChange={(e) => setForm({ ...form, name: e.target.value })}
          />
        </label>
        <label className="block">
          <span className="label">{t("orgs.domain")}</span>
          <input
            className="input"
            value={form.email_domain}
            placeholder="parissociety.com"
            onChange={(e) => setForm({ ...form, email_domain: e.target.value })}
          />
        </label>
        <label className="col-span-2 block">
          <span className="label">{t("contacts.notes")}</span>
          <textarea
            className="input min-h-[60px]"
            value={form.notes}
            onChange={(e) => setForm({ ...form, notes: e.target.value })}
          />
        </label>
      </div>
      <p className="text-2xs text-fg-subtle">{t("orgs.mailed_once")}</p>

      <div>
        <div className="eyebrow mb-2 flex items-center gap-2 text-fg">
          {t("orgs.contacts")} ({contacts.length})
          {onShowContacts && contacts.length > 0 && (
            <button
              className="ml-auto text-2xs normal-case tracking-normal text-accent hover:underline"
              onClick={() => onShowContacts(org.id!)}
            >
              {t("contacts.filter_organisation")}
            </button>
          )}
        </div>
        <ul className="max-h-[30vh] space-y-1 overflow-y-auto text-xs">
          {contacts.map((c) => (
            <li key={c.id} className="flex items-center gap-2">
              <span className="truncate font-medium">{c.name}</span>
              <span className="truncate text-fg-subtle">{c.email || c.area || c.venue}</span>
              <span className="ml-auto shrink-0">
                <StatusBadge status={c.status} />
              </span>
              <button
                className="btn-ghost shrink-0 px-1.5 py-1"
                title={t("orgs.detach")}
                aria-label={t("orgs.detach")}
                disabled={detach.isPending}
                onClick={() => detach.mutate(c.id!)}
              >
                <X size={12} />
              </button>
            </li>
          ))}
        </ul>
      </div>

      {venues.length > 0 && (
        <div>
          <div className="eyebrow mb-2 text-fg">
            {t("orgs.venues")} ({venues.length})
          </div>
          <ul className="max-h-[20vh] space-y-1 overflow-y-auto text-xs">
            {venues.map((v) => (
              <li key={v.id} className="flex items-center gap-2">
                <span className="truncate font-medium">{v.nom}</span>
                <span className="truncate text-fg-subtle">
                  {[v.ville, v.pays].filter(Boolean).join(", ")}
                </span>
              </li>
            ))}
          </ul>
        </div>
      )}

      <div className="flex items-center justify-between border-t border-border pt-4">
        <button
          className="btn-danger py-1.5"
          disabled={del.isPending}
          onClick={() => confirm(t("orgs.delete_confirm")) && del.mutate()}
        >
          <Trash2 size={14} />
          {t("common.delete")}
        </button>
        <button
          className="btn-primary"
          disabled={save.isPending || !form.name.trim()}
          onClick={() => save.mutate()}
        >
          {t("common.save")}
        </button>
      </div>
    </div>
  );
}
