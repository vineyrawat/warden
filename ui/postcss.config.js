import { fileURLToPath } from "node:url";

// Resolve the Tailwind config from this folder, not from process.cwd(), so the UI builds the
// same way whether it is started from ui/, the repo root (tauri dev) or an external launcher.
export default {
  plugins: {
    tailwindcss: { config: fileURLToPath(new URL("./tailwind.config.js", import.meta.url)) },
    autoprefixer: {},
  },
};
