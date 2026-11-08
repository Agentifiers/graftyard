import { byUpdatedDesc, gate, http, notFound, page } from "../http";
import { db } from "./core-db";

// Pull requests, their files, and the check runs on their heads.
export const handlers = [
  http.get("/repos/{owner}/{repo}/pulls", async ({ params, query, response }) => {
    const fault = await gate("pulls");
    if (fault) return response.untyped(fault);
    const state = db.state(params.owner, params.repo);
    if (!state) return response.untyped(notFound());
    const wanted = query.get("state") ?? "open";
    const items = state.pulls
      .filter((p) => wanted === "all" || p.state === wanted)
      .sort(byUpdatedDesc)
      .map(db.simplePull);
    return response(200).json(page(items, query));
  }),

  http.get("/repos/{owner}/{repo}/pulls/{pull_number}", async ({ params, response }) => {
    const fault = await gate("pull");
    if (fault) return response.untyped(fault);
    const pr = db
      .state(params.owner, params.repo)
      ?.pulls.find((p) => p.number === Number(params.pull_number));
    return pr ? response(200).json(pr) : response.untyped(notFound());
  }),

  http.get(
    "/repos/{owner}/{repo}/pulls/{pull_number}/files",
    async ({ params, query, response }) => {
      const fault = await gate("files");
      if (fault) return response.untyped(fault);
      const files = db.state(params.owner, params.repo)?.files.get(Number(params.pull_number));
      return files ? response(200).json(page(files, query)) : response.untyped(notFound());
    },
  ),

  http.get("/repos/{owner}/{repo}/commits/{ref}/check-runs", async ({ params, response }) => {
    const fault = await gate("checks");
    if (fault) return response.untyped(fault);
    if (!db.state(params.owner, params.repo)) return response.untyped(notFound());
    const runs = db.checkRuns(params.ref);
    return response(200).json({ total_count: runs.length, check_runs: runs });
  }),
];
