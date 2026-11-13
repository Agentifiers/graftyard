import {
  useMutation,
  useQueryClient,
  type QueryClient,
  type QueryKey,
} from "@tanstack/react-query";
import { toast } from "sonner";
import { api, ApiError, type Schemas } from "@/api/client";
import { pullQuery, threadsQuery, viewedQuery } from "./queries";
import { ui, type DraftComment, type MergeMethod, type ThreadState } from "./ui-api";

// Every write on the pull request pages. None retries (the query client's default), each
// rolls its optimistic change back and toasts on failure, and each refreshes what it
// changed. A 422's field errors arrive on WriteError so a form can show them in place.

type Pull = { owner: string; repo: string; number: number };
type Result<T> = { data?: T; error?: unknown; response: Response };

export class WriteError extends ApiError {
  // GHES validation errors: field-level ones by field, plain strings in `details`.
  readonly fields: Record<string, string>;
  readonly details: string[];
  constructor(status: number, body: unknown) {
    super(status, body);
    const errors =
      body && typeof body === "object" && "errors" in body && Array.isArray(body.errors)
        ? (body.errors as unknown[])
        : [];
    this.fields = {};
    this.details = [];
    for (const e of errors) {
      if (typeof e === "string") this.details.push(e);
      else if (e && typeof e === "object" && "field" in e) {
        const field = String(e.field);
        const text =
          "message" in e && e.message
            ? String(e.message)
            : `${field} is ${"code" in e ? String(e.code).replace(/_/g, " ") : "invalid"}`;
        this.fields[field] = text;
      }
    }
    const first = this.details[0] ?? Object.values(this.fields)[0];
    if (first) this.message = first;
  }
}

export async function write<T>(call: Promise<Result<T>>): Promise<T> {
  const { data, error, response } = await call;
  if (!response.ok || error !== undefined) throw new WriteError(response.status, error);
  return data as T;
}

export const describe = (e: unknown) =>
  e instanceof ApiError
    ? `${e.message} (HTTP ${e.status})`
    : e instanceof Error
      ? e.message
      : String(e);

// Applies `update` to a cached read now, and returns how to undo it.
async function optimistic<T>(qc: QueryClient, key: QueryKey, update: (old: T) => T) {
  await qc.cancelQueries({ queryKey: key, exact: true });
  const before = qc.getQueryData<T>(key);
  if (before !== undefined) qc.setQueryData<T>(key, update(before));
  return () => qc.setQueryData<T>(key, before);
}

// Everything under the pull, the lists, and the issue reads that mirror it.
export function refreshPull(qc: QueryClient, p: Pull) {
  return Promise.all([
    qc.invalidateQueries({ queryKey: ["repos", p.owner, p.repo, "pulls"] }),
    qc.invalidateQueries({ queryKey: ["repos", p.owner, p.repo, "issues"] }),
    qc.invalidateQueries({ queryKey: ["repos", p.owner, p.repo, "commits"] }),
    qc.invalidateQueries({ queryKey: ["repos", p.owner, p.repo, "branches"] }),
    qc.invalidateQueries({ queryKey: ["repos", p.owner, p.repo, "compare"] }),
  ]);
}

const path = (p: Pull) => ({ owner: p.owner, repo: p.repo, pull_number: p.number });

function usePullWrite<V, R>(
  p: Pull,
  what: string,
  fn: (v: V) => Promise<R>,
  opts: {
    apply?: (qc: QueryClient, v: V) => Promise<() => void>;
    success?: (r: R, v: V) => string | undefined;
    // A form shows field errors itself; skip the toast for a 422.
    quiet422?: boolean;
  } = {},
) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: fn,
    onMutate: async (v: V) => ({ undo: await opts.apply?.(qc, v) }),
    onError: (e, _v, ctx) => {
      ctx?.undo?.();
      if (!(opts.quiet422 && e instanceof ApiError && e.status === 422))
        toast.error(`${what} failed`, { description: describe(e) });
    },
    onSuccess: (r, v) => {
      const msg = opts.success?.(r, v);
      if (msg) toast.success(msg);
    },
    onSettled: () => refreshPull(qc, p),
  });
}

type PullRecord = Schemas["pull-request"];

