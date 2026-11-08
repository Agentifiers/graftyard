import { createFileRoute, Link } from "@tanstack/react-router";
import { useQuery, useSuspenseQuery } from "@tanstack/react-query";
import { useDebouncedCallback } from "@tanstack/react-pacer";
import { HugeiconsIcon } from "@hugeicons/react";
import { GitPullRequestCreateIcon, Search01Icon } from "@hugeicons/core-free-icons";
import { BranchName, CHECK_LOOK, CommentCount, ReviewBadge, StateIconOf } from "@/areas/pulls/bits";
import { assigneesQuery, labelsQuery, pullsQuery, pullSummariesQuery } from "@/areas/pulls/queries";
import { Ago, LabelBadge, pullState, StateIcon } from "@/components/github";
import { Region } from "@/components/region";
import { Frame, FrameHeader, FramePanel } from "@/components/reui/frame";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { NativeSelect, NativeSelectOption } from "@/components/ui/native-select";

// Every filter is a typed search param, so a filtered list is a link.
const STATES = ["open", "closed", "merged", "all"] as const;
const REVIEWS = ["none", "required", "approved", "changes_requested"] as const;
const DRAFTS = ["draft", "ready"] as const;
const SORTS = ["newest", "oldest", "updated", "comments"] as const;

type Search = {
  state?: (typeof STATES)[number];
  author?: string;
  label?: string;
  review?: (typeof REVIEWS)[number];
  draft?: (typeof DRAFTS)[number];
  sort?: (typeof SORTS)[number];
  q?: string;
};

const oneOf = <T extends string>(list: readonly T[], v: unknown): T | undefined =>
  list.includes(v as T) ? (v as T) : undefined;
const text = (v: unknown) => (typeof v === "string" && v ? v : undefined);

export const Route = createFileRoute("/$owner/$repo/pulls")({
  validateSearch: (s: Record<string, unknown>): Search => ({
    state: oneOf(STATES, s.state),
    author: text(s.author),
    label: text(s.label),
    review: oneOf(REVIEWS, s.review),
    draft: oneOf(DRAFTS, s.draft),
    sort: oneOf(SORTS, s.sort),
    q: text(s.q),
  }),
  loader: ({ context, params }) => {
    void context.queryClient.prefetchQuery(pullsQuery(params));
    void context.queryClient.prefetchQuery(pullSummariesQuery(params));
  },
  component: PullsPage,
});

function PullsPage() {
  const params = Route.useParams();
  return (
    <div className="flex flex-col gap-3">
      <div className="flex flex-wrap items-center gap-2">
        <SearchBox />
        <Button
          size="sm"
          className="ml-auto"
          render={<Link to="/$owner/$repo/compare" params={params} search={{}} />}
        >
          <HugeiconsIcon icon={GitPullRequestCreateIcon} strokeWidth={2} data-icon="inline-start" />
          New pull request
        </Button>
      </div>
      <Frame spacing="sm" dense>
        <FrameHeader className="flex-row flex-wrap items-center gap-2 py-2">
          <StateTabs />
          <Region name="filters" fallback={null}>
            <Filters />
          </Region>
        </FrameHeader>
        <FramePanel className="rounded-t-none p-0">
          <Region name="pull requests" rows={6}>
            <PullList />
          </Region>
        </FramePanel>
      </Frame>
    </div>
  );
}

function SearchBox() {
  const { q = "" } = Route.useSearch();
  const navigate = Route.useNavigate();
  const update = useDebouncedCallback(
    (value: string) =>
      navigate({ search: (prev) => ({ ...prev, q: value || undefined }), replace: true }),
    { wait: 250 },
  );
  return (
    <label className="relative w-full max-w-sm">
      <HugeiconsIcon
        icon={Search01Icon}
        strokeWidth={2}
        className="text-muted-foreground absolute top-1/2 left-2.5 size-3.5 -translate-y-1/2"
      />
      <Input
        type="search"
        aria-label="Filter pull requests"
        placeholder="Filter by title, #number, or branch"
        className="rounded-sm pl-8"
        defaultValue={q}
        onChange={(e) => update(e.target.value)}
      />
    </label>
  );
}

