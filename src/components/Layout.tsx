import { useEffect } from "react";
import { NavLink, Outlet, useNavigate } from "react-router-dom";
import { useTranslation } from "react-i18next";
import { listen } from "@tauri-apps/api/event";
import {
  LayoutDashboard,
  CalendarDays,
  Music2,
  Users,
  Upload,
  Mail,
  Wallet,
  Target,
  Plane,
  Settings as SettingsIcon,
  type LucideIcon,
} from "lucide-react";
import clsx from "clsx";
import { UpdateBanner } from "./UpdateBanner";

type NavItem = { to: string; key: string; icon: LucideIcon; end?: boolean };
type NavGroup = { label: string; items: NavItem[] };

const GROUPS: NavGroup[] = [
  {
    label: "steer",
    items: [
      { to: "/", key: "dashboard", icon: LayoutDashboard, end: true },
      { to: "/agenda", key: "agenda", icon: CalendarDays },
      { to: "/kpis", key: "kpis", icon: Target },
    ],
  },
  {
    label: "roster",
    items: [
      { to: "/artists", key: "artists", icon: Music2 },
      { to: "/contacts", key: "contacts", icon: Users },
    ],
  },
  {
    label: "outreach",
    items: [
      { to: "/emails", key: "emails", icon: Mail },
      { to: "/import", key: "import", icon: Upload },
    ],
  },
  {
    label: "ops",
    items: [
      { to: "/budget", key: "budget", icon: Wallet },
      { to: "/visa", key: "visa", icon: Plane },
    ],
  },
];

function useMenuActions() {
  const navigate = useNavigate();
  useEffect(() => {
    const routes: Record<string, string> = {
      "nav:dashboard": "/",
      "nav:agenda": "/agenda",
      "nav:artists": "/artists",
      "nav:contacts": "/contacts",
      "nav:emails": "/emails",
      "nav:budget": "/budget",
      "nav:visa": "/visa",
      "nav:import": "/import",
      "nav:settings": "/settings",
      "check-updates": "/settings",
      "new:artist": "/artists",
      "new:contact": "/contacts",
      "new:event": "/agenda",
      "new:campaign": "/emails",
    };
    const un = listen<string>("menu-action", (e) => {
      const id = e.payload;
      const route = routes[id];
      if (route) navigate(route);
      if (id.startsWith("new:")) {
        // let the destination page react after it mounts
        setTimeout(
          () => window.dispatchEvent(new CustomEvent("app:new", { detail: id })),
          60
        );
      }
      if (id === "check-updates")
        setTimeout(
          () => window.dispatchEvent(new CustomEvent("app:check-updates")),
          60
        );
    });
    return () => {
      un.then((f) => f());
    };
  }, [navigate]);
}

export function Layout() {
  const { t } = useTranslation();
  useMenuActions();

  return (
    <div className="flex h-screen w-screen overflow-hidden bg-bg text-fg">
      {/* Sidebar */}
      <aside className="flex w-[236px] shrink-0 flex-col border-r border-border bg-surface/60">
        <div className="drag flex h-[52px] items-center gap-2.5 px-4 pl-20">
          <div className="flex h-[26px] w-[26px] items-center justify-center rounded-[8px] bg-accent text-accent-fg text-[13px] font-bold shadow-accent-glow">
            P
          </div>
          <span className="text-[15px] font-semibold tracking-tight">PiersCRM</span>
        </div>

        <nav className="flex-1 overflow-y-auto px-3 py-2">
          {GROUPS.map((group) => (
            <div key={group.label} className="mb-4">
              <div className="px-3 pb-1.5 text-[10px] font-semibold uppercase tracking-[0.08em] text-fg-faint">
                {t(`nav_group.${group.label}`)}
              </div>
              <div className="space-y-0.5">
                {group.items.map(({ to, key, icon: Icon, end }) => (
                  <NavLink
                    key={to}
                    to={to}
                    end={end}
                    className={({ isActive }) =>
                      clsx(
                        "group relative flex items-center gap-2.5 rounded-lg px-3 py-[7px] text-[13.5px] font-medium transition-all",
                        isActive
                          ? "bg-accent-soft text-accent"
                          : "text-fg-subtle hover:bg-muted hover:text-fg"
                      )
                    }
                  >
                    {({ isActive }) => (
                      <>
                        {isActive && (
                          <span className="absolute left-0 top-1/2 h-4 w-[3px] -translate-y-1/2 rounded-r-full bg-accent" />
                        )}
                        <Icon size={17} strokeWidth={2} />
                        {t(`nav.${key}`)}
                      </>
                    )}
                  </NavLink>
                ))}
              </div>
            </div>
          ))}
        </nav>

        <div className="border-t border-border p-3">
          <NavLink
            to="/settings"
            className={({ isActive }) =>
              clsx(
                "flex items-center gap-2.5 rounded-lg px-3 py-[7px] text-[13.5px] font-medium transition-all",
                isActive
                  ? "bg-accent-soft text-accent"
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
        <div className="drag h-[52px] w-full shrink-0" />
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
    <div className="sticky top-0 z-20 flex items-end justify-between gap-4 border-b border-border bg-bg/80 px-8 pb-4 pt-1 backdrop-blur-xl">
      <div>
        <h1 className="text-[22px] font-semibold tracking-tight">{title}</h1>
        {subtitle && <p className="mt-0.5 text-sm text-fg-subtle">{subtitle}</p>}
      </div>
      {actions && (
        <div className="no-drag flex items-center gap-2 pb-0.5">{actions}</div>
      )}
    </div>
  );
}

/** Consistent empty-state block. */
export function EmptyState({
  icon: Icon,
  title,
  hint,
  action,
}: {
  icon: LucideIcon;
  title: string;
  hint?: string;
  action?: React.ReactNode;
}) {
  return (
    <div className="flex flex-col items-center justify-center gap-3 py-20 text-center">
      <div className="flex h-14 w-14 items-center justify-center rounded-2xl bg-muted text-fg-faint">
        <Icon size={26} />
      </div>
      <div>
        <div className="text-sm font-medium text-fg">{title}</div>
        {hint && <div className="mt-1 text-sm text-fg-subtle">{hint}</div>}
      </div>
      {action}
    </div>
  );
}
