import React from "react";
import ReactDOM from "react-dom/client";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import "@fontsource/ibm-plex-sans/400.css";
import "@fontsource/ibm-plex-sans/500.css";
import "@fontsource/ibm-plex-sans/600.css";
import "@fontsource/ibm-plex-mono/400.css";
import "@fontsource/ibm-plex-mono/500.css";
import "./index.css";
import App from "./App";
import { SettingsApp } from "./screens/Settings";
import { settingsRouteFromSearch } from "./lib/nav";
import { installExternalLinkHandler } from "./lib/external-links";

installExternalLinkHandler();

const queryClient = new QueryClient({
  defaultOptions: { queries: { refetchOnWindowFocus: false, retry: 1 } },
});

// The Tauri Settings window loads index.html?view=settings&pane=… (see open_settings_window).
const settingsRoute = settingsRouteFromSearch(window.location.search);

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <QueryClientProvider client={queryClient}>{settingsRoute ? <SettingsApp initial={settingsRoute} /> : <App />}</QueryClientProvider>
  </React.StrictMode>,
);
