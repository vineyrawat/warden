import { describe, expect, it } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import { createElement } from "react";

import { inlineText, parseInline, parseMarkdown, safeHref, suggestionRange } from "./markdown";
import { NoteMarkdown } from "./note-markdown";

const html = (source: string, props: { before?: string | null; line?: number } = {}) =>
  renderToStaticMarkup(createElement(NoteMarkdown, { source, ...props }));

describe("parseInline", () => {
  it("parses bold, em, strike and code", () => {
    expect(parseInline("a **b** *c* ~~d~~ `e`")).toEqual([
      { t: "text", v: "a " },
      { t: "strong", c: [{ t: "text", v: "b" }] },
      { t: "text", v: " " },
      { t: "em", c: [{ t: "text", v: "c" }] },
      { t: "text", v: " " },
      { t: "del", c: [{ t: "text", v: "d" }] },
      { t: "text", v: " " },
      { t: "code", v: "e" },
    ]);
  });

  it("nests emphasis inside bold", () => {
    expect(parseInline("**x *y* z**")).toEqual([
      { t: "strong", c: [{ t: "text", v: "x " }, { t: "em", c: [{ t: "text", v: "y" }] }, { t: "text", v: " z" }] },
    ]);
  });

  it("does not italicise snake_case words", () => {
    expect(parseInline("call get_attempts_for_quiz now")).toEqual([{ t: "text", v: "call get_attempts_for_quiz now" }]);
    expect(parseInline("__init__ method")).toEqual([{ t: "strong", c: [{ t: "text", v: "init" }] }, { t: "text", v: " method" }]);
  });

  it("keeps delimiters inside code spans literal", () => {
    expect(parseInline("`a*b*c` and ``x ` y``")).toEqual([
      { t: "code", v: "a*b*c" },
      { t: "text", v: " and " },
      { t: "code", v: "x ` y" },
    ]);
  });

  it("leaves unmatched delimiters as text", () => {
    expect(inlineText(parseInline("2 * 3 = 6, **open, `tick"))).toBe("2 * 3 = 6, **open, `tick");
  });

  it("honours backslash escapes", () => {
    expect(parseInline("\\*not em\\*")).toEqual([{ t: "text", v: "*not em*" }]);
  });

  it("parses links, autolinks and bare URLs", () => {
    expect(parseInline("[docs](https://x.dev/a) <https://y.dev> see https://z.dev/p.")).toEqual([
      { t: "link", href: "https://x.dev/a", c: [{ t: "text", v: "docs" }] },
      { t: "text", v: " " },
      { t: "link", href: "https://y.dev", c: [{ t: "text", v: "https://y.dev" }] },
      { t: "text", v: " see " },
      { t: "link", href: "https://z.dev/p", c: [{ t: "text", v: "https://z.dev/p" }] },
      { t: "text", v: "." },
    ]);
  });

  it("drops unsafe link targets but keeps the label", () => {
    expect(parseInline("[click](javascript:alert(1))")).toEqual([{ t: "text", v: "click" }]);
    expect(parseInline("[img](data:text/html;base64,xx)")).toEqual([{ t: "text", v: "img" }]);
  });

  it("joins soft line breaks with a space", () => {
    expect(parseInline("one\ntwo")).toEqual([{ t: "text", v: "one two" }]);
  });
});

describe("safeHref", () => {
  it("allows http(s) and mailto only", () => {
    expect(safeHref("https://gitlab.com/a")).toBe("https://gitlab.com/a");
    expect(safeHref("mailto:a@b.c")).toBe("mailto:a@b.c");
    expect(safeHref("javascript:alert(1)")).toBeNull();
    expect(safeHref("JaVaScRiPt:alert(1)")).toBeNull();
    expect(safeHref("/relative")).toBeNull();
    expect(safeHref("vbscript:x")).toBeNull();
  });
});

