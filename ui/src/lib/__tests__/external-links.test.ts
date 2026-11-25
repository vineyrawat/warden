// @vitest-environment jsdom
import { describe, expect, it } from "vitest";
import { externalHref } from "../external-links";

describe("externalHref", () => {
  const link = (href: string) => {
    const a = document.createElement("a");
    a.setAttribute("href", href);
    const span = document.createElement("span");
    a.appendChild(span);
    return span;
  };
  it("returns http(s) and mailto links, also from a child element", () => {
    expect(externalHref(link("https://gitlab.example.com/x/-/merge_requests/47"))).toBe("https://gitlab.example.com/x/-/merge_requests/47");
    expect(externalHref(link("mailto:dev@example.com"))).toBe("mailto:dev@example.com");
  });
  it("ignores in-app and script links", () => {
    expect(externalHref(link("#settings"))).toBeNull();
    expect(externalHref(link("javascript:alert(1)"))).toBeNull();
    expect(externalHref(document.createElement("div"))).toBeNull();
  });
});
