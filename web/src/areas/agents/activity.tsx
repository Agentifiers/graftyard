import { useEffect, useReducer } from "react";
import { Link } from "@tanstack/react-router";
import { useQueryClient } from "@tanstack/react-query";
import { HugeiconsIcon } from "@hugeicons/react";
import {
  Alert02Icon,
  GitBranchIcon,
  GitPullRequestIcon,
  LockIcon,
  SecurityCheckIcon,
  Task01Icon,
} from "@hugeicons/core-free-icons";
import { Badge, type BadgeProps } from "@/components/reui/badge";
import { Button } from "@/components/ui/button";
import { Spinner } from "@/components/ui/spinner";
import { shortId } from "./states";
import { GraftyardError, gy, type ActivityEvent, type Verdict } from "./api";
import { leasesKey, tasksKey } from "./queries";
import { TaskStateBadge } from "./task-state";
import { TrustInline } from "./trust-view";

type Repo = { owner: string; repo: string };

// The feed client as a reducer: each answer moves the cursor and prepends its events.
// A 410 restarts from `oldest_cursor` and records a gap; a 429 or a 5xx backs off.
type Received = { seq: number; at: string; event: ActivityEvent };
type Feed = {
  cursor: string | undefined;
  events: Received[];
  status: "connecting" | "live" | "backoff" | "stopped";
  gaps: number;
  error?: string;
  seq: number;
};

type Action =
  | { type: "events"; cursor: string; events: ActivityEvent[] }
  | { type: "gone"; oldest: string }
  | { type: "backoff"; error: string }
  | { type: "stop" }
  | { type: "resume" };

const KEEP = 200;

function reduce(s: Feed, a: Action): Feed {
  switch (a.type) {
    case "events": {
      const at = new Date().toISOString();
      const fresh = a.events.map((event, i) => ({ seq: s.seq + i, at, event })).reverse();
      return {
        ...s,
        cursor: a.cursor,
        status: "live",
        error: undefined,
        seq: s.seq + a.events.length,
        events: [...fresh, ...s.events].slice(0, KEEP),
      };
    }
    case "gone":
      return { ...s, cursor: a.oldest, gaps: s.gaps + 1, status: "live" };
    case "backoff":
      return { ...s, status: "backoff", error: a.error };
    case "stop":
      return { ...s, status: "stopped" };
    case "resume":
      return { ...s, status: "connecting" };
  }
}

const WAIT = 25;

export function useActivityFeed(r: Repo) {
  const [feed, dispatch] = useReducer(reduce, {
    cursor: undefined,
    events: [],
    status: "connecting",
    gaps: 0,
    seq: 0,
  });
  const client = useQueryClient();
  const running = feed.status !== "stopped";

  useEffect(() => {
    if (!running) return;
    const abort = new AbortController();
    let cursor = feed.cursor;
    let failures = 0;
    void (async () => {
      while (!abort.signal.aborted) {
        const { data, error, response } = await gy
          .GET("/repos/{owner}/{repo}/activity", {
            params: { path: r, query: { cursor, wait: cursor === undefined ? 0 : WAIT } },
            signal: abort.signal,
          })
          .catch((e: unknown) => ({ data: undefined, error: e, response: undefined }));
        if (abort.signal.aborted) return;
        if (data) {
          failures = 0;
          cursor = data.cursor;
          dispatch({ type: "events", cursor: data.cursor, events: data.events });
          // The feed says what moved; the lists refetch what it touched.
          if (data.events.some((e) => e.type === "lease"))
            void client.invalidateQueries({ queryKey: leasesKey(r) });
          if (data.events.some((e) => e.type === "task" || e.type === "agent_pr"))
            void client.invalidateQueries({ queryKey: tasksKey(r) });
          continue;
        }
        if (response?.status === 410) {
          const gone = new GraftyardError(response, error);
          cursor = gone.body.oldest_cursor;
          dispatch({ type: "gone", oldest: cursor ?? "" });
          // A cursor older than the ring: reload leases and tasks, as the design says.
          void client.invalidateQueries({ queryKey: leasesKey(r) });
          void client.invalidateQueries({ queryKey: tasksKey(r) });
          continue;
        }
        failures++;
        const err = response ? new GraftyardError(response, error) : undefined;
        const delay = err?.retryAfter
          ? err.retryAfter * 1000
          : Math.random() * Math.min(15_000, 1000 * 2 ** failures);
        dispatch({
          type: "backoff",
          error: err ? `${err.status}: ${err.message}` : "The connection failed.",
        });
        await new Promise((ok) => setTimeout(ok, delay));
      }
    })();
    return () => abort.abort();
  }, [r.owner, r.repo, running]);

  return {
    feed,
    stop: () => dispatch({ type: "stop" }),
    resume: () => dispatch({ type: "resume" }),
  };
}

