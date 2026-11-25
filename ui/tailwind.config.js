import animate from "tailwindcss-animate";
import containerQueries from "@tailwindcss/container-queries";

// Colour tokens follow the docs/plan.html mockup palette. CSS variables (HSL triplets) live in
// src/index.css and swap for dark mode (prefers-color-scheme, or data-theme on <html>).
const v = (name) => `hsl(var(--${name}) / <alpha-value>)`;

/** @type {import('tailwindcss').Config} */
export default {
  darkMode: "media",
  content: { relative: true, files: ["./index.html", "./src/**/*.{ts,tsx}"] },
  theme: {
    extend: {
      fontFamily: {
        sans: ['"IBM Plex Sans"', "system-ui", "-apple-system", '"Segoe UI"', "sans-serif"],
        mono: ['"IBM Plex Mono"', "ui-monospace", '"SF Mono"', "Menlo", "monospace"],
      },
      fontSize: {
        "2xs": ["11px", "1.4"],
      },
      colors: {
        // shadcn semantic names
        border: v("line"),
        input: v("line"),
        ring: v("brand"),
        background: v("bg"),
        foreground: v("fg"),
        primary: { DEFAULT: v("brand"), foreground: v("surface") },
        secondary: { DEFAULT: v("surface-2"), foreground: v("fg") },
        destructive: { DEFAULT: v("red"), foreground: v("surface") },
        muted: { DEFAULT: v("surface-2"), foreground: v("muted") },
        accent: { DEFAULT: v("surface-2"), foreground: v("fg") },
        popover: { DEFAULT: v("surface"), foreground: v("fg") },
        card: { DEFAULT: v("surface"), foreground: v("fg") },
        // mockup palette names
        surface: { DEFAULT: v("surface"), 2: v("surface-2") },
        chrome: v("chrome"),
        line: v("line"),
        codebg: v("code-bg"),
        brand: { DEFAULT: v("brand"), soft: v("brand-soft") },
        red: { DEFAULT: v("red"), soft: v("red-soft") },
        orange: { DEFAULT: v("orange"), soft: v("orange-soft") },
        yellow: { DEFAULT: v("yellow"), soft: v("yellow-soft") },
        green: { DEFAULT: v("green"), soft: v("green-soft") },
        diff: { add: v("add"), del: v("del") },
      },
      borderRadius: {
        lg: "var(--radius)",
        md: "calc(var(--radius) - 2px)",
        sm: "calc(var(--radius) - 4px)",
      },
    },
  },
  plugins: [animate, containerQueries],
};
