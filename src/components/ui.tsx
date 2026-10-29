import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useId,
  useRef,
  useState,
  type ReactNode,
} from "react";
import { createPortal } from "react-dom";
import { useTranslation } from "react-i18next";
import { X, CheckCircle2, AlertCircle } from "lucide-react";
import clsx from "clsx";

// ---------------- Modal ----------------

export function Modal({
  open,
  onClose,
  title,
  children,
  width = "max-w-lg",
}: {
  open: boolean;
  onClose: () => void;
  title?: ReactNode;
  children: ReactNode;
  width?: string;
}) {
  const { t } = useTranslation();
  const ref = useRef<HTMLDivElement>(null);
  const titleId = useId();
  // Callers pass inline arrows: depending on `onClose` directly re-ran the effect
  // on every parent render, which yanked focus back to the first field mid-typing.
  const onCloseRef = useRef(onClose);
  onCloseRef.current = onClose;

  useEffect(() => {
    if (!open) return;
    const prevFocus = document.activeElement as HTMLElement | null;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        onCloseRef.current();
        return;
      }
      if (e.key === "Tab" && ref.current) {
        const f = ref.current.querySelectorAll<HTMLElement>(
          'a[href],button:not([disabled]),textarea,input,select,[tabindex]:not([tabindex="-1"])'
        );
        if (f.length === 0) return;
        const first = f[0];
        const last = f[f.length - 1];
        if (e.shiftKey && document.activeElement === first) {
          e.preventDefault();
          last.focus();
        } else if (!e.shiftKey && document.activeElement === last) {
          e.preventDefault();
          first.focus();
        }
      }
    };
    window.addEventListener("keydown", onKey);
    const prevOverflow = document.body.style.overflow;
    document.body.style.overflow = "hidden";
    const to = setTimeout(() => {
      const el = ref.current?.querySelector<HTMLElement>("input,textarea,select,button");
      (el ?? ref.current)?.focus();
    }, 0);
    return () => {
      window.removeEventListener("keydown", onKey);
      document.body.style.overflow = prevOverflow;
      clearTimeout(to);
      prevFocus?.focus?.();
    };
  }, [open]);

  if (!open) return null;
  return createPortal(
    <div
      className="fixed inset-0 z-50 flex items-start justify-center overflow-y-auto bg-black/45 p-6"
      onClick={onClose}
    >
      <div
        ref={ref}
        role="dialog"
        aria-modal="true"
        aria-labelledby={title !== undefined ? titleId : undefined}
        tabIndex={-1}
        className={clsx("card w-full my-8 animate-fade-in overflow-hidden bg-bg shadow-pop outline-none", width)}
        onClick={(e) => e.stopPropagation()}
      >
        {title !== undefined && (
          <div className="flex items-center justify-between border-b border-border px-5 py-3.5">
            <h3 id={titleId} className="eyebrow text-fg">
              {title}
            </h3>
            <button
              className="btn-ghost -mr-2 px-2 py-1"
              aria-label={t("common.close")}
              onClick={onClose}
            >
              <X size={16} />
            </button>
          </div>
        )}
        <div className="p-5">{children}</div>
      </div>
    </div>,
    document.body
  );
}

// ---------------- Toast ----------------

/// Readable text from whatever a failed invoke() rejects with (Rust `Err(String)`
/// arrives as a bare string).
export function errorMessage(e: unknown): string {
  if (typeof e === "string") return e;
  if (e instanceof Error) return e.message;
  if (e && typeof e === "object" && "message" in e) return String((e as { message: unknown }).message);
  return String(e);
}

/// Wrap a save handler: ignores re-entry while it runs (double-click on Save made
/// duplicate rows), reports failures as a toast, and exposes `busy` for the button.
export function useSubmit<A extends unknown[]>(fn: (...args: A) => Promise<unknown>) {
  const toast = useToast();
  const [busy, setBusy] = useState(false);
  const running = useRef(false);
  const fnRef = useRef(fn);
  fnRef.current = fn;
  const run = useCallback(
    async (...args: A) => {
      if (running.current) return;
      running.current = true;
      setBusy(true);
      try {
        await fnRef.current(...args);
      } catch (e) {
        toast(errorMessage(e), "error");
      } finally {
        running.current = false;
        setBusy(false);
      }
    },
    [toast]
  );
  return [run, busy] as const;
}

type Toast = { id: number; msg: string; kind: "ok" | "error" };
const ToastCtx = createContext<(msg: string, kind?: "ok" | "error") => void>(
  () => {}
);
export const useToast = () => useContext(ToastCtx);

export function ToastProvider({ children }: { children: ReactNode }) {
  const [toasts, setToasts] = useState<Toast[]>([]);
  const push = useCallback((msg: string, kind: "ok" | "error" = "ok") => {
    const id = Date.now() + Math.floor(Math.random() * 1000);
    setToasts((t) => [...t, { id, msg, kind }]);
    setTimeout(
      () => setToasts((t) => t.filter((x) => x.id !== id)),
      kind === "error" ? 6000 : 3600
    );
  }, []);
  // Safety net: a failed save that nobody caught (an awaited invoke without
  // try/catch, a mutation without onError) used to fail in total silence.
  useEffect(() => {
    const onRejection = (e: PromiseRejectionEvent) => push(errorMessage(e.reason), "error");
    const onAppError = (e: Event) => push(errorMessage((e as CustomEvent).detail), "error");
    window.addEventListener("unhandledrejection", onRejection);
    window.addEventListener("app:error", onAppError);
    return () => {
      window.removeEventListener("unhandledrejection", onRejection);
      window.removeEventListener("app:error", onAppError);
    };
  }, [push]);
  return (
    <ToastCtx.Provider value={push}>
      {children}
      {createPortal(
        <div
          className="fixed bottom-5 right-5 z-[60] flex flex-col gap-2"
          aria-live="polite"
          aria-atomic="true"
        >
          {toasts.map((t) => (
            <div
              key={t.id}
              role={t.kind === "error" ? "alert" : "status"}
              className={clsx(
                "flex items-center gap-2.5 rounded-md border px-4 py-3 text-sm shadow-pop animate-fade-in bg-surface",
                t.kind === "ok"
                  ? "border-ok/30 text-fg"
                  : "border-danger/30 text-fg"
              )}
            >
              {t.kind === "ok" ? (
                <CheckCircle2 size={16} className="text-ok shrink-0" />
              ) : (
                <AlertCircle size={16} className="text-danger shrink-0" />
              )}
              <span className="max-w-xs">{t.msg}</span>
            </div>
          ))}
        </div>,
        document.body
      )}
    </ToastCtx.Provider>
  );
}

// ---------------- Confirm dialog ----------------

export function useConfirm() {
  // Lightweight window.confirm replacement kept in one place for future theming.
  return useCallback((msg: string) => window.confirm(msg), []);
}

// ---------------- Field ----------------

export function Field({
  label,
  children,
  className,
}: {
  label: string;
  children: ReactNode;
  className?: string;
}) {
  return (
    <label className={clsx("block", className)}>
      <span className="label">{label}</span>
      {children}
    </label>
  );
}