export function useUpdatePull(p: Pull, opts: { quiet422?: boolean } = {}) {
  return usePullWrite(
    p,
    "Updating the pull request",
    (body: { title?: string; body?: string; base?: string; state?: "open" | "closed" }) =>
      write(
        api.PATCH("/repos/{owner}/{repo}/pulls/{pull_number}", { params: { path: path(p) }, body }),
      ),
    {
      ...opts,
      apply: (qc, body) =>
        optimistic<PullRecord>(qc, pullQuery(p).queryKey, (pr) => ({
          ...pr,
          ...(body.title !== undefined ? { title: body.title } : {}),
          ...(body.body !== undefined ? { body: body.body } : {}),
          ...(body.state
            ? {
                state: body.state,
                closed_at: body.state === "closed" ? new Date().toISOString() : null,
              }
            : {}),
          ...(body.base ? { base: { ...pr.base, ref: body.base } } : {}),
        })),
      success: (_r, body) =>
        body.state === "closed"
          ? "Pull request closed"
          : body.state === "open"
            ? "Pull request reopened"
            : body.base
              ? `Base changed to ${body.base}`
              : undefined,
    },
  );
}

export function useSetDraft(p: Pull) {
  return usePullWrite(
    p,
    "Changing the draft state",
    (draft: boolean) =>
      write(
        ui.PUT("/repos/{owner}/{repo}/pulls/{pull_number}/draft", {
          params: { path: path(p) },
          body: { draft },
        }),
      ),
    {
      apply: (qc, draft) =>
        optimistic<PullRecord>(qc, pullQuery(p).queryKey, (pr) => ({ ...pr, draft })),
      success: (_r, draft) => (draft ? "Converted to draft" : "Marked ready for review"),
    },
  );
}

export function useMerge(p: Pull) {
  return usePullWrite(
    p,
    "Merging",
    (v: { method: MergeMethod; title: string; message: string; sha: string }) =>
      write(
        api.PUT("/repos/{owner}/{repo}/pulls/{pull_number}/merge", {
          params: { path: path(p) },
          body: {
            merge_method: v.method,
            commit_title: v.title,
            commit_message: v.message,
            sha: v.sha,
          },
        }),
      ),
    { success: (r) => `Merged as ${r.sha.slice(0, 7)}` },
  );
}

export function useUpdateBranch(p: Pull) {
  return usePullWrite(
    p,
    "Updating the branch",
    (expected: string) =>
      write(
        api.PUT("/repos/{owner}/{repo}/pulls/{pull_number}/update-branch", {
          params: { path: path(p) },
          body: { expected_head_sha: expected },
        }),
      ),
    { success: () => "Branch updated. Checks are running again." },
  );
}

export function useAutoMerge(p: Pull) {
  return usePullWrite(
    p,
    "Changing auto-merge",
    (v: { method: MergeMethod; title: string; message: string } | null) =>
      v
        ? write(
            ui.PUT("/repos/{owner}/{repo}/pulls/{pull_number}/auto-merge", {
              params: { path: path(p) },
              body: { merge_method: v.method, commit_title: v.title, commit_message: v.message },
            }),
          )
        : write(
            ui.DELETE("/repos/{owner}/{repo}/pulls/{pull_number}/auto-merge", {
              params: { path: path(p) },
            }),
          ),
    {
      apply: (qc, v) =>
        optimistic<PullRecord>(qc, pullQuery(p).queryKey, (pr) => ({
          ...pr,
          auto_merge: v
            ? {
                enabled_by: pr.user,
                merge_method: v.method,
                commit_title: v.title,
                commit_message: v.message,
              }
            : null,
        })),
      success: (_r, v) => (v ? "Auto-merge enabled" : "Auto-merge disabled"),
    },
  );
}

export function useHeadBranch(p: Pull) {
  return usePullWrite(
    p,
    "Changing the head branch",
    (v: { action: "delete"; ref: string } | { action: "restore"; ref: string; sha: string }) =>
      v.action === "delete"
        ? write(
            api.DELETE("/repos/{owner}/{repo}/git/refs/{ref}", {
              params: { path: { owner: p.owner, repo: p.repo, ref: `heads/${v.ref}` } },
            }),
          )
        : write(
            api.POST("/repos/{owner}/{repo}/git/refs", {
              params: { path: { owner: p.owner, repo: p.repo } },
              body: { ref: `refs/heads/${v.ref}`, sha: v.sha },
            }),
          ),
    { success: (_r, v) => (v.action === "delete" ? `Deleted ${v.ref}` : `Restored ${v.ref}`) },
  );
}

