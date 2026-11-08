import { memo, useEffect, useMemo, useState, type CSSProperties } from "react";
import { HugeiconsIcon } from "@hugeicons/react";
import { ArrowDown01Icon, ArrowRight01Icon } from "@hugeicons/core-free-icons";
import type { Schemas } from "@/api/client";
import type { CodeBlockToken } from "@/components/reui/code-block/code-block";
import { cn } from "cn";
import type { LogLine, StepLog } from "./log";
import { Duration, StatusIcon } from "./status";

type Step = NonNullable<Schemas["job"]["steps"]>[number];

// GitHub's line anchors: #step:<step>:<line>.
export const lineAnchor = (step: number, n: number) => `step:${step}:${n}`;

export function parseAnchor(hash: string) {
  const m = /^#?step:(\d+)(?::(\d+))?$/.exec(decodeURIComponent(hash));
  return m ? { step: Number(m[1]), line: m[2] ? Number(m[2]) : undefined } : undefined;
}

const tokenStyle = (t: CodeBlockToken): CSSProperties | undefined =>
  t.color || t.fontStyle
    ? ({
        "--log-c": t.color,
        "--log-cd": t.colorDark ?? t.color,
        fontWeight: t.fontStyle === "bold" ? 700 : undefined,
        fontStyle: t.fontStyle === "italic" ? "italic" : undefined,
        textDecoration: t.fontStyle === "underline" ? "underline" : undefined,
      } as CSSProperties)
    : undefined;

// Splits the line's tokens again around search matches, so a match keeps its colour.
function Highlighted({ tokens, query }: { tokens: CodeBlockToken[]; query: string }) {
  const q = query.toLowerCase();
  return tokens.map((t, i) => {
    const style = tokenStyle(t);
    const cls = style ? "text-[color:var(--log-c)] dark:text-[color:var(--log-cd)]" : undefined;
    if (!q || !t.content.toLowerCase().includes(q))
      return (
        <span key={i} className={cls} style={style}>
          {t.content}
        </span>
      );
    const parts: React.ReactNode[] = [];
    let at = 0;
    const lower = t.content.toLowerCase();
    for (let hit = lower.indexOf(q); hit >= 0; hit = lower.indexOf(q, at)) {
      if (hit > at) parts.push(t.content.slice(at, hit));
      parts.push(
        <mark key={hit} data-match className="bg-warning/40 text-foreground rounded-sm">
          {t.content.slice(hit, hit + q.length)}
        </mark>,
      );
      at = hit + q.length;
    }
    parts.push(t.content.slice(at));
    return (
      <span key={i} className={cls} style={style}>
        {parts}
      </span>
    );
  });
}

const KIND = {
  plain: "",
  command: "text-info-foreground dark:text-info",
  error: "bg-destructive/10 text-destructive",
  warning: "bg-warning/10 text-warning-foreground dark:text-warning",
  notice: "bg-info/10",
} as const;

const Line = memo(function Line({
  step,
  line,
  query,
  target,
  folded,
  onFold,
}: {
  step: number;
  line: LogLine;
  query: string;
  target: boolean;
  folded?: boolean;
  onFold?: () => void;
}) {
  return (
    <div
      id={lineAnchor(step, line.n)}
      data-line={line.n}
      data-kind={line.kind}
      className={cn(
        "group/line hover:bg-muted/60 flex min-h-5 items-start font-mono text-xs leading-5",
        KIND[line.kind],
        target && "bg-warning/20 dark:bg-warning/15",
      )}
    >
      <a
        href={`#${lineAnchor(step, line.n)}`}
        className="text-muted-foreground hover:text-foreground w-12 shrink-0 pr-3 text-right tabular-nums select-none"
        aria-label={`Line ${line.n}`}
      >
        {line.n}
      </a>
      <span
        className={cn(
          "min-w-0 flex-1 break-all whitespace-pre-wrap",
          line.group !== undefined && !line.opensGroup && "pl-4",
        )}
      >
        {line.opensGroup ? (
          <button
            type="button"
            onClick={onFold}
            aria-expanded={!folded}
            data-group-toggle
            className="flex items-center gap-1 font-semibold hover:underline"
          >
            <HugeiconsIcon
              icon={folded ? ArrowRight01Icon : ArrowDown01Icon}
              strokeWidth={2}
              className="size-3"
            />
            <Highlighted tokens={line.tokens} query={query} />
          </button>
        ) : line.kind === "error" || line.kind === "warning" || line.kind === "notice" ? (
          <>
            <span className="font-semibold capitalize">{line.kind}: </span>
            <Highlighted tokens={line.tokens} query={query} />
          </>
        ) : (
          <Highlighted tokens={line.tokens} query={query} />
        )}
      </span>
    </div>
  );
});

