import { useEffect, useMemo, useState } from "react";
import { NavLink, Outlet, useNavigate } from "react-router-dom";
import { useTranslation } from "react-i18next";
import { useQuery } from "@tanstack/react-query";
import { listen } from "@tauri-apps/api/event";
import clsx from "clsx";
import { Sun, Moon, type LucideIcon } from "lucide-react";
import { listArtists, listEvents } from "../lib/api";
import { getTheme, toggleTheme, type Theme } from "../lib/theme";
import { UpdateBanner } from "./UpdateBanner";

type NavItem = { to: string; key: string; end?: boolean };

const NAV: NavItem[] = [
  { to: "/", key: "dashboard", end: true },
  { to: "/agenda", key: "agenda" },
  { to: "/kpis", key: "kpis" },
  { to: "/artists", key: "artists" },
  { to: "/contacts", key: "contacts" },
  { to: "/emails", key: "emails" },
  { to: "/import", key: "import" },
  { to: "/budget", key: "budget" },
  { to: "/visa", key: "visa" },
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
      if (id.startsWith("new:"))
        setTimeout(
          () => window.dispatchEvent(new CustomEvent("app:new", { detail: id })),
          60
        );
      if (id === "check-updates")
        setTimeout(() => window.dispatchEvent(new CustomEvent("app:check-updates")), 60);
    });
    return () => {
      un.then((f) => f());
    };
  }, [navigate]);
}

function Ticker() {
  const { t } = useTranslation();
  const today = useMemo(() => {
    const d = new Date();
    return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-${String(
      d.getDate()
    ).padStart(2, "0")}`;
  }, []);
  const { data: events = [] } = useQuery({
    queryKey: ["events", "ticker", today],
    queryFn: () => listEvents({ from: today }),
  });
  const { data: artists = [] } = useQuery({ queryKey: ["artists"], queryFn: listArtists });

  const months = t("agenda.months").split("_");
  const text = useMemo(() => {
    const parts = events.slice(0, 12).map((e) => {
      const d = new Date(e.date);
      const day = `${String(d.getDate()).padStart(2, "0")} ${months[d.getMonth()]
        ?.slice(0, 3)
        .toUpperCase()}`;
      const artist = artists.find((a) => a.id === e.artist_id)?.name;
      return `${(artist || e.title).toUpperCase()} — ${day}${
        e.start_time ? " — " + e.start_time : ""
      }`;
    });
    if (parts.length === 0) parts.push("PIERSCRM · INSRT.STUDIO · BOOKING & ARTIST MANAGEMENT");
    return parts.join("   ●   ") + "   ●   ";
  }, [events, artists, months]);

  return (
    <div data-tauri-drag-region className="relative h-8 shrink-0 overflow-hidden bg-accent">
      <div className="pointer-events-none flex w-max whitespace-nowrap pl-6 animate-ticker">
        <span className="py-2 text-2xs font-bold tracking-[0.18em] text-accent-ink">{text}</span>
        <span className="py-2 text-2xs font-bold tracking-[0.18em] text-accent-ink">{text}</span>
      </div>
    </div>
  );
}

export function Layout() {
  const { t } = useTranslation();
  useMenuActions();
  const [theme, setTheme] = useState<Theme>(getTheme());
  useEffect(() => {
    const h = (e: Event) => setTheme((e as CustomEvent).detail as Theme);
    window.addEventListener("app:theme", h);
    return () => window.removeEventListener("app:theme", h);
  }, []);

  return (
    <div className="flex h-screen w-screen flex-col overflow-hidden bg-bg text-fg">
      {/* Dedicated macOS title bar — houses the traffic-light controls, keeps the
          ticker below fully clear of them */}
      <div
        data-tauri-drag-region
        className="relative flex h-7 shrink-0 items-center justify-center bg-black"
      >
        <span className="pointer-events-none select-none text-2xs font-bold uppercase tracking-[0.22em] text-fg-faint">
          PiersCRM
        </span>
      </div>
      <Ticker />
      <div className="flex min-h-0 flex-1">
        {/* Red poster sidebar */}
        <aside className="flex w-[224px] shrink-0 flex-col bg-accent text-accent-ink">
          <div className="px-6 pb-8 pt-6">
            <div className="text-[42px] font-black leading-[0.85] tracking-tightest">P.</div>
            <div className="mt-2 text-2xs font-bold tracking-[0.2em]">
              PIERSCRM — INSRT.STUDIO
            </div>
          </div>

          <nav className="flex-1 overflow-y-auto text-[13.5px] font-bold">
            {NAV.map(({ to, key, end }) => (
              <NavLink
                key={to}
                to={to}
                end={end}
                className={({ isActive }) =>
                  clsx(
                    "flex items-center justify-between px-6 py-2.5 transition-colors",
                    "border-b border-accent-ink/15",
                    isActive
                      ? "bg-accent-ink text-accent"
                      : "text-accent-ink hover:bg-accent-ink/[0.14]"
                  )
                }
              >
                {({ isActive }) => (
                  <>
                    <span>{t(`nav.${key}`)}</span>
                    {isActive && <span>→</span>}
                  </>
                )}
              </NavLink>
            ))}
          </nav>

          <div className="flex items-stretch border-t border-accent-ink/15">
            <NavLink
              to="/settings"
              className={({ isActive }) =>
                clsx(
                  "flex-1 px-6 py-3.5 text-2xs font-bold tracking-[0.14em] transition-colors",
                  isActive ? "bg-accent-ink text-accent" : "text-accent-ink hover:bg-accent-ink/[0.14]"
                )
              }
            >
              {t("nav.settings").toUpperCase()} · FR/EN
            </NavLink>
            <button
              onClick={toggleTheme}
              title={theme === "dark" ? t("settings.theme_light") : t("settings.theme_dark")}
              className="flex w-12 items-center justify-center border-l border-accent-ink/15 text-accent-ink transition-colors hover:bg-accent-ink/[0.14]"
            >
              {theme === "dark" ? <Sun size={16} /> : <Moon size={16} />}
            </button>
          </div>
        </aside>

        {/* Main canvas */}
        <main className="scanlines flex min-w-0 flex-1 flex-col overflow-hidden bg-bg">
          <UpdateBanner />
          <div className="flex-1 overflow-y-auto">
            <Outlet />
          </div>
        </main>
      </div>
    </div>
  );
}

export function PageHeader({
  title,
  subtitle,
  kicker,
  actions,
}: {
  title: string;
  subtitle?: string;
  kicker?: string;
  actions?: React.ReactNode;
}) {
  return (
    <div className="flex items-end justify-between gap-4 border-b-2 border-border px-8 pb-5 pt-7">
      <div>
        {kicker && <div className="kicker mb-2">{kicker}</div>}
        <h1 className="text-[40px] font-black leading-[0.95] tracking-tightest">
          {title}
          <span className="text-accent">.</span>
        </h1>
        {subtitle && (
          <p className="mt-2 text-2xs font-semibold uppercase tracking-widest text-fg-subtle">
            {subtitle}
          </p>
        )}
      </div>
      {actions && <div className="flex items-center gap-2 pb-1">{actions}</div>}
    </div>
  );
}

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
      <div className="flex h-14 w-14 items-center justify-center border-2 border-border text-fg-faint">
        <Icon size={24} />
      </div>
      <div>
        <div className="text-sm font-bold uppercase tracking-wide text-fg">{title}</div>
        {hint && <div className="mt-1 text-sm text-fg-subtle">{hint}</div>}
      </div>
      {action}
    </div>
  );
}
