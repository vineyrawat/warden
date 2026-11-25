// A tiny Markdown parser for note previews (unit tested in markdown.test.ts). It produces a plain
// AST that note-markdown.tsx renders as React elements, so raw input never reaches the DOM as HTML:
// every character ends up in a text node, and links are kept only for http(s) and mailto URLs.
//
// Supported, as GitLab renders it: ATX headings, paragraphs (soft breaks join with a space),
// **bold** / __bold__, *em* / _em_ (not inside words), ~~strike~~, `code`, [links](url), <autolinks>,
// bare http(s) URLs, backslash escapes, fenced code blocks, ```suggestion blocks (with the
// optional :-A+B range), block quotes, bullet and numbered lists, and horizontal rules.

export type Inline =
  | { t: "text"; v: string }
  | { t: "code"; v: string }
  | { t: "strong"; c: Inline[] }
  | { t: "em"; c: Inline[] }
  | { t: "del"; c: Inline[] }
  | { t: "link"; href: string; c: Inline[] };

export type Block =
  | { t: "heading"; level: number; c: Inline[] }
  | { t: "para"; c: Inline[] }
  | { t: "code"; lang: string; text: string }
  | { t: "suggestion"; above: number; below: number; text: string }
  | { t: "quote"; c: Block[] }
  | { t: "list"; ordered: boolean; start: number; items: Inline[][] }
  | { t: "hr" };

