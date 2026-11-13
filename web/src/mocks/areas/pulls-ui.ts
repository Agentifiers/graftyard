import { createOpenApiHttp } from "openapi-msw";
import { lineAt, parsePatch } from "@/areas/pulls/patch";
import { UI_BASE, type UiPaths } from "@/areas/pulls/ui-api";
import { gate, notFound } from "../http";
import { db } from "./core-db";
import {
  addEvent,
  afterReview,
  commitsOf,
  eventBase,
  extra,
  makeComment,
  requirementsView,
  summary,
  sync,
  touch,
  viewer,
} from "./pulls-db";

// The private /_t/ui routes behind the pull request pages (see web/src/areas/pulls/ui-api.ts).

const ui = createOpenApiHttp<UiPaths>({ baseUrl: UI_BASE });

const invalid = (field: string, message: string) => ({
  message: "Validation Failed",
  documentation_url: "https://docs.github.com/enterprise-server@3.22/rest/pulls",
  errors: [{ resource: "PullRequest", field, code: "invalid", message }],
});

function find(params: { owner: string; repo: string; pull_number: string }) {
  const state = db.state(params.owner, params.repo);
  const pr = state?.pulls.find((p) => p.number === Number(params.pull_number));
  if (!state || !pr) return undefined;
  sync(state, pr);
  return { state, pr };
}

