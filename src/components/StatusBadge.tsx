import { useTranslation } from "react-i18next";
import clsx from "clsx";
import { STATUS_STYLES } from "../lib/constants";
import { STATUSES, type Status } from "../lib/types";

export function StatusBadge({ status }: { status: string }) {
  const { t } = useTranslation();
  const style = STATUS_STYLES[status as Status] ?? "text-fg-subtle bg-muted";
  return (
    <span className={clsx("badge", style)}>{t(`status.${status}`, status)}</span>
  );
}

export function StatusSelect({
  value,
  onChange,
  className,
}: {
  value: string;
  onChange: (v: Status) => void;
  className?: string;
}) {
  const { t } = useTranslation();
  return (
    <select
      className={clsx("input", className)}
      value={value}
      onChange={(e) => onChange(e.target.value as Status)}
    >
      {STATUSES.map((s) => (
        <option key={s} value={s}>
          {t(`status.${s}`)}
        </option>
      ))}
    </select>
  );
}

export function PriorityBadge({ priority }: { priority?: string | null }) {
  if (!priority) return null;
  const p = priority.trim().toUpperCase();
  const tone =
    p === "P1" || p === "A"
      ? "bg-accent text-accent-fg"
      : p === "P2" || p === "B"
      ? "bg-accent-soft text-accent-2"
      : p === "P3" || p === "C"
      ? "bg-muted text-fg-subtle"
      : "bg-muted text-fg-subtle";
  return (
    <span
      className={clsx(
        "inline-flex items-center px-1.5 py-0.5 text-2xs font-bold tabular",
        tone
      )}
    >
      {p}
    </span>
  );
}
