import { branchSha } from "../world";
import { gate, http, notFound, page } from "../http";
import { admin, people } from "./repo-admin-db";

// Minimal reads other areas own, until their handlers land: the branches list (code) and
// the users list (orgs). The merge deletes this file.
export const handlers = [
  http.get("/repos/{owner}/{repo}/branches", async ({ params, query, response }) => {
    const fault = await gate("code-branches");
    if (fault) return response.untyped(fault);
    const found = admin.state(params.owner, params.repo);
    if (!found) return response.untyped(notFound());
    const { repo, admin: rec } = found;
    const list = rec.branches.map((name) => ({
      name,
      commit: {
        sha: branchSha(repo.full_name, name),
        url: `${repo.url}/commits/${branchSha(repo.full_name, name)}`,
      },
      protected: Boolean(admin.ruleFor(rec, name)),
    }));
    return response(200).json(page(list, query));
  }),

  http.get("/users", async ({ query, response }) => {
    const fault = await gate("users");
    if (fault) return response.untyped(fault);
    const since = Number(query.get("since")) || 0;
    const perPage = Math.min(100, Number(query.get("per_page")) || 30);
    return response(200).json(people.filter((p) => p.id > since).slice(0, perPage));
  }),
];
