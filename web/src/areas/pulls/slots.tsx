import { Link } from "@tanstack/react-router";
import { useQuery, useSuspenseQuery } from "@tanstack/react-query";
import { HugeiconsIcon } from "@hugeicons/react";
import { CheckmarkCircle02Icon, CircleIcon, UserGroupIcon } from "@hugeicons/core-free-icons";
import { issueQuery } from "@/areas/issues/queries";
import type { SlotEntry, SlotProps } from "@/areas/slots";
import { LabelBadge, UserAvatar } from "@/components/github";
import { PickerSection } from "./bits";
import { linkedIssues } from "./merge-rules";
import { useAssignee, useReviewRequest, useSetLabels } from "./mutations";
import {
  assigneesQuery,
  labelsQuery,
  pullQuery,
  requestedReviewersQuery,
  reviewsQuery,
  teamsQuery,
} from "./queries";
import { verdicts } from "./merge-rules";

type Props = SlotProps["pull-sidebar"];

const VERDICT: Record<string, { label: string; tone: string }> = {
  APPROVED: { label: "Approved", tone: "text-success-foreground dark:text-success" },
  CHANGES_REQUESTED: { label: "Requested changes", tone: "text-destructive dark:text-red-300" },
  COMMENTED: { label: "Commented", tone: "text-muted-foreground" },
  DISMISSED: { label: "Dismissed", tone: "text-muted-foreground" },
};

// Reviewers, assignees, labels, and the issues this pull request closes.
function PullSidebar(props: Props) {
  const pull = { owner: props.owner, repo: props.repo, number: props.number };
  const { data: pr } = useSuspenseQuery(pullQuery(pull));
  const { data: requested } = useSuspenseQuery(requestedReviewersQuery(pull));
  const { data: reviews } = useSuspenseQuery(reviewsQuery(pull));
  const { data: people } = useSuspenseQuery(assigneesQuery(pull));
  const { data: teams } = useSuspenseQuery(teamsQuery(pull));
  const { data: labels } = useSuspenseQuery(labelsQuery(pull));
  const request = useReviewRequest(pull);
  const assign = useAssignee(pull);
  const setLabels = useSetLabels(pull);
  const v = verdicts(reviews);
  const open = pr.state === "open";

  const reviewerLogins = new Set([...requested.users.map((u) => u.login), ...v.keys()]);
  const candidates = people.filter((u) => u.login !== pr.user.login);
  const chosen = new Set(pr.labels.map((l) => l.name));

  return (
    <div className="bg-card flex flex-col gap-3 rounded-sm border p-3" data-testid="pull-sidebar">
      <PickerSection
        title="Reviewers"
        label="Request reviewers"
        disabled={!open}
        items={[
          ...candidates.map((u) => ({ kind: "user" as const, user: u })),
          ...teams.map((t) => ({ kind: "team" as const, team: t })),
        ]}
        keyOf={(x) => (x.kind === "user" ? x.user.login : `graftyard/${x.team.slug}`)}
        render={(x) =>
          x.kind === "user" ? (
            <>
              <UserAvatar user={x.user} /> {x.user.login}
            </>
          ) : (
            <>
              <HugeiconsIcon icon={UserGroupIcon} strokeWidth={2} className="size-4" /> graftyard/
              {x.team.slug}
            </>
          )
        }
        selected={(x) =>
          x.kind === "user"
            ? requested.users.some((u) => u.login === x.user.login)
            : requested.teams.some((t) => t.slug === x.team.slug)
        }
        onToggle={(x, on) =>
          request.mutate(x.kind === "user" ? { add: on, user: x.user } : { add: on, team: x.team })
        }
      >
        {reviewerLogins.size === 0 && requested.teams.length === 0 ? (
          <p className="text-muted-foreground text-xs">No reviews requested.</p>
        ) : (
          <ul className="flex flex-col gap-1 text-sm" data-testid="reviewers">
            {[...reviewerLogins].map((login) => {
              const user =
                requested.users.find((u) => u.login === login) ??
                reviews.find((r) => r.user?.login === login)?.user ??
                null;
              const verdict = v.get(login);
              const look = verdict ? VERDICT[verdict] : undefined;
              const pending = requested.users.some((u) => u.login === login);
              return (
                <li key={login} className="flex items-center gap-2" data-reviewer={login}>
                  <UserAvatar user={user} />
                  <span className="min-w-0 flex-1 truncate">{login}</span>
                  {pending ? (
                    <span
                      className="text-warning-foreground dark:text-warning flex items-center gap-1 text-xs"
                      title="Awaiting review"
                    >
                      <HugeiconsIcon icon={CircleIcon} strokeWidth={3} className="size-2.5" />
                      Pending
                    </span>
                  ) : look ? (
                    <span className={`text-xs ${look.tone}`}>{look.label}</span>
                  ) : null}
                </li>
              );
            })}
            {requested.teams.map((t) => (
              <li key={t.slug} className="flex items-center gap-2" data-reviewer={`team:${t.slug}`}>
                <HugeiconsIcon icon={UserGroupIcon} strokeWidth={2} className="size-4" />
                <span className="min-w-0 flex-1 truncate">graftyard/{t.slug}</span>
                <span className="text-warning-foreground dark:text-warning text-xs">Pending</span>
              </li>
            ))}
          </ul>
        )}
      </PickerSection>

      <PickerSection
        title="Assignees"
        label="Assign people"
        items={people}
        keyOf={(u) => u.login}
        render={(u) => (
          <>
            <UserAvatar user={u} /> {u.login}
          </>
        )}
        selected={(u) => (pr.assignees ?? []).some((a) => a.login === u.login)}
        onToggle={(u, on) => assign.mutate({ add: on, user: u })}
      >
        {(pr.assignees ?? []).length === 0 ? (
          <p className="text-muted-foreground text-xs">No one assigned.</p>
        ) : (
          <ul className="flex flex-col gap-1 text-sm" data-testid="assignees">
            {(pr.assignees ?? []).map((u) => (
              <li key={u.login} className="flex items-center gap-2">
                <UserAvatar user={u} /> {u.login}
              </li>
            ))}
          </ul>
        )}
      </PickerSection>

      <PickerSection
        title="Labels"
        label="Apply labels"
        items={labels}
        keyOf={(l) => l.name}
        render={(l) => <LabelBadge label={l} />}
        selected={(l) => chosen.has(l.name)}
        onToggle={(l, on) =>
          setLabels.mutate(on ? [...chosen, l.name] : [...chosen].filter((n) => n !== l.name))
        }
      >
        {pr.labels.length === 0 ? (
          <p className="text-muted-foreground text-xs">None yet.</p>
        ) : (
          <div className="flex flex-wrap gap-1" data-testid="labels">
            {pr.labels.map((l) => (
              <LabelBadge key={l.id} label={l} />
            ))}
          </div>
        )}
      </PickerSection>

      <section className="flex flex-col gap-1.5">
        <h3 className="text-muted-foreground text-xs font-semibold">Development</h3>
        <LinkedIssues {...props} body={pr.body} merged={pr.merged} />
      </section>
    </div>
  );
}

