import { gate, http } from "../http";
import { people } from "./repo-admin-db";

// The users list, which the orgs area owns. Delete this file once its handler lands.
export const handlers = [
  http.get("/users", async ({ query, response }) => {
    const fault = await gate("users");
    if (fault) return response.untyped(fault);
    const since = Number(query.get("since")) || 0;
    const perPage = Math.min(100, Number(query.get("per_page")) || 30);
    return response(200).json(people.filter((p) => p.id > since).slice(0, perPage));
  }),
];