function StateTabs() {
  const { state = "open" } = Route.useSearch();
  return (
    <div
      className="bg-background flex rounded-sm border p-0.5 text-sm"
      role="tablist"
      aria-label="State"
    >
      {STATES.map((s) => (
        <Link
          key={s}
          from={Route.fullPath}
          search={(prev) => ({ ...prev, state: s === "open" ? undefined : s })}
          role="tab"
          aria-selected={state === s}
          className="text-muted-foreground aria-selected:bg-muted aria-selected:text-foreground rounded-sm px-2.5 py-1 font-medium capitalize"
        >
          {s}
        </Link>
      ))}
    </div>
  );
}

function Filters() {
  const params = Route.useParams();
  const search = Route.useSearch();
  const navigate = Route.useNavigate();
  const { data: labels } = useSuspenseQuery(labelsQuery(params));
  const { data: people } = useSuspenseQuery(assigneesQuery(params));
  const set = (patch: Partial<Search>) =>
    navigate({ search: (prev) => ({ ...prev, ...patch }), replace: true });
  const select = (
    name: string,
    value: string | undefined,
    options: [string, string][],
    onChange: (v: string | undefined) => void,
  ) => (
    <NativeSelect
      size="sm"
      aria-label={name}
      value={value ?? ""}
      onChange={(e) => onChange(e.target.value || undefined)}
      className="[&_select]:rounded-sm"
    >
      <NativeSelectOption value="">{name}</NativeSelectOption>
      {options.map(([v, l]) => (
        <NativeSelectOption key={v} value={v}>
          {l}
        </NativeSelectOption>
      ))}
    </NativeSelect>
  );
  const active = search.author || search.label || search.review || search.draft || search.q;
  return (
    <div className="ml-auto flex flex-wrap items-center gap-1.5" data-testid="pull-filters">
      {select(
        "Author",
        search.author,
        people.map((u) => [u.login, u.login]),
        (author) => set({ author }),
      )}
      {select(
        "Label",
        search.label,
        labels.map((l) => [l.name, l.name]),
        (label) => set({ label }),
      )}
      {select(
        "Reviews",
        search.review,
        [
          ["none", "No reviews"],
          ["required", "Review required"],
          ["approved", "Approved"],
          ["changes_requested", "Changes requested"],
        ],
        (v) => set({ review: oneOf(REVIEWS, v) }),
      )}
      {select(
        "Draft",
        search.draft,
        [
          ["draft", "Drafts only"],
          ["ready", "Ready for review"],
        ],
        (v) => set({ draft: oneOf(DRAFTS, v) }),
      )}
      {select(
        "Sort",
        search.sort,
        [
          ["newest", "Newest"],
          ["oldest", "Oldest"],
          ["updated", "Recently updated"],
          ["comments", "Most commented"],
        ],
        (v) => set({ sort: oneOf(SORTS, v) }),
      )}
      {active && (
        <Button
          variant="ghost"
          size="sm"
          onClick={() =>
            set({
              author: undefined,
              label: undefined,
              review: undefined,
              draft: undefined,
              q: undefined,
            })
          }
        >
          Clear filters
        </Button>
      )}
    </div>
  );
}

