import createClient from "openapi-fetch";
import { waitForMocks } from "@/api/client";

// The private /_t/ui API: what the git cache computes and GHES REST does not serve
// (design/SHARED.md, "Frontend and native app"). Traffic lives here too, because the
// GHES 3.22 description has no traffic operations. Paths follow openapi-typescript's
// shape so openapi-fetch and openapi-msw type both ends.

export type CommitAuthor = { login: string | null; name: string; avatar_url: string | null };

export type CommitSummary = {
  sha: string;
  message: string;
  date: string;
  author: CommitAuthor;
};

export type ResolvedRef = {
  // The branch, tag, or sha the splat starts with, and the path after it.
  ref: string;
  path: string;
  kind: "branch" | "tag" | "commit";
  sha: string;
  // What the path names at that commit: null when it does not exist.
  type: "dir" | "file" | null;
  default_branch: string;
};

export type TreeCommits = {
  // The newest commit that touched the directory, and how many did.
  latest: CommitSummary;
  count: number;
  entries: { name: string; path: string; type: "dir" | "file"; commit: CommitSummary }[];
};

export type BlameRange = { start: number; end: number; commit: CommitSummary };

export type Blame = {
  path: string;
  lines: string[];
  ranges: BlameRange[];
  oldest: string;
  newest: string;
};

export type BranchSummary = {
  name: string;
  sha: string;
  ahead: number;
  behind: number;
  updated_at: string;
  author: CommitAuthor;
  protected: boolean;
  default: boolean;
  // The combined commit status of the tip, as GET /commits/{ref}/status computes it.
  status: "success" | "failure" | "pending" | null;
  pull: { number: number; state: "open" | "draft" | "merged" | "closed" } | null;
};

// A directory's README, decoded (GET /repos/{owner}/{repo}/readme answers base64).
export type Readme = { name: string; path: string; sha: string; size: number; text: string };

export type TrafficPoint = { timestamp: string; count: number; uniques: number };
export type ViewTraffic = { count: number; uniques: number; views: TrafficPoint[] };
export type CloneTraffic = { count: number; uniques: number; clones: TrafficPoint[] };
export type Referrer = { referrer: string; count: number; uniques: number };
export type PopularPath = { path: string; title: string; count: number; uniques: number };

type Basic = { message: string; status: string };
type Repo = { owner: string; repo: string };
type Json<T> = { headers: Record<string, unknown>; content: { "application/json": T } };
type Get<P, T, Q = never> = {
  parameters: { query?: Q; header?: never; path: P; cookie?: never };
  requestBody?: never;
  responses: { 200: Json<T>; 404: Json<Basic>; 503: Json<Basic> };
};
type Unused = { put?: never; post?: never; delete?: never; patch?: never };

export type UiPaths = {
  "/repos/{owner}/{repo}/resolve/{splat}": {
    get: Get<Repo & { splat: string }, ResolvedRef>;
  } & Unused;
  "/repos/{owner}/{repo}/tree-commits/{ref}": {
    get: Get<Repo & { ref: string }, TreeCommits>;
  } & Unused;
  "/repos/{owner}/{repo}/tree-commits/{ref}/{path}": {
    get: Get<Repo & { ref: string; path: string }, TreeCommits>;
  } & Unused;
  "/repos/{owner}/{repo}/blame/{ref}/{path}": {
    get: Get<Repo & { ref: string; path: string }, Blame>;
  } & Unused;
  "/repos/{owner}/{repo}/readme": { get: Get<Repo, Readme, { ref?: string }> } & Unused;
  "/repos/{owner}/{repo}/branch-summaries": { get: Get<Repo, BranchSummary[]> } & Unused;
  "/repos/{owner}/{repo}/traffic/views": {
    get: Get<Repo, ViewTraffic, { per?: "day" | "week" }>;
  } & Unused;
  "/repos/{owner}/{repo}/traffic/clones": {
    get: Get<Repo, CloneTraffic, { per?: "day" | "week" }>;
  } & Unused;
  "/repos/{owner}/{repo}/traffic/popular/referrers": { get: Get<Repo, Referrer[]> } & Unused;
  "/repos/{owner}/{repo}/traffic/popular/paths": { get: Get<Repo, PopularPath[]> } & Unused;
};

export const UI_BASE = "/_t/ui";

export const ui = createClient<UiPaths>({ baseUrl: UI_BASE });
ui.use(waitForMocks);
