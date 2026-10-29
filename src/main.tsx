import React from "react";
import ReactDOM from "react-dom/client";
import { MutationCache, QueryClient, QueryClientProvider } from "@tanstack/react-query";
import "@fontsource-variable/archivo/wdth.css";
import "@fontsource-variable/geist-mono";
import "@fontsource/anton";
import "./i18n";
import "./styles.css";
import { App } from "./App";
import { ToastProvider } from "./components/ui";

// Apply saved theme before first paint. Paper is the default, like the
// insrt.studio back office.
const theme = localStorage.getItem("theme") || "light";
document.documentElement.classList.toggle("dark", theme === "dark");

const queryClient = new QueryClient({
  // A mutation without its own onError reports through the global toast instead
  // of failing silently (the UI looked updated until the next refetch).
  mutationCache: new MutationCache({
    onError: (error, _vars, _ctx, mutation) => {
      if (!mutation.options.onError)
        window.dispatchEvent(new CustomEvent("app:error", { detail: error }));
    },
  }),
  defaultOptions: {
    queries: {
      retry: false,
      refetchOnWindowFocus: false,
      staleTime: 30_000,
      gcTime: 5 * 60_000,
    },
  },
});

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <QueryClientProvider client={queryClient}>
      <ToastProvider>
        <App />
      </ToastProvider>
    </QueryClientProvider>
  </React.StrictMode>
);
