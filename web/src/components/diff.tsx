import { useMemo, useRef, type ReactNode } from "react";
import { useVirtualizer } from "@tanstack/react-virtual";
import { HugeiconsIcon } from "@hugeicons/react";
import { Add01Icon } from "@hugeicons/core-free-icons";
import type { Schemas } from "@/api/client";
import {
  anchorOf,
  parsePatch,
  splitLines,
  type Anchor,
  type CodeLine,
  type Line,
} from "@/areas/pulls/patch";
import { Badge } from "@/components/reui/badge";
import { cn } from "cn";

export { parsePatch };
export type { Anchor };

// A file's diff, unified or split. Rows are one flat list, so a long diff renders
// through TanStack Virtual inside a fixed-height scroller (design/SHARED.md, "Frontend
// and native app": long diffs virtualize lines). Line-number gutters are buttons when
// `onSelect` is given: a click picks a line and shift-click extends to a range on the
// same side. `after(anchor)` puts content (a thread, a comment form) under that line.

export type DiffView = "unified" | "split";
export type Selection = { start: Anchor; end: Anchor };

type Row =
  | { kind: "hunk"; key: string; text: string }
  | { kind: "line"; key: string; line: CodeLine }
  | { kind: "pair"; key: string; left?: CodeLine; right?: CodeLine }
  | { kind: "after"; key: string; node: ReactNode };

// Rows past this many render virtualized.
export const VIRTUALIZE_AT = 400;

const TONE = {
  context: "",
  add: "bg-success/12",
  del: "bg-destructive/12",
} as const;
const SIGN = { context: " ", add: "+", del: "-" } as const;

function inSelection(sel: Selection | null | undefined, a: Anchor) {
  if (!sel || sel.start.side !== a.side) return false;
  const lo = Math.min(sel.start.line, sel.end.line);
  const hi = Math.max(sel.start.line, sel.end.line);
  return a.line >= lo && a.line <= hi;
}

export function FileDiff({
  file,
  view = "unified",
  selection,
  onSelect,
  after,
  collapsed,
  header,
  maxHeight = 640,
}: {
  file: Schemas["diff-entry"];
  view?: DiffView;
  selection?: Selection | null;
  onSelect?: (sel: Selection) => void;
  after?: (a: Anchor) => ReactNode;
  collapsed?: boolean;
  // Extra controls at the right of the file header, such as a viewed checkbox.
  header?: ReactNode;
  maxHeight?: number;
}) {
  const lines = useMemo(() => (file.patch ? parsePatch(file.patch) : []), [file.patch]);
  const rows = useMemo(() => buildRows(lines, view, after), [lines, view, after]);

  return (
    <section
      className="bg-card overflow-hidden rounded-sm border"
      data-file={file.filename}
      data-view={view}
    >
      <header className="bg-muted/50 flex items-center gap-2 border-b px-3 py-1.5 text-xs">
        <code className="truncate font-mono font-medium">
          {file.previous_filename ? `${file.previous_filename} → ` : ""}
          {file.filename}
        </code>
        <Badge variant="outline" size="sm" className="capitalize">
          {file.status}
        </Badge>
        <span className="ml-auto font-mono">
          <span className="text-success">+{file.additions}</span>{" "}
          <span className="text-destructive">-{file.deletions}</span>
        </span>
        {header}
      </header>
      {collapsed ? null : !file.patch ? (
        <p className="text-muted-foreground px-3 py-2 text-xs">
          Binary file or diff too large to show.
        </p>
      ) : rows.length > VIRTUALIZE_AT ? (
        <VirtualRows
          rows={rows}
          view={view}
          selection={selection}
          onSelect={onSelect}
          maxHeight={maxHeight}
        />
      ) : (
        <div className="overflow-x-auto font-mono text-xs leading-5" role="table">
          {rows.map((r) => (
            <RowView key={r.key} row={r} view={view} selection={selection} onSelect={onSelect} />
          ))}
        </div>
      )}
    </section>
  );
}

function buildRows(lines: Line[], view: DiffView, after?: (a: Anchor) => ReactNode): Row[] {
  const rows: Row[] = [];
  const extra = (l: CodeLine | undefined, key: string) => {
    if (!l || !after) return;
    const node = after(anchorOf(l));
    if (node) rows.push({ kind: "after", key: `after-${key}`, node });
  };
  if (view === "unified") {
    lines.forEach((l, i) => {
      if (l.kind === "hunk") rows.push({ kind: "hunk", key: `h${i}`, text: l.text });
      else {
        rows.push({ kind: "line", key: `l${i}`, line: l });
        extra(l, `l${i}`);
      }
    });
  } else {
    splitLines(lines).forEach((r, i) => {
      if (r.kind === "hunk") rows.push({ kind: "hunk", key: `h${i}`, text: r.text });
      else {
        rows.push({ kind: "pair", key: `p${i}`, left: r.left, right: r.right });
        // A context line has one anchor; a changed pair may hold two.
        if (r.left && r.left === r.right) extra(r.right, `p${i}`);
        else {
          extra(r.left, `p${i}l`);
          extra(r.right, `p${i}r`);
        }
      }
    });
  }
  return rows;
}