export function useReviewRequest(p: Pull) {
  return usePullWrite(
    p,
    "Changing reviewers",
    (v: { add: boolean; user?: Schemas["simple-user"]; team?: Schemas["team"] }) => {
      const body = {
        reviewers: v.user ? [v.user.login] : [],
        team_reviewers: v.team ? [v.team.slug] : [],
      };
      return v.add
        ? write(
            api.POST("/repos/{owner}/{repo}/pulls/{pull_number}/requested_reviewers", {
              params: { path: path(p) },
              body,
            }),
          )
        : write(
            api.DELETE("/repos/{owner}/{repo}/pulls/{pull_number}/requested_reviewers", {
              params: { path: path(p) },
              body,
            }),
          );
    },
    {
      // The sidebar reads requested reviewers from the pull itself.
      apply: (qc, v) =>
        optimistic<PullRecord>(qc, pullQuery(p).queryKey, (pr) => {
          const users = pr.requested_reviewers ?? [];
          const teams = pr.requested_teams ?? [];
          return {
            ...pr,
            requested_reviewers: v.user
              ? v.add
                ? [...users, v.user]
                : users.filter((u) => u.login !== v.user!.login)
              : users,
            requested_teams: v.team
              ? v.add
                ? [...teams, v.team]
                : teams.filter((t) => t.slug !== v.team!.slug)
              : teams,
          };
        }),
    },
  );
}

export function useSetLabels(p: Pull) {
  return usePullWrite(
    p,
    "Changing labels",
    (names: string[]) =>
      write(
        api.PUT("/repos/{owner}/{repo}/issues/{issue_number}/labels", {
          params: { path: { owner: p.owner, repo: p.repo, issue_number: p.number } },
          body: { labels: names },
        }),
      ),
    {
      apply: (qc, names) =>
        optimistic<PullRecord>(qc, pullQuery(p).queryKey, (pr) => ({
          ...pr,
          labels: pr.labels.filter((l) => names.includes(l.name)),
        })),
    },
  );
}

export function useAssignee(p: Pull) {
  return usePullWrite(
    p,
    "Changing assignees",
    (v: { add: boolean; user: Schemas["simple-user"] }) => {
      const params = { path: { owner: p.owner, repo: p.repo, issue_number: p.number } };
      const body = { assignees: [v.user.login] };
      return v.add
        ? write(api.POST("/repos/{owner}/{repo}/issues/{issue_number}/assignees", { params, body }))
        : write(
            api.DELETE("/repos/{owner}/{repo}/issues/{issue_number}/assignees", { params, body }),
          );
    },
    {
      apply: (qc, v) =>
        optimistic<PullRecord>(qc, pullQuery(p).queryKey, (pr) => ({
          ...pr,
          assignees: v.add
            ? [...(pr.assignees ?? []), v.user]
            : (pr.assignees ?? []).filter((u) => u.login !== v.user.login),
        })),
    },
  );
}

// ---- Reviews ----

export type ReviewEvent = "COMMENT" | "APPROVE" | "REQUEST_CHANGES";

export function useStartReview(p: Pull) {
  return usePullWrite(p, "Starting a review", (first: DraftComment) =>
    write(
      api.POST("/repos/{owner}/{repo}/pulls/{pull_number}/reviews", {
        params: { path: path(p) },
        body: { comments: [first] },
      }),
    ),
  );
}

export function useAddDraftComment(p: Pull) {
  return usePullWrite(p, "Adding to the review", (v: { reviewId: number; comment: DraftComment }) =>
    write(
      ui.POST("/repos/{owner}/{repo}/pulls/{pull_number}/reviews/{review_id}/comments", {
        params: { path: { ...path(p), review_id: v.reviewId } },
        body: v.comment,
      }),
    ),
  );
}

export function useSubmitReview(p: Pull) {
  return usePullWrite(
    p,
    "Submitting the review",
    (v: { reviewId?: number; event: ReviewEvent; body: string }) =>
      v.reviewId
        ? write(
            api.POST("/repos/{owner}/{repo}/pulls/{pull_number}/reviews/{review_id}/events", {
              params: { path: { ...path(p), review_id: v.reviewId } },
              body: { event: v.event, body: v.body },
            }),
          )
        : write(
            api.POST("/repos/{owner}/{repo}/pulls/{pull_number}/reviews", {
              params: { path: path(p) },
              body: { event: v.event, body: v.body },
            }),
          ),
    {
      quiet422: true,
      success: (_r, v) =>
        v.event === "APPROVE"
          ? "Approved"
          : v.event === "REQUEST_CHANGES"
            ? "Changes requested"
            : "Review submitted",
    },
  );
}

export function useDiscardReview(p: Pull) {
  return usePullWrite(
    p,
    "Discarding the review",
    (reviewId: number) =>
      write(
        api.DELETE("/repos/{owner}/{repo}/pulls/{pull_number}/reviews/{review_id}", {
          params: { path: { ...path(p), review_id: reviewId } },
        }),
      ),
    { success: () => "Pending review discarded" },
  );
}

