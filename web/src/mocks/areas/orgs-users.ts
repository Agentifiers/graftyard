import { createOpenApiHttp } from "openapi-msw";
import type { UiPaths } from "@/areas/orgs/api";
import { gate, http, notFound, page, unprocessable } from "../http";
import { me, type Repository } from "../world";
import { db } from "./core-db";
import { sortRepos } from "./orgs";
import {
  allUsers,
  contributions,
  findUser,
  follows,
  minimalRepo,
  orgAsUser,
  orgDb,
  orgSimple,
  profilesDb,
  publicUser,
  reachable,
} from "./orgs-db";

// People: the instance's user list, each user's profile, follows, repositories, and
// organizations, plus the private UI API's profile README, pins, and contribution counts.

const ui = createOpenApiHttp<UiPaths>({ baseUrl: "/_t/ui" });
const viewer = me.login;
const isOrg = (login: string) => !orgDb.deleted && login.toLowerCase() === orgDb.org.login;

// What the viewer may see of a user's or the org's repositories: public ones, plus private
// and internal ones they own or reach through the org.
function visibleTo(full: string): Repository | undefined {
  const [owner, name] = full.split("/");
  const r = db.state(owner, name)?.repo;
  if (!r) return undefined;
  if (r.visibility === "public" || r.owner.login === viewer) return r;
  return reachable(viewer).has(r.full_name.toLowerCase()) ? r : undefined;
}

function reposOf(login: string, type: string | null) {
  const owned = db.repos().filter((r) => r.owner.login === login && visibleTo(r.full_name));
  if (type === "member") {
    return [...reachable(login).keys()].flatMap((full) => visibleTo(full) ?? []);
  }
  if (type === "all") {
    const member = [...reachable(login).keys()].flatMap((full) => visibleTo(full) ?? []);
    return [...owned, ...member.filter((r) => !owned.includes(r))];
  }
  return owned;
}

