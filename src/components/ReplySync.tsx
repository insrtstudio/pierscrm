import { useEffect } from "react";
import { useQueryClient, type QueryClient } from "@tanstack/react-query";
import { getSetting, syncReplies } from "../lib/api";
import type { ReplySyncResult } from "../lib/types";

const EVERY_MS = 10 * 60_000;
/// First run shortly after launch, once the first screen has loaded.
const FIRST_MS = 45_000;

/// What a reply sync can change: the history, campaign stats, follow-ups,
/// the pipeline and the guardrails tab.
export function invalidateAfterReplySync(qc: QueryClient) {
  for (const key of [
    "emails",
    "campaigns",
    "followups",
    "contacts",
    "dashboard",
    "suppressions",
    "reply_sync_status",
  ])
    qc.invalidateQueries({ queryKey: [key] });
}

/// Manual sync, shared by the button and the timer.
export async function runReplySync(qc: QueryClient): Promise<ReplySyncResult> {
  const res = await syncReplies();
  if (!res.skipped) invalidateAfterReplySync(qc);
  return res;
}

/// Background reply detection: scans the inbox every 10 minutes while the app is
/// open (read-only on the server). Silent: errors are recorded by the backend and
/// shown in Outbox > Guardrails, never as a toast every 10 minutes. Off when the
/// `imap_auto` setting is "0" or no sender is configured.
export function ReplySync() {
  const qc = useQueryClient();
  useEffect(() => {
    let stopped = false;
    const tick = async () => {
      try {
        if ((await getSetting("imap_auto")) === "0") return;
        const smtp = await getSetting("smtp");
        if (!smtp || !smtp.includes("@")) return;
        await runReplySync(qc);
      } catch {
        // Recorded as imap_last_error by the backend.
      }
    };
    const first = setTimeout(() => !stopped && tick(), FIRST_MS);
    const id = setInterval(() => !stopped && tick(), EVERY_MS);
    return () => {
      stopped = true;
      clearTimeout(first);
      clearInterval(id);
    };
  }, [qc]);
  return null;
}
