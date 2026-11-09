import type { ReactNode } from "react";
import { Link } from "@tanstack/react-router";
import { useSuspenseInfiniteQuery, useSuspenseQuery } from "@tanstack/react-query";
import { HugeiconsIcon, type IconSvgElement } from "@hugeicons/react";
import {
  Comment01Icon,
  Delete02Icon,
  GitBranchIcon,
  GitCommitIcon,
  GitForkIcon,
  GitPullRequestIcon,
  RecordIcon,
  StarIcon,
  Tag01Icon,
} from "@hugeicons/core-free-icons";
import type { Schemas } from "@/api/client";
import { userQuery } from "@/areas/core/queries";
import { Ago, UserAvatar } from "@/components/github";
import { Panel } from "@/components/panel";
import { Button } from "@/components/ui/button";
import { Spinner } from "@/components/ui/spinner";
import { feedQuery } from "./queries";

// The home page's activity feed: GET /users/{viewer}/received_events, one row per event,
// each with an icon, the actor, what happened, and a link to it.

type Event = Schemas["event"];
type P = Event["payload"];

const linkClass = "text-foreground font-semibold hover:underline";

function RepoLink({ name }: { name: string }) {
  const [owner, repo] = name.split("/");
  return (
    <Link to="/$owner/$repo" params={{ owner, repo }} className={linkClass}>
      {name}
    </Link>
  );
}

// Each event type: its icon and the sentence after the actor. A type the table lacks
// falls back to its raw name, so a new GHES event never breaks the feed.
type Describe = (e: Event) => { icon: IconSvgElement; text: ReactNode; detail?: ReactNode };

const pick = <K extends string>(p: P, key: K) =>
  key in p ? (p as Record<K, unknown>)[key] : undefined;

const DESCRIBE: Record<string, Describe> = {
  PushEvent: (e) => {
    const p = e.payload as Schemas["push-event"];
    const branch = p.ref.replace("refs/heads/", "");
    const [owner, repo] = e.repo.name.split("/");
    return {
      icon: GitCommitIcon,
      text: (
        <>
          pushed {p.size ?? p.commits?.length ?? 0} commit{(p.size ?? 1) === 1 ? "" : "s"} to{" "}
          <Link
            to="/$owner/$repo/tree/$"
            params={{ owner, repo, _splat: branch }}
            className={linkClass}
          >
            {branch}
          </Link>{" "}
          in <RepoLink name={e.repo.name} />
        </>
      ),
      detail: (
        <ul className="flex flex-col gap-0.5">
          {(p.commits ?? []).map((c) => (
            <li key={c.sha} className="flex gap-2">
              <Link
                to="/$owner/$repo/commit/$sha"
                params={{ owner, repo, sha: c.sha ?? "" }}
                className="text-primary font-mono hover:underline"
              >
                {c.sha?.slice(0, 7)}
              </Link>
              <span className="truncate">{c.message?.split("\n")[0]}</span>
            </li>
          ))}
        </ul>
      ),
    };
  },
  PullRequestEvent: (e) => {
    const p = e.payload as Schemas["pull-request-event"];
    const [owner, repo] = e.repo.name.split("/");
    const verb = p.action === "closed" && p.pull_request.merged_at ? "merged" : p.action;
    return {
      icon: GitPullRequestIcon,
      text: (
        <>
          {verb} pull request{" "}
          <Link
            to="/$owner/$repo/pull/$number"
            params={{ owner, repo, number: p.number }}
            className={linkClass}
          >
            {e.repo.name}#{p.number}
          </Link>
        </>
      ),
      detail: p.pull_request.title,
    };
  },
  IssuesEvent: (e) => {
    const p = e.payload as Schemas["issues-event"];
    const [owner, repo] = e.repo.name.split("/");
    return {
      icon: RecordIcon,
      text: (
        <>
          {p.action} issue{" "}
          <Link
            to="/$owner/$repo/issues/$number"
            params={{ owner, repo, number: p.issue.number }}
            className={linkClass}
          >
            {e.repo.name}#{p.issue.number}
          </Link>
        </>
      ),
      detail: p.issue.title,
    };
  },
  IssueCommentEvent: (e) => {
    const p = e.payload as Schemas["issue-comment-event"];
    const [owner, repo] = e.repo.name.split("/");
    const n = p.issue.number;
    return {
      icon: Comment01Icon,
      text: (
        <>
          commented on{" "}
          {p.issue.pull_request ? (
            <Link
              to="/$owner/$repo/pull/$number"
              params={{ owner, repo, number: n }}
              className={linkClass}
            >
              {e.repo.name}#{n}
            </Link>
          ) : (
            <Link
              to="/$owner/$repo/issues/$number"
              params={{ owner, repo, number: n }}
              className={linkClass}
            >
              {e.repo.name}#{n}
            </Link>
          )}
        </>
      ),
      detail: <span className="line-clamp-2">{p.comment.body}</span>,
    };
  },
  ReleaseEvent: (e) => {
    const p = e.payload as Schemas["release-event"];
    const [owner, repo] = e.repo.name.split("/");
    return {
      icon: Tag01Icon,
      text: (
        <>
          {p.action} release{" "}
          <Link
            to="/$owner/$repo/releases/tag/$tag"
            params={{ owner, repo, tag: p.release.tag_name }}
            className={linkClass}
          >
            {p.release.name ?? p.release.tag_name}
          </Link>{" "}
          of <RepoLink name={e.repo.name} />
        </>
      ),
    };
  },
  WatchEvent: (e) => ({
    icon: StarIcon,
    text: (
      <>
        starred <RepoLink name={e.repo.name} />
      </>
    ),
  }),
  ForkEvent: (e) => {
    const forkee = pick(e.payload, "forkee") as Schemas["fork-event"]["forkee"];
    return {
      icon: GitForkIcon,
      text: (
        <>
          forked <RepoLink name={e.repo.name} /> to{" "}
          <span className="font-semibold">{forkee.full_name}</span>
        </>
      ),
    };
  },
  CreateEvent: (e) => {
    const p = e.payload as Schemas["create-event"];
    const [owner, repo] = e.repo.name.split("/");
    return {
      icon: GitBranchIcon,
      text: (
        <>
          created {p.ref_type}{" "}
          {p.ref_type === "branch" ? (
            <Link
              to="/$owner/$repo/tree/$"
              params={{ owner, repo, _splat: p.ref }}
              className={linkClass}
            >
              {p.ref}
            </Link>
          ) : (
            <span className="font-semibold">{p.ref}</span>
          )}{" "}
          in <RepoLink name={e.repo.name} />
        </>
      ),
    };
  },
  DeleteEvent: (e) => {
    const p = e.payload as Schemas["delete-event"];
    return {
      icon: Delete02Icon,
      text: (
        <>
          deleted {p.ref_type} <span className="font-mono">{p.ref}</span> in{" "}
          <RepoLink name={e.repo.name} />
        </>
      ),
    };
  },
};

