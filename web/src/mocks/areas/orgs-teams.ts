import type { Schemas } from "@/api/client";
import { gate, http, notFound, page, unprocessable } from "../http";
import { API, me, type Repository } from "../world";
import { db } from "./core-db";
import { invalid } from "./orgs";
import {
  addTeam,
  asPermission,
  findOrg,
  findUser,
  log,
  minimalRepo,
  orgDb,
  permissionFlags,
  slugify,
  teamById,
  teamBySlug,
  teamFull,
  teamMember,
  teamView,
  type TeamRec,
} from "./orgs-db";

// Teams, by slug under /orgs/{org}/teams and by id under the legacy /teams/{team_id}.
// Both routes run the same operations on the same records.

type TeamRole = "member" | "maintainer";
type Found = { team: TeamRec } | { error: Response };

const viewer = me.login;

function bySlug(org: string, slug: string): Found {
  if (!findOrg(org)) return { error: notFound() };
  const team = teamBySlug(slug);
  return team ? { team } : { error: notFound() };
}

function byId(id: string): Found {
  const team = orgDb.deleted ? undefined : teamById(Number(id));
  return team ? { team } : { error: notFound() };
}

const children = (t: TeamRec) => orgDb.teams.filter((c) => c.parentId === t.id);

// Every team under t, for the cycle check and for inherited members.
function descendants(t: TeamRec): TeamRec[] {
  return children(t).flatMap((c) => [c, ...descendants(c)]);
}

// GHES lists a parent team's members with its child teams' members marked inherited.
function members(t: TeamRec, role: string | null) {
  const direct = [...t.members].map(([login, r]) => teamMember(login, r));
  const seen = new Set(t.members.keys());
  const inherited = descendants(t).flatMap((c) =>
    [...c.members].flatMap(([login, r]) => {
      if (seen.has(login)) return [];
      seen.add(login);
      return [teamMember(login, r, true)];
    }),
  );
  return [...direct, ...inherited]
    .filter((m) => !role || role === "all" || m.role === role)
    .sort((a, b) => a.login.localeCompare(b.login));
}

function repos(t: TeamRec) {
  return [...t.repos].flatMap(([full, p]) => {
    const [owner, name] = full.split("/");
    const r = db.state(owner, name)?.repo;
    return r ? [minimalRepo(r, p)] : [];
  });
}

function teamRepository(r: Repository, t: TeamRec): Schemas["team-repository"] {
  const p = t.repos.get(r.full_name.toLowerCase()) ?? "pull";
  const { role_name } = minimalRepo(r, p);
  return {
    ...r,
    allow_forking: r.allow_forking ?? true,
    permissions: permissionFlags(p),
    role_name,
    master_branch: r.default_branch,
  };
}

// The parent a create or update names, checked for existence, secrecy, and cycles.
function parentFor(
  body: { parent_team_id?: number | null; parent_team_slug?: string | null; privacy?: string },
  self: TeamRec | undefined,
): { id: number | null | undefined } | { error: Response } {
  const raw =
    body.parent_team_id !== undefined
      ? body.parent_team_id
      : body.parent_team_slug !== undefined
        ? body.parent_team_slug === null
          ? null
          : (teamBySlug(body.parent_team_slug)?.id ?? -1)
        : undefined;
  if (raw === undefined || raw === null) return { id: raw };
  const parent = teamById(raw);
  if (!parent) return { error: unprocessable("Team", "parent_team_id", "invalid") };
  const privacy = body.privacy ?? self?.privacy ?? "closed";
  if (privacy === "secret" || parent.privacy === "secret") {
    return { error: unprocessable("Team", "parent_team_id", "secret_nesting") };
  }
  if (self && (parent.id === self.id || descendants(self).includes(parent))) {
    return { error: unprocessable("Team", "parent_team_id", "cycle") };
  }
  return { id: parent.id };
}

type TeamPatch = {
  name?: string;
  description?: string;
  privacy?: "secret" | "closed";
  notification_setting?: "notifications_enabled" | "notifications_disabled";
  permission?: string;
  parent_team_id?: number | null;
  parent_team_slug?: string | null;
};

