import { infiniteQueryOptions, queryOptions } from "@tanstack/react-query";
import { api, ApiError, unwrap } from "@/api/client";
import type { paths } from "@/api/ghes";
import { ext, ui } from "./api";

// One factory per page region. Keys start with the REST path segments, so another area
// reading the same path (GET /users, GET /orgs/{org}/repos) shares the cache entry.

type RepoListQuery = NonNullable<paths["/orgs/{org}/repos"]["get"]["parameters"]["query"]>;
type UserReposQuery = NonNullable<paths["/users/{username}/repos"]["get"]["parameters"]["query"]>;

// ---- Users and profiles ----

export const profileUserQuery = (username: string) =>
  queryOptions({
    queryKey: ["users", username],
    queryFn: ({ signal }) =>
      unwrap(api.GET("/users/{username}", { params: { path: { username } }, signal })),
  });

export const profileExtrasQuery = (login: string) =>
  queryOptions({
    queryKey: ["_t", "ui", "profiles", login],
    queryFn: ({ signal }) =>
      unwrap(ui.GET("/profiles/{login}", { params: { path: { login } }, signal })),
  });

export const contributionsQuery = (login: string) =>
  queryOptions({
    queryKey: ["_t", "ui", "profiles", login, "contributions"],
    queryFn: ({ signal }) =>
      unwrap(ui.GET("/profiles/{login}/contributions", { params: { path: { login } }, signal })),
  });

export const userReposListQuery = (username: string, q: UserReposQuery = {}) =>
  queryOptions({
    queryKey: ["users", username, "repos", q],
    queryFn: ({ signal }) =>
      unwrap(
        api.GET("/users/{username}/repos", {
          params: { path: { username }, query: { per_page: 100, ...q } },
          signal,
        }),
      ),
  });

export const userOrgsListQuery = (username: string) =>
  queryOptions({
    queryKey: ["users", username, "orgs"],
    queryFn: ({ signal }) =>
      unwrap(api.GET("/users/{username}/orgs", { params: { path: { username } }, signal })),
  });

export const followersQuery = (username: string) =>
  queryOptions({
    queryKey: ["users", username, "followers"],
    queryFn: ({ signal }) =>
      unwrap(
        api.GET("/users/{username}/followers", {
          params: { path: { username }, query: { per_page: 100 } },
          signal,
        }),
      ),
  });

export const followingQuery = (username: string) =>
  queryOptions({
    queryKey: ["users", username, "following"],
    queryFn: ({ signal }) =>
      unwrap(
        api.GET("/users/{username}/following", {
          params: { path: { username }, query: { per_page: 100 } },
          signal,
        }),
      ),
  });

// GHES answers repositories, or starred-repository wrappers under the star+json type.
export const starredByQuery = (username: string) =>
  queryOptions({
    queryKey: ["users", username, "starred"],
    queryFn: async ({ signal }) => {
      const rows = await unwrap(
        api.GET("/users/{username}/starred", {
          params: { path: { username }, query: { per_page: 100, sort: "created" } },
          signal,
        }),
      );
      return rows.map((r) => ("repo" in r ? r.repo : r));
    },
  });

// GET /user/following/{username}: 204 when the viewer follows them, 404 when not.
export const isFollowingQuery = (username: string) =>
  queryOptions({
    queryKey: ["user", "following", username],
    queryFn: async ({ signal }) => {
      const { response, error } = await api.GET("/user/following/{username}", {
        params: { path: { username } },
        signal,
      });
      if (response.status === 204) return true;
      if (response.status === 404) return false;
      throw new ApiError(response.status, error);
    },
  });

export const usersDirectoryQuery = () =>
  queryOptions({
    queryKey: ["users"],
    queryFn: ({ signal }) =>
      unwrap(api.GET("/users", { params: { query: { per_page: 100 } }, signal })),
    staleTime: 5 * 60_000,
  });

// ---- The organization ----

const path = (org: string) => ({ params: { path: { org } } });

// The full list, which the Actions secret picker also reads under this key.
export const orgReposQuery = (org: string) =>
  queryOptions({
    queryKey: ["orgs", org, "repos"],
    queryFn: ({ signal }) =>
      unwrap(
        api.GET("/orgs/{org}/repos", {
          params: { path: { org }, query: { per_page: 100 } },
          signal,
        }),
      ),
  });

