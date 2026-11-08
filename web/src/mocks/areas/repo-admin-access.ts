import { HttpResponse } from "msw";
import { roleOf, ROLES, type Role } from "@/areas/repo-admin/domain";
import { gate, http, notFound, page, unprocessable } from "../http";
import { orgMembers } from "../world";
import { admin, collaborator, findPerson, invitation, teamFor } from "./repo-admin-db";
import { invalid } from "./repo-admin";

// Who can reach a repository: collaborators, pending invitations, and teams.

export const handlers = [
  http.get("/repos/{owner}/{repo}/collaborators", async ({ params, query, response }) => {
    const fault = await gate("admin-repo-collaborators");
    if (fault) return response.untyped(fault);
    const found = admin.state(params.owner, params.repo);
    if (!found) return response.untyped(notFound());
    const { repo, admin: rec } = found;
    const affiliation = query.get("affiliation") ?? "all";
    const owner = repo.owner.type === "User" ? [{ user: repo.owner, role: "admin" as Role }] : [];
    const direct = [...owner, ...rec.collaborators];
    const outside = direct.filter(
      (g) =>
        repo.owner.type === "Organization" && !orgMembers.some((m) => m.login === g.user.login),
    );
    const list = affiliation === "outside" ? outside : direct;
    return response(200).json(page(list.map(collaborator), query));
  }),

  http.get("/repos/{owner}/{repo}/collaborators/{username}", async ({ params, response }) => {
    const fault = await gate("admin-repo-collaborators");
    if (fault) return response.untyped(fault);
    const rec = admin.state(params.owner, params.repo)?.admin;
    const is = rec?.collaborators.some((g) => g.user.login === params.username);
    return is ? response(204).empty() : response.untyped(notFound());
  }),

  http.get(
    "/repos/{owner}/{repo}/collaborators/{username}/permission",
    async ({ params, response }) => {
      const fault = await gate("admin-repo-collaborators");
      if (fault) return response.untyped(fault);
      const rec = admin.state(params.owner, params.repo)?.admin;
      const g = rec?.collaborators.find((c) => c.user.login === params.username);
      if (!g) return response.untyped(notFound());
      return response(200).json({
        permission: g.role === "triage" ? "read" : g.role === "maintain" ? "write" : g.role,
        role_name: g.role,
        user: collaborator(g),
      });
    },
  ),

  // Adds an invitation, or changes the role of someone already on the repository.
  http.put(
    "/repos/{owner}/{repo}/collaborators/{username}",
    async ({ params, request, response }) => {
      const body = await request.json().catch(() => null);
      const existing = admin
        .state(params.owner, params.repo)
        ?.admin.collaborators.find((c) => c.user.login === params.username);
      const fault = await gate(existing ? "admin-repo-collaborator-role" : "admin-repo-invite");
      if (fault) return response.untyped(fault);
      const found = admin.state(params.owner, params.repo);
      if (!found) return response.untyped(notFound());
      const { repo, admin: rec } = found;
      const role = roleOf(body?.permission ?? "push");
      if (!role) {
        return response.untyped(
          invalid(
            "Repository",
            "permission",
            "invalid",
            `permission must be one of ${ROLES.join(", ")}`,
          ),
        );
      }
      const person = findPerson(params.username);
      if (!person) {
        return response.untyped(
          invalid("Repository", "username", "invalid", `${params.username} is not a user`),
        );
      }
      if (person.login === repo.owner.login) {
        return response.untyped(
          invalid("Repository", "username", "invalid", "The repository owner is already an admin"),
        );
      }
      if (existing) {
        existing.role = role;
        return response(204).empty();
      }
      const pending = rec.invitations.find((i) => i.invitee?.login === person.login);
      if (pending) {
        return response.untyped(
          invalid(
            "RepositoryInvitation",
            "invitee",
            "already_exists",
            `${person.login} already has a pending invitation`,
          ),
        );
      }
      const created = invitation(repo, person, role, new Date().toISOString());
      rec.invitations.push(created);
      // GHES answers a new invitation with 201 and its body; the 3.22 description lists
      // only the 204 for an existing collaborator, so this one response is untyped.
      return response.untyped(HttpResponse.json(created, { status: 201 }));
    },
  ),

  http.delete("/repos/{owner}/{repo}/collaborators/{username}", async ({ params, response }) => {
    const fault = await gate("admin-repo-collaborator-remove");
    if (fault) return response.untyped(fault);
    const rec = admin.state(params.owner, params.repo)?.admin;
    if (!rec) return response.untyped(notFound());
    const before = rec.collaborators.length;
    rec.collaborators = rec.collaborators.filter((c) => c.user.login !== params.username);
    if (rec.collaborators.length === before) {
      return response.untyped(unprocessable("Collaborator", "username", "missing"));
    }
    return response(204).empty();
  }),

  http.get("/repos/{owner}/{repo}/invitations", async ({ params, query, response }) => {
    const fault = await gate("admin-repo-invitations");
    if (fault) return response.untyped(fault);
    const rec = admin.state(params.owner, params.repo)?.admin;
    if (!rec) return response.untyped(notFound());
    return response(200).json(page(rec.invitations, query));
  }),

  http.patch(
    "/repos/{owner}/{repo}/invitations/{invitation_id}",
    async ({ params, request, response }) => {
      const fault = await gate("admin-repo-invitation-update");
      if (fault) return response.untyped(fault);
      const rec = admin.state(params.owner, params.repo)?.admin;
      const inv = rec?.invitations.find((i) => i.id === Number(params.invitation_id));
      if (!inv) return response.untyped(notFound());
      const { permissions } = (await request.json()) ?? {};
      if (permissions) inv.permissions = permissions;
      return response(200).json(inv);
    },
  ),

  http.delete("/repos/{owner}/{repo}/invitations/{invitation_id}", async ({ params, response }) => {
    const fault = await gate("admin-repo-invitation-cancel");
    if (fault) return response.untyped(fault);
    const rec = admin.state(params.owner, params.repo)?.admin;
    if (rec) rec.invitations = rec.invitations.filter((i) => i.id !== Number(params.invitation_id));
    return response(204).empty();
  }),

  http.get("/repos/{owner}/{repo}/teams", async ({ params, query, response }) => {
    const fault = await gate("admin-repo-teams");
    if (fault) return response.untyped(fault);
    const rec = admin.state(params.owner, params.repo)?.admin;
    if (!rec) return response.untyped(notFound());
    return response(200).json(
      page(
        rec.teams.map((t) => teamFor(t.slug, t.role)),
        query,
      ),
    );
  }),
];