function update(t: TeamRec, body: TeamPatch): Response | undefined {
  if (body.name !== undefined) {
    const name = body.name.trim();
    if (!name) return unprocessable("Team", "name", "missing_field");
    const slug = slugify(name);
    if (orgDb.teams.some((x) => x !== t && (x.slug === slug || x.name === name))) {
      return unprocessable("Team", "name", "already_exists");
    }
    t.name = name;
    t.slug = slug;
  }
  if (body.privacy === "secret" && (t.parentId !== null || children(t).length)) {
    return unprocessable("Team", "privacy", "secret_nesting");
  }
  const parent = parentFor(body, t);
  if ("error" in parent) return parent.error;
  if (parent.id !== undefined) t.parentId = parent.id;
  if (body.description !== undefined) t.description = body.description.trim() || null;
  if (body.privacy) t.privacy = body.privacy;
  if (body.notification_setting) t.notification_setting = body.notification_setting;
  if (body.permission) t.permission = body.permission;
  t.updated_at = new Date().toISOString();
  log("team.update", viewer, { team: `${orgDb.org.login}/${t.slug}` });
  return undefined;
}

function remove(t: TeamRec) {
  // GHES moves the deleted team's children up to its parent.
  for (const c of children(t)) c.parentId = t.parentId;
  orgDb.teams.splice(orgDb.teams.indexOf(t), 1);
  log("team.destroy", viewer, { team: `${orgDb.org.login}/${t.slug}` });
}

function addMember(
  t: TeamRec,
  username: string,
  role: TeamRole,
): Response | Schemas["team-membership"] {
  const login = username.toLowerCase();
  if (!findUser(login)) return notFound();
  if (!orgDb.members.has(login)) {
    return invalid(
      "Team",
      "username",
      "invalid",
      `${login} is not a member of ${orgDb.org.login}.`,
    );
  }
  const had = t.members.has(login);
  t.members.set(login, role);
  t.updated_at = new Date().toISOString();
  if (!had) log("team.add_member", viewer, { team: `${orgDb.org.login}/${t.slug}`, user: login });
  return { url: `${t.id}`, role, state: "active" };
}

function membershipOf(t: TeamRec, login: string): Schemas["team-membership"] {
  return {
    url: `${API}/teams/${t.id}/memberships/${login}`,
    role: t.members.get(login)!,
    state: "active",
  };
}

function removeMember(t: TeamRec, username: string) {
  const login = username.toLowerCase();
  if (!t.members.delete(login)) return false;
  log("team.remove_member", viewer, { team: `${orgDb.org.login}/${t.slug}`, user: login });
  return true;
}

function setRepo(t: TeamRec, owner: string, repo: string, raw: string | undefined) {
  const r = db.state(owner, repo)?.repo;
  if (!r || r.owner.login !== orgDb.org.login) return notFound();
  const p = asPermission(raw ?? "push");
  if (!p) return unprocessable("TeamRepository", "permission", "invalid");
  const key = r.full_name.toLowerCase();
  const had = t.repos.has(key);
  t.repos.set(key, p);
  log(had ? "team.update_repository_permission" : "team.add_repository", viewer, {
    team: `${orgDb.org.login}/${t.slug}`,
    repo: r.full_name,
    data: { permission: p },
  });
  return undefined;
}

function removeRepo(t: TeamRec, owner: string, repo: string) {
  const key = `${owner}/${repo}`.toLowerCase();
  if (!t.repos.delete(key)) return false;
  log("team.remove_repository", viewer, { team: `${orgDb.org.login}/${t.slug}`, repo: key });
  return true;
}

function checkRepo(t: TeamRec, owner: string, repo: string) {
  const r = db.state(owner, repo)?.repo;
  return r && t.repos.has(r.full_name.toLowerCase()) ? r : undefined;
}

