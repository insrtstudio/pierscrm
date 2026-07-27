export type Theme = "light" | "dark";

export function getTheme(): Theme {
  return (localStorage.getItem("theme") as Theme) || "dark";
}

export function applyTheme(theme: Theme) {
  localStorage.setItem("theme", theme);
  document.documentElement.classList.toggle("dark", theme === "dark");
  // let listeners (sidebar toggle, settings) sync their local state
  window.dispatchEvent(new CustomEvent("app:theme", { detail: theme }));
}

export function toggleTheme() {
  applyTheme(getTheme() === "dark" ? "light" : "dark");
}
