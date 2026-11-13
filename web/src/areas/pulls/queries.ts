import { queryOptions } from "@tanstack/react-query";
import { api, unwrap, type Schemas } from "@/api/client";
import { graphql } from "@/api/graphql";
import { ui } from "./ui-api";

// One factory per page region. Keys are the REST path segments, so a /_t/ui read sits
// under the pull it describes and one invalidation of ["repos", o, r, "pulls", n]
// refreshes the whole page.

type Repo = { owner: string; repo: string };
export type Pull = Repo & { number: number };

const pullPath = ({ owner, repo, number }: Pull) => ({ owner, repo, pull_number: number });

export const pullsQuery = ({ owner, repo }: Repo) =>
  queryOptions({
    queryKey: ["repos", owner, repo, "pulls"],
    queryFn: ({ signal }) =>
      unwrap(
        api.GET("/repos/{owner}/{repo}/pulls", {
          params: { path: { owner, repo }, query: { state: "all", per_page: 100 } },
          signal,
        }),
      ),
  });

export const pullSummariesQuery = ({ owner, repo }: Repo) =>
  queryOptions({
    queryKey: ["repos", owner, repo, "pulls", "summary"],
    queryFn: ({ signal }) =>
      unwrap(
        ui.GET("/repos/{owner}/{repo}/pulls/summary", {
          params: { path: { owner, repo } },
          signal,
        }),
      ),
    // A check that is still running changes the row; poll until every row settles.
    refetchInterval: (q) => (q.state.data?.some((s) => s.checks.pending > 0) ? 4000 : false),
  });

export const pullQuery = (p: Pull) =>
  queryOptions({
    queryKey: ["repos", p.owner, p.repo, "pulls", p.number],
    queryFn: ({ signal }) =>
      unwrap(
        api.GET("/repos/{owner}/{repo}/pulls/{pull_number}", {
          params: { path: pullPath(p) },
          signal,
        }),
      ),
    // Auto-merge happens on the server when the last requirement passes.
    refetchInterval: (q) =>
      q.state.data?.auto_merge && q.state.data.state === "open" ? 3000 : false,
  });

export const pullFilesQuery = (p: Pull) =>
  queryOptions({
    queryKey: ["repos", p.owner, p.repo, "pulls", p.number, "files"],
    queryFn: ({ signal }) =>
      unwrap(
        ui.GET("/repos/{owner}/{repo}/pulls/{pull_number}/files", {
          params: { path: pullPath(p) },
          signal,
        }),
      ),
  });

export const pullCommitsQuery = (p: Pull) =>
  queryOptions({
    queryKey: ["repos", p.owner, p.repo, "pulls", p.number, "commits"],
    queryFn: ({ signal }) =>
      unwrap(
        ui.GET("/repos/{owner}/{repo}/pulls/{pull_number}/commits", {
          params: { path: pullPath(p) },
          signal,
        }),
      ),
  });

export const reviewsQuery = (p: Pull) =>
  queryOptions({
    queryKey: ["repos", p.owner, p.repo, "pulls", p.number, "reviews"],
    queryFn: ({ signal }) =>
      unwrap(
        api.GET("/repos/{owner}/{repo}/pulls/{pull_number}/reviews", {
          params: { path: pullPath(p), query: { per_page: 100 } },
          signal,
        }),
      ),
  });

export const reviewCommentsQuery = (p: Pull) =>
  queryOptions({
    queryKey: ["repos", p.owner, p.repo, "pulls", p.number, "comments"],
    queryFn: ({ signal }) =>
      unwrap(
        api.GET("/repos/{owner}/{repo}/pulls/{pull_number}/comments", {
          params: { path: pullPath(p), query: { per_page: 100 } },
          signal,
        }),
      ),
  });

export const pendingCommentsQuery = (p: Pull & { reviewId: number }) =>
  queryOptions({
    queryKey: ["repos", p.owner, p.repo, "pulls", p.number, "reviews", p.reviewId, "comments"],
    queryFn: ({ signal }) =>
      unwrap(
        api.GET("/repos/{owner}/{repo}/pulls/{pull_number}/reviews/{review_id}/comments", {
          params: { path: { ...pullPath(p), review_id: p.reviewId }, query: { per_page: 100 } },
          signal,
        }),
      ),
  });

export const pullEventsQuery = (p: Pull) =>
  queryOptions({
    queryKey: ["repos", p.owner, p.repo, "pulls", p.number, "events"],
    queryFn: ({ signal }) =>
      unwrap(
        ui.GET("/repos/{owner}/{repo}/pulls/{pull_number}/events", {
          params: { path: pullPath(p) },
          signal,
        }),
      ),
  });

export const threadsQuery = (p: Pull) =>
  queryOptions({
    queryKey: ["repos", p.owner, p.repo, "pulls", p.number, "threads"],
    queryFn: ({ signal }) =>
      unwrap(
        ui.GET("/repos/{owner}/{repo}/pulls/{pull_number}/threads", {
          params: { path: pullPath(p) },
          signal,
        }),
      ),
  });

export const viewedQuery = (p: Pull) =>
  queryOptions({
    queryKey: ["repos", p.owner, p.repo, "pulls", p.number, "viewed"],
    queryFn: ({ signal }) =>
      unwrap(
        ui.GET("/repos/{owner}/{repo}/pulls/{pull_number}/viewed", {
          params: { path: pullPath(p) },
          signal,
        }),
      ),
  });

