import { gate, http, notFound } from "../http";
import { me, users } from "../world";

// Reads another area owns that the agents pages need before that area's handler lands.
// The merge deletes this file once the owner's handler is in.
export const handlers = [
  // MSW matches /user/repos and /user/orgs here too; only a numeric id is this read.
  http.get("/user/{account_id}", async ({ params, response }) => {
    if (!/^\d+$/.test(params.account_id)) return undefined;
    const fault = await gate("user-by-id");
    if (fault) return response.untyped(fault);
    const u = Object.values(users).find((x) => x.id === Number(params.account_id));
    if (!u) return response.untyped(notFound());
    return response(200).json({
      ...u,
      user_view_type: "public",
      gravatar_id: u.gravatar_id ?? null,
      company: null,
      blog: null,
      location: null,
      email: null,
      hireable: null,
      bio: null,
      public_repos: 1,
      public_gists: 0,
      followers: 0,
      following: 0,
      created_at: me.created_at,
      updated_at: me.updated_at,
      name: u.name ?? null,
    });
  }),
];
