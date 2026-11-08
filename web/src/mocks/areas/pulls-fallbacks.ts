import { HttpResponse } from "msw";
import type { Schemas } from "@/api/client";
import { gate, http, notFound, page, unprocessable } from "../http";
import { API, HOST, orgMembers, teams } from "../world";
import { db, label, LABELS } from "./core-db";
import { deleteBranch, graph, minimalRepo, restoreBranch } from "./pulls-db";

// Minimal handlers for routes other areas own, until theirs land: the code area's branch
// list, refs, and combined status; the issues area's labels and assignees; repo-admin's
// teams. The pull request sidebar, compare pickers, and Checks tab read and write these.

const issueOf = (owner: string, repo: string, n: string) =>
  db.state(owner, repo)?.issues.find((i) => i.number === Number(n));

const STATUS_AT = "2026-11-08T10:15:00Z";
const status = (
  full: string,
  shaValue: string,
  context: string,
  state: string,
  description: string,
): Schemas["simple-commit-status"] => ({
  id: parseInt(shaValue.slice(0, 6), 16) + context.length,
  node_id: `SC_${shaValue.slice(0, 8)}${context.length}`,
  state,
  context,
  description,
  target_url: `${HOST}/${full}/commit/${shaValue}`,
  required: context.startsWith("ci/"),
  avatar_url: null,
  url: `${API}/repos/${full}/statuses/${shaValue}`,
  created_at: STATUS_AT,
  updated_at: STATUS_AT,
});

