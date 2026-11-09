import { gate, http, notFound } from "../http";
import { org } from "../world";

// What the shared layouts read: the organization header and the viewer's organizations.
export const handlers = [
  http.get("/orgs/{org}", async ({ params, response }) => {
    const fault = await gate("org");
    if (fault) return response.untyped(fault);
    if (params.org.toLowerCase() !== org.login) return response.untyped(notFound());
    return response(200).json(org);
  }),

  http.get("/user/orgs", async ({ response }) => {
    const fault = await gate("orgs");
    if (fault) return response.untyped(fault);
    const { login, id, node_id, url, repos_url, events_url, hooks_url, issues_url } = org;
    const { members_url, public_members_url, avatar_url, description } = org;
    return response(200).json([
      {
        login,
        id,
        node_id,
        url,
        repos_url,
        events_url,
        hooks_url,
        issues_url,
        members_url,
        public_members_url,
        avatar_url,
        description,
      },
    ]);
  }),
];
