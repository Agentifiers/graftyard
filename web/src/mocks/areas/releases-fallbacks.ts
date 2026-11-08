import type { Schemas } from "@/api/client";
import { gate, http, notFound, page } from "../http";
import { API, branchSha, branches, findRepo, teams, users } from "../world";
import { releasesDb } from "./releases-db";

// Minimal reads other areas own (code: branches and tags; repo-admin: collaborators and
// teams). The merge deletes this file once their handlers land.

export const handlers = [
  http.get("/repos/{owner}/{repo}/branches", async ({ params, query, response }) => {
    const fault = await gate("code-branches");
    if (fault) return response.untyped(fault);
    const repo = findRepo(params.owner, params.repo);
    if (!repo) return response.untyped(notFound());
    const list: Schemas["short-branch"][] = (branches[repo.full_name] ?? []).map((name) => ({
      name,
      commit: {
        sha: branchSha(repo.full_name, name),
        url: `${repo.url}/commits/${branchSha(repo.full_name, name)}`,
      },
      protected: name === "main" || name.startsWith("release/"),
    }));
    return response(200).json(page(list, query), { headers: { Link: "" } });
  }),

  http.get("/repos/{owner}/{repo}/tags", async ({ params, query, response }) => {
    const fault = await gate("code-tags");
    if (fault) return response.untyped(fault);
    const s = releasesDb.state(params.owner, params.repo);
    if (!s) return response.untyped(notFound());
    const list: Schemas["tag"][] = [...releasesDb.tags(s)].reverse().map((name) => ({
      name,
      commit: {
        sha: releasesDb.tagSha(s, name),
        url: `${s.repo.url}/commits/${releasesDb.tagSha(s, name)}`,
      },
      zipball_url: `${s.repo.url}/zipball/refs/tags/${name}`,
      tarball_url: `${s.repo.url}/tarball/refs/tags/${name}`,
      node_id: `REF_${releasesDb.tagSha(s, name).slice(0, 12)}`,
    }));
    return response(200).json(page(list, query), { headers: { Link: "" } });
  }),

  http.get("/repos/{owner}/{repo}/collaborators", async ({ params, query, response }) => {
    const fault = await gate("admin-repo-collaborators");
    if (fault) return response.untyped(fault);
    if (!findRepo(params.owner, params.repo)) return response.untyped(notFound());
    const list: Schemas["collaborator"][] = [users.mira, users.tomas, users.lin, users.devi].map(
      (u) => ({
        ...u,
        permissions: {
          pull: true,
          triage: true,
          push: true,
          maintain: u.login === "mira" || u.login === "devi",
          admin: u.login === "mira" || u.login === "devi",
        },
        role_name: u.login === "mira" || u.login === "devi" ? "admin" : "write",
      }),
    );
    return response(200).json(page(list, query), { headers: { Link: "" } });
  }),

  http.get("/repos/{owner}/{repo}/teams", async ({ params, query, response }) => {
    const fault = await gate("admin-repo-teams");
    if (fault) return response.untyped(fault);
    if (!findRepo(params.owner, params.repo)) return response.untyped(notFound());
    return response(200).json(page(Object.values(teams), query), {
      headers: { Link: `<${API}/repos/${params.owner}/${params.repo}/teams?page=1>; rel="first"` },
    });
  }),
];
