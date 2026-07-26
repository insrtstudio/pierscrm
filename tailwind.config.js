/** @type {import('tailwindcss').Config} */
export default {
  content: ["./index.html", "./src/**/*.{ts,tsx}"],
  darkMode: "class",
  theme: {
    extend: {
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
      },
      fontFamily: {
        // Archivo everywhere — the Modernist system's single family
        sans: ["Archivo Variable", "Archivo", "system-ui", "sans-serif"],
        heading: ["Archivo Variable", "Archivo", "system-ui", "sans-serif"],
        mono: ["ui-monospace", "SFMono-Regular", "Menlo", "monospace"],
      },
      fontSize: {
        "2xs": ["0.6875rem", { lineHeight: "1rem" }],
      },
      letterSpacing: {
        tightest: "-0.05em",
      },
      borderRadius: {
        none: "0",
        sm: "0",
        DEFAULT: "0",
        md: "0",
        lg: "0",
        xl: "0",
        "2xl": "0",
        "3xl": "0",
        full: "0",
      },
      boxShadow: {
        xs: "0 1px 2px color-mix(in srgb, #000 40%, transparent)",
        soft: "0 1px 2px color-mix(in srgb, #000 30%, transparent)",
        card: "0 3px 14px color-mix(in srgb, #000 40%, transparent)",
        pop: "0 12px 40px color-mix(in srgb, #000 55%, transparent)",
        glow: "0 0 24px rgb(var(--accent) / 0.35)",
        "glow-strong": "0 0 30px rgb(var(--accent) / 0.55)",
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
        ticker: "ticker 30s linear infinite",
        livepulse: "livepulse 1.4s ease-in-out infinite",
      },
    },
  },
  plugins: [],
};
