import { useQuery } from "@tanstack/react-query";
import { invoke } from "@tauri-apps/api/core";
import { useTranslation } from "react-i18next";
import { MailCheck } from "lucide-react";

interface CheckStats {
  service_set: boolean;
  checked: number;
  valid: number;
  risky: number;
  unknown: number;
  invalid: number;
}

/// Address check before every first email: always the local layer (typo,
/// throwaway, dead domain), plus the mailbox itself once a MillionVerifier
/// key is set. Invalid addresses are dropped like a bounce.
export function EmailVerifierCard() {
  const { t } = useTranslation();
  const { data: stats } = useQuery({
    queryKey: ["email_check_stats"],
    queryFn: () => invoke<CheckStats>("email_check_stats"),
    refetchInterval: 60_000,
  });
  return (
    <div className="card p-6">
      <div className="mb-1 flex items-center gap-2 text-sm font-semibold">
        <MailCheck size={15} /> {t("verify.title")}
      </div>
      <p className="mb-2 text-xs text-fg-subtle">{t("verify.help")}</p>
      <p className="mt-3 text-2xs text-fg-faint">
        {stats?.service_set ? t("verify.mode_service") : t("verify.mode_local")}
      </p>
      {!!stats?.checked && (
        <div className="mt-4 grid grid-cols-4 gap-2 text-center">
          {(["valid", "risky", "unknown", "invalid"] as const).map((k) => (
            <div key={k} className="rounded-lg bg-fg/[0.04] px-2 py-2">
              <div className="font-condensed text-xl tabular">{stats[k]}</div>
              <div className="eyebrow text-[9px]">{t(`verify.${k}`)}</div>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