function LinkedIssues({
  owner,
  repo,
  body,
  merged,
}: Props & { body: string | null; merged: boolean }) {
  const numbers = linkedIssues(body);
  if (numbers.length === 0)
    return (
      <p className="text-muted-foreground text-xs">
        Write "Closes #N" in the description to link an issue.
      </p>
    );
  return (
    <ul className="flex flex-col gap-1 text-sm" data-testid="linked-issues">
      {numbers.map((n) => (
        <LinkedIssue key={n} owner={owner} repo={repo} number={n} merged={merged} />
      ))}
    </ul>
  );
}

function LinkedIssue({
  owner,
  repo,
  number,
  merged,
}: {
  owner: string;
  repo: string;
  number: number;
  merged: boolean;
}) {
  // A missing issue (a typo in the body) is not an error for the whole sidebar.
  const { data: issue, isError } = useQuery({
    ...issueQuery({ owner, repo, number }),
    retry: false,
  });
  const closed = issue?.state === "closed";
  return (
    <li className="flex items-center gap-2" data-linked={number}>
      <HugeiconsIcon
        icon={closed ? CheckmarkCircle02Icon : CircleIcon}
        strokeWidth={2}
        className={`size-4 shrink-0 ${closed ? "text-info" : "text-success"}`}
      />
      <Link
        to="/$owner/$repo/issues/$number"
        params={{ owner, repo, number }}
        className="min-w-0 flex-1 truncate hover:underline"
      >
        #{number} {issue?.title ?? (isError ? "(not found)" : "")}
      </Link>
      <span className="text-muted-foreground text-xs">
        {closed ? "Closed" : merged ? "" : "Closes on merge"}
      </span>
    </li>
  );
}

// A type-only import with `satisfies`: src/areas/slots.tsx loads this module while it is
// still initializing, so calling its runtime slot() here would read it before it exists.
export const slots: SlotEntry[] = [
  {
    slot: "pull-sidebar",
    region: "pull request details",
    order: 0,
    Component: PullSidebar,
  } satisfies SlotEntry,
];
