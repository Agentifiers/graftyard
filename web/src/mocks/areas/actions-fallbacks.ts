import { gate, http, notFound } from "../http";
import { API, HOST, branchSha, branches, org, repositories, sha } from "../world";
import { environments } from "./actions-db";

// Reads the Actions pages make that other areas own: environments and their secrets
// (releases), branches (code), and the organization's repositories (orgs). Minimal and typed; the merge deletes this file once the
// owners' handlers land, and the pages read the owners' fuller data the same way.

const full = (p: { owner: string; repo: string }) => `${p.owner}/${p.repo}`.toLowerCase();
const day = (n: number) =>
  new Date(Date.parse("2026-11-08T10:00:00Z") - n * 86_400_000).toISOString();

export const handlers = [
  http.get("/repos/{owner}/{repo}/environments", async ({ params, response }) => {
    const fault = await gate("environments");
    if (fault) return response.untyped(fault);
    if (!(full(params) in branches)) return response.untyped(notFound());
    const list = environments(full(params)).map((e) => ({
      id: e.id,
      node_id: `EN_${sha(`env:${e.id}`).slice(0, 12)}`,
      name: e.name,
      url: `${API}/repos/${full(params)}/environments/${e.name}`,
      html_url: `${HOST}/${full(params)}/deployments/activity_log?environments_filter=${e.name}`,
      created_at: "2025-04-11T12:00:00Z",
      updated_at: "2026-10-01T00:00:00Z",
    }));
    return response(200).json({ total_count: list.length, environments: list });
  }),

  http.get(
    "/repos/{owner}/{repo}/environments/{environment_name}/secrets",
    async ({ params, response }) => {
      const fault = await gate("environments");
      if (fault) return response.untyped(fault);
      const env = environments(full(params)).find((e) => e.name === params.environment_name);
      if (!env) return response.untyped(notFound());
      const names =
        env.name === "production"
          ? ["CLOUDFLARE_API_TOKEN", "RELEASE_SIGNING_KEY"]
          : ["CLOUDFLARE_API_TOKEN"];
      const secrets = names.map((name, i) => ({
        name,
        created_at: day(60 + i),
        updated_at: day(8 + i),
      }));
      return response(200).json({ total_count: secrets.length, secrets });
    },
  ),

  http.get("/repos/{owner}/{repo}/branches", async ({ params, response }) => {
    const fault = await gate("branches");
    if (fault) return response.untyped(fault);
    const names = branches[full(params)];
    if (!names) return response.untyped(notFound());
    return response(200).json(
      names.map((name) => ({
        name,
        commit: {
          sha: branchSha(full(params), name),
          url: `${API}/repos/${full(params)}/commits/${branchSha(full(params), name)}`,
        },
        protected: name === "main",
      })),
    );
  }),

  http.get("/orgs/{org}/repos", async ({ params, response }) => {
    const fault = await gate("org-repos");
    if (fault) return response.untyped(fault);
    if (params.org.toLowerCase() !== org.login) return response.untyped(notFound());
    return response(200).json(
      repositories
        .filter((r) => r.owner.login === org.login)
        .map(({ license, ...r }) => ({
          ...r,
          license: license && {
            key: license.key,
            name: license.name,
            url: license.url,
            node_id: license.node_id,
            spdx_id: license.spdx_id ?? undefined,
          },
        })),
    );
  }),
];