function VirtualRows({
  rows,
  view,
  selection,
  onSelect,
  maxHeight,
}: {
  rows: Row[];
  view: DiffView;
  selection?: Selection | null;
  onSelect?: (sel: Selection) => void;
  maxHeight: number;
}) {
  const scroller = useRef<HTMLDivElement>(null);
  const v = useVirtualizer({
    count: rows.length,
    getScrollElement: () => scroller.current,
    estimateSize: (i) => (rows[i].kind === "after" ? 120 : 20),
    overscan: 20,
    getItemKey: (i) => rows[i].key,
  });
  return (
    <div
      ref={scroller}
      className="overflow-auto font-mono text-xs leading-5"
      style={{ maxHeight }}
      data-virtualized={rows.length}
      role="table"
    >
      <div style={{ height: v.getTotalSize(), position: "relative" }}>
        {v.getVirtualItems().map((item) => (
          <div
            key={item.key}
            ref={v.measureElement}
            data-index={item.index}
            className="absolute inset-x-0 top-0 min-w-max"
            style={{ transform: `translateY(${item.start}px)` }}
          >
            <RowView row={rows[item.index]} view={view} selection={selection} onSelect={onSelect} />
          </div>
        ))}
      </div>
    </div>
  );
}

function RowView({
  row,
  view,
  selection,
  onSelect,
}: {
  row: Row;
  view: DiffView;
  selection?: Selection | null;
  onSelect?: (sel: Selection) => void;
}) {
  if (row.kind === "hunk")
    return (
      <div className="bg-info/10 text-muted-foreground px-3 py-0.5 whitespace-pre" role="row">
        {row.text}
      </div>
    );
  if (row.kind === "after")
    return (
      <div className="bg-muted/30 border-y px-3 py-2 font-sans text-sm leading-normal">
        {row.node}
      </div>
    );
  if (row.kind === "line")
    return (
      <div className={cn("flex", TONE[row.line.kind])} role="row" data-line={lineId(row.line)}>
        <Gutter line={row.line} which="old" selection={selection} onSelect={onSelect} />
        <Gutter line={row.line} which="new" selection={selection} onSelect={onSelect} />
        <Code line={row.line} />
      </div>
    );
  return (
    <div className="grid grid-cols-2 items-stretch" role="row" data-view={view}>
      <Half line={row.left} which="old" selection={selection} onSelect={onSelect} />
      <Half line={row.right} which="new" selection={selection} onSelect={onSelect} border />
    </div>
  );
}

const lineId = (l: CodeLine) => {
  const a = anchorOf(l);
  return `${a.side}${a.line}`;
};

function Half({
  line,
  which,
  selection,
  onSelect,
  border,
}: {
  line?: CodeLine;
  which: "old" | "new";
  selection?: Selection | null;
  onSelect?: (sel: Selection) => void;
  border?: boolean;
}) {
  if (!line) return <div className={cn("bg-muted/40", border && "border-l")} />;
  return (
    <div
      className={cn("flex min-w-0 items-start", TONE[line.kind], border && "border-l")}
      data-line={line.kind === "context" && which === "old" ? undefined : lineId(line)}
    >
      <Gutter line={line} which={which} selection={selection} onSelect={onSelect} />
      <Code line={line} wrap />
    </div>
  );
}

function Gutter({
  line,
  which,
  selection,
  onSelect,
}: {
  line: CodeLine;
  which: "old" | "new";
  selection?: Selection | null;
  onSelect?: (sel: Selection) => void;
}) {
  const n = which === "old" ? line.old : line.new;
  // Comments anchor where GitHub puts them: deletions LEFT, everything else RIGHT, so a
  // context line's old number is not a target of its own.
  const anchor = anchorOf(line);
  const owns = n !== undefined && !(line.kind === "context" && which === "old");
  const selectable = onSelect && owns;
  const selected = owns && inSelection(selection, anchor);
  const cls = cn(
    "text-muted-foreground w-11 shrink-0 self-stretch border-r px-2 text-right select-none",
    selected && "bg-warning/25 text-foreground",
  );
  if (!selectable) return <span className={cls}>{n}</span>;
  return (
    <button
      type="button"
      className={cn(cls, "group/gutter hover:bg-info/20 relative cursor-pointer")}
      aria-label={`Comment on line ${n}`}
      aria-pressed={selected || undefined}
      onClick={(e) => {
        const start =
          e.shiftKey && selection && selection.start.side === anchor.side
            ? selection.start
            : anchor;
        onSelect({ start, end: anchor });
      }}
    >
      <HugeiconsIcon
        icon={Add01Icon}
        strokeWidth={2.5}
        className="bg-info text-background absolute top-0.5 -left-2 hidden size-4 rounded-sm group-hover/gutter:block"
      />
      {n}
    </button>
  );
}

function Code({ line, wrap }: { line: CodeLine; wrap?: boolean }) {
  return (
    <>
      <span className="w-4 shrink-0 pl-2 select-none">{SIGN[line.kind]}</span>
      <span
        className={cn("pr-3", wrap ? "min-w-0 break-words whitespace-pre-wrap" : "whitespace-pre")}
      >
        {line.text}
      </span>
    </>
  );
}