export function ActivityFeed(r: Repo) {
  const { feed, stop, resume } = useActivityFeed(r);
  return (
    <div className="flex flex-col" data-feed-status={feed.status} data-feed-gaps={feed.gaps}>
      <div className="text-muted-foreground flex items-center gap-2 border-b px-4 py-1.5 text-xs">
        {feed.status === "connecting" && <Spinner className="size-3" />}
        {feed.status === "live" && (
          <span className="bg-success inline-block size-2 rounded-sm" aria-hidden />
        )}
        {feed.status === "backoff" && (
          <span className="bg-warning inline-block size-2 rounded-sm" aria-hidden />
        )}
        <span>
          {
            {
              connecting: "Connecting…",
              live: `Live. Long-polling every ${WAIT} s or on the next event.`,
              backoff: `Retrying. ${feed.error ?? ""}`,
              stopped: "Paused.",
            }[feed.status]
          }
        </span>
        <Button
          size="xs"
          variant="ghost"
          className="ml-auto"
          onClick={feed.status === "stopped" ? resume : stop}
        >
          {feed.status === "stopped" ? "Resume" : "Pause"}
        </Button>
      </div>
      {feed.gaps > 0 && (
        <p
          className="border-warning/40 bg-warning/10 text-warning-foreground dark:text-warning flex items-center gap-1.5 border-b px-4 py-1.5 text-xs"
          data-missed-events
        >
          <HugeiconsIcon icon={Alert02Icon} strokeWidth={2} className="size-3.5" />
          Missed events: the feed moved past this page's cursor{" "}
          {feed.gaps === 1 ? "once" : `${feed.gaps} times`}. It restarted from the oldest event it
          still holds, and leases and tasks were reloaded.
        </p>
      )}
      {feed.events.length === 0 ? (
        <p className="text-muted-foreground px-4 py-8 text-center text-sm" data-feed-empty>
          Waiting for the next event. Leases, task states, agent pushes, verdicts, and agent PRs
          appear here.
        </p>
      ) : (
        <ol className="max-h-[28rem] divide-y overflow-y-auto" data-testid="feed">
          {feed.events.map((e) => (
            <li
              key={e.seq}
              className="flex items-start gap-2 px-4 py-2 text-xs"
              data-event={e.event.type}
            >
              <EventRow event={e.event} {...r} />
              <time className="text-muted-foreground ml-auto shrink-0 tabular-nums" dateTime={e.at}>
                {new Date(e.at).toLocaleTimeString()}
              </time>
            </li>
          ))}
        </ol>
      )}
    </div>
  );
}

const VERDICT: Record<Verdict, { label: string; variant: BadgeProps["variant"] }> = {
  clean: { label: "Clean", variant: "success-light" },
  path_overlap: { label: "Path overlap", variant: "warning-light" },
  conflict: { label: "Conflict", variant: "destructive-light" },
  stale: { label: "Stale", variant: "outline" },
};

export function VerdictBadge({ verdict }: { verdict: Verdict }) {
  const v = VERDICT[verdict];
  return (
    <Badge variant={v.variant} size="sm">
      {v.label}
    </Badge>
  );
}

const icon = "text-muted-foreground mt-0.5 size-3.5 shrink-0";

function EventRow({ event: e, owner, repo }: Repo & { event: ActivityEvent }) {
  switch (e.type) {
    case "lease":
      return (
        <>
          <HugeiconsIcon icon={LockIcon} strokeWidth={2} className={icon} />
          <div className="flex min-w-0 flex-col gap-0.5">
            <span>
              {e.lease.mode === "exclusive" ? "Exclusive" : "Shared"} lease{" "}
              <strong className="font-medium">{e.change}</strong> on{" "}
              {e.lease.paths.map((p) => (
                <code key={p} className="bg-muted mr-1 rounded-sm px-1 font-mono">
                  {p}
                </code>
              ))}
            </span>
            {(e.lease.intent.visible || e.lease.intent.hidden.html_comments > 0) && (
              <span className="text-muted-foreground">
                <TrustInline view={e.lease.intent} />
              </span>
            )}
          </div>
        </>
      );
    case "task":
      return (
        <>
          <HugeiconsIcon icon={Task01Icon} strokeWidth={2} className={icon} />
          <span className="flex flex-wrap items-center gap-1.5">
            Task
            <Link
              to="/$owner/$repo/_graftyard/sessions/$uuid"
              params={{ owner, repo, uuid: e.task_id }}
              className="font-mono hover:underline"
            >
              {shortId(e.task_id)}
            </Link>
            is now <TaskStateBadge state={e.state} size="sm" />
          </span>
        </>
      );
    case "agent_push":
      return (
        <>
          <HugeiconsIcon icon={GitBranchIcon} strokeWidth={2} className={icon} />
          <span className="min-w-0">
            Agent pushed{" "}
            <Link
              to="/$owner/$repo/commit/$sha"
              params={{ owner, repo, sha: e.head_sha }}
              className="font-mono hover:underline"
            >
              {e.head_sha.slice(0, 7)}
            </Link>{" "}
            to <code className="font-mono">{e.ref.replace("refs/heads/", "")}</code>, touching{" "}
            {e.paths.map((p) => (
              <code key={p} className="bg-muted mr-1 rounded-sm px-1 font-mono">
                {p}
              </code>
            ))}
          </span>
        </>
      );
    case "verdict":
      return (
        <>
          <HugeiconsIcon icon={SecurityCheckIcon} strokeWidth={2} className={icon} />
          <span className="flex flex-wrap items-center gap-1.5">
            Overlap verdict for{" "}
            <code className="font-mono">{e.ref.replace("refs/heads/", "")}</code>
            <VerdictBadge verdict={e.verdict} />
            {e.files?.map((f) => (
              <code key={f} className="bg-muted rounded-sm px-1 font-mono">
                {f}
              </code>
            ))}
          </span>
        </>
      );
    case "agent_pr":
      return (
        <>
          <HugeiconsIcon icon={GitPullRequestIcon} strokeWidth={2} className={icon} />
          <span className="min-w-0">
            Agent PR{" "}
            <Link
              to="/$owner/$repo/pull/$number"
              params={{ owner, repo, number: e.number }}
              className="text-primary hover:underline"
            >
              #{e.number}
            </Link>{" "}
            {e.action}: <TrustInline view={e.title} />
          </span>
        </>
      );
  }
}
