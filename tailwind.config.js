/** @type {import('tailwindcss').Config} */
export default {
  content: ["./index.html", "./src/**/*.{ts,tsx}"],
  darkMode: "class",
  theme: {
    extend: {
      // Page gutter that breathes with the window (see --page-x).
      spacing: { page: "var(--page-x)" },
      colors: {
        bg: "rgb(var(--bg) / <alpha-value>)",
        surface: "rgb(var(--surface) / <alpha-value>)",
        "surface-2": "rgb(var(--surface-2) / <alpha-value>)",
        elevated: "rgb(var(--elevated) / <alpha-value>)",
        border: "rgb(var(--border) / <alpha-value>)",
        "border-strong": "rgb(var(--border-strong) / <alpha-value>)",
        muted: "rgb(var(--muted) / <alpha-value>)",
        fg: "rgb(var(--fg) / <alpha-value>)",
        "fg-subtle": "rgb(var(--fg-subtle) / <alpha-value>)",
        "fg-faint": "rgb(var(--fg-faint) / <alpha-value>)",
        accent: "rgb(var(--accent) / <alpha-value>)",
        "accent-fg": "rgb(var(--accent-fg) / <alpha-value>)",
        "accent-soft": "rgb(var(--accent-soft) / <alpha-value>)",
        "accent-2": "rgb(var(--accent-2) / <alpha-value>)",
        "accent-ink": "rgb(var(--accent-ink) / <alpha-value>)",
        chrome: "rgb(var(--chrome) / <alpha-value>)",
        "chrome-dark": "rgb(var(--chrome-dark) / <alpha-value>)",
        ok: "rgb(var(--ok) / <alpha-value>)",
        warn: "rgb(var(--warn) / <alpha-value>)",
        danger: "rgb(var(--danger) / <alpha-value>)",
      },
      fontFamily: {
        // insrt.studio type roles: Overused Grotesk (UI), Archivo (display),
        // Geist Mono (eyebrows, labels, buttons), Anton (big numbers)
        sans: ["Overused Grotesk", "Helvetica Neue", "Arial", "sans-serif"],
        heading: ["Overused Grotesk", "Helvetica Neue", "Arial", "sans-serif"],
        display: ["Archivo Variable", "Arial Black", "sans-serif"],
        mono: ["Geist Mono Variable", "ui-monospace", "Menlo", "monospace"],
        condensed: ["Anton", "Impact", "sans-serif"],
      },
      fontSize: {
        "2xs": ["0.6875rem", { lineHeight: "1rem" }],
      },
      letterSpacing: {
        tightest: "-0.05em",
      },
      boxShadow: {
        xs: "0 1px 2px rgb(0 0 0 / 0.06)",
        soft: "0 1px 3px rgb(0 0 0 / 0.08)",
        card: "0 4px 18px rgb(0 0 0 / 0.10)",
        pop: "0 16px 48px rgb(0 0 0 / 0.22)",
        // the old red glow, neutralised
        glow: "none",
        "glow-strong": "none",
      },
      keyframes: {
        "fade-in": {
          from: { opacity: "0", transform: "translateY(6px)" },
          to: { opacity: "1", transform: "translateY(0)" },
        },
        "scale-in": {
          from: { opacity: "0", transform: "scale(0.98)" },
          to: { opacity: "1", transform: "scale(1)" },
        },
        ticker: {
          from: { transform: "translateX(0)" },
          to: { transform: "translateX(-50%)" },
        },
        livepulse: {
          "0%,100%": { opacity: "1" },
          "50%": { opacity: "0.25" },
        },
      },
      animation: {
        "fade-in": "fade-in 0.22s cubic-bezier(0.16,1,0.3,1)",
        "scale-in": "scale-in 0.16s cubic-bezier(0.16,1,0.3,1)",
        ticker: "ticker 60s linear infinite",
        livepulse: "livepulse 1.4s ease-in-out infinite",
      },
    },
  },
  plugins: [],
};