export const handlers = [
  http.get("/users", async ({ query, response }) => {
    const fault = await gate("users");
    if (fault) return response.untyped(fault);
    const since = Number(query.get("since")) || 0;
    const perPage = Math.min(100, Number(query.get("per_page")) || 30);
    const list = allUsers().sort((a, b) => a.id - b.id);
    return response(200).json(list.filter((u) => u.id > since).slice(0, perPage));
  }),

  http.get("/users/{username}", async ({ params, response }) => {
    const fault = await gate("users-profile");
    if (fault) return response.untyped(fault);
    if (isOrg(params.username)) return response(200).json(orgAsUser());
    const u = findUser(params.username);
    return u ? response(200).json(publicUser(u)) : response.untyped(notFound());
  }),

  http.get("/users/{username}/repos", async ({ params, query, response }) => {
    const fault = await gate("users-repos");
    if (fault) return response.untyped(fault);
    const login = params.username.toLowerCase();
    if (!findUser(login) && !isOrg(login)) return response.untyped(notFound());
    const mine = reachable(viewer);
    const list = sortRepos(
      reposOf(login, query.get("type")),
      query.get("sort") ?? "full_name",
      query.get("direction"),
    );
    return response(200).json(
      page(list, query).map((r) => minimalRepo(r, mine.get(r.full_name.toLowerCase()))),
    );
  }),

  http.get("/users/{username}/orgs", async ({ params, query, response }) => {
    const fault = await gate("users-orgs");
    if (fault) return response.untyped(fault);
    const login = params.username.toLowerCase();
    if (!findUser(login)) return response.untyped(notFound());
    // Public memberships only, unless the viewer asks about themself.
    const shows =
      !orgDb.deleted &&
      orgDb.members.has(login) &&
      (login === viewer || orgDb.publicMembers.has(login));
    return response(200).json(page(shows ? [orgSimple()] : [], query));
  }),

  http.get("/users/{username}/followers", async ({ params, query, response }) => {
    const fault = await gate("users-followers");
    if (fault) return response.untyped(fault);
    if (!findUser(params.username)) return response.untyped(notFound());
    return response(200).json(page(follows.followers(params.username.toLowerCase()), query));
  }),

  http.get("/users/{username}/following", async ({ params, query, response }) => {
    const fault = await gate("users-following");
    if (fault) return response.untyped(fault);
    if (!findUser(params.username)) return response.untyped(notFound());
    return response(200).json(page(follows.following(params.username.toLowerCase()), query));
  }),

  http.get("/users/{username}/following/{target_user}", async ({ params, response }) => {
    const fault = await gate("users-following");
    if (fault) return response.untyped(fault);
    return follows.isFollowing(params.username.toLowerCase(), params.target_user.toLowerCase())
      ? response(204).empty()
      : response(404).empty();
  }),

  http.get("/user/followers", async ({ query, response }) => {
    const fault = await gate("users-followers");
    if (fault) return response.untyped(fault);
    return response(200).json(page(follows.followers(viewer), query));
  }),

  http.get("/user/following", async ({ query, response }) => {
    const fault = await gate("users-following");
    if (fault) return response.untyped(fault);
    return response(200).json(page(follows.following(viewer), query));
  }),

  http.get("/user/following/{username}", async ({ params, response }) => {
    const fault = await gate("users-follow-status");
    if (fault) return response.untyped(fault);
    return follows.isFollowing(viewer, params.username.toLowerCase())
      ? response(204).empty()
      : response.untyped(notFound());
  }),

  http.put("/user/following/{username}", async ({ params, response }) => {
    const fault = await gate("users-follow");
    if (fault) return response.untyped(fault);
    const login = params.username.toLowerCase();
    if (!findUser(login)) return response.untyped(notFound());
    if (login === viewer) return response.untyped(unprocessable("Follow", "username", "invalid"));
    follows.follow(viewer, login);
    me.following = follows.following(viewer).length;
    return response(204).empty();
  }),

  http.delete("/user/following/{username}", async ({ params, response }) => {
    const fault = await gate("users-unfollow");
    if (fault) return response.untyped(fault);
    const login = params.username.toLowerCase();
    if (!findUser(login)) return response.untyped(notFound());
    follows.unfollow(viewer, login);
    me.following = follows.following(viewer).length;
    return response(204).empty();
  }),

  // ---- The private UI API's profile extras ----

  ui.get("/profiles/{login}", async ({ params, response }) => {
    const fault = await gate("users-profile-extras");
    if (fault) return response.untyped(fault);
    const login = params.login.toLowerCase();
    if (!findUser(login) && !isOrg(login)) return response.untyped(notFound());
    return response(200).json(profilesDb.extras(login, visibleTo));
  }),

  ui.get("/profiles/{login}/contributions", async ({ params, response }) => {
    const fault = await gate("users-contributions");
    if (fault) return response.untyped(fault);
    const login = params.login.toLowerCase();
    if (!findUser(login)) return response.untyped(notFound());
    return response(200).json(contributions(login));
  }),

  ui.put("/profiles/{login}/pins", async ({ params, request, response }) => {
    const fault = await gate("users-pins-save");
    if (fault) return response.untyped(fault);
    const login = params.login.toLowerCase();
    if (login !== viewer && !(isOrg(login) && orgDb.members.get(viewer)?.role === "admin")) {
      return response.untyped(notFound());
    }
    const ids = (await request.json()).repository_ids;
    if (ids.length > 6)
      return response.untyped(unprocessable("Profile", "repository_ids", "too_many"));
    const repos = ids.map((id) => db.repos().find((r) => r.id === id));
    if (repos.some((r) => !r || !visibleTo(r.full_name))) {
      return response.untyped(unprocessable("Profile", "repository_ids", "invalid"));
    }
    profilesDb.setPins(
      login,
      repos.map((r) => r!.full_name),
    );
    return response(200).json(profilesDb.extras(login, visibleTo));
  }),
];
