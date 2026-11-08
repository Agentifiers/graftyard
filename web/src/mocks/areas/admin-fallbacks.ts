import { gate, http, notFound } from "../http";
import { db as core } from "./core-db";
import { repoHooks } from "./admin-db";
import { installedVersion } from "./admin-ops-db";

// Reads other areas own, served here until their handlers land. The merge deletes this file.
export const handlers = [
  // Owned by activity.
  http.get("/meta", async ({ response }) => {
    const fault = await gate("meta");
    if (fault) return response.untyped(fault);
    return response(200).json({
      verifiable_password_authentication: false,
      installed_version: installedVersion(),
    });
  }),

  // Owned by repo-admin: a repository's view of the instance's pre-receive hooks.
  http.get("/repos/{owner}/{repo}/pre-receive-hooks", async ({ params, response }) => {
    const fault = await gate("admin-repo-pre-receive");
    if (fault) return response.untyped(fault);
    const repo = core.repo(params.owner, params.repo);
    return repo ? response(200).json(repoHooks()) : response.untyped(notFound());
  }),
];