function PullList() {
  const params = Route.useParams();
  const search = Route.useSearch();
  const { data: pulls } = useSuspenseQuery(pullsQuery(params));
  // Check and review badges come from a second read; the rows render without them.
  const summary = useQuery(pullSummariesQuery(params));
  const byNumber = new Map((summary.data ?? []).map((s) => [s.number, s]));
  const { state = "open", sort = "newest" } = search;
  const q = search.q?.toLowerCase().replace(/^#/, "");

  const rows = pulls
    .filter((pr) => {
      const s = byNumber.get(pr.number);
      const display = pullState(pr);
      if (state === "open" && pr.state !== "open") return false;
      if (state === "closed" && display !== "pr-closed") return false;
      if (state === "merged" && display !== "pr-merged") return false;
      if (search.author && pr.user?.login !== search.author) return false;
      if (search.label && !pr.labels.some((l) => l.name === search.label)) return false;
      if (search.draft === "draft" && !pr.draft) return false;
      if (search.draft === "ready" && pr.draft) return false;
      if (search.review) {
        const d = s?.review_decision ?? null;
        const want = {
          none: null,
          required: "REVIEW_REQUIRED",
          approved: "APPROVED",
          changes_requested: "CHANGES_REQUESTED",
        }[search.review];
        if (d !== want) return false;
      }
      if (
        q &&
        !(
          pr.title.toLowerCase().includes(q) ||
          String(pr.number) === q ||
          pr.head.ref.toLowerCase().includes(q)
        )
      )
        return false;
      return true;
    })
    .sort((a, b) => {
      if (sort === "oldest") return a.created_at.localeCompare(b.created_at);
      if (sort === "updated") return b.updated_at.localeCompare(a.updated_at);
      if (sort === "comments") {
        const n = (x: typeof a) => byNumber.get(x.number)?.review_comments ?? 0;
        return n(b) - n(a);
      }
      return b.created_at.localeCompare(a.created_at);
    });

  const openCount = pulls.filter((p) => p.state === "open").length;
  if (pulls.length === 0)
    return (
      <div className="flex flex-col items-center gap-2 px-4 py-10 text-center text-sm">
        <p className="font-medium">No pull requests yet</p>
        <p className="text-muted-foreground">Compare two branches to open the first one.</p>
      </div>
    );
  return (
    <>
      <p className="text-muted-foreground border-b px-4 py-1.5 text-xs" data-testid="pull-count">
        {rows.length} shown · {openCount} open of {pulls.length}
        {summary.isError && (
          <span className="text-destructive" data-testid="summary-error">
            {" "}
            · Checks and reviews could not load.{" "}
            <button type="button" className="underline" onClick={() => void summary.refetch()}>
              Try again
            </button>
          </span>
        )}
      </p>
      {rows.length === 0 ? (
        <p className="text-muted-foreground px-4 py-10 text-center text-sm">
          No pull requests match these filters.
        </p>
      ) : (
        <ul className="divide-y" data-testid="pull-list">
          {rows.map((pr) => {
            const s = byNumber.get(pr.number);
            const display = pullState(pr);
            const check = s ? CHECK_LOOK[s.checks.state] : undefined;
            return (
              <li
                key={pr.id}
                className="hover:bg-muted/40 flex items-start gap-3 px-4 py-2.5"
                data-number={pr.number}
              >
                <span className="mt-0.5">
                  <StateIcon state={display} />
                </span>
                <div className="min-w-0 flex-1">
                  <div className="flex flex-wrap items-center gap-x-2 gap-y-1">
                    <Link
                      to="/$owner/$repo/pull/$number"
                      params={{ ...params, number: pr.number }}
                      className="text-sm font-semibold hover:underline"
                    >
                      {pr.title}
                    </Link>
                    {check && s && s.checks.total > 0 && (
                      <span
                        title={`${check.label}: ${s.checks.passed} of ${s.checks.total} passed`}
                        data-checks={s.checks.state}
                      >
                        <StateIconOf {...check} className="size-3.5" />
                      </span>
                    )}
                    {pr.labels.map((l) => (
                      <LabelBadge key={l.id} label={l} />
                    ))}
                  </div>
                  <p className="text-muted-foreground mt-0.5 flex flex-wrap items-center gap-x-1.5 text-xs">
                    <span>
                      #{pr.number}{" "}
                      {display === "pr-merged"
                        ? "merged"
                        : display === "pr-closed"
                          ? "closed"
                          : "opened"}{" "}
                      <Ago
                        at={
                          (display === "pr-merged"
                            ? pr.merged_at
                            : display === "pr-closed"
                              ? pr.closed_at
                              : pr.created_at) ?? pr.created_at
                        }
                      />{" "}
                      by {pr.user?.login}
                    </span>
                    {pr.draft && <span>· Draft</span>}
                    <span>·</span>
                    <BranchName>{pr.head.ref}</BranchName>
                    {s?.review_decision && pr.state === "open" && (
                      <ReviewBadge decision={s.review_decision} />
                    )}
                  </p>
                </div>
                <div className="flex shrink-0 items-center gap-3 self-center">
                  {pr.requested_reviewers && pr.requested_reviewers.length > 0 && (
                    <span className="text-muted-foreground text-xs" title="Review requested">
                      {pr.requested_reviewers.map((u) => u.login).join(", ")}
                    </span>
                  )}
                  <CommentCount n={s?.review_comments ?? 0} />
                </div>
              </li>
            );
          })}
        </ul>
      )}
    </>
  );
}
