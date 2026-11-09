import { HttpResponse } from "msw";
import type { Schemas } from "@/api/client";
import { lineAt, parsePatch } from "@/areas/pulls/patch";
import { byUpdatedDesc, gate, http, notFound, page, unprocessable } from "../http";
import { actionsApp, HOST, id, teams, users, type SimpleUser } from "../world";
import { db, type PullRequest, type RepoState } from "./core-db";
import {
  addEvent,
  afterReview,
  annotations,
  commitsOf,
  comparison,
  createPull,
  eventBase,
  extra,
  graph,
  makeComment,
  makeReview,
  merge,
  resolveRef,
  retarget,
  setOpen,
  simulate,
  sync,
  touch,
  updateBranch,
  viewer,
  visibleComments,
  minimalRepo,
  compareBranches,
} from "./pulls-db";

// Pull requests, reviews, review comments, compare, and checks. The private /_t/ui
// routes the pull request pages also read live in pulls-ui.ts.

const DOCS = "https://docs.github.com/enterprise-server@3.22/rest/pulls";

const invalid = (
  message: string,
  errors: { resource: string; field: string; code: string; message?: string }[] = [],
) =>
  HttpResponse.json({ message, documentation_url: DOCS, status: "422", errors }, { status: 422 });

type Ctx = { state: RepoState; pr: PullRequest };

function find(params: { owner: string; repo: string; pull_number: string }): Ctx | undefined {
  const state = db.state(params.owner, params.repo);
  const pr = state?.pulls.find((p) => p.number === Number(params.pull_number));
  if (!state || !pr) return undefined;
  sync(state, pr);
  return { state, pr };
}

const person = (login: string): SimpleUser | undefined =>
  Object.values(users).find((u) => u.login === login && u.type === "User");

const teamBySlug = (slug: string) => Object.values(teams).find((t) => t.slug === slug);

const list = (pr: PullRequest, state: RepoState) => ({
  ...db.simplePull(pr),
  requested_teams: extra(state, pr.number).requestedTeams,
});

// The run's latest state, resolved by sha, branch, or tag-less ref.
function runsFor(state: RepoState, ref: string) {
  const branch = resolveRef(state.repo.full_name, ref);
  const shaValue = branch?.history.at(-1)?.sha ?? ref;
  return { sha: shaValue, runs: db.checkRuns(shaValue) };
}

function findRun(state: RepoState, runId: number) {
  for (const pr of state.pulls) {
    const run = db.checkRuns(pr.head.sha).find((r) => r.id === runId);
    if (run) return run;
  }
  for (const b of graph(state.repo.full_name).values()) {
    const run = db.checkRuns(b.history.at(-1)?.sha ?? "").find((r) => r.id === runId);
    if (run) return run;
  }
  return undefined;
}

function suites(state: RepoState, runs: Schemas["check-run"][]): Schemas["check-suite"][] {
  const groups = new Map<number, Schemas["check-run"][]>();
  for (const r of runs) {
    const key = r.check_suite?.id ?? r.id;
    groups.set(key, [...(groups.get(key) ?? []), r]);
  }
  const repo = state.repo;
  const minimal = minimalRepo(repo);
  return [...groups].map(([suiteId, rs]) => {
    const done = rs.every((r) => r.status === "completed");
    const failed = rs.some((r) => r.conclusion === "failure" || r.conclusion === "timed_out");
    const headSha = rs[0].head_sha;
    const pr = state.pulls.find((p) => p.head.sha === headSha);
    return {
      id: suiteId,
      node_id: `CS_${suiteId}`,
      head_branch: pr?.head.ref ?? null,
      head_sha: headSha,
      status: done
        ? "completed"
        : rs.some((r) => r.status === "in_progress")
          ? "in_progress"
          : "queued",
      conclusion: done ? (failed ? "failure" : "success") : null,
      url: `${repo.url}/check-suites/${suiteId}`,
      before: null,
      after: headSha,
      pull_requests: [],
      app: actionsApp,
      repository: minimal,
      created_at: rs[0].started_at,
      updated_at:
        rs
          .map((r) => r.completed_at ?? r.started_at ?? "")
          .sort()
          .at(-1) ?? null,
      head_commit: {
        id: headSha,
        tree_id: headSha,
        message: pr?.title ?? "",
        timestamp: rs[0].started_at ?? "",
        author: null,
        committer: null,
      },
      latest_check_runs_count: rs.length,
      check_runs_url: `${repo.url}/check-suites/${suiteId}/check-runs`,
      rerequestable: true,
      runs_rerequestable: true,
    };
  });
}

function rerun(run: Schemas["check-run"]) {
  // A rerun of the rustfmt failure passes, as if the author pushed the formatting fix.
  simulate(
    run,
    "success",
    run.name === "rustfmt"
      ? "Formatted"
      : run.name.startsWith("test")
        ? "312 tests passed"
        : "No problems found",
  );
  if (run.name === "rustfmt") annotations.delete(run.id);
}

