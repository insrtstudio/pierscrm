import { getCurrentWindow } from "@tauri-apps/api/window";

export type Theme = "light" | "dark";

export function getTheme(): Theme {
  return (localStorage.getItem("theme") as Theme) || "light";
}

/// The sidebar glass is native macOS vibrancy: its material follows the
/// window's appearance, not our CSS class. Pin the window to the app theme so
/// paper gets light glass and ink gets dark glass whatever the system says.
export function syncNativeTheme(theme: Theme = getTheme()) {
  try {
    getCurrentWindow()
      .setTheme(theme)
      .catch(() => {});
  } catch {
    // Plain browser (vite dev without Tauri): no native window.
  }
}

export function applyTheme(theme: Theme) {
  localStorage.setItem("theme", theme);
  document.documentElement.classList.toggle("dark", theme === "dark");
  syncNativeTheme(theme);
  // let listeners (sidebar toggle, settings) sync their local state
  window.dispatchEvent(new CustomEvent("app:theme", { detail: theme }));
}

export function toggleTheme() {
  applyTheme(getTheme() === "dark" ? "light" : "dark");
}
