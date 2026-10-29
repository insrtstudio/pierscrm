import { useTranslation } from "react-i18next";
import { TabbedPage } from "../components/Layout";
import { Dashboard } from "./Dashboard";
import { Kpis } from "./Kpis";

// Landing section (loaded eagerly): overview + the goals that used to be the
// standalone KPIs page.
export function Home() {
  const { t } = useTranslation();
  return (
    <TabbedPage
      kicker={t("dashboard.kicker")}
      title={t("nav.home")}
      tabs={[
        { key: "overview", label: t("sections.home_overview"), render: () => <Dashboard /> },
        { key: "goals", label: t("sections.home_goals"), render: () => <Kpis /> },
      ]}
    />
  );
}
