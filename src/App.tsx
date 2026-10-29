import { lazy, Suspense } from "react";
import { HashRouter, Navigate, Route, Routes } from "react-router-dom";
import { Layout } from "./components/Layout";

import { Home } from "./pages/Home";

// Lazy-load secondary routes so the initial payload stays tiny and each screen's
// code is fetched on demand: faster cold start, lower memory footprint. The
// multi-page sections share one chunk (Sections.tsx).
const AgendaSection = lazy(() =>
  import("./pages/Sections").then((m) => ({ default: m.AgendaSection }))
);
const Discovery = lazy(() => import("./pages/Sections").then((m) => ({ default: m.Discovery })));
const Artists = lazy(() => import("./pages/Artists").then((m) => ({ default: m.Artists })));
const ArtistDetail = lazy(() =>
  import("./pages/ArtistDetail").then((m) => ({ default: m.ArtistDetail }))
);
const Contacts = lazy(() => import("./pages/Contacts").then((m) => ({ default: m.Contacts })));
const Import = lazy(() => import("./pages/Import").then((m) => ({ default: m.Import })));
const Emails = lazy(() => import("./pages/Emails").then((m) => ({ default: m.Emails })));
const Budget = lazy(() => import("./pages/Budget").then((m) => ({ default: m.Budget })));
const Visa = lazy(() => import("./pages/Visa").then((m) => ({ default: m.Visa })));
const Pulse = lazy(() => import("./pages/Pulse").then((m) => ({ default: m.Pulse })));
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

/// Former URLs, kept so old links and muscle memory still land somewhere.
const REDIRECTS: [string, string][] = [
  ["kpis", "/?tab=goals"],
  ["timeline", "/agenda?tab=planning"],
  ["venues", "/discovery"],
  ["radar", "/discovery?tab=curators"],
  ["pulse", "/audience"],
  ["emails", "/outbox"],
];

const page = (el: React.ReactNode) => <Suspense fallback={<Fallback />}>{el}</Suspense>;

export function App() {
  return (
    <HashRouter>
      <Routes>
        <Route element={<Layout />}>
          <Route index element={page(<Home />)} />
          <Route path="agenda" element={page(<AgendaSection />)} />
          <Route path="artists" element={page(<Artists />)} />
          <Route path="artists/:id" element={page(<ArtistDetail />)} />
          <Route path="audience" element={page(<Pulse />)} />
          <Route path="contacts" element={page(<Contacts />)} />
          <Route path="import" element={page(<Import />)} />
          <Route path="discovery" element={page(<Discovery />)} />
          <Route path="campaigns" element={page(<Emails section="campaigns" />)} />
          <Route path="outbox" element={page(<Emails section="outbox" />)} />
          <Route path="budget" element={page(<Budget />)} />
          <Route path="visa" element={page(<Visa />)} />
          <Route path="settings" element={page(<Settings />)} />
          {REDIRECTS.map(([from, to]) => (
            <Route key={from} path={from} element={<Navigate to={to} replace />} />
          ))}
          <Route path="*" element={<Navigate to="/" replace />} />
        </Route>
      </Routes>
    </HashRouter>
  );
}
