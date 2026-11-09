import { useEffect, useRef } from "react";
import { useSuspenseInfiniteQuery } from "@tanstack/react-query";
import { useVirtualizer } from "@tanstack/react-virtual";
import { Link } from "@tanstack/react-router";
import type { Schemas } from "@/api/client";
import { Badge } from "@/components/reui/badge";
import { EmptyRow } from "./kit";
import { auditQuery, type AuditFilters } from "./queries";

// The audit log as a virtualized grid: about 30 rows render at a time over thousands of
// events, and the next page of 100 loads as the scroll nears the end of what has loaded.

type Event = Schemas["audit-log-event"];

export type AuditSearch = {
  actor?: string;
  action?: string;
  since?: string;
  until?: string;
  q?: string;
  include?: "web" | "git" | "all";
};

// GHES's audit-log search phrase from the page's filters.
export function phraseOf(s: AuditSearch): string {
  return [
    s.actor && `actor:${s.actor}`,
    s.action && `action:${s.action}`,
    s.since && `created:>=${s.since}`,
    s.until && `created:<=${s.until}`,
    s.q,
  ]
    .filter(Boolean)
    .join(" ");
}

const ROW = 36;
const COLUMNS = "grid-cols-[10rem_8rem_minmax(12rem,1fr)_minmax(10rem,1fr)_8rem]";

const CATEGORY_TONE: Record<
  string,
  "info-light" | "warning-light" | "success-light" | "destructive-light" | "outline"
> = {
  git: "outline",
  team: "info-light",
  org: "warning-light",
  hook: "success-light",
  repo: "info-light",
  repository_ruleset: "destructive-light",
  protected_branch: "destructive-light",
  pre_receive_hook: "warning-light",
};

export function AuditLog({ org, filters }: { org: string; filters: AuditFilters }) {
  const { data, fetchNextPage, hasNextPage, isFetchingNextPage } = useSuspenseInfiniteQuery(
    auditQuery(org, filters),
  );
  const rows = data.pages.flat();
  const scroller = useRef<HTMLDivElement>(null);
  const v = useVirtualizer({
    count: rows.length,
    getScrollElement: () => scroller.current,
    estimateSize: () => ROW,
    overscan: 12,
  });
  const items = v.getVirtualItems();
  const last = items.at(-1)?.index ?? 0;
  useEffect(() => {
    if (hasNextPage && !isFetchingNextPage && last >= rows.length - 20) void fetchNextPage();
  }, [last, rows.length, hasNextPage, isFetchingNextPage, fetchNextPage]);

  if (!rows.length) return <EmptyRow>No events match these filters.</EmptyRow>;
  return (
    <div className="flex flex-col" data-testid="audit-log" data-loaded={rows.length}>
      <div
        className={`bg-muted/50 text-muted-foreground grid ${COLUMNS} gap-3 border-b px-4 py-2 text-xs font-medium`}
        role="row"
      >
        <span>Time</span>
        <span>Actor</span>
        <span>Action</span>
        <span>Target</span>
        <span>Location</span>
      </div>
      <div
        ref={scroller}
        className="h-[32rem] overflow-auto"
        role="rowgroup"
        data-testid="audit-scroller"
      >
        <div className="relative" style={{ height: v.getTotalSize() }}>
          {items.map((item) => (
            <AuditRow key={item.key} event={rows[item.index]} top={item.start} index={item.index} />
          ))}
        </div>
      </div>
      <p className="text-muted-foreground border-t px-4 py-1.5 text-xs" data-testid="audit-count">
        {rows.length.toLocaleString()} events loaded
        {hasNextPage
          ? isFetchingNextPage
            ? ", loading more…"
            : ", scroll for more"
          : ", end of the log"}
        .
      </p>
    </div>
  );
}

function AuditRow({ event: e, top, index }: { event: Event; top: number; index: number }) {
  const category = (e.action ?? "").split(".")[0];
  const at = new Date(e["@timestamp"] ?? 0);
  const target = e.repo ?? e.team ?? e.user ?? e.name ?? (e.hook_id ? `hook ${e.hook_id}` : "");
  return (
    <div
      className={`absolute inset-x-0 grid ${COLUMNS} items-center gap-3 border-b px-4 text-xs`}
      style={{ top, height: ROW }}
      role="row"
      data-audit-row={index}
      data-action={e.action}
      data-actor={e.actor}
    >
      <time
        dateTime={at.toISOString()}
        title={at.toLocaleString()}
        className="text-muted-foreground tabular-nums"
      >
        {at.toLocaleString(undefined, {
          month: "short",
          day: "numeric",
          hour: "2-digit",
          minute: "2-digit",
        })}
      </time>
      {e.actor ? (
        <Link
          to="/$owner"
          params={{ owner: e.actor }}
          className="truncate font-medium hover:underline"
        >
          {e.actor}
        </Link>
      ) : (
        <span />
      )}
      <span className="flex min-w-0 items-center gap-1.5">
        <Badge variant={CATEGORY_TONE[category] ?? "outline"} size="sm" className="font-mono">
          {category}
        </Badge>
        <span className="truncate font-mono">{e.action?.slice(category.length + 1)}</span>
      </span>
      <span className="truncate font-mono" title={target}>
        {target}
      </span>
      <span className="text-muted-foreground truncate">{e.actor_location?.country_name}</span>
    </div>
  );
}