export const handlers = [
  ui.get("/repos/{owner}/{repo}/pulls/summary", async ({ params, response }) => {
    const fault = await gate("pulls-summary");
    if (fault) return response.untyped(fault);
    const state = db.state(params.owner, params.repo);
    if (!state) return response.untyped(notFound());
    return response(200).json(state.pulls.map((pr) => summary(state, pr)));
  }),

  // The edge serves the diff and the commits here, from the git cache, not REST's
  // paginated pulls/{n}/files and pulls/{n}/commits.
  ui.get("/repos/{owner}/{repo}/pulls/{pull_number}/files", async ({ params, response }) => {
    const fault = await gate("files");
    if (fault) return response.untyped(fault);
    const files = db.state(params.owner, params.repo)?.files.get(Number(params.pull_number));
    return files ? response(200).json(files) : response.untyped(notFound());
  }),

  ui.get("/repos/{owner}/{repo}/pulls/{pull_number}/commits", async ({ params, response }) => {
    const fault = await gate("pulls-commits");
    if (fault) return response.untyped(fault);
    const ctx = find(params);
    if (!ctx) return response.untyped(notFound());
    return response(200).json(commitsOf(ctx.state, ctx.pr));
  }),

  ui.get("/repos/{owner}/{repo}/pulls/{pull_number}/events", async ({ params, response }) => {
    const fault = await gate("pulls-events");
    if (fault) return response.untyped(fault);
    const ctx = find(params);
    if (!ctx) return response.untyped(notFound());
    return response(200).json(extra(ctx.state, ctx.pr.number).events);
  }),

  ui.get(
    "/repos/{owner}/{repo}/pulls/{pull_number}/merge-requirements",
    async ({ params, response }) => {
      const fault = await gate("pulls-requirements");
      if (fault) return response.untyped(fault);
      const ctx = find(params);
      if (!ctx) return response.untyped(notFound());
      return response(200).json(requirementsView(ctx.state, ctx.pr));
    },
  ),

  ui.get("/repos/{owner}/{repo}/pulls/{pull_number}/threads", async ({ params, response }) => {
    const fault = await gate("pulls-threads");
    if (fault) return response.untyped(fault);
    const ctx = find(params);
    if (!ctx) return response.untyped(notFound());
    return response(200).json([...extra(ctx.state, ctx.pr.number).threads.values()]);
  }),

  ui.put(
    "/repos/{owner}/{repo}/pulls/{pull_number}/threads/{comment_id}",
    async ({ params, response }) => {
      const fault = await gate("pulls-resolve");
      if (fault) return response.untyped(fault);
      const ctx = find(params);
      if (!ctx) return response.untyped(notFound());
      const e = extra(ctx.state, ctx.pr.number);
      const rootId = Number(params.comment_id);
      if (!e.comments.some((c) => c.id === rootId && !c.in_reply_to_id))
        return response.untyped(notFound());
      const t = { root_id: rootId, resolved: true, resolved_by: viewer };
      e.threads.set(rootId, t);
      return response(200).json(t);
    },
  ),

  ui.delete(
    "/repos/{owner}/{repo}/pulls/{pull_number}/threads/{comment_id}",
    async ({ params, response }) => {
      const fault = await gate("pulls-resolve");
      if (fault) return response.untyped(fault);
      const ctx = find(params);
      if (!ctx) return response.untyped(notFound());
      const e = extra(ctx.state, ctx.pr.number);
      const rootId = Number(params.comment_id);
      const t = { root_id: rootId, resolved: false, resolved_by: null };
      e.threads.set(rootId, t);
      return response(200).json(t);
    },
  ),

  ui.get("/repos/{owner}/{repo}/pulls/{pull_number}/viewed", async ({ params, response }) => {
    const fault = await gate("pulls-viewed");
    if (fault) return response.untyped(fault);
    const ctx = find(params);
    if (!ctx) return response.untyped(notFound());
    return response(200).json({
      paths: [...(extra(ctx.state, ctx.pr.number).viewed.get(viewer.login) ?? [])],
    });
  }),

  ui.put(
    "/repos/{owner}/{repo}/pulls/{pull_number}/viewed",
    async ({ params, request, response }) => {
      const fault = await gate("pulls-viewed-write");
      if (fault) return response.untyped(fault);
      const ctx = find(params);
      if (!ctx) return response.untyped(notFound());
      const { path, viewed } = await request.json();
      if (!ctx.state.files.get(ctx.pr.number)?.some((f) => f.filename === path))
        return response(422).json(invalid("path", `${path} is not part of the pull request`));
      const e = extra(ctx.state, ctx.pr.number);
      const set = e.viewed.get(viewer.login) ?? new Set<string>();
      if (viewed) set.add(path);
      else set.delete(path);
      e.viewed.set(viewer.login, set);
      return response(200).json({ paths: [...set] });
    },
  ),

  ui.put(
    "/repos/{owner}/{repo}/pulls/{pull_number}/draft",
    async ({ params, request, response }) => {
      const fault = await gate("pulls-draft");
      if (fault) return response.untyped(fault);
      const ctx = find(params);
      if (!ctx) return response.untyped(notFound());
      const { state, pr } = ctx;
      if (pr.state !== "open")
        return response(422).json(
          invalid("draft", "Only an open pull request can change its draft state."),
        );
      const { draft } = await request.json();
      if (draft === pr.draft) return response(200).json(pr);
      pr.draft = draft;
      const issue = state.issues.find((i) => i.number === pr.number);
      if (issue) issue.draft = draft;
      if (draft) pr.auto_merge = null;
      addEvent(state, pr.number, {
        ...eventBase(),
        event: draft ? "convert_to_draft" : "ready_for_review",
      });
      touch(state, pr);
      sync(state, pr);
      afterReview();
      return response(200).json(pr);
    },
  ),

  ui.put(
    "/repos/{owner}/{repo}/pulls/{pull_number}/auto-merge",
    async ({ params, request, response }) => {
      const fault = await gate("pulls-auto-merge");
      if (fault) return response.untyped(fault);
      const ctx = find(params);
      if (!ctx) return response.untyped(notFound());
      const { state, pr } = ctx;
      if (!state.repo.allow_auto_merge)
        return response(422).json(
          invalid("auto_merge", "Auto-merge is not allowed for this repository."),
        );
      if (pr.state !== "open" || pr.draft)
        return response(422).json(
          invalid("auto_merge", "Auto-merge needs an open pull request that is ready for review."),
        );
      if (pr.mergeable_state === "dirty")
        return response(422).json(invalid("auto_merge", "Pull request has merge conflicts."));
      const body = await request.json();
      pr.auto_merge = {
        enabled_by: viewer,
        merge_method: body.merge_method,
        commit_title: body.commit_title,
        commit_message: body.commit_message,
      };
      addEvent(state, pr.number, {
        ...eventBase(),
        event: "auto_merge_enabled",
        merge_method: body.merge_method,
      });
      touch(state, pr);
      // It may already be clean, in which case it merges now.
      afterReview();
      return response(200).json(pr);
    },
  ),

  ui.delete(
    "/repos/{owner}/{repo}/pulls/{pull_number}/auto-merge",
    async ({ params, response }) => {
      const fault = await gate("pulls-auto-merge");
      if (fault) return response.untyped(fault);
      const ctx = find(params);
      if (!ctx) return response.untyped(notFound());
      const { state, pr } = ctx;
      if (pr.auto_merge)
        addEvent(state, pr.number, { ...eventBase(), event: "auto_merge_disabled", reason: null });
      pr.auto_merge = null;
      touch(state, pr);
      return response(200).json(pr);
    },
  ),

  ui.post(
    "/repos/{owner}/{repo}/pulls/{pull_number}/reviews/{review_id}/comments",
    async ({ params, request, response }) => {
      const fault = await gate("pulls-draft-comment");
      if (fault) return response.untyped(fault);
      const ctx = find(params);
      if (!ctx) return response.untyped(notFound());
      const { state, pr } = ctx;
      const e = extra(state, pr.number);
      const review = e.reviews.find(
        (r) => r.id === Number(params.review_id) && r.state === "PENDING",
      );
      if (!review) return response.untyped(notFound());
      const draft = await request.json();
      if (!draft.body.trim()) return response(422).json(invalid("body", "Write something first."));
      const f = state.files.get(pr.number)?.find((x) => x.filename === draft.path);
      if (!f?.patch || !lineAt(parsePatch(f.patch), { side: draft.side, line: draft.line }))
        return response(422).json(
          invalid("line", "pull_request_review_thread.line must be part of the diff"),
        );
      const c = makeComment(state, pr, viewer, review.id, draft, new Date().toISOString());
      e.comments.push(c);
      return response(200).json(c);
    },
  ),
];
