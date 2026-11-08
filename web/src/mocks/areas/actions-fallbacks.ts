import { gate, http, notFound } from "../http";
import { org, repositories } from "../world";

// The organization's repository list, which the orgs area owns. Delete this file once
// its handler lands.
export const handlers = [
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
