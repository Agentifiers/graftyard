import { HttpResponse } from "msw";
import { createOpenApiHttp } from "openapi-msw";
import type { Schemas } from "@/api/client";
import type { ExtPaths, UiPaths } from "@/areas/orgs/api";
import { gate, http, notFound, page, unprocessable } from "../http";
import { API, me, users, type Repository } from "../world";
import { db } from "./core-db";
import {
  allUsers,
  findOrg,
  findUser,
  invitation,
  log,
  minimalRepo,
  orgDb,
  orgSimple,
  reachable,
  searchAudit,
  teamById,
} from "./orgs-db";

// The organization itself: its profile, deletion, a queued rename, member privileges,
// team defaults, members and memberships, invitations, outside collaborators, its
// repository list, and the audit log.

const ext = createOpenApiHttp<ExtPaths>({ baseUrl: "/api/v3" });
const ui = createOpenApiHttp<UiPaths>({ baseUrl: "/_t/ui" });
const DOCS = "https://docs.github.com/enterprise-server@3.22/rest";

// GHES's validation error with a message the form shows under its field.
export function invalid(resource: string, field: string, code: string, message: string) {
  return HttpResponse.json(
    {
      message: "Validation Failed",
      documentation_url: DOCS,
      status: "422",
      errors: [{ resource, field, code, message }],
    },
    { status: 422 },
  );
}

const EMAIL_RE = /^[^\s@]+@[^\s@]+\.[^\s@]+$/;
const URL_RE = /^https?:\/\/\S+\.\S+$/;
const LOGIN_RE = /^[a-z\d](?:[a-z\d]|-(?=[a-z\d])){0,38}$/i;
const viewer = me.login;

const ownerCount = () => [...orgDb.members.values()].filter((m) => m.role === "admin").length;

function membership(login: string): Schemas["org-membership"] | undefined {
  const m = orgDb.members.get(login);
  const u = findUser(login);
  if (!m || !u) return undefined;
  return {
    url: `${API}/orgs/${orgDb.org.login}/memberships/${login}`,
    state: "active",
    role: m.role,
    direct_membership: true,
    organization_url: orgDb.org.url,
    organization: orgSimple(),
    user: u,
    permissions: {
      can_create_repository:
        m.role === "admin" || orgDb.org.members_can_create_repositories === true,
    },
  };
}

function orgRepos(): Repository[] {
  return db.repos().filter((r) => r.owner.login === orgDb.org.login);
}

// GET /orgs/{org}/repos and GET /users/{username}/repos share their sort.
export function sortRepos(list: Repository[], sort: string | null, direction: string | null) {
  const key = sort ?? "created";
  const dir = direction ?? (key === "full_name" ? "asc" : "desc");
  const value = (r: Repository) =>
    key === "full_name"
      ? r.full_name.toLowerCase()
      : key === "updated"
        ? (r.updated_at ?? "")
        : key === "pushed"
          ? (r.pushed_at ?? "")
          : (r.created_at ?? "");
  const sorted = [...list].sort((a, b) => value(a).localeCompare(value(b)));
  return dir === "desc" ? sorted.reverse() : sorted;
}

// A person removed from the org leaves every team too.
function dropFromTeams(login: string) {
  for (const t of orgDb.teams) t.members.delete(login);
}

