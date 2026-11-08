import { byUpdatedDesc, gate, http, notFound, page } from "../http";
import { db } from "./core-db";

// Issues and their comments. The issues list includes pull requests, as GHES's does.
export const handlers = [
  http.get("/repos/{owner}/{repo}/issues", async ({ params, query, response }) => {
    const fault = await gate("issues");
    if (fault) return response.untyped(fault);
    const state = db.state(params.owner, params.repo);
    if (!state) return response.untyped(notFound());
    const wanted = query.get("state") ?? "open";
    const items = state.issues
      .filter((i) => wanted === "all" || i.state === wanted)
      .sort(byUpdatedDesc);
    return response(200).json(page(items, query));
  }),

  http.post("/repos/{owner}/{repo}/issues", async ({ params, request, response }) => {
    const state = db.state(params.owner, params.repo);
    if (!state) return response.untyped(notFound());
    const body = await request.json();
    const labels = (body.labels ?? []).map((l) => (typeof l === "string" ? l : (l.name ?? "")));
    return response(201).json(
      db.createIssue(state, { title: String(body.title), body: body.body, labels }),
    );
  }),

  http.get("/repos/{owner}/{repo}/issues/{issue_number}", async ({ params, response }) => {
    const fault = await gate("issue");
    if (fault) return response.untyped(fault);
    const issue = db
      .state(params.owner, params.repo)
      ?.issues.find((i) => i.number === Number(params.issue_number));
    return issue ? response(200).json(issue) : response.untyped(notFound());
  }),

  http.get(
    "/repos/{owner}/{repo}/issues/{issue_number}/comments",
    async ({ params, query, response }) => {
      const fault = await gate("comments");
      if (fault) return response.untyped(fault);
      const state = db.state(params.owner, params.repo);
      if (!state) return response.untyped(notFound());
      return response(200).json(page(state.comments.get(Number(params.issue_number)) ?? [], query));
    },
  ),

  http.post(
    "/repos/{owner}/{repo}/issues/{issue_number}/comments",
    async ({ params, request, response }) => {
      const fault = await gate("post-comment");
      if (fault) return response.untyped(fault);
      const state = db.state(params.owner, params.repo);
      const number = Number(params.issue_number);
      if (!state?.issues.some((i) => i.number === number)) return response.untyped(notFound());
      const { body } = await request.json();
      if (!body.trim()) {
        return response(422).json({
          message: "Validation Failed",
          documentation_url:
            "https://docs.github.com/enterprise-server@3.22/rest/issues/comments#create-an-issue-comment",
          errors: [{ resource: "IssueComment", field: "body", code: "missing_field" }],
        });
      }
      return response(201).json(db.createComment(state, number, body));
    },
  ),
];