export const handlers = [
  // Registered before /pulls/{pull_number}, which would otherwise match "comments".
  http.get("/repos/{owner}/{repo}/pulls/comments", async ({ params, query, response }) => {
    const fault = await gate("pulls-comments");
    if (fault) return response.untyped(fault);
    const state = db.state(params.owner, params.repo);
    if (!state) return response.untyped(notFound());
    const all = state.pulls.flatMap((p) => visibleComments(extra(state, p.number)));
    return response(200).json(
      page(
        all.sort((a, b) => a.created_at.localeCompare(b.created_at)),
        query,
      ),
    );
  }),

  http.get("/repos/{owner}/{repo}/pulls/comments/{comment_id}", async ({ params, response }) => {
    const fault = await gate("pulls-comments");
    if (fault) return response.untyped(fault);
    const hit = findComment(params.owner, params.repo, Number(params.comment_id));
    return hit ? response(200).json(hit.comment) : response.untyped(notFound());
  }),

  http.patch(
    "/repos/{owner}/{repo}/pulls/comments/{comment_id}",
    async ({ params, request, response }) => {
      const fault = await gate("pulls-comment-edit");
      if (fault) return response.untyped(fault);
      const hit = findComment(params.owner, params.repo, Number(params.comment_id));
      if (!hit) return response.untyped(notFound());
      const { body } = await request.json();
      if (!body.trim())
        return response.untyped(unprocessable("PullRequestReviewComment", "body", "missing_field"));
      hit.comment.body = body;
      hit.comment.updated_at = new Date().toISOString();
      return response(200).json(hit.comment);
    },
  ),

  http.delete("/repos/{owner}/{repo}/pulls/comments/{comment_id}", async ({ params, response }) => {
    const fault = await gate("pulls-comment-edit");
    if (fault) return response.untyped(fault);
    const hit = findComment(params.owner, params.repo, Number(params.comment_id));
    if (!hit) return response.untyped(notFound());
    hit.extra.comments = hit.extra.comments.filter((c) => c !== hit.comment);
    hit.extra.threads.delete(hit.comment.id);
    return response(204).empty();
  }),

  // ---- Pull requests ----

  http.get("/repos/{owner}/{repo}/pulls", async ({ params, query, response }) => {
    const fault = await gate("pulls");
    if (fault) return response.untyped(fault);
    const state = db.state(params.owner, params.repo);
    if (!state) return response.untyped(notFound());
    const wanted = query.get("state") ?? "open";
    const head = query.get("head");
    const base = query.get("base");
    const sort = query.get("sort") ?? "created";
    const dir = query.get("direction") ?? (sort === "long-running" ? "asc" : "desc");
    const key = (p: PullRequest) =>
      sort === "updated"
        ? p.updated_at
        : sort === "popularity"
          ? String(p.comments + p.review_comments).padStart(6, "0")
          : p.created_at;
    const items = state.pulls
      .filter((p) => wanted === "all" || p.state === wanted)
      .filter((p) => !head || p.head.label === head || p.head.ref === head)
      .filter((p) => !base || p.base.ref === base)
      .map((p) => (sync(state, p), p))
      .sort(
        (a, b) => (dir === "asc" ? 1 : -1) * key(a).localeCompare(key(b)) || byUpdatedDesc(a, b),
      )
      .map((p) => list(p, state));
    return response(200).json(page(items, query));
  }),

  http.post("/repos/{owner}/{repo}/pulls", async ({ params, request, response }) => {
    const fault = await gate("pulls-create");
    if (fault) return response.untyped(fault);
    const state = db.state(params.owner, params.repo);
    if (!state) return response.untyped(notFound());
    const body = await request.json();
    const title = body.title?.trim() ?? "";
    const headRef = body.head.includes(":")
      ? body.head.slice(body.head.indexOf(":") + 1)
      : body.head;
    const g = graph(state.repo.full_name);
    if (!title) return response.untyped(unprocessable("PullRequest", "title", "missing_field"));
    if (!g.has(body.base)) return response.untyped(unprocessable("PullRequest", "base", "invalid"));
    if (!g.has(headRef)) return response.untyped(unprocessable("PullRequest", "head", "invalid"));
    if (headRef === body.base)
      return response.untyped(
        invalid("Validation Failed", [
          {
            resource: "PullRequest",
            field: "head",
            code: "custom",
            message: `The head and base branches are the same.`,
          },
        ]),
      );
    const existing = state.pulls.find(
      (p) => p.state === "open" && p.head.ref === headRef && p.base.ref === body.base,
    );
    if (existing)
      return response.untyped(
        invalid("Validation Failed", [
          {
            resource: "PullRequest",
            field: "head",
            code: "custom",
            message: `A pull request already exists for ${state.repo.owner.login}:${headRef} (#${existing.number}).`,
          },
        ]),
      );
    if (compareBranches(g.get(body.base)!, g.get(headRef)!).ahead.length === 0)
      return response.untyped(
        invalid("Validation Failed", [
          {
            resource: "PullRequest",
            field: "head",
            code: "custom",
            message: `No commits between ${body.base} and ${headRef}`,
          },
        ]),
      );
    const pr = createPull(state, {
      title,
      body: body.body ?? "",
      head: headRef,
      base: body.base,
      draft: body.draft ?? false,
    });
    return response(201).json(pr, { headers: { Location: pr.url } });
  }),

  http.get("/repos/{owner}/{repo}/pulls/{pull_number}", async ({ params, response }) => {
    const fault = await gate("pull");
    if (fault) return response.untyped(fault);
    const ctx = find(params);
    return ctx ? response(200).json(ctx.pr) : response.untyped(notFound());
  }),

  http.patch("/repos/{owner}/{repo}/pulls/{pull_number}", async ({ params, request, response }) => {
    const fault = await gate("pulls-update");
    if (fault) return response.untyped(fault);
    const ctx = find(params);
    if (!ctx) return response.untyped(notFound());
    const { state, pr } = ctx;
    const body = (await request.json()) ?? {};
    const issue = state.issues.find((i) => i.number === pr.number);
    if (body.title !== undefined) {
      const title = body.title.trim();
      if (!title) return response.untyped(unprocessable("PullRequest", "title", "missing_field"));
      if (title !== pr.title)
        addEvent(state, pr.number, {
          ...eventBase(),
          event: "renamed",
          rename: { from: pr.title, to: title },
        });
      pr.title = title;
      if (issue) issue.title = title;
    }
    if (body.body !== undefined) {
      pr.body = body.body;
      if (issue) issue.body = body.body;
    }
    if (body.base !== undefined && body.base !== pr.base.ref) {
      if (pr.state !== "open")
        return response.untyped(unprocessable("PullRequest", "base", "invalid"));
      const g = graph(state.repo.full_name);
      if (!g.has(body.base))
        return response.untyped(unprocessable("PullRequest", "base", "invalid"));
      if (body.base === pr.head.ref)
        return response.untyped(
          invalid("Validation Failed", [
            {
              resource: "PullRequest",
              field: "base",
              code: "invalid",
              message: "The base branch cannot be the head branch.",
            },
          ]),
        );
      addEvent(state, pr.number, {
        ...eventBase(),
        event: "base_ref_changed",
        base: { from: pr.base.ref, to: body.base },
      });
      retarget(pr, body.base);
      const head = g.get(pr.head.ref);
      if (head) {
        const files = compareBranches(g.get(body.base)!, head).files;
        state.files.set(pr.number, files);
        pr.changed_files = files.length;
        pr.additions = files.reduce((n, f) => n + f.additions, 0);
        pr.deletions = files.reduce((n, f) => n + f.deletions, 0);
      }
    }
    if (body.state !== undefined && body.state !== pr.state) {
      if (pr.merged) return response.untyped(invalid("Cannot reopen a merged pull request."));
      if (body.state === "open" && !graph(state.repo.full_name).has(pr.head.ref))
        return response.untyped(
          invalid("Validation Failed", [
            {
              resource: "PullRequest",
              field: "state",
              code: "invalid",
              message: "The head branch was deleted. Restore it to reopen.",
            },
          ]),
        );
      setOpen(state, pr, body.state === "open");
    }
    touch(state, pr);
    sync(state, pr);
    afterReview();
    return response(200).json(pr);
  }),

  http.get(
    "/repos/{owner}/{repo}/pulls/{pull_number}/commits",
    async ({ params, query, response }) => {
      const fault = await gate("pulls-commits");
      if (fault) return response.untyped(fault);
      const ctx = find(params);
      if (!ctx) return response.untyped(notFound());
      return response(200).json(page(commitsOf(ctx.state, ctx.pr), query));
    },
  ),

  http.get(
    "/repos/{owner}/{repo}/pulls/{pull_number}/files",
    async ({ params, query, response }) => {
      const fault = await gate("files");
      if (fault) return response.untyped(fault);
      const files = db.state(params.owner, params.repo)?.files.get(Number(params.pull_number));
      return files ? response(200).json(page(files, query)) : response.untyped(notFound());
    },
  ),

  // ---- Merging ----

  http.get("/repos/{owner}/{repo}/pulls/{pull_number}/merge", async ({ params, response }) => {
    const fault = await gate("pulls-merge");
    if (fault) return response.untyped(fault);
    const ctx = find(params);
    return ctx?.pr.merged ? response(204).empty() : response(404).empty();
  }),

  http.put(
    "/repos/{owner}/{repo}/pulls/{pull_number}/merge",
    async ({ params, request, response }) => {
      const fault = await gate("pulls-merge");
      if (fault) return response.untyped(fault);
      const ctx = find(params);
      if (!ctx) return response.untyped(notFound());
      const body = (await request.json()) ?? {};
      const result = merge(ctx.state, ctx.pr, {
        method: body.merge_method ?? "merge",
        title: body.commit_title,
        message: body.commit_message,
        sha: body.sha,
      });
      if (!result.ok)
        return response(result.status).json({
          message: result.message,
          documentation_url: `${DOCS}/pulls#merge-a-pull-request`,
        });
      return response(200).json({
        sha: result.sha,
        merged: true,
        message: "Pull Request successfully merged",
      });
    },
  ),

  http.put(
    "/repos/{owner}/{repo}/pulls/{pull_number}/update-branch",
    async ({ params, request, response }) => {
      const fault = await gate("pulls-update-branch");
      if (fault) return response.untyped(fault);
      const ctx = find(params);
      if (!ctx) return response.untyped(notFound());
      const { state, pr } = ctx;
      const body = (await request.json()) ?? {};
      if (body.expected_head_sha && body.expected_head_sha !== pr.head.sha)
        return response.untyped(
          invalid("expected_head_sha does not match the pull request's HEAD"),
        );
      const g = graph(state.repo.full_name);
      const head = g.get(pr.head.ref);
      const base = g.get(pr.base.ref);
      if (!head || !base || pr.state !== "open")
        return response.untyped(invalid("The head branch cannot be updated."));
      if (head.conflicts) return response.untyped(invalid("merge conflict between base and head"));
      if (compareBranches(base, head).behind.length === 0)
        return response.untyped(invalid("There are no new commits on the base branch."));
      updateBranch(state, pr);
      return response(202).json({
        message: "Updating pull request branch.",
        url: `${HOST}/${state.repo.full_name}/pull/${pr.number}`,
      });
    },
  ),

  // ---- Requested reviewers ----

  http.get(
    "/repos/{owner}/{repo}/pulls/{pull_number}/requested_reviewers",
    async ({ params, response }) => {
      const fault = await gate("pulls-reviewers");
      if (fault) return response.untyped(fault);
      const ctx = find(params);
      if (!ctx) return response.untyped(notFound());
      return response(200).json({
        users: ctx.pr.requested_reviewers ?? [],
        teams: extra(ctx.state, ctx.pr.number).requestedTeams,
      });
    },
  ),

  http.post(
    "/repos/{owner}/{repo}/pulls/{pull_number}/requested_reviewers",
    async ({ params, request, response }) => {
      const fault = await gate("pulls-request-reviewers");
      if (fault) return response.untyped(fault);
      const ctx = find(params);
      if (!ctx) return response.untyped(notFound());
      const { state, pr } = ctx;
      const body = ((await request.json()) ?? {}) as {
        reviewers?: string[];
        team_reviewers?: string[];
      };
      const e = extra(state, pr.number);
      for (const login of body.reviewers ?? []) {
        const u = person(login);
        if (!u) return response.untyped(unprocessable("PullRequest", "reviewers", "invalid"));
        if (u.login === pr.user.login)
          return response.untyped(invalid("Review cannot be requested from pull request author."));
      }
      for (const slug of body.team_reviewers ?? [])
        if (!teamBySlug(slug))
          return response.untyped(unprocessable("PullRequest", "team_reviewers", "invalid"));
      for (const login of body.reviewers ?? []) {
        const u = person(login)!;
        if (pr.requested_reviewers?.some((r) => r.login === login)) continue;
        pr.requested_reviewers = [...(pr.requested_reviewers ?? []), u];
        addEvent(state, pr.number, {
          ...eventBase(),
          event: "review_requested",
          requested_reviewer: u,
          requested_team: null,
        });
      }
      for (const slug of body.team_reviewers ?? []) {
        const t = teamBySlug(slug)!;
        if (e.requestedTeams.some((x) => x.slug === slug)) continue;
        e.requestedTeams = [...e.requestedTeams, t];
        addEvent(state, pr.number, {
          ...eventBase(),
          event: "review_requested",
          requested_reviewer: null,
          requested_team: { slug: t.slug, name: t.name },
        });
      }
      touch(state, pr);
      sync(state, pr);
      return response(201).json(list(pr, state));
    },
  ),

  http.delete(
    "/repos/{owner}/{repo}/pulls/{pull_number}/requested_reviewers",
    async ({ params, request, response }) => {
      const fault = await gate("pulls-request-reviewers");
      if (fault) return response.untyped(fault);
      const ctx = find(params);
      if (!ctx) return response.untyped(notFound());
      const { state, pr } = ctx;
      const body = await request.json();
      const e = extra(state, pr.number);
      for (const login of body.reviewers) {
        const u = pr.requested_reviewers?.find((r) => r.login === login);
        if (!u) continue;
        pr.requested_reviewers = pr.requested_reviewers?.filter((r) => r.login !== login);
        addEvent(state, pr.number, {
          ...eventBase(),
          event: "review_request_removed",
          requested_reviewer: u,
          requested_team: null,
        });
      }
      for (const slug of body.team_reviewers ?? []) {
        const t = e.requestedTeams.find((x) => x.slug === slug);
        if (!t) continue;
        e.requestedTeams = e.requestedTeams.filter((x) => x.slug !== slug);
        addEvent(state, pr.number, {
          ...eventBase(),
          event: "review_request_removed",
          requested_reviewer: null,
          requested_team: { slug: t.slug, name: t.name },
        });
      }
      touch(state, pr);
      sync(state, pr);
      return response(200).json(list(pr, state));
    },
  ),

  // ---- Reviews ----

  http.get(
    "/repos/{owner}/{repo}/pulls/{pull_number}/reviews",
    async ({ params, query, response }) => {
      const fault = await gate("pulls-reviews");
      if (fault) return response.untyped(fault);
      const ctx = find(params);
      if (!ctx) return response.untyped(notFound());
      // A pending review is visible only to its author, who is always the viewer here.
      const reviews = extra(ctx.state, ctx.pr.number).reviews.filter(
        (r) => r.state !== "PENDING" || r.user?.login === viewer.login,
      );
      return response(200).json(page(reviews, query));
    },
  ),

  http.post(
    "/repos/{owner}/{repo}/pulls/{pull_number}/reviews",
    async ({ params, request, response }) => {
      const fault = await gate("pulls-review-create");
      if (fault) return response.untyped(fault);
      const ctx = find(params);
      if (!ctx) return response.untyped(notFound());
      const { state, pr } = ctx;
      const body = (await request.json()) ?? {};
      const e = extra(state, pr.number);
      if (
        !body.event &&
        e.reviews.some((r) => r.state === "PENDING" && r.user?.login === viewer.login)
      )
        return response(422).json({
          message: "Unprocessable Entity",
          documentation_url: DOCS,
          errors: ["User can only have one pending review per pull request"],
        });
      const refused = submitRefusal(
        pr,
        body.event ?? "PENDING",
        body.body ?? "",
        (body.comments ?? []).length,
      );
      if (refused)
        return response(422).json({
          message: "Unprocessable Entity",
          documentation_url: DOCS,
          errors: [refused],
        });
      for (const c of body.comments ?? []) {
        const bad = badLine(state, pr, {
          path: c.path,
          line: c.line ?? 0,
          side: c.side === "LEFT" ? "LEFT" : "RIGHT",
        });
        if (bad)
          return response(422).json({
            message: "Unprocessable Entity",
            documentation_url: DOCS,
            errors: [bad],
          });
      }
      const now = new Date().toISOString();
      const stateName =
        body.event === "APPROVE"
          ? "APPROVED"
          : body.event === "REQUEST_CHANGES"
            ? "CHANGES_REQUESTED"
            : body.event === "COMMENT"
              ? "COMMENTED"
              : "PENDING";
      const review = makeReview(
        pr,
        viewer,
        stateName,
        body.body ?? "",
        stateName === "PENDING" ? undefined : now,
      );
      e.reviews.push(review);
      for (const c of body.comments ?? [])
        e.comments.push(
          makeComment(
            state,
            pr,
            viewer,
            review.id,
            {
              path: c.path,
              body: c.body,
              line: c.line ?? 0,
              side: c.side === "LEFT" ? "LEFT" : "RIGHT",
              ...(c.start_line
                ? {
                    start_line: c.start_line,
                    start_side: c.start_side === "LEFT" ? "LEFT" : "RIGHT",
                  }
                : {}),
            },
            now,
          ),
        );
      if (stateName !== "PENDING") submitted(state, pr);
      return response(200).json(review);
    },
  ),

  http.get(
    "/repos/{owner}/{repo}/pulls/{pull_number}/reviews/{review_id}",
    async ({ params, response }) => {
      const fault = await gate("pulls-reviews");
      if (fault) return response.untyped(fault);
      const ctx = find(params);
      const review =
        ctx &&
        extra(ctx.state, ctx.pr.number).reviews.find((r) => r.id === Number(params.review_id));
      return review ? response(200).json(review) : response.untyped(notFound());
    },
  ),

  http.put(
    "/repos/{owner}/{repo}/pulls/{pull_number}/reviews/{review_id}",
    async ({ params, request, response }) => {
      const fault = await gate("pulls-review-edit");
      if (fault) return response.untyped(fault);
      const ctx = find(params);
      const review =
        ctx &&
        extra(ctx.state, ctx.pr.number).reviews.find((r) => r.id === Number(params.review_id));
      if (!review) return response.untyped(notFound());
      review.body = (await request.json()).body;
      return response(200).json(review);
    },
  ),

  http.delete(
    "/repos/{owner}/{repo}/pulls/{pull_number}/reviews/{review_id}",
    async ({ params, response }) => {
      const fault = await gate("pulls-review-discard");
      if (fault) return response.untyped(fault);
      const ctx = find(params);
      if (!ctx) return response.untyped(notFound());
      const e = extra(ctx.state, ctx.pr.number);
      const review = e.reviews.find((r) => r.id === Number(params.review_id));
      if (!review) return response.untyped(notFound());
      if (review.state !== "PENDING")
        return response(422).json({
          message: "Unprocessable Entity",
          documentation_url: DOCS,
          errors: ["Can not delete a non-pending pull request review"],
        });
      e.reviews = e.reviews.filter((r) => r !== review);
      e.comments = e.comments.filter((c) => c.pull_request_review_id !== review.id);
      return response(200).json(review);
    },
  ),

  http.get(
    "/repos/{owner}/{repo}/pulls/{pull_number}/reviews/{review_id}/comments",
    async ({ params, query, response }) => {
      const fault = await gate("pulls-reviews");
      if (fault) return response.untyped(fault);
      const ctx = find(params);
      if (!ctx) return response.untyped(notFound());
      const reviewId = Number(params.review_id);
      const comments = extra(ctx.state, ctx.pr.number)
        .comments.filter((c) => c.pull_request_review_id === reviewId)
        .map(
          ({
            start_line,
            original_start_line,
            start_side,
            line,
            original_line,
            side,
            subject_type,
            ...legacy
          }) => ({
            ...legacy,
            position: line ?? null,
            original_position: original_line ?? 0,
            start_line,
            original_start_line,
            start_side,
            line,
            original_line,
            side,
            subject_type,
          }),
        );
      return response(200).json(page(comments, query));
    },
  ),

  http.put(
    "/repos/{owner}/{repo}/pulls/{pull_number}/reviews/{review_id}/dismissals",
    async ({ params, request, response }) => {
      const fault = await gate("pulls-review-dismiss");
      if (fault) return response.untyped(fault);
      const ctx = find(params);
      if (!ctx) return response.untyped(notFound());
      const { state, pr } = ctx;
      const review = extra(state, pr.number).reviews.find((r) => r.id === Number(params.review_id));
      if (!review) return response.untyped(notFound());
      const { message } = await request.json();
      if (!message.trim())
        return response(422).json({
          message: "Unprocessable Entity",
          documentation_url: DOCS,
          errors: ["message is missing"],
        });
      if (review.state !== "APPROVED" && review.state !== "CHANGES_REQUESTED")
        return response(422).json({
          message: "Unprocessable Entity",
          documentation_url: DOCS,
          errors: ["Can not dismiss a commented pull request review"],
        });
      review.state = "DISMISSED";
      addEvent(state, pr.number, {
        ...eventBase(),
        event: "review_dismissed",
        review_id: review.id,
        dismissal_message: message,
        reviewer: review.user ?? users.org,
      });
      touch(state, pr);
      afterReview();
      return response(200).json(review);
    },
  ),

  http.post(
    "/repos/{owner}/{repo}/pulls/{pull_number}/reviews/{review_id}/events",
    async ({ params, request, response }) => {
      const fault = await gate("pulls-review-submit");
      if (fault) return response.untyped(fault);
      const ctx = find(params);
      if (!ctx) return response.untyped(notFound());
      const { state, pr } = ctx;
      const e = extra(state, pr.number);
      const review = e.reviews.find((r) => r.id === Number(params.review_id));
      if (!review) return response.untyped(notFound());
      if (review.state !== "PENDING")
        return response(422).json({
          message: "Unprocessable Entity",
          documentation_url: DOCS,
          errors: ["Can not submit a non-pending review"],
        });
      const body = await request.json();
      const count = e.comments.filter((c) => c.pull_request_review_id === review.id).length;
      const refused = submitRefusal(pr, body.event, body.body ?? "", count);
      if (refused)
        return response(422).json({
          message: "Unprocessable Entity",
          documentation_url: DOCS,
          errors: [refused],
        });
      const now = new Date().toISOString();
      review.state =
        body.event === "APPROVE"
          ? "APPROVED"
          : body.event === "REQUEST_CHANGES"
            ? "CHANGES_REQUESTED"
            : "COMMENTED";
      review.body = body.body ?? review.body;
      review.submitted_at = now;
      review.commit_id = pr.head.sha;
      for (const c of e.comments.filter((c) => c.pull_request_review_id === review.id))
        c.created_at = c.updated_at = now;
      submitted(state, pr);
      return response(200).json(review);
    },
  ),

  // ---- Review comments ----

  http.get(
    "/repos/{owner}/{repo}/pulls/{pull_number}/comments",
    async ({ params, query, response }) => {
      const fault = await gate("pulls-comments");
      if (fault) return response.untyped(fault);
      const ctx = find(params);
      if (!ctx) return response.untyped(notFound());
      return response(200).json(page(visibleComments(extra(ctx.state, ctx.pr.number)), query));
    },
  ),

  http.post(
    "/repos/{owner}/{repo}/pulls/{pull_number}/comments",
    async ({ params, request, response }) => {
      const fault = await gate("pulls-comment");
      if (fault) return response.untyped(fault);
      const ctx = find(params);
      if (!ctx) return response.untyped(notFound());
      const { state, pr } = ctx;
      const body = await request.json();
      if (!body.body.trim())
        return response.untyped(unprocessable("PullRequestReviewComment", "body", "missing_field"));
      const e = extra(state, pr.number);
      const now = new Date().toISOString();
      const parent = body.in_reply_to
        ? e.comments.find((c) => c.id === body.in_reply_to)
        : undefined;
      if (body.in_reply_to && !parent) return response.untyped(notFound());
      const anchor = parent
        ? { path: parent.path, line: parent.line ?? 0, side: parent.side }
        : { path: body.path, line: body.line ?? 0, side: body.side ?? ("RIGHT" as const) };
      if (!parent) {
        const bad = badLine(state, pr, anchor);
        if (bad)
          return response.untyped(
            invalid("Validation Failed", [
              {
                resource: "PullRequestReviewComment",
                field: "line",
                code: "invalid",
                message: bad,
              },
            ]),
          );
      }
      // A lone comment arrives in its own submitted review, as on GitHub.
      const review = makeReview(pr, viewer, "COMMENTED", "", now);
      e.reviews.push(review);
      const c = makeComment(
        state,
        pr,
        viewer,
        review.id,
        {
          ...anchor,
          body: body.body,
          ...(parent
            ? { in_reply_to_id: parent.id }
            : body.start_line
              ? {
                  start_line: body.start_line,
                  start_side: body.start_side === "LEFT" ? "LEFT" : "RIGHT",
                }
              : {}),
        },
        now,
      );
      e.comments.push(c);
      touch(state, pr, now);
      sync(state, pr);
      return response(201).json(c, { headers: { Location: c.url } });
    },
  ),

  http.post(
    "/repos/{owner}/{repo}/pulls/{pull_number}/comments/{comment_id}/replies",
    async ({ params, request, response }) => {
      const fault = await gate("pulls-reply");
      if (fault) return response.untyped(fault);
      const ctx = find(params);
      if (!ctx) return response.untyped(notFound());
      const { state, pr } = ctx;
      const e = extra(state, pr.number);
      const parent = e.comments.find((c) => c.id === Number(params.comment_id));
      if (!parent) return response.untyped(notFound());
      const { body } = await request.json();
      if (!body.trim())
        return response.untyped(unprocessable("PullRequestReviewComment", "body", "missing_field"));
      const root = parent.in_reply_to_id ?? parent.id;
      const now = new Date().toISOString();
      const pending = e.reviews.find(
        (r) => r.state === "PENDING" && r.user?.login === viewer.login,
      );
      const review = pending ?? makeReview(pr, viewer, "COMMENTED", "", now);
      if (!pending) e.reviews.push(review);
      const c = makeComment(
        state,
        pr,
        viewer,
        review.id,
        {
          path: parent.path,
          line: parent.line ?? 0,
          side: parent.side,
          body,
          in_reply_to_id: root,
        },
        now,
      );
      e.comments.push(c);
      touch(state, pr, now);
      sync(state, pr);
      return response(201).json(c, { headers: { Location: c.url } });
    },
  ),

  // ---- Compare ----

  http.get("/repos/{owner}/{repo}/compare/{basehead}", async ({ params, response }) => {
    const fault = await gate("pulls-compare");
    if (fault) return response.untyped(fault);
    const state = db.state(params.owner, params.repo);
    if (!state) return response.untyped(notFound());
    const [baseRef, headRef] = params.basehead.split("...");
    if (!headRef) return response.untyped(unprocessable("Comparison", "basehead", "invalid"));
    const base = resolveRef(state.repo.full_name, baseRef);
    const head = resolveRef(state.repo.full_name, headRef);
    if (!base || !head) return response.untyped(notFound());
    return response(200).json(comparison(state, base, head));
  }),

  // ---- Checks ----

  http.get(
    "/repos/{owner}/{repo}/commits/{ref}/check-runs",
    async ({ params, query, response }) => {
      const fault = await gate("checks");
      if (fault) return response.untyped(fault);
      const state = db.state(params.owner, params.repo);
      if (!state) return response.untyped(notFound());
      const name = query.get("check_name");
      const status = query.get("status");
      const runs = runsFor(state, params.ref).runs.filter(
        (r) => (!name || r.name === name) && (!status || r.status === status),
      );
      return response(200).json({ total_count: runs.length, check_runs: page(runs, query) });
    },
  ),

  http.get(
    "/repos/{owner}/{repo}/commits/{ref}/check-suites",
    async ({ params, query, response }) => {
      const fault = await gate("pulls-suites");
      if (fault) return response.untyped(fault);
      const state = db.state(params.owner, params.repo);
      if (!state) return response.untyped(notFound());
      const list = suites(state, runsFor(state, params.ref).runs);
      return response(200).json({ total_count: list.length, check_suites: page(list, query) });
    },
  ),

  http.get("/repos/{owner}/{repo}/check-runs/{check_run_id}", async ({ params, response }) => {
    const fault = await gate("pulls-check-run");
    if (fault) return response.untyped(fault);
    const state = db.state(params.owner, params.repo);
    const run = state && findRun(state, Number(params.check_run_id));
    return run ? response(200).json(run) : response.untyped(notFound());
  }),

  http.get(
    "/repos/{owner}/{repo}/check-runs/{check_run_id}/annotations",
    async ({ params, query, response }) => {
      const fault = await gate("pulls-annotations");
      if (fault) return response.untyped(fault);
      const state = db.state(params.owner, params.repo);
      const run = state && findRun(state, Number(params.check_run_id));
      if (!run) return response.untyped(notFound());
      return response(200).json(page(annotations.get(run.id) ?? [], query));
    },
  ),

  http.post(
    "/repos/{owner}/{repo}/check-runs/{check_run_id}/rerequest",
    async ({ params, response }) => {
      const fault = await gate("pulls-rerun");
      if (fault) return response.untyped(fault);
      const state = db.state(params.owner, params.repo);
      const run = state && findRun(state, Number(params.check_run_id));
      if (!run) return response.untyped(notFound());
      if (run.status !== "completed")
        return response(422).json({
          message: "This check run is not rerequestable until it completes.",
          documentation_url: DOCS,
        });
      rerun(run);
      return response(201).json({});
    },
  ),

  http.post("/repos/{owner}/{repo}/check-runs", async ({ params, request, response }) => {
    const fault = await gate("pulls-check-create");
    if (fault) return response.untyped(fault);
    const state = db.state(params.owner, params.repo);
    if (!state) return response.untyped(notFound());
    const body = await request.json();
    const n = id();
    const run: Schemas["check-run"] = {
      id: n,
      head_sha: body.head_sha,
      node_id: `CR_${n}`,
      external_id: body.external_id ?? null,
      url: `${state.repo.url}/check-runs/${n}`,
      html_url: `${state.repo.html_url}/runs/${n}`,
      details_url: body.details_url ?? null,
      status: body.status ?? "queued",
      conclusion: body.conclusion === "stale" ? "neutral" : (body.conclusion ?? null),
      started_at: body.started_at ?? new Date().toISOString(),
      completed_at: body.completed_at ?? null,
      output: {
        title: body.output?.title ?? null,
        summary: body.output?.summary ?? null,
        text: body.output?.text ?? null,
        annotations_count: body.output?.annotations?.length ?? 0,
        annotations_url: `${state.repo.url}/check-runs/${n}/annotations`,
      },
      name: body.name,
      check_suite: { id: id() },
      app: actionsApp,
      pull_requests: [],
    };
    if (body.output?.annotations)
      annotations.set(
        n,
        body.output.annotations.map((a) => ({
          path: a.path,
          start_line: a.start_line,
          end_line: a.end_line,
          start_column: a.start_column ?? null,
          end_column: a.end_column ?? null,
          annotation_level: a.annotation_level,
          title: a.title ?? null,
          message: a.message,
          raw_details: a.raw_details ?? null,
          blob_href: `${HOST}/${state.repo.full_name}/blob/${body.head_sha}/${a.path}#L${a.start_line}`,
        })),
      );
    db.addCheckRun(body.head_sha, run);
    afterReview();
    return response(201).json(run);
  }),

  http.get("/repos/{owner}/{repo}/check-suites/{check_suite_id}", async ({ params, response }) => {
    const fault = await gate("pulls-suites");
    if (fault) return response.untyped(fault);
    const state = db.state(params.owner, params.repo);
    if (!state) return response.untyped(notFound());
    const suiteId = Number(params.check_suite_id);
    const runs = allRuns(state).filter((r) => r.check_suite?.id === suiteId);
    const suite = runs.length ? suites(state, runs)[0] : undefined;
    return suite ? response(200).json(suite) : response.untyped(notFound());
  }),

  http.get(
    "/repos/{owner}/{repo}/check-suites/{check_suite_id}/check-runs",
    async ({ params, query, response }) => {
      const fault = await gate("pulls-suites");
      if (fault) return response.untyped(fault);
      const state = db.state(params.owner, params.repo);
      if (!state) return response.untyped(notFound());
      const runs = allRuns(state).filter(
        (r) => r.check_suite?.id === Number(params.check_suite_id),
      );
      return response(200).json({ total_count: runs.length, check_runs: page(runs, query) });
    },
  ),

  http.post(
    "/repos/{owner}/{repo}/check-suites/{check_suite_id}/rerequest",
    async ({ params, response }) => {
      const fault = await gate("pulls-rerun");
      if (fault) return response.untyped(fault);
      const state = db.state(params.owner, params.repo);
      if (!state) return response.untyped(notFound());
      const runs = allRuns(state).filter(
        (r) => r.check_suite?.id === Number(params.check_suite_id),
      );
      if (!runs.length) return response.untyped(notFound());
      runs.filter((r) => r.status === "completed").forEach(rerun);
      return response(201).json({});
    },
  ),
];