function describe(e: Event): ReturnType<Describe> {
  const d = e.type ? DESCRIBE[e.type] : undefined;
  return d
    ? d(e)
    : {
        icon: GitCommitIcon,
        text: (
          <>
            {e.type ?? "did something"} in <RepoLink name={e.repo.name} />
          </>
        ),
      };
}

export function ActivityFeed() {
  return (
    <Panel title="Recent activity" flush>
      <FeedList />
    </Panel>
  );
}

function FeedList() {
  const { data: viewer } = useSuspenseQuery(userQuery());
  const feed = useSuspenseInfiniteQuery(feedQuery(viewer.login));
  const events = feed.data.pages.flatMap((p) => p.events);
  if (!events.length) {
    return (
      <p className="text-muted-foreground px-3 py-6 text-center text-sm">
        Nothing has happened in repositories you watch yet.
      </p>
    );
  }
  return (
    <>
      <ul className="divide-y" data-testid="activity-feed">
        {events.map((e) => {
          const d = describe(e);
          return (
            <li key={e.id} className="flex gap-2.5 px-3 py-2.5 text-sm" data-event-type={e.type}>
              <HugeiconsIcon
                icon={d.icon}
                strokeWidth={2}
                className="text-muted-foreground mt-0.5 size-4 shrink-0"
              />
              <div className="min-w-0 flex-1">
                <p className="text-muted-foreground leading-snug">
                  <span className="mr-1 inline-flex translate-y-1">
                    <UserAvatar user={{ login: e.actor.login, avatar_url: e.actor.avatar_url }} />
                  </span>
                  <Link to="/$owner" params={{ owner: e.actor.login }} className={linkClass}>
                    {e.actor.display_login ?? e.actor.login}
                  </Link>{" "}
                  {d.text}
                </p>
                {d.detail && <div className="text-muted-foreground mt-1 text-xs">{d.detail}</div>}
                {e.created_at && (
                  <p className="text-muted-foreground mt-1 text-xs">
                    <Ago at={e.created_at} />
                  </p>
                )}
              </div>
            </li>
          );
        })}
      </ul>
      {feed.hasNextPage && (
        <div className="border-t p-2">
          <Button
            variant="ghost"
            size="sm"
            className="w-full rounded-sm"
            disabled={feed.isFetchingNextPage}
            onClick={() => void feed.fetchNextPage()}
            data-testid="feed-more"
          >
            {feed.isFetchingNextPage && <Spinner className="size-3" />}
            Show more
          </Button>
        </div>
      )}
    </>
  );
}