export const orgReposFilteredQuery = (org: string, q: RepoListQuery) =>
  queryOptions({
    queryKey: ["orgs", org, "repos", q],
    queryFn: ({ signal }) =>
      unwrap(
        api.GET("/orgs/{org}/repos", {
          params: { path: { org }, query: { per_page: 100, ...q } },
          signal,
        }),
      ),
  });

export const membersQuery = (org: string) =>
  queryOptions({
    queryKey: ["orgs", org, "members"],
    queryFn: ({ signal }) =>
      unwrap(
        api.GET("/orgs/{org}/members", {
          params: { path: { org }, query: { per_page: 100 } },
          signal,
        }),
      ),
  });

// The owners, for the role column: GHES lists them with ?role=admin.
export const ownersQuery = (org: string) =>
  queryOptions({
    queryKey: ["orgs", org, "members", { role: "admin" }],
    queryFn: ({ signal }) =>
      unwrap(
        api.GET("/orgs/{org}/members", {
          params: { path: { org }, query: { role: "admin", per_page: 100 } },
          signal,
        }),
      ),
  });

export const twoFactorDisabledQuery = (org: string) =>
  queryOptions({
    queryKey: ["orgs", org, "members", { filter: "2fa_disabled" }],
    queryFn: ({ signal }) =>
      unwrap(
        api.GET("/orgs/{org}/members", {
          params: { path: { org }, query: { filter: "2fa_disabled", per_page: 100 } },
          signal,
        }),
      ),
  });

export const publicMembersQuery = (org: string) =>
  queryOptions({
    queryKey: ["orgs", org, "public_members"],
    queryFn: ({ signal }) =>
      unwrap(api.GET("/orgs/{org}/public_members", { ...path(org), signal })),
  });

export const invitationsQuery = (org: string) =>
  queryOptions({
    queryKey: ["orgs", org, "invitations"],
    queryFn: ({ signal }) =>
      unwrap(
        ext.GET("/orgs/{org}/invitations", {
          params: { path: { org }, query: { per_page: 100 } },
          signal,
        }),
      ),
  });

export const outsideCollaboratorsQuery = (org: string) =>
  queryOptions({
    queryKey: ["orgs", org, "outside_collaborators"],
    queryFn: ({ signal }) =>
      unwrap(api.GET("/orgs/{org}/outside_collaborators", { ...path(org), signal })),
  });

export const teamsQuery = (org: string) =>
  queryOptions({
    queryKey: ["orgs", org, "teams"],
    queryFn: ({ signal }) =>
      unwrap(
        api.GET("/orgs/{org}/teams", {
          params: { path: { org }, query: { per_page: 100 } },
          signal,
        }),
      ),
  });

type Team = { org: string; team: string };

export const teamQuery = ({ org, team }: Team) =>
  queryOptions({
    queryKey: ["orgs", org, "teams", team],
    queryFn: ({ signal }) =>
      unwrap(
        api.GET("/orgs/{org}/teams/{team_slug}", {
          params: { path: { org, team_slug: team } },
          signal,
        }),
      ),
  });

export const teamMembersQuery = ({ org, team }: Team) =>
  queryOptions({
    queryKey: ["orgs", org, "teams", team, "members"],
    queryFn: ({ signal }) =>
      unwrap(
        api.GET("/orgs/{org}/teams/{team_slug}/members", {
          params: { path: { org, team_slug: team }, query: { per_page: 100 } },
          signal,
        }),
      ),
  });

export const teamMaintainersQuery = ({ org, team }: Team) =>
  queryOptions({
    queryKey: ["orgs", org, "teams", team, "members", { role: "maintainer" }],
    queryFn: ({ signal }) =>
      unwrap(
        api.GET("/orgs/{org}/teams/{team_slug}/members", {
          params: { path: { org, team_slug: team }, query: { role: "maintainer", per_page: 100 } },
          signal,
        }),
      ),
  });

export const teamReposQuery = ({ org, team }: Team) =>
  queryOptions({
    queryKey: ["orgs", org, "teams", team, "repos"],
    queryFn: ({ signal }) =>
      unwrap(
        api.GET("/orgs/{org}/teams/{team_slug}/repos", {
          params: { path: { org, team_slug: team }, query: { per_page: 100 } },
          signal,
        }),
      ),
  });