export const handlers = [
  http.get("/repos/{owner}/{repo}/labels", async ({ params, query, response }) => {
    const fault = await gate("issues-labels");
    if (fault) return response.untyped(fault);
    const state = db.state(params.owner, params.repo);
    if (!state) return response.untyped(notFound());
    return response(200).json(
      page(
        Object.keys(LABELS).map((n) => ({ ...label(state.repo, n), archived_at: null })),
        query,
      ),
    );
  }),

  http.get("/repos/{owner}/{repo}/assignees", async ({ params, query, response }) => {
    const fault = await gate("issues-assignees");
    if (fault) return response.untyped(fault);
    if (!db.state(params.owner, params.repo)) return response.untyped(notFound());
    return response(200).json(page(orgMembers, query));
  }),

  http.get("/repos/{owner}/{repo}/teams", async ({ params, query, response }) => {
    const fault = await gate("admin-repo-collaborators");
    if (fault) return response.untyped(fault);
    if (!db.state(params.owner, params.repo)) return response.untyped(notFound());
    return response(200).json(page(Object.values(teams), query));
  }),

  http.put(
    "/repos/{owner}/{repo}/issues/{issue_number}/labels",
    async ({ params, request, response }) => {
      const fault = await gate("issues-labels-write");
      if (fault) return response.untyped(fault);
      const state = db.state(params.owner, params.repo);
      const issue = issueOf(params.owner, params.repo, params.issue_number);
      if (!state || !issue) return response.untyped(notFound());
      const body = await request.json();
      const names = (
        body && typeof body === "object" && "labels" in body ? (body.labels ?? []) : []
      ).map((l) => (typeof l === "string" ? l : l.name));
      const unknown = names.find((n) => !(n in LABELS));
      if (unknown) return response.untyped(unprocessable("Label", "name", "invalid"));
      const labels = names.map((n) => ({ ...label(state.repo, n), archived_at: null }));
      issue.labels = labels;
      issue.updated_at = new Date().toISOString();
      return response(200).json(labels);
    },
  ),

  http.post(
    "/repos/{owner}/{repo}/issues/{issue_number}/assignees",
    async ({ params, request, response }) => {
      const fault = await gate("issues-assignees-write");
      if (fault) return response.untyped(fault);
      const issue = issueOf(params.owner, params.repo, params.issue_number);
      if (!issue) return response.untyped(notFound());
      const logins = ((await request.json())?.assignees ?? []).map((a) =>
        typeof a === "string" ? a : a.login,
      );
      const add = orgMembers.filter(
        (u) => logins.includes(u.login) && !issue.assignees?.some((a) => a.login === u.login),
      );
      issue.assignees = [...(issue.assignees ?? []), ...add];
      issue.assignee = issue.assignees[0] ?? null;
      issue.updated_at = new Date().toISOString();
      return response(201).json(issue);
    },
  ),

  http.delete(
    "/repos/{owner}/{repo}/issues/{issue_number}/assignees",
    async ({ params, request, response }) => {
      const fault = await gate("issues-assignees-write");
      if (fault) return response.untyped(fault);
      const issue = issueOf(params.owner, params.repo, params.issue_number);
      if (!issue) return response.untyped(notFound());
      const logins = (await request.json())?.assignees ?? [];
      issue.assignees = (issue.assignees ?? []).filter((a) => !logins.includes(a.login));
      issue.assignee = issue.assignees[0] ?? null;
      issue.updated_at = new Date().toISOString();
      return response(200).json(issue);
    },
  ),

  http.get("/repos/{owner}/{repo}/commits/{ref}/status", async ({ params, response }) => {
    const fault = await gate("code-status");
    if (fault) return response.untyped(fault);
    const state = db.state(params.owner, params.repo);
    if (!state) return response.untyped(notFound());
    const full = state.repo.full_name;
    const shaValue = graph(full).get(params.ref)?.history.at(-1)?.sha ?? params.ref;
    const pr = state.pulls.find((p) => p.head.sha === shaValue);
    const statuses =
      pr && pr.state === "open"
        ? [
            status(full, shaValue, "ci/wire-oracle", "success", "GHES 3.22 response shapes match"),
            status(
              full,
              shaValue,
              "deploy/preview",
              pr.draft ? "pending" : "success",
              pr.draft ? "Waiting for ready for review" : "Preview deployed",
            ),
          ]
        : [];
    const states = statuses.map((s) => s.state);
    const repo = state.repo;
    return response(200).json({
      state: states.includes("failure")
        ? "failure"
        : states.includes("pending") || !states.length
          ? "pending"
          : "success",
      statuses,
      sha: shaValue,
      total_count: statuses.length,
      repository: minimalRepo(repo),
      commit_url: `${API}/repos/${full}/commits/${shaValue}`,
      url: `${API}/repos/${full}/commits/${shaValue}/status`,
    });
  }),

  http.get("/repos/{owner}/{repo}/branches", async ({ params, query, response }) => {
    const fault = await gate("code-branches");
    if (fault) return response.untyped(fault);
    const state = db.state(params.owner, params.repo);
    if (!state) return response.untyped(notFound());
    const full = state.repo.full_name;
    const list = [...graph(full).values()].map((b) => {
      const tip = b.history.at(-1)?.sha ?? "";
      return {
        name: b.name,
        commit: { sha: tip, url: `${API}/repos/${full}/commits/${tip}` },
        protected: b.name === state.repo.default_branch,
      };
    });
    return response(200).json(page(list, query));
  }),

  http.delete("/repos/{owner}/{repo}/git/refs/{ref}", async ({ params, response }) => {
    const fault = await gate("code-delete-ref");
    if (fault) return response.untyped(fault);
    const state = db.state(params.owner, params.repo);
    if (!state) return response.untyped(notFound());
    const name = params.ref.replace(/^heads\//, "");
    if (name === state.repo.default_branch) return response(422).empty();
    return deleteBranch(state, name) ? response(204).empty() : response(422).empty();
  }),

  http.post("/repos/{owner}/{repo}/git/refs", async ({ params, request, response }) => {
    const fault = await gate("code-create-ref");
    if (fault) return response.untyped(fault);
    const state = db.state(params.owner, params.repo);
    if (!state) return response.untyped(notFound());
    const { ref, sha } = await request.json();
    const name = ref.replace(/^refs\/heads\//, "");
    if (graph(state.repo.full_name).has(name))
      return HttpResponse.json(
        {
          message: "Reference already exists",
          documentation_url: "",
          status: "422",
          errors: [{ resource: "Reference", field: "ref", code: "already_exists" }],
        },
        { status: 422 },
      );
    const branch = restoreBranch(state, name, sha);
    const tip = branch.history.at(-1)?.sha ?? sha;
    return response(201).json({
      ref: `refs/heads/${name}`,
      node_id: `REF_${tip.slice(0, 12)}`,
      url: `${API}/repos/${state.repo.full_name}/git/refs/heads/${name}`,
      object: {
        type: "commit",
        sha: tip,
        url: `${API}/repos/${state.repo.full_name}/git/commits/${tip}`,
      },
    });
  }),
];
