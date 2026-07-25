import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useState,
  type ReactNode,
} from "react";
import { createPortal } from "react-dom";
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
  useEffect(() => {
    if (!open) return;
    const onEsc = (e: KeyboardEvent) => e.key === "Escape" && onClose();
    window.addEventListener("keydown", onEsc);
    return () => window.removeEventListener("keydown", onEsc);
  }, [open, onClose]);

  if (!open) return null;
  return createPortal(
    <div className="fixed inset-0 z-50 flex items-start justify-center overflow-y-auto bg-black/40 backdrop-blur-sm p-6">
      <div
        className={clsx(
          "card w-full my-8 animate-fade-in overflow-hidden",
          width
        )}
        onClick={(e) => e.stopPropagation()}
      >
        {title !== undefined && (
          <div className="flex items-center justify-between border-b border-border px-5 py-3.5">
            <h3 className="text-sm font-semibold">{title}</h3>
            <button className="btn-ghost -mr-2 px-2 py-1" onClick={onClose}>
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
    setTimeout(() => setToasts((t) => t.filter((x) => x.id !== id)), 3600);
  }, []);
  return (
    <ToastCtx.Provider value={push}>
      {children}
      {createPortal(
        <div className="fixed bottom-5 right-5 z-[60] flex flex-col gap-2">
          {toasts.map((t) => (
            <div
              key={t.id}
              className={clsx(
                "flex items-center gap-2.5 rounded-xl border px-4 py-3 text-sm shadow-card animate-fade-in bg-surface",
                t.kind === "ok"
                  ? "border-emerald-500/30 text-fg"
                  : "border-rose-500/30 text-fg"
              )}
            >
              {t.kind === "ok" ? (
                <CheckCircle2 size={16} className="text-emerald-500 shrink-0" />
              ) : (
                <AlertCircle size={16} className="text-rose-500 shrink-0" />
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
