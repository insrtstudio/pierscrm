import { useTranslation } from "react-i18next";
import { TabbedPage } from "../components/Layout";
import { Agenda } from "./Agenda";
import { Timeline } from "./Timeline";
import { Actions } from "./Actions";
import { Venues } from "./Venues";
import { Radar } from "./Radar";
import { RoleBanner } from "../components/RoleBanner";

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
        { key: "actions", label: t("sections.agenda_actions"), subtitle: t("sections.agenda_actions_subtitle"), render: () => <Actions /> },
        {
          key: "planning",
          label: t("sections.agenda_planning"),
          subtitle: t("sections.agenda_planning_subtitle"),
          render: () => <Timeline />,
        },
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
      intro={
        <RoleBanner
          id="scouting"
          text={t("scouting.banner")}
          link={t("scouting.banner_link")}
          to="/contacts"
        />
      }
      tabs={[
        { key: "venues", label: t("sections.discovery_venues"), render: () => <Venues /> },
        { key: "curators", label: t("sections.discovery_curators"), render: () => <Radar /> },
      ]}
    />
  );
}
