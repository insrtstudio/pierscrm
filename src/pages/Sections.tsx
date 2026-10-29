import { useTranslation } from "react-i18next";
import { TabbedPage } from "../components/Layout";
import { Agenda } from "./Agenda";
import { Timeline } from "./Timeline";
import { Venues } from "./Venues";
import { Radar } from "./Radar";

// Nav sections made of several former pages. Each page keeps its own logic;
// TabbedPage shows the section title and its header collapses to its actions.

export function AgendaSection() {
  const { t } = useTranslation();
  return (
    <TabbedPage
      title={t("agenda.title")}
      subtitle={t("agenda.subtitle")}
      tabs={[
        { key: "calendar", label: t("sections.agenda_calendar"), render: () => <Agenda /> },
        { key: "planning", label: t("sections.agenda_planning"), render: () => <Timeline /> },
      ]}
    />
  );
}

export function Discovery() {
  const { t } = useTranslation();
  return (
    <TabbedPage
      title={t("nav.discovery")}
      subtitle={t("sections.discovery_subtitle")}
      tabs={[
        { key: "venues", label: t("sections.discovery_venues"), render: () => <Venues /> },
        { key: "curators", label: t("sections.discovery_curators"), render: () => <Radar /> },
      ]}
    />
  );
}