export const childTeamsQuery = ({ org, team }: Team) =>
  queryOptions({
    queryKey: ["orgs", org, "teams", team, "teams"],
    queryFn: ({ signal }) =>
      unwrap(
        api.GET("/orgs/{org}/teams/{team_slug}/teams", {
          params: { path: { org, team_slug: team } },
          signal,
        }),
      ),
  });

// ---- Settings ----

export const renameQuery = (org: string) =>
  queryOptions({
    queryKey: ["orgs", org, "rename"],
    queryFn: ({ signal }) => unwrap(ext.GET("/orgs/{org}/rename", { ...path(org), signal })),
  });

export const teamSettingsQuery = (org: string) =>
  queryOptions({
    queryKey: ["_t", "ui", "orgs", org, "team-settings"],
    queryFn: ({ signal }) => unwrap(ui.GET("/orgs/{org}/team-settings", { ...path(org), signal })),
  });

export const hooksQuery = (org: string) =>
  queryOptions({
    queryKey: ["orgs", org, "hooks"],
    queryFn: ({ signal }) => unwrap(api.GET("/orgs/{org}/hooks", { ...path(org), signal })),
  });

export const hookQuery = (org: string, id: number) =>
  queryOptions({
    queryKey: ["orgs", org, "hooks", id],
    queryFn: ({ signal }) =>
      unwrap(
        api.GET("/orgs/{org}/hooks/{hook_id}", {
          params: { path: { org, hook_id: id } },
          signal,
        }),
      ),
  });

export const deliveriesQuery = (org: string, id: number, status?: "success" | "failure") =>
  queryOptions({
    queryKey: ["orgs", org, "hooks", id, "deliveries", status ?? "all"],
    queryFn: ({ signal }) =>
      unwrap(
        api.GET("/orgs/{org}/hooks/{hook_id}/deliveries", {
          params: { path: { org, hook_id: id }, query: { per_page: 50, status } },
          signal,
        }),
      ),
  });

export const deliveryQuery = (org: string, hook: number, id: number) =>
  queryOptions({
    queryKey: ["orgs", org, "hooks", hook, "deliveries", "detail", id],
    queryFn: ({ signal }) =>
      unwrap(
        api.GET("/orgs/{org}/hooks/{hook_id}/deliveries/{delivery_id}", {
          params: { path: { org, hook_id: hook, delivery_id: id } },
          signal,
        }),
      ),
  });

export const rulesetsQuery = (org: string) =>
  queryOptions({
    queryKey: ["orgs", org, "rulesets"],
    queryFn: ({ signal }) => unwrap(api.GET("/orgs/{org}/rulesets", { ...path(org), signal })),
  });

export const rulesetQuery = (org: string, id: number) =>
  queryOptions({
    queryKey: ["orgs", org, "rulesets", id],
    queryFn: ({ signal }) =>
      unwrap(
        api.GET("/orgs/{org}/rulesets/{ruleset_id}", {
          params: { path: { org, ruleset_id: id } },
          signal,
        }),
      ),
  });

export const rulesetHistoryQuery = (org: string, id: number) =>
  queryOptions({
    queryKey: ["orgs", org, "rulesets", id, "history"],
    queryFn: ({ signal }) =>
      unwrap(
        api.GET("/orgs/{org}/rulesets/{ruleset_id}/history", {
          params: { path: { org, ruleset_id: id } },
          signal,
        }),
      ),
  });

export const preReceiveQuery = (org: string) =>
  queryOptions({
    queryKey: ["orgs", org, "pre-receive-hooks"],
    queryFn: ({ signal }) =>
      unwrap(api.GET("/orgs/{org}/pre-receive-hooks", { ...path(org), signal })),
  });

export type AuditFilters = { phrase: string; include: "web" | "git" | "all" };

// Paged as GHES pages it, 100 a page; the virtual table loads the next page as it nears
// the end of what it has.
export const auditQuery = (org: string, f: AuditFilters) =>
  infiniteQueryOptions({
    queryKey: ["orgs", org, "audit-log", f],
    initialPageParam: 1,
    queryFn: ({ signal, pageParam }) =>
      unwrap(
        api.GET("/orgs/{org}/audit-log", {
          params: {
            path: { org },
            query: { phrase: f.phrase, include: f.include, per_page: 100, page: pageParam },
          },
          signal,
        }),
      ),
    getNextPageParam: (last, _all, n) => (last.length === 100 ? n + 1 : undefined),
  });
