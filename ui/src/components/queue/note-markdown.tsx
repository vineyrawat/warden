// Renders parseMarkdown() output as React elements, styled like a GitLab note. No HTML string is
// ever injected: React puts every piece of text in a text node, so markup in a finding is shown
// literally.
import { Fragment, type ReactNode } from "react";

import { cn } from "@/lib/utils";
import { parseMarkdown, type Block, type Inline } from "./markdown";

function renderInline(nodes: Inline[], key = "i"): ReactNode[] {
  return nodes.map((n, i) => {
    const k = `${key}-${i}`;
    switch (n.t) {
      case "text":
        return <Fragment key={k}>{n.v}</Fragment>;
      case "code":
        return (
          <code key={k} className="rounded bg-codebg px-1 py-px font-mono text-[0.92em]">
            {n.v}
          </code>
        );
      case "strong":
        return <strong key={k} className="font-semibold">{renderInline(n.c, k)}</strong>;
      case "em":
        return <em key={k}>{renderInline(n.c, k)}</em>;
      case "del":
        return <del key={k}>{renderInline(n.c, k)}</del>;
      case "link":
        return (
          <a key={k} href={n.href} target="_blank" rel="noopener noreferrer" className="text-primary hover:underline">
            {renderInline(n.c, k)}
          </a>
        );
    }
  });
}

const HEADING_CLASS = ["text-[15px]", "text-[14px]", "text-[13px]", "text-[13px]", "text-xs", "text-xs"];

/** GitLab's "Suggested change" box: the commented line in red, the replacement in green. */
export function SuggestionDiff({ text, before, line, above, below }: { text: string; before?: string | null; line?: number; above?: number; below?: number }) {
  const added = text.replace(/\n+$/, "").split("\n");
  const showBefore = before != null && !above && !below;
  const rows: { kind: "del" | "add"; n: number | null; text: string }[] = [];
  if (showBefore) rows.push({ kind: "del", n: line ?? null, text: before });
  added.forEach((t, i) => rows.push({ kind: "add", n: line != null ? line + i : null, text: t }));
  return (
    <div className="overflow-hidden rounded-md border font-mono text-[11.5px] leading-[1.6]">
      <div className="border-b bg-surface-2 px-2.5 py-1 font-sans text-2xs font-medium text-muted-foreground">
        Suggested change
        {(above || below) ? <span className="ml-1 font-normal">· replaces {1 + (above ?? 0) + (below ?? 0)} lines</span> : null}
      </div>
      <div className="overflow-x-auto bg-codebg">
        <table className="min-w-full border-collapse">
          <tbody>
            {rows.map((r, i) => (
              <tr key={i} className={r.kind === "add" ? "bg-diff-add" : "bg-diff-del"}>
                <td className="w-8 select-none pl-2 pr-1 text-right text-muted-foreground">{r.n ?? ""}</td>
                <td className="w-4 select-none text-center text-muted-foreground">{r.kind === "add" ? "+" : "−"}</td>
                <td className="whitespace-pre pr-3">{r.text || " "}</td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </div>
  );
}

function renderBlocks(blocks: Block[], ctx: { before?: string | null; line?: number }, key = "b"): ReactNode[] {
  return blocks.map((b, i) => {
    const k = `${key}-${i}`;
    switch (b.t) {
      case "heading": {
        const Tag = `h${Math.min(6, Math.max(1, b.level))}` as "h1";
        return (
          <Tag key={k} className={cn("font-semibold", HEADING_CLASS[b.level - 1])}>
            {renderInline(b.c, k)}
          </Tag>
        );
      }
      case "para":
        return <p key={k}>{renderInline(b.c, k)}</p>;
      case "code":
        return (
          <pre key={k} className="overflow-x-auto rounded-md border bg-codebg px-2.5 py-2 font-mono text-[11.5px] leading-[1.6]" data-lang={b.lang || undefined}>
            <code>{b.text}</code>
          </pre>
        );
      case "suggestion":
        return <SuggestionDiff key={k} text={b.text} before={ctx.before} line={ctx.line} above={b.above} below={b.below} />;
      case "quote":
        return (
          <blockquote key={k} className="space-y-2 border-l-[3px] pl-3 text-muted-foreground">
            {renderBlocks(b.c, ctx, k)}
          </blockquote>
        );
      case "list": {
        const items = b.items.map((it, j) => <li key={`${k}-${j}`}>{renderInline(it, `${k}-${j}`)}</li>);
        return b.ordered ? (
          <ol key={k} start={b.start} className="list-decimal space-y-0.5 pl-5">
            {items}
          </ol>
        ) : (
          <ul key={k} className="list-disc space-y-0.5 pl-5">
            {items}
          </ul>
        );
      }
      case "hr":
        return <hr key={k} className="border-border" />;
    }
  });
}

/**
 * A note body as GitLab shows it. `before` / `line` describe the commented line so a
 * ```suggestion block can show what it replaces.
 */
export function NoteMarkdown({ source, before, line, className }: { source: string; before?: string | null; line?: number; className?: string }) {
  return <div className={cn("space-y-2 break-words text-[12.5px] leading-[1.55]", className)}>{renderBlocks(parseMarkdown(source), { before, line })}</div>;
}
