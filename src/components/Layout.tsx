import { createContext, useContext, useEffect, useMemo, useState } from "react";
import { createPortal } from "react-dom";
import { NavLink, Outlet, useNavigate, useSearchParams } from "react-router-dom";
import { useTranslation } from "react-i18next";
import { useQuery } from "@tanstack/react-query";
import { listen } from "@tauri-apps/api/event";
import clsx from "clsx";
import {
  Sun,
  Moon,
  RefreshCw,
  AlertTriangle,
  Home as HomeIcon,
  CalendarDays,
  Mic2,
  Activity,
  Users,
  Radar as RadarIcon,
  Megaphone,
  Send,
  Wallet,
  Stamp,
  Settings as SettingsIcon,
  PanelLeftClose,
  PanelLeftOpen,
  type LucideIcon,
} from "lucide-react";
import { listArtists, listEvents } from "../lib/api";
import { getTheme, toggleTheme, type Theme } from "../lib/theme";
import { UpdateBanner } from "./UpdateBanner";
import { DeskBadge } from "./ReplyDesk";
import { DeliverabilityBanner } from "./Deliverability";
import { emitIntent } from "../lib/hooks";
import { parseDay, today } from "../lib/format";
import logoBlack from "../assets/brand/insrt-logo-black.png";
import logoWhite from "../assets/brand/insrt-logo-white.png";

type NavItem = { to: string; key: string; icon: LucideIcon; end?: boolean };
type NavGroup = { key: string; tone: string; items: NavItem[] };

// Grouped like the insrt.studio back office (AdminNav): a mono eyebrow with a
// coloured dot per universe. The native "Aller à" menu (menu.rs) follows this
// order for its Cmd+1..9 shortcuts.
export const NAV_GROUPS: NavGroup[] = [
  {
    key: "steer",
    tone: "bg-accent",
    items: [
      { to: "/", key: "home", icon: HomeIcon, end: true },
      { to: "/agenda", key: "agenda", icon: CalendarDays },
    ],
  },
  {
    key: "roster",
    tone: "bg-fg",
    items: [
      { to: "/artists", key: "artists", icon: Mic2 },
      { to: "/audience", key: "audience", icon: Activity },
    ],
  },
  {
    key: "prospect",
    tone: "bg-chrome-dark",
    items: [
      { to: "/contacts", key: "pipeline", icon: Users },
      { to: "/discovery", key: "discovery", icon: RadarIcon },
    ],
  },
  {
    key: "campaigns",
    tone: "bg-warn",
    items: [
      { to: "/campaigns", key: "campaigns", icon: Megaphone },
      { to: "/outbox", key: "outbox", icon: Send },
    ],
  },
  {
    key: "ops",
    tone: "bg-fg-faint",
    items: [
      { to: "/budget", key: "budget", icon: Wallet },
      { to: "/visa", key: "visa", icon: Stamp },
    ],
  },
];

const NAV_PREF_KEY = "nav_collapsed";
// Below this width the sidebar always folds to an icon rail: content first.
const NARROW_QUERY = "(max-width: 1179px)";

/// Sidebar state: the user's explicit choice when there is one, otherwise
/// folded on narrow windows and open on wide ones.
function useNavCollapsed(): [boolean, boolean, () => void] {
  const read = () => {
    try {
      const v = localStorage.getItem(NAV_PREF_KEY);
      return v == null ? null : v === "1";
    } catch {
      return null;
    }
  };
  const [pref, setPref] = useState<boolean | null>(read);
  const [narrow, setNarrow] = useState(() => window.matchMedia(NARROW_QUERY).matches);
  useEffect(() => {
    const mq = window.matchMedia(NARROW_QUERY);
    const h = (e: MediaQueryListEvent) => setNarrow(e.matches);
    mq.addEventListener("change", h);
    return () => mq.removeEventListener("change", h);
  }, []);
  const collapsed = narrow || (pref ?? false);
  const toggle = () => {
    const next = !collapsed;
    setPref(next);
    try {
      localStorage.setItem(NAV_PREF_KEY, next ? "1" : "0");
    } catch {
      // Private window: the choice just won't survive a restart.
    }
  };
  return [collapsed, narrow, toggle];
}

