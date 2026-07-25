import { NavLink, Outlet } from "react-router-dom";
import { useTranslation } from "react-i18next";
import { UpdateBanner } from "./UpdateBanner";
import {
  LayoutDashboard,
  Music2,
  Users,
  Upload,
  Mail,
  Wallet,
  CalendarClock,
  Target,
  Plane,
  Settings as SettingsIcon,
} from "lucide-react";
import clsx from "clsx";

const NAV = [
  { to: "/", key: "dashboard", icon: LayoutDashboard, end: true },
  { to: "/artists", key: "artists", icon: Music2 },
  { to: "/contacts", key: "contacts", icon: Users },
  { to: "/import", key: "import", icon: Upload },
  { to: "/emails", key: "emails", icon: Mail },
  { to: "/budget", key: "budget", icon: Wallet },
  { to: "/timeline", key: "timeline", icon: CalendarClock },
  { to: "/kpis", key: "kpis", icon: Target },
  { to: "/visa", key: "visa", icon: Plane },
];

export function Layout() {
  const { t } = useTranslation();
  return (
    <div className="flex h-screen w-screen overflow-hidden bg-bg text-fg">
      {/* Sidebar */}
      <aside className="flex w-60 shrink-0 flex-col border-r border-border bg-surface">
        <div
          className="flex h-14 items-center gap-2.5 px-5"
          data-tauri-drag-region
        >
          <div className="flex h-7 w-7 items-center justify-center rounded-lg bg-accent text-accent-fg text-sm font-bold">
            P
          </div>
          <span className="text-[15px] font-semibold tracking-tight">
            PiersCRM
          </span>
        </div>

        <nav className="flex-1 space-y-0.5 px-3 py-2">
          {NAV.map(({ to, key, icon: Icon, end }) => (
            <NavLink
              key={to}
              to={to}
              end={end}
              className={({ isActive }) =>
                clsx(
                  "flex items-center gap-3 rounded-lg px-3 py-2 text-sm font-medium transition-colors",
                  isActive
                    ? "bg-accent/10 text-accent"
                    : "text-fg-subtle hover:bg-muted hover:text-fg"
                )
              }
            >
              <Icon size={17} strokeWidth={2} />
              {t(`nav.${key}`)}
            </NavLink>
          ))}
        </nav>

        <div className="px-3 py-3">
          <NavLink
            to="/settings"
            className={({ isActive }) =>
              clsx(
                "flex items-center gap-3 rounded-lg px-3 py-2 text-sm font-medium transition-colors",
                isActive
                  ? "bg-accent/10 text-accent"
                  : "text-fg-subtle hover:bg-muted hover:text-fg"
              )
            }
          >
            <SettingsIcon size={17} />
            {t("nav.settings")}
          </NavLink>
        </div>
      </aside>

      {/* Main */}
      <main className="flex min-w-0 flex-1 flex-col overflow-hidden">
        <div className="h-3 w-full shrink-0" data-tauri-drag-region />
        <UpdateBanner />
        <div className="flex-1 overflow-y-auto">
          <Outlet />
        </div>
      </main>
    </div>
  );
}

export function PageHeader({
  title,
  subtitle,
  actions,
}: {
  title: string;
  subtitle?: string;
  actions?: React.ReactNode;
}) {
  return (
    <div className="flex items-start justify-between gap-4 px-8 pb-5 pt-3">
      <div>
        <h1 className="text-xl font-semibold tracking-tight">{title}</h1>
        {subtitle && (
          <p className="mt-0.5 text-sm text-fg-subtle">{subtitle}</p>
        )}
      </div>
      {actions && <div className="flex items-center gap-2">{actions}</div>}
    </div>
  );
}
