import { useEffect, useMemo, useState } from "react";
import { createFileRoute, Link } from "@tanstack/react-router";
import { useQuery, useSuspenseQuery } from "@tanstack/react-query";
import { HugeiconsIcon, type IconSvgElement } from "@hugeicons/react";
import {
  Bookmark02Icon,
  CheckmarkCircle02Icon,
  InboxIcon,
  MailOpen01Icon,
  Mail01Icon,
  NotificationOff02Icon,
  RepositoryIcon,
  Undo02Icon,
} from "@hugeicons/core-free-icons";
import {
  apply,
  usePendingActions,
  useNotificationAction,
  type Action,
  type Row,
  type View,
} from "@/areas/activity/notification-actions";
import { doneQuery, inboxQuery, savedQuery, type Thread } from "@/areas/activity/queries";
import {
  reasonLabel,
  REASONS,
  SubjectIcon,
  subjectLabel,
  subjectLink,
} from "@/areas/activity/subjects";
import { Ago } from "@/components/github";
import { PageHeader } from "@/components/panel";
import { Region } from "@/components/region";
import { Badge } from "@/components/reui/badge";
import { Frame, FrameHeader, FramePanel } from "@/components/reui/frame";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import {
  Empty,
  EmptyDescription,
  EmptyHeader,
  EmptyMedia,
  EmptyTitle,
} from "@/components/ui/empty";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { cn } from "cn";

// The GHES inbox. The sidebar filters by box (inbox, saved, done), then repository, then
// reason; ?unread narrows the inbox to unread threads. Rows select for bulk actions.

const VIEWS = ["inbox", "saved", "done"] as const;

type Search = { view?: View; repo?: string; reason?: string; unread?: boolean };

export const Route = createFileRoute("/notifications")({
  validateSearch: (s: Record<string, unknown>): Search => ({
    view: VIEWS.includes(s.view as View) && s.view !== "inbox" ? (s.view as View) : undefined,
    repo: typeof s.repo === "string" && /^[\w.-]+\/[\w.-]+$/.test(s.repo) ? s.repo : undefined,
    reason: typeof s.reason === "string" && s.reason in REASONS ? s.reason : undefined,
    unread: s.unread === true || s.unread === "true" ? true : undefined,
  }),
  loader: ({ context }) => {
    void context.queryClient.prefetchQuery(inboxQuery());
    void context.queryClient.prefetchQuery(savedQuery());
    void context.queryClient.prefetchQuery(doneQuery());
  },
  component: NotificationsPage,
});

function NotificationsPage() {
  return (
    <main className="mx-auto max-w-6xl p-4 sm:p-6">
      <PageHeader
        title="Notifications"
        description="Everything that needs your attention, across every repository."
      />
      <div className="grid items-start gap-4 md:grid-cols-[14rem_minmax(0,1fr)]">
        <Region name="notification filters" rows={8}>
          <Sidebar />
        </Region>
        <Region name="notifications" rows={8}>
          <Inbox />
        </Region>
      </div>
    </main>
  );
}

// The rows a view shows, with every action still in flight applied over the server's.
function useRows(view: View): Row[] {
  const inbox = useSuspenseQuery(inboxQuery()).data.threads;
  const saved = useSuspenseQuery(savedQuery()).data;
  const done = useQuery({ ...doneQuery(), enabled: view === "done" });
  const pending = usePendingActions();
  const savedIds = new Set(saved.map((t) => t.id));
  const base: Thread[] = view === "inbox" ? inbox : view === "saved" ? saved : (done.data ?? []);
  if (done.error && view === "done") throw done.error;
  return pending.reduce(
    (rows, a) => apply(rows, a, view),
    base.map((t) => ({ ...t, saved: savedIds.has(t.id) })),
  );
}

function SideLink({
  search,
  active,
  icon,
  label,
  count,
}: {
  search: Search;
  active: boolean;
  icon?: IconSvgElement;
  label: string;
  count?: number;
}) {
  return (
    <Link
      to="/notifications"
      search={search}
      data-active={active || undefined}
      className="text-muted-foreground hover:bg-muted hover:text-foreground data-active:bg-muted data-active:text-foreground data-active:border-primary flex items-center gap-2 rounded-sm border-l-2 border-transparent px-2 py-1.5 text-sm font-medium"
    >
      {icon && <HugeiconsIcon icon={icon} strokeWidth={2} className="size-4 shrink-0" />}
      <span className="min-w-0 flex-1 truncate">{label}</span>
      {count !== undefined && count > 0 && (
        <span className="text-muted-foreground text-xs tabular-nums">{count}</span>
      )}
    </Link>
  );
}

