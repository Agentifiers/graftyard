import { useMutation, useQueryClient } from "@tanstack/react-query";
import { api, type Schemas } from "@/api/client";
import type { paths } from "@/api/ghes";
import { send, toastFailure } from "./api";
import { issueQuery, issuesQuery, milestonesQuery, timelineQuery } from "./queries";

// Every change the issue page makes, as one union. Each variant has an optimistic
// `apply` and the one request that persists it, so the header, the sidebar, and the
// close button share one write path, one rollback, and one set of refetches.

type Issue = Schemas["issue"];
type Thread = { owner: string; repo: string; number: number };
type UpdateBody = NonNullable<
  paths["/repos/{owner}/{repo}/issues/{issue_number}"]["patch"]["requestBody"]
>["content"]["application/json"];

export type LockReason = "off-topic" | "too heated" | "resolved" | "spam";

export type IssueChange =
  | { kind: "title"; title: string }
  | { kind: "body"; body: string }
  | { kind: "state"; state: "open" | "closed"; reason: "completed" | "not_planned" | null }
  | { kind: "milestone"; milestone: Schemas["milestone"] | null }
  | { kind: "labels"; labels: Schemas["label"][] }
  | { kind: "assignees"; users: Schemas["simple-user"][] }
  | { kind: "lock"; reason: LockReason | null }
  | { kind: "unlock" };

const WHAT: Record<IssueChange["kind"], string> = {
  title: "Renaming the issue",
  body: "Saving the description",
  state: "Changing the issue's state",
  milestone: "Setting the milestone",
  labels: "Setting labels",
  assignees: "Setting assignees",
  lock: "Locking the conversation",
  unlock: "Unlocking the conversation",
};

export function apply(issue: Issue, c: IssueChange): Issue {
  const now = new Date().toISOString();
  switch (c.kind) {
    case "title":
      return { ...issue, title: c.title, updated_at: now };
    case "body":
      return { ...issue, body: c.body, updated_at: now };
    case "state":
      return {
        ...issue,
        state: c.state,
        state_reason: c.state === "open" ? "reopened" : c.reason,
        closed_at: c.state === "closed" ? now : null,
        updated_at: now,
      };
    case "milestone":
      return { ...issue, milestone: c.milestone, updated_at: now };
    case "labels":
      return { ...issue, labels: c.labels, updated_at: now };
    case "assignees":
      return { ...issue, assignees: c.users, assignee: c.users[0] ?? null, updated_at: now };
    case "lock":
      return { ...issue, locked: true, active_lock_reason: c.reason, updated_at: now };
    case "unlock":
      return { ...issue, locked: false, active_lock_reason: null, updated_at: now };
  }
}

async function request(t: Thread, c: IssueChange) {
  const path = { owner: t.owner, repo: t.repo, issue_number: t.number };
  const patch = (body: UpdateBody) =>
    send(api.PATCH("/repos/{owner}/{repo}/issues/{issue_number}", { params: { path }, body }));
  switch (c.kind) {
    case "title":
      return patch({ title: c.title });
    case "body":
      return patch({ body: c.body });
    case "state":
      return patch({ state: c.state, state_reason: c.state === "open" ? "reopened" : c.reason });
    case "milestone":
      return patch({ milestone: c.milestone?.number ?? null });
    case "labels":
      return send(
        api.PUT("/repos/{owner}/{repo}/issues/{issue_number}/labels", {
          params: { path },
          body: { labels: c.labels.map((l) => l.name) },
        }),
      );
    case "assignees":
      return patch({ assignees: c.users.map((u) => u.login) });
    case "lock":
      return send(
        api.PUT("/repos/{owner}/{repo}/issues/{issue_number}/lock", {
          params: { path },
          body: c.reason ? { lock_reason: c.reason } : {},
        }),
      );
    case "unlock":
      return send(
        api.DELETE("/repos/{owner}/{repo}/issues/{issue_number}/lock", { params: { path } }),
      );
  }
}

// One mutation for every change. It patches the issue read at once, sends the request
// once, and on failure restores the issue and toasts. It always refetches the issue,
// the list, the timeline (the mock writes an event per change), and milestone counts.
export function useIssueChange(t: Thread) {
  const queryClient = useQueryClient();
  const key = issueQuery(t).queryKey;
  return useMutation({
    mutationFn: (c: IssueChange) => request(t, c),
    onMutate: async (c) => {
      await queryClient.cancelQueries({ queryKey: key });
      const before = queryClient.getQueryData(key);
      if (before) queryClient.setQueryData(key, apply(before, c));
      return { before };
    },
    onError: (e, c, ctx) => {
      if (ctx?.before) queryClient.setQueryData(key, ctx.before);
      toastFailure(WHAT[c.kind], e);
    },
    onSettled: () =>
      Promise.all(
        [key, issuesQuery(t).queryKey, timelineQuery(t).queryKey, milestonesQuery(t).queryKey].map(
          (queryKey) => queryClient.invalidateQueries({ queryKey }),
        ),
      ),
  });
}
