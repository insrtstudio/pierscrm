import { lazy, Suspense } from "react";
import { HashRouter, Route, Routes } from "react-router-dom";
import { Layout } from "./components/Layout";
import { Dashboard } from "./pages/Dashboard";

// Lazy-load secondary routes so the initial payload stays tiny and each screen's
// code is fetched on demand — faster cold start, lower memory footprint.
const Agenda = lazy(() => import("./pages/Agenda").then((m) => ({ default: m.Agenda })));
const Artists = lazy(() => import("./pages/Artists").then((m) => ({ default: m.Artists })));
const ArtistDetail = lazy(() =>
  import("./pages/ArtistDetail").then((m) => ({ default: m.ArtistDetail }))
);
const Contacts = lazy(() => import("./pages/Contacts").then((m) => ({ default: m.Contacts })));
const Import = lazy(() => import("./pages/Import").then((m) => ({ default: m.Import })));
const Emails = lazy(() => import("./pages/Emails").then((m) => ({ default: m.Emails })));
const Budget = lazy(() => import("./pages/Budget").then((m) => ({ default: m.Budget })));
const Timeline = lazy(() => import("./pages/Timeline").then((m) => ({ default: m.Timeline })));
const Kpis = lazy(() => import("./pages/Kpis").then((m) => ({ default: m.Kpis })));
const Visa = lazy(() => import("./pages/Visa").then((m) => ({ default: m.Visa })));
const Venues = lazy(() => import("./pages/Venues").then((m) => ({ default: m.Venues })));
const Settings = lazy(() => import("./pages/Settings").then((m) => ({ default: m.Settings })));

function Fallback() {
  return (
    <div className="flex h-full items-center justify-center">
      <div className="h-1 w-24 overflow-hidden bg-muted">
        <div className="h-full w-1/2 animate-ticker bg-accent" />
      </div>
    </div>
  );
}

export function App() {
  return (
    <HashRouter>
      <Routes>
        <Route element={<Layout />}>
          <Route index element={<Dashboard />} />
          <Route
            path="agenda"
            element={
              <Suspense fallback={<Fallback />}>
                <Agenda />
              </Suspense>
            }
          />
          <Route
            path="artists"
            element={
              <Suspense fallback={<Fallback />}>
                <Artists />
              </Suspense>
            }
          />
          <Route
            path="artists/:id"
            element={
              <Suspense fallback={<Fallback />}>
                <ArtistDetail />
              </Suspense>
            }
          />
          <Route
            path="contacts"
            element={
              <Suspense fallback={<Fallback />}>
                <Contacts />
              </Suspense>
            }
          />
          <Route
            path="import"
            element={
              <Suspense fallback={<Fallback />}>
                <Import />
              </Suspense>
            }
          />
          <Route
            path="emails"
            element={
              <Suspense fallback={<Fallback />}>
                <Emails />
              </Suspense>
            }
          />
          <Route
            path="budget"
            element={
              <Suspense fallback={<Fallback />}>
                <Budget />
              </Suspense>
            }
          />
          <Route
            path="timeline"
            element={
              <Suspense fallback={<Fallback />}>
                <Timeline />
              </Suspense>
            }
          />
          <Route
            path="kpis"
            element={
              <Suspense fallback={<Fallback />}>
                <Kpis />
              </Suspense>
            }
          />
          <Route
            path="visa"
            element={
              <Suspense fallback={<Fallback />}>
                <Visa />
              </Suspense>
            }
          />
          <Route
            path="venues"
            element={
              <Suspense fallback={<Fallback />}>
                <Venues />
              </Suspense>
            }
          />
          <Route
            path="settings"
            element={
              <Suspense fallback={<Fallback />}>
                <Settings />
              </Suspense>
            }
          />
        </Route>
      </Routes>
    </HashRouter>
  );
}