const PUNCT = /[!"#$%&'()*+,\-./:;<=>?@[\\\]^_`{|}~]/;

/** http(s) and mailto only; anything else (javascript:, data:, relative paths) is not linked. */
export function safeHref(raw: string): string | null {
  const href = raw.trim();
  if (/^https?:\/\/[^\s]+$/i.test(href)) return href;
  if (/^mailto:[^\s]+$/i.test(href)) return href;
  return null;
}

function pushText(out: Inline[], v: string) {
  if (!v) return;
  const last = out[out.length - 1];
  if (last && last.t === "text") last.v += v;
  else out.push({ t: "text", v });
}

const isWordChar = (ch: string | undefined) => !!ch && /[\p{L}\p{N}]/u.test(ch);
const isSpace = (ch: string | undefined) => ch == null || /\s/.test(ch);

/** Index of the closing delimiter for emphasis starting at `from`, or -1. */
function findClose(s: string, delim: string, from: number): number {
  let i = from;
  while (i < s.length) {
    const ch = s[i];
    if (ch === "\\") {
      i += 2;
      continue;
    }
    if (ch === "`") {
      // Skip a code span so delimiters inside it do not close emphasis.
      let n = 0;
      while (s[i + n] === "`") n++;
      const end = findBackticks(s, i + n, n);
      i = end < 0 ? i + n : end + n;
      continue;
    }
    if (s.startsWith(delim, i) && !isSpace(s[i - 1])) {
      if (delim.length === 1) {
        // A single * or _ must not be half of a double delimiter.
        if (s[i + 1] === delim || s[i - 1] === delim) {
          i += 1;
          continue;
        }
        if (delim === "_" && isWordChar(s[i + 1])) {
          i += 1;
          continue;
        }
      }
      return i;
    }
    i++;
  }
  return -1;
}

/** Position of a run of exactly `n` backticks at or after `from`, or -1. */
function findBackticks(s: string, from: number, n: number): number {
  let i = from;
  while (i < s.length) {
    if (s[i] !== "`") {
      i++;
      continue;
    }
    let m = 0;
    while (s[i + m] === "`") m++;
    if (m === n) return i;
    i += m;
  }
  return -1;
}

/** Index of the `]` that closes the `[` at `open`, honouring nesting and escapes, or -1. */
function findBracket(s: string, open: number): number {
  let depth = 0;
  for (let i = open; i < s.length; i++) {
    const ch = s[i];
    if (ch === "\\") {
      i++;
      continue;
    }
    if (ch === "[") depth++;
    else if (ch === "]") {
      depth--;
      if (depth === 0) return i;
    }
  }
  return -1;
}

/** Index of the `)` that closes the `(` at `open` (URLs may contain balanced parens), or -1. */
function findParen(s: string, open: number): number {
  let depth = 0;
  for (let i = open; i < s.length; i++) {
    if (s[i] === "\\") {
      i++;
      continue;
    }
    if (s[i] === "(") depth++;
    else if (s[i] === ")") {
      depth--;
      if (depth === 0) return i;
    }
  }
  return -1;
}

export function parseInline(src: string): Inline[] {
  const s = src.replace(/\s*\n\s*/g, " ");
  const out: Inline[] = [];
  let i = 0;
  while (i < s.length) {
    const ch = s[i];

    if (ch === "\\" && i + 1 < s.length && PUNCT.test(s[i + 1])) {
      pushText(out, s[i + 1]);
      i += 2;
      continue;
    }

    if (ch === "`") {
      let n = 0;
      while (s[i + n] === "`") n++;
      const end = findBackticks(s, i + n, n);
      if (end < 0) {
        pushText(out, "`".repeat(n));
        i += n;
        continue;
      }
      let v = s.slice(i + n, end);
      if (v.length > 2 && v.startsWith(" ") && v.endsWith(" ") && v.trim()) v = v.slice(1, -1);
      out.push({ t: "code", v });
      i = end + n;
      continue;
    }

    if ((s.startsWith("**", i) || s.startsWith("__", i) || s.startsWith("~~", i)) && !isSpace(s[i + 2])) {
      const delim = s.slice(i, i + 2);
      const intraword = delim === "__" && isWordChar(s[i - 1]);
      const end = intraword ? -1 : findClose(s, delim, i + 2);
      if (end > i + 2 && !(delim === "__" && isWordChar(s[end + 2]))) {
        out.push({ t: delim === "~~" ? "del" : "strong", c: parseInline(s.slice(i + 2, end)) });
        i = end + 2;
        continue;
      }
      pushText(out, delim);
      i += 2;
      continue;
    }

    if ((ch === "*" || ch === "_") && !isSpace(s[i + 1]) && s[i + 1] !== ch) {
      const intraword = ch === "_" && isWordChar(s[i - 1]);
      const end = intraword ? -1 : findClose(s, ch, i + 1);
      if (end > i + 1) {
        out.push({ t: "em", c: parseInline(s.slice(i + 1, end)) });
        i = end + 1;
        continue;
      }
      pushText(out, ch);
      i += 1;
      continue;
    }

    if (ch === "[") {
      const close = findBracket(s, i);
      if (close > i && s[close + 1] === "(") {
        const paren = findParen(s, close + 1);
        if (paren > 0) {
          const target = s.slice(close + 2, paren).trim().split(/\s+/)[0] ?? "";
          const label = parseInline(s.slice(i + 1, close));
          const href = safeHref(target.replace(/^<|>$/g, ""));
          if (href) out.push({ t: "link", href, c: label });
          else for (const n of label) n.t === "text" ? pushText(out, n.v) : out.push(n);
          i = paren + 1;
          continue;
        }
      }
      pushText(out, ch);
      i += 1;
      continue;
    }

    if (ch === "<") {
      const m = /^<((?:https?:\/\/|mailto:)[^\s<>]+)>/i.exec(s.slice(i));
      const href = m ? safeHref(m[1]) : null;
      if (m && href) {
        out.push({ t: "link", href, c: [{ t: "text", v: m[1] }] });
        i += m[0].length;
        continue;
      }
      pushText(out, ch);
      i += 1;
      continue;
    }

    if ((ch === "h" || ch === "H") && !isWordChar(s[i - 1])) {
      const m = /^https?:\/\/[^\s<>]+/i.exec(s.slice(i));
      if (m) {
        const url = m[0].replace(/[.,;:!?)'"]+$/, "");
        out.push({ t: "link", href: url, c: [{ t: "text", v: url }] });
        i += url.length;
        continue;
      }
    }

    pushText(out, ch);
    i += 1;
  }
  return out;
}

const FENCE = /^ {0,3}(`{3,}|~{3,})\s*([^`\s]*)\s*$/;
const HEADING = /^ {0,3}(#{1,6})(?:\s+(.*?))?(?:\s+#+)?\s*$/;
const HR = /^ {0,3}([-*_])(?:\s*\1){2,}\s*$/;
const BULLET = /^ {0,3}[-*+]\s+(.*)$/;
const ORDERED = /^ {0,3}(\d{1,9})[.)]\s+(.*)$/;
const QUOTE = /^ {0,3}>\s?(.*)$/;

/** "suggestion", "suggestion:-2+1" → the range; anything else → null. */
export function suggestionRange(info: string): { above: number; below: number } | null {
  const m = /^suggestion(?::-(\d+)\+(\d+))?$/.exec(info.trim());
  if (!m) return null;
  return { above: Number(m[1] ?? 0), below: Number(m[2] ?? 0) };
}

function startsBlock(line: string): boolean {
  return FENCE.test(line) || HEADING.test(line) || HR.test(line) || BULLET.test(line) || ORDERED.test(line) || QUOTE.test(line);
}

export function parseMarkdown(src: string): Block[] {
  const lines = src.replace(/\r\n?/g, "\n").split("\n");
  const blocks: Block[] = [];
  let i = 0;
  while (i < lines.length) {
    const line = lines[i];
    if (!line.trim()) {
      i++;
      continue;
    }

    const fence = FENCE.exec(line);
    if (fence) {
      const marker = fence[1];
      const info = fence[2] ?? "";
      const body: string[] = [];
      i++;
      while (i < lines.length) {
        const close = new RegExp(`^ {0,3}${marker[0] === "`" ? "`" : "~"}{${marker.length},}\\s*$`);
        if (close.test(lines[i])) {
          i++;
          break;
        }
        body.push(lines[i]);
        i++;
      }
      const text = body.join("\n");
      const range = suggestionRange(info);
      blocks.push(range ? { t: "suggestion", ...range, text } : { t: "code", lang: info, text });
      continue;
    }

    const heading = HEADING.exec(line);
    if (heading) {
      blocks.push({ t: "heading", level: heading[1].length, c: parseInline(heading[2] ?? "") });
      i++;
      continue;
    }

    if (HR.test(line)) {
      blocks.push({ t: "hr" });
      i++;
      continue;
    }

    if (QUOTE.test(line)) {
      const inner: string[] = [];
      while (i < lines.length && lines[i].trim()) {
        const q = QUOTE.exec(lines[i]);
        inner.push(q ? q[1] : lines[i]);
        i++;
      }
      blocks.push({ t: "quote", c: parseMarkdown(inner.join("\n")) });
      continue;
    }

    const bullet = BULLET.exec(line);
    const ordered = ORDERED.exec(line);
    if (bullet || ordered) {
      const isOrdered = !bullet;
      const re = isOrdered ? ORDERED : BULLET;
      const items: string[] = [];
      const start = ordered ? Number(ordered[1]) : 1;
      while (i < lines.length) {
        const l = lines[i];
        const m = re.exec(l);
        if (m) {
          items.push(isOrdered ? m[2] : m[1]);
        } else if (l.trim() && /^\s+/.test(l) && items.length > 0) {
          items[items.length - 1] += `\n${l.trim()}`;
        } else if (l.trim() && !startsBlock(l) && items.length > 0 && lines[i - 1]?.trim()) {
          // Lazy continuation line.
          items[items.length - 1] += `\n${l.trim()}`;
        } else break;
        i++;
      }
      blocks.push({ t: "list", ordered: isOrdered, start, items: items.map(parseInline) });
      continue;
    }

    const para: string[] = [];
    while (i < lines.length && lines[i].trim() && (para.length === 0 || !startsBlock(lines[i]))) {
      para.push(lines[i]);
      i++;
    }
    blocks.push({ t: "para", c: parseInline(para.join("\n").trim()) });
  }
  return blocks;
}

/** Plain text of inline nodes (for titles, tests and accessible labels). */
export function inlineText(nodes: Inline[]): string {
  return nodes.map((n) => (n.t === "text" || n.t === "code" ? n.v : inlineText(n.c))).join("");
}
