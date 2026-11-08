import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import Markdown from "react-markdown";
import remarkGfm from "remark-gfm";
import type { Schemas } from "@/api/client";
import { users } from "../world";
import { gate, http, notFound, page, unprocessable } from "../http";
import { db, type Issue, type RepoState } from "./core-db";
import {
  assignable,
  createLabel,
  crossReference,
  deleteLabel,
  deleteMilestone,
  events,
  extrasFor,
  findComment,
  labelNames,
  lockIssue,
  milestone,
  react,
  reactions,
  REACTIONS,
  recountMilestones,
  renameIssue,
  setIssueAssignees,
  setIssueLabels,
  setIssueMilestone,
  setIssueState,
  unlockIssue,
  unreact,
  updateLabel,
  type Subject,
} from "./issues-db";

// Issues, comments, labels, milestones, the timeline, reactions, and markdown rendering.
// The issues list includes pull requests, as GHES's does. Every write has its own fault key.

const me = users.mira;
const LOCK_REASONS = ["off-topic", "too heated", "resolved", "spam"];

function find(owner: string, repo: string, number: string) {
  const state = db.state(owner, repo);
  const issue = state?.issues.find((i) => i.number === Number(number));
  return state && issue ? { state, issue } : undefined;
}

function byKey(sort: string | null, direction: string | null) {
  const dir = direction === "asc" ? 1 : -1;
  const key = sort === "updated" ? "updated_at" : "created_at";
  if (sort === "comments") return (a: Issue, b: Issue) => dir * (a.comments - b.comments);
  return (a: Issue, b: Issue) => dir * a[key].localeCompare(b[key]);
}

function milestoneBody(state: RepoState, value: string | number | null | undefined) {
  if (value === null || value === undefined || value === "") return null;
  return extrasFor(state).milestones.find((m) => m.number === Number(value));
}

function readLabels(
  body:
    | string
    | (string | { name?: string })[]
    | { labels?: (string | { name?: string })[] }
    | undefined,
): string[] {
  const raw =
    body === undefined
      ? []
      : typeof body === "string"
        ? [body]
        : Array.isArray(body)
          ? body
          : (body.labels ?? []);
  return raw.map((l) => (typeof l === "string" ? l : (l.name ?? ""))).filter(Boolean);
}

const isReaction = (c: string): c is (typeof REACTIONS)[number] =>
  (REACTIONS as readonly string[]).includes(c);

// Everything the timeline shows, comments included, oldest first.
function timeline(state: RepoState, number: number): Schemas["timeline-issue-events"][] {
  const comments: Schemas["timeline-issue-events"][] = (state.comments.get(number) ?? []).map(
    (c) => ({
      ...c,
      event: "commented",
      actor: c.user ?? users.mira,
      user: c.user ?? users.mira,
      author_association: c.author_association ?? "NONE",
    }),
  );
  return [...events(state, number).map((e) => e.ev), ...comments].sort((a, b) =>
    ("created_at" in a && a.created_at ? a.created_at : "").localeCompare(
      "created_at" in b && b.created_at ? b.created_at : "",
    ),
  );
}

// /issues/{n}/events serves the issue-event-for-issue union, which has no member for
// closed, reopened, or cross-referenced; those appear on the timeline only.
function issueEvents(state: RepoState, number: number): Schemas["issue-event-for-issue"][] {
  return events(state, number).flatMap((e): Schemas["issue-event-for-issue"][] => {
    switch (e.kind) {
      case "label":
      case "milestone":
      case "rename":
      case "lock":
        return [e.ev];
      case "assign":
        return [{ ...e.ev, assigner: e.ev.actor }];
      case "state":
      case "xref":
        return [];
    }
  });
}

function subjectFor(state: RepoState, kind: "issue" | "comment", raw: string): Subject | undefined {
  if (kind === "issue") {
    const n = Number(raw);
    return state.issues.some((i) => i.number === n) ? { kind, number: n } : undefined;
  }
  const commentId = Number(raw);
  return findComment(state, commentId) ? { kind, id: commentId } : undefined;
}