function Sidebar() {
  const search = Route.useSearch();
  const view = search.view ?? "inbox";
  const inbox = useRows("inbox");
  const saved = useRows("saved");
  const unread = inbox.filter((t) => t.unread).length;
  // Repository and reason counts come from the inbox, unread first, as GHES's do.
  const byRepo = countBy(inbox, (t) => t.repository.full_name);
  const byReason = countBy(inbox, (t) => t.reason);
  return (
    <nav
      aria-label="Notification filters"
      className="flex flex-col gap-4 md:sticky md:top-4"
      data-testid="notification-sidebar"
    >
      <div className="flex flex-col gap-0.5">
        <SideLink
          search={{}}
          active={view === "inbox" && !search.repo && !search.reason}
          icon={InboxIcon}
          label="Inbox"
          count={unread}
        />
        <SideLink
          search={{ view: "saved" }}
          active={view === "saved"}
          icon={Bookmark02Icon}
          label="Saved"
          count={saved.length}
        />
        <SideLink
          search={{ view: "done" }}
          active={view === "done"}
          icon={CheckmarkCircle02Icon}
          label="Done"
        />
      </div>
      <div className="flex flex-col gap-0.5">
        <h2 className="text-muted-foreground px-2 pb-1 text-xs font-medium">Repositories</h2>
        {byRepo.map(([repo, n]) => (
          <SideLink
            key={repo}
            search={{ repo }}
            active={search.repo === repo}
            icon={RepositoryIcon}
            label={repo}
            count={n}
          />
        ))}
        {byRepo.length === 0 && <p className="text-muted-foreground px-2 text-xs">None</p>}
      </div>
      <div className="flex flex-col gap-0.5">
        <h2 className="text-muted-foreground px-2 pb-1 text-xs font-medium">Reasons</h2>
        {byReason.map(([reason, n]) => (
          <SideLink
            key={reason}
            search={{ reason }}
            active={search.reason === reason}
            label={reasonLabel(reason)}
            count={n}
          />
        ))}
      </div>
    </nav>
  );
}

function countBy(rows: Row[], key: (t: Row) => string) {
  const m = new Map<string, number>();
  for (const t of rows) m.set(key(t), (m.get(key(t)) ?? 0) + 1);
  return [...m].sort((a, b) => b[1] - a[1] || a[0].localeCompare(b[0]));
}