export const mergeRequirementsQuery = (p: Pull) =>
  queryOptions({
    queryKey: ["repos", p.owner, p.repo, "pulls", p.number, "merge-requirements"],
    queryFn: ({ signal }) =>
      unwrap(
        ui.GET("/repos/{owner}/{repo}/pulls/{pull_number}/merge-requirements", {
          params: { path: pullPath(p) },
          signal,
        }),
      ),
    refetchInterval: (q) =>
      q.state.data?.requirements.some((r) => r.state === "pending") ? 3000 : false,
  });

export const checkRunsQuery = ({ owner, repo, ref }: Repo & { ref: string }) =>
  queryOptions({
    queryKey: ["repos", owner, repo, "commits", ref, "check-runs"],
    queryFn: ({ signal }) =>
      unwrap(
        api.GET("/repos/{owner}/{repo}/commits/{ref}/check-runs", {
          params: { path: { owner, repo, ref } },
          signal,
        }),
      ),
    // Poll while any run is queued or in progress, as GHES's checks page does.
    refetchInterval: (q) =>
      q.state.data?.check_runs.some((r) => r.status !== "completed") ? 3000 : false,
  });

export const checkSuitesQuery = ({ owner, repo, ref }: Repo & { ref: string }) =>
  queryOptions({
    queryKey: ["repos", owner, repo, "commits", ref, "check-suites"],
    queryFn: ({ signal }) =>
      unwrap(
        api.GET("/repos/{owner}/{repo}/commits/{ref}/check-suites", {
          params: { path: { owner, repo, ref } },
          signal,
        }),
      ),
  });

export const annotationsQuery = ({ owner, repo, runId }: Repo & { runId: number }) =>
  queryOptions({
    queryKey: ["repos", owner, repo, "check-runs", runId, "annotations"],
    queryFn: ({ signal }) =>
      unwrap(
        api.GET("/repos/{owner}/{repo}/check-runs/{check_run_id}/annotations", {
          params: { path: { owner, repo, check_run_id: runId } },
          signal,
        }),
      ),
  });

// The statuses on a commit, newest first; the Checks tab derives the combined state.
// (GET /commits/{ref}/status is not served by the edge.)
export const commitStatusesQuery = ({ owner, repo, ref }: Repo & { ref: string }) =>
  queryOptions({
    queryKey: ["repos", owner, repo, "commits", ref, "statuses"],
    queryFn: ({ signal }) =>
      unwrap(
        api.GET("/repos/{owner}/{repo}/commits/{ref}/statuses", {
          params: { path: { owner, repo, ref }, query: { per_page: 100 } },
          signal,
        }),
      ),
  });

// GraphQL's mergeable, as a persisted document. GHES computes the test merge in the
// background, so the merge box polls while it is UNKNOWN.
export const pullMergeStateQuery = (p: Pull) =>
  queryOptions({
    queryKey: ["graphql", "PullMergeState", p.owner, p.repo, p.number],
    queryFn: ({ signal }) =>
      graphql("PullMergeState", { owner: p.owner, name: p.repo, number: p.number }, signal).then(
        (d) => d.repository?.pullRequest ?? null,
      ),
    refetchInterval: (q) => (q.state.data?.mergeable === "UNKNOWN" ? 3000 : false),
  });

export const compareQuery = ({ owner, repo, base, head }: Repo & { base: string; head: string }) =>
  queryOptions({
    queryKey: ["repos", owner, repo, "compare", `${base}...${head}`],
    // The operation also offers diff and patch text; this read asks for JSON.
    queryFn: ({ signal }): Promise<Schemas["commit-comparison"]> =>
      unwrap(
        api.GET("/repos/{owner}/{repo}/compare/{basehead}", {
          params: { path: { owner, repo, basehead: `${base}...${head}` } },
          parseAs: "json",
          signal,
        }),
      ).then((r) => {
        if (typeof r === "string") throw new Error("Expected a JSON comparison");
        return r;
      }),
  });

// Reads other areas own. The keys match theirs, so their fuller handlers fill the same cache.
export const branchesQuery = ({ owner, repo }: Repo) =>
  queryOptions({
    queryKey: ["repos", owner, repo, "branches"],
    queryFn: ({ signal }) =>
      unwrap(
        api.GET("/repos/{owner}/{repo}/branches", {
          params: { path: { owner, repo }, query: { per_page: 100 } },
          signal,
        }),
      ),
  });

export const labelsQuery = ({ owner, repo }: Repo) =>
  queryOptions({
    queryKey: ["repos", owner, repo, "labels"],
    queryFn: ({ signal }) =>
      unwrap(
        api.GET("/repos/{owner}/{repo}/labels", {
          params: { path: { owner, repo }, query: { per_page: 100 } },
          signal,
        }),
      ),
    staleTime: 5 * 60_000,
  });

export const assigneesQuery = ({ owner, repo }: Repo) =>
  queryOptions({
    queryKey: ["repos", owner, repo, "assignees"],
    queryFn: ({ signal }) =>
      unwrap(
        api.GET("/repos/{owner}/{repo}/assignees", {
          params: { path: { owner, repo }, query: { per_page: 100 } },
          signal,
        }),
      ),
    staleTime: 5 * 60_000,
  });

export const teamsQuery = ({ owner, repo }: Repo) =>
  queryOptions({
    queryKey: ["repos", owner, repo, "teams"],
    queryFn: ({ signal }) =>
      unwrap(
        api.GET("/repos/{owner}/{repo}/teams", {
          params: { path: { owner, repo }, query: { per_page: 100 } },
          signal,
        }),
      ),
    staleTime: 5 * 60_000,
  });