function markdown(text: string) {
  return renderToStaticMarkup(createElement(Markdown, { remarkPlugins: [remarkGfm] }, text));
}

export const handlers = [
  // ---- Issues ----

  http.get("/repos/{owner}/{repo}/issues", async ({ params, query, response }) => {
    const fault = await gate("issues");
    if (fault) return response.untyped(fault);
    const state = db.state(params.owner, params.repo);
    if (!state) return response.untyped(notFound());
    const wanted = query.get("state") ?? "open";
    const labels = query.get("labels")?.split(",").filter(Boolean) ?? [];
    const assignee = query.get("assignee");
    const creator = query.get("creator");
    const ms = query.get("milestone");
    const items = state.issues
      .filter((i) => wanted === "all" || i.state === wanted)
      .filter((i) => labels.every((l) => labelNames(i).includes(l)))
      .filter((i) => !creator || i.user?.login === creator)
      .filter((i) => {
        if (!assignee) return true;
        const logins = (i.assignees ?? []).map((u) => u.login);
        if (assignee === "none") return logins.length === 0;
        if (assignee === "*") return logins.length > 0;
        return logins.includes(assignee);
      })
      .filter((i) => {
        if (!ms) return true;
        if (ms === "none") return !i.milestone;
        if (ms === "*") return Boolean(i.milestone);
        return i.milestone?.number === Number(ms);
      })
      .sort(byKey(query.get("sort"), query.get("direction")));
    return response(200).json(page(items, query));
  }),

  http.post("/repos/{owner}/{repo}/issues", async ({ params, request, response }) => {
    const fault = await gate("issues-create");
    if (fault) return response.untyped(fault);
    const state = db.state(params.owner, params.repo);
    if (!state) return response.untyped(notFound());
    const body = await request.json();
    const title = String(body.title ?? "").trim();
    if (!title) return response.untyped(unprocessable("Issue", "title", "missing_field"));
    const ms = milestoneBody(state, body.milestone);
    if (ms === undefined) return response.untyped(unprocessable("Issue", "milestone", "invalid"));
    const created = db.createIssue(state, { title, body: body.body ?? "" });
    setIssueLabels(state, created, readLabels(body.labels), me);
    setIssueAssignees(state, created, body.assignees ?? (body.assignee ? [body.assignee] : []), me);
    setIssueMilestone(state, created, ms, me);
    if (created.body) crossReference(state, created, created.body, me);
    recountMilestones(state);
    return response(201).json(created);
  }),

  http.get("/repos/{owner}/{repo}/issues/{issue_number}", async ({ params, response }) => {
    const fault = await gate("issue");
    if (fault) return response.untyped(fault);
    const hit = find(params.owner, params.repo, params.issue_number);
    return hit ? response(200).json(hit.issue) : response.untyped(notFound());
  }),

  http.patch(
    "/repos/{owner}/{repo}/issues/{issue_number}",
    async ({ params, request, response }) => {
      const fault = await gate("issues-update");
      if (fault) return response.untyped(fault);
      const hit = find(params.owner, params.repo, params.issue_number);
      if (!hit) return response.untyped(notFound());
      const { state, issue } = hit;
      const body = (await request.json()) ?? {};
      if (body.title !== undefined && body.title !== null) {
        const title = String(body.title).trim();
        if (!title) return response.untyped(unprocessable("Issue", "title", "missing_field"));
        renameIssue(state, issue, title, me);
      }
      if (body.body !== undefined) {
        issue.body = body.body ?? "";
        issue.updated_at = new Date().toISOString();
        const pr = state.pulls.find((p) => p.number === issue.number);
        if (pr) pr.body = issue.body;
        crossReference(state, issue, issue.body, me);
      }
      if (body.milestone !== undefined) {
        const ms = milestoneBody(state, body.milestone);
        if (ms === undefined)
          return response.untyped(unprocessable("Issue", "milestone", "invalid"));
        setIssueMilestone(state, issue, ms, me);
      }
      if (body.labels !== undefined) setIssueLabels(state, issue, readLabels(body.labels), me);
      if (body.assignees !== undefined) {
        const logins = body.assignees.map((a) => (typeof a === "string" ? a : (a.login ?? "")));
        setIssueAssignees(state, issue, logins, me);
      }
      if (body.state) {
        const reason = body.state_reason === "duplicate" ? "duplicate" : body.state_reason;
        setIssueState(state, issue, body.state, reason ?? null, me);
      }
      recountMilestones(state);
      return response(200).json(issue);
    },
  ),

  http.put(
    "/repos/{owner}/{repo}/issues/{issue_number}/lock",
    async ({ params, request, response }) => {
      const fault = await gate("issues-lock");
      if (fault) return response.untyped(fault);
      const hit = find(params.owner, params.repo, params.issue_number);
      if (!hit) return response.untyped(notFound());
      const text = await request.text();
      const reason: unknown = text ? JSON.parse(text)?.lock_reason : undefined;
      if (reason !== undefined && (typeof reason !== "string" || !LOCK_REASONS.includes(reason))) {
        return response.untyped(unprocessable("Issue", "lock_reason", "invalid"));
      }
      lockIssue(hit.state, hit.issue, typeof reason === "string" ? reason : null, me);
      return response(204).empty();
    },
  ),

  http.delete("/repos/{owner}/{repo}/issues/{issue_number}/lock", async ({ params, response }) => {
    const fault = await gate("issues-lock");
    if (fault) return response.untyped(fault);
    const hit = find(params.owner, params.repo, params.issue_number);
    if (!hit) return response.untyped(notFound());
    unlockIssue(hit.state, hit.issue, me);
    return response(204).empty();
  }),

  // ---- Labels and assignees on one issue ----

  http.get(
    "/repos/{owner}/{repo}/issues/{issue_number}/labels",
    async ({ params, query, response }) => {
      const fault = await gate("issues-labels");
      if (fault) return response.untyped(fault);
      const hit = find(params.owner, params.repo, params.issue_number);
      if (!hit) return response.untyped(notFound());
      const names = labelNames(hit.issue);
      const list = extrasFor(hit.state).labels.filter((l) => names.includes(l.name));
      return response(200).json(page(list, query));
    },
  ),

  http.put(
    "/repos/{owner}/{repo}/issues/{issue_number}/labels",
    async ({ params, request, response }) => {
      const fault = await gate("issues-issue-labels");
      if (fault) return response.untyped(fault);
      const hit = find(params.owner, params.repo, params.issue_number);
      if (!hit) return response.untyped(notFound());
      const names = readLabels(await request.json());
      return response(200).json(setIssueLabels(hit.state, hit.issue, names, me));
    },
  ),

  http.post(
    "/repos/{owner}/{repo}/issues/{issue_number}/labels",
    async ({ params, request, response }) => {
      const fault = await gate("issues-issue-labels");
      if (fault) return response.untyped(fault);
      const hit = find(params.owner, params.repo, params.issue_number);
      if (!hit) return response.untyped(notFound());
      const added = readLabels(await request.json());
      const known = extrasFor(hit.state).labels.map((l) => l.name);
      const unknown = added.find((n) => !known.includes(n));
      if (unknown) return response.untyped(unprocessable("Label", "name", "invalid"));
      const names = [...new Set([...labelNames(hit.issue), ...added])];
      return response(200).json(setIssueLabels(hit.state, hit.issue, names, me));
    },
  ),

  http.delete(
    "/repos/{owner}/{repo}/issues/{issue_number}/labels",
    async ({ params, response }) => {
      const fault = await gate("issues-issue-labels");
      if (fault) return response.untyped(fault);
      const hit = find(params.owner, params.repo, params.issue_number);
      if (!hit) return response.untyped(notFound());
      setIssueLabels(hit.state, hit.issue, [], me);
      return response(204).empty();
    },
  ),

  http.delete(
    "/repos/{owner}/{repo}/issues/{issue_number}/labels/{name}",
    async ({ params, response }) => {
      const fault = await gate("issues-issue-labels");
      if (fault) return response.untyped(fault);
      const hit = find(params.owner, params.repo, params.issue_number);
      if (!hit) return response.untyped(notFound());
      const names = labelNames(hit.issue);
      if (!names.includes(params.name)) return response.untyped(notFound());
      const left = setIssueLabels(
        hit.state,
        hit.issue,
        names.filter((n) => n !== params.name),
        me,
      );
      return response(200).json(left);
    },
  ),

  http.post(
    "/repos/{owner}/{repo}/issues/{issue_number}/assignees",
    async ({ params, request, response }) => {
      const fault = await gate("issues-assignees");
      if (fault) return response.untyped(fault);
      const hit = find(params.owner, params.repo, params.issue_number);
      if (!hit) return response.untyped(notFound());
      const body = (await request.json()) ?? {};
      const add = (body.assignees ?? []).map((a) => (typeof a === "string" ? a : a.login));
      const current = (hit.issue.assignees ?? []).map((u) => u.login);
      setIssueAssignees(hit.state, hit.issue, [...new Set([...current, ...add])].slice(0, 10), me);
      return response(201).json(hit.issue);
    },
  ),

  http.delete(
    "/repos/{owner}/{repo}/issues/{issue_number}/assignees",
    async ({ params, request, response }) => {
      const fault = await gate("issues-assignees");
      if (fault) return response.untyped(fault);
      const hit = find(params.owner, params.repo, params.issue_number);
      if (!hit) return response.untyped(notFound());
      const body = (await request.json()) ?? {};
      const remove = body.assignees ?? [];
      const left = (hit.issue.assignees ?? [])
        .map((u) => u.login)
        .filter((l) => !remove.includes(l));
      setIssueAssignees(hit.state, hit.issue, left, me);
      return response(200).json(hit.issue);
    },
  ),

  http.get("/repos/{owner}/{repo}/assignees", async ({ params, query, response }) => {
    const fault = await gate("issues-assignable");
    if (fault) return response.untyped(fault);
    if (!db.state(params.owner, params.repo)) return response.untyped(notFound());
    return response(200).json(page(assignable, query));
  }),

  // ---- Events and timeline ----

  http.get(
    "/repos/{owner}/{repo}/issues/{issue_number}/events",
    async ({ params, query, response }) => {
      const fault = await gate("issues-events");
      if (fault) return response.untyped(fault);
      const hit = find(params.owner, params.repo, params.issue_number);
      if (!hit) return response.untyped(notFound());
      return response(200).json(page(issueEvents(hit.state, hit.issue.number), query));
    },
  ),

  http.get(
    "/repos/{owner}/{repo}/issues/{issue_number}/timeline",
    async ({ params, query, response }) => {
      const fault = await gate("issues-timeline");
      if (fault) return response.untyped(fault);
      const hit = find(params.owner, params.repo, params.issue_number);
      if (!hit) return response.untyped(notFound());
      return response(200).json(page(timeline(hit.state, hit.issue.number), query));
    },
  ),

  // ---- Comments ----

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
      const hit = find(params.owner, params.repo, params.issue_number);
      if (!hit) return response.untyped(notFound());
      if (hit.issue.locked)
        return response.untyped(unprocessable("IssueComment", "issue", "locked"));
      const { body } = await request.json();
      if (!body.trim())
        return response.untyped(unprocessable("IssueComment", "body", "missing_field"));
      const created = db.createComment(hit.state, hit.issue.number, body);
      crossReference(hit.state, hit.issue, body, me);
      return response(201).json(created);
    },
  ),

  http.get("/repos/{owner}/{repo}/issues/comments/{comment_id}", async ({ params, response }) => {
    const fault = await gate("issues-comment");
    if (fault) return response.untyped(fault);
    const state = db.state(params.owner, params.repo);
    const hit = state && findComment(state, Number(params.comment_id));
    return hit ? response(200).json(hit.comment) : response.untyped(notFound());
  }),

  http.patch(
    "/repos/{owner}/{repo}/issues/comments/{comment_id}",
    async ({ params, request, response }) => {
      const fault = await gate("issues-comment-update");
      if (fault) return response.untyped(fault);
      const state = db.state(params.owner, params.repo);
      const hit = state && findComment(state, Number(params.comment_id));
      if (!state || !hit) return response.untyped(notFound());
      const { body } = await request.json();
      if (!body.trim())
        return response.untyped(unprocessable("IssueComment", "body", "missing_field"));
      hit.comment.body = body;
      hit.comment.updated_at = new Date().toISOString();
      const parent = state.issues.find((i) => i.number === hit.number);
      if (parent) crossReference(state, parent, body, me);
      return response(200).json(hit.comment);
    },
  ),

  http.delete(
    "/repos/{owner}/{repo}/issues/comments/{comment_id}",
    async ({ params, response }) => {
      const fault = await gate("issues-comment-delete");
      if (fault) return response.untyped(fault);
      const state = db.state(params.owner, params.repo);
      const hit = state && findComment(state, Number(params.comment_id));
      if (!state || !hit) return response.untyped(notFound());
      const left = hit.list.filter((c) => c !== hit.comment);
      state.comments.set(hit.number, left);
      for (const i of state.issues) if (i.number === hit.number) i.comments = left.length;
      for (const p of state.pulls) if (p.number === hit.number) p.comments = left.length;
      return response(204).empty();
    },
  ),

  http.put(
    "/repos/{owner}/{repo}/issues/comments/{comment_id}/pin",
    async ({ params, response }) => {
      const fault = await gate("issues-comment-pin");
      if (fault) return response.untyped(fault);
      const state = db.state(params.owner, params.repo);
      const hit = state && findComment(state, Number(params.comment_id));
      if (!state || !hit) return response.untyped(notFound());
      // One pinned comment per issue: pinning this one unpins the last.
      for (const c of hit.list) c.pin = null;
      hit.comment.pin = { pinned_at: new Date().toISOString(), pinned_by: me };
      const parent = state.issues.find((i) => i.number === hit.number);
      if (parent) parent.pinned_comment = hit.comment;
      return response(200).json(hit.comment);
    },
  ),

  http.delete(
    "/repos/{owner}/{repo}/issues/comments/{comment_id}/pin",
    async ({ params, response }) => {
      const fault = await gate("issues-comment-pin");
      if (fault) return response.untyped(fault);
      const state = db.state(params.owner, params.repo);
      const hit = state && findComment(state, Number(params.comment_id));
      if (!state || !hit) return response.untyped(notFound());
      hit.comment.pin = null;
      const parent = state.issues.find((i) => i.number === hit.number);
      if (parent) parent.pinned_comment = null;
      return response(204).empty();
    },
  ),

  // ---- Reactions, on an issue and on a comment ----

  ...(["issue", "comment"] as const).flatMap((kind) => {
    const list = async (owner: string, repo: string, raw: string, content: string | null) => {
      const state = db.state(owner, repo);
      const subject = state && subjectFor(state, kind, raw);
      if (!state || !subject) return undefined;
      return reactions(state, subject).filter((r) => !content || r.content === content);
    };
    const create = async (owner: string, repo: string, raw: string, content: string) => {
      const state = db.state(owner, repo);
      const subject = state && subjectFor(state, kind, raw);
      if (!state || !subject) return undefined;
      if (!isReaction(content)) return "invalid" as const;
      return react(state, subject, me, content);
    };
    const remove = (owner: string, repo: string, raw: string, reactionId: string) => {
      const state = db.state(owner, repo);
      const subject = state && subjectFor(state, kind, raw);
      return Boolean(state && subject && unreact(state, subject, Number(reactionId)));
    };
    if (kind === "issue") {
      return [
        http.get(
          "/repos/{owner}/{repo}/issues/{issue_number}/reactions",
          async ({ params, query, response }) => {
            const fault = await gate("issues-reactions");
            if (fault) return response.untyped(fault);
            const found = await list(
              params.owner,
              params.repo,
              params.issue_number,
              query.get("content"),
            );
            return found ? response(200).json(page(found, query)) : response.untyped(notFound());
          },
        ),
        http.post(
          "/repos/{owner}/{repo}/issues/{issue_number}/reactions",
          async ({ params, request, response }) => {
            const fault = await gate("issues-react");
            if (fault) return response.untyped(fault);
            const { content } = await request.json();
            const r = await create(params.owner, params.repo, params.issue_number, content);
            if (!r) return response.untyped(notFound());
            if (r === "invalid")
              return response.untyped(unprocessable("Reaction", "content", "invalid"));
            return r.created ? response(201).json(r.reaction) : response(200).json(r.reaction);
          },
        ),
        http.delete(
          "/repos/{owner}/{repo}/issues/{issue_number}/reactions/{reaction_id}",
          async ({ params, response }) => {
            const fault = await gate("issues-unreact");
            if (fault) return response.untyped(fault);
            return remove(params.owner, params.repo, params.issue_number, params.reaction_id)
              ? response(204).empty()
              : response.untyped(notFound());
          },
        ),
      ];
    }
    return [
      http.get(
        "/repos/{owner}/{repo}/issues/comments/{comment_id}/reactions",
        async ({ params, query, response }) => {
          const fault = await gate("issues-reactions");
          if (fault) return response.untyped(fault);
          const found = await list(
            params.owner,
            params.repo,
            params.comment_id,
            query.get("content"),
          );
          return found ? response(200).json(page(found, query)) : response.untyped(notFound());
        },
      ),
      http.post(
        "/repos/{owner}/{repo}/issues/comments/{comment_id}/reactions",
        async ({ params, request, response }) => {
          const fault = await gate("issues-react");
          if (fault) return response.untyped(fault);
          const { content } = await request.json();
          const r = await create(params.owner, params.repo, params.comment_id, content);
          if (!r) return response.untyped(notFound());
          if (r === "invalid")
            return response.untyped(unprocessable("Reaction", "content", "invalid"));
          return r.created ? response(201).json(r.reaction) : response(200).json(r.reaction);
        },
      ),
      http.delete(
        "/repos/{owner}/{repo}/issues/comments/{comment_id}/reactions/{reaction_id}",
        async ({ params, response }) => {
          const fault = await gate("issues-unreact");
          if (fault) return response.untyped(fault);
          return remove(params.owner, params.repo, params.comment_id, params.reaction_id)
            ? response(204).empty()
            : response.untyped(notFound());
        },
      ),
    ];
  }),

  // ---- Repository labels ----

  http.get("/repos/{owner}/{repo}/labels", async ({ params, query, response }) => {
    const fault = await gate("issues-labels");
    if (fault) return response.untyped(fault);
    const state = db.state(params.owner, params.repo);
    if (!state) return response.untyped(notFound());
    const sorted = [...extrasFor(state).labels].sort((a, b) => a.name.localeCompare(b.name));
    return response(200).json(page(sorted, query));
  }),

  http.post("/repos/{owner}/{repo}/labels", async ({ params, request, response }) => {
    const fault = await gate("issues-label-create");
    if (fault) return response.untyped(fault);
    const state = db.state(params.owner, params.repo);
    if (!state) return response.untyped(notFound());
    const body = await request.json();
    const name = body.name.trim();
    const color = (body.color ?? "ededed").replace(/^#/, "").toLowerCase();
    if (!name) return response.untyped(unprocessable("Label", "name", "missing_field"));
    if (!/^[0-9a-f]{6}$/.test(color))
      return response.untyped(unprocessable("Label", "color", "invalid"));
    const taken = extrasFor(state).labels.some((l) => l.name.toLowerCase() === name.toLowerCase());
    if (taken) return response.untyped(unprocessable("Label", "name", "already_exists"));
    return response(201).json(
      createLabel(state, { name, color, description: body.description ?? "" }),
    );
  }),

  http.get("/repos/{owner}/{repo}/labels/{name}", async ({ params, response }) => {
    const fault = await gate("issues-labels");
    if (fault) return response.untyped(fault);
    const state = db.state(params.owner, params.repo);
    const l = state && extrasFor(state).labels.find((x) => x.name === params.name);
    return l ? response(200).json(l) : response.untyped(notFound());
  }),

  http.patch("/repos/{owner}/{repo}/labels/{name}", async ({ params, request, response }) => {
    const fault = await gate("issues-label-update");
    if (fault) return response.untyped(fault);
    const state = db.state(params.owner, params.repo);
    const l = state && extrasFor(state).labels.find((x) => x.name === params.name);
    if (!state || !l) return response.untyped(notFound());
    const body = (await request.json()) ?? {};
    const name = body.new_name?.trim();
    const color = body.color?.replace(/^#/, "").toLowerCase();
    if (name !== undefined && !name)
      return response.untyped(unprocessable("Label", "name", "missing_field"));
    if (color !== undefined && !/^[0-9a-f]{6}$/.test(color)) {
      return response.untyped(unprocessable("Label", "color", "invalid"));
    }
    const clash =
      name &&
      extrasFor(state).labels.some((x) => x !== l && x.name.toLowerCase() === name.toLowerCase());
    if (clash) return response.untyped(unprocessable("Label", "name", "already_exists"));
    updateLabel(state, l, { name, color, description: body.description });
    return response(200).json(l);
  }),

  http.delete("/repos/{owner}/{repo}/labels/{name}", async ({ params, response }) => {
    const fault = await gate("issues-label-delete");
    if (fault) return response.untyped(fault);
    const state = db.state(params.owner, params.repo);
    const l = state && extrasFor(state).labels.find((x) => x.name === params.name);
    if (!state || !l) return response.untyped(notFound());
    deleteLabel(state, l);
    return response(204).empty();
  }),

  // ---- Milestones ----

  http.get("/repos/{owner}/{repo}/milestones", async ({ params, query, response }) => {
    const fault = await gate("issues-milestones");
    if (fault) return response.untyped(fault);
    const state = db.state(params.owner, params.repo);
    if (!state) return response.untyped(notFound());
    recountMilestones(state);
    const wanted = query.get("state") ?? "open";
    const dir = query.get("direction") === "desc" ? -1 : 1;
    const done = (m: Schemas["milestone"]) =>
      m.open_issues + m.closed_issues ? m.closed_issues / (m.open_issues + m.closed_issues) : 0;
    const list = extrasFor(state)
      .milestones.filter((m) => wanted === "all" || m.state === wanted)
      .sort((a, b) =>
        query.get("sort") === "completeness"
          ? dir * (done(a) - done(b))
          : dir * (a.due_on ?? "9999").localeCompare(b.due_on ?? "9999"),
      );
    return response(200).json(page(list, query));
  }),

  http.post("/repos/{owner}/{repo}/milestones", async ({ params, request, response }) => {
    const fault = await gate("issues-milestone-create");
    if (fault) return response.untyped(fault);
    const state = db.state(params.owner, params.repo);
    if (!state) return response.untyped(notFound());
    const body = await request.json();
    const title = body.title.trim();
    if (!title) return response.untyped(unprocessable("Milestone", "title", "missing_field"));
    const x = extrasFor(state);
    if (x.milestones.some((m) => m.title.toLowerCase() === title.toLowerCase())) {
      return response.untyped(unprocessable("Milestone", "title", "already_exists"));
    }
    const now = new Date().toISOString();
    const m = milestone(state, {
      number: Math.max(0, ...x.milestones.map((y) => y.number)) + 1,
      title,
      description: body.description || null,
      due: body.due_on ?? null,
      closed: body.state === "closed" ? now : undefined,
      created: now,
    });
    x.milestones.push(m);
    return response(201).json(m);
  }),

  http.get("/repos/{owner}/{repo}/milestones/{milestone_number}", async ({ params, response }) => {
    const fault = await gate("issues-milestone");
    if (fault) return response.untyped(fault);
    const state = db.state(params.owner, params.repo);
    if (!state) return response.untyped(notFound());
    recountMilestones(state);
    const m = extrasFor(state).milestones.find((y) => y.number === Number(params.milestone_number));
    return m ? response(200).json(m) : response.untyped(notFound());
  }),

  http.patch(
    "/repos/{owner}/{repo}/milestones/{milestone_number}",
    async ({ params, request, response }) => {
      const fault = await gate("issues-milestone-update");
      if (fault) return response.untyped(fault);
      const state = db.state(params.owner, params.repo);
      const x = state && extrasFor(state);
      const m = x?.milestones.find((y) => y.number === Number(params.milestone_number));
      if (!state || !x || !m) return response.untyped(notFound());
      const body = (await request.json()) ?? {};
      const title = body.title?.trim();
      if (title !== undefined && !title) {
        return response.untyped(unprocessable("Milestone", "title", "missing_field"));
      }
      if (
        title &&
        x.milestones.some((y) => y !== m && y.title.toLowerCase() === title.toLowerCase())
      ) {
        return response.untyped(unprocessable("Milestone", "title", "already_exists"));
      }
      const now = new Date().toISOString();
      if (title) m.title = title;
      if (body.description !== undefined) m.description = body.description || null;
      if (body.due_on !== undefined) m.due_on = body.due_on || null;
      if (body.state && body.state !== m.state) {
        m.state = body.state;
        m.closed_at = body.state === "closed" ? now : null;
      }
      m.updated_at = now;
      // Issues carry a copy of their milestone; keep the copies current.
      for (const i of state.issues) if (i.milestone?.number === m.number) i.milestone = m;
      recountMilestones(state);
      return response(200).json(m);
    },
  ),

  http.delete(
    "/repos/{owner}/{repo}/milestones/{milestone_number}",
    async ({ params, response }) => {
      const fault = await gate("issues-milestone-delete");
      if (fault) return response.untyped(fault);
      const state = db.state(params.owner, params.repo);
      const m =
        state &&
        extrasFor(state).milestones.find((y) => y.number === Number(params.milestone_number));
      if (!state || !m) return response.untyped(notFound());
      deleteMilestone(state, m);
      return response(204).empty();
    },
  ),

  http.get(
    "/repos/{owner}/{repo}/milestones/{milestone_number}/labels",
    async ({ params, query, response }) => {
      const fault = await gate("issues-labels");
      if (fault) return response.untyped(fault);
      const state = db.state(params.owner, params.repo);
      if (!state) return response.untyped(notFound());
      const n = Number(params.milestone_number);
      const names = new Set(
        state.issues.filter((i) => i.milestone?.number === n).flatMap((i) => labelNames(i)),
      );
      return response(200).json(
        page(
          extrasFor(state).labels.filter((l) => names.has(l.name)),
          query,
        ),
      );
    },
  ),

  // ---- Markdown ----

  http.post("/markdown", async ({ request, response }) => {
    const fault = await gate("markdown");
    if (fault) return response.untyped(fault);
    const { text } = await request.json();
    return response(200).text(markdown(text), {
      headers: {
        "Content-Type": "text/html;charset=utf-8",
        "X-CommonMarker-Version": "0.23.10",
      },
    });
  }),
];