function Inbox() {
  const search = Route.useSearch();
  const view = search.view ?? "inbox";
  const all = useRows(view);
  const run = useNotificationAction();
  const rows = all.filter(
    (t) =>
      (!search.repo || t.repository.full_name === search.repo) &&
      (!search.reason || t.reason === search.reason) &&
      (!search.unread || t.unread),
  );
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const visible = useMemo(() => new Set(rows.map((r) => r.id)), [rows]);
  // A thread that left the list (done elsewhere, or a filter change) leaves the selection.
  useEffect(() => {
    setSelected((s) => {
      const next = new Set([...s].filter((id) => visible.has(id)));
      return next.size === s.size ? s : next;
    });
  }, [visible]);
  const ids = [...selected];
  const allChecked = rows.length > 0 && selected.size === rows.length;
  const act = (a: Action) => {
    run(a);
    setSelected(new Set());
  };
  const selectedRows = rows.filter((r) => selected.has(r.id));
  const unreadHere = rows.some((r) => r.unread);

  return (
    <Frame spacing="sm" dense>
      <FrameHeader className="min-h-11 flex-row flex-wrap items-center gap-2 py-1.5">
        <Checkbox
          aria-label="Select all"
          checked={allChecked}
          indeterminate={selected.size > 0 && !allChecked}
          onCheckedChange={(on) => setSelected(on ? new Set(rows.map((r) => r.id)) : new Set())}
          disabled={!rows.length}
          data-testid="select-all"
        />
        {selected.size > 0 ? (
          <div className="flex flex-wrap items-center gap-1.5" data-testid="bulk-actions">
            <span className="text-muted-foreground mr-1 text-xs">{selected.size} selected</span>
            {view !== "done" ? (
              <>
                {selectedRows.some((r) => r.unread) ? (
                  <BulkButton icon={MailOpen01Icon} onClick={() => act({ kind: "read", ids })}>
                    Mark as read
                  </BulkButton>
                ) : (
                  <BulkButton icon={Mail01Icon} onClick={() => act({ kind: "unread", ids })}>
                    Mark as unread
                  </BulkButton>
                )}
                <BulkButton icon={CheckmarkCircle02Icon} onClick={() => act({ kind: "done", ids })}>
                  Done
                </BulkButton>
                <BulkButton
                  icon={NotificationOff02Icon}
                  onClick={() => act({ kind: "unsubscribe", ids })}
                >
                  Unsubscribe
                </BulkButton>
                {selectedRows.every((r) => r.saved) ? (
                  <BulkButton icon={Bookmark02Icon} onClick={() => act({ kind: "unsave", ids })}>
                    Unsave
                  </BulkButton>
                ) : (
                  <BulkButton icon={Bookmark02Icon} onClick={() => act({ kind: "save", ids })}>
                    Save
                  </BulkButton>
                )}
              </>
            ) : (
              <BulkButton icon={Undo02Icon} onClick={() => act({ kind: "inbox", ids })}>
                Move to inbox
              </BulkButton>
            )}
          </div>
        ) : (
          <div className="flex flex-wrap items-center gap-2">
            {view === "inbox" && (
              <div
                className="bg-background flex rounded-sm border p-0.5 text-xs"
                role="tablist"
                aria-label="Read state"
              >
                {[
                  { label: "All", unread: undefined },
                  { label: "Unread", unread: true },
                ].map((t) => (
                  <Link
                    key={t.label}
                    from="/notifications"
                    to="/notifications"
                    search={(prev) => ({ ...prev, unread: t.unread })}
                    role="tab"
                    aria-selected={Boolean(search.unread) === Boolean(t.unread)}
                    className="text-muted-foreground aria-selected:bg-muted aria-selected:text-foreground rounded-sm px-2 py-0.5 font-medium"
                  >
                    {t.label}
                  </Link>
                ))}
              </div>
            )}
            <FilterChips search={search} />
          </div>
        )}
        {view === "inbox" && (
          <Button
            variant="outline"
            size="sm"
            className="ml-auto rounded-sm"
            disabled={!unreadHere}
            onClick={() => {
              const [owner, repo] = search.repo?.split("/") ?? [];
              act({
                kind: "read-all",
                before: new Date().toISOString(),
                repo: owner ? { owner, repo } : undefined,
              });
            }}
            data-testid="mark-all-read"
          >
            <HugeiconsIcon icon={MailOpen01Icon} strokeWidth={2} data-icon="inline-start" />
            {search.repo ? `Mark all in ${search.repo} as read` : "Mark all as read"}
          </Button>
        )}
      </FrameHeader>
      <FramePanel className="rounded-t-none p-0">
        {rows.length === 0 ? (
          <EmptyBox view={view} filtered={Boolean(search.repo || search.reason || search.unread)} />
        ) : (
          <ul className="divide-y" data-testid="notification-list" data-view={view}>
            {rows.map((t) => (
              <ThreadRow
                key={t.id}
                thread={t}
                view={view}
                checked={selected.has(t.id)}
                onCheck={(on) =>
                  setSelected((s) => {
                    const next = new Set(s);
                    if (on) next.add(t.id);
                    else next.delete(t.id);
                    return next;
                  })
                }
                run={run}
              />
            ))}
          </ul>
        )}
      </FramePanel>
    </Frame>
  );
}

function FilterChips({ search }: { search: Search }) {
  const chips = [
    search.repo && { label: search.repo, drop: { repo: undefined } },
    search.reason && { label: reasonLabel(search.reason), drop: { reason: undefined } },
  ].filter(Boolean) as { label: string; drop: Partial<Search> }[];
  return chips.map((c) => (
    <Badge key={c.label} variant="outline" size="lg" className="gap-1">
      {c.label}
      <Link
        from="/notifications"
        to="/notifications"
        search={(prev) => ({ ...prev, ...c.drop })}
        aria-label={`Clear ${c.label}`}
        className="hover:text-foreground"
      >
        ×
      </Link>
    </Badge>
  ));
}

function BulkButton({
  icon,
  onClick,
  children,
}: {
  icon: IconSvgElement;
  onClick: () => void;
  children: string;
}) {
  return (
    <Button variant="outline" size="xs" className="rounded-sm" onClick={onClick}>
      <HugeiconsIcon icon={icon} strokeWidth={2} data-icon="inline-start" />
      {children}
    </Button>
  );
}