export const handlers = [
  // ---- The organization ----

  // The shell answers GET /orgs/{org}; this only hides an org this session deleted.
  http.get("/orgs/{org}", ({ params, response }) =>
    orgDb.deleted && params.org.toLowerCase() === orgDb.org.login
      ? response.untyped(notFound())
      : undefined,
  ),

  http.patch("/orgs/{org}", async ({ params, request, response }) => {
    const fault = await gate("orgs-profile-save");
    if (fault) return response.untyped(fault);
    const o = findOrg(params.org);
    if (!o) return response.untyped(notFound());
    const body = (await request.json()) ?? {};
    if (body.name !== undefined && body.name.length > 100) {
      return response.untyped(invalid("Organization", "name", "too_long", "Name is too long."));
    }
    if (body.description !== undefined && body.description.length > 160) {
      return response.untyped(
        invalid(
          "Organization",
          "description",
          "too_long",
          "Description is at most 160 characters.",
        ),
      );
    }
    for (const field of ["email", "billing_email"] as const) {
      const v = body[field];
      if (v && !EMAIL_RE.test(v)) {
        return response.untyped(invalid("Organization", field, "invalid", "Not an email address."));
      }
      if (v && /@example\.(com|org)$/i.test(v)) {
        return response.untyped(unprocessable("Organization", field, "reserved_domain"));
      }
    }
    if (body.billing_email === "") {
      return response.untyped(unprocessable("Organization", "billing_email", "missing_field"));
    }
    if (body.blog && !URL_RE.test(body.blog)) {
      return response.untyped(
        invalid(
          "Organization",
          "blog",
          "invalid",
          "URL must be a full address, such as https://example.com.",
        ),
      );
    }
    const blank = (v: string | undefined) => (v === undefined ? undefined : v.trim() || null);
    const { org } = o;
    if (body.name !== undefined) org.name = body.name.trim() || org.login;
    if (body.description !== undefined) org.description = blank(body.description) ?? null;
    for (const k of ["email", "blog", "location", "company", "twitter_username"] as const) {
      if (body[k] !== undefined) org[k] = blank(body[k]) ?? null;
    }
    if (body.billing_email !== undefined) org.billing_email = body.billing_email;
    if (body.default_repository_permission !== undefined) {
      org.default_repository_permission = body.default_repository_permission;
    }
    for (const k of [
      "members_can_create_repositories",
      "members_can_create_public_repositories",
      "members_can_create_private_repositories",
      "members_can_create_internal_repositories",
      "members_can_create_pages",
      "members_can_fork_private_repositories",
      "web_commit_signoff_required",
      "has_organization_projects",
      "has_repository_projects",
    ] as const) {
      if (body[k] !== undefined) org[k] = body[k];
    }
    if (body.members_can_create_repositories === false) {
      org.members_can_create_public_repositories = false;
      org.members_can_create_private_repositories = false;
      org.members_can_create_internal_repositories = false;
    }
    org.updated_at = new Date().toISOString();
    log("org.update", viewer, { data: { fields: Object.keys(body) } });
    return response(200).json(org);
  }),

  http.delete("/orgs/{org}", async ({ params, response }) => {
    const fault = await gate("orgs-delete");
    if (fault) return response.untyped(fault);
    if (!findOrg(params.org)) return response.untyped(notFound());
    orgDb.deleted = true;
    log("org.delete", viewer);
    return response(202).json({});
  }),

  ext.get("/orgs/{org}/rename", async ({ params, response }) => {
    const fault = await gate("orgs-rename-status");
    if (fault) return response.untyped(fault);
    if (!findOrg(params.org)) return response.untyped(notFound());
    return response(200).json(orgDb.rename);
  }),

  ext.post("/orgs/{org}/rename", async ({ params, request, response }) => {
    const fault = await gate("orgs-rename");
    if (fault) return response.untyped(fault);
    if (!findOrg(params.org)) return response.untyped(notFound());
    const login = (await request.json()).login.trim();
    if (!login) return response.untyped(unprocessable("Organization", "login", "missing_field"));
    if (!LOGIN_RE.test(login)) {
      return response.untyped(
        invalid(
          "Organization",
          "login",
          "invalid",
          "Use letters, digits, and single hyphens, at most 39 characters, not starting or ending with a hyphen.",
        ),
      );
    }
    if (login.toLowerCase() === orgDb.org.login) {
      return response.untyped(
        invalid("Organization", "login", "invalid", "That is already this organization's name."),
      );
    }
    if (
      findUser(login) ||
      ["admin", "stafftools", "settings", "orgs"].includes(login.toLowerCase())
    ) {
      return response.untyped(unprocessable("Organization", "login", "already_exists"));
    }
    Object.assign(orgDb.rename, {
      login,
      queued_at: new Date().toISOString(),
      message: `Queued. ${orgDb.org.login} becomes ${login} in the next maintenance window. Every repository URL redirects afterwards.`,
    });
    log("org.rename", viewer, { data: { new_login: login } });
    return response(202).json(orgDb.rename);
  }),

  ext.delete("/orgs/{org}/rename", async ({ params, response }) => {
    const fault = await gate("orgs-rename-cancel");
    if (fault) return response.untyped(fault);
    if (!findOrg(params.org) || !orgDb.rename.login) return response.untyped(notFound());
    Object.assign(orgDb.rename, { login: null, queued_at: null, message: "No rename is queued." });
    return response(204).empty();
  }),

  ui.patch("/orgs/{org}/member-privileges", async ({ params, request, response }) => {
    const fault = await gate("orgs-privileges-save");
    if (fault) return response.untyped(fault);
    const o = findOrg(params.org);
    if (!o) return response.untyped(notFound());
    const body = await request.json();
    Object.assign(o.org, body);
    o.org.updated_at = new Date().toISOString();
    log("org.update_member_privileges", viewer, { data: body });
    return response(200).json(o.org);
  }),

  ui.get("/orgs/{org}/team-settings", async ({ params, response }) => {
    const fault = await gate("orgs-team-settings");
    if (fault) return response.untyped(fault);
    const o = findOrg(params.org);
    if (!o) return response.untyped(notFound());
    return response(200).json({
      members_can_create_teams: o.org.members_can_create_teams ?? true,
      ...o.teamDefaults,
    });
  }),

  ui.patch("/orgs/{org}/team-settings", async ({ params, request, response }) => {
    const fault = await gate("orgs-team-settings-save");
    if (fault) return response.untyped(fault);
    const o = findOrg(params.org);
    if (!o) return response.untyped(notFound());
    const { members_can_create_teams, ...defaults } = await request.json();
    if (members_can_create_teams !== undefined) {
      o.org.members_can_create_teams = members_can_create_teams;
    }
    Object.assign(o.teamDefaults, defaults);
    log("org.update_team_settings", viewer);
    return response(200).json({
      members_can_create_teams: o.org.members_can_create_teams ?? true,
      ...o.teamDefaults,
    });
  }),

  http.get("/organizations", async ({ query, response }) => {
    const fault = await gate("orgs-list");
    if (fault) return response.untyped(fault);
    const since = Number(query.get("since")) || 0;
    const perPage = Math.min(100, Number(query.get("per_page")) || 30);
    const list = orgDb.deleted ? [] : [orgSimple()];
    return response(200).json(list.filter((o) => o.id > since).slice(0, perPage));
  }),

  // ---- Repositories ----

  http.get("/orgs/{org}/repos", async ({ params, query, response }) => {
    const fault = await gate("orgs-repos");
    if (fault) return response.untyped(fault);
    if (!findOrg(params.org)) return response.untyped(notFound());
    const type = query.get("type") ?? "all";
    const mine = reachable(viewer);
    const list = orgRepos().filter((r) =>
      type === "public"
        ? r.visibility === "public"
        : type === "private"
          ? r.private
          : type === "internal"
            ? r.visibility === "internal"
            : type === "forks"
              ? r.fork
              : type === "sources"
                ? !r.fork
                : type === "member"
                  ? mine.has(r.full_name.toLowerCase())
                  : true,
    );
    const sorted = sortRepos(list, query.get("sort"), query.get("direction"));
    return response(200).json(
      page(sorted, query).map((r) => minimalRepo(r, mine.get(r.full_name.toLowerCase()))),
    );
  }),

  // ---- Members ----

  http.get("/orgs/{org}/members", async ({ params, query, response }) => {
    const fault = await gate("orgs-members");
    if (fault) return response.untyped(fault);
    if (!findOrg(params.org)) return response.untyped(notFound());
    const filter = query.get("filter") ?? "all";
    const role = query.get("role") ?? "all";
    const list = [...orgDb.members]
      .filter(([, m]) => (role === "all" ? true : m.role === role))
      .filter(([, m]) => (filter === "2fa_disabled" ? !m.twoFactor : true))
      .flatMap(([login]) => findUser(login) ?? [])
      .sort((a, b) => a.login.localeCompare(b.login));
    return response(200).json(page(list, query));
  }),

  http.get("/orgs/{org}/members/{username}", async ({ params, response }) => {
    const fault = await gate("orgs-members");
    if (fault) return response.untyped(fault);
    if (!findOrg(params.org)) return response.untyped(notFound());
    return orgDb.members.has(params.username.toLowerCase())
      ? response(204).empty()
      : response.untyped(notFound());
  }),

  http.delete("/orgs/{org}/members/{username}", async ({ params, response }) => {
    const fault = await gate("orgs-member-remove");
    if (fault) return response.untyped(fault);
    if (!findOrg(params.org)) return response.untyped(notFound());
    const login = params.username.toLowerCase();
    const m = orgDb.members.get(login);
    if (!m) return response.untyped(notFound());
    if (m.role === "admin" && ownerCount() === 1) {
      return response.untyped(unprocessable("Membership", "role", "last_owner"));
    }
    orgDb.members.delete(login);
    orgDb.publicMembers.delete(login);
    dropFromTeams(login);
    log("org.remove_member", viewer, { user: login });
    return response(204).empty();
  }),

  http.get("/orgs/{org}/memberships/{username}", async ({ params, response }) => {
    const fault = await gate("orgs-membership");
    if (fault) return response.untyped(fault);
    if (!findOrg(params.org)) return response.untyped(notFound());
    const m = membership(params.username.toLowerCase());
    return m ? response(200).json(m) : response.untyped(notFound());
  }),

  http.put("/orgs/{org}/memberships/{username}", async ({ params, request, response }) => {
    const fault = await gate("orgs-member-role");
    if (fault) return response.untyped(fault);
    if (!findOrg(params.org)) return response.untyped(notFound());
    const login = params.username.toLowerCase();
    const u = findUser(login);
    if (!u) return response.untyped(notFound());
    const role = (await request.json())?.role ?? "member";
    const m = orgDb.members.get(login);
    if (m) {
      if (m.role === "admin" && role === "member" && ownerCount() === 1) {
        return response.untyped(unprocessable("Membership", "role", "last_owner"));
      }
      m.role = role;
      log("org.update_member", viewer, { user: login, data: { role } });
      return response(200).json(membership(login)!);
    }
    // Not a member yet: GHES sends an invitation and answers a pending membership.
    if (!orgDb.invitations.some((i) => i.login === login)) {
      orgDb.invitations.unshift(
        invitation(
          { login, email: null },
          role === "admin" ? "admin" : "direct_member",
          [],
          users.mira,
          new Date(),
        ),
      );
      log("org.invite_member", viewer, { user: login });
    }
    return response(200).json({ ...membership(viewer)!, state: "pending", role, user: u });
  }),

  http.delete("/orgs/{org}/memberships/{username}", async ({ params, response }) => {
    const fault = await gate("orgs-member-remove");
    if (fault) return response.untyped(fault);
    if (!findOrg(params.org)) return response.untyped(notFound());
    const login = params.username.toLowerCase();
    const pending = orgDb.invitations.findIndex((i) => i.login === login);
    if (pending >= 0) {
      orgDb.invitations.splice(pending, 1);
      return response(204).empty();
    }
    const m = orgDb.members.get(login);
    if (!m) return response.untyped(notFound());
    if (m.role === "admin" && ownerCount() === 1) {
      return response.untyped(unprocessable("Membership", "role", "last_owner"));
    }
    orgDb.members.delete(login);
    dropFromTeams(login);
    log("org.remove_member", viewer, { user: login });
    return response(204).empty();
  }),

  http.get("/orgs/{org}/public_members", async ({ params, query, response }) => {
    const fault = await gate("orgs-public-members");
    if (fault) return response.untyped(fault);
    if (!findOrg(params.org)) return response.untyped(notFound());
    const list = [...orgDb.publicMembers]
      .filter((l) => orgDb.members.has(l))
      .flatMap((l) => findUser(l) ?? [])
      .sort((a, b) => a.login.localeCompare(b.login));
    return response(200).json(page(list, query));
  }),

  http.get("/orgs/{org}/public_members/{username}", async ({ params, response }) => {
    const fault = await gate("orgs-public-members");
    if (fault) return response.untyped(fault);
    return findOrg(params.org) && orgDb.publicMembers.has(params.username.toLowerCase())
      ? response(204).empty()
      : response.untyped(notFound());
  }),

  // GHES lets only the member themself publicize or conceal their own membership.
  http.put("/orgs/{org}/public_members/{username}", async ({ params, response }) => {
    const fault = await gate("orgs-publicize");
    if (fault) return response.untyped(fault);
    const login = params.username.toLowerCase();
    if (!findOrg(params.org) || !orgDb.members.has(login)) return response.untyped(notFound());
    if (login !== viewer) {
      return response.untyped(
        HttpResponse.json(
          {
            message: "You can only publicize your own membership.",
            documentation_url: DOCS,
            status: "403",
          },
          { status: 403 },
        ),
      );
    }
    orgDb.publicMembers.add(login);
    return response(204).empty();
  }),

  http.delete("/orgs/{org}/public_members/{username}", async ({ params, response }) => {
    const fault = await gate("orgs-publicize");
    if (fault) return response.untyped(fault);
    const login = params.username.toLowerCase();
    if (!findOrg(params.org) || login !== viewer) return response.untyped(notFound());
    orgDb.publicMembers.delete(login);
    return response(204).empty();
  }),

  // ---- Invitations ----

  ext.get("/orgs/{org}/invitations", async ({ params, query, response }) => {
    const fault = await gate("orgs-invitations");
    if (fault) return response.untyped(fault);
    if (!findOrg(params.org)) return response.untyped(notFound());
    return response(200).json(page(orgDb.invitations, query));
  }),

  ext.post("/orgs/{org}/invitations", async ({ params, request, response }) => {
    const fault = await gate("orgs-invite");
    if (fault) return response.untyped(fault);
    if (!findOrg(params.org)) return response.untyped(notFound());
    const body = await request.json();
    const role = body.role ?? "direct_member";
    const teamIds = body.team_ids ?? [];
    if (teamIds.some((id) => !teamById(id))) {
      return response.untyped(
        invalid(
          "OrganizationInvitation",
          "team_ids",
          "invalid",
          "One of the teams does not exist.",
        ),
      );
    }
    let target: { login: string | null; email: string | null };
    if (body.invitee_id !== undefined) {
      const u = allUsers().find((x) => x.id === body.invitee_id);
      if (!u)
        return response.untyped(
          invalid("OrganizationInvitation", "invitee_id", "invalid", "No user has that id."),
        );
      if (orgDb.members.has(u.login)) {
        return response.untyped(
          unprocessable("OrganizationInvitation", "invitee_id", "already_member"),
        );
      }
      if (orgDb.invitations.some((i) => i.login === u.login)) {
        return response.untyped(
          unprocessable("OrganizationInvitation", "invitee_id", "already_invited"),
        );
      }
      target = { login: u.login, email: null };
    } else {
      const email = (body.email ?? "").trim().toLowerCase();
      if (!email)
        return response.untyped(unprocessable("OrganizationInvitation", "email", "missing_field"));
      if (!EMAIL_RE.test(email)) {
        return response.untyped(
          invalid("OrganizationInvitation", "email", "invalid", "Not an email address."),
        );
      }
      if (/@example\.(com|org)$/.test(email)) {
        return response.untyped(
          unprocessable("OrganizationInvitation", "email", "reserved_domain"),
        );
      }
      if (orgDb.invitations.some((i) => i.email === email)) {
        return response.untyped(
          unprocessable("OrganizationInvitation", "email", "already_invited"),
        );
      }
      target = { login: null, email };
    }
    const inv = invitation(target, role, teamIds, users.mira, new Date());
    orgDb.invitations.unshift(inv);
    orgDb.invitationTeams.set(inv.id, teamIds);
    log("org.invite_member", viewer, { user: target.login ?? target.email ?? undefined });
    return response(201).json(inv);
  }),

  ext.delete("/orgs/{org}/invitations/{invitation_id}", async ({ params, response }) => {
    const fault = await gate("orgs-invite-cancel");
    if (fault) return response.untyped(fault);
    if (!findOrg(params.org)) return response.untyped(notFound());
    const i = orgDb.invitations.findIndex((x) => x.id === Number(params.invitation_id));
    if (i < 0) return response.untyped(notFound());
    const [gone] = orgDb.invitations.splice(i, 1);
    orgDb.invitationTeams.delete(gone.id);
    log("org.cancel_invitation", viewer, { user: gone.login ?? gone.email ?? undefined });
    return response(204).empty();
  }),

  // ---- Outside collaborators ----

  http.get("/orgs/{org}/outside_collaborators", async ({ params, query, response }) => {
    const fault = await gate("orgs-outside");
    if (fault) return response.untyped(fault);
    if (!findOrg(params.org)) return response.untyped(notFound());
    const list = [...orgDb.outside.keys()]
      .flatMap((l) => findUser(l) ?? [])
      .sort((a, b) => a.login.localeCompare(b.login));
    return response(200).json(page(list, query));
  }),

  http.put("/orgs/{org}/outside_collaborators/{username}", async ({ params, response }) => {
    const fault = await gate("orgs-outside-convert");
    if (fault) return response.untyped(fault);
    if (!findOrg(params.org)) return response.untyped(notFound());
    const login = params.username.toLowerCase();
    const m = orgDb.members.get(login);
    if (!m) return response.untyped(notFound());
    if (m.role === "admin") {
      return response.untyped(
        HttpResponse.json(
          {
            message: "An owner cannot become an outside collaborator. Make them a member first.",
            documentation_url: DOCS,
            status: "403",
          },
          { status: 403 },
        ),
      );
    }
    // They keep access to the repositories they reached through teams, as collaborators.
    const repos = [...reachable(login).keys()];
    orgDb.members.delete(login);
    orgDb.publicMembers.delete(login);
    dropFromTeams(login);
    orgDb.outside.set(login, repos);
    log("org.convert_member_to_outside_collaborator", viewer, { user: login });
    return response(204).empty();
  }),

  http.delete("/orgs/{org}/outside_collaborators/{username}", async ({ params, response }) => {
    const fault = await gate("orgs-outside-remove");
    if (fault) return response.untyped(fault);
    if (!findOrg(params.org)) return response.untyped(notFound());
    const login = params.username.toLowerCase();
    if (!orgDb.outside.delete(login)) return response.untyped(notFound());
    log("org.remove_outside_collaborator", viewer, { user: login });
    return response(204).empty();
  }),

  // ---- Audit log ----

  http.get("/orgs/{org}/audit-log", async ({ params, query, response }) => {
    const fault = await gate("orgs-audit");
    if (fault) return response.untyped(fault);
    if (!findOrg(params.org)) return response.untyped(notFound());
    const hits = searchAudit(
      query.get("phrase") ?? "",
      query.get("order") === "asc" ? "asc" : "desc",
      query.get("include") ?? "web",
    );
    return response(200).json(page(hits, query));
  }),
];
