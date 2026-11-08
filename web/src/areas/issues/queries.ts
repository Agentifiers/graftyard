import { queryOptions } from "@tanstack/react-query";
import { api, unwrap } from "@/api/client";

// One factory per page region. Keys start with the REST path segments.

type Repo = { owner: string; repo: string };
type Thread = Repo & { number: number };

// The GHES issues list includes pull requests; the UI filters them out.
export const issuesQuery = ({ owner, repo }: Repo) =>
  queryOptions({
    queryKey: ["repos", owner, repo, "issues"],
    queryFn: ({ signal }) =>
      unwrap(
        api.GET("/repos/{owner}/{repo}/issues", {
          params: { path: { owner, repo }, query: { state: "all", per_page: 100 } },
          signal,
        }),
      ),
  });

export const issueQuery = ({ owner, repo, number }: Thread) =>
  queryOptions({
    queryKey: ["repos", owner, repo, "issues", number],
    queryFn: ({ signal }) =>
      unwrap(
        api.GET("/repos/{owner}/{repo}/issues/{issue_number}", {
          params: { path: { owner, repo, issue_number: number } },
          signal,
        }),
      ),
  });

export const commentsQuery = ({ owner, repo, number }: Thread) =>
  queryOptions({
    queryKey: ["repos", owner, repo, "issues", number, "comments"],
    queryFn: ({ signal }) =>
      unwrap(
        api.GET("/repos/{owner}/{repo}/issues/{issue_number}/comments", {
          params: { path: { owner, repo, issue_number: number }, query: { per_page: 100 } },
          signal,
        }),
      ),
  });

export const timelineQuery = ({ owner, repo, number }: Thread) =>
  queryOptions({
    queryKey: ["repos", owner, repo, "issues", number, "timeline"],
    queryFn: ({ signal }) =>
      unwrap(
        api.GET("/repos/{owner}/{repo}/issues/{issue_number}/timeline", {
          params: { path: { owner, repo, issue_number: number }, query: { per_page: 100 } },
          signal,
        }),
      ),
  });

export const issueReactionsQuery = ({ owner, repo, number }: Thread) =>
  queryOptions({
    queryKey: ["repos", owner, repo, "issues", number, "reactions"],
    queryFn: ({ signal }) =>
      unwrap(
        api.GET("/repos/{owner}/{repo}/issues/{issue_number}/reactions", {
          params: { path: { owner, repo, issue_number: number }, query: { per_page: 100 } },
          signal,
        }),
      ),
  });

export const commentReactionsQuery = ({ owner, repo, id }: Repo & { id: number }) =>
  queryOptions({
    queryKey: ["repos", owner, repo, "issues", "comments", id, "reactions"],
    queryFn: ({ signal }) =>
      unwrap(
        api.GET("/repos/{owner}/{repo}/issues/comments/{comment_id}/reactions", {
          params: { path: { owner, repo, comment_id: id }, query: { per_page: 100 } },
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
  });

export const milestonesQuery = ({ owner, repo }: Repo) =>
  queryOptions({
    queryKey: ["repos", owner, repo, "milestones"],
    queryFn: ({ signal }) =>
      unwrap(
        api.GET("/repos/{owner}/{repo}/milestones", {
          params: { path: { owner, repo }, query: { state: "all", per_page: 100 } },
          signal,
        }),
      ),
  });

export const milestoneQuery = ({ owner, repo, number }: Thread) =>
  queryOptions({
    queryKey: ["repos", owner, repo, "milestones", number],
    queryFn: ({ signal }) =>
      unwrap(
        api.GET("/repos/{owner}/{repo}/milestones/{milestone_number}", {
          params: { path: { owner, repo, milestone_number: number } },
          signal,
        }),
      ),
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
  });

// The issue page uses the issue's id as its notification thread.
export const subscriptionQuery = (threadId: number) =>
  queryOptions({
    queryKey: ["notifications", "threads", threadId, "subscription"],
    queryFn: ({ signal }) =>
      unwrap(
        api.GET("/notifications/threads/{thread_id}/subscription", {
          params: { path: { thread_id: threadId } },
          signal,
        }),
      ),
  });

export const markdownQuery = (text: string, context: string) =>
  queryOptions({
    queryKey: ["markdown", context, text],
    staleTime: Infinity,
    queryFn: ({ signal }) =>
      unwrap(
        api.POST("/markdown", {
          body: { text, mode: "gfm", context },
          parseAs: "text",
          signal,
        }),
      ),
  });