function ThreadRow({
  thread: t,
  view,
  checked,
  onCheck,
  run,
}: {
  thread: Row;
  view: View;
  checked: boolean;
  onCheck: (on: boolean) => void;
  run: ReturnType<typeof useNotificationAction>;
}) {
  const link = subjectLink(t);
  const [, repo] = t.repository.full_name.split("/");
  const number = "number" in link.params ? link.params.number : undefined;
  return (
    <li
      className={cn(
        "group hover:bg-muted/40 flex items-center gap-3 px-3 py-2",
        checked && "bg-primary/5",
      )}
      data-thread={t.id}
      data-unread={t.unread || undefined}
    >
      <Checkbox
        checked={checked}
        onCheckedChange={onCheck}
        aria-label={`Select ${t.subject.title}`}
      />
      <span
        className={cn("size-1.5 shrink-0", t.unread ? "bg-primary" : "bg-transparent")}
        aria-label={t.unread ? "Unread" : undefined}
      />
      <SubjectIcon type={t.subject.type} />
      <div className="min-w-0 flex-1">
        <p className="text-muted-foreground truncate text-xs">
          {t.repository.full_name}
          {number !== undefined && <span> #{number}</span>}
          <span className="sr-only"> {subjectLabel(t.subject.type)}</span>
        </p>
        <Link
          {...link}
          onClick={() => t.unread && run({ kind: "read", ids: [t.id] }, { quiet: true })}
          className={cn(
            "block truncate text-sm hover:underline",
            t.unread ? "font-semibold" : "text-muted-foreground",
          )}
          data-testid="thread-title"
        >
          {t.subject.title}
        </Link>
      </div>
      <Badge
        variant="outline"
        size="sm"
        className="hidden shrink-0 sm:inline-flex"
        data-testid="thread-reason"
      >
        {reasonLabel(t.reason)}
      </Badge>
      {t.saved && (
        <HugeiconsIcon
          icon={Bookmark02Icon}
          strokeWidth={2}
          className="text-primary size-3.5 shrink-0"
          aria-label="Saved"
        />
      )}
      <span className="text-muted-foreground w-24 shrink-0 text-right text-xs group-hover:hidden">
        <Ago at={t.updated_at} />
      </span>
      <div className="hidden shrink-0 items-center gap-0.5 group-hover:flex">
        {view === "done" ? (
          <RowAction
            icon={Undo02Icon}
            label="Move to inbox"
            onClick={() => run({ kind: "inbox", ids: [t.id] })}
          />
        ) : (
          <>
            {t.unread ? (
              <RowAction
                icon={MailOpen01Icon}
                label="Mark as read"
                onClick={() => run({ kind: "read", ids: [t.id] })}
              />
            ) : (
              <RowAction
                icon={Mail01Icon}
                label="Mark as unread"
                onClick={() => run({ kind: "unread", ids: [t.id] })}
              />
            )}
            <RowAction
              icon={CheckmarkCircle02Icon}
              label="Done"
              onClick={() => run({ kind: "done", ids: [t.id] })}
            />
            <RowAction
              icon={Bookmark02Icon}
              label={t.saved ? "Unsave" : "Save"}
              onClick={() => run({ kind: t.saved ? "unsave" : "save", ids: [t.id] })}
            />
            <RowAction
              icon={NotificationOff02Icon}
              label="Unsubscribe"
              onClick={() => run({ kind: "unsubscribe", ids: [t.id] })}
            />
          </>
        )}
      </div>
      <span className="sr-only">in {repo}</span>
    </li>
  );
}

function RowAction({
  icon,
  label,
  onClick,
}: {
  icon: IconSvgElement;
  label: string;
  onClick: () => void;
}) {
  return (
    <Tooltip>
      <TooltipTrigger
        render={
          <Button
            variant="ghost"
            size="icon-xs"
            className="rounded-sm"
            aria-label={label}
            onClick={onClick}
          >
            <HugeiconsIcon icon={icon} strokeWidth={2} />
          </Button>
        }
      />
      <TooltipContent>{label}</TooltipContent>
    </Tooltip>
  );
}

function EmptyBox({ view, filtered }: { view: View; filtered: boolean }) {
  const copy = filtered
    ? { title: "Nothing matches these filters", body: "Clear a filter in the sidebar to see more." }
    : {
        inbox: { title: "All caught up", body: "New notifications land here as they happen." },
        saved: { title: "Nothing saved", body: "Save a notification to come back to it later." },
        done: { title: "Nothing is done yet", body: "Notifications you mark as done move here." },
      }[view];
  return (
    <Empty className="py-12" data-testid="notifications-empty">
      <EmptyHeader>
        <EmptyMedia variant="icon">
          <HugeiconsIcon icon={view === "saved" ? Bookmark02Icon : InboxIcon} strokeWidth={2} />
        </EmptyMedia>
        <EmptyTitle>{copy.title}</EmptyTitle>
        <EmptyDescription>{copy.body}</EmptyDescription>
      </EmptyHeader>
    </Empty>
  );
}