// Matches in a step's lines, counted in the parsed text (ANSI codes stripped).
export const matchesIn = (log: StepLog | undefined, query: string) => {
  if (!log || !query) return 0;
  const q = query.toLowerCase();
  return log.lines.reduce((n, l) => n + (l.text.toLowerCase().split(q).length - 1), 0);
};

export function StepSection({
  step,
  log,
  open,
  onToggle,
  query,
  target,
  live,
}: {
  step: Step;
  log: StepLog | undefined;
  open: boolean;
  onToggle: () => void;
  query: string;
  target?: number;
  live: boolean;
}) {
  // Groups start folded, except one holding the linked line, an error, or a match.
  const [folded, setFolded] = useState<Set<number>>(() => new Set());
  const [touched, setTouched] = useState<Set<number>>(() => new Set());
  const effective = useMemo(() => {
    const out = new Set(folded);
    for (const [g] of (log?.groups ?? []).entries()) {
      if (touched.has(g)) continue;
      const inGroup = log!.lines.filter((l) => l.group === g);
      const keepOpen =
        inGroup.some((l) => l.n === target || l.kind === "error") ||
        (query && inGroup.some((l) => l.text.toLowerCase().includes(query.toLowerCase())));
      if (!keepOpen) out.add(g);
    }
    return out;
  }, [folded, touched, log, target, query]);

  const toggleGroup = (g: number) => {
    setTouched((t) => new Set(t).add(g));
    setFolded(() => {
      const next = new Set(effective);
      if (next.has(g)) next.delete(g);
      else next.add(g);
      return next;
    });
  };

  const matches = matchesIn(log, query);
  const running = step.status === "in_progress";
  return (
    <section
      data-step={step.number}
      data-step-status={step.conclusion ?? step.status}
      className="border-b last:border-b-0"
    >
      <button
        type="button"
        onClick={onToggle}
        aria-expanded={open}
        className="hover:bg-muted/50 flex w-full items-center gap-2 px-3 py-2 text-left text-sm"
      >
        <HugeiconsIcon
          icon={open ? ArrowDown01Icon : ArrowRight01Icon}
          strokeWidth={2}
          className="text-muted-foreground size-4"
        />
        <StatusIcon item={step} />
        <span className="min-w-0 flex-1 truncate font-medium">{step.name}</span>
        {query && matches > 0 && (
          <span
            className="text-warning-foreground dark:text-warning text-xs tabular-nums"
            data-step-matches={matches}
          >
            {matches} match{matches === 1 ? "" : "es"}
          </span>
        )}
        <span className="text-muted-foreground text-xs">
          {step.conclusion !== "skipped" && (
            <Duration start={step.started_at} end={step.completed_at} running={running} />
          )}
        </span>
      </button>
      {open && (
        <div className="bg-muted/20 pb-2" data-step-log={step.number}>
          {!log || log.lines.length === 0 ? (
            <p className="text-muted-foreground px-12 py-1 font-mono text-xs">
              {step.conclusion === "skipped"
                ? "This step was skipped."
                : live && running
                  ? "Waiting for output…"
                  : "No output."}
            </p>
          ) : (
            log.lines
              .filter((l) => l.opensGroup || l.group === undefined || !effective.has(l.group))
              .map((l) => (
                <Line
                  key={l.n}
                  step={step.number}
                  line={l}
                  query={query}
                  target={target === l.n}
                  folded={l.opensGroup ? effective.has(l.group!) : undefined}
                  onFold={l.opensGroup ? () => toggleGroup(l.group!) : undefined}
                />
              ))
          )}
          {live && running && (
            <div
              className="text-muted-foreground flex items-center gap-2 px-12 py-1 font-mono text-xs"
              data-testid="live-cursor"
            >
              <span className="bg-success inline-block size-1.5 animate-pulse rounded-sm" />
              streaming
            </div>
          )}
        </div>
      )}
    </section>
  );
}

// Scrolls a linked line into view once it is rendered.
export function useScrollToAnchor(
  anchor: { step: number; line?: number } | undefined,
  ready: boolean,
) {
  useEffect(() => {
    if (!anchor?.line || !ready) return;
    const el = document.getElementById(lineAnchor(anchor.step, anchor.line));
    el?.scrollIntoView({ block: "center" });
  }, [anchor?.step, anchor?.line, ready]);
}
