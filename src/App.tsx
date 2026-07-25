import { HashRouter, Route, Routes } from "react-router-dom";
import { Layout } from "./components/Layout";
import { Dashboard } from "./pages/Dashboard";
import { Artists } from "./pages/Artists";
import { ArtistDetail } from "./pages/ArtistDetail";
import { Contacts } from "./pages/Contacts";
import { Import } from "./pages/Import";
import { Emails } from "./pages/Emails";
import { Budget } from "./pages/Budget";
import { Timeline } from "./pages/Timeline";
import { Kpis } from "./pages/Kpis";
import { Visa } from "./pages/Visa";
import { Settings } from "./pages/Settings";

export function App() {
  return (
    <HashRouter>
      <Routes>
        <Route element={<Layout />}>
          <Route index element={<Dashboard />} />
          <Route path="artists" element={<Artists />} />
          <Route path="artists/:id" element={<ArtistDetail />} />
          <Route path="contacts" element={<Contacts />} />
          <Route path="import" element={<Import />} />
          <Route path="emails" element={<Emails />} />
          <Route path="budget" element={<Budget />} />
          <Route path="timeline" element={<Timeline />} />
          <Route path="kpis" element={<Kpis />} />
          <Route path="visa" element={<Visa />} />
          <Route path="settings" element={<Settings />} />
        </Route>
      </Routes>
    </HashRouter>
  );
}
