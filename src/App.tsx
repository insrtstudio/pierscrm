import { lazy, Suspense } from "react";
import { HashRouter, Navigate, Route, Routes, useLocation } from "react-router-dom";
import { Layout } from "./components/Layout";
import { ReplySync } from "./components/ReplySync";
import { ReplyDeskWatcher } from "./components/ReplyDesk";

import { Today } from "./pages/Today";

// Lazy-load secondary routes so the initial payload stays tiny and each screen's
// code is fetched on demand: faster cold start, lower memory footprint. The
// multi-page sections share one chunk (Sections.tsx).
const section = <K extends string>(name: K) =>
  lazy(() => import("./pages/Sections").then((m) => ({ default: (m as unknown as Record<K, React.ComponentType>)[name] })));
const AgendaSection = section("AgendaSection");
const Prospection = section("Prospection");
const ArtistsSection = section("ArtistsSection");
const Results = section("Results");
const Admin = section("Admin");
const ArtistDetail = lazy(() =>
  import("./pages/ArtistDetail").then((m) => ({ default: m.ArtistDetail }))
);
const Import = lazy(() => import("./pages/Import").then((m) => ({ default: m.Import })));
const Emails = lazy(() => import("./pages/Emails").then((m) => ({ default: m.Emails })));
const ResidencyPage = lazy(() =>
  import("./pages/Residency").then((m) => ({ default: m.ResidencyPage }))
);
const Settings = lazy(() => import("./pages/Settings").then((m) => ({ default: m.Settings })));

function Fallback() {
  return (
    <div className="flex h-full items-center justify-center">
      <div className="h-0.5 w-24 overflow-hidden rounded-full bg-muted">
        <div className="h-full w-1/2 animate-ticker bg-accent" />
      </div>
    </div>
  );
}

/// Former URLs (v0.22 and before), kept so old links, notifications and
/// muscle memory still land on the right tab. The query string is carried
/// over (?event=, ?dossier=, ?item=, ?view=...).
const OUTBOX_CAMPAIGN_TABS = ["followups", "compose", "log", "templates", "bulk"];
const REDIRECTS: [string, (tab: string | null) => [string, string | null]][] = [
  ["kpis", () => ["/results", "goals"]],
  ["timeline", () => ["/agenda", "planning"]],
  ["venues", () => ["/contacts", "venues"]],
  ["radar", () => ["/contacts", "curators"]],
  ["discovery", (tab) => ["/contacts", tab === "curators" ? "curators" : "venues"]],
  ["pulse", () => ["/artists", "audience"]],
  ["audience", () => ["/artists", "audience"]],
  ["budget", () => ["/admin", null]],
  ["visa", () => ["/admin", "visa"]],
  ["emails", (tab) => (tab && OUTBOX_CAMPAIGN_TABS.includes(tab) ? ["/campaigns", tab] : ["/replies", tab])],
  ["outbox", (tab) => (tab && OUTBOX_CAMPAIGN_TABS.includes(tab) ? ["/campaigns", tab] : ["/replies", tab])],
];

function Redirect({ to }: { to: (tab: string | null) => [string, string | null] }) {
  const loc = useLocation();
  const params = new URLSearchParams(loc.search);
  const [path, tab] = to(params.get("tab"));
  if (tab) params.set("tab", tab);
  else params.delete("tab");
  const q = params.toString();
  return <Navigate to={q ? `${path}?${q}` : path} replace />;
}

const page = (el: React.ReactNode) => <Suspense fallback={<Fallback />}>{el}</Suspense>;

export function App() {
  return (
    <HashRouter>
      {/* Background IMAP reply detection, every 10 minutes while the app is open. */}
      <ReplySync />
      {/* "Réponses à traiter": refresh on backend passes, open a notified conversation. */}
      <ReplyDeskWatcher />
      <Routes>
        <Route element={<Layout />}>
          <Route index element={<Today />} />
          <Route path="replies" element={page(<Emails section="replies" />)} />
          <Route path="agenda" element={page(<AgendaSection />)} />
          <Route path="contacts" element={page(<Prospection />)} />
          <Route path="campaigns" element={page(<Emails section="campaigns" />)} />
          <Route path="campaigns/:id" element={page(<ResidencyPage />)} />
          <Route path="artists" element={page(<ArtistsSection />)} />
          <Route path="artists/:id" element={page(<ArtistDetail />)} />
          <Route path="results" element={page(<Results />)} />
          <Route path="admin" element={page(<Admin />)} />
          <Route path="import" element={page(<Import />)} />
          <Route path="settings" element={page(<Settings />)} />
          {REDIRECTS.map(([from, to]) => (
            <Route key={from} path={from} element={<Redirect to={to} />} />
          ))}
          <Route path="*" element={<Navigate to="/" replace />} />
        </Route>
      </Routes>
    </HashRouter>
  );
}
