import { queryOptions } from "@tanstack/react-query";
import { api, unwrap, type Schemas } from "@/api/client";
import { ui } from "./ui-api";

// One queryOptions factory per page region. Keys start with the REST path segments; the
// private /_t/ui reads start with "_t" so a GHES key never collides with them.

type Repo = { owner: string; repo: string };

export const resolveQuery = ({ owner, repo, splat }: Repo & { splat: string }) =>
  queryOptions({
    queryKey: ["_t", "repos", owner, repo, "resolve", splat],
    queryFn: ({ signal }) =>
      unwrap(
        ui.GET("/repos/{owner}/{repo}/resolve/{splat}", {
          params: { path: { owner, repo, splat } },
          signal,
        }),
      ),
  });

export const readmeQuery = ({ owner, repo, ref }: Repo & { ref?: string }) =>
  queryOptions({
    queryKey: ["_t", "repos", owner, repo, "readme", ref ?? null],
    queryFn: ({ signal }) =>
      unwrap(
        ui.GET("/repos/{owner}/{repo}/readme", {
          params: { path: { owner, repo }, query: ref ? { ref } : {} },
          signal,
        }),
      ),
  });

export const contentsQuery = ({ owner, repo, ref, path }: Repo & { ref: string; path: string }) =>
  queryOptions({
    queryKey: ["repos", owner, repo, "contents", path, ref],
    queryFn: ({ signal }) =>
      unwrap(
        api.GET("/repos/{owner}/{repo}/contents/{path}", {
          params: { path: { owner, repo, path }, query: { ref } },
          signal,
        }),
      ),
  });

export const treeCommitsQuery = ({
  owner,
  repo,
  ref,
  path,
}: Repo & { ref: string; path: string }) =>
  queryOptions({
    queryKey: ["_t", "repos", owner, repo, "tree-commits", ref, path],
    queryFn: ({ signal }) =>
      unwrap(
        path
          ? ui.GET("/repos/{owner}/{repo}/tree-commits/{ref}/{path}", {
              params: { path: { owner, repo, ref, path } },
              signal,
            })
          : ui.GET("/repos/{owner}/{repo}/tree-commits/{ref}", {
              params: { path: { owner, repo, ref } },
              signal,
            }),
      ),
  });

export const gitTreeQuery = ({ owner, repo, sha }: Repo & { sha: string }) =>
  queryOptions({
    queryKey: ["repos", owner, repo, "git", "trees", sha, "recursive"],
    queryFn: ({ signal }) =>
      unwrap(
        api.GET("/repos/{owner}/{repo}/git/trees/{tree_sha}", {
          params: { path: { owner, repo, tree_sha: sha }, query: { recursive: "1" } },
          signal,
        }),
      ),
    staleTime: Infinity,
  });

export const blameQuery = ({ owner, repo, ref, path }: Repo & { ref: string; path: string }) =>
  queryOptions({
    queryKey: ["_t", "repos", owner, repo, "blame", ref, path],
    queryFn: ({ signal }) =>
      unwrap(
        ui.GET("/repos/{owner}/{repo}/blame/{ref}/{path}", {
          params: { path: { owner, repo, ref, path } },
          signal,
        }),
      ),
  });

export type CommitsFilter = { sha: string; path?: string; author?: string; page: number };

export const COMMITS_PER_PAGE = 20;

export const commitsQuery = ({ owner, repo, sha, path, author, page }: Repo & CommitsFilter) =>
  queryOptions({
    queryKey: [
      "repos",
      owner,
      repo,
      "commits",
      { sha, path: path ?? null, author: author ?? null, page },
    ],
    queryFn: async ({ signal }) => {
      const { data, error, response } = await api.GET("/repos/{owner}/{repo}/commits", {
        params: {
          path: { owner, repo },
          query: {
            sha,
            per_page: COMMITS_PER_PAGE,
            page,
            ...(path ? { path } : {}),
            ...(author ? { author } : {}),
          },
        },
        signal,
      });
      const commits = await unwrap(Promise.resolve({ data, error, response }));
      return { commits, hasNext: /rel="next"/.test(response.headers.get("Link") ?? "") };
    },
  });

// JSON is the default media type; the operation also lists diff and patch as strings.
export const commitQuery = ({ owner, repo, ref }: Repo & { ref: string }) =>
  queryOptions({
    queryKey: ["repos", owner, repo, "commits", ref],
    queryFn: async ({ signal }): Promise<Schemas["commit"]> => {
      const commit = await unwrap(
        api.GET("/repos/{owner}/{repo}/commits/{ref}", {
          params: { path: { owner, repo, ref } },
          signal,
        }),
      );
      if (typeof commit === "string") throw new Error("Expected a JSON commit");
      return commit;
    },
  });

export const combinedStatusQuery = ({ owner, repo, ref }: Repo & { ref: string }) =>
  queryOptions({
    queryKey: ["repos", owner, repo, "commits", ref, "status"],
    queryFn: ({ signal }) =>
      unwrap(
        api.GET("/repos/{owner}/{repo}/commits/{ref}/status", {
          params: { path: { owner, repo, ref } },
          signal,
        }),
      ),
  });

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

export const branchSummariesQuery = ({ owner, repo }: Repo) =>
  queryOptions({
    queryKey: ["_t", "repos", owner, repo, "branch-summaries"],
    queryFn: ({ signal }) =>
      unwrap(
        ui.GET("/repos/{owner}/{repo}/branch-summaries", {
          params: { path: { owner, repo } },
          signal,
        }),
      ),
  });