function useMenuActions() {
  const navigate = useNavigate();
  useEffect(() => {
    const routes: Record<string, string> = {
      "nav:home": "/",
      "nav:agenda": "/agenda",
      "nav:artists": "/artists",
      "nav:audience": "/audience",
      "nav:pipeline": "/contacts",
      "nav:discovery": "/discovery",
      "nav:campaigns": "/campaigns",
      "nav:outbox": "/outbox",
      "nav:budget": "/budget",
      "nav:visa": "/visa",
      "nav:import": "/import",
      "nav:settings": "/settings",
      "check-updates": "/settings",
      "new:artist": "/artists",
      "new:contact": "/contacts",
      "new:event": "/agenda",
      "new:campaign": "/campaigns",
    };
    const un = listen<string>("menu-action", (e) => {
      const id = e.payload;
      const route = routes[id];
      if (route) navigate(route);
      if (id.startsWith("new:") || id === "check-updates") emitIntent(id);
    });
    return () => {
      un.then((f) => f());
    };
  }, [navigate]);
}

/// Top bar: the macOS drag region (traffic lights sit on its left) carrying a
/// discreet mono marquee of the next events, like the site's marquee.
function TopBar() {
  const { t } = useTranslation();
  const day = today();
  const { data: events = [] } = useQuery({
    queryKey: ["events", "ticker", day],
    queryFn: () => listEvents({ from: day }),
  });
  const { data: artists = [] } = useQuery({ queryKey: ["artists"], queryFn: listArtists });

  const months = t("agenda.months").split("_");
  const parts = useMemo(
    () =>
      events
        .filter((e) => e.status !== "cancelled")
        .slice(0, 12)
        .map((e) => {
          const d = parseDay(e.date);
          const date = d
            ? `${String(d.getDate()).padStart(2, "0")} ${months[d.getMonth()]?.slice(0, 3)}`
            : e.date;
          const artist = artists.find((a) => a.id === e.artist_id)?.name;
          return [artist || e.title, date, e.start_time].filter(Boolean).join(" · ");
        }),
    [events, artists, months]
  );

  return (
    <div
      data-tauri-drag-region
      className="relative flex h-9 shrink-0 items-center border-b border-border bg-bg pl-[88px]"
    >
      {/* The edge fade is for the scrolling ticker only: on the static empty
          message it ate the first letters. */}
      <div
        className={clsx(
          "pointer-events-none relative min-w-0 flex-1 overflow-hidden",
          parts.length > 0 && "[mask-image:linear-gradient(90deg,transparent,#000_6%,#000_94%,transparent)]"
        )}
      >
        {parts.length > 0 ? (
          <div className="flex w-max animate-ticker whitespace-nowrap">
            {[0, 1].map((k) => (
              <span key={k} className="eyebrow flex items-center">
                {parts.map((p, i) => (
                  <span key={i} className="flex items-center">
                    <span className="mx-5 h-1.5 w-1.5 rounded-full bg-accent" />
                    {p}
                  </span>
                ))}
              </span>
            ))}
          </div>
        ) : (
          <span className="eyebrow">{t("layout.no_upcoming")}</span>
        )}
      </div>
      <span className="pointer-events-none shrink-0 px-5 eyebrow text-fg-faint">PiersCRM</span>
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

  const [collapsed, narrow, toggleNav] = useNavCollapsed();

  const item = (isActive: boolean) =>
    clsx(
      "relative flex items-center rounded py-1.5 text-[13.5px] transition-colors outline-none",
      "focus-visible:ring-2 focus-visible:ring-accent",
      collapsed ? "justify-center px-0 py-2" : "gap-2.5 px-3",
      isActive ? "bg-surface-2 font-medium text-fg" : "text-fg-subtle hover:bg-surface-2 hover:text-fg"
    );

  return (
    <div className="flex h-screen w-screen flex-col overflow-hidden bg-bg text-fg">
      <TopBar />
      <div className="flex min-h-0 flex-1">
        <aside
          className={clsx(
            "flex shrink-0 flex-col border-r border-border bg-bg transition-[width] duration-200",
            collapsed ? "w-[60px]" : "w-[220px]"
          )}
        >
          <NavLink
            to="/"
            className={clsx("block", collapsed ? "px-2.5 pb-5 pt-4" : "px-6 pb-6 pt-5")}
            aria-label="Insrt"
          >
            <img
              src={theme === "dark" ? logoWhite : logoBlack}
              alt="Insrt"
              className={clsx("w-auto", collapsed ? "mx-auto h-4" : "h-7")}
              draggable={false}
            />
            {!collapsed && <div className="eyebrow mt-2 text-[10px]">{t("layout.tagline")}</div>}
          </NavLink>

          <nav className={clsx("flex-1 overflow-y-auto pb-6", collapsed ? "space-y-3 px-2" : "space-y-6 px-3")}>
            {NAV_GROUPS.map((g) => (
              <div key={g.key}>
                {collapsed ? (
                  // The group survives folding as its coloured dot.
                  <div className="mb-1 flex justify-center" title={t(`nav_group.${g.key}`)}>
                    <span className={clsx("h-1.5 w-1.5 rounded-full", g.tone)} />
                  </div>
                ) : (
                  <p className="eyebrow mb-1.5 flex items-center gap-2 px-3 text-[10.5px]">
                    <span className={clsx("h-2 w-2 rounded-full", g.tone)} />
                    {t(`nav_group.${g.key}`)}
                  </p>
                )}
                <ul className="space-y-0.5">
                  {g.items.map(({ to, key, end, icon: Icon }) => (
                    <li key={to}>
                      <NavLink
                        to={to}
                        end={end}
                        className={({ isActive }) => item(isActive)}
                        title={collapsed ? t(`nav.${key}`) : undefined}
                        aria-label={t(`nav.${key}`)}
                      >
                        <Icon size={collapsed ? 17 : 15} strokeWidth={1.75} className="shrink-0" />
                        {!collapsed && t(`nav.${key}`)}
                        {key === "outbox" && <DeskBadge collapsed={collapsed} />}
                      </NavLink>
                    </li>
                  ))}
                </ul>
              </div>
            ))}
          </nav>

          <div
            className={clsx(
              "flex border-t border-border p-3",
              collapsed ? "flex-col items-center gap-1 px-2" : "items-center gap-1"
            )}
          >
            <NavLink
              to="/settings"
              className={({ isActive }) => clsx(item(isActive), !collapsed && "flex-1", collapsed && "w-full")}
              title={collapsed ? t("nav.settings") : undefined}
              aria-label={t("nav.settings")}
            >
              <SettingsIcon size={collapsed ? 17 : 15} strokeWidth={1.75} className="shrink-0" />
              {!collapsed && t("nav.settings")}
            </NavLink>
            <button
              onClick={toggleTheme}
              title={theme === "dark" ? t("settings.theme_light") : t("settings.theme_dark")}
              aria-label={theme === "dark" ? t("settings.theme_light") : t("settings.theme_dark")}
              className="btn-icon"
            >
              {theme === "dark" ? <Sun size={15} /> : <Moon size={15} />}
            </button>
            {/* On a narrow window the rail is forced: no toggle to offer. */}
            {!narrow && (
              <button
                onClick={toggleNav}
                title={collapsed ? t("layout.expand") : t("layout.collapse")}
                aria-label={collapsed ? t("layout.expand") : t("layout.collapse")}
                className="btn-icon"
              >
                {collapsed ? <PanelLeftOpen size={15} /> : <PanelLeftClose size={15} />}
              </button>
            )}
          </div>
        </aside>

        <main className="flex min-w-0 flex-1 flex-col overflow-hidden bg-bg">
          <UpdateBanner />
          <DeliverabilityBanner />
          <div className="flex-1 overflow-y-auto">
            <Outlet />
          </div>
        </main>
      </div>
    </div>
  );
}

/// Inside a TabbedPage, a page's own header collapses to its action buttons,
/// portalled onto the parent's tab row: the parent already shows the title.
/// `false` = not embedded; `null` = embedded, slot not mounted yet.
const EmbeddedCtx = createContext<HTMLElement | null | false>(false);

export function PageHeader({
  title,
  subtitle,
  kicker,
  actions,
  children,
}: {
  title: string;
  subtitle?: string;
  kicker?: string;
  actions?: React.ReactNode;
  /** Rendered under the title row (tabs). */
  children?: React.ReactNode;
}) {
  const slot = useContext(EmbeddedCtx);
  if (slot !== false) return slot && actions ? createPortal(actions, slot) : null;
  return (
    <div className="border-b border-border px-page pt-7">
      <div className="flex flex-wrap items-end justify-between gap-4 pb-5">
        <div className="min-w-0">
          {kicker && <div className="kicker mb-3">{kicker}</div>}
          <h1 className="font-display text-[30px] xl:text-[38px]">{title}</h1>
          {subtitle && <p className="mt-2.5 max-w-2xl text-sm text-fg-subtle">{subtitle}</p>}
        </div>
        {actions && <div className="flex shrink-0 flex-wrap items-center justify-end gap-2 pb-1">{actions}</div>}
      </div>
      {children}
    </div>
  );
}

export type PageTab = { key: string; label: string; render: () => React.ReactNode };

/// A section made of several former pages (Accueil = overview + goals,
/// Découverte = venues + curators...). The active tab lives in `?tab=` so the
/// menu and links can deep-link into it.
export function TabbedPage({
  title,
  subtitle,
  kicker,
  tabs,
}: {
  title: string;
  subtitle?: string;
  kicker?: string;
  tabs: PageTab[];
}) {
  const [params, setParams] = useSearchParams();
  const [slot, setSlot] = useState<HTMLElement | null>(null);
  const active = tabs.find((tb) => tb.key === params.get("tab")) ?? tabs[0];
  return (
    <div>
      <PageHeader title={title} subtitle={subtitle} kicker={kicker}>
        <div className="flex flex-wrap items-end justify-between gap-x-4">
        <div role="tablist" className="-mb-px flex gap-6 overflow-x-auto">
          {tabs.map((tb) => (
            <button
              key={tb.key}
              role="tab"
              aria-selected={tb.key === active.key}
              onClick={() => setParams(tb.key === tabs[0].key ? {} : { tab: tb.key }, { replace: true })}
              className={clsx(
                "border-b pb-2.5 font-mono text-[11px] uppercase tracking-[0.14em] transition-colors",
                tb.key === active.key
                  ? "border-accent text-fg"
                  : "border-transparent text-fg-subtle hover:text-fg"
              )}
            >
              {tb.label}
            </button>
          ))}
        </div>
        <div ref={setSlot} className="flex flex-wrap items-center justify-end gap-2 pb-2.5" />
        </div>
      </PageHeader>
      <EmbeddedCtx.Provider value={slot}>
        <div key={active.key}>{active.render()}</div>
      </EmbeddedCtx.Provider>
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
      <div className="flex h-14 w-14 items-center justify-center rounded-full border border-border text-fg-faint">
        <Icon size={22} strokeWidth={1.5} />
      </div>
      <div>
        <div className="eyebrow text-fg">{title}</div>
        {hint && <div className="mt-1 text-sm text-fg-subtle">{hint}</div>}
      </div>
      {action}
    </div>
  );
}

export function Loader({ label }: { label?: string }) {
  const { t } = useTranslation();
  return (
    <div
      className="flex items-center justify-center gap-2 py-16 text-sm text-fg-subtle"
      role="status"
      aria-live="polite"
    >
      <RefreshCw size={16} className="animate-spin" />
      {label ?? t("common.loading")}
    </div>
  );
}

export function ErrorState({ onRetry }: { onRetry?: () => void }) {
  const { t } = useTranslation();
  return (
    <div
      className="flex flex-col items-center justify-center gap-3 py-16 text-center"
      role="alert"
    >
      <div className="flex h-12 w-12 items-center justify-center rounded-full border border-danger/40 text-danger">
        <AlertTriangle size={22} />
      </div>
      <div className="text-sm text-fg-subtle">{t("common.load_error")}</div>
      {onRetry && (
        <button className="btn-outline" onClick={onRetry}>
          {t("common.retry")}
        </button>
      )}
    </div>
  );
}
