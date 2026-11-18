import { useEffect, useMemo, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { openUrl } from "@tauri-apps/plugin-opener";
import { open as pickFile } from "@tauri-apps/plugin-dialog";
import { listen } from "@tauri-apps/api/event";
import clsx from "clsx";
import {
  AlertTriangle,
  Briefcase,
  Check,
  ExternalLink,
  Eye,
  FileText,
  FolderOpen,
  Mail,
  MailCheck,
  Plus,
  RefreshCw,
  Search,
  Send,
  ShieldCheck,
  Trash2,
  X,
} from "lucide-react";
import { EmptyState, ErrorState, Loader, PageHeader, Segmented, TabbedPage } from "../components/Layout";
import { Field, Modal, errorMessage, useConfirm, useToast } from "../components/ui";
import { formatDate } from "../lib/format";
import {
  JOB_STATUSES,
  jobDelete,
  jobDetail,
  jobMarkApplied,
  jobOpenCv,
  jobPrepare,
  jobSave,
  jobSaveDraft,
  jobSend,
  jobSendFollowup,
  jobSetStatus,
  jobsBoard,
  jobsCheckPdf,
  jobsConfig,
  jobsDeleteBoard,
  jobsDeleteSearch,
  jobsImportSheet,
  jobsRun,
  jobsSaveBoard,
  jobsSavePrefs,
  jobsSaveProfile,
  jobsSaveSearch,
  syncOpensQuiet,
  type AtsReport,
  type Job,
  type JobStatus,
  type JobsBoard,
  type JobsProgress,
} from "../lib/jobs";

type T = (k: string, o?: Record<string, unknown>) => string;

const today = () => new Date().toLocaleDateString("sv-SE");
const isSent = (j: Job) => j.app_state === "sent" || j.app_state === "manual";
const followupDue = (j: Job) =>
  isSent(j) &&
  !j.replied_at &&
  !j.followup_sent_at &&
  !["rejected", "closed", "dismissed", "interview", "offer"].includes(j.status) &&
  !!j.followup_due &&
  j.followup_due <= today();
const hostOf = (url: string) => url.replace(/^https?:\/\/(www\.)?/, "").split(/[/?#]/)[0];
const openExt = (url?: string | null) => url && openUrl(url).catch(() => {});

function useBoard() {
  return useQuery({ queryKey: ["jobs_board"], queryFn: jobsBoard });
}

/// Progress of the daily pass, shared by the tabs; refreshes the board when it ends.
function useRun() {
  const qc = useQueryClient();
  const toast = useToast();
  const { t } = useTranslation();
  const [progress, setProgress] = useState<JobsProgress | null>(null);
  useEffect(() => {
    const un = listen<JobsProgress>("jobs:progress", (e) => {
      if (e.payload.finished) {
        setProgress(null);
        qc.invalidateQueries({ queryKey: ["jobs_board"] });
        toast(t("jobs.run_done", { added: e.payload.added, prepared: e.payload.prepared }), "ok");
      } else {
        setProgress(e.payload);
      }
    });
    return () => {
      un.then((f) => f());
    };
  }, [qc, toast, t]);
  const start = () =>
    jobsRun()
      .then(() => setProgress({ stage: "collect", label: "", done: 0, total: 0, finished: false, found: 0, added: 0, analysed: 0, prepared: 0, notes: [] }))
      .catch((e) => toast(errorMessage(e), "error"));
  return { progress, start };
}

export function JobsSection() {
  const { t } = useTranslation();
  return (
    <TabbedPage
      title={t("nav.jobs")}
      subtitle={t("jobs.subtitle")}
      tabs={[
        { key: "board", label: t("jobs.tab_board"), render: () => <BoardTab /> },
        { key: "posts", label: t("jobs.tab_posts"), render: () => <PostsTab /> },
        { key: "settings", label: t("jobs.tab_settings"), render: () => <SettingsTab /> },
      ]}
    />
  );
}

// ---------------- Small pieces ----------------

function ScoreDot({ score }: { score: number | null }) {
  if (score == null) return <span className="w-9 shrink-0 text-center text-xs text-fg-faint">·</span>;
  return (
    <span
      className={clsx(
        "w-9 shrink-0 rounded-md py-0.5 text-center font-condensed text-base leading-none",
        score >= 75 ? "bg-ok/15 text-ok" : score >= 55 ? "bg-accent-2/15 text-accent-2" : "bg-fg/[0.07] text-fg-subtle"
      )}
    >
      {score}
    </span>
  );
}

function AtsBadge({ score, verdict, t }: { score: number | null; verdict?: string; t: T }) {
  if (score == null) return null;
  const v = verdict ?? (score >= 75 ? "pass" : score >= 55 ? "borderline" : "risk");
  return (
    <span
      className={clsx("badge", v === "pass" ? "bg-ok/15 text-ok" : v === "borderline" ? "bg-accent-2/15 text-accent-2" : "bg-danger/15 text-danger")}
      title={t("jobs.ats.hint")}
    >
      <ShieldCheck size={11} /> {t(`jobs.ats.${v}`)} {score}
    </span>
  );
}

function StatusBadge({ job: j, t }: { job: Job; t: T }) {
  if (j.replied_at) return <span className="badge bg-ok/15 text-ok">{t("jobs.state.replied")}</span>;
  if (isSent(j) && j.opened_at)
    return (
      <span className="badge bg-accent/15 text-accent" title={formatDate(j.opened_at)}>
        <Eye size={11} /> {t("jobs.state.opened", { count: j.open_count })}
      </span>
    );
  if (j.app_state === "draft" && !["dismissed", "closed"].includes(j.status))
    return <span className="badge bg-accent-2/15 text-accent-2">{t("jobs.state.to_validate")}</span>;
  return <span className="badge bg-fg/[0.07] text-fg-subtle">{t(`jobs.status.${j.status}`)}</span>;
}

/// The one line that says everything known about a posting.
function Condensed({ job: j, t }: { job: Job; t: T }) {
  const bits = [j.contract, j.salary].filter(Boolean).join(" · ");
  return (
    <div className="min-w-0 text-[12.5px] leading-snug text-fg-subtle">
      {j.line ? <span className="text-fg">{j.line}</span> : <span className="italic">{t("jobs.not_analysed")}</span>}
      {bits && <span> · {bits}</span>}
      {j.company_brief && (
        <span>
          {" "}
          · <span className="text-fg-subtle">{j.company_brief}</span>
        </span>
      )}
      {j.roster && (
        <span>
          {" "}
          · {t("jobs.roster")} : <span className="text-fg">{j.roster}</span>
        </span>
      )}
    </div>
  );
}

function Who({ job: j, t }: { job: Job; t: T }) {
  const email = j.apply_email ?? j.contact_email;
  return (
    <div className="min-w-0 text-[12.5px] leading-snug">
      {j.contact_name && (
        <div className="truncate">
          {j.contact_name}
          {j.contact_role && <span className="text-fg-subtle">, {j.contact_role}</span>}
        </div>
      )}
      {email ? (
        <div className="truncate text-fg-subtle">{email}</div>
      ) : (
        <div className="text-fg-faint">{j.apply_kind === "form" ? t("jobs.channel.form") : t("jobs.channel.none")}</div>
      )}
    </div>
  );
}

function JobLine({ job: j, onOpen, t }: { job: Job; onOpen: () => void; t: T }) {
  return (
    <button onClick={onOpen} className="flex w-full items-start gap-3 px-4 py-2.5 text-left hover:bg-fg/[0.03]">
      <ScoreDot score={j.score} />
      <div className="w-56 shrink-0">
        <div className="truncate text-sm font-medium">{j.title}</div>
        <div className="truncate text-[12.5px] text-fg-subtle">
          {[j.company, j.location || j.country].filter(Boolean).join(" · ")}
        </div>
      </div>
      <div className="min-w-0 flex-1">
        <Condensed job={j} t={t} />
      </div>
      <div className="hidden w-48 shrink-0 @3xl:block">
        <Who job={j} t={t} />
      </div>
      <div className="flex w-40 shrink-0 flex-col items-end gap-1">
        <StatusBadge job={j} t={t} />
        {j.app_state === "draft" && <AtsBadge score={j.ats_score} t={t} />}
        {j.next_action && isSent(j) && !j.replied_at && (
          <span className={clsx("text-right text-[11.5px]", followupDue(j) ? "text-accent-2" : "text-fg-faint")}>
            {j.next_action}
            {j.next_action_at ? ` (${formatDate(j.next_action_at)})` : ""}
          </span>
        )}
      </div>
    </button>
  );
}

// ---------------- Board ----------------

function Setup({ board: b, t }: { board: JobsBoard; t: T }) {
  const missing = [
    !b.profile_ready && t("jobs.setup.profile"),
    !b.ai_ready && t("jobs.setup.ai"),
    !b.sender && t("jobs.setup.sender"),
    !b.pdf_ready && t("jobs.setup.pdf"),
    !b.adzuna_ready && t("jobs.setup.adzuna"),
    !b.search_ready && t("jobs.setup.search"),
  ].filter(Boolean) as string[];
  if (!missing.length) return null;
  return (
    <div className="mb-4 rounded-xl border border-accent-2/40 bg-accent-2/[0.06] px-4 py-3 text-[13px]">
      <div className="mb-1 flex items-center gap-2 font-medium">
        <AlertTriangle size={14} className="text-accent-2" /> {t("jobs.setup.title")}
      </div>
      <ul className="list-disc space-y-0.5 pl-5 text-fg-subtle">
        {missing.map((m) => (
          <li key={m}>{m}</li>
        ))}
      </ul>
    </div>
  );
}

function Tile({ label, value, tone }: { label: string; value: number; tone?: string }) {
  return (
    <div className="card px-4 py-3">
      <div className={clsx("font-condensed text-3xl leading-none", tone)}>{value}</div>
      <div className="mt-1 text-[12px] text-fg-subtle">{label}</div>
    </div>
  );
}

function RunButton({ run, t }: { run: ReturnType<typeof useRun>; t: T }) {
  const p = run.progress;
  return (
    <button className="btn-primary" disabled={!!p} onClick={run.start} title={t("jobs.run_hint")}>
      <RefreshCw size={14} className={p ? "animate-spin" : undefined} />
      {p ? `${t(`jobs.stage.${p.stage}`)}${p.total ? ` ${p.done + 1}/${p.total}` : ""}` : t("jobs.run")}
    </button>
  );
}

function Section({ title, jobs, onOpen, t, empty }: { title: string; jobs: Job[]; onOpen: (id: number) => void; t: T; empty?: string }) {
  if (!jobs.length && !empty) return null;
  return (
    <div className="mb-5">
      <div className="kicker mb-2">
        {title} {jobs.length > 0 && <span className="text-fg-faint">· {jobs.length}</span>}
      </div>
      {jobs.length ? (
        <div className="card divide-y divide-border overflow-hidden">
          {jobs.map((j) => (
            <JobLine key={j.id} job={j} onOpen={() => onOpen(j.id)} t={t} />
          ))}
        </div>
      ) : (
        <div className="card px-4 py-4 text-[13px] text-fg-subtle">{empty}</div>
      )}
    </div>
  );
}

function BoardTab() {
  const { t } = useTranslation();
  const qc = useQueryClient();
  const { data: b, isLoading, isError, refetch } = useBoard();
  const run = useRun();
  const [openId, setOpenId] = useState<number | null>(null);
  const [adding, setAdding] = useState(false);
  useEffect(() => {
    syncOpensQuiet().then((n) => {
      if (n > 0) qc.invalidateQueries({ queryKey: ["jobs_board"] });
    });
  }, [qc]);
  if (isLoading) return <Loader />;
  if (isError || !b) return <ErrorState onRetry={refetch} />;
  const live = b.jobs.filter((j) => !["dismissed", "closed"].includes(j.status));
  const toValidate = live.filter((j) => j.app_state === "draft");
  const due = live.filter(followupDue);
  const talking = live.filter((j) => j.replied_at || ["interview", "offer"].includes(j.status));
  const waiting = live.filter((j) => isSent(j) && !j.replied_at && !followupDue(j) && !["interview", "offer", "rejected"].includes(j.status));
  const fresh = live
    .filter((j) => !j.app_state && j.status === "new" && j.score != null)
    .slice(0, 12);
  const sheet = live.filter((j) => !j.app_state && j.status === "shortlisted");
  return (
    <div className="px-page py-5">
      <PageHeader
        title={t("nav.jobs")}
        actions={
          <>
            <button className="btn-outline" onClick={() => setAdding(true)}>
              <Plus size={14} /> {t("jobs.add")}
            </button>
            <RunButton run={run} t={t} />
          </>
        }
      />
      <Setup board={b} t={t} />
      {run.progress && (
        <div className="mb-4 rounded-xl border border-border px-4 py-2.5 text-[13px] text-fg-subtle">
          {t(`jobs.stage.${run.progress.stage}`)}
          {run.progress.label ? ` · ${run.progress.label}` : ""}
        </div>
      )}
      <div className="mb-5 grid grid-cols-2 gap-3 @lg:grid-cols-3 @3xl:grid-cols-6">
        <Tile label={t("jobs.kpi.new_today")} value={b.new_today} />
        <Tile label={t("jobs.kpi.to_validate")} value={b.to_validate} tone={b.to_validate ? "text-accent-2" : undefined} />
        <Tile label={t("jobs.kpi.sent")} value={b.sent} />
        <Tile label={t("jobs.kpi.opened")} value={b.opened} />
        <Tile label={t("jobs.kpi.replies")} value={b.replies} tone={b.replies ? "text-ok" : undefined} />
        <Tile label={t("jobs.kpi.followups")} value={b.followups_due} tone={b.followups_due ? "text-accent-2" : undefined} />
      </div>
      <p className="mb-5 text-[12.5px] text-fg-subtle">
        {b.last_run ? t("jobs.last_run", { date: formatDate(b.last_run) }) : t("jobs.never_run")}
        {b.last_run_notes ? ` · ${b.last_run_notes}` : ""}
      </p>
      {!b.jobs.length ? (
        <EmptyState icon={Briefcase} title={t("jobs.empty")} hint={t("jobs.empty_hint")} />
      ) : (
        <>
          <Section title={t("jobs.sec.validate")} jobs={toValidate} onOpen={setOpenId} t={t} empty={t("jobs.sec.validate_empty")} />
          <Section title={t("jobs.sec.followups")} jobs={due} onOpen={setOpenId} t={t} />
          <Section title={t("jobs.sec.talking")} jobs={talking} onOpen={setOpenId} t={t} />
          <Section title={t("jobs.sec.sheet")} jobs={sheet} onOpen={setOpenId} t={t} />
          <Section title={t("jobs.sec.waiting")} jobs={waiting} onOpen={setOpenId} t={t} />
          <Section title={t("jobs.sec.fresh")} jobs={fresh} onOpen={setOpenId} t={t} />
        </>
      )}
      {openId != null && <JobModal id={openId} sender={b.sender} onClose={() => setOpenId(null)} />}
      {adding && <AddModal onClose={() => setAdding(false)} onSaved={(id) => setOpenId(id)} />}
    </div>
  );
}

// ---------------- Postings ----------------

type Filter = "all" | "new" | "todo" | "sent" | "out";

function PostsTab() {
  const { t } = useTranslation();
  const { data: b, isLoading, isError, refetch } = useBoard();
  const run = useRun();
  const [filter, setFilter] = useState<Filter>("all");
  const [country, setCountry] = useState("");
  const [domain, setDomain] = useState("");
  const [q, setQ] = useState("");
  const [openId, setOpenId] = useState<number | null>(null);
  const jobs = useMemo(() => {
    const needle = q.trim().toLowerCase();
    return (b?.jobs ?? []).filter((j) => {
      const out = ["dismissed", "closed", "rejected"].includes(j.status);
      if (filter === "out" ? !out : out && filter !== "all") return false;
      if (filter === "all" && out) return false;
      if (filter === "new" && (j.status !== "new" || j.app_state)) return false;
      if (filter === "todo" && !(j.app_state === "draft" || (j.status === "shortlisted" && !isSent(j)))) return false;
      if (filter === "sent" && !isSent(j)) return false;
      if (country && j.country !== country) return false;
      if (domain && j.domain !== domain) return false;
      if (needle && !`${j.title} ${j.company} ${j.location} ${j.line ?? ""} ${j.roster ?? ""}`.toLowerCase().includes(needle)) return false;
      return true;
    });
  }, [b, filter, country, domain, q]);
  if (isLoading) return <Loader />;
  if (isError || !b) return <ErrorState onRetry={refetch} />;
  const sel = "input w-auto py-1.5 text-xs";
  return (
    <div className="px-page py-5">
      <PageHeader title={t("jobs.tab_posts")} actions={<RunButton run={run} t={t} />} />
      <div className="mb-3 flex flex-wrap items-center gap-2">
        <Segmented
          items={(["all", "new", "todo", "sent", "out"] as Filter[]).map((k) => ({ key: k, label: t(`jobs.filter.${k}`) }))}
          value={filter}
          onChange={setFilter}
        />
        <select className={sel} value={country} onChange={(e) => setCountry(e.target.value)}>
          <option value="">{t("jobs.all_countries")}</option>
          {["FR", "UK", "US"].map((c) => (
            <option key={c} value={c}>
              {t(`jobs.country.${c}`)}
            </option>
          ))}
        </select>
        <select className={sel} value={domain} onChange={(e) => setDomain(e.target.value)}>
          <option value="">{t("jobs.all_domains")}</option>
          {["sync", "label", "booking"].map((d) => (
            <option key={d} value={d}>
              {t(`jobs.domain.${d}`)}
            </option>
          ))}
        </select>
        <div className="relative">
          <Search size={13} className="pointer-events-none absolute left-2.5 top-1/2 -translate-y-1/2 text-fg-faint" />
          <input className="input w-56 py-1.5 pl-8 text-xs" placeholder={t("jobs.search")} value={q} onChange={(e) => setQ(e.target.value)} />
        </div>
        <span className="text-xs text-fg-faint">{t("jobs.count", { count: jobs.length })}</span>
      </div>
      {jobs.length ? (
        <div className="card divide-y divide-border overflow-hidden">
          {jobs.map((j) => (
            <JobLine key={j.id} job={j} onOpen={() => setOpenId(j.id)} t={t} />
          ))}
        </div>
      ) : (
        <EmptyState icon={Briefcase} title={t("jobs.none")} hint={t("jobs.empty_hint")} />
      )}
      {openId != null && <JobModal id={openId} sender={b.sender} onClose={() => setOpenId(null)} />}
    </div>
  );
}

// ---------------- One posting ----------------

function AtsPanel({ report: r, t }: { report: AtsReport; t: T }) {
  const failed = r.checks.filter((c) => !c.ok);
  return (
    <div className="rounded-xl border border-border px-3 py-2.5 text-[12.5px]">
      <div className="flex flex-wrap items-center gap-2">
        <AtsBadge score={r.score} verdict={r.verdict} t={t} />
        <span className="text-fg-subtle">{t("jobs.ats.coverage", { pct: r.coverage })}</span>
      </div>
      {failed.length > 0 ? (
        <ul className="mt-2 space-y-0.5">
          {failed.map((c) => (
            <li key={c.key} className="flex gap-1.5 text-danger">
              <X size={13} className="mt-0.5 shrink-0" /> {c.detail}
            </li>
          ))}
        </ul>
      ) : (
        <p className="mt-2 flex gap-1.5 text-fg-subtle">
          <Check size={13} className="mt-0.5 shrink-0 text-ok" /> {t("jobs.ats.readable")}
        </p>
      )}
      {r.keywords_found.length > 0 && (
        <p className="mt-2">
          <span className="text-fg-subtle">{t("jobs.ats.found")} : </span>
          {r.keywords_found.join(", ")}
        </p>
      )}
      {r.keywords_addable.length > 0 && (
        <p className="mt-1">
          <span className="text-accent-2">{t("jobs.ats.addable")} : </span>
          {r.keywords_addable.join(", ")}
        </p>
      )}
      {r.keywords_missing.length > 0 && (
        <p className="mt-1">
          <span className="text-fg-subtle">{t("jobs.ats.missing")} : </span>
          {r.keywords_missing.join(", ")}
        </p>
      )}
      <p className="mt-2 text-[11.5px] text-fg-faint">{t("jobs.ats.hint")}</p>
    </div>
  );
}

function Row({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div className="flex gap-3 py-1.5 text-[13px]">
      <span className="w-28 shrink-0 text-fg-subtle">{label}</span>
      <div className="min-w-0 flex-1">{children}</div>
    </div>
  );
}

function JobModal({ id, sender, onClose }: { id: number; sender: string | null; onClose: () => void }) {
  const { t } = useTranslation();
  const toast = useToast();
  const confirm = useConfirm();
  const qc = useQueryClient();
  const { data: d, refetch } = useQuery({ queryKey: ["job_detail", id], queryFn: () => jobDetail(id) });
  const [busy, setBusy] = useState<string | null>(null);
  const [to, setTo] = useState("");
  const [subject, setSubject] = useState("");
  const [body, setBody] = useState("");
  const [followup, setFollowup] = useState("");
  const [notes, setNotes] = useState("");
  const [showDesc, setShowDesc] = useState(false);
  const app = d?.application ?? null;
  useEffect(() => {
    if (!d) return;
    setTo(d.job.apply_email ?? d.job.contact_email ?? "");
    setSubject(d.application?.subject ?? "");
    setBody(d.application?.body ?? "");
    setFollowup(d.application?.followup_body ?? "");
    setNotes(d.job.notes ?? "");
  }, [d]);
  const refresh = () => {
    refetch();
    qc.invalidateQueries({ queryKey: ["jobs_board"] });
  };
  const act = async (key: string, fn: () => Promise<unknown>, done?: string) => {
    setBusy(key);
    try {
      const res: any = await fn();
      if (res && typeof res === "object" && "ok" in res && !res.ok) {
        toast(res.error ?? t("jobs.send_failed"), "error");
      } else if (done) {
        toast(done, "ok");
      }
      refresh();
    } catch (e) {
      toast(errorMessage(e), "error");
    } finally {
      setBusy(null);
    }
  };
  if (!d) {
    return (
      <Modal open onClose={onClose} title="…" width="max-w-4xl">
        <Loader />
      </Modal>
    );
  }
  const j = d.job;
  const sent = isSent(j);
  const saveNotes = () =>
    notes !== (j.notes ?? "") &&
    jobSave({ id: j.id, title: j.title, company: j.company, location: j.location, country: j.country, url: j.url, contract: j.contract, salary: j.salary, apply_email: j.apply_email, contact_name: j.contact_name, contact_email: j.contact_email, next_action: j.next_action, next_action_at: j.next_action_at, notes }).then(refresh);
  return (
    <Modal open onClose={onClose} title={j.title} width="max-w-4xl">
      <div className="flex flex-wrap items-center gap-x-3 gap-y-1 text-[12.5px] text-fg-subtle">
        <span className="text-fg">{j.company || t("jobs.no_company")}</span>
        {j.location && <span>{j.location}</span>}
        {j.contract && <span>{j.contract}</span>}
        {j.salary && <span>{j.salary}</span>}
        {j.posted_at && <span>{t("jobs.posted", { date: formatDate(j.posted_at) })}</span>}
        <ScoreDot score={j.score} />
        <StatusBadge job={j} t={t} />
        {j.url && (
          <button className="badge bg-fg/[0.08] text-fg hover:bg-fg/15" onClick={() => openExt(j.url)}>
            {t("jobs.open_posting")} <ExternalLink size={10} />
          </button>
        )}
      </div>
      {j.line && <p className="mt-3 text-sm leading-relaxed">{j.line}</p>}

      <div className="mt-4 grid gap-5 lg:grid-cols-2">
        <div>
          <div className="rounded-xl border border-border px-3 py-1">
            <Row label={t("jobs.card.company")}>
              {j.company_brief ?? <span className="text-fg-faint">{t("jobs.card.unknown")}</span>}
              {j.company_site && (
                <button className="ml-2 underline" onClick={() => openExt(j.company_site)}>
                  {hostOf(j.company_site)}
                </button>
              )}
            </Row>
            {j.roster && <Row label={t("jobs.roster")}>{j.roster}</Row>}
            <Row label={t("jobs.card.contact")}>
              {j.contact_name || j.contact_email || j.apply_email ? (
                <>
                  {j.contact_name && (
                    <div>
                      {j.contact_name}
                      {j.contact_role ? `, ${j.contact_role}` : ""}
                    </div>
                  )}
                  {(j.apply_email || j.contact_email) && <div>{j.apply_email ?? j.contact_email}</div>}
                  {j.contact_source && (
                    <div className="text-[12px] text-fg-subtle">
                      {t("jobs.card.read_on")}{" "}
                      {j.contact_source.startsWith("http") ? (
                        <button className="underline" onClick={() => openExt(j.contact_source)}>
                          {hostOf(j.contact_source)}
                        </button>
                      ) : (
                        j.contact_source
                      )}
                    </div>
                  )}
                </>
              ) : (
                <span className="text-fg-faint">{t("jobs.card.no_contact")}</span>
              )}
            </Row>
            <Row label={t("jobs.card.apply")}>
              {j.apply_kind === "email" ? t("jobs.channel.email") : j.apply_kind === "form" ? t("jobs.channel.form") : t("jobs.channel.none")}
            </Row>
            {d.fit?.work_right && <Row label={t("jobs.card.work_right")}>{d.fit.work_right}</Row>}
            {d.fit?.deadline && <Row label={t("jobs.card.deadline")}>{d.fit.deadline}</Row>}
          </div>
          {d.fit && (d.fit.strengths.length > 0 || d.fit.gaps.length > 0) && (
            <div className="mt-3 grid gap-3 text-[13px] md:grid-cols-2">
              <div>
                <div className="kicker mb-1">{t("jobs.card.strengths")}</div>
                <ul className="list-disc space-y-0.5 pl-4">
                  {d.fit.strengths.map((s) => (
                    <li key={s}>{s}</li>
                  ))}
                </ul>
              </div>
              <div>
                <div className="kicker mb-1">{t("jobs.card.gaps")}</div>
                <ul className="list-disc space-y-0.5 pl-4 text-fg-subtle">
                  {d.fit.gaps.map((s) => (
                    <li key={s}>{s}</li>
                  ))}
                </ul>
              </div>
            </div>
          )}
          <Field label={t("jobs.card.notes")} className="mt-3">
            <textarea className="input min-h-[64px]" value={notes} onChange={(e) => setNotes(e.target.value)} onBlur={saveNotes} />
          </Field>
          <div className="mt-2 flex flex-wrap items-center gap-2">
            <select
              className="input w-auto py-1.5 text-xs"
              value={j.status}
              onChange={(e) => act("status", () => jobSetStatus(j.id, e.target.value as JobStatus))}
            >
              {JOB_STATUSES.map((s) => (
                <option key={s} value={s}>
                  {t(`jobs.status.${s}`)}
                </option>
              ))}
            </select>
            {d.description && (
              <button className="btn-ghost text-xs" onClick={() => setShowDesc((v) => !v)}>
                <FileText size={13} /> {showDesc ? t("jobs.hide_desc") : t("jobs.show_desc")}
              </button>
            )}
            <button
              className="btn-ghost text-xs text-danger"
              onClick={() => confirm(t("jobs.delete_confirm")) && jobDelete(j.id).then(() => (qc.invalidateQueries({ queryKey: ["jobs_board"] }), onClose()))}
            >
              <Trash2 size={13} /> {t("common.delete")}
            </button>
          </div>
          {showDesc && <pre className="mt-2 max-h-72 overflow-auto whitespace-pre-wrap rounded-xl border border-border p-3 text-[12px] text-fg-subtle">{d.description}</pre>}
        </div>

        <div>
          {!app ? (
            <div className="rounded-xl border border-dashed border-border px-4 py-6 text-center">
              <p className="mb-3 text-[13px] text-fg-subtle">{t("jobs.app.none")}</p>
              <button className="btn-primary" disabled={!!busy} onClick={() => act("prepare", () => jobPrepare(j.id), t("jobs.app.prepared"))}>
                <RefreshCw size={14} className={busy === "prepare" ? "animate-spin" : undefined} />
                {busy === "prepare" ? t("jobs.app.preparing") : t("jobs.app.prepare")}
              </button>
              <div className="mt-3">
                <button className="btn-ghost text-xs" disabled={!!busy} onClick={() => act("manual", () => jobMarkApplied(j.id), t("jobs.app.marked"))}>
                  {t("jobs.app.already")}
                </button>
              </div>
            </div>
          ) : (
            <div className="space-y-3">
              {app.cv_path ? (
                <div className="flex flex-wrap items-center gap-2">
                  <button className="btn-outline" onClick={() => jobOpenCv(j.id, false).catch((e) => toast(errorMessage(e), "error"))}>
                    <FileText size={14} /> {t("jobs.app.open_cv")}
                  </button>
                  <button className="btn-ghost text-xs" onClick={() => jobOpenCv(j.id, true).catch((e) => toast(errorMessage(e), "error"))}>
                    <FolderOpen size={13} /> {t("jobs.app.reveal_cv")}
                  </button>
                  {app.state === "draft" && (
                    <button className="btn-ghost text-xs" disabled={!!busy} onClick={() => act("prepare", () => jobPrepare(j.id), t("jobs.app.prepared"))}>
                      <RefreshCw size={13} className={busy === "prepare" ? "animate-spin" : undefined} /> {t("jobs.app.redo")}
                    </button>
                  )}
                </div>
              ) : (
                app.state === "draft" && <p className="text-[13px] text-danger">{t("jobs.app.no_cv")}</p>
              )}
              {app.ats && <AtsPanel report={app.ats} t={t} />}
              {app.cv_warnings.length > 0 && (
                <ul className="space-y-0.5 text-[12px] text-fg-subtle">
                  {app.cv_warnings.map((w) => (
                    <li key={w}>· {w}</li>
                  ))}
                </ul>
              )}

              {app.state === "draft" ? (
                <>
                  <Field label={t("jobs.app.to")}>
                    <input className="input" value={to} onChange={(e) => setTo(e.target.value)} placeholder={t("jobs.app.to_ph")} />
                  </Field>
                  <Field label={t("jobs.app.subject")}>
                    <input className="input" value={subject} onChange={(e) => setSubject(e.target.value)} />
                  </Field>
                  <Field label={t("jobs.app.body")}>
                    <textarea
                      className="input min-h-[230px] text-[13px] leading-relaxed"
                      value={body}
                      onChange={(e) => setBody(e.target.value)}
                      onBlur={() => jobSaveDraft(j.id, subject, body, followup).catch(() => {})}
                    />
                  </Field>
                  <p className="text-[12px] text-fg-subtle">
                    {sender ? t("jobs.app.from", { sender }) : t("jobs.setup.sender")}
                  </p>
                  <div className="flex flex-wrap items-center gap-2">
                    <button
                      className="btn-primary"
                      disabled={!!busy || !to.includes("@") || !app.cv_path || !sender}
                      onClick={() => confirm(t("jobs.app.send_confirm", { to })) && act("send", () => jobSend(j.id, to.trim(), subject, body), t("jobs.app.sent"))}
                    >
                      <Send size={14} /> {busy === "send" ? t("jobs.app.sending") : t("jobs.app.send")}
                    </button>
                    {(j.apply_url || j.url) && (
                      <button className="btn-outline" onClick={() => openExt(j.apply_url ?? j.url)}>
                        <ExternalLink size={14} /> {t("jobs.app.open_form")}
                      </button>
                    )}
                    <button className="btn-ghost text-xs" disabled={!!busy} onClick={() => act("manual", () => jobMarkApplied(j.id), t("jobs.app.marked"))}>
                      <Check size={13} /> {t("jobs.app.mark_filed")}
                    </button>
                    <button className="btn-ghost text-xs" disabled={!!busy} onClick={() => act("dismiss", () => jobSetStatus(j.id, "dismissed")).then(onClose)}>
                      <X size={13} /> {t("jobs.app.dismiss")}
                    </button>
                  </div>
                  {!to.includes("@") && <p className="text-[12px] text-fg-subtle">{t("jobs.app.form_hint")}</p>}
                </>
              ) : (
                <div className="rounded-xl border border-border px-3 py-2.5 text-[13px]">
                  <div className="flex flex-wrap items-center gap-2">
                    <MailCheck size={14} className="text-ok" />
                    {app.state === "sent" ? t("jobs.app.sent_on", { date: j.sent_at ? formatDate(j.sent_at) : "" }) : t("jobs.app.filed_on", { date: j.sent_at ? formatDate(j.sent_at) : "" })}
                    {app.state === "sent" &&
                      (j.opened_at ? (
                        <span className="badge bg-accent/15 text-accent">
                          <Eye size={11} /> {t("jobs.state.opened", { count: j.open_count })}
                        </span>
                      ) : (
                        <span className="text-fg-faint">{j.tracked ? t("jobs.app.not_opened") : t("jobs.app.untracked")}</span>
                      ))}
                    {j.replied_at && <span className="badge bg-ok/15 text-ok">{t("jobs.app.replied_on", { date: formatDate(j.replied_at) })}</span>}
                  </div>
                  {app.state === "sent" && j.opened_at && <p className="mt-1 text-[11.5px] text-fg-faint">{t("jobs.app.open_caveat")}</p>}
                </div>
              )}

              {sent && !j.replied_at && (
                <div>
                  <div className="kicker mb-1">
                    {j.followup_sent_at
                      ? t("jobs.app.followup_sent", { date: formatDate(j.followup_sent_at) })
                      : t("jobs.app.followup", { date: j.followup_due ? formatDate(j.followup_due) : "" })}
                  </div>
                  {!j.followup_sent_at && (
                    <>
                      <Field label={t("jobs.app.to")}>
                        <input className="input" value={to} onChange={(e) => setTo(e.target.value)} placeholder={t("jobs.app.to_ph")} />
                      </Field>
                      <textarea
                        className="input mt-2 min-h-[110px] text-[13px] leading-relaxed"
                        value={followup}
                        onChange={(e) => setFollowup(e.target.value)}
                        onBlur={() => jobSaveDraft(j.id, subject, body, followup).catch(() => {})}
                        placeholder={t("jobs.app.followup_ph")}
                      />
                      <button
                        className="btn-outline mt-2"
                        disabled={!!busy || !to.includes("@") || !followup.trim() || !sender}
                        onClick={() => confirm(t("jobs.app.send_confirm", { to })) && act("followup", () => jobSendFollowup(j.id, to.trim(), followup), t("jobs.app.followup_done"))}
                      >
                        <Mail size={14} /> {busy === "followup" ? t("jobs.app.sending") : t("jobs.app.send_followup")}
                      </button>
                    </>
                  )}
                </div>
              )}
            </div>
          )}
        </div>
      </div>
    </Modal>
  );
}

function AddModal({ onClose, onSaved }: { onClose: () => void; onSaved: (id: number) => void }) {
  const { t } = useTranslation();
  const toast = useToast();
  const qc = useQueryClient();
  const [f, setF] = useState({ title: "", company: "", location: "", url: "", apply_email: "", description: "" });
  const set = (k: keyof typeof f) => (e: React.ChangeEvent<HTMLInputElement | HTMLTextAreaElement>) => setF({ ...f, [k]: e.target.value });
  const save = () =>
    jobSave({ ...f, country: "" })
      .then((id) => {
        qc.invalidateQueries({ queryKey: ["jobs_board"] });
        onClose();
        onSaved(id);
      })
      .catch((e) => toast(errorMessage(e), "error"));
  return (
    <Modal open onClose={onClose} title={t("jobs.add")} width="max-w-xl">
      <p className="mb-3 text-[13px] text-fg-subtle">{t("jobs.add_hint")}</p>
      <div className="grid gap-3 md:grid-cols-2">
        <Field label={t("jobs.f.title")}>
          <input className="input" value={f.title} onChange={set("title")} autoFocus />
        </Field>
        <Field label={t("jobs.f.company")}>
          <input className="input" value={f.company} onChange={set("company")} />
        </Field>
        <Field label={t("jobs.f.location")}>
          <input className="input" value={f.location} onChange={set("location")} placeholder="London, UK" />
        </Field>
        <Field label={t("jobs.f.apply_email")}>
          <input className="input" value={f.apply_email} onChange={set("apply_email")} />
        </Field>
        <Field label={t("jobs.f.url")} className="md:col-span-2">
          <input className="input" value={f.url} onChange={set("url")} placeholder="https://" />
        </Field>
        <Field label={t("jobs.f.description")} className="md:col-span-2">
          <textarea className="input min-h-[140px]" value={f.description} onChange={set("description")} />
        </Field>
      </div>
      <div className="mt-4 flex justify-end gap-2">
        <button className="btn-ghost" onClick={onClose}>
          {t("common.cancel")}
        </button>
        <button className="btn-primary" disabled={!f.title.trim()} onClick={save}>
          {t("common.save")}
        </button>
      </div>
    </Modal>
  );
}

// ---------------- Settings ----------------

function SettingsTab() {
  const { t } = useTranslation();
  const toast = useToast();
  const qc = useQueryClient();
  const { data: c, isLoading, refetch } = useQuery({ queryKey: ["jobs_config"], queryFn: jobsConfig });
  const [prefs, setPrefs] = useState({ enabled: true, min_score: 65, daily_prepare: 6, followup_days: 7, adzuna_app_id: "", adzuna_app_key: "" });
  const [profile, setProfile] = useState("");
  const [search, setSearch] = useState({ domain: "sync", query: "", country: "FR" });
  const [board, setBoard] = useState({ platform: "greenhouse", token: "", company: "" });
  const [report, setReport] = useState<AtsReport | null>(null);
  useEffect(() => {
    if (!c) return;
    setPrefs({ enabled: c.enabled, min_score: c.min_score, daily_prepare: c.daily_prepare, followup_days: c.followup_days, adzuna_app_id: c.adzuna_app_id, adzuna_app_key: c.adzuna_app_key });
    setProfile(c.profile);
  }, [c]);
  if (isLoading || !c) return <Loader />;
  const ok = (msg?: string) => () => {
    refetch();
    qc.invalidateQueries({ queryKey: ["jobs_board"] });
    if (msg) toast(msg, "ok");
  };
  const fail = (e: unknown) => toast(errorMessage(e), "error");
  const num = (k: "min_score" | "daily_prepare" | "followup_days") => (e: React.ChangeEvent<HTMLInputElement>) =>
    setPrefs({ ...prefs, [k]: Number(e.target.value) || 0 });
  const importSheet = async () => {
    const path = await pickFile({ multiple: false, filters: [{ name: "CSV", extensions: ["csv"] }] });
    if (typeof path === "string") jobsImportSheet(path).then((n) => ok(t("jobs.set.imported", { count: n }))()).catch(fail);
  };
  const checkPdf = async () => {
    const path = await pickFile({ multiple: false, filters: [{ name: "PDF", extensions: ["pdf"] }] });
    if (typeof path === "string") jobsCheckPdf(path).then(setReport).catch(fail);
  };
  const sel = "input w-auto py-1.5 text-xs";
  return (
    <div className="mx-auto max-w-4xl space-y-6 px-page py-5">
      <section className="card p-4">
        <div className="kicker mb-3">{t("jobs.set.daily")}</div>
        <label className="mb-3 flex items-center gap-2 text-sm">
          <input type="checkbox" checked={prefs.enabled} onChange={(e) => setPrefs({ ...prefs, enabled: e.target.checked })} />
          {t("jobs.set.enabled")}
        </label>
        <div className="grid gap-3 @md:grid-cols-3">
          <Field label={t("jobs.set.min_score")}>
            <input className="input" type="number" min={0} max={100} value={prefs.min_score} onChange={num("min_score")} />
          </Field>
          <Field label={t("jobs.set.daily_prepare")}>
            <input className="input" type="number" min={0} max={30} value={prefs.daily_prepare} onChange={num("daily_prepare")} />
          </Field>
          <Field label={t("jobs.set.followup_days")}>
            <input className="input" type="number" min={2} max={30} value={prefs.followup_days} onChange={num("followup_days")} />
          </Field>
          <Field label="Adzuna app_id">
            <input className="input" value={prefs.adzuna_app_id} onChange={(e) => setPrefs({ ...prefs, adzuna_app_id: e.target.value })} />
          </Field>
          <Field label="Adzuna app_key">
            <input className="input" type="password" value={prefs.adzuna_app_key} onChange={(e) => setPrefs({ ...prefs, adzuna_app_key: e.target.value })} />
          </Field>
        </div>
        <p className="mt-2 text-[12.5px] text-fg-subtle">
          {t("jobs.set.adzuna_hint")}{" "}
          <button className="underline" onClick={() => openExt("https://developer.adzuna.com/")}>
            developer.adzuna.com
          </button>
        </p>
        <p className="mt-1 text-[12.5px] text-fg-subtle">{t("jobs.set.daily_hint")}</p>
        <button className="btn-primary mt-3" onClick={() => jobsSavePrefs(prefs).then(ok(t("jobs.saved"))).catch(fail)}>
          {t("common.save")}
        </button>
      </section>

      <section className="card p-4">
        <div className="kicker mb-3">{t("jobs.set.searches")}</div>
        <div className="flex flex-wrap gap-1.5">
          {c.searches.map((s) => (
            <span key={s.id} className={clsx("badge gap-1.5", s.active ? "bg-fg/[0.08] text-fg" : "bg-fg/[0.04] text-fg-faint line-through")}>
              <button onClick={() => jobsSaveSearch({ ...s, active: !s.active }).then(ok()).catch(fail)} title={t("jobs.set.toggle")}>
                {s.query} · {s.country}
              </button>
              <button onClick={() => s.id != null && jobsDeleteSearch(s.id).then(ok()).catch(fail)} aria-label={t("common.delete")}>
                <X size={11} />
              </button>
            </span>
          ))}
        </div>
        <div className="mt-3 flex flex-wrap items-center gap-2">
          <select className={sel} value={search.domain} onChange={(e) => setSearch({ ...search, domain: e.target.value })}>
            {["sync", "label", "booking"].map((d) => (
              <option key={d} value={d}>
                {t(`jobs.domain.${d}`)}
              </option>
            ))}
          </select>
          <input className="input w-64 py-1.5 text-xs" placeholder={t("jobs.set.query_ph")} value={search.query} onChange={(e) => setSearch({ ...search, query: e.target.value })} />
          <select className={sel} value={search.country} onChange={(e) => setSearch({ ...search, country: e.target.value })}>
            {["FR", "UK", "US"].map((d) => (
              <option key={d} value={d}>
                {d}
              </option>
            ))}
          </select>
          <button
            className="btn-outline"
            disabled={!search.query.trim()}
            onClick={() => jobsSaveSearch({ id: null, ...search, active: true }).then(() => (setSearch({ ...search, query: "" }), ok()())).catch(fail)}
          >
            <Plus size={14} /> {t("common.add")}
          </button>
        </div>
      </section>

      <section className="card p-4">
        <div className="kicker mb-1">{t("jobs.set.boards")}</div>
        <p className="mb-3 text-[12.5px] text-fg-subtle">{t("jobs.set.boards_hint")}</p>
        <div className="divide-y divide-border">
          {c.boards.map((b) => (
            <div key={b.id} className="flex items-center gap-3 py-1.5 text-[13px]">
              <span className="w-44 truncate">{b.company || b.token}</span>
              <span className="w-28 text-fg-subtle">{b.platform}</span>
              <span className="min-w-0 flex-1 truncate text-fg-faint">{b.last_error ? <span className="text-danger">{b.last_error}</span> : b.last_ok ? t("jobs.set.read_on", { date: formatDate(b.last_ok) }) : b.token}</span>
              <button className="text-fg-faint hover:text-danger" onClick={() => b.id != null && jobsDeleteBoard(b.id).then(ok()).catch(fail)} aria-label={t("common.delete")}>
                <Trash2 size={13} />
              </button>
            </div>
          ))}
        </div>
        <div className="mt-3 flex flex-wrap items-center gap-2">
          <select className={sel} value={board.platform} onChange={(e) => setBoard({ ...board, platform: e.target.value })}>
            {["greenhouse", "lever", "ashby", "smartrecruiters", "workable", "recruitee", "rss"].map((p) => (
              <option key={p} value={p}>
                {p}
              </option>
            ))}
          </select>
          <input className="input w-44 py-1.5 text-xs" placeholder={t("jobs.set.company_ph")} value={board.company} onChange={(e) => setBoard({ ...board, company: e.target.value })} />
          <input className="input w-56 py-1.5 text-xs" placeholder={t("jobs.set.token_ph")} value={board.token} onChange={(e) => setBoard({ ...board, token: e.target.value })} />
          <button
            className="btn-outline"
            disabled={!board.token.trim()}
            onClick={() => jobsSaveBoard({ id: null, ...board, active: true }).then(() => (setBoard({ ...board, token: "", company: "" }), ok()())).catch(fail)}
          >
            <Plus size={14} /> {t("common.add")}
          </button>
        </div>
      </section>

      <section className="card p-4">
        <div className="kicker mb-1">{t("jobs.set.profile")}</div>
        <p className="mb-3 text-[12.5px] text-fg-subtle">{t("jobs.set.profile_hint")}</p>
        <textarea className="input min-h-[320px] font-mono text-[12px]" spellCheck={false} value={profile} onChange={(e) => setProfile(e.target.value)} />
        <button className="btn-primary mt-3" onClick={() => jobsSaveProfile(profile).then(ok(t("jobs.saved"))).catch(fail)}>
          {t("common.save")}
        </button>
      </section>

      <section className="card p-4">
        <div className="kicker mb-3">{t("jobs.set.tools")}</div>
        <div className="flex flex-wrap gap-2">
          <button className="btn-outline" onClick={checkPdf}>
            <ShieldCheck size={14} /> {t("jobs.set.check_pdf")}
          </button>
          <button className="btn-outline" onClick={importSheet}>
            <FolderOpen size={14} /> {t("jobs.set.import")}
          </button>
        </div>
        {report && (
          <div className="mt-3">
            <AtsPanel report={report} t={t} />
          </div>
        )}
      </section>
    </div>
  );
}
