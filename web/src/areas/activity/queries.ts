import { infiniteQueryOptions, queryOptions } from "@tanstack/react-query";
import { api, ApiError, unwrap, type Schemas } from "@/api/client";
import { SEARCH_TYPES, queryFor, type SearchType } from "./search-query";
import { ui, type WatchEvent } from "./ui-api";

// One factory per page region. Keys start with the REST path segments.

type Repo = { owner: string; repo: string };
export type Thread = Schemas["thread"];

// The design says the client never polls faster than the server's X-Poll-Interval
// (design/SHARED.md, "Frontend and native app"), so the inbox carries it with the data.
const DEFAULT_POLL_S = 60;
const pollOf = (r: Response) =>
  Math.max(1, Number(r.headers.get("X-Poll-Interval")) || DEFAULT_POLL_S);

export type Inbox = { threads: Thread[]; pollInterval: number };

// Every thread in the inbox, read and unread. The bell and the inbox page share it, so
// one poll keeps both current.
export const inboxQuery = () =>
  queryOptions({
    queryKey: ["notifications"],
    queryFn: async ({ signal }): Promise<Inbox> => {
      const { data, error, response } = await api.GET("/notifications", {
        params: { query: { all: true, per_page: 100 } },
        signal,
      });
      if (!response.ok || error !== undefined || !data) throw new ApiError(response.status, error);
      return { threads: data, pollInterval: pollOf(response) };
    },
    refetchInterval: (q) => (q.state.data?.pollInterval ?? DEFAULT_POLL_S) * 1000,
    staleTime: 0,
  });

export const savedQuery = () =>
  queryOptions({
    queryKey: ["_t", "ui", "notifications", "saved"],
    queryFn: ({ signal }) => unwrap(ui.GET("/notifications/saved", { signal })),
  });

export const doneQuery = () =>
  queryOptions({
    queryKey: ["_t", "ui", "notifications", "done"],
    queryFn: ({ signal }) => unwrap(ui.GET("/notifications/done", { signal })),
  });

// ---- Search ----

export const SEARCH_PAGE_SIZE = 10;

export type ValidationError = Schemas["validation-error"];

// A 422 is the server's verdict on the query, not an outage, so it comes back as data
// and the page shows its field errors instead of the region's error box.
export type SearchResult<T> =
  | { kind: "ok"; total: number; incomplete: boolean; items: T[] }
  | { kind: "invalid"; error: ValidationError };

type SearchParams = {
  q: string;
  sort?: string;
  order?: "asc" | "desc";
  page?: number;
  perPage?: number;
};

// GHES sends text_matches only to clients that ask for them.
const TEXT_MATCH = { Accept: "application/vnd.github.text-match+json" };

async function settle<T>(
  call: Promise<{
    data?: { total_count: number; incomplete_results: boolean; items: T[] };
    error?: unknown;
    response: Response;
  }>,
): Promise<SearchResult<T>> {
  const { data, error, response } = await call;
  if (response.status === 422) return { kind: "invalid", error: error as ValidationError };
  if (!response.ok || !data) throw new ApiError(response.status, error);
  return {
    kind: "ok",
    total: data.total_count,
    incomplete: data.incomplete_results,
    items: data.items,
  };
}

export type SearchItems = {
  repositories: Schemas["repo-search-result-item"];
  code: Schemas["code-search-result-item"];
  commits: Schemas["commit-search-result-item"];
  issues: Schemas["issue-search-result-item"];
  pullrequests: Schemas["issue-search-result-item"];
  users: Schemas["user-search-result-item"];
  labels: Schemas["label-search-result-item"];
};

function run(type: SearchType, p: SearchParams & { repositoryId?: number }, signal: AbortSignal) {
  const q = queryFor(type, p.q);
  const base = { q, order: p.order, page: p.page, per_page: p.perPage ?? SEARCH_PAGE_SIZE };
  const sort = p.sort as never;
  const init = { headers: TEXT_MATCH, signal };
  switch (SEARCH_TYPES[type].endpoint) {
    case "repositories":
      return settle(
        api.GET("/search/repositories", { params: { query: { ...base, sort } }, ...init }),
      );
    case "code":
      return settle(api.GET("/search/code", { params: { query: { ...base, sort } }, ...init }));
    case "commits":
      return settle(api.GET("/search/commits", { params: { query: { ...base, sort } }, ...init }));
    case "issues":
      return settle(api.GET("/search/issues", { params: { query: { ...base, sort } }, ...init }));
    case "users":
      return settle(api.GET("/search/users", { params: { query: { ...base, sort } }, ...init }));
    case "labels":
      return settle(
        api.GET("/search/labels", {
          params: { query: { ...base, sort, repository_id: p.repositoryId ?? 0 } },
          ...init,
        }),
      );
  }
}

export const searchQuery = <T extends SearchType>(
  type: T,
  p: SearchParams & { repositoryId?: number },
) =>
  queryOptions({
    queryKey: ["search", type, p],
    queryFn: ({ signal }) => run(type, p, signal) as Promise<SearchResult<SearchItems[T]>>,
    staleTime: 60_000,
  });

