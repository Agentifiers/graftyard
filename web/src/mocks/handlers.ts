import { HttpResponse, delay } from "msw";
import { createOpenApiHttp } from "openapi-msw";
import type { paths } from "@/api/ghes";
import { db } from "./db";
import { failing, serviceUnavailable, type FaultRegion } from "./faults";

const http = createOpenApiHttp<paths>({ baseUrl: "/api/v3" });

const notFound = () =>
  HttpResponse.json(
    {
      message: "Not Found",
      documentation_url: "https://docs.github.com/enterprise-server@3.22/rest",
      status: "404",
    },
    { status: 404 },
  );

// A little latency so skeletons show, then the region's fault if one is set.
async function gate(region: FaultRegion) {
  await delay(150 + Math.random() * 250);
  return failing(region) ? HttpResponse.json(serviceUnavailable(), { status: 503 }) : undefined;
}

function page<T>(items: T[], query: { get(name: "per_page" | "page"): string | null }) {
  const perPage = Math.min(100, Number(query.get("per_page")) || 30);
  const pageNo = Math.max(1, Number(query.get("page")) || 1);
  return items.slice((pageNo - 1) * perPage, pageNo * perPage);
}

const byUpdatedDesc = (a: { updated_at: string }, b: { updated_at: string }) =>
  b.updated_at.localeCompare(a.updated_at);

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