export function useDismissReview(p: Pull) {
  return usePullWrite(
    p,
    "Dismissing the review",
    (v: { reviewId: number; message: string }) =>
      write(
        api.PUT("/repos/{owner}/{repo}/pulls/{pull_number}/reviews/{review_id}/dismissals", {
          params: { path: { ...path(p), review_id: v.reviewId } },
          body: { message: v.message, event: "DISMISS" },
        }),
      ),
    { quiet422: true, success: () => "Review dismissed" },
  );
}

// ---- Review comments ----

export function useLineComment(p: Pull) {
  return usePullWrite(p, "Commenting", (v: DraftComment & { commit_id: string }) =>
    write(
      api.POST("/repos/{owner}/{repo}/pulls/{pull_number}/comments", {
        params: { path: path(p) },
        body: v,
      }),
    ),
  );
}

export function useReply(p: Pull) {
  return usePullWrite(p, "Replying", (v: { to: number; body: string }) =>
    write(
      api.POST("/repos/{owner}/{repo}/pulls/{pull_number}/comments/{comment_id}/replies", {
        params: { path: { ...path(p), comment_id: v.to } },
        body: { body: v.body },
      }),
    ),
  );
}

export function useEditComment(p: Pull) {
  return usePullWrite(
    p,
    "Editing the comment",
    (v: { id: number; body: string | null }) =>
      v.body === null
        ? write(
            api.DELETE("/repos/{owner}/{repo}/pulls/comments/{comment_id}", {
              params: { path: { owner: p.owner, repo: p.repo, comment_id: v.id } },
            }),
          )
        : write(
            api.PATCH("/repos/{owner}/{repo}/pulls/comments/{comment_id}", {
              params: { path: { owner: p.owner, repo: p.repo, comment_id: v.id } },
              body: { body: v.body },
            }),
          ),
    { success: (_r, v) => (v.body === null ? "Comment deleted" : undefined) },
  );
}

export function useResolveThread(p: Pull) {
  return usePullWrite(
    p,
    "Changing the conversation",
    (v: { rootId: number; resolved: boolean }) => {
      const params = { path: { ...path(p), comment_id: v.rootId } };
      return v.resolved
        ? write(
            ui.PUT("/repos/{owner}/{repo}/pulls/{pull_number}/threads/{comment_id}", {
              params,
              body: {},
            }),
          )
        : write(
            ui.DELETE("/repos/{owner}/{repo}/pulls/{pull_number}/threads/{comment_id}", { params }),
          );
    },
    {
      apply: (qc, v) =>
        optimistic<ThreadState[]>(qc, threadsQuery(p).queryKey, (list) => [
          ...list.filter((t) => t.root_id !== v.rootId),
          { root_id: v.rootId, resolved: v.resolved, resolved_by: null },
        ]),
    },
  );
}

export function useViewed(p: Pull) {
  const qc = useQueryClient();
  const key = viewedQuery(p).queryKey;
  return useMutation({
    mutationFn: (v: { path: string; viewed: boolean }) =>
      write(
        ui.PUT("/repos/{owner}/{repo}/pulls/{pull_number}/viewed", {
          params: { path: path(p) },
          body: v,
        }),
      ),
    onMutate: async (v) => ({
      undo: await optimistic<{ paths: string[] }>(qc, key, (old) => ({
        paths: v.viewed ? [...old.paths, v.path] : old.paths.filter((x) => x !== v.path),
      })),
    }),
    onError: (e, _v, ctx) => {
      ctx?.undo();
      toast.error("Marking the file failed", { description: describe(e) });
    },
    onSettled: () => qc.invalidateQueries({ queryKey: key, exact: true }),
  });
}

// ---- Checks ----

export function useRerun(p: Pull) {
  return usePullWrite(
    p,
    "Re-running",
    (v: { runId: number } | { suiteId: number }) =>
      "runId" in v
        ? write(
            api.POST("/repos/{owner}/{repo}/check-runs/{check_run_id}/rerequest", {
              params: { path: { owner: p.owner, repo: p.repo, check_run_id: v.runId } },
            }),
          )
        : write(
            api.POST("/repos/{owner}/{repo}/check-suites/{check_suite_id}/rerequest", {
              params: { path: { owner: p.owner, repo: p.repo, check_suite_id: v.suiteId } },
            }),
          ),
    { success: () => "Re-run requested" },
  );
}