// ---- Stars ----

export const starredQuery = ({ owner, repo }: Repo) =>
  queryOptions({
    queryKey: ["user", "starred", owner, repo],
    queryFn: async ({ signal }) => {
      const { response, error } = await api.GET("/user/starred/{owner}/{repo}", {
        params: { path: { owner, repo } },
        signal,
      });
      if (response.status === 204) return true;
      if (response.status === 404) return false;
      throw new ApiError(response.status, error);
    },
  });

type Repository = Schemas["repository"];

// GET /user/starred answers plain repositories unless asked for star+json.
export const myStarsQuery = () =>
  queryOptions({
    queryKey: ["user", "starred"],
    queryFn: async ({ signal }) => {
      const list = await unwrap(
        api.GET("/user/starred", { params: { query: { sort: "created", per_page: 10 } }, signal }),
      );
      return list.filter((r): r is Repository => "full_name" in r);
    },
  });

export const PEOPLE_PAGE_SIZE = 9;

export type Stargazer = Schemas["simple-user"];

// GHES answers simple-user rows with a starred_at, or stargazer wrappers under
// star+json. Both become simple-user rows here.
export const stargazersQuery = ({ owner, repo, page }: Repo & { page: number }) =>
  queryOptions({
    queryKey: ["repos", owner, repo, "stargazers", page],
    queryFn: async ({ signal }): Promise<Stargazer[]> => {
      const rows = await unwrap(
        api.GET("/repos/{owner}/{repo}/stargazers", {
          params: { path: { owner, repo }, query: { page, per_page: PEOPLE_PAGE_SIZE } },
          signal,
        }),
      );
      return rows.flatMap((r) =>
        "login" in r ? [r] : r.user ? [{ ...r.user, starred_at: r.starred_at }] : [],
      );
    },
  });

// Every watcher, for the count; GHES caps a page at 100, which the mock never exceeds.
export const watchersQuery = ({ owner, repo }: Repo) =>
  queryOptions({
    queryKey: ["repos", owner, repo, "subscribers"],
    queryFn: ({ signal }) =>
      unwrap(
        api.GET("/repos/{owner}/{repo}/subscribers", {
          params: { path: { owner, repo }, query: { per_page: 100 } },
          signal,
        }),
      ),
  });

// ---- Watching ----

export type WatchMode = "participating" | "all" | "ignore" | "custom";
export type WatchState = { mode: WatchMode; events: WatchEvent[] };

// GHES has no subscription at all for "participating and @mentions" (GET answers 404).
// The custom event list lives in the private UI API.
export const watchQuery = ({ owner, repo }: Repo) =>
  queryOptions({
    queryKey: ["repos", owner, repo, "subscription"],
    queryFn: async ({ signal }): Promise<WatchState> => {
      const path = { params: { path: { owner, repo } }, signal };
      const [sub, custom] = await Promise.all([
        api.GET("/repos/{owner}/{repo}/subscription", path),
        unwrap(ui.GET("/repos/{owner}/{repo}/subscription/events", path)),
      ]);
      if (!sub.response.ok && sub.response.status !== 404) {
        throw new ApiError(sub.response.status, sub.error);
      }
      if (custom.events) return { mode: "custom", events: custom.events };
      const s = sub.data;
      const mode: WatchMode = !s
        ? "participating"
        : s.ignored
          ? "ignore"
          : s.subscribed
            ? "all"
            : "participating";
      return { mode, events: [] };
    },
  });

// ---- Events ----

export const FEED_PAGE_SIZE = 8;

export const feedQuery = (username: string) =>
  infiniteQueryOptions({
    queryKey: ["users", username, "received_events"],
    queryFn: async ({ pageParam, signal }) => {
      const { data, error, response } = await api.GET("/users/{username}/received_events", {
        params: { path: { username }, query: { page: pageParam, per_page: FEED_PAGE_SIZE } },
        signal,
      });
      if (!response.ok || error !== undefined || !data) throw new ApiError(response.status, error);
      return { events: data, pollInterval: pollOf(response) };
    },
    initialPageParam: 1,
    getNextPageParam: (last, pages) =>
      last.events.length === FEED_PAGE_SIZE ? pages.length + 1 : undefined,
    refetchInterval: (q) => (q.state.data?.pages[0]?.pollInterval ?? DEFAULT_POLL_S) * 1000,
  });

// ---- Rate limit and meta ----

export const rateLimitQuery = () =>
  queryOptions({
    queryKey: ["rate_limit"],
    queryFn: ({ signal }) => unwrap(api.GET("/rate_limit", { signal })),
    refetchInterval: 15_000,
    staleTime: 0,
  });

export const metaQuery = () =>
  queryOptions({
    queryKey: ["meta"],
    queryFn: ({ signal }) => unwrap(api.GET("/meta", { signal })),
    staleTime: 5 * 60_000,
  });
