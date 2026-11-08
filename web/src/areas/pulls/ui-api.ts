import createClient from "openapi-fetch";
import { waitForMocks, type Schemas } from "@/api/client";
import type { CheckState, Requirement, ReviewDecision } from "./merge-rules";

// The private /_t/ui API the pull request pages read where GHES REST has no route:
// GitHub serves these through GraphQL or its own UI endpoints (thread resolution,
// draft state, auto-merge, viewed files, the list's review and check rollup, and the
// PR-only timeline events). Hand-typed in openapi-typescript's shape, so the client
// and the mock agree on one contract.

type User = Schemas["simple-user"];
type Json<T> = { content: { "application/json": T } };
type Res<T> = { headers: { [name: string]: unknown } } & Json<T>;
type Invalid = Res<Schemas["validation-error"]>;
type Params<P> = { query?: never; header?: never; path: P; cookie?: never };

type Get<P, R> = { parameters: Params<P>; requestBody?: never; responses: { 200: Res<R> } };
type Write<P, B, R> = {
  parameters: Params<P>;
  requestBody: Json<B>;
  responses: { 200: Res<R>; 422: Invalid };
};
type Remove<P, R> = { parameters: Params<P>; requestBody?: never; responses: { 200: Res<R> } };

type RepoPath = { owner: string; repo: string };
type PullPath = RepoPath & { pull_number: number };

export type PullSummary = {
  number: number;
  // Null when nobody has reviewed and the base branch requires no approval.
  review_decision: ReviewDecision | null;
  approvals: number;
  review_comments: number;
  checks: { state: CheckState; passed: number; failed: number; pending: number; total: number };
};

type Base = { id: number; actor: User; created_at: string };

// Events GHES puts on a pull request's timeline that the issue timeline does not own.
export type PullEvent = Base &
  (
    | { event: "merged"; commit_id: string; merge_method: MergeMethod; base_ref: string }
    | { event: "closed" | "reopened" | "ready_for_review" | "convert_to_draft" }
    | { event: "renamed"; rename: { from: string; to: string } }
    | { event: "base_ref_changed"; base: { from: string; to: string } }
    | { event: "head_ref_deleted" | "head_ref_restored"; ref: string }
    | { event: "branch_updated"; commit_id: string; base_ref: string }
    | {
        event: "review_requested" | "review_request_removed";
        requested_reviewer: User | null;
        requested_team: { slug: string; name: string } | null;
      }
    | { event: "review_dismissed"; review_id: number; dismissal_message: string; reviewer: User }
    | { event: "auto_merge_enabled"; merge_method: MergeMethod }
    | { event: "auto_merge_disabled"; reason: string | null }
  );

export type MergeMethod = "merge" | "squash" | "rebase";

// The base branch's protection as it applies to one pull request, evaluated by the
// rules in merge-rules.ts.
export type MergeRequirements = {
  mergeable_state: string;
  requirements: Requirement[];
  required_checks: string[];
  required_approvals: number;
  strict: boolean;
  behind_by: number;
  head_exists: boolean;
};

export type ThreadState = { root_id: number; resolved: boolean; resolved_by: User | null };

export type DraftComment = {
  path: string;
  body: string;
  line: number;
  side: "LEFT" | "RIGHT";
  start_line?: number;
  start_side?: "LEFT" | "RIGHT";
};

type PR = Schemas["pull-request"];

export type UiPaths = {
  "/repos/{owner}/{repo}/pulls/summary": { get: Get<RepoPath, PullSummary[]> };
  "/repos/{owner}/{repo}/pulls/{pull_number}/events": { get: Get<PullPath, PullEvent[]> };
  "/repos/{owner}/{repo}/pulls/{pull_number}/threads": { get: Get<PullPath, ThreadState[]> };
  "/repos/{owner}/{repo}/pulls/{pull_number}/merge-requirements": {
    get: Get<PullPath, MergeRequirements>;
  };
  "/repos/{owner}/{repo}/pulls/{pull_number}/threads/{comment_id}": {
    put: Write<PullPath & { comment_id: number }, Record<string, never>, ThreadState>;
    delete: Remove<PullPath & { comment_id: number }, ThreadState>;
  };
  "/repos/{owner}/{repo}/pulls/{pull_number}/viewed": {
    get: Get<PullPath, { paths: string[] }>;
    put: Write<PullPath, { path: string; viewed: boolean }, { paths: string[] }>;
  };
  "/repos/{owner}/{repo}/pulls/{pull_number}/draft": {
    put: Write<PullPath, { draft: boolean }, PR>;
  };
  "/repos/{owner}/{repo}/pulls/{pull_number}/auto-merge": {
    put: Write<
      PullPath,
      { merge_method: MergeMethod; commit_title: string; commit_message: string },
      PR
    >;
    delete: Remove<PullPath, PR>;
  };
  // Adds a line comment to the viewer's pending review.
  "/repos/{owner}/{repo}/pulls/{pull_number}/reviews/{review_id}/comments": {
    post: Write<
      PullPath & { review_id: number },
      DraftComment,
      Schemas["pull-request-review-comment"]
    >;
  };
};

export const UI_BASE = "/_t/ui";

export const ui = createClient<UiPaths>({ baseUrl: UI_BASE });
ui.use(waitForMocks);
