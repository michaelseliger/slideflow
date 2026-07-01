/** @type {import('tailwindcss').Config} */
export default {
  darkMode: "class",
  content: ["./index.html", "./src/**/*.{ts,tsx}"],
  theme: {
    extend: {
      colors: {
        // System accent, overridable at runtime via the --accent CSS var.
        accent: {
          DEFAULT: "rgb(var(--accent-rgb) / <alpha-value>)",
          soft: "rgb(var(--accent-rgb) / 0.10)",
        },
        // Semantic surface greys — mapped to CSS vars so dark mode is a
        // single class flip on <html>.
        canvas: "rgb(var(--canvas-rgb) / <alpha-value>)",
        surface: "rgb(var(--surface-rgb) / <alpha-value>)",
        elevated: "rgb(var(--elevated-rgb) / <alpha-value>)",
        hairline: "rgb(var(--hairline-rgb) / <alpha-value>)",
        ink: "rgb(var(--ink-rgb) / <alpha-value>)",
        subtle: "rgb(var(--subtle-rgb) / <alpha-value>)",
      },
      fontFamily: {
        sans: [
          "-apple-system",
          "BlinkMacSystemFont",
          "SF Pro Text",
          "SF Pro Display",
          "Helvetica Neue",
          "Helvetica",
          "Arial",
          "sans-serif",
        ],
      },
      fontSize: {
        caption: ["11px", { lineHeight: "14px" }],
        body: ["13px", { lineHeight: "18px" }],
        title: ["15px", { lineHeight: "20px" }],
        heading: ["17px", { lineHeight: "22px" }],
      },
      borderRadius: {
        card: "8px",
        control: "6px",
      },
      boxShadow: {
        tile: "0 1px 2px rgb(0 0 0 / 0.06), 0 1px 3px rgb(0 0 0 / 0.08)",
        "tile-hover":
          "0 6px 16px rgb(0 0 0 / 0.14), 0 2px 6px rgb(0 0 0 / 0.10)",
        peek: "0 24px 60px rgb(0 0 0 / 0.35)",
      },
      transitionTimingFunction: {
        spring: "cubic-bezier(0.34, 1.56, 0.64, 1)",
      },
    },
  },
  plugins: [],
};