export const tagsQuery = ({ owner, repo }: Repo) =>
  queryOptions({
    queryKey: ["repos", owner, repo, "tags"],
    queryFn: ({ signal }) =>
      unwrap(
        api.GET("/repos/{owner}/{repo}/tags", {
          params: { path: { owner, repo }, query: { per_page: 100 } },
          signal,
        }),
      ),
  });

export const gitTagQuery = ({ owner, repo, sha }: Repo & { sha: string }) =>
  queryOptions({
    queryKey: ["repos", owner, repo, "git", "tags", sha],
    queryFn: ({ signal }) =>
      unwrap(
        api.GET("/repos/{owner}/{repo}/git/tags/{tag_sha}", {
          params: { path: { owner, repo, tag_sha: sha } },
          signal,
        }),
      ),
    staleTime: Infinity,
  });

export const tagRefsQuery = ({ owner, repo }: Repo) =>
  queryOptions({
    queryKey: ["repos", owner, repo, "git", "matching-refs", "tags"],
    queryFn: ({ signal }) =>
      unwrap(
        api.GET("/repos/{owner}/{repo}/git/matching-refs/{ref}", {
          params: { path: { owner, repo, ref: "tags" } },
          signal,
        }),
      ),
  });

export const languagesQuery = ({ owner, repo }: Repo) =>
  queryOptions({
    queryKey: ["repos", owner, repo, "languages"],
    queryFn: ({ signal }) =>
      unwrap(
        api.GET("/repos/{owner}/{repo}/languages", { params: { path: { owner, repo } }, signal }),
      ),
  });

export const contributorsQuery = ({ owner, repo }: Repo) =>
  queryOptions({
    queryKey: ["repos", owner, repo, "contributors"],
    queryFn: async ({ signal }) =>
      (await unwrap(
        api.GET("/repos/{owner}/{repo}/contributors", {
          params: { path: { owner, repo }, query: { per_page: 100 } },
          signal,
        }),
      )) ?? [],
  });

// GHES answers a stats read with 202 and `{}` while it is still computing. The UI shows
// that the same way as no data.
const computed = <T>(body: T[] | object | undefined): T[] => (Array.isArray(body) ? body : []);

export const contributorStatsQuery = ({ owner, repo }: Repo) =>
  queryOptions({
    queryKey: ["repos", owner, repo, "stats", "contributors"],
    queryFn: async ({ signal }) =>
      computed(
        await unwrap(
          api.GET("/repos/{owner}/{repo}/stats/contributors", {
            params: { path: { owner, repo } },
            signal,
          }),
        ),
      ),
  });

export const commitActivityQuery = ({ owner, repo }: Repo) =>
  queryOptions({
    queryKey: ["repos", owner, repo, "stats", "commit_activity"],
    queryFn: async ({ signal }) =>
      computed(
        await unwrap(
          api.GET("/repos/{owner}/{repo}/stats/commit_activity", {
            params: { path: { owner, repo } },
            signal,
          }),
        ),
      ),
  });

export const codeFrequencyQuery = ({ owner, repo }: Repo) =>
  queryOptions({
    queryKey: ["repos", owner, repo, "stats", "code_frequency"],
    queryFn: async ({ signal }) =>
      computed(
        await unwrap(
          api.GET("/repos/{owner}/{repo}/stats/code_frequency", {
            params: { path: { owner, repo } },
            signal,
          }),
        ),
      ),
  });

export const participationQuery = ({ owner, repo }: Repo) =>
  queryOptions({
    queryKey: ["repos", owner, repo, "stats", "participation"],
    queryFn: ({ signal }) =>
      unwrap(
        api.GET("/repos/{owner}/{repo}/stats/participation", {
          params: { path: { owner, repo } },
          signal,
        }),
      ),
  });

export const punchCardQuery = ({ owner, repo }: Repo) =>
  queryOptions({
    queryKey: ["repos", owner, repo, "stats", "punch_card"],
    queryFn: async ({ signal }) =>
      computed(
        await unwrap(
          api.GET("/repos/{owner}/{repo}/stats/punch_card", {
            params: { path: { owner, repo } },
            signal,
          }),
        ),
      ),
  });

export const viewsQuery = ({ owner, repo, per }: Repo & { per: "day" | "week" }) =>
  queryOptions({
    queryKey: ["_t", "repos", owner, repo, "traffic", "views", per],
    queryFn: ({ signal }) =>
      unwrap(
        ui.GET("/repos/{owner}/{repo}/traffic/views", {
          params: { path: { owner, repo }, query: { per } },
          signal,
        }),
      ),
  });

export const clonesQuery = ({ owner, repo, per }: Repo & { per: "day" | "week" }) =>
  queryOptions({
    queryKey: ["_t", "repos", owner, repo, "traffic", "clones", per],
    queryFn: ({ signal }) =>
      unwrap(
        ui.GET("/repos/{owner}/{repo}/traffic/clones", {
          params: { path: { owner, repo }, query: { per } },
          signal,
        }),
      ),
  });

export const referrersQuery = ({ owner, repo }: Repo) =>
  queryOptions({
    queryKey: ["_t", "repos", owner, repo, "traffic", "popular", "referrers"],
    queryFn: ({ signal }) =>
      unwrap(
        ui.GET("/repos/{owner}/{repo}/traffic/popular/referrers", {
          params: { path: { owner, repo } },
          signal,
        }),
      ),
  });

export const popularPathsQuery = ({ owner, repo }: Repo) =>
  queryOptions({
    queryKey: ["_t", "repos", owner, repo, "traffic", "popular", "paths"],
    queryFn: ({ signal }) =>
      unwrap(
        ui.GET("/repos/{owner}/{repo}/traffic/popular/paths", {
          params: { path: { owner, repo } },
          signal,
        }),
      ),
  });
