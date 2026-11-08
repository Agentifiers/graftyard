import { useSuspenseQuery } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { HugeiconsIcon, type IconSvgElement } from "@hugeicons/react";
import {
  CheckmarkCircle02Icon,
  CircleIcon,
  Edit02Icon,
  GitCommitIcon,
  Link04Icon,
  LockIcon,
  MilestoneIcon,
  SquareUnlock02Icon,
  Tag01Icon,
  UnavailableIcon,
  UserIcon,
} from "@hugeicons/core-free-icons";
import type { Schemas } from "@/api/client";
import type { TimelineEvent } from "@/components/conversation";
import { Ago, LabelBadge } from "@/components/github";
import { timelineQuery } from "./queries";

type Entry = Schemas["timeline-issue-events"];
type Thread = { owner: string; repo: string; number: number };

// What one timeline row says, by event name. Comments are left out: the Conversation
// rail draws those from the comments collection.
type Row = { icon: IconSvgElement; tone?: string; text: React.ReactNode };

function describe(e: Entry, thread: Thread): Row | undefined {
  if (!("event" in e)) return undefined;
  switch (e.event) {
    case "labeled":
    case "unlabeled":
      if (!("label" in e) || !e.label || !("color" in e.label)) return undefined;
      return {
        icon: Tag01Icon,
        text: (
          <>
            {e.event === "labeled" ? "added" : "removed"} the{" "}
            <LabelBadge label={{ name: e.label.name, color: e.label.color }} /> label
          </>
        ),
      };
    case "assigned":
    case "unassigned": {
      if (!("assignee" in e) || !e.assignee) return undefined;
      const self = "actor" in e && e.actor?.login === e.assignee.login;
      return {
        icon: UserIcon,
        text: self ? (
          <>{e.event === "assigned" ? "self-assigned this" : "removed their assignment"}</>
        ) : (
          <>
            {e.event === "assigned" ? "assigned" : "unassigned"}{" "}
            <span className="text-foreground font-medium">{e.assignee.login}</span>
          </>
        ),
      };
    }
    case "milestoned":
    case "demilestoned":
      if (!("milestone" in e) || !e.milestone) return undefined;
      return {
        icon: MilestoneIcon,
        text: (
          <>
            {e.event === "milestoned" ? "added this to the" : "removed this from the"}{" "}
            <span className="text-foreground font-medium">{e.milestone.title}</span> milestone
          </>
        ),
      };
    case "renamed":
      if (!("rename" in e)) return undefined;
      return {
        icon: Edit02Icon,
        text: (
          <>
            changed the title <del className="text-muted-foreground">{e.rename.from}</del>{" "}
            <span className="text-foreground">{e.rename.to}</span>
          </>
        ),
      };
    case "closed": {
      const notPlanned = "state_reason" in e && e.state_reason === "not_planned";
      return {
        icon: notPlanned ? UnavailableIcon : CheckmarkCircle02Icon,
        tone: notPlanned ? "text-muted-foreground" : "text-info",
        text: <>closed this as {notPlanned ? "not planned" : "completed"}</>,
      };
    }
    case "reopened":
      return { icon: CircleIcon, tone: "text-success", text: <>reopened this</> };
    case "locked":
      return {
        icon: LockIcon,
        text: (
          <>
            locked {"lock_reason" in e && e.lock_reason ? <>as {e.lock_reason} and </> : null}
            limited conversation to collaborators
          </>
        ),
      };
    case "unlocked":
      return { icon: SquareUnlock02Icon, text: <>unlocked this conversation</> };
    case "referenced":
      return {
        icon: GitCommitIcon,
        text: (
          <>
            added a commit that referenced this issue{" "}
            {"commit_id" in e && e.commit_id && (
              <code className="bg-muted rounded-sm px-1 font-mono text-xs">
                {e.commit_id.slice(0, 7)}
              </code>
            )}
          </>
        ),
      };
    case "cross-referenced": {
      if (!("source" in e)) return undefined;
      const src = e.source.issue;
      if (!src) return undefined;
      const isPull = Boolean(src.pull_request);
      return {
        icon: Link04Icon,
        text: (
          <>
            mentioned this in{" "}
            <Link
              to={isPull ? "/$owner/$repo/pull/$number" : "/$owner/$repo/issues/$number"}
              params={{ owner: thread.owner, repo: thread.repo, number: src.number }}
              className="text-foreground font-medium hover:underline"
            >
              {src.title} #{src.number}
            </Link>
          </>
        ),
      };
    }
    default:
      return undefined;
  }
}

function at(e: Entry) {
  return "created_at" in e && e.created_at ? e.created_at : "";
}

function actorOf(e: Entry) {
  return "actor" in e && e.actor ? e.actor : null;
}

// The timeline's non-comment events as rail items for Conversation's `events` prop.
export function useTimelineEvents(thread: Thread): TimelineEvent[] {
  const { data } = useSuspenseQuery(timelineQuery(thread));
  return data.flatMap((e, i) => {
    const row = describe(e, thread);
    if (!row) return [];
    const actor = actorOf(e);
    return [
      {
        key: `event-${"id" in e ? e.id : i}-${i}`,
        at: at(e),
        author: actor,
        node: (
          <div
            className="text-muted-foreground flex flex-wrap items-center gap-1.5 py-1 text-sm"
            data-event={"event" in e ? e.event : undefined}
          >
            <HugeiconsIcon
              icon={row.icon}
              strokeWidth={2}
              className={`size-4 shrink-0 ${row.tone ?? ""}`}
            />
            <span className="text-foreground font-semibold">{actor?.login ?? "ghost"}</span>
            {row.text}
            <Ago at={at(e)} />
          </div>
        ),
      },
    ];
  });
}