export const handlers = [
  // ---- By slug ----

  http.get("/orgs/{org}/teams", async ({ params, query, response }) => {
    const fault = await gate("orgs-teams");
    if (fault) return response.untyped(fault);
    if (!findOrg(params.org)) return response.untyped(notFound());
    const list = [...orgDb.teams].sort((a, b) => a.name.localeCompare(b.name)).map(teamView);
    return response(200).json(page(list, query));
  }),

  http.post("/orgs/{org}/teams", async ({ params, request, response }) => {
    const fault = await gate("orgs-team-create");
    if (fault) return response.untyped(fault);
    if (!findOrg(params.org)) return response.untyped(notFound());
    const body = await request.json();
    const name = body.name.trim();
    if (!name) return response.untyped(unprocessable("Team", "name", "missing_field"));
    if (name.length > 255) return response.untyped(unprocessable("Team", "name", "too_long"));
    const slug = slugify(name);
    if (!slug) {
      return response.untyped(
        invalid("Team", "name", "invalid", "Use at least one letter or digit."),
      );
    }
    if (orgDb.teams.some((t) => t.slug === slug || t.name.toLowerCase() === name.toLowerCase())) {
      return response.untyped(unprocessable("Team", "name", "already_exists"));
    }
    const privacy = body.privacy ?? orgDb.teamDefaults.default_privacy;
    const parent = parentFor({ ...body, privacy }, undefined);
    if ("error" in parent) return response.untyped(parent.error);
    const maintainers = (body.maintainers ?? []).map((l) => l.toLowerCase());
    const stranger = maintainers.find((l) => !orgDb.members.has(l));
    if (stranger) {
      return response.untyped(
        invalid(
          "Team",
          "maintainers",
          "invalid",
          `${stranger} is not a member of ${orgDb.org.login}.`,
        ),
      );
    }
    const now = new Date().toISOString();
    const t = addTeam({
      slug,
      name,
      description: body.description?.trim() || null,
      privacy,
      notification_setting:
        body.notification_setting ?? orgDb.teamDefaults.default_notification_setting,
      permission: body.permission ?? "pull",
      parentId: parent.id ?? null,
      // GHES makes the creator a maintainer unless maintainers are named.
      members: (maintainers.length ? maintainers : [viewer]).map((l) => [l, "maintainer"]),
      repos: (body.repo_names ?? []).flatMap((full) => {
        const [owner, repo] = full.includes("/") ? full.split("/") : [orgDb.org.login, full];
        const r = db.state(owner, repo)?.repo;
        return r ? [[r.full_name.toLowerCase(), "pull"] as [string, "pull"]] : [];
      }),
      created: now,
    });
    t.created_at = t.updated_at = now;
    log("team.create", viewer, { team: `${orgDb.org.login}/${t.slug}` });
    return response(201).json(teamFull(t));
  }),

  http.get("/orgs/{org}/teams/{team_slug}", async ({ params, response }) => {
    const fault = await gate("orgs-team");
    if (fault) return response.untyped(fault);
    const f = bySlug(params.org, params.team_slug);
    return "error" in f ? response.untyped(f.error) : response(200).json(teamFull(f.team));
  }),

  http.patch("/orgs/{org}/teams/{team_slug}", async ({ params, request, response }) => {
    const fault = await gate("orgs-team-save");
    if (fault) return response.untyped(fault);
    const f = bySlug(params.org, params.team_slug);
    if ("error" in f) return response.untyped(f.error);
    const problem = update(f.team, (await request.json()) ?? {});
    return problem ? response.untyped(problem) : response(200).json(teamFull(f.team));
  }),

  http.delete("/orgs/{org}/teams/{team_slug}", async ({ params, response }) => {
    const fault = await gate("orgs-team-delete");
    if (fault) return response.untyped(fault);
    const f = bySlug(params.org, params.team_slug);
    if ("error" in f) return response.untyped(f.error);
    remove(f.team);
    return response(204).empty();
  }),

  http.get("/orgs/{org}/teams/{team_slug}/members", async ({ params, query, response }) => {
    const fault = await gate("orgs-team-members");
    if (fault) return response.untyped(fault);
    const f = bySlug(params.org, params.team_slug);
    if ("error" in f) return response.untyped(f.error);
    return response(200).json(page(members(f.team, query.get("role")), query));
  }),

  http.get("/orgs/{org}/teams/{team_slug}/memberships/{username}", async ({ params, response }) => {
    const fault = await gate("orgs-team-members");
    if (fault) return response.untyped(fault);
    const f = bySlug(params.org, params.team_slug);
    if ("error" in f) return response.untyped(f.error);
    const login = params.username.toLowerCase();
    return f.team.members.has(login)
      ? response(200).json(membershipOf(f.team, login))
      : response(404).empty();
  }),

  http.put(
    "/orgs/{org}/teams/{team_slug}/memberships/{username}",
    async ({ params, request, response }) => {
      const fault = await gate("orgs-team-member-add");
      if (fault) return response.untyped(fault);
      const f = bySlug(params.org, params.team_slug);
      if ("error" in f) return response.untyped(f.error);
      const role = (await request.json())?.role ?? "member";
      const out = addMember(f.team, params.username, role);
      return out instanceof Response
        ? response.untyped(out)
        : response(200).json(membershipOf(f.team, params.username.toLowerCase()));
    },
  ),

  http.delete(
    "/orgs/{org}/teams/{team_slug}/memberships/{username}",
    async ({ params, response }) => {
      const fault = await gate("orgs-team-member-remove");
      if (fault) return response.untyped(fault);
      const f = bySlug(params.org, params.team_slug);
      if ("error" in f) return response.untyped(f.error);
      return removeMember(f.team, params.username) ? response(204).empty() : response(403).empty();
    },
  ),

  http.get("/orgs/{org}/teams/{team_slug}/repos", async ({ params, query, response }) => {
    const fault = await gate("orgs-team-repos");
    if (fault) return response.untyped(fault);
    const f = bySlug(params.org, params.team_slug);
    if ("error" in f) return response.untyped(f.error);
    return response(200).json(page(repos(f.team), query));
  }),

  http.get("/orgs/{org}/teams/{team_slug}/repos/{owner}/{repo}", async ({ params, response }) => {
    const fault = await gate("orgs-team-repos");
    if (fault) return response.untyped(fault);
    const f = bySlug(params.org, params.team_slug);
    if ("error" in f) return response.untyped(f.error);
    const r = checkRepo(f.team, params.owner, params.repo);
    return r ? response(200).json(teamRepository(r, f.team)) : response(404).empty();
  }),

  http.put(
    "/orgs/{org}/teams/{team_slug}/repos/{owner}/{repo}",
    async ({ params, request, response }) => {
      const fault = await gate("orgs-team-repo-save");
      if (fault) return response.untyped(fault);
      const f = bySlug(params.org, params.team_slug);
      if ("error" in f) return response.untyped(f.error);
      const body = (await request.json()) ?? {};
      const problem = setRepo(f.team, params.owner, params.repo, body.permission);
      return problem ? response.untyped(problem) : response(204).empty();
    },
  ),

  http.delete(
    "/orgs/{org}/teams/{team_slug}/repos/{owner}/{repo}",
    async ({ params, response }) => {
      const fault = await gate("orgs-team-repo-remove");
      if (fault) return response.untyped(fault);
      const f = bySlug(params.org, params.team_slug);
      if ("error" in f) return response.untyped(f.error);
      return removeRepo(f.team, params.owner, params.repo)
        ? response(204).empty()
        : response.untyped(notFound());
    },
  ),

  http.get("/orgs/{org}/teams/{team_slug}/teams", async ({ params, query, response }) => {
    const fault = await gate("orgs-team-children");
    if (fault) return response.untyped(fault);
    const f = bySlug(params.org, params.team_slug);
    if ("error" in f) return response.untyped(f.error);
    return response(200).json(page(children(f.team).map(teamView), query));
  }),

  // ---- By id (legacy) ----

  http.get("/teams/{team_id}", async ({ params, response }) => {
    const fault = await gate("orgs-team");
    if (fault) return response.untyped(fault);
    const f = byId(params.team_id);
    return "error" in f ? response.untyped(f.error) : response(200).json(teamFull(f.team));
  }),

  http.patch("/teams/{team_id}", async ({ params, request, response }) => {
    const fault = await gate("orgs-team-save");
    if (fault) return response.untyped(fault);
    const f = byId(params.team_id);
    if ("error" in f) return response.untyped(f.error);
    const problem = update(f.team, await request.json());
    return problem ? response.untyped(problem) : response(200).json(teamFull(f.team));
  }),

  http.delete("/teams/{team_id}", async ({ params, response }) => {
    const fault = await gate("orgs-team-delete");
    if (fault) return response.untyped(fault);
    const f = byId(params.team_id);
    if ("error" in f) return response.untyped(f.error);
    remove(f.team);
    return response(204).empty();
  }),

  http.get("/teams/{team_id}/members", async ({ params, query, response }) => {
    const fault = await gate("orgs-team-members");
    if (fault) return response.untyped(fault);
    const f = byId(params.team_id);
    if ("error" in f) return response.untyped(f.error);
    return response(200).json(page(members(f.team, query.get("role")), query));
  }),

  http.get("/teams/{team_id}/memberships/{username}", async ({ params, response }) => {
    const fault = await gate("orgs-team-members");
    if (fault) return response.untyped(fault);
    const f = byId(params.team_id);
    if ("error" in f) return response.untyped(f.error);
    const login = params.username.toLowerCase();
    return f.team.members.has(login)
      ? response(200).json(membershipOf(f.team, login))
      : response.untyped(notFound());
  }),

  http.put("/teams/{team_id}/memberships/{username}", async ({ params, request, response }) => {
    const fault = await gate("orgs-team-member-add");
    if (fault) return response.untyped(fault);
    const f = byId(params.team_id);
    if ("error" in f) return response.untyped(f.error);
    const role = (await request.json())?.role ?? "member";
    const out = addMember(f.team, params.username, role);
    return out instanceof Response
      ? response.untyped(out)
      : response(200).json(membershipOf(f.team, params.username.toLowerCase()));
  }),

  http.delete("/teams/{team_id}/memberships/{username}", async ({ params, response }) => {
    const fault = await gate("orgs-team-member-remove");
    if (fault) return response.untyped(fault);
    const f = byId(params.team_id);
    if ("error" in f) return response.untyped(f.error);
    return removeMember(f.team, params.username) ? response(204).empty() : response(403).empty();
  }),

  http.get("/teams/{team_id}/repos", async ({ params, query, response }) => {
    const fault = await gate("orgs-team-repos");
    if (fault) return response.untyped(fault);
    const f = byId(params.team_id);
    if ("error" in f) return response.untyped(f.error);
    return response(200).json(page(repos(f.team), query));
  }),

  http.put("/teams/{team_id}/repos/{owner}/{repo}", async ({ params, request, response }) => {
    const fault = await gate("orgs-team-repo-save");
    if (fault) return response.untyped(fault);
    const f = byId(params.team_id);
    if ("error" in f) return response.untyped(f.error);
    const body = (await request.json()) ?? {};
    const problem = setRepo(f.team, params.owner, params.repo, body.permission);
    return problem ? response.untyped(problem) : response(204).empty();
  }),

  http.delete("/teams/{team_id}/repos/{owner}/{repo}", async ({ params, response }) => {
    const fault = await gate("orgs-team-repo-remove");
    if (fault) return response.untyped(fault);
    const f = byId(params.team_id);
    if ("error" in f) return response.untyped(f.error);
    return removeRepo(f.team, params.owner, params.repo)
      ? response(204).empty()
      : response.untyped(notFound());
  }),

  http.get("/teams/{team_id}/teams", async ({ params, query, response }) => {
    const fault = await gate("orgs-team-children");
    if (fault) return response.untyped(fault);
    const f = byId(params.team_id);
    if ("error" in f) return response.untyped(f.error);
    return response(200).json(page(children(f.team).map(teamView), query));
  }),
];
