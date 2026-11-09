import { useState } from "react";
import { createFileRoute, Link } from "@tanstack/react-router";
import { useSuspenseQuery } from "@tanstack/react-query";
import { useLiveSuspenseQuery } from "@tanstack/react-db";
import { useDebouncedCallback } from "@tanstack/react-pacer";
import { HugeiconsIcon } from "@hugeicons/react";
import {
  CheckmarkCircle02Icon,
  CircleIcon,
  Comment01Icon,
  MilestoneIcon,
  PinIcon,
  Search01Icon,
  SortingIcon,
  Tag01Icon,
  UnavailableIcon,
} from "@hugeicons/core-free-icons";
import type { Schemas } from "@/api/client";
import { issuesCollection, pinnedCollection } from "@/areas/issues/collections";
import {
  matchesExceptState,
  NONE,
  OPERATORS,
  PER_PAGE,
  sortKey,
  SORTS,
  stateOf,
  validateIssueSearch,
  withState,
  type Rule,
  type Sort,
} from "@/areas/issues/filters";
import { LabelDot } from "@/areas/issues/pickers";
import { assigneesQuery, labelsQuery, milestonesQuery } from "@/areas/issues/queries";
import { Ago, LabelBadge, StateIcon, UserAvatar } from "@/components/github";
import { Region } from "@/components/region";
import { Frame, FrameHeader, FramePanel } from "@/components/reui/frame";
import {
  createFilterQuery,
  createFilterRule,
  Filters,
  flattenFilterRules,
  type FilterField,
  type FilterQuery,
} from "@/components/reui/filters/filters";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuGroup,
  DropdownMenuLabel,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Input } from "@/components/ui/input";
import { Pagination, PaginationContent, PaginationItem } from "@/components/ui/pagination";

export const Route = createFileRoute("/$owner/$repo/issues/")({
  validateSearch: validateIssueSearch,
  // Start syncing early; the region shows any failure.
  loader: ({ context, params }) => {
    void issuesCollection(params)
      .preload()
      .catch(() => {});
    void context.queryClient.prefetchQuery(labelsQuery(params));
    void context.queryClient.prefetchQuery(milestonesQuery(params));
    void context.queryClient.prefetchQuery(assigneesQuery(params));
  },
  component: IssuesPage,
});

function IssuesPage() {
  const params = Route.useParams();
  return (
    <div className="flex flex-col gap-3">
      <div className="flex flex-wrap items-center gap-2">
        <SearchBox />
        <Button
          variant="outline"
          size="sm"
          render={<Link to="/$owner/$repo/labels" params={params} />}
          nativeButton={false}
        >
          <HugeiconsIcon icon={Tag01Icon} strokeWidth={2} data-icon="inline-start" />
          Labels
        </Button>
        <Button
          variant="outline"
          size="sm"
          render={<Link to="/$owner/$repo/milestones" params={params} />}
          nativeButton={false}
        >
          <HugeiconsIcon icon={MilestoneIcon} strokeWidth={2} data-icon="inline-start" />
          Milestones
        </Button>
        <Button
          size="sm"
          render={<Link to="/$owner/$repo/issues/new" params={params} />}
          nativeButton={false}
        >
          New issue
        </Button>
      </div>
      <Region name="issue filters" fallback={<div className="h-8" />}>
        <FilterBar />
      </Region>
      <Region
        name="pinned issues"
        fallback={null}
        onRetry={() => issuesCollection(params).reload()}
      >
        <Pinned />
      </Region>
      <Frame spacing="sm" dense>
        <Region name="issues" rows={8} onRetry={() => issuesCollection(params).reload()}>
          <IssueList />
        </Region>
      </Frame>
    </div>
  );
}

// Typing updates `q` in the URL, debounced so history and the live query do not churn.
function SearchBox() {
  const { q = "" } = Route.useSearch();
  const navigate = Route.useNavigate();
  const setQ = useDebouncedCallback(
    (value: string) =>
      void navigate({
        search: (prev) => ({ ...prev, q: value || undefined, page: undefined }),
        replace: true,
      }),
    { wait: 200 },
  );
  return (
    <label className="relative min-w-48 flex-1">
      <HugeiconsIcon
        icon={Search01Icon}
        strokeWidth={2}
        className="text-muted-foreground absolute top-1/2 left-2.5 size-3.5 -translate-y-1/2"
      />
      <Input
        type="search"
        aria-label="Search issues"
        placeholder="Search by title or #number"
        className="rounded-sm pl-8"
        defaultValue={q}
        onChange={(e) => setQ(e.target.value)}
      />
    </label>
  );
}

// ---- The ReUI filter bar, kept in sync with the `filters` search param ----

type Value = string[];

