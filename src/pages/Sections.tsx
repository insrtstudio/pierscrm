import { useTranslation } from "react-i18next";
import { TabbedPage } from "../components/Layout";
import { Agenda } from "./Agenda";
import { Timeline } from "./Timeline";
import { Venues } from "./Venues";
import { Radar } from "./Radar";
import { Contacts } from "./Contacts";
import { Artists } from "./Artists";
import { Pulse } from "./Pulse";
import { Dashboard } from "./Dashboard";
import { Kpis } from "./Kpis";
import { Funnel } from "./Funnel";
import { Budget } from "./Budget";
import { Visa } from "./Visa";

// Nav sections made of several screens. Each screen keeps its own logic;
// TabbedPage shows the section title and the screen's header collapses to its
// actions. Tab keys of former sections are kept so old deep links still land.

export function AgendaSection() {
  const { t } = useTranslation();
  return (
    <TabbedPage
      title={t("nav.agenda")}
      tabs={[
        { key: "calendar", label: t("sections.agenda_calendar"), render: () => <Agenda /> },
        { key: "planning", label: t("sections.agenda_planning"), render: () => <Timeline /> },
      ]}
    />
  );
}

/// Démarchage + Repérage: who to write to, from the pipeline to the places and
/// curators still to discover.
export function Prospection() {
  const { t } = useTranslation();
  return (
    <TabbedPage
      title={t("nav.prospect")}
      tabs={[
        { key: "pipeline", label: t("sections.prospect_pipeline"), render: () => <Contacts /> },
        { key: "venues", label: t("sections.discovery_venues"), render: () => <Venues /> },
        { key: "curators", label: t("sections.discovery_curators"), render: () => <Radar /> },
      ]}
    />
  );
}

export function ArtistsSection() {
  const { t } = useTranslation();
  return (
    <TabbedPage
      title={t("nav.artists")}
      tabs={[
        { key: "roster", label: t("sections.artists_roster"), render: () => <Artists /> },
        { key: "audience", label: t("nav.audience"), render: () => <Pulse /> },
      ]}
    />
  );
}

/// Everything that measures: the overview, the funnel, the goals.
export function Results() {
  const { t } = useTranslation();
  return (
    <TabbedPage
      title={t("nav.results")}
      tabs={[
        { key: "overview", label: t("sections.home_overview"), render: () => <Dashboard /> },
        { key: "funnel", label: t("sections.home_funnel"), render: () => <Funnel /> },
        { key: "goals", label: t("sections.home_goals"), render: () => <Kpis /> },
      ]}
    />
  );
}

export function Admin() {
  const { t } = useTranslation();
  return (
    <TabbedPage
      title={t("nav.admin")}
      tabs={[
        { key: "budget", label: t("nav.budget"), render: () => <Budget /> },
        { key: "visa", label: t("nav.visa"), render: () => <Visa /> },
      ]}
    />
  );
}