describe("parseMarkdown", () => {
  it("splits headings, paragraphs and rules", () => {
    const b = parseMarkdown("# Title #\n\nFirst line\nsecond line\n\n---\n\n### C#");
    expect(b.map((x) => x.t)).toEqual(["heading", "para", "hr", "heading"]);
    expect(b[0]).toEqual({ t: "heading", level: 1, c: [{ t: "text", v: "Title" }] });
    expect(b[1]).toEqual({ t: "para", c: [{ t: "text", v: "First line second line" }] });
    expect(b[3]).toEqual({ t: "heading", level: 3, c: [{ t: "text", v: "C#" }] });
  });

  it("does not treat #hashtag as a heading", () => {
    expect(parseMarkdown("#41 is the MR")[0].t).toBe("para");
  });

  it("parses fenced code with a language and keeps its content raw", () => {
    expect(parseMarkdown("```python\nx = **1**\n```")).toEqual([{ t: "code", lang: "python", text: "x = **1**" }]);
  });

  it("parses suggestion blocks with and without a range", () => {
    expect(parseMarkdown("```suggestion:-0+0\nfoo()\n```")).toEqual([{ t: "suggestion", above: 0, below: 0, text: "foo()" }]);
    expect(parseMarkdown("```suggestion:-2+1\na\nb\n```")).toEqual([{ t: "suggestion", above: 2, below: 1, text: "a\nb" }]);
    expect(parseMarkdown("```suggestion\nbar()\n```")[0]).toEqual({ t: "suggestion", above: 0, below: 0, text: "bar()" });
  });

  it("needs a closing fence at least as long as the opening one", () => {
    const b = parseMarkdown("````suggestion\nuse ``` here\n```\n````\nafter");
    expect(b[0]).toEqual({ t: "suggestion", above: 0, below: 0, text: "use ``` here\n```" });
    expect(b[1]).toEqual({ t: "para", c: [{ t: "text", v: "after" }] });
  });

  it("runs an unclosed fence to the end", () => {
    expect(parseMarkdown("```\na\nb")).toEqual([{ t: "code", lang: "", text: "a\nb" }]);
  });

  it("parses lists and quotes", () => {
    const b = parseMarkdown("- one\n- **two**\n  more\n\n3. c\n4. d\n\n> quoted *text*");
    expect(b[0]).toEqual({ t: "list", ordered: false, start: 1, items: [[{ t: "text", v: "one" }], [{ t: "strong", c: [{ t: "text", v: "two" }] }, { t: "text", v: " more" }]] });
    expect(b[1]).toMatchObject({ t: "list", ordered: true, start: 3 });
    expect(b[2]).toEqual({ t: "quote", c: [{ t: "para", c: [{ t: "text", v: "quoted " }, { t: "em", c: [{ t: "text", v: "text" }] }] }] });
  });

  it("parses the note Warden posts", () => {
    const note = "🔴 Blocker · **SQL built with an f-string**\n\n`doctype` comes from the request.\n\n```suggestion:-0+0\nfrappe.db.set_value(doctype, name)\n```\n\n— Warden · rule FR-002 · confidence 0.94";
    expect(parseMarkdown(note).map((b) => b.t)).toEqual(["para", "para", "suggestion", "para"]);
  });

  it("parses suggestionRange", () => {
    expect(suggestionRange("suggestion")).toEqual({ above: 0, below: 0 });
    expect(suggestionRange("suggestion:-3+4")).toEqual({ above: 3, below: 4 });
    expect(suggestionRange("python")).toBeNull();
  });
});

describe("NoteMarkdown rendering", () => {
  it("escapes raw HTML everywhere", () => {
    const out = html('<script>alert(1)</script>\n\n**<img src=x onerror=alert(1)>**\n\n```\n<b>x</b>\n```\n\n`<i>`');
    expect(out).not.toContain("<script");
    expect(out).not.toContain("<img");
    expect(out).not.toContain("<b>");
    expect(out).not.toContain("<i>");
    expect(out).toContain("&lt;script&gt;alert(1)&lt;/script&gt;");
    expect(out).toContain("&lt;img src=x onerror=alert(1)&gt;");
  });

  it("never renders an unsafe href", () => {
    const out = html("[x](javascript:alert(1)) [y](https://ok.dev)");
    expect(out).not.toContain("javascript:");
    expect(out).toContain('href="https://ok.dev"');
    expect(out).toContain('rel="noopener noreferrer"');
  });

  it("renders a suggestion as a diff with the commented line", () => {
    const out = html("```suggestion:-0+0\nnew()\n```", { before: "old()", line: 17 });
    expect(out).toContain("Suggested change");
    expect(out).toMatch(/bg-diff-del.*old\(\)/);
    expect(out).toMatch(/bg-diff-add.*new\(\)/);
    expect(out).toContain(">17<");
  });

  it("shows only the added lines when the original line is unknown", () => {
    const out = html("```suggestion\nnew()\n```");
    expect(out).not.toContain("bg-diff-del");
    expect(out).toContain("bg-diff-add");
  });
});
