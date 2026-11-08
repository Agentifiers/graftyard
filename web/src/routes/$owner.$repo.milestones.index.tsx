import { useState } from "react";
import { createFileRoute, Link } from "@tanstack/react-router";
import { useLiveSuspenseQuery } from "@tanstack/react-db";
import { HugeiconsIcon } from "@hugeicons/react";
import {
  Calendar03Icon,
  CheckmarkCircle02Icon,
  MilestoneIcon,
  SortingIcon,
} from "@hugeicons/core-free-icons";
import type { Schemas } from "@/api/client";
import { toastFailure } from "@/areas/issues/api";
import { milestonesCollection } from "@/areas/issues/collections";
import { formatDue } from "@/areas/issues/pickers";
import { Ago } from "@/components/github";
import { PageHeader } from "@/components/panel";
import { Region } from "@/components/region";
import { Frame, FrameHeader, FramePanel } from "@/components/reui/frame";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Progress } from "@/components/ui/progress";

const SORTS = {
  due: "Closest due date",
  "due-desc": "Furthest due date",
  complete: "Most complete",
  incomplete: "Least complete",
  title: "Alphabetically",
} as const;
type Sort = keyof typeof SORTS;

export const Route = createFileRoute("/$owner/$repo/milestones/")({
  validateSearch: (s: Record<string, unknown>): { state?: "closed"; sort?: Sort } => ({
    state: s.state === "closed" ? "closed" : undefined,
    sort:
      typeof s.sort === "string" && s.sort in SORTS && s.sort !== "due"
        ? (s.sort as Sort)
        : undefined,
  }),
  loader: ({ params }) =>
    void milestonesCollection(params)
      .preload()
      .catch(() => {}),
  component: MilestonesPage,
});

function MilestonesPage() {
  const params = Route.useParams();
  return (
    <div className="flex flex-col gap-3">
      <PageHeader
        title="Milestones"
        description="Group issues toward a release or a date and follow their progress."
        actions={
          <>
            <Button
              variant="outline"
              size="sm"
              nativeButton={false}
              render={<Link to="/$owner/$repo/labels" params={params} />}
            >
              Labels
            </Button>
            <Button
              size="sm"
              nativeButton={false}
              render={<Link to="/$owner/$repo/milestones/new" params={params} />}
            >
              New milestone
            </Button>
          </>
        }
      />
      <Frame spacing="sm" dense>
        <Region name="milestones" rows={6} onRetry={() => milestonesCollection(params).reload()}>
          <MilestoneList />
        </Region>
      </Frame>
    </div>
  );
}

const done = (m: Schemas["milestone"]) => {
  const total = m.open_issues + m.closed_issues;
  return total ? m.closed_issues / total : 0;
};

function MilestoneList() {
  const params = Route.useParams();
  const { state = "open", sort = "due" } = Route.useSearch();
  const navigate = Route.useNavigate();
  const { data } = useLiveSuspenseQuery({
    query: (q) => q.from({ m: milestonesCollection(params) }),
  });
  const open = data.filter((m) => m.state === "open");
  const closed = data.filter((m) => m.state === "closed");
  const compare: Record<Sort, (a: Schemas["milestone"], b: Schemas["milestone"]) => number> = {
    due: (a, b) => (a.due_on ?? "9999").localeCompare(b.due_on ?? "9999"),
    "due-desc": (a, b) => (b.due_on ?? "0000").localeCompare(a.due_on ?? "0000"),
    complete: (a, b) => done(b) - done(a),
    incomplete: (a, b) => done(a) - done(b),
    title: (a, b) => a.title.localeCompare(b.title),
  };
  const shown = (state === "open" ? open : closed).sort(compare[sort]);
  return (
    <>
      <FrameHeader className="flex-row flex-wrap items-center gap-4 py-2 text-sm">
        <Link
          from={Route.fullPath}
          search={(prev) => ({ ...prev, state: undefined })}
          data-active={state === "open" || undefined}
          className="text-muted-foreground data-active:text-foreground flex items-center gap-1.5 font-medium data-active:font-semibold"
        >
          <HugeiconsIcon icon={MilestoneIcon} strokeWidth={2} className="size-4" />
          <span className="tabular-nums">{open.length}</span> Open
        </Link>
        <Link
          from={Route.fullPath}
          search={(prev) => ({ ...prev, state: "closed" as const })}
          data-active={state === "closed" || undefined}
          className="text-muted-foreground data-active:text-foreground flex items-center gap-1.5 font-medium data-active:font-semibold"
        >
          <HugeiconsIcon icon={CheckmarkCircle02Icon} strokeWidth={2} className="size-4" />
          <span className="tabular-nums">{closed.length}</span> Closed
        </Link>
        <DropdownMenu>
          <DropdownMenuTrigger
            render={
              <Button variant="ghost" size="sm" className="ml-auto">
                <HugeiconsIcon icon={SortingIcon} strokeWidth={2} data-icon="inline-start" />
                {SORTS[sort]}
              </Button>
            }
          />
          <DropdownMenuContent align="end" className="w-48">
            <DropdownMenuRadioGroup
              value={sort}
              onValueChange={(v) =>
                void navigate({
                  search: (prev) => ({ ...prev, sort: v === "due" ? undefined : (v as Sort) }),
                })
              }
            >
              {(Object.keys(SORTS) as Sort[]).map((s) => (
                <DropdownMenuRadioItem key={s} value={s}>
                  {SORTS[s]}
                </DropdownMenuRadioItem>
              ))}
            </DropdownMenuRadioGroup>
          </DropdownMenuContent>
        </DropdownMenu>
      </FrameHeader>
      <FramePanel className="rounded-t-none p-0">
        {shown.length === 0 ? (
          <div className="px-4 py-10 text-center text-sm" data-testid="milestones-empty">
            <p className="font-semibold">
              {state === "open" ? "No open milestones." : "No closed milestones."}
            </p>
            <p className="text-muted-foreground mt-1">
              Milestones group issues toward a target date.
            </p>
          </div>
        ) : (
          <ul className="divide-y" data-testid="milestone-list">
            {shown.map((m) => (
              <MilestoneRow key={m.number} milestone={m} pending={m.$hasPendingWrites} />
            ))}
          </ul>
        )}
      </FramePanel>
    </>
  );
}