function toQuery(rules: Rule[] = []): FilterQuery<Value> {
  return createFilterQuery(
    rules.map((r, i) =>
      createFilterRule<Value>({ id: `r${i}-${r.f}`, path: [r.f], operator: r.op, value: r.v }),
    ),
  );
}

function fromQuery(query: FilterQuery<Value>): Rule[] | undefined {
  const rules = flattenFilterRules(query).flatMap((r): Rule[] => {
    const f = r.path[0];
    const v = Array.isArray(r.value)
      ? r.value.filter((x): x is string => typeof x === "string")
      : [];
    if (!v.length) return [];
    const parsed = validateIssueSearch({ filters: [{ f, op: r.operator, v }] }).filters;
    return parsed ?? [];
  });
  return rules.length ? rules : undefined;
}

const op = (value: string, label: string) => ({ value, label, arity: "many" as const });
const OP_LABELS: Record<string, string> = {
  is_any_of: "is",
  is_none_of: "is not",
  has_any_of: "has any of",
  has_all_of: "has all of",
  has_none_of: "has none of",
};

function FilterBar() {
  const params = Route.useParams();
  const search = Route.useSearch();
  const navigate = Route.useNavigate();
  const { data: labels } = useSuspenseQuery(labelsQuery(params));
  const { data: milestones } = useSuspenseQuery(milestonesQuery(params));
  const { data: people } = useSuspenseQuery(assigneesQuery(params));
  // The bar holds rules still being built (a field with no value yet); the URL holds only
  // complete ones. A URL change from elsewhere (a state tab, a label link) resets the bar.
  const urlKey = JSON.stringify(search.filters ?? []);
  const [draft, setDraft] = useState(() => ({ key: urlKey, query: toQuery(search.filters) }));
  const query = draft.key === urlKey ? draft.query : toQuery(search.filters);
  const ops = (f: Rule["f"]) => OPERATORS[f].map((o) => op(o, OP_LABELS[o]));
  const avatar = (u: Schemas["simple-user"]) => <UserAvatar user={u} />;
  const fields: FilterField<Value>[] = [
    {
      id: "state",
      label: "State",
      icon: <HugeiconsIcon icon={CircleIcon} strokeWidth={2} />,
      type: "multiselect",
      operators: ops("state"),
      options: [
        { value: "open", label: "Open" },
        { value: "closed", label: "Closed" },
      ],
    },
    {
      id: "author",
      label: "Author",
      type: "multiselect",
      operators: ops("author"),
      options: people.map((u) => ({ value: u.login, label: u.login, icon: avatar(u) })),
    },
    {
      id: "label",
      label: "Label",
      icon: <HugeiconsIcon icon={Tag01Icon} strokeWidth={2} />,
      type: "multiselect",
      operators: ops("label"),
      options: labels.map((l) => ({
        value: l.name,
        label: l.name,
        icon: <LabelDot color={l.color} />,
      })),
    },
    {
      id: "assignee",
      label: "Assignee",
      type: "multiselect",
      operators: ops("assignee"),
      options: [
        ...people.map((u) => ({ value: u.login, label: u.login, icon: avatar(u) })),
        { value: NONE, label: "No one assigned" },
      ],
    },
    {
      id: "milestone",
      label: "Milestone",
      icon: <HugeiconsIcon icon={MilestoneIcon} strokeWidth={2} />,
      type: "multiselect",
      operators: ops("milestone"),
      options: [
        ...milestones.map((m) => ({ value: String(m.number), label: m.title })),
        { value: NONE, label: "No milestone" },
      ],
    },
  ];
  return (
    <div className="flex flex-wrap items-start gap-2" data-testid="filter-bar">
      <div className="min-w-0 flex-1">
        <Filters<Value>
          fields={fields}
          query={query}
          size="sm"
          showClear
          onQueryChange={(next) => {
            const filters = fromQuery(next);
            setDraft({ key: JSON.stringify(filters ?? []), query: next });
            void navigate({ search: (prev) => ({ ...prev, filters, page: undefined }) });
          }}
        />
      </div>
      <SortMenu />
    </div>
  );
}