function allRuns(state: RepoState) {
  const shas = new Set([
    ...state.pulls.map((p) => p.head.sha),
    ...[...graph(state.repo.full_name).values()].map((b) => b.history.at(-1)?.sha ?? ""),
  ]);
  return [...shas].flatMap((s) => db.checkRuns(s));
}

function findComment(owner: string, repo: string, commentId: number) {
  const state = db.state(owner, repo);
  if (!state) return undefined;
  for (const pr of state.pulls) {
    const e = extra(state, pr.number);
    const comment = e.comments.find((c) => c.id === commentId);
    if (comment) return { comment, extra: e };
  }
  return undefined;
}

function badLine(
  state: RepoState,
  pr: PullRequest,
  a: { path: string; line: number; side: "LEFT" | "RIGHT" },
) {
  const f = state.files.get(pr.number)?.find((x) => x.filename === a.path);
  if (!f) return `path ${a.path} is not part of the pull request`;
  if (!f.patch || !lineAt(parsePatch(f.patch), { side: a.side, line: a.line }))
    return "pull_request_review_thread.line must be part of the diff";
  return undefined;
}

// GitHub's rules for submitting a review, in its error strings.
function submitRefusal(pr: PullRequest, event: string, body: string, comments: number) {
  if (event !== "COMMENT" && event !== "PENDING" && pr.user.login === viewer.login)
    return event === "APPROVE"
      ? "Can not approve your own pull request"
      : "Can not request changes on your own pull request";
  if (pr.state !== "open" && event !== "COMMENT" && event !== "PENDING")
    return "Can not review a closed pull request";
  if ((event === "REQUEST_CHANGES" || event === "COMMENT") && !body.trim() && comments === 0)
    return `Review body is required for ${event === "COMMENT" ? "a comment" : "requesting changes"}`;
  return undefined;
}

// A submitted review answers the viewer's review request, and may unblock auto-merge.
function submitted(state: RepoState, pr: PullRequest) {
  pr.requested_reviewers = pr.requested_reviewers?.filter((u) => u.login !== viewer.login);
  touch(state, pr);
  sync(state, pr);
  afterReview();
}