function MilestoneRow({
  milestone: m,
  pending,
}: {
  milestone: Schemas["milestone"];
  pending: boolean;
}) {
  const params = Route.useParams();
  const [confirm, setConfirm] = useState(false);
  const pct = Math.round(done(m) * 100);
  const overdue = m.state === "open" && m.due_on && new Date(m.due_on) < new Date();
  const write = async (what: string, tx: { when: (s: "settled") => Promise<unknown> }) => {
    try {
      await tx.when("settled");
    } catch (e) {
      toastFailure(what, e);
    }
  };
  const c = milestonesCollection(params);
  return (
    <li
      className="grid gap-3 px-4 py-3 sm:grid-cols-[minmax(0,1fr)_minmax(0,18rem)]"
      data-milestone={m.number}
      data-pending={pending || undefined}
    >
      <div className="min-w-0">
        <Link
          to="/$owner/$repo/milestones/$number"
          params={{ ...params, number: m.number }}
          className="text-base font-semibold hover:underline"
        >
          {m.title}
        </Link>
        <p className="text-muted-foreground mt-1 flex flex-wrap items-center gap-x-3 gap-y-1 text-xs">
          <span
            className={
              overdue
                ? "text-destructive flex items-center gap-1 font-medium"
                : "flex items-center gap-1"
            }
          >
            <HugeiconsIcon icon={Calendar03Icon} strokeWidth={2} className="size-3.5" />
            {m.state === "closed" && m.closed_at ? (
              <>
                Closed <Ago at={m.closed_at} />
              </>
            ) : m.due_on ? (
              <>
                {overdue ? "Past due by" : "Due by"} {formatDue(m.due_on)}
              </>
            ) : (
              "No due date"
            )}
          </span>
          <span>
            Last updated <Ago at={m.updated_at} />
          </span>
        </p>
        {m.description && <p className="text-muted-foreground mt-1 text-sm">{m.description}</p>}
      </div>
      <div className="flex flex-col gap-1.5">
        <Progress value={pct} aria-label={`${m.title} progress`} className="gap-1" />
        <p className="text-muted-foreground flex gap-3 text-xs">
          <span>
            <span className="text-foreground font-semibold">{pct}%</span> complete
          </span>
          <span>{m.open_issues} open</span>
          <span>{m.closed_issues} closed</span>
        </p>
        <div className="flex flex-wrap gap-1 text-xs">
          <Button
            variant="link"
            size="xs"
            className="h-auto px-0"
            nativeButton={false}
            disabled={m.number < 0}
            render={
              <Link
                to="/$owner/$repo/milestones/$number/edit"
                params={{ ...params, number: m.number }}
              />
            }
          >
            Edit
          </Button>
          <Button
            variant="link"
            size="xs"
            className="h-auto px-1"
            disabled={m.number < 0}
            onClick={() =>
              void write(
                m.state === "open" ? "Closing the milestone" : "Reopening the milestone",
                c.update(m.number, (d) => {
                  d.state = d.state === "open" ? "closed" : "open";
                  d.closed_at = d.state === "closed" ? new Date().toISOString() : null;
                }),
              )
            }
          >
            {m.state === "open" ? "Close" : "Reopen"}
          </Button>
          <Button
            variant="link"
            size="xs"
            className="text-destructive h-auto px-1"
            disabled={m.number < 0}
            onClick={() => setConfirm(true)}
          >
            Delete
          </Button>
        </div>
      </div>
      <AlertDialog open={confirm} onOpenChange={setConfirm}>
        <AlertDialogContent className="rounded-sm">
          <AlertDialogHeader>
            <AlertDialogTitle>Delete “{m.title}”?</AlertDialogTitle>
            <AlertDialogDescription>
              Its issues stay open or closed as they are; they just lose the milestone.
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>Cancel</AlertDialogCancel>
            <AlertDialogAction
              variant="destructive"
              onClick={() => {
                setConfirm(false);
                void write("Deleting the milestone", c.delete(m.number));
              }}
            >
              Delete
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </li>
  );
}
