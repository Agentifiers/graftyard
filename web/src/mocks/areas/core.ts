import { gate, http, notFound, page } from "../http";
import { db } from "./core-db";

// The viewer and repositories. Issues and pull requests live in issues.ts and pulls.ts;
// all three read and write the one store in core-db.ts.
export const handlers = [
  http.get("/user", async ({ response }) => {
    const fault = await gate("user");
    if (fault) return response.untyped(fault);
    return response(200).json(db.me);
  }),

  http.get("/user/repos", async ({ query, response }) => {
    const fault = await gate("repos");
    if (fault) return response.untyped(fault);
    const repos = db.repos().sort((a, b) => a.full_name.localeCompare(b.full_name));
    return response(200).json(page(repos, query));
  }),

  http.get("/repos/{owner}/{repo}", async ({ params, response }) => {
    const fault = await gate("repo");
    if (fault) return response.untyped(fault);
    const repo = db.repo(params.owner, params.repo);
    return repo ? response(200).json(repo) : response.untyped(notFound());
  }),
];
