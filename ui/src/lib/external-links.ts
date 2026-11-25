// Tauri's webview does not hand `<a target="_blank">` links to the system browser, so clicks on
// external links do nothing in the app. One capturing listener routes every http(s)/mailto link
// (present and future) to the default browser via the opener plugin. In the browser preview the
// links keep their normal behaviour.
import { openUrl } from "@tauri-apps/plugin-opener";
import { toast } from "sonner";

import { isTauri } from "./api";

/** The external URL a click should open, or null to leave the click alone. */
export function externalHref(target: EventTarget | null): string | null {
  const el = target instanceof Element ? target.closest("a[href]") : null;
  const href = el?.getAttribute("href") ?? "";
  return /^(https?:|mailto:)/i.test(href) ? href : null;
}

export function installExternalLinkHandler(): void {
  if (!isTauri()) return;
  document.addEventListener(
    "click",
    (e) => {
      if (e.defaultPrevented || e.button !== 0) return;
      const href = externalHref(e.target);
      if (!href) return;
      e.preventDefault();
      openUrl(href).catch((err) => toast.error(`Could not open ${href}: ${String(err)}`));
    },
    true,
  );
}