function SortMenu() {
  const { sort = "newest" } = Route.useSearch();
  const navigate = Route.useNavigate();
  return (
    <DropdownMenu>
      <DropdownMenuTrigger
        render={
          <Button variant="outline" size="sm" data-sort={sort}>
            <HugeiconsIcon icon={SortingIcon} strokeWidth={2} data-icon="inline-start" />
            {SORTS[sort]}
          </Button>
        }
      />
      <DropdownMenuContent align="end" className="w-48">
        <DropdownMenuGroup>
          <DropdownMenuLabel>Sort by</DropdownMenuLabel>
          <DropdownMenuRadioGroup
            value={sort}
            onValueChange={(v) =>
              void navigate({
                search: (prev) => ({
                  ...prev,
                  sort: v === "newest" ? undefined : (v as Sort),
                  page: undefined,
                }),
              })
            }
          >
            {(Object.keys(SORTS) as Sort[]).map((s) => (
              <DropdownMenuRadioItem key={s} value={s}>
                {SORTS[s]}
              </DropdownMenuRadioItem>
            ))}
          </DropdownMenuRadioGroup>
        </DropdownMenuGroup>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

// ---- Pinned issues, from this browser's pins ----

function Pinned() {
  const params = Route.useParams();
  const issues = issuesCollection(params);
  const { data: pins } = useLiveSuspenseQuery({
    query: (q) => q.from({ p: pinnedCollection() }).orderBy(({ p }) => p.pinnedAt, "asc"),
  });
  const { data } = useLiveSuspenseQuery({ query: (q) => q.from({ issue: issues }) });
  const mine = pins
    .filter((p) => p.owner === params.owner.toLowerCase() || p.owner === params.owner)
    .filter((p) => p.repo.toLowerCase() === params.repo.toLowerCase())
    .flatMap((p) => data.filter((i) => i.number === p.number));
  if (mine.length === 0) return null;
  return (
    <section
      className="grid gap-2 sm:grid-cols-3"
      aria-label="Pinned issues"
      data-testid="pinned-issues"
    >
      {mine.map((i) => (
        <Link
          key={i.number}
          to="/$owner/$repo/issues/$number"
          params={{ ...params, number: i.number }}
          className="bg-card hover:bg-muted/40 flex flex-col gap-1 rounded-sm border p-3 text-sm"
        >
          <span className="text-muted-foreground flex items-center gap-1 text-xs">
            <HugeiconsIcon icon={PinIcon} strokeWidth={2} className="size-3.5" />
            Pinned
          </span>
          <span className="font-semibold">{i.title}</span>
          <span className="text-muted-foreground text-xs">#{i.number}</span>
        </Link>
      ))}
    </section>
  );
}

// ---- The list ----

function IssueList() {
  const params = Route.useParams();
  const search = Route.useSearch();
  const { filters, q = "", page = 1 } = search;
  const state = stateOf(filters);
  const { by, dir } = sortKey(search.sort);
  const issues = issuesCollection(params);
  // The live query applies every rule but state and sorts; state splits it into the
  // open and closed counts and the shown rows.
  const { data } = useLiveSuspenseQuery({
    query: (qb) =>
      qb
        .from({ issue: issues })
        .fn.where(({ issue }) => matchesExceptState(issue, filters, q))
        .orderBy(({ issue }) => issue[by], dir)
        .orderBy(({ issue }) => issue.number, dir),
    queryKey: ["issues-list", params.owner, params.repo, JSON.stringify(filters ?? []), q, by, dir],
  });
  const open = data.filter((i) => i.state === "open");
  const closed = data.filter((i) => i.state === "closed");
  const shown = state === "all" ? data : state === "open" ? open : closed;
  const pages = Math.max(1, Math.ceil(shown.length / PER_PAGE));
  const current = Math.min(page, pages);
  const rows = shown.slice((current - 1) * PER_PAGE, current * PER_PAGE);
  const filtered = Boolean(filters?.some((r) => r.f !== "state") || q);

  return (
    <>
      <FrameHeader className="flex-row flex-wrap items-center gap-3 py-2 text-sm">
        <StateTab
          to="open"
          active={state === "open"}
          count={open.length}
          icon={CircleIcon}
          label="Open"
        />
        <StateTab
          to="closed"
          active={state === "closed"}
          count={closed.length}
          icon={CheckmarkCircle02Icon}
          label="Closed"
        />
        <StateTab to="all" active={state === "all"} count={data.length} label="All" />
      </FrameHeader>
      <FramePanel className="rounded-t-none p-0">
        {rows.length === 0 ? (
          <div className="px-4 py-10 text-center text-sm" data-testid="issues-empty">
            <p className="font-semibold">No results matched your search.</p>
            <p className="text-muted-foreground mt-1">
              {filtered
                ? "Try removing a filter or changing the search."
                : "There are no issues here yet."}
            </p>
          </div>
        ) : (
          <ul className="divide-y" data-testid="issue-list">
            {rows.map((issue) => (
              <IssueRow key={issue.number} issue={issue} pending={issue.$hasPendingWrites} />
            ))}
          </ul>
        )}
      </FramePanel>
      {pages > 1 && <Pager current={current} pages={pages} />}
    </>
  );
}

function StateTab({
  to,
  active,
  count,
  icon,
  label,
}: {
  to: "open" | "closed" | "all";
  active: boolean;
  count: number;
  icon?: typeof CircleIcon;
  label: string;
}) {
  return (
    <Link
      from={Route.fullPath}
      search={(prev) => ({ ...prev, filters: withState(prev.filters, to), page: undefined })}
      data-active={active || undefined}
      data-state-tab={to}
      className="text-muted-foreground data-active:text-foreground flex items-center gap-1.5 font-medium data-active:font-semibold"
    >
      {icon && <HugeiconsIcon icon={icon} strokeWidth={2} className="size-4" />}
      <span className="tabular-nums">{count}</span> {label}
    </Link>
  );
}

function IssueRow({ issue, pending }: { issue: Schemas["issue"]; pending: boolean }) {
  const params = Route.useParams();
  const notPlanned = issue.state === "closed" && issue.state_reason === "not_planned";
  return (
    <li
      className="hover:bg-muted/40 flex items-start gap-3 px-4 py-2.5"
      data-pending={pending || undefined}
    >
      <span className="mt-0.5">
        {notPlanned ? (
          <HugeiconsIcon
            icon={UnavailableIcon}
            strokeWidth={2}
            className="text-muted-foreground size-4"
            aria-label="Closed as not planned"
          />
        ) : (
          <StateIcon state={issue.state === "closed" ? "closed" : "open"} />
        )}
      </span>
      <div className="min-w-0 flex-1">
        <div className="flex flex-wrap items-center gap-x-2 gap-y-1">
          {issue.number > 0 ? (
            <Link
              to="/$owner/$repo/issues/$number"
              params={{ ...params, number: issue.number }}
              className="text-sm font-semibold hover:underline"
            >
              {issue.title}
            </Link>
          ) : (
            <span className="text-sm font-semibold">{issue.title}</span>
          )}
          {issue.labels.map((l) => (
            <LabelBadge key={typeof l === "string" ? l : l.name} label={l} />
          ))}
        </div>
        <p className="text-muted-foreground mt-0.5 flex flex-wrap items-center gap-x-1 text-xs">
          {issue.number > 0 ? `#${issue.number}` : "Creating…"}{" "}
          {issue.state === "closed" ? "closed" : "opened"}{" "}
          <Ago
            at={issue.state === "closed" && issue.closed_at ? issue.closed_at : issue.created_at}
          />{" "}
          by {issue.user?.login}
          {issue.milestone && (
            <span className="ml-2 flex items-center gap-1">
              <HugeiconsIcon icon={MilestoneIcon} strokeWidth={2} className="size-3" />
              {issue.milestone.title}
            </span>
          )}
        </p>
      </div>
      <div className="flex shrink-0 items-center gap-3 self-center">
        {(issue.assignees ?? []).length > 0 && (
          <span
            className="flex -space-x-1"
            title={(issue.assignees ?? []).map((u) => u.login).join(", ")}
          >
            {(issue.assignees ?? []).map((u) => (
              <UserAvatar key={u.login} user={u} />
            ))}
          </span>
        )}
        {issue.comments > 0 && (
          <span
            className="text-muted-foreground flex w-8 items-center justify-end gap-1 text-xs tabular-nums"
            title={`${issue.comments} comments`}
          >
            <HugeiconsIcon icon={Comment01Icon} strokeWidth={2} className="size-3.5" />
            {issue.comments}
          </span>
        )}
      </div>
    </li>
  );
}

function Pager({ current, pages }: { current: number; pages: number }) {
  const to = (n: number, label: React.ReactNode, aria?: string, active?: boolean) => {
    const disabled = n < 1 || n > pages;
    return (
      <Button
        variant={active ? "outline" : "ghost"}
        size={typeof label === "number" ? "icon" : "default"}
        aria-label={aria}
        aria-current={active ? "page" : undefined}
        disabled={disabled}
        nativeButton={false}
        render={
          <Link
            from={Route.fullPath}
            search={(prev) => ({ ...prev, page: n > 1 ? n : undefined })}
            disabled={disabled}
            data-page={n}
          />
        }
      >
        {label}
      </Button>
    );
  };
  return (
    <Pagination className="border-t py-2" data-testid="pager">
      <PaginationContent>
        <PaginationItem>{to(current - 1, "Previous", "Go to previous page")}</PaginationItem>
        {Array.from({ length: pages }, (_, i) => i + 1).map((n) => (
          <PaginationItem key={n}>{to(n, n, `Page ${n}`, n === current)}</PaginationItem>
        ))}
        <PaginationItem>{to(current + 1, "Next", "Go to next page")}</PaginationItem>
      </PaginationContent>
    </Pagination>
  );
}
