import type { Config } from "tailwindcss";

export default {
  darkMode: ["class"],
  content: ["./index.html", "./src/**/*.{ts,tsx}"],
  theme: {
    extend: {
      colors: {
        brand: { DEFAULT: "#4f46e5", dark: "#3730a3", soft: "#eef2ff" },
        surface: "#f8fafc",
        ink: "#0f172a",
        danger: "#dc2626",
      },
      fontFamily: {
        sans: ["Inter", "system-ui", "sans-serif"],
        display: ["Sora", "Inter", "sans-serif"],
      },
      borderRadius: { lg: "12px", md: "8px", sm: "4px" },
    },
  },
} satisfies Config;
