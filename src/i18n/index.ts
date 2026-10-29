import i18n from "i18next";
import { initReactI18next } from "react-i18next";
import { invoke } from "@tauri-apps/api/core";
import { fr } from "./fr";
import { en } from "./en";

const saved = localStorage.getItem("lang") || "fr";

i18n.use(initReactI18next).init({
  resources: {
    fr: { translation: fr },
    en: { translation: en },
  },
  lng: saved,
  fallbackLng: "en",
  interpolation: { escapeValue: false },
});

// The native menu bar is built in French at launch; keep it in the UI language.
const syncMenu = (lng: string) =>
  invoke("set_menu_lang", { lang: lng }).catch(() => {});
if (!saved.startsWith("fr")) syncMenu(saved);
i18n.on("languageChanged", syncMenu);

export default i18n;
